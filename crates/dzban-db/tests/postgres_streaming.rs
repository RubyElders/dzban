use std::sync::Arc;

use dzban_core::adapter::{AdapterFactory, DatabaseSession};
use dzban_core::error::{ErrorKind, ExecutionError, NativeErrorCode};
use dzban_core::result_store::{ResultCutoff, ResultLimits};
use dzban_core::result_stream::{ResultPhase, ResultSnapshot, ResultTermination, start_results};
use dzban_core::{DatabaseConfig, ResultCell};
use dzban_db::adapters::PostgresqlAdapter;

async fn session(connection: &str) -> Arc<dyn DatabaseSession> {
    let session = PostgresqlAdapter
        .create(DatabaseConfig::from_connection_string(connection))
        .unwrap();
    session.connect_detailed().await.unwrap();
    session
}

async fn capture(session: &dyn DatabaseSession, sql: &str, limits: ResultLimits) -> ResultSnapshot {
    let (mut producer, reader) = start_results(limits);
    let outcome = session.execute_streamed(sql, &mut producer).await;
    producer.finish(outcome.map(|_| ())).await.unwrap();
    reader.borrow().clone()
}

const PG15: &str = "host=localhost port=5415 user=testuser password=testpass dbname=testdb";

#[tokio::test]
#[ignore = "requires PostgreSQL 15, 16, and 17 from docker-compose.test.yml"]
async fn native_streams_preserve_schema_order_null_and_cutoff_across_versions() {
    for connection in [
        PG15,
        "host=localhost port=5416 user=user16 password=pass16 dbname=proddb",
        "host=localhost port=5417 user=admin password=secret dbname=devdb",
    ] {
        let session = session(connection).await;
        let snapshot = capture(session.as_ref(),
            "SELECT i, NULL::text AS empty, 'žluť' || chr(10) AS value FROM generate_series(1,50) i",
            ResultLimits { rows: 3, memory_rows: 1, ..Default::default() }).await;
        assert_eq!(
            snapshot.phase,
            ResultPhase::Finished(ResultTermination::Limited(ResultCutoff::Rows))
        );
        assert_eq!(snapshot.rows, 3);
        let page = snapshot.result.unwrap().read_page(0, 3).await.unwrap();
        assert_eq!(
            page.columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["i", "empty", "value"]
        );
        assert_eq!(
            page.rows[0],
            [
                ResultCell::Text("1".into()),
                ResultCell::Null,
                ResultCell::Text("žluť\n".into())
            ]
        );
        assert_eq!(page.rows[2][0], ResultCell::Text("3".into()));
        let empty = capture(
            session.as_ref(),
            "SELECT 1 AS id WHERE false",
            ResultLimits::default(),
        )
        .await;
        assert_eq!(
            empty.phase,
            ResultPhase::Finished(ResultTermination::Complete)
        );
        let page = empty.result.unwrap().read_page(0, 1).await.unwrap();
        assert_eq!(page.columns[0].name, "id");
        assert!(page.rows.is_empty());
        let bounded = capture(
            session.as_ref(),
            "SELECT repeat('x',100) AS value",
            ResultLimits {
                bytes: 32,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(bounded.cutoff, Some(ResultCutoff::Bytes));
        assert_eq!(bounded.rows, 0);
        session.ping_detailed().await.unwrap();
        session.disconnect().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires PostgreSQL from docker-compose.test.yml"]
async fn returning_cutoff_drains_writes_without_aborting_the_open_transaction() {
    let session = session(PG15).await;
    session
        .execute_detailed("CREATE TEMP TABLE dzb_stream_write (id integer)")
        .await
        .unwrap();
    let (mut producer, _) = start_results(ResultLimits::default());
    let command = session
        .execute_streamed("BEGIN", &mut producer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(command.affected_rows, Some(0));
    producer.finish(Ok(())).await.unwrap();
    let snapshot = capture(
        session.as_ref(),
        "INSERT INTO dzb_stream_write SELECT i FROM generate_series(1,20) i RETURNING id",
        ResultLimits {
            rows: 2,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        snapshot.phase,
        ResultPhase::Finished(ResultTermination::Limited(ResultCutoff::Rows))
    );
    let count = session
        .execute_detailed("SELECT count(*) FROM dzb_stream_write")
        .await
        .unwrap()
        .into_parts()
        .0;
    assert_eq!(count.rows[0][0], ResultCell::Text("20".into()));
    session.execute_detailed("ROLLBACK").await.unwrap();
    let count = session
        .execute_detailed("SELECT count(*) FROM dzb_stream_write")
        .await
        .unwrap()
        .into_parts()
        .0;
    assert_eq!(count.rows[0][0], ResultCell::Text("0".into()));
    session.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore = "requires PostgreSQL from docker-compose.test.yml"]
async fn early_rows_are_visible_and_user_cancellation_can_interrupt_cap_draining() {
    let session = session(PG15).await;
    let (mut producer, mut reader) = start_results(ResultLimits {
        rows: 2,
        ..Default::default()
    });
    let running = session.clone();
    let task = tokio::spawn(async move {
        let outcome = running
            .execute_streamed(
                "SELECT i, repeat('x',9000), pg_sleep(0.1) FROM generate_series(1,100) i",
                &mut producer,
            )
            .await;
        producer.finish(outcome.clone().map(|_| ())).await.unwrap();
        outcome
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if reader.borrow_and_update().phase == ResultPhase::Draining(ResultCutoff::Rows) {
                break;
            }
            reader.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    let handle = reader.borrow().result.clone().unwrap();
    assert_eq!(
        handle.read_page(0, 1).await.unwrap().rows[0][0],
        ResultCell::Text("1".into())
    );
    session.cancel_detailed().await.unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    let ExecutionError::Database(error) = error else {
        panic!("native cancellation diagnostic expected")
    };
    assert_eq!(error.kind, ErrorKind::Canceled);
    assert_eq!(error.code, Some(NativeErrorCode::SqlState("57014".into())));
    session.ping_detailed().await.unwrap();
    let restarted = capture(session.as_ref(), "SELECT 42", ResultLimits::default()).await;
    assert_eq!(
        restarted.phase,
        ResultPhase::Finished(ResultTermination::Complete)
    );
    session.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore = "requires PostgreSQL from docker-compose.test.yml"]
async fn a_late_server_failure_is_not_hidden_by_a_capture_cutoff() {
    let session = session(PG15).await;
    let snapshot = capture(
        session.as_ref(),
        "SELECT CASE WHEN i = 5 THEN 1 / (i - 5) ELSE i END FROM generate_series(1,10) i",
        ResultLimits {
            rows: 2,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(snapshot.rows, 2);
    assert_eq!(snapshot.cutoff, Some(ResultCutoff::Rows));
    let ResultPhase::Finished(ResultTermination::Failed(ExecutionError::Database(error))) =
        snapshot.phase
    else {
        panic!("native server failure expected")
    };
    assert_eq!(error.code, Some(NativeErrorCode::SqlState("22012".into())));
    session.ping_detailed().await.unwrap();
    session.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore = "requires PostgreSQL from docker-compose.test.yml"]
async fn transport_failure_finishes_cleanup_and_allows_reconnection() {
    let session = session(PG15).await;
    let pid = session
        .execute_detailed("SELECT pg_backend_pid()")
        .await
        .unwrap()
        .into_parts()
        .0
        .rows[0][0]
        .display()
        .to_owned();
    let (mut producer, mut reader) = start_results(ResultLimits::default());
    let running = session.clone();
    let task = tokio::spawn(async move {
        let outcome = running
            .execute_streamed(
                "SELECT i, repeat('x',9000), pg_sleep(0.1) FROM generate_series(1,100) i",
                &mut producer,
            )
            .await;
        producer.finish(outcome.clone().map(|_| ())).await.unwrap();
        outcome
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if reader.borrow_and_update().rows > 0 {
                break;
            }
            reader.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let control = PostgresqlAdapter
        .create(DatabaseConfig::from_connection_string(PG15))
        .unwrap();
    control.connect_detailed().await.unwrap();
    let terminated = control
        .execute_detailed(&format!("SELECT pg_terminate_backend({pid})"))
        .await
        .unwrap()
        .into_parts()
        .0;
    assert_eq!(terminated.rows[0][0], ResultCell::Text("true".into()));
    let error = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    let ExecutionError::Database(error) = error else {
        panic!("native termination diagnostic expected")
    };
    assert_eq!(error.code, Some(NativeErrorCode::SqlState("57P01".into())));
    assert!(session.ping_detailed().await.is_err());
    session.connect_detailed().await.unwrap();
    let restarted = capture(session.as_ref(), "SELECT 1", ResultLimits::default()).await;
    assert_eq!(
        restarted.phase,
        ResultPhase::Finished(ResultTermination::Complete)
    );
    session.disconnect().await.unwrap();
    control.disconnect().await.unwrap();
}
