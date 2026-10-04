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
