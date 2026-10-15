use std::sync::Arc;
use std::time::Duration;

use dzban_core::adapter::{AdapterFactory, DatabaseSession};
use dzban_core::error::{ErrorKind, ExecutionError, NativeErrorCode};
use dzban_core::result_store::ResultLimits;
use dzban_core::result_stream::start_results;
use dzban_core::{DatabaseBackend, DatabaseConfig};
use dzban_db::adapters::{ClickhouseAdapter, PostgresqlAdapter};

fn session(backend: DatabaseBackend, host: &str) -> Arc<dyn DatabaseSession> {
    let variable = match backend {
        DatabaseBackend::Postgresql => "DZBAN_TLS_PG_PORT",
        DatabaseBackend::Clickhouse => "DZBAN_TLS_CH_PORT",
    };
    let port: u16 = std::env::var(variable)
        .expect("use scripts/test-macos-tls.py")
        .parse()
        .unwrap();
    let config = DatabaseConfig {
        name: "tls-fixture".into(),
        backend,
        host: Some(host.into()),
        port: Some(port),
        user: "dzban".into(),
        database: "default".into(),
        password: Some("disposable-tls-fixture".into()),
        url: (backend == DatabaseBackend::Clickhouse).then(|| format!("https://{host}:{port}")),
    };
    match backend {
        DatabaseBackend::Postgresql => PostgresqlAdapter.create(config),
        DatabaseBackend::Clickhouse => ClickhouseAdapter.create(config),
    }
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable TLS endpoints from scripts/test-macos-tls.py"]
async fn certificate_validation_and_cancel_recovery() {
    let mode = std::env::var("DZBAN_TLS_EXPECT").expect("TLS expectation is required");
    for backend in [DatabaseBackend::Postgresql, DatabaseBackend::Clickhouse] {
        let host = if mode == "hostname" {
            "127.0.0.1"
        } else {
            "localhost"
        };
        let session = session(backend, host);
        let connected = tokio::time::timeout(Duration::from_secs(15), session.connect_detailed())
            .await
            .expect("TLS connection timed out");
        if mode != "trusted" {
            assert!(
                matches!(connected, Err(ExecutionError::Database(ref error)) if error.kind == ErrorKind::Transport),
                "{backend:?} accepted an invalid certificate or returned the wrong error: {connected:?}"
            );
            continue;
        }
        connected.unwrap();
        let value = session.execute("SELECT 42").await.unwrap();
        assert_eq!(value.rows[0][0].display(), "42");
        let running = session.clone();
        let (mut sink, mut reader) = start_results(ResultLimits::default());
        let sql = match backend {
            DatabaseBackend::Postgresql => "SELECT pg_sleep(30)",
            DatabaseBackend::Clickhouse => {
                "SELECT number, sleepEachRow(0.1) FROM numbers(300) SETTINGS max_block_size=1, max_threads=1"
            }
        };
        let task = tokio::spawn(async move { running.execute_streamed(sql, &mut sink).await });
        tokio::time::timeout(Duration::from_secs(10), async {
            while reader.borrow().result.is_none() {
                reader.changed().await.unwrap();
            }
        })
        .await
        .expect("TLS query did not publish its schema");
        session.cancel_detailed().await.unwrap();
        let canceled = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(canceled, Err(ExecutionError::Canceled))
                || matches!(canceled, Err(ExecutionError::Database(ref error)) if error.kind == ErrorKind::Canceled || error.code == Some(NativeErrorCode::ClickHouse(394))),
            "TLS query did not report cancellation: {canceled:?}"
        );
        session.ping_detailed().await.unwrap();
        assert_eq!(
            session.execute("SELECT 43").await.unwrap().rows[0][0].display(),
            "43"
        );
        session.disconnect().await.unwrap();
    }
}
