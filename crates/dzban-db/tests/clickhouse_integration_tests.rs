use dzban_db::{
    ConnectionOperation, ConnectionState, DatabaseBackend, DatabaseConfig, DbConnection,
    ResultCell, RuntimeBridge,
};
use std::time::{Duration, Instant};

fn clickhouse_config(password: &str) -> DatabaseConfig {
    DatabaseConfig {
        name: "Integration ClickHouse".to_string(),
        backend: DatabaseBackend::Clickhouse,
        host: None,
        port: None,
        user: "dzban".to_string(),
        database: "dzban_test".to_string(),
        password: Some(password.to_string()),
        url: Some("http://127.0.0.1:58123".to_string()),
    }
}

async fn connected_connection() -> DbConnection {
    let connection = DbConnection::from_config(clickhouse_config("dzban-test"));
    connection.connect();
    let deadline = Instant::now() + Duration::from_secs(15);
    while connection.state() == ConnectionState::Connecting && Instant::now() < deadline {
        glib::timeout_future(Duration::from_millis(10)).await;
    }
    assert_eq!(
        connection.state(),
        ConnectionState::Connected,
        "{}",
        connection.last_error()
    );
    connection
}

#[glib::async_test]
#[ignore = "requires the ClickHouse service from docker-compose.test.yml"]
async fn clickhouse_query_error_and_plan_work_through_the_public_connection() {
    let connection = connected_connection().await;

    let result = connection
        .execute_query_rows("SELECT id, name, occurred_at FROM events ORDER BY id".to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.result_set.rows.len(), 3);
    assert_eq!(result.result_set.columns[0].name, "id");
    assert_eq!(result.result_set.columns[0].type_name, "UInt64");
    assert_eq!(
        result.result_set.rows[0][0],
        ResultCell::Text("1".to_string())
    );
    assert_eq!(
        result.result_set.rows[0][2],
        ResultCell::Text("2026-09-24 10:00:00.123".to_string())
    );

    let plan = connection
        .explain_query("SELECT * FROM events WHERE id > 1".to_string())
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.backend, DatabaseBackend::Clickhouse);
    assert!(plan.estimate.rows.is_some());
    assert!(plan.estimate.parts.is_some());
    assert!(plan.estimate.marks.is_some());

    let error = connection
        .execute_query_rows("SELECT * FROM missing_clickhouse_table".to_string())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.contains("ClickHouse error"), "{error}");
    assert!(error.contains("missing_clickhouse_table"), "{error}");
    connection.disconnect();
}

#[test]
#[ignore = "requires the ClickHouse service from docker-compose.test.yml"]
fn clickhouse_authentication_errors_are_descriptive() {
    let bridge = RuntimeBridge::new().unwrap();
    let receiver = bridge.test_connection(clickhouse_config("wrong-password"));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let error = runtime.block_on(receiver).unwrap().unwrap_err();

    assert!(error.contains("ClickHouse"), "{error}");
    assert!(error.contains("Authentication failed"), "{error}");
}

#[glib::async_test]
#[ignore = "requires the ClickHouse service from docker-compose.test.yml"]
async fn running_clickhouse_query_can_be_cancelled() {
    let connection = connected_connection().await;
    let query = connection.execute_query_rows(
        "SELECT sleepEachRow(1) FROM numbers(20) SETTINGS max_block_size = 1".to_string(),
    );
    glib::timeout_future(Duration::from_millis(250)).await;

    connection.cancel_query().await.unwrap().unwrap();
    let error = query.await.unwrap().unwrap_err();
    assert!(
        error.contains("cancel") || error.contains("killed"),
        "{error}"
    );
    assert!(!connection.query_running());
    connection.disconnect();
}

#[glib::async_test]
#[ignore = "requires the ClickHouse service from docker-compose.test.yml"]
async fn planned_clickhouse_query_can_be_cancelled() {
    let connection = connected_connection().await;
    let (query, plan) = connection
        .execute_query_rows_with_explain(
            "SELECT sleepEachRow(1) FROM numbers(20) SETTINGS max_block_size = 1".to_string(),
        )
        .unwrap();
    glib::timeout_future(Duration::from_millis(250)).await;

    connection.cancel_query().await.unwrap().unwrap();
    assert!(plan.await.unwrap().is_ok());
    let error = query.await.unwrap().unwrap_err();
    assert!(
        error.contains("cancel") || error.contains("killed"),
        "{error}"
    );
    assert_eq!(connection.operation(), ConnectionOperation::Idle);
    connection.disconnect();
}
