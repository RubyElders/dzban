use dzban_db::Config;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_config_path(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("dzban-{name}-{}-{unique}.toml", std::process::id()))
}

#[test]
fn loads_a_valid_config_file() {
    let path = temp_config_path("valid");
    fs::write(
        &path,
        r#"
[[databases]]
name = "local"
host = "localhost"
port = 5432
user = "postgres"
database = "postgres"
"#,
    )
    .unwrap();

    let config = Config::load_from_path(&path).unwrap();
    fs::remove_file(path).unwrap();

    assert_eq!(config.databases.len(), 1);
    assert_eq!(config.databases[0].name, "local");
}

#[test]
fn reports_a_malformed_config_file() {
    let path = temp_config_path("malformed");
    fs::write(&path, "[[databases]\nname = 42").unwrap();

    let error = Config::load_from_path(&path).unwrap_err();
    fs::remove_file(path).unwrap();

    assert!(error.contains("Failed to parse TOML"));
}

#[test]
fn reports_a_missing_config_file() {
    let path = temp_config_path("missing");
    let error = Config::load_from_path(&path).unwrap_err();

    assert!(error.contains("Failed to read"));
}
