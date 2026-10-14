use std::io::{IsTerminal, Read};
use std::sync::Arc;
use std::time::Instant;

use dzban_core::execution::StreamedQueryResult;
use dzban_core::execution::{QueryEngine, QueryPhase};
use dzban_core::history::QueryHistoryStore;

use crate::args::{Args, Command, InfoCommand};
use crate::context::{OptionSource, OutputPolicy};
use crate::{CliError, output};

pub async fn run(args: Args) -> Result<(), CliError> {
    match &args.command {
        None => {
            Args::grouped_command()
                .print_help()
                .map_err(|error| CliError::runtime(error.to_string()))?;
            println!();
            Ok(())
        }
        Some(Command::Help { command }) => {
            let mut root = Args::grouped_command();
            let help = match command {
                Some(name) => root
                    .find_subcommand_mut(name)
                    .ok_or_else(|| CliError::usage(format!("Unknown command: {name}")))?,
                None => &mut root,
            };
            help.print_help()
                .map_err(|error| CliError::runtime(error.to_string()))?;
            println!();
            Ok(())
        }
        Some(Command::Version) => {
            println!("dzb {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(Command::Info { command }) => {
            let context = crate::context::CommandContext::load(&args)?;
            if matches!(command, InfoCommand::Config) {
                let source = context
                    .config_path
                    .value
                    .clone()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "empty defaults (no file found)".into());
                let origin = match context.config_path.source {
                    OptionSource::Default | OptionSource::FileDiscovery => "discovery",
                    OptionSource::Environment => "DZBAN_CONFIG",
                    OptionSource::CommandLine => "--config",
                };
                println!("Configuration: {source} (source: {origin})");
                if let Some(connection) = &context.connection.value {
                    println!("Profile override: {connection} (source: --connection)");
                }
                if let Some(database) = &context.database.value {
                    println!("Database override: {database} (source: --database, session only)");
                }
            }
            for profile in context.config.databases {
                println!("{}", output::connection_summary(&profile));
            }
            Ok(())
        }
        Some(Command::Query { sql, file, .. }) => {
            let command_context = crate::context::CommandContext::load(&args)?;
            let OutputPolicy::Batch(format) = &command_context.output else {
                return Err(CliError::runtime("Query output policy is unavailable"));
            };
            let profile = command_context.profile()?;
            let sql = match (sql, file) {
                (Some(sql), _) => sql.clone(),
                (_, Some(path)) => std::fs::read_to_string(path)
                    .map_err(|error| CliError::usage(error.to_string()))?,
                _ => {
                    if std::io::stdin().is_terminal() {
                        return Err(CliError::usage("Provide SQL, --file, or piped input"));
                    }
                    let mut sql = String::new();
                    std::io::stdin()
                        .read_to_string(&mut sql)
                        .map_err(|error| CliError::runtime(error.to_string()))?;
                    sql
                }
            };
            if sql.trim().is_empty() {
                return Err(CliError::usage("SQL input is empty"));
            }
            let context = command_context.application();
            let engine = context
                .connect_profile(profile.clone())
                .await
                .map_err(|error| CliError::usage(error.to_string()))?;
            let session = engine.id();
            let result = execute(engine, &profile.name, &sql).await;
            let result = finish_session(&context, session, result).await?;
            crate::streamed_output::write_stdout(&result, format.value).await
        }
        Some(command @ (Command::List { .. } | Command::Describe { .. })) => {
            let command_context = crate::context::CommandContext::load(&args)?;
            let OutputPolicy::Batch(format) = &command_context.output else {
                return Err(CliError::runtime("Catalog output policy is unavailable"));
            };
            let context = command_context.application();
            let engine = context
                .connect_profile(command_context.profile()?)
                .await
                .map_err(|error| CliError::usage(error.to_string()))?;
            let result = match command {
                Command::List { objects, .. } => engine.catalog(objects.request()).await,
                Command::Describe { object, .. } => {
                    let (kind, selection) = object.selection();
                    engine
                        .describe_kind(
                            selection.name.clone(),
                            kind,
                            if selection.full {
                                dzban_core::catalog::Detail::Full
                            } else {
                                dzban_core::catalog::Detail::Summary
                            },
                        )
                        .await
                }
                _ => unreachable!(),
            }
            .map_err(|error| CliError::runtime(error.to_string()));
            let entries = finish_session(&context, engine.id(), result).await?;
            output::write(&output::render(&output::catalog(entries), format.value)?)
        }
        Some(Command::Console { .. }) => crate::console::run(&args).await,
    }
}

pub fn application_context() -> Arc<dzban_core::application::ApplicationContext> {
    Arc::new(dzban_core::application::ApplicationContext::new(
        dzban_db::adapters::registry(),
    ))
}

pub async fn finish_session<T>(
    context: &Arc<dzban_core::application::ApplicationContext>,
    session: dzban_core::execution::SessionId,
    result: Result<T, CliError>,
) -> Result<T, CliError> {
    let closed = context.close(session).await;
    match (result, closed) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(CliError::runtime(error.to_string())),
        (Err(error), Err(cleanup_error)) => {
            eprintln!("dzb: could not close session: {cleanup_error}");
            Err(error)
        }
    }
}

pub async fn execute(
    engine: Arc<QueryEngine>,
    profile: &str,
    sql: &str,
) -> Result<StreamedQueryResult, CliError> {
    let mut snapshot = engine.subscribe();
    let query = start_query(&engine, profile, sql.to_owned())?.wait_completion();
    tokio::pin!(query);
    let start = Instant::now();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    let mut cancellation: Option<
        tokio::task::JoinHandle<Result<(), dzban_core::error::ExecutionError>>,
    > = None;
    let result = loop {
        tokio::select! {
            result = &mut query => break result,
            result = async { cancellation.as_mut().expect("pending cancellation exists").await }, if cancellation.is_some() => {
                cancellation = None;
                report_cancellation(result);
            }
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(|error| CliError::runtime(error.to_string()))?;
                let id = match snapshot.borrow().phase { QueryPhase::Queued(id) | QueryPhase::Explaining(id) | QueryPhase::Executing(id) => Some(id), _ => None };
                if let Some(id) = id && cancellation.is_none() {
                    let engine = engine.clone();
                    cancellation = Some(tokio::spawn(async move { engine.cancel_detailed(id).await }));
                }
            }
            _ = snapshot.changed() => {},
            _ = tick.tick(), if std::io::stderr().is_terminal() => {
                let state = snapshot.borrow();
                let label = match state.phase { QueryPhase::Explaining(_) => "Estimating", QueryPhase::Canceling(_) => "Stopping", _ => "Running" };
                let estimate = state.plan.as_ref().map(output::estimate).filter(|text| !text.is_empty()).map(|text| format!(" | {text}")).unwrap_or_default();
                let capture = state.capture.as_ref().map(crate::streamed_output::progress).filter(|text| !text.is_empty()).map(|text| format!(" | {text}")).unwrap_or_default();
                eprint!("\r\x1b[2K{label} {:.1}s{estimate}{capture}", start.elapsed().as_secs_f64());
            }
        }
    };
    if let Some(cancellation) = cancellation {
        report_cancellation(cancellation.await);
    }
    if std::io::stderr().is_terminal() {
        eprint!("\r\x1b[2K");
    }
    let result = match result {
        Ok(completion) => {
            report_history(&completion);
            completion.result
        }
        Err(error) => Err(error),
    };
    result.map_err(|error| CliError {
        message: error.to_string(),
        code: if matches!(error, dzban_core::error::ExecutionError::Canceled) {
            130
        } else {
            1
        },
    })
}

fn report_cancellation(
    result: Result<Result<(), dzban_core::error::ExecutionError>, tokio::task::JoinError>,
) {
    let result = result.unwrap_or_else(|error| {
        Err(dzban_core::error::ExecutionError::Message(
            error.to_string(),
        ))
    });
    if let Err(error) = result {
        if std::io::stderr().is_terminal() {
            eprint!("\r\x1b[2K");
        }
        eprintln!("dzb: cancellation failed: {error}");
    }
}

pub fn start_query(
    engine: &Arc<QueryEngine>,
    profile: &str,
    sql: String,
) -> Result<dzban_core::execution::StreamedQueryHandle, CliError> {
    let query = match QueryHistoryStore::load(profile) {
        Ok(history) => engine.start_streamed(sql, Default::default(), Some(history)),
        Err(error) => {
            eprintln!("dzb: history unavailable: {error}");
            engine.start_streamed(sql, Default::default(), None)
        }
    };
    query.map_err(|error| CliError::runtime(error.to_string()))
}

pub fn report_history(completion: &dzban_core::execution::StreamedQueryCompletion) {
    if let Some(error) = completion.snapshot.phase.id().and_then(|id| {
        completion
            .history
            .as_ref()
            .and_then(|history| history.error_for(id))
    }) {
        eprintln!("dzb: could not save query history: {error}");
    }
}
