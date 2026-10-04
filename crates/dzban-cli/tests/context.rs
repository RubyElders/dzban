use dzban_cli::args::{Args, ConfigSource, OutputFormat};
use dzban_cli::context::{CommandContext, OptionSource, OutputPolicy, Resolved};

fn args(extra: &[&str]) -> Args {
    let config =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config.toml");
    let matches = Args::grouped_command()
        .try_get_matches_from(
            ["dzb", "--config", config.to_str().unwrap()]
                .into_iter()
                .chain(extra.iter().copied()),
        )
        .unwrap();
    Args::from_matches(&matches).unwrap()
}

#[test]
fn context_preserves_configuration_and_resolves_session_only_overrides() {
    let context = CommandContext::load(&args(&[
        "--connection",
        "pg",
        "--database",
        "temporary",
        "q",
        "SELECT 1",
        "--format",
        "json",
    ]))
    .unwrap();
    assert_eq!(context.config_path.source, OptionSource::CommandLine);
    assert_eq!(context.connection.source, OptionSource::CommandLine);
    assert_eq!(context.database.source, OptionSource::CommandLine);
    assert_eq!(context.profile().unwrap().database, "temporary");
    assert_eq!(
        context
            .config
            .databases
            .iter()
            .find(|profile| profile.name == "pg")
            .unwrap()
            .database,
        "testdb"
    );
    assert!(matches!(
        context.output,
        OutputPolicy::Batch(Resolved {
            value: OutputFormat::Json,
            source: OptionSource::CommandLine
        })
    ));
    let application = context.application();
    assert!(std::sync::Arc::ptr_eq(&application, &context.application()));
}

#[test]
fn catalog_output_options_resolve_for_canonical_commands_and_aliases() {
    for arguments in [
        vec!["list", "tables", "--format", "json"],
        vec!["ls", "--format", "json", "t"],
        vec!["describe", "view", "name", "--format", "json"],
        vec!["d", "--format", "json", "v", "name"],
    ] {
        let context = CommandContext::load(&args(&arguments)).unwrap();
        assert!(matches!(
            context.output,
            OutputPolicy::Batch(Resolved {
                value: OutputFormat::Json,
                source: OptionSource::CommandLine
            })
        ));
    }
}

#[test]
fn context_keeps_environment_origin_and_default_override_policy() {
    let mut args = args(&["c", "--no-pager"]);
    args.config_source = ConfigSource::Environment;
    let context = CommandContext::load(&args).unwrap();
    assert_eq!(context.config_path.source, OptionSource::Environment);
    assert_eq!(context.connection.source, OptionSource::Default);
    assert_eq!(context.database.source, OptionSource::Default);
    assert!(context.profile().is_err());
    assert!(matches!(
        context.output,
        OutputPolicy::Console { enabled: false, .. }
    ));
}

#[test]
fn pager_resolution_preserves_precedence_and_explicit_disabling() {
    use dzban_cli::context::resolve_pager;
    let value = |text: &str| Some(text.to_owned());
    for (explicit, dzban, generic, expected, source) in [
        (
            value("explicit"),
            value("dzban"),
            value("generic"),
            value("explicit"),
            OptionSource::CommandLine,
        ),
        (
            None,
            value("dzban"),
            value("generic"),
            value("dzban"),
            OptionSource::Environment,
        ),
        (
            None,
            None,
            value("generic"),
            value("generic"),
            OptionSource::Environment,
        ),
        (
            value(" "),
            value("dzban"),
            value("generic"),
            None,
            OptionSource::CommandLine,
        ),
        (
            None,
            value(""),
            value("generic"),
            None,
            OptionSource::Environment,
        ),
        (None, None, None, None, OptionSource::Default),
    ] {
        let resolved = resolve_pager(explicit, dzban, generic);
        assert_eq!(resolved.value, expected);
        assert_eq!(resolved.source, source);
    }
}

#[test]
fn default_output_format_is_distinct_from_an_explicit_default_value() {
    for (arguments, expected) in [
        (vec!["q", "SELECT 1"], OptionSource::Default),
        (
            vec!["q", "SELECT 1", "--format", "table"],
            OptionSource::CommandLine,
        ),
    ] {
        let context = CommandContext::load(&args(&arguments)).unwrap();
        let OutputPolicy::Batch(format) = context.output else {
            panic!("Expected batch output")
        };
        assert!(matches!(format.value, OutputFormat::Table));
        assert_eq!(format.source, expected);
    }
}
