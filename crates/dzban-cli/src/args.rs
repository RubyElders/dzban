use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "dzb",
    version,
    about = "Dzban SQL console",
    disable_help_subcommand = true
)]
pub struct Args {
    #[arg(skip)]
    pub config_source: ConfigSource,
    #[arg(skip)]
    pub format_source: Option<clap::parser::ValueSource>,
    #[arg(long, global = true, env = "DZBAN_CONFIG")]
    pub config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub connection: Option<String>,
    #[arg(long, global = true)]
    pub database: Option<String>,
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Default, Debug, Clone, Copy)]
pub enum ConfigSource {
    #[default]
    Discovery,
    Environment,
    CommandLine,
}

impl Args {
    pub fn from_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        use clap::FromArgMatches;
        let mut args = Self::from_arg_matches(matches)?;
        args.config_source = match matches.value_source("config") {
            Some(clap::parser::ValueSource::CommandLine) => ConfigSource::CommandLine,
            Some(clap::parser::ValueSource::EnvVariable) => ConfigSource::Environment,
            _ => ConfigSource::Discovery,
        };
        args.format_source = matches
            .subcommand()
            .filter(|(name, _)| matches!(*name, "query" | "list" | "describe"))
            .and_then(|(_, matches)| matches.value_source("format"));
        Ok(args)
    }

    pub fn grouped_command() -> clap::Command {
        let mut command = Self::command();
        let original = command.render_help().to_string();
        let mut groups: Vec<(&str, Vec<&clap::Command>)> = Vec::new();
        for child in command.get_subcommands() {
            let category = child.get_next_help_heading().unwrap_or("Commands");
            if let Some((_, children)) = groups.iter_mut().find(|(heading, _)| *heading == category)
            {
                children.push(child);
            } else {
                groups.push((category, vec![child]));
            }
        }
        let mut grouped = String::new();
        for (category, children) in groups {
            grouped.push_str(&format!("{category}:\n"));
            for child in children {
                let aliases = child.get_visible_aliases().collect::<Vec<_>>().join(", ");
                grouped.push_str(&format!(
                    "  {:8} {} [aliases: {aliases}]\n",
                    child.get_name(),
                    child
                        .get_about()
                        .map(|text| text.to_string())
                        .unwrap_or_default()
                ));
            }
            grouped.push('\n');
        }
        let help = original
            .split_once("Commands:\n")
            .and_then(|(prefix, rest)| {
                rest.split_once("Options:\n")
                    .map(|(_, options)| format!("{prefix}{grouped}Options:\n{options}"))
            });
        if let Some(help) = help {
            command = command.override_help(help);
        }
        command
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(
        visible_alias = "c",
        next_help_heading = "Workflow",
        about = "Open the interactive SQL console"
    )]
    Console {
        #[arg(long, conflicts_with = "no_pager")]
        pager: Option<String>,
        #[arg(long)]
        no_pager: bool,
    },
    #[command(
        visible_alias = "q",
        next_help_heading = "Workflow",
        about = "Execute SQL without entering the console"
    )]
    Query {
        #[arg(conflicts_with = "file")]
        sql: Option<String>,
        #[arg(short, long)]
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    #[command(
        visible_alias = "ls",
        next_help_heading = "Catalog",
        about = "List backend catalog objects"
    )]
    List {
        #[command(subcommand)]
        objects: CatalogList,
        #[arg(long, global = true, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    #[command(
        visible_alias = "d",
        next_help_heading = "Catalog",
        about = "Describe backend catalog objects"
    )]
    Describe {
        #[command(subcommand)]
        object: CatalogDescribe,
        #[arg(long, global = true, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    #[command(
        visible_alias = "i",
        next_help_heading = "Diagnostics",
        about = "Inspect configuration without connecting"
    )]
    Info {
        #[command(subcommand)]
        command: InfoCommand,
    },
    #[command(
        visible_alias = "h",
        next_help_heading = "Utility",
        about = "Show command help"
    )]
    Help { command: Option<String> },
    #[command(
        visible_alias = "v",
        next_help_heading = "Utility",
        about = "Show version information"
    )]
    Version,
}

#[derive(Subcommand, Debug)]
pub enum InfoCommand {
    Config,
    Connections,
}

#[derive(clap::Args, Debug)]
pub struct CatalogListArgs {
    pub glob: Option<String>,
    #[arg(long)]
    pub system: bool,
}

#[derive(Subcommand, Debug)]
pub enum CatalogList {
    #[command(visible_alias = "db")]
    Databases(CatalogListArgs),
    #[command(visible_alias = "s")]
    Schemas(CatalogListArgs),
    #[command(visible_alias = "t")]
    Tables(CatalogListArgs),
    #[command(visible_alias = "v")]
    Views(CatalogListArgs),
    #[command(visible_alias = "u")]
    Users(CatalogListArgs),
    #[command(visible_alias = "r")]
    Roles(CatalogListArgs),
    #[command(visible_alias = "i")]
    Indexes(CatalogListArgs),
}

impl CatalogList {
    pub fn request(&self) -> dzban_core::catalog::CatalogRequest {
        use dzban_core::catalog::{CatalogRequest, ObjectKind, Visibility};
        let (kind, args) = match self {
            Self::Databases(args) => (ObjectKind::Database, args),
            Self::Schemas(args) => (ObjectKind::Schema, args),
            Self::Tables(args) => (ObjectKind::Table, args),
            Self::Views(args) => (ObjectKind::View, args),
            Self::Users(args) => (ObjectKind::User, args),
            Self::Roles(args) => (ObjectKind::Role, args),
            Self::Indexes(args) => (ObjectKind::Index, args),
        };
        CatalogRequest::List {
            kind,
            database: None,
            schema: None,
            glob: args.glob.clone(),
            visibility: if args.system {
                Visibility::AllObjects
            } else {
                Visibility::UserObjects
            },
        }
    }
}

#[derive(clap::Args, Debug)]
pub struct CatalogDescribeArgs {
    pub name: String,
    #[arg(long)]
    pub full: bool,
}

#[derive(Subcommand, Debug)]
pub enum CatalogDescribe {
    #[command(visible_alias = "t")]
    Table(CatalogDescribeArgs),
    #[command(visible_alias = "v")]
    View(CatalogDescribeArgs),
}

impl CatalogDescribe {
    pub fn selection(&self) -> (dzban_core::catalog::ObjectKind, &CatalogDescribeArgs) {
        match self {
            Self::Table(args) => (dzban_core::catalog::ObjectKind::Table, args),
            Self::View(args) => (dzban_core::catalog::ObjectKind::View, args),
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OutputFormat {
    Table,
    Csv,
    Json,
    Expanded,
}
