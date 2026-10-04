use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use dzban_core::application::ApplicationContext;
use dzban_core::{Config, ConnectionProfileId, DatabaseConfig};

use crate::CliError;
use crate::args::{Args, Command, ConfigSource, OutputFormat};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionSource {
    Default,
    Environment,
    FileDiscovery,
    CommandLine,
}

pub struct Resolved<T> {
    pub value: T,
    pub source: OptionSource,
}

pub enum OutputPolicy {
    Batch(Resolved<OutputFormat>),
    Console {
        pager: Resolved<Option<String>>,
        enabled: bool,
    },
    Diagnostic,
}

pub struct CommandContext {
    pub config: Config,
    pub config_path: Resolved<Option<PathBuf>>,
    pub connection: Resolved<Option<String>>,
    pub database: Resolved<Option<String>>,
    pub output: OutputPolicy,
    application: OnceLock<Arc<ApplicationContext>>,
}

impl CommandContext {
    pub fn load(args: &Args) -> Result<Self, CliError> {
        let path = args.config.clone().or_else(Config::discovered_path);
        let config = match &path {
            Some(path) => Config::load_from_path(path),
            None => Ok(Config {
                databases: Vec::new(),
            }),
        }
        .map_err(CliError::usage)?;
        let source = match args.config_source {
            ConfigSource::CommandLine => OptionSource::CommandLine,
            ConfigSource::Environment => OptionSource::Environment,
            ConfigSource::Discovery if path.is_some() => OptionSource::FileDiscovery,
            ConfigSource::Discovery => OptionSource::Default,
        };
        let explicit = |value: &Option<String>| Resolved {
            value: value.clone(),
            source: if value.is_some() {
                OptionSource::CommandLine
            } else {
                OptionSource::Default
            },
        };
        let output = match &args.command {
            Some(
                Command::Query { format, .. }
                | Command::List { format, .. }
                | Command::Describe { format, .. },
            ) => OutputPolicy::Batch(Resolved {
                value: *format,
                source: if args.format_source == Some(clap::parser::ValueSource::CommandLine) {
                    OptionSource::CommandLine
                } else {
                    OptionSource::Default
                },
            }),
            Some(Command::Console { pager, no_pager }) => OutputPolicy::Console {
                pager: resolve_pager(
                    pager.clone(),
                    std::env::var("DZBAN_PAGER").ok(),
                    std::env::var("PAGER").ok(),
                ),
                enabled: !no_pager,
            },
            _ => OutputPolicy::Diagnostic,
        };
        Ok(Self {
            config,
            config_path: Resolved {
                value: path,
                source,
            },
            connection: explicit(&args.connection),
            database: explicit(&args.database),
            output,
            application: OnceLock::new(),
        })
    }

    pub fn profile(&self) -> Result<DatabaseConfig, CliError> {
        let name =
            self.connection.value.as_ref().ok_or_else(|| {
                CliError::usage("Select a configured profile with --connection NAME")
            })?;
        self.config
            .resolve_profile(
                &ConnectionProfileId(name.clone()),
                self.database.value.as_deref(),
            )
            .map_err(|error| CliError::usage(error.to_string()))
    }

    pub fn application(&self) -> Arc<ApplicationContext> {
        self.application
            .get_or_init(crate::dispatch::application_context)
            .clone()
    }
}

pub fn resolve_pager(
    explicit: Option<String>,
    dzban_pager: Option<String>,
    pager: Option<String>,
) -> Resolved<Option<String>> {
    let (value, source) = if let Some(command) = explicit {
        (Some(command), OptionSource::CommandLine)
    } else if let Some(command) = dzban_pager.or(pager) {
        (Some(command), OptionSource::Environment)
    } else {
        (None, OptionSource::Default)
    };
    Resolved {
        value: value.filter(|command| !command.trim().is_empty()),
        source,
    }
}
