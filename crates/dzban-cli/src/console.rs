use std::io::{IsTerminal, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use dzban_core::execution::{QueryEngine, QueryPhase};

use crate::args::{Args, OutputFormat};
use crate::console_command::Command;
use crate::editor::{Editor, Effect, Input};
use crate::{CliError, dispatch, output};

struct Terminal;

impl Terminal {
    fn enter() -> Result<Self, CliError> {
        terminal::enable_raw_mode().map_err(io_error)?;
        if let Err(error) = crossterm::execute!(
            std::io::stdout(),
            event::EnableBracketedPaste,
            event::PushKeyboardEnhancementFlags(
                event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        ) {
            let _ = terminal::disable_raw_mode();
            return Err(io_error(error));
        }
        Ok(Self)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = crossterm::execute!(
            std::io::stdout(),
            event::PopKeyboardEnhancementFlags,
            event::DisableBracketedPaste
        );
        let _ = terminal::disable_raw_mode();
    }
}

fn io_error(error: std::io::Error) -> CliError {
    CliError::runtime(error.to_string())
}

pub async fn run(args: &Args) -> Result<(), CliError> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(CliError::usage(
            "The console requires a terminal; use dzb query for piped SQL",
        ));
    }
    let command_context = crate::context::CommandContext::load(args)?;
    let crate::context::OutputPolicy::Console { pager, enabled } = &command_context.output else {
        return Err(CliError::runtime("Console output policy is unavailable"));
    };
    let pager = pager.value.as_deref();
    let config = &command_context.config;
    let mut profile = if args.connection.is_some() {
        command_context.profile()?
    } else {
        let mut profile = pick_connection(&config.databases).await?;
        if let Some(database) = &args.database {
            profile.database.clone_from(database);
        }
        profile
    };
    let context = command_context.application();
    let mut engine = context
        .connect_profile(profile.clone())
        .await
        .map_err(|error| CliError::usage(error.to_string()))?;
    println!(
        "Connected to {}. End SQL with ; then Enter. \\g submits without ;. \\? for help.",
        profile.name
    );
    let mut terminal_guard = Some(Terminal::enter()?);
    let mut editor = Editor::default();
    editor.set_dialect(statement_dialect(profile.backend));
    let mut renderer = crate::editor_view::Renderer::default();
    let mut query: Option<(String, dzban_core::execution::QueryHandle, OutputFormat)> = None;
    let mut format = OutputFormat::Table;
    let mut paging = *enabled;
    let mut timing = false;
    let mut last_sql = String::new();
    let mut last_error = None;
    let mut cancellation: Option<
        tokio::task::JoinHandle<Result<(), dzban_core::error::ExecutionError>>,
    > = None;
    loop {
        if cancellation.as_ref().is_some_and(|task| task.is_finished()) {
            let result = cancellation
                .take()
                .expect("finished cancellation exists")
                .await;
            let result = result.unwrap_or_else(|error| {
                Err(dzban_core::error::ExecutionError::Message(
                    error.to_string(),
                ))
            });
            if let Err(error) = result {
                print!("{}", renderer.clear());
                drop(terminal_guard.take());
                println!();
                eprintln!("dzb: {error}");
                last_error = Some(error);
                terminal_guard = Some(Terminal::enter()?);
            }
        }
        render_editor(&editor, &engine, &profile.name, &mut renderer)?;
        if query
            .as_ref()
            .is_some_and(|(_, task, _)| task.is_finished())
        {
            let (_, task, query_format) = query.take().expect("finished query exists");
            let completion = task.wait_completion().await;
            print!("{}", renderer.clear());
            drop(terminal_guard.take());
            println!();
            let (result, terminal) = match completion {
                Ok(completion) => {
                    dispatch::report_history(&completion);
                    (completion.result, Some(completion.snapshot))
                }
                Err(error) => (Err(error), None),
            };
            match result {
                Ok(result) => show_output(
                    &output::render_query(&result, query_format)?,
                    pager,
                    !paging,
                )?,
                Err(error) => {
                    eprintln!("dzb: {error}");
                    last_error = Some(error);
                }
            }
            if timing && let Some(state) = terminal {
                println!(
                    "{}",
                    output::timing(
                        state.elapsed(),
                        state.planning_duration,
                        state.execution_duration
                    )
                );
            }
            terminal_guard = Some(Terminal::enter()?);
            continue;
        }
        if !event::poll(Duration::ZERO).map_err(io_error)? {
            tokio::time::sleep(Duration::from_millis(30)).await;
            continue;
        }
        let input = match event::read().map_err(io_error)? {
            Event::Paste(text) => Some(Input::Text(text)),
            Event::Key(key) => match crate::keyboard::action(key, &editor) {
                crate::keyboard::Action::Clear => {
                    print!("{}\x1b[2J\x1b[H", renderer.clear());
                    None
                }
                crate::keyboard::Action::History(previous) => {
                    let history = dzban_core::history::QueryHistoryStore::load(&profile.name)
                        .map_err(CliError::runtime)?;
                    let entries = history
                        .entries()
                        .iter()
                        .map(|entry| entry.sql.clone())
                        .collect::<Vec<_>>();
                    editor.recall(&entries, previous);
                    None
                }
                crate::keyboard::Action::SearchHistory => {
                    print!("{}", renderer.clear());
                    let history = dzban_core::history::QueryHistoryStore::load(&profile.name)
                        .map_err(CliError::runtime)?;
                    let choices = history
                        .entries()
                        .iter()
                        .map(|entry| entry.sql.replace(['\n', '\r'], " "))
                        .collect::<Vec<_>>();
                    drop(terminal_guard.take());
                    let selection = pick_items(&choices, "History search").await;
                    terminal_guard = Some(Terminal::enter()?);
                    match selection {
                        Ok(index) => {
                            editor.apply(Input::Interrupt, false);
                            editor.apply(Input::Text(history.entries()[index].sql.clone()), false);
                            None
                        }
                        Err(error) if error.code == 130 => Some(Input::Escape),
                        Err(error) => return Err(error),
                    }
                }
                crate::keyboard::Action::EndOfInput
                    if query.is_none() && editor.text().is_empty() =>
                {
                    break;
                }
                crate::keyboard::Action::EndOfInput => Some(Input::Delete),
                crate::keyboard::Action::ClientCommand => {
                    let (command, draft) = editor.client_command().expect("Client command present");
                    let command = command.to_owned();
                    let draft = draft.to_owned();
                    print!("{}", renderer.clear());
                    drop(terminal_guard.take());
                    println!();
                    let mut next_buffer = Some(draft.clone());
                    match Command::parse(&command) {
                        Ok(Command::Quit) if query.is_none() => break,
                        Ok(Command::Quit) => eprintln!("Cancel the running query before quitting"),
                        Ok(Command::Help) => {
                            let registry = engine
                                .action_registry()
                                .map_err(|error| CliError::runtime(error.to_string()))?;
                            print!(
                                "{}",
                                crate::console_command::help(&registry, &engine.capabilities())
                            );
                        }
                        Ok(Command::Cancel) => {
                            if let Some((_, task, _)) = &query {
                                if cancellation.is_none() {
                                    let engine = engine.clone();
                                    let id = task.id;
                                    cancellation = Some(tokio::spawn(async move {
                                        engine.cancel_detailed(id).await
                                    }));
                                }
                            } else {
                                println!("No query is running");
                            }
                        }
                        Ok(Command::Clear) => next_buffer = None,
                        Ok(Command::Write(path)) => {
                            let sql = if draft.is_empty() { &last_sql } else { &draft };
                            match crate::external_editor::save_buffer(&path, sql) {
                                Ok(()) => println!("Saved query buffer"),
                                Err(error) => eprintln!("dzb: {error}; keeping original SQL"),
                            }
                        }
                        Ok(Command::Edit) => {
                            let original = if draft.is_empty() {
                                last_sql.clone()
                            } else {
                                draft.clone()
                            };
                            next_buffer = Some(
                                match crate::external_editor::edit(
                                    &crate::external_editor::configured_editor(),
                                    &original,
                                ) {
                                    Ok(edited) => edited,
                                    Err(error) => {
                                        eprintln!("dzb: {error}; keeping original SQL");
                                        original
                                    }
                                },
                            );
                        }
                        Ok(Command::Connect(_)) if query.is_some() => {
                            eprintln!("Cancel the running query before switching connections");
                        }
                        Ok(Command::Connect(name)) => {
                            let candidate = match name {
                                Some(name) => config
                                    .resolve_profile(&dzban_core::ConnectionProfileId(name), None)
                                    .map_err(|error| CliError::usage(error.to_string())),
                                None => pick_connection(&config.databases).await,
                            };
                            match candidate {
                                Ok(candidate) => match context
                                    .connect_profile(candidate.clone())
                                    .await
                                {
                                    Ok(connected) => match context.close(engine.id()).await {
                                        Ok(()) => {
                                            engine = connected;
                                            profile = candidate;
                                            editor.set_dialect(statement_dialect(profile.backend));
                                            last_sql.clear();
                                            last_error = None;
                                            println!("Connected to {}", profile.name);
                                        }
                                        Err(error) => {
                                            if let Err(cleanup_error) =
                                                context.close(connected.id()).await
                                            {
                                                eprintln!(
                                                    "dzb: could not close candidate session: {cleanup_error}"
                                                );
                                            }
                                            eprintln!("dzb: {error}; keeping current connection");
                                        }
                                    },
                                    Err(error) => {
                                        eprintln!("dzb: {}; keeping current connection", error)
                                    }
                                },
                                Err(error) if error.code == 130 => {}
                                Err(error) => eprintln!("dzb: {}", error.message),
                            }
                        }
                        Ok(Command::ConnectionInfo) => {
                            println!("{}", output::connection_summary(&profile))
                        }
                        Ok(
                            command @ (Command::Tables
                            | Command::Views
                            | Command::Catalog(_)
                            | Command::Describe(_, _)),
                        ) => {
                            let result = match command {
                                Command::Describe(name, detail) => {
                                    engine.describe_named(name, detail).await
                                }
                                _ => {
                                    engine
                                        .catalog(
                                            command
                                                .catalog_request()
                                                .expect("Catalog command has a request"),
                                        )
                                        .await
                                }
                            };
                            match result {
                                Ok(entries) => {
                                    let result = output::catalog(entries);
                                    show_output(&output::render(&result, format)?, pager, !paging)?;
                                }
                                Err(error) => eprintln!("dzb: {error}"),
                            }
                        }
                        Ok(Command::Connections) => {
                            for profile in &config.databases {
                                println!("{}", output::connection_summary(profile));
                            }
                        }
                        Ok(Command::LastSql) => {
                            println!("{}", if draft.is_empty() { &last_sql } else { &draft })
                        }
                        Ok(Command::ErrorDetails) => {
                            let text = match &last_error {
                                Some(error) => serde_json::to_string_pretty(error)
                                    .map_err(|error| CliError::runtime(error.to_string()))?,
                                None => "No query error available".into(),
                            };
                            show_output(&format!("{text}\n"), pager, !paging)?;
                        }
                        Ok(Command::Plan) => {
                            let snapshot = engine.subscribe();
                            let state = snapshot.borrow().clone();
                            if let Some(plan) = &state.plan {
                                let document = serde_json::to_string_pretty(plan)
                                    .map_err(|error| CliError::runtime(error.to_string()))?;
                                show_output(&format!("{document}\n"), pager, !paging)?;
                            } else {
                                println!(
                                    "{}",
                                    state
                                        .explain_error
                                        .as_deref()
                                        .unwrap_or("No plan available")
                                );
                            }
                        }
                        Ok(Command::History) => {
                            let history =
                                dzban_core::history::QueryHistoryStore::load(&profile.name)
                                    .map_err(CliError::runtime)?;
                            for (index, entry) in history.entries().iter().enumerate() {
                                println!("{}  {}", index + 1, entry.sql.replace(['\n', '\r'], " "));
                            }
                        }
                        Ok(Command::Expanded) => {
                            format = match format {
                                OutputFormat::Expanded => OutputFormat::Table,
                                _ => OutputFormat::Expanded,
                            }
                        }
                        Ok(Command::Pager(value)) => {
                            if let Some(value) = value {
                                paging = value;
                            }
                            println!("Pager {}", if paging { "on" } else { "off" });
                        }
                        Ok(Command::Timing(value)) => {
                            timing = value.unwrap_or(!timing);
                            println!("Timing {}", if timing { "on" } else { "off" });
                        }
                        Err(error) => eprintln!("{error}"),
                    }
                    terminal_guard = Some(Terminal::enter()?);
                    editor.apply(Input::Interrupt, false);
                    if let Some(text) = next_buffer {
                        editor.apply(Input::Text(text), false);
                    }
                    None
                }
                crate::keyboard::Action::Edit(input) => Some(input),
                crate::keyboard::Action::Ignore => None,
            },
            _ => None,
        };
        if let Some(input) = input {
            let effect = editor.apply(input, query.is_some());
            let query_format = if matches!(&effect, Effect::SubmitExpanded(_)) {
                OutputFormat::Expanded
            } else {
                format
            };
            match effect {
                Effect::Submit(sql) | Effect::SubmitExpanded(sql) => {
                    last_sql.clone_from(&sql);
                    let task = dispatch::start_query(&engine, &profile.name, sql.clone())?;
                    query = Some((sql, task, query_format));
                }
                Effect::Cancel => {
                    let snapshot = engine.subscribe();
                    let id = match snapshot.borrow().phase {
                        QueryPhase::Queued(id)
                        | QueryPhase::Explaining(id)
                        | QueryPhase::Executing(id) => Some(id),
                        _ => None,
                    };
                    if let Some(id) = id
                        && cancellation.is_none()
                    {
                        let engine = engine.clone();
                        cancellation =
                            Some(tokio::spawn(
                                async move { engine.cancel_detailed(id).await },
                            ));
                    }
                }
                Effect::Edit | Effect::Busy => {}
            }
        }
    }
    print!("{}", renderer.clear());
    drop(terminal_guard);
    println!();
    context
        .close(engine.id())
        .await
        .map_err(|error| CliError::runtime(error.to_string()))?;
    Ok(())
}

fn statement_dialect(backend: dzban_core::DatabaseBackend) -> crate::statement::Dialect {
    match backend {
        dzban_core::DatabaseBackend::Postgresql => crate::statement::Dialect::Postgresql,
        dzban_core::DatabaseBackend::Clickhouse => crate::statement::Dialect::Clickhouse,
    }
}

async fn pick_connection(
    profiles: &[dzban_core::DatabaseConfig],
) -> Result<dzban_core::DatabaseConfig, CliError> {
    if profiles.is_empty() {
        return Err(CliError::usage(
            "No connection profiles configured; use dzb info config",
        ));
    }
    let choices = profiles
        .iter()
        .map(|profile| {
            format!(
                "{} | {:?} | {} | {}",
                profile.name,
                profile.backend,
                profile.database,
                profile.host.as_deref().unwrap_or("HTTP/local")
            )
        })
        .collect::<Vec<_>>();
    let index = pick_items(&choices, "Select connection").await?;
    Ok(profiles[index].clone())
}

async fn pick_items(choices: &[String], title: &str) -> Result<usize, CliError> {
    let _terminal = Terminal::enter()?;
    let mut picker = crate::picker::Picker::default();
    loop {
        let (width, height) = crossterm::terminal::size().map_err(io_error)?;
        let width = usize::from(width).saturating_sub(1);
        let header = crate::editor_view::single_line(
            &format!("{title} (type to search, arrows, Enter): {}", picker.search),
            width,
        );
        print!("\r\x1b[2K{header}\r\n");
        let matches = picker.visible_items(choices, usize::from(height).saturating_sub(2));
        let selected = picker.selected_item(choices);
        for index in &matches {
            print!(
                "\x1b[2K{} {}\r\n",
                if selected == Some(*index) { ">" } else { " " },
                crate::editor_view::single_line(&choices[*index], width.saturating_sub(2))
            );
        }
        if matches.is_empty() {
            print!(
                "\x1b[2K{}\r\n",
                crate::editor_view::single_line("No matching items", width)
            );
        }
        std::io::stdout().flush().map_err(io_error)?;
        let event = loop {
            if event::poll(Duration::ZERO).map_err(io_error)? {
                break event::read().map_err(io_error)?;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        };
        let lines = matches.len().max(1) + 1;
        print!("\x1b[{lines}A\r\x1b[J");
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => {
                    if let Some(index) = selected {
                        return Ok(index);
                    }
                }
                KeyCode::Esc => {
                    return Err(CliError {
                        message: format!("{title} canceled"),
                        code: 130,
                    });
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err(CliError {
                        message: format!("{title} canceled"),
                        code: 130,
                    });
                }
                KeyCode::Up => picker.move_item(-1, choices),
                KeyCode::Down => picker.move_item(1, choices),
                KeyCode::Backspace => picker.backspace(),
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    picker.search(&ch.to_string())
                }
                _ => {}
            },
            Event::Paste(text) => picker.search(&text),
            _ => {}
        }
    }
}

fn render_editor(
    editor: &Editor,
    engine: &QueryEngine,
    profile: &str,
    renderer: &mut crate::editor_view::Renderer,
) -> Result<(), CliError> {
    let snapshot = engine.subscribe();
    let state = snapshot.borrow();
    let phase = match state.phase {
        QueryPhase::Queued(_) => "queued",
        QueryPhase::Explaining(_) => "estimating",
        QueryPhase::Executing(_) => "running",
        QueryPhase::Canceling(_) => "stopping",
        _ => "ready",
    };
    let estimate = match state.phase {
        QueryPhase::Executing(_) | QueryPhase::Canceling(_) => state
            .plan
            .as_ref()
            .map(output::estimate)
            .filter(|text| !text.is_empty())
            .map(|text| format!(" | {text}")),
        _ => None,
    }
    .unwrap_or_default();
    let (width, height) = terminal::size().map_err(io_error)?;
    let elapsed = match state.phase {
        QueryPhase::Queued(_)
        | QueryPhase::Explaining(_)
        | QueryPhase::Executing(_)
        | QueryPhase::Canceling(_) => format!(" {:.1}s", state.elapsed().as_secs_f64()),
        _ => String::new(),
    };
    let layout = crate::editor_view::layout(
        editor,
        &format!("{profile} [{phase}{elapsed}]{estimate}"),
        width as usize,
        height as usize,
    );
    print!("{}", renderer.paint(&layout));
    std::io::stdout().flush().map_err(io_error)
}

fn show_output(text: &str, command: Option<&str>, disabled: bool) -> Result<(), CliError> {
    if !disabled && let Some(command) = command {
        match crate::pager::run(command, text) {
            Ok(()) => return Ok(()),
            Err(error) => eprintln!("dzb: {error}; displaying without pager"),
        }
    }
    output::write(text)
}
