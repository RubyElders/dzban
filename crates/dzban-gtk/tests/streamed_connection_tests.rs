use std::time::{Duration, Instant};

use dzban_core::execution::QueryPhase;
use dzban_core::result_stream::{ResultPhase, ResultTermination};
use dzban_gtk::database::{ConnectionState, DatabaseBackend, DatabaseConfig, DbConnection};

#[glib::async_test]
#[ignore = "requires PostgreSQL 15 and ClickHouse from docker-compose.test.yml"]
async fn glib_streamed_delivery_exposes_early_rows_cancellation_and_restart() {
    for (config, slow) in [
        (
            DatabaseConfig::from_connection_string(
                "host=localhost port=5415 user=testuser password=testpass dbname=testdb",
            ),
            "SELECT i, repeat('x',9000), pg_sleep(0.1) FROM generate_series(1,100) i",
        ),
        (
            DatabaseConfig {
                name: "stream-ui-fixture".into(),
                backend: DatabaseBackend::Clickhouse,
                host: None,
                port: None,
                user: "dzban".into(),
                password: Some("dzban-test".into()),
                database: "dzban_test".into(),
                url: Some("http://127.0.0.1:58123".into()),
            },
            "SELECT number + 1 AS i, repeat('x',9000), sleepEachRow(0.1) FROM numbers(100) SETTINGS max_block_size=1, max_threads=1",
        ),
    ] {
        let connection = DbConnection::from_config(config);
        connection.connect();
        let deadline = Instant::now() + Duration::from_secs(5);
        while connection.state() == ConnectionState::Connecting {
            assert!(Instant::now() < deadline);
            glib::timeout_future(Duration::from_millis(10)).await;
        }
        assert_eq!(
            connection.state(),
            ConnectionState::Connected,
            "{}",
            connection.last_error()
        );
        let delivery = connection
            .execute_streamed_query(slow.into(), None)
            .unwrap();
        let id = delivery.id;
        let deadline = Instant::now() + Duration::from_secs(5);
        let capture = loop {
            let snapshot = delivery.state.borrow().clone();
            assert!(!matches!(snapshot.phase, QueryPhase::Failed(_, _)));
            if let Some(capture) = snapshot.capture
                && capture.rows > 0
            {
                break capture;
            }
            assert!(Instant::now() < deadline, "first streamed row timed out");
            glib::timeout_future(Duration::from_millis(10)).await;
        };
        assert_eq!(capture.phase, ResultPhase::Receiving);
        assert!(connection.query_running());
        let page = delivery
            .bridge
            .read_result_page(capture.result.unwrap(), 0, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.rows[0][0].display(), "1");
        connection.cancel_query().await.unwrap().unwrap();
        assert!(matches!(
            delivery.result.await.unwrap(),
            Err(dzban_core::error::ExecutionError::Canceled)
        ));
        assert_eq!(delivery.state.borrow().phase, QueryPhase::Canceled(id));
        assert_eq!(
            delivery.state.borrow().capture.as_ref().unwrap().phase,
            ResultPhase::Finished(ResultTermination::Canceled)
        );
        assert!(!connection.query_running());
        let restarted = connection
            .execute_streamed_query("SELECT 42".into(), None)
            .unwrap();
        let result = restarted.result.await.unwrap().unwrap();
        assert_eq!(result.capture.rows, 1);
        let page = restarted
            .bridge
            .read_result_page(result.capture.result.unwrap(), 0, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.rows[0][0].display(), "42");
        connection
            .disconnect_with_completion()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }
}
