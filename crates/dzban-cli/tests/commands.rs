use std::process::Command;

fn dzb(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_dzb"))
        .args(arguments)
        .env("DZBAN_CONFIG", "/definitely-missing-dzban-test-config.toml")
        .output()
        .unwrap()
}

#[test]
fn bare_command_and_help_do_not_load_configuration() {
    for arguments in [vec![], vec!["h"], vec!["--help"], vec!["h", "query"]] {
        let output = dzb(&arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
}

#[test]
fn visible_aliases_match_canonical_commands() {
    for (canonical, alias) in [
        (vec!["console", "--help"], vec!["c", "--help"]),
        (vec!["query", "--help"], vec!["q", "--help"]),
        (vec!["list", "--help"], vec!["ls", "--help"]),
        (vec!["describe", "--help"], vec!["d", "--help"]),
        (vec!["list", "tables", "--help"], vec!["ls", "t", "--help"]),
        (vec!["describe", "view", "--help"], vec!["d", "v", "--help"]),
        (vec!["info", "--help"], vec!["i", "--help"]),
        (vec!["help", "query"], vec!["h", "query"]),
        (vec!["version"], vec!["v"]),
    ] {
        let expected = dzb(&canonical);
        let actual = dzb(&alias);
        assert!(expected.status.success(), "{canonical:?}");
        assert_eq!(actual.status.code(), expected.status.code(), "{alias:?}");
        assert_eq!(actual.stdout, expected.stdout, "{alias:?}");
        assert_eq!(actual.stderr, expected.stderr, "{alias:?}");
    }
}

#[test]
fn help_groups_commands_without_hiding_aliases() {
    let output = dzb(&[]);
    let help = String::from_utf8(output.stdout).unwrap();
    for heading in ["Workflow:", "Catalog:", "Diagnostics:", "Utility:"] {
        assert!(help.contains(heading), "{help}");
    }
    for alias in ["c", "q", "ls", "d", "i", "h", "v"] {
        assert!(help.contains(&format!("[aliases: {alias}]")));
    }
}

#[test]
fn catalog_help_and_argument_validation_do_not_connect() {
    for args in [vec!["list", "--help"], vec!["describe", "table", "--help"]] {
        assert!(dzb(&args).status.success());
    }
    for args in [
        vec!["list"],
        vec!["describe", "table"],
        vec!["list", "missing-family"],
    ] {
        let output = dzb(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("missing-dzban-test-config"));
    }
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_dzb"))
        .arg("--config")
        .arg(fixture)
        .args(["ls", "t"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Select a configured profile"));
}

#[test]
fn version_alias_does_not_connect() {
    let output = dzb(&["v"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("dzb {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn batch_profile_selection_fails_without_prompting_or_producing_result_output() {
    let config =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config.toml");
    for (selection, message) in [
        (vec![], "Select a configured profile with --connection NAME"),
        (
            vec!["--connection", "not-configured"],
            "Unknown connection profile: not-configured",
        ),
        (vec!["--connection", "PG"], "Unknown connection profile: PG"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dzb"))
            .arg("--config")
            .arg(&config)
            .args(selection)
            .args(["q", "SELECT 1"])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
    }
}

#[test]
fn query_alias_and_invalid_arguments_use_usage_exit_code() {
    let output = dzb(&["q", "select 1"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("config"));
    let output = dzb(&["q", "select 1", "--file", "query.sql"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn diagnostics_show_configuration_origin_without_connecting_or_exposing_passwords() {
    let config =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config.toml");
    for explicit in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dzb"));
        command.env("DZBAN_CONFIG", &config);
        if explicit {
            command.arg("--config").arg(&config);
        }
        let output = command
            .args([
                "--connection",
                "pg",
                "--database",
                "temporary",
                "i",
                "config",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains(if explicit {
                "source: --config"
            } else {
                "source: DZBAN_CONFIG"
            }),
            "{text}"
        );
        assert!(text.contains("session only"));
        assert!(!text.contains("testpass"));
        assert!(!text.contains("dzban-test"));
        assert!(text.contains("testdb"));
        assert!(output.stderr.is_empty());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_dzb"))
        .env("DZBAN_CONFIG", "/nonexistent-dzb-config")
        .arg("--config")
        .arg(config)
        .args(["i", "connections"])
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn diagnostics_report_the_discovered_file_from_the_execution_directory() {
    let directory =
        std::env::temp_dir().join(format!("dzb-config-context-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("config.toml");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config.toml"),
        &path,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dzb"))
        .current_dir(&directory)
        .env_remove("DZBAN_CONFIG")
        .args(["i", "config"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(path.to_str().unwrap()), "{text}");
    assert!(text.contains("source: discovery"), "{text}");
    assert!(!text.contains("testpass"));
    assert!(output.stderr.is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}
