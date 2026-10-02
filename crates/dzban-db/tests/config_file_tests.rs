use dzban_db::{Config, DatabaseBackend};
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

#[test]
fn loads_clickhouse_http_configuration() {
    let path = temp_config_path("clickhouse");
    fs::write(
        &path,
        r#"
[[databases]]
name = "analytics"
backend = "clickhouse"
url = "https://clickhouse.example.test:8443"
user = "reader"
database = "events"
"#,
    )
    .unwrap();

    let config = Config::load_from_path(&path).unwrap();
    fs::remove_file(path).unwrap();

    assert_eq!(config.databases[0].backend, DatabaseBackend::Clickhouse);
    assert_eq!(
        config.databases[0].url.as_deref(),
        Some("https://clickhouse.example.test:8443")
    );
}

#[test]
fn rejects_clickhouse_without_an_http_url() {
    let path = temp_config_path("invalid-clickhouse");
    fs::write(
        &path,
        r#"
[[databases]]
name = "analytics"
backend = "clickhouse"
url = "clickhouse://localhost:9000"
user = "reader"
database = "events"
"#,
    )
    .unwrap();

    let error = Config::load_from_path(&path).unwrap_err();
    fs::remove_file(path).unwrap();

    assert!(error.contains("must use an http:// or https:// URL"));
}
