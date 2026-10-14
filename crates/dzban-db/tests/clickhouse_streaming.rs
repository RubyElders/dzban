use dzban_core::adapter::{AdapterFactory, DatabaseSession};
use dzban_core::error::{ExecutionError, NativeErrorCode};
use dzban_core::result_store::{ResultCutoff, ResultLimits};
use dzban_core::result_stream::{ResultPhase, ResultSnapshot, ResultTermination, start_results};
use dzban_core::{DatabaseBackend, DatabaseConfig, ResultCell};
use dzban_db::adapters::ClickhouseAdapter;

fn session() -> std::sync::Arc<dyn DatabaseSession> {
    ClickhouseAdapter
        .create(DatabaseConfig {
            name: "stream-fixture".into(),
            backend: DatabaseBackend::Clickhouse,
            host: None,
            port: None,
            user: "dzban".into(),
            password: Some("dzban-test".into()),
            database: "dzban_test".into(),
            url: Some("http://127.0.0.1:58123".into()),
        })
        .unwrap()
}

async fn capture(session: &dyn DatabaseSession, sql: &str, limits: ResultLimits) -> ResultSnapshot {
    let (mut sink, reader) = start_results(limits);
    let result = session.execute_streamed(sql, &mut sink).await;
    sink.finish(result.map(|_| ())).await.unwrap();
    reader.borrow().clone()
}

#[tokio::test]
#[ignore = "requires ClickHouse from docker-compose.test.yml"]
async fn http_stream_preserves_values_cutoff_empty_schema_and_restart() {
    let session = session();
    session.connect_detailed().await.unwrap();
    let snapshot = capture(
        session.as_ref(),
        "SELECT number AS id, NULL AS missing, 'žluť\n' AS value FROM numbers(50)",
        ResultLimits {
            rows: 3,
            memory_rows: 1,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        snapshot.phase,
        ResultPhase::Finished(ResultTermination::Limited(ResultCutoff::Rows))
    );
    assert_eq!(snapshot.rows, 3);
    let page = snapshot.result.unwrap().read_page(0, 3).await.unwrap();
    assert_eq!(page.columns[0].name, "id");
    assert_eq!(page.rows[0][1], ResultCell::Null);
    assert_eq!(page.rows[0][2], ResultCell::Text("žluť\n".into()));
    assert_eq!(page.rows[2][0].display(), "2");
    let empty = capture(
        session.as_ref(),
        "SELECT 1 AS id WHERE false",
        ResultLimits::default(),
    )
    .await;
    assert_eq!(empty.rows, 0);
    assert_eq!(
        empty.result.unwrap().read_page(0, 1).await.unwrap().columns[0].name,
        "id"
    );
    let failed = capture(
        session.as_ref(),
        "SELECT * FROM dzban_stream_missing_table",
        ResultLimits::default(),
    )
    .await;
    assert!(
        matches!(failed.phase, ResultPhase::Finished(ResultTermination::Failed(ExecutionError::Database(ref error))) if error.code == Some(NativeErrorCode::ClickHouse(60)))
    );
    session.ping_detailed().await.unwrap();
    let restarted = capture(session.as_ref(), "SELECT 42", ResultLimits::default()).await;
    assert_eq!(
        restarted.phase,
        ResultPhase::Finished(ResultTermination::Complete)
    );
    assert_eq!(
        restarted
            .result
            .unwrap()
            .read_page(0, 1)
            .await
            .unwrap()
            .rows[0][0]
            .display(),
        "42"
    );
}

#[tokio::test]
#[ignore = "requires ClickHouse from docker-compose.test.yml"]
async fn early_rows_remain_readable_while_draining_and_canceling() {
    let session = session();
    let (mut sink, mut reader) = start_results(ResultLimits {
        rows: 2,
        memory_rows: 1,
        ..Default::default()
    });
    let running = session.clone();
    let task = tokio::spawn(async move {
        let outcome = running.execute_streamed("SELECT number, repeat('x', 9000), sleepEachRow(0.1) FROM numbers(100) SETTINGS max_block_size=1, max_threads=1", &mut sink).await;
        sink.finish(outcome.clone().map(|_| ())).await.unwrap();
        outcome
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let phase = reader.borrow().phase.clone();
            if matches!(phase, ResultPhase::Draining(_)) {
                break;
            }
            assert!(
                !matches!(phase, ResultPhase::Finished(_)),
                "query ended before incremental delivery: {phase:?}"
            );
            reader.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    let result = reader.borrow().result.clone().unwrap();
    assert_eq!(
        result.read_page(0, 2).await.unwrap().rows[1][0].display(),
        "1"
    );
    session.cancel_detailed().await.unwrap();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(outcome, Err(ExecutionError::Canceled))
            || matches!(outcome, Err(ExecutionError::Database(ref native)) if native.code == Some(NativeErrorCode::ClickHouse(394))),
        "unexpected cancel outcome: {outcome:?}"
    );
    session.ping_detailed().await.unwrap();
    assert!(session.cancel_detailed().await.is_err());
}
