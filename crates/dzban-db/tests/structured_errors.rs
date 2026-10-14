use dzban_core::DatabaseConfig;
use dzban_core::adapter::AdapterFactory;
use dzban_core::error::{ErrorKind, ExecutionError, NativeErrorCode};
use dzban_db::adapters::PostgresqlAdapter;

#[tokio::test]
#[ignore = "requires PostgreSQL from docker-compose.test.yml"]
async fn auxiliary_failures_preserve_aborted_transaction_diagnostics_and_allow_recovery() {
    let engine = dzban_db::adapters::registry()
        .connect(DatabaseConfig::from_connection_string(
            "host=localhost port=5415 user=testuser password=testpass dbname=testdb",
        ))
        .await
        .unwrap();
    engine.execute_detailed("BEGIN").await.unwrap();
    assert!(
        engine
            .execute_detailed("SELECT dzb_missing_column")
            .await
            .is_err()
    );
    for error in [
        engine.ping_detailed().await.unwrap_err(),
        engine.version_detailed().await.unwrap_err(),
    ] {
        let ExecutionError::Database(diagnostic) = error else {
            panic!("Expected a native auxiliary failure")
        };
        assert_eq!(
            diagnostic.code,
            Some(NativeErrorCode::SqlState("25P02".into()))
        );
    }
    engine.execute_detailed("ROLLBACK").await.unwrap();
    assert!(engine.ping_detailed().await.is_ok());
    assert!(!engine.version_detailed().await.unwrap().is_empty());
    engine.close().await.unwrap();
}

#[tokio::test]
#[ignore = "requires PostgreSQL from docker-compose.test.yml"]
async fn authentication_failure_keeps_native_identity_at_registry_boundary() {
    let config = DatabaseConfig::from_connection_string(
        "host=localhost port=5415 user=testuser password=wrong-fixture-password dbname=testdb",
    );
    let error = dzban_db::adapters::registry()
        .connect(config)
        .await
        .err()
        .unwrap();
    let dzban_core::adapter::AdapterError::Connect {
        backend,
        error: ExecutionError::Database(diagnostic),
    } = error
    else {
        panic!("Expected a structured connection failure");
    };
    assert_eq!(backend, dzban_core::DatabaseBackend::Postgresql);
    assert_eq!(diagnostic.kind, ErrorKind::Server);
    assert_eq!(
        diagnostic.code,
        Some(NativeErrorCode::SqlState("28P01".into()))
    );
}

#[tokio::test]
#[ignore = "requires ClickHouse from docker-compose.test.yml"]
async fn clickhouse_snapshot_retains_native_exception_code() {
    let config = DatabaseConfig {
        name: "clickhouse fixture".into(),
        backend: dzban_core::DatabaseBackend::Clickhouse,
        host: None,
        port: None,
        user: "dzban".into(),
        password: Some("dzban-test".into()),
        database: "dzban_test".into(),
        url: Some("http://localhost:58123".into()),
    };
    let engine = dzban_db::adapters::registry()
        .connect(config)
        .await
        .unwrap();
    let preview = engine
        .preview_detailed("SELECT * FROM dzb_missing_table")
        .await
        .unwrap_err();
    assert!(
        matches!(preview, ExecutionError::Database(error) if error.code == Some(NativeErrorCode::ClickHouse(60)))
    );
    assert!(
        engine
            .execute("SELECT * FROM dzb_missing_table")
            .await
            .is_err()
    );
    let state = engine.subscribe();
    let Some(ExecutionError::Database(error)) = state.borrow().execution_error.clone() else {
        panic!("Expected a structured database error");
    };
    assert_eq!(error.code, Some(NativeErrorCode::ClickHouse(60)));
    assert_eq!(error.kind, ErrorKind::Server);
    assert!(
        matches!(state.borrow().planning_error.as_ref(), Some(ExecutionError::Database(error)) if error.code == Some(NativeErrorCode::ClickHouse(60)))
    );
    engine.execute("SELECT 1").await.unwrap();
    assert!(state.borrow().execution_error.is_none());
    assert!(state.borrow().planning_error.is_none());
    engine.close().await.unwrap();
    assert!(matches!(
        engine.execute_detailed("SELECT 1").await,
        Err(ExecutionError::Closed)
    ));
}

#[tokio::test]
#[ignore = "requires PostgreSQL from docker-compose.test.yml"]
async fn postgres_execution_retains_native_error_identity() {
    let session = PostgresqlAdapter
        .create(DatabaseConfig::from_connection_string(
            "host=localhost port=5415 user=testuser password=testpass dbname=testdb",
        ))
        .unwrap();
    session.connect().await.unwrap();
    let error = session
        .execute_detailed("SELECT dzb_missing_column")
        .await
        .unwrap_err();
    let ExecutionError::Database(error) = error else {
        panic!("Expected a structured database error");
    };
    assert_eq!(error.kind, ErrorKind::Server);
    assert_eq!(error.code, Some(NativeErrorCode::SqlState("42703".into())));
    assert_eq!(
        error.message,
        "column \"dzb_missing_column\" does not exist"
    );
    let engine = std::sync::Arc::new(dzban_core::execution::QueryEngine::new(session));
    let preview = engine
        .preview_detailed("SELECT dzb_missing_column")
        .await
        .unwrap_err();
    assert_eq!(preview, ExecutionError::Database(error.clone()));
    let returned = engine
        .start("SELECT dzb_missing_column".into())
        .unwrap()
        .wait_detailed()
        .await
        .unwrap_err();
    assert_eq!(returned, ExecutionError::Database(error.clone()));
    let state = engine.subscribe();
    assert_eq!(
        state.borrow().execution_error,
        Some(ExecutionError::Database(error))
    );
    assert!(
        matches!(state.borrow().planning_error.as_ref(), Some(ExecutionError::Database(error)) if error.code == Some(NativeErrorCode::SqlState("42703".into())))
    );
    engine.execute("SELECT 1").await.unwrap();
    assert!(state.borrow().execution_error.is_none());
    assert!(state.borrow().planning_error.is_none());
    engine.close().await.unwrap();
    assert!(matches!(
        engine.execute_detailed("SELECT 1").await,
        Err(ExecutionError::Closed)
    ));
}
