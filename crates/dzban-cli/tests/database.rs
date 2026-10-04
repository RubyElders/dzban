use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("dzb-batch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            r#"
[[databases]]
name = "pg"
host = "localhost"
port = 5415
user = "testuser"
password = "testpass"
database = "testdb"

[[databases]]
name = "ch"
backend = "clickhouse"
url = "http://localhost:58123"
user = "dzban"
password = "dzban-test"
database = "dzban_test"
"#,
        )
        .unwrap();
        Self(root)
    }

    fn command(&self, profile: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dzb"));
        command
            .args(["--config"])
            .arg(self.0.join("config.toml"))
            .args(["--connection", profile])
            .env("XDG_DATA_HOME", self.0.join("data"));
        command
    }

    fn query(&self, profile: &str, sql: &str, format: &str) -> Output {
        self.command(profile)
            .args(["q", sql, "--format", format])
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
fn batch_capture_stops_at_row_limit_and_reports_it_without_changing_order() {
    let fixture = Fixture::new();
    for (profile, sql) in [
        ("pg", "SELECT i FROM generate_series(1,12000) i"),
        ("ch", "SELECT number + 1 AS i FROM numbers(12000)"),
    ] {
        let output = fixture.query(profile, sql, "json");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["cutoff"], "Rows");
        let rows = result["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 10_000);
        assert_eq!(rows[0][0], "1");
        assert_eq!(rows[9999][0], "10000");
        let csv = fixture.query(profile, sql, "csv");
        assert!(csv.status.success());
        assert_eq!(String::from_utf8_lossy(&csv.stdout).lines().count(), 10_001);
        assert!(String::from_utf8_lossy(&csv.stderr).contains("result limited by row limit"));
    }
}

#[test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
fn batch_capture_stops_at_byte_limit_and_pages_wide_values() {
    let fixture = Fixture::new();
    for (profile, sql) in [
        (
            "pg",
            "SELECT repeat('xx',524288) AS payload FROM generate_series(1,40)",
        ),
        (
            "ch",
            "SELECT repeat('xx',524288) AS payload FROM numbers(40)",
        ),
    ] {
        let output = fixture.query(profile, sql, "json");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["cutoff"], "Bytes");
        let rows = result["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 31);
        assert!(
            rows.iter()
                .all(|row| row[0].as_str().unwrap().len() == 1048576)
        );
    }
}

#[test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
fn batch_catalog_commands_share_actions_aliases_and_clean_pipe_contracts() {
    let fixture = Fixture::new();
    for profile in ["pg", "ch"] {
        let table = if profile == "pg" { "users" } else { "events" };
        let canonical = fixture
            .command(profile)
            .args(["list", "tables", table, "--format", "json"])
            .output()
            .unwrap();
        let alias = fixture
            .command(profile)
            .args(["ls", "t", table, "--format", "json"])
            .output()
            .unwrap();
        assert!(
            canonical.status.success(),
            "{}",
            String::from_utf8_lossy(&canonical.stderr)
        );
        assert_eq!(alias.status.code(), Some(0));
        assert_eq!(alias.stdout, canonical.stdout);
        assert!(alias.stderr.is_empty());
        let result: dzban_core::ResultSet = serde_json::from_slice(&alias.stdout).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][2].display(), table);
        let full = fixture
            .command(profile)
            .args(["d", "t", table, "--full", "--format", "json"])
            .output()
            .unwrap();
        assert!(
            full.status.success(),
            "{}",
            String::from_utf8_lossy(&full.stderr)
        );
        assert!(full.stderr.is_empty());
        let result: dzban_core::ResultSet = serde_json::from_slice(&full.stdout).unwrap();
        let column = result
            .columns
            .iter()
            .position(|column| column.name == "column_name")
            .unwrap();
        assert_eq!(result.rows[0][column].display(), "id");
        assert!(result.columns.iter().any(|column| column.name == "comment"));
        let wrong_kind = fixture
            .command(profile)
            .args(["describe", "view", table, "--format", "json"])
            .output()
            .unwrap();
        assert_eq!(wrong_kind.status.code(), Some(1));
        assert!(wrong_kind.stdout.is_empty());
        assert!(String::from_utf8_lossy(&wrong_kind.stderr).contains("No matching object"));
        let empty = fixture
            .command(profile)
            .args(["ls", "t", "no-such-object", "--format", "json"])
            .output()
            .unwrap();
        assert!(empty.status.success());
        let result: dzban_core::ResultSet = serde_json::from_slice(&empty.stdout).unwrap();
        assert!(result.rows.is_empty());
        let csv = fixture
            .command(profile)
            .args(["list", "tables", "catalog odd's table", "--format", "csv"])
            .output()
            .unwrap();
        assert!(csv.status.success());
        assert!(String::from_utf8_lossy(&csv.stdout).contains("catalog odd's table"));
        assert!(
            !fixture.0.join("data/dzban").exists(),
            "Catalog reads must not create query history"
        );
    }
    let unsupported = fixture
        .command("ch")
        .args(["ls", "schemas", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(unsupported.status.code(), Some(1));
    assert!(unsupported.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unsupported.stderr).contains("Unsupported catalog action"));
    let system = fixture
        .command("pg")
        .args(["ls", "s", "pg_catalog", "--system", "--format", "json"])
        .output()
        .unwrap();
    assert!(system.status.success());
    let result: dzban_core::ResultSet = serde_json::from_slice(&system.stdout).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0][2].display(), "pg_catalog");
}

#[test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
fn history_storage_failure_preserves_batch_results_and_native_query_errors() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.0.join("data")).unwrap();
    std::fs::write(fixture.0.join("data/dzban"), b"not a directory").unwrap();
    for profile in ["pg", "ch"] {
        let output = fixture.query(profile, "SELECT 42 AS answer", "json");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: dzban_core::ResultSet = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result.rows[0][0].display(), "42");
        assert!(String::from_utf8_lossy(&output.stderr).contains("could not save query history"));
        let output = fixture.query(profile, "SELECT * FROM missing_history_fixture", "json");
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("could not save query history"));
        assert!(
            error.contains(if profile == "pg" {
                "SQLSTATE 42P01"
            } else {
                "ClickHouse code 60"
            }),
            "{error}"
        );
    }
}

#[test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
fn batch_commands_report_backend_acknowledgments_without_fabricated_counts() {
    let fixture = Fixture::new();
    for (profile, sql, expected) in [
        ("pg", "BEGIN", Some(0)),
        ("ch", "SET max_threads = 2", None),
    ] {
        let output = fixture.query(profile, sql, "json");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value.get("command").is_some());
        assert_eq!(value["command"]["affected_rows"].as_u64(), expected);
    }
}

#[test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
fn batch_output_preserves_values_and_history_contains_the_plan() {
    let fixture = Fixture::new();
    for profile in ["pg", "ch"] {
        let output = fixture.query(profile, "SELECT 42 AS answer", "json");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let result: dzban_core::ResultSet = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result.columns[0].name, "answer");
        assert_eq!(result.rows[0][0].display(), "42");
    }
    let history_root = fixture.0.join("data/dzban/query-history");
    let directories: Vec<_> = std::fs::read_dir(history_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(directories.len(), 2);
    for directory in directories {
        let records: Vec<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(records.len(), 1);
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&records[0]).unwrap()).unwrap();
        assert_eq!(record["entry"]["outcome"]["status"], "success");
        assert!(!record["entry"]["outcome"]["explain"].is_null());
        assert_eq!(record["entry"]["outcome"]["result"]["rows"][0][0], "42");
    }
}

#[test]
#[ignore = "requires PostgreSQL from docker-compose.test.yml"]
fn file_and_stdin_are_opaque_inputs_and_sql_errors_leave_stdout_empty() {
    let fixture = Fixture::new();
    let sql = "SELECT 'žbán;\nline' AS value";
    let path = fixture.0.join("query.sql");
    std::fs::write(&path, sql).unwrap();
    let file = fixture
        .command("pg")
        .args(["query", "--file"])
        .arg(path)
        .args(["--format", "csv"])
        .output()
        .unwrap();
    let mut child = fixture
        .command("pg")
        .args(["q", "--format", "csv"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(sql.as_bytes())
        .unwrap();
    let stdin = child.wait_with_output().unwrap();
    assert!(file.status.success());
    assert!(stdin.status.success());
    assert_eq!(file.stdout, stdin.stdout);
    assert_eq!(
        String::from_utf8(file.stdout).unwrap(),
        "\"value\"\n\"žbán;\nline\"\n"
    );
    let error = fixture.query("pg", "SELECT nonexistent_dzb_column", "table");
    assert_eq!(error.status.code(), Some(1));
    assert!(error.stdout.is_empty());
    assert!(String::from_utf8_lossy(&error.stderr).contains("42703"));
    let empty = fixture.query("pg", "SELECT 42 AS answer WHERE false", "csv");
    assert!(empty.status.success());
    assert_eq!(empty.stdout, b"\"answer\"\n");
}

#[test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
fn batch_failures_store_native_diagnostics_in_shared_history() {
    for (profile, sql, code) in [
        (
            "pg",
            "SELECT nonexistent_dzb_column",
            serde_json::json!({"SqlState": "42703"}),
        ),
        (
            "ch",
            "SELECT * FROM nonexistent_dzb_table",
            serde_json::json!({"ClickHouse": 60}),
        ),
    ] {
        let fixture = Fixture::new();
        let result = fixture.query(profile, sql, "table");
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        let directory = std::fs::read_dir(fixture.0.join("data/dzban/query-history"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let record = std::fs::read_dir(directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let entry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(record).unwrap()).unwrap();
        assert_eq!(
            entry["entry"]["outcome"]["diagnostic"]["Database"]["code"],
            code
        );
    }
}
