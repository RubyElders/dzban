use dzban_db::{ConnectionOperation, ConnectionState, DbConnection, ResultCell};
use glib::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

async fn connected_connection() -> DbConnection {
    let connection = DbConnection::new(
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb".to_string(),
    );
    connection.connect();
    let deadline = Instant::now() + Duration::from_secs(10);
    while connection.state() == ConnectionState::Connecting && Instant::now() < deadline {
        glib::timeout_future(Duration::from_millis(10)).await;
    }
    assert_eq!(connection.state(), ConnectionState::Connected);
    connection
}

#[glib::async_test]
#[ignore = "requires the PostgreSQL service from docker-compose.test.yml"]
async fn row_query_reports_rows_timing_and_state() {
    let connection = connected_connection().await;
    let finished = Rc::new(RefCell::new(None));
    let finished_clone = finished.clone();
    connection.connect_closure(
        "query-finished",
        false,
        glib::closure_local!(
            move |_connection: DbConnection, rows: i64, duration_ms: i64| {
                *finished_clone.borrow_mut() = Some((rows, duration_ms));
            }
        ),
    );

    let receiver = connection.execute_query_rows("SELECT generate_series(1, 3)".to_string());
    assert!(connection.query_running());
    let result = receiver.await.unwrap().unwrap();

    assert_eq!(result.result_set.rows.len(), 3);
    assert!(!connection.query_running());
    assert_eq!(connection.last_error(), "");
    assert_eq!(finished.borrow().as_ref().unwrap().0, 3);

    let nulls = connection
        .execute_query_rows("SELECT 'NULL'::text, NULL::text".to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        nulls.result_set.rows[0],
        vec![ResultCell::Text("NULL".to_string()), ResultCell::Null]
    );
    connection.disconnect();
}

#[glib::async_test]
#[ignore = "requires the PostgreSQL service from docker-compose.test.yml"]
async fn running_query_can_be_cancelled() {
    let connection = connected_connection().await;
    let query_receiver = connection.execute_query_rows("SELECT pg_sleep(10)".to_string());
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        glib::timeout_future(Duration::from_millis(10)).await;
    }

    let cancel_result = connection.cancel_query().await.unwrap();
    assert!(cancel_result.is_ok());
    let query_result = query_receiver.await.unwrap();

    assert!(query_result.is_err());
    assert!(!connection.query_running());
    assert!(!connection.last_error().is_empty());
    connection.disconnect();
}

#[glib::async_test]
#[ignore = "requires the PostgreSQL service from docker-compose.test.yml"]
async fn planned_query_can_be_cancelled() {
    let connection = connected_connection().await;
    let (query_receiver, plan_receiver) = connection
        .execute_query_rows_with_explain("SELECT pg_sleep(10)".to_string())
        .unwrap();
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        glib::timeout_future(Duration::from_millis(10)).await;
    }

    assert!(connection.cancel_query().await.unwrap().is_ok());
    assert!(plan_receiver.await.unwrap().is_ok());
    assert!(query_receiver.await.unwrap().is_err());
    assert_eq!(connection.operation(), ConnectionOperation::Idle);
    connection.disconnect();
}

#[glib::async_test]
#[ignore = "requires the PostgreSQL service from docker-compose.test.yml"]
async fn one_connection_rejects_overlapping_queries_explains_and_pings() {
    let connection = connected_connection().await;
    let operations = Rc::new(RefCell::new(Vec::new()));
    let operations_for_notify = operations.clone();
    connection.connect_notify_local(Some("operation"), move |connection, _| {
        operations_for_notify
            .borrow_mut()
            .push(connection.operation());
    });
    assert!(connection.operation_available());
    let initial_ping_count = connection.ping_count();
    let (query_receiver, plan_receiver) = connection
        .execute_query_rows_with_explain("SELECT pg_sleep(0.2), generate_series(1, 3)".to_string())
        .unwrap();

    assert_eq!(connection.operation(), ConnectionOperation::PlanningQuery);
    assert!(!connection.operation_available());
    assert!(connection.query_running());
    let duplicate = connection
        .execute_query_rows("SELECT 2".to_string())
        .await
        .unwrap();
    assert_eq!(duplicate.unwrap_err(), "Connection is busy");
    let overlapping_explain = connection.explain_query("SELECT 3".to_string());
    assert!(matches!(
        overlapping_explain,
        Err(error) if error == "Connection is busy"
    ));
    connection.ping();
    assert_eq!(connection.ping_count(), initial_ping_count);

    assert!(plan_receiver.await.unwrap().is_ok());
    assert_eq!(connection.operation(), ConnectionOperation::Query);
    let result = query_receiver.await.unwrap().unwrap();
    assert_eq!(result.result_set.rows.len(), 3);
    assert_eq!(connection.operation(), ConnectionOperation::Idle);
    assert!(connection.operation_available());
    assert!(!connection.query_running());
    assert_eq!(
        *operations.borrow(),
        vec![
            ConnectionOperation::PlanningQuery,
            ConnectionOperation::Query,
            ConnectionOperation::Idle,
        ]
    );
    connection.disconnect();
}
