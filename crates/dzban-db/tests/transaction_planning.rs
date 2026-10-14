use std::sync::Arc;

use dzban_core::DatabaseConfig;
use dzban_core::adapter::AdapterFactory;
use dzban_core::execution::QueryEngine;
use dzban_db::adapters::PostgresqlAdapter;

#[tokio::test]
#[ignore = "requires PostgreSQL 15, 16, and 17 from docker-compose.test.yml"]
async fn automatic_planning_preserves_transactions_and_explicit_rollback() {
    for connection in [
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb",
        "host=localhost port=5416 user=user16 password=pass16 dbname=proddb",
        "host=localhost port=5417 user=admin password=secret dbname=devdb",
    ] {
        exercise_transaction(connection).await;
    }
}

async fn exercise_transaction(connection: &str) {
    let config = DatabaseConfig::from_connection_string(connection);
    let session = PostgresqlAdapter.create(config).unwrap();
    session.connect().await.unwrap();
    let engine = Arc::new(QueryEngine::new(session));
    let begin = engine.execute("BEGIN").await.unwrap();
    assert_eq!(begin.command.unwrap().affected_rows, Some(0));
    engine
        .execute("CREATE TEMP TABLE dzb_transaction_test (value integer)")
        .await
        .unwrap();
    assert!(engine.subscribe().borrow().explain_error.is_some());
    let inserted = engine
        .execute("INSERT INTO dzb_transaction_test VALUES (1)")
        .await
        .unwrap();
    assert_eq!(inserted.command.unwrap().affected_rows, Some(1));
    let empty = engine
        .execute("SELECT value FROM dzb_transaction_test WHERE false")
        .await
        .unwrap();
    assert!(empty.command.is_none());
    assert_eq!(empty.result_set.columns[0].name, "value");
    assert!(empty.result_set.rows.is_empty());
    assert!(
        engine
            .preview("SELECT missing_preview_column")
            .await
            .unwrap_err()
            .contains("42703")
    );
    assert_eq!(
        engine
            .execute("SELECT value FROM dzb_transaction_test")
            .await
            .unwrap()
            .result_set
            .rows[0][0]
            .display(),
        "1"
    );
    engine
        .execute("SET LOCAL statement_timeout = '5s'")
        .await
        .unwrap();
    assert!(engine.subscribe().borrow().explain_error.is_some());
    engine.execute("COMMIT").await.unwrap();
    let result = engine
        .execute("SELECT value FROM dzb_transaction_test")
        .await
        .unwrap();
    assert_eq!(result.result_set.rows[0][0].display(), "1");
    engine.execute("BEGIN").await.unwrap();
    engine
        .execute("INSERT INTO dzb_transaction_test VALUES (2)")
        .await
        .unwrap();
    assert!(
        engine
            .execute("SELECT missing_dzb_column")
            .await
            .unwrap_err()
            .contains("42703")
    );
    assert!(
        engine
            .execute("SELECT 1")
            .await
            .unwrap_err()
            .contains("25P02")
    );
    engine.execute("ROLLBACK").await.unwrap();
    let result = engine
        .execute("SELECT count(*) FROM dzb_transaction_test")
        .await
        .unwrap();
    assert_eq!(result.result_set.rows[0][0].display(), "1");
}
