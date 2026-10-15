use dzban_core::adapter::AdapterFactory;
use dzban_core::{DatabaseBackend, DatabaseConfig};
use dzban_db::adapters::{ClickhouseAdapter, PostgresqlAdapter};

fn profile(backend: DatabaseBackend) -> DatabaseConfig {
    DatabaseConfig {
        name: "fixture".into(),
        backend,
        host: Some("localhost".into()),
        port: None,
        user: "fixture".into(),
        database: "fixture".into(),
        password: None,
        url: (backend == DatabaseBackend::Clickhouse).then(|| "http://localhost:8123".into()),
    }
}

#[test]
fn factories_reject_profiles_for_the_other_backend() {
    assert!(
        PostgresqlAdapter
            .create(profile(DatabaseBackend::Clickhouse))
            .is_err()
    );
    assert!(
        ClickhouseAdapter
            .create(profile(DatabaseBackend::Postgresql))
            .is_err()
    );
}

#[test]
fn factories_validate_profiles_without_connecting() {
    assert!(
        PostgresqlAdapter
            .create(profile(DatabaseBackend::Postgresql))
            .is_ok()
    );
    assert!(
        ClickhouseAdapter
            .create(profile(DatabaseBackend::Clickhouse))
            .is_ok()
    );
    let mut invalid = profile(DatabaseBackend::Clickhouse);
    invalid.url = None;
    assert!(ClickhouseAdapter.create(invalid).is_err());
}

#[tokio::test]
async fn factory_sessions_do_not_connect_implicitly() {
    let session = PostgresqlAdapter
        .create(profile(DatabaseBackend::Postgresql))
        .unwrap();
    assert_eq!(
        session.execute("SELECT 1").await.unwrap_err(),
        "Not connected"
    );
}

#[tokio::test]
async fn clickhouse_connection_errors_do_not_expose_url_parameters() {
    let mut config = profile(DatabaseBackend::Clickhouse);
    config.url = Some("http://127.0.0.1:0/?token=private-fixture-token".into());
    let session = ClickhouseAdapter.create(config).unwrap();
    let error = session.connect_detailed().await.unwrap_err();
    let dzban_core::error::ExecutionError::Database(error) = error else {
        panic!("expected a structured transport error");
    };
    assert_eq!(error.kind, dzban_core::error::ErrorKind::Transport);
    assert!(!error.message.contains("private-fixture-token"));
    assert!(!error.message.contains("query_id="));
}
