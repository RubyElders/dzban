#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Tables,
    Views,
    Catalog(dzban_core::catalog::CatalogRequest),
    Describe(String, dzban_core::catalog::Detail),
    Quit,
    Clear,
    Edit,
    Write(std::path::PathBuf),
    Cancel,
    LastSql,
    Plan,
    ErrorDetails,
    History,
    ConnectionInfo,
    Connections,
    Connect(Option<String>),
    Expanded,
    Pager(Option<bool>),
    Timing(Option<bool>),
}

impl Command {
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let text = text.trim();
        let token = text.split_whitespace().next().unwrap_or_default();
        let detailed = token.ends_with('+');
        let without_detail = token.strip_suffix('+').unwrap_or(token);
        let base = without_detail.strip_suffix('S').unwrap_or(without_detail);
        if let Some(kind) = match base {
            "\\l" => Some(dzban_core::catalog::ObjectKind::Database),
            "\\dn" => Some(dzban_core::catalog::ObjectKind::Schema),
            "\\dt" | "\\d" => Some(dzban_core::catalog::ObjectKind::Table),
            "\\dv" => Some(dzban_core::catalog::ObjectKind::View),
            "\\du" => Some(dzban_core::catalog::ObjectKind::User),
            "\\dg" => Some(dzban_core::catalog::ObjectKind::Role),
            "\\di" => Some(dzban_core::catalog::ObjectKind::Index),
            _ => None,
        } {
            if text == "\\d" || text == "\\dt" {
                return Ok(Self::Tables);
            }
            if text == "\\dv" {
                return Ok(Self::Views);
            }
            let words = shell_words::split(text).map_err(|_| "Usage: catalog-command [glob]")?;
            if words.len() > 2 || words.get(1).is_some_and(|word| word.is_empty()) {
                return Err("Usage: catalog-command [glob]");
            }
            if base == "\\d" && words.len() == 2 {
                return Ok(Self::Describe(
                    words[1].clone(),
                    if detailed {
                        dzban_core::catalog::Detail::Full
                    } else {
                        dzban_core::catalog::Detail::Summary
                    },
                ));
            }
            if detailed {
                return Err("Detailed descriptions require \\d+ <object>");
            }
            return Ok(Self::Catalog(dzban_core::catalog::CatalogRequest::List {
                kind,
                database: None,
                schema: None,
                glob: words.get(1).cloned(),
                visibility: if base == without_detail {
                    dzban_core::catalog::Visibility::UserObjects
                } else {
                    dzban_core::catalog::Visibility::AllObjects
                },
            }));
        }
        if matches!(text.split_whitespace().next(), Some("\\w" | "\\write")) {
            let words = shell_words::split(text).map_err(|_| "Usage: \\w <new-file>")?;
            if words.len() != 2 || words[1].is_empty() {
                return Err("Usage: \\w <new-file>");
            }
            return Ok(Self::Write(words[1].clone().into()));
        }
        if let Some((command, name)) = text.split_once(char::is_whitespace)
            && matches!(command, "\\c" | "\\connect")
        {
            return Ok(Self::Connect(Some(name.trim().to_owned())));
        }
        match text {
            "\\c" | "\\connect" => Ok(Self::Connect(None)),
            "\\?" => Ok(Self::Help),
            "\\q" => Ok(Self::Quit),
            "\\r" => Ok(Self::Clear),
            "\\e" | "\\edit" => Ok(Self::Edit),
            "\\cancel" => Ok(Self::Cancel),
            "\\p" => Ok(Self::LastSql),
            "\\plan" => Ok(Self::Plan),
            "\\errverbose" => Ok(Self::ErrorDetails),
            "\\s" | "\\history" => Ok(Self::History),
            "\\conninfo" => Ok(Self::ConnectionInfo),
            "\\connections" => Ok(Self::Connections),
            "\\x" => Ok(Self::Expanded),
            "\\pager" => Ok(Self::Pager(None)),
            "\\pager on" => Ok(Self::Pager(Some(true))),
            "\\pager off" => Ok(Self::Pager(Some(false))),
            "\\timing" => Ok(Self::Timing(None)),
            "\\timing on" => Ok(Self::Timing(Some(true))),
            "\\timing off" => Ok(Self::Timing(Some(false))),
            text if text.starts_with("\\timing ") => Err("Usage: \\timing [on|off]"),
            text if text.starts_with("\\pager ") => Err("Usage: \\pager [on|off]"),
            _ => Err("Unknown console command"),
        }
    }

    pub fn catalog_request(&self) -> Option<dzban_core::catalog::CatalogRequest> {
        use dzban_core::catalog::{CatalogRequest, ObjectKind, Visibility};
        match self {
            Self::Catalog(request) => Some(request.clone()),
            Self::Tables | Self::Views => Some(CatalogRequest::List {
                kind: if *self == Self::Tables {
                    ObjectKind::Table
                } else {
                    ObjectKind::View
                },
                database: None,
                schema: None,
                glob: None,
                visibility: Visibility::UserObjects,
            }),
            _ => None,
        }
    }
}

pub fn help(
    registry: &dzban_core::action::ActionRegistry,
    capabilities: &dzban_core::catalog::Capabilities,
) -> String {
    let mut text = String::new();
    for (heading, bindings) in [
        (
            "Query",
            vec![
                (
                    "Enter / Ctrl+Enter",
                    "Submit SQL ending with ;, otherwise add a line",
                    None,
                ),
                ("\\g / \\gx", "", Some("query.submit")),
                ("Up / Down / Ctrl+P / Ctrl+N", "Browse query history", None),
                ("Ctrl+L", "Clear screen without losing the draft", None),
                ("Ctrl+A / Ctrl+E", "Move to start / end of line", None),
                (
                    "Ctrl+K / Ctrl+U / Ctrl+W / Ctrl+Y",
                    "Kill line / prefix / word / yank",
                    None,
                ),
                ("Shift+Enter / Ctrl+J", "Insert a newline", None),
                ("Ctrl+C / Esc / \\cancel", "", Some("query.cancel")),
                ("\\p", "Show the current buffer or last submitted SQL", None),
                ("\\r", "Clear the input buffer", None),
                (
                    "\\w / \\write <new-file>",
                    "Save the current buffer without overwriting a file",
                    None,
                ),
                (
                    "\\e / \\edit",
                    "Edit the current buffer or last submitted SQL without running it",
                    None,
                ),
                ("\\plan", "Show the captured plan for the last query", None),
                ("\\errverbose", "Show the last query error details", None),
            ],
        ),
        (
            "Catalog",
            vec![
                ("\\l [glob]", "", Some("catalog.list.database")),
                ("\\d / \\dt [glob]", "", Some("catalog.list.table")),
                ("\\dv [glob]", "", Some("catalog.list.view")),
                ("\\d[+] <object>", "", Some("catalog.describe.table")),
                ("\\dn [glob]", "", Some("catalog.list.schema")),
                ("\\du [glob]", "", Some("catalog.list.user")),
                ("\\dg [glob]", "", Some("catalog.list.role")),
                ("\\di [glob]", "", Some("catalog.list.index")),
            ],
        ),
        (
            "Connection",
            vec![
                (
                    "\\c / \\connect [profile]",
                    "Switch connection profile",
                    None,
                ),
                ("\\conninfo", "Show the current connection", None),
                ("\\connections", "List configured profiles", None),
            ],
        ),
        (
            "Output and history",
            vec![
                ("\\x", "Toggle expanded output", None),
                ("\\pager [on|off]", "Inspect or change pager use", None),
                ("\\timing [on|off]", "Toggle or set query timing", None),
                ("\\s / \\history", "List shared query history", None),
                (
                    "Ctrl+R",
                    "Search history and load SQL without running it",
                    None,
                ),
            ],
        ),
        (
            "Console",
            vec![
                ("\\?", "Show this help", None),
                ("\\q / Ctrl+D", "Quit when no query is running", None),
            ],
        ),
    ] {
        text.push_str(&format!("{heading}\n"));
        for (binding, description, action_id) in bindings {
            let description = match action_id {
                Some(id) => {
                    let Some(action) = registry
                        .get(id)
                        .filter(|action| action.capability.supported(capabilities))
                    else {
                        continue;
                    };
                    action.description.as_str()
                }
                None => description,
            };
            text.push_str(&format!("  {binding:<28} {description}\n"));
        }
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_have_exact_arguments_and_aliases() {
        assert_eq!(
            Command::parse(r#"\w "query draft.sql""#),
            Ok(Command::Write("query draft.sql".into()))
        );
        assert_eq!(
            Command::parse("\\write query.sql"),
            Command::parse("\\w query.sql")
        );
        for invalid in ["\\w", "\\w a b", "\\w ''", "\\w 'broken"] {
            assert!(Command::parse(invalid).is_err());
        }
        assert_eq!(Command::parse("\\s"), Command::parse("\\history"));
        assert_eq!(
            Command::parse(" \\pager off "),
            Ok(Command::Pager(Some(false)))
        );
        assert_eq!(Command::parse("\\pager on"), Ok(Command::Pager(Some(true))));
        assert_eq!(Command::parse("\\pager"), Ok(Command::Pager(None)));
        assert!(Command::parse("\\pager on extra").is_err());
        assert_eq!(Command::parse("\\timing"), Ok(Command::Timing(None)));
        assert_eq!(
            Command::parse("\\timing on"),
            Ok(Command::Timing(Some(true)))
        );
        assert_eq!(
            Command::parse("\\timing off"),
            Ok(Command::Timing(Some(false)))
        );
        assert!(Command::parse("\\timing on extra").is_err());
        assert!(Command::parse("\\q extra").is_err());
        assert_eq!(Command::parse("\\e"), Command::parse("\\edit"));
        assert!(Command::parse("\\e extra").is_err());
        assert_eq!(Command::parse("\\cancel"), Ok(Command::Cancel));
        assert!(Command::parse("\\cancel extra").is_err());
        assert_eq!(Command::parse("\\errverbose"), Ok(Command::ErrorDetails));
        assert!(Command::parse("\\errverbose extra").is_err());
        assert!(Command::parse("SELECT 1").is_err());
        assert_eq!(Command::parse("\\c"), Ok(Command::Connect(None)));
        assert_eq!(
            Command::parse("\\c local"),
            Command::parse("\\connect local")
        );
        assert_eq!(
            Command::parse("\\connect\tlocal dev"),
            Ok(Command::Connect(Some("local dev".into())))
        );
    }
}
