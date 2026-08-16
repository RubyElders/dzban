use dzban_db::{ConnectionState, DbConnection};
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn connected_connection() -> DbConnection {
    let connection = DbConnection::new(
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb".to_string(),
    );
    connection.connect();
    let context = glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while connection.state() == ConnectionState::Connecting && Instant::now() < deadline {
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(connection.state(), ConnectionState::Connected);
    connection
}

#[gtk4::test]
#[ignore]
fn row_query_reports_rows_timing_and_state() {
    let connection = connected_connection();
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
    let result = glib::MainContext::default()
        .block_on(receiver)
        .unwrap()
        .unwrap();

    assert_eq!(result.rows.len(), 3);
    assert!(!connection.query_running());
    assert_eq!(connection.last_error(), "");
    assert_eq!(finished.borrow().as_ref().unwrap().0, 3);
    connection.disconnect();
}

#[gtk4::test]
#[ignore]
fn running_query_can_be_cancelled() {
    let connection = connected_connection();
    let query_receiver = connection.execute_query_rows("SELECT pg_sleep(10)".to_string());
    let context = glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(10));
    }

    let cancel_result = context.block_on(connection.cancel_query()).unwrap();
    assert!(cancel_result.is_ok());
    let query_result = context.block_on(query_receiver).unwrap();

    assert!(query_result.is_err());
    assert!(!connection.query_running());
    assert!(!connection.last_error().is_empty());
    connection.disconnect();
}
