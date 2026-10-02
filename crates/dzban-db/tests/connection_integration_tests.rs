use dzban_db::{ConnectionState, DbConnection, ResultCell};
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
#[ignore]
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
#[ignore]
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
