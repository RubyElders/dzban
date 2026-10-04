use dzban_cli::console_command::{Command, help};
use dzban_core::action::ActionRegistry;
use dzban_core::catalog::Capabilities;

#[test]
fn query_help_uses_registry_descriptions_and_capability_filtering() {
    let portable = ActionRegistry::portable();
    let mut registry = ActionRegistry::default();
    for (id, description) in [
        ("query.submit", "Fixture submit description"),
        ("query.cancel", "Fixture cancel description"),
    ] {
        let mut action = portable.get(id).unwrap().clone();
        action.description = description.into();
        registry.register(action).unwrap();
    }
    let mut capabilities = Capabilities::default();
    let text = help(&registry, &capabilities);
    assert!(text.contains("Fixture submit description"));
    assert!(text.contains("Fixture cancel description"));
    assert!(!text.contains("Explain and execute the submitted SQL"));
    capabilities.cancellation = false;
    let text = help(&registry, &capabilities);
    assert!(text.contains("Fixture submit description"));
    assert!(!text.contains("Fixture cancel description"));
    assert!(!text.contains("\\cancel"));
    assert!(text.contains("\\plan"));
    assert!(text.contains("\\pager"));
}

#[test]
fn console_help_advertises_supported_local_commands_and_aliases() {
    let text = help(&ActionRegistry::portable(), &Capabilities::default());
    for category in ["Query", "Connection", "Output and history", "Console"] {
        assert!(text.lines().any(|line| line == category), "{text}");
    }
    for (input, command, advertised) in [
        ("\\?", Command::Help, "\\?"),
        ("\\q", Command::Quit, "\\q"),
        ("\\r", Command::Clear, "\\r"),
        ("\\e", Command::Edit, "\\e"),
        ("\\edit", Command::Edit, "\\edit"),
        ("\\w draft.sql", Command::Write("draft.sql".into()), "\\w"),
        (
            "\\write draft.sql",
            Command::Write("draft.sql".into()),
            "\\write",
        ),
        ("\\p", Command::LastSql, "\\p"),
        ("\\plan", Command::Plan, "\\plan"),
        ("\\cancel", Command::Cancel, "\\cancel"),
        ("\\errverbose", Command::ErrorDetails, "\\errverbose"),
        ("\\s", Command::History, "\\s"),
        ("\\history", Command::History, "\\history"),
        ("\\conninfo", Command::ConnectionInfo, "\\conninfo"),
        ("\\connections", Command::Connections, "\\connections"),
        ("\\c", Command::Connect(None), "\\c"),
        ("\\connect", Command::Connect(None), "\\connect"),
        ("\\x", Command::Expanded, "\\x"),
        (
            "\\pager off",
            Command::Pager(Some(false)),
            "\\pager [on|off]",
        ),
        (
            "\\timing on",
            Command::Timing(Some(true)),
            "\\timing [on|off]",
        ),
    ] {
        assert_eq!(Command::parse(input), Ok(command));
        let binding = advertised.split_whitespace().next().unwrap();
        assert!(
            text.lines().any(|line| line.contains(advertised)
                && line.split_whitespace().any(|token| token == binding)),
            "Missing {advertised}: {text}"
        );
    }
    assert_eq!(Command::parse("\\d"), Ok(Command::Tables));
    assert_eq!(Command::parse("\\dt"), Ok(Command::Tables));
    assert_eq!(Command::parse("\\dv"), Ok(Command::Views));
    for supported in ["\\l", "\\du", "\\dg", "\\di", "\\dn", "\\dS"] {
        assert!(Command::parse(supported).is_ok());
    }
    for deferred in ["\\dP", "\\dm", "\\df"] {
        assert!(Command::parse(deferred).is_err());
        assert!(!text.split_whitespace().any(|token| token == deferred));
    }
}

#[test]
fn catalog_commands_accept_only_documented_patterns_and_filter_help_by_capability() {
    use dzban_core::catalog::{CatalogAction, CatalogRequest, ObjectKind, Visibility};
    assert_eq!(
        Command::parse("\\dtS 'catalog odd*'"),
        Ok(Command::Catalog(CatalogRequest::List {
            kind: ObjectKind::Table,
            database: None,
            schema: None,
            glob: Some("catalog odd*".into()),
            visibility: Visibility::AllObjects,
        }))
    );
    for invalid in ["\\dt a b", "\\dt ''", "\\dt 'broken", "\\dt+", "\\d+"] {
        assert!(Command::parse(invalid).is_err(), "{invalid}");
    }
    assert_eq!(
        Command::parse("\\d users"),
        Ok(Command::Describe(
            "users".into(),
            dzban_core::catalog::Detail::Summary
        ))
    );
    assert_eq!(
        Command::parse("\\d+ 'catalog odd.s table'"),
        Ok(Command::Describe(
            "catalog odd.s table".into(),
            dzban_core::catalog::Detail::Full
        ))
    );
    let mut capabilities = Capabilities::default();
    capabilities
        .catalog
        .insert(CatalogAction::List(ObjectKind::Table));
    let text = help(&ActionRegistry::portable(), &capabilities);
    assert!(text.contains("\\dt [glob]"));
    assert!(!text.contains("\\dn [glob]"));
    assert!(!text.contains("\\du [glob]"));
    capabilities
        .catalog
        .insert(CatalogAction::List(ObjectKind::User));
    assert!(help(&ActionRegistry::portable(), &capabilities).contains("\\du [glob]"));
}
