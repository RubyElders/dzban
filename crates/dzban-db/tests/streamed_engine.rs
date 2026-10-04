use std::sync::Arc;
use std::time::Duration;

use dzban_core::adapter::{AdapterFactory, DatabaseSession};
use dzban_core::error::ExecutionError;
use dzban_core::execution::{QueryEngine, QueryPhase};
use dzban_core::result_store::{ResultCutoff, ResultLimits};
use dzban_core::result_stream::{ResultPhase, ResultTermination};
use dzban_core::{DatabaseBackend, DatabaseConfig};
use dzban_db::adapters::{ClickhouseAdapter, PostgresqlAdapter};

fn sessions() -> [(Arc<dyn DatabaseSession>, &'static str, &'static str); 2] {
    [
        (
            PostgresqlAdapter
                .create(DatabaseConfig::from_connection_string(
                    "host=localhost port=5415 user=testuser password=testpass dbname=testdb",
                ))
                .unwrap(),
            "SELECT i FROM generate_series(1, 50) i",
            "SELECT i, repeat('x', 9000), pg_sleep(0.1) FROM generate_series(1,100) i",
        ),
        (
            ClickhouseAdapter
                .create(DatabaseConfig {
                    name: "engine-fixture".into(),
                    backend: DatabaseBackend::Clickhouse,
                    host: None,
                    port: None,
                    user: "dzban".into(),
                    password: Some("dzban-test".into()),
                    database: "dzban_test".into(),
                    url: Some("http://127.0.0.1:58123".into()),
                })
                .unwrap(),
            "SELECT number + 1 AS i FROM numbers(50)",
            "SELECT number + 1 AS i, repeat('x', 9000), sleepEachRow(0.1) FROM numbers(100) SETTINGS max_block_size=1, max_threads=1",
        ),
    ]
}

#[tokio::test]
#[ignore = "requires PostgreSQL 15 and ClickHouse from docker-compose.test.yml"]
async fn native_adapters_publish_bounded_engine_results_with_plans() {
    for (session, sql, _) in sessions() {
        session.connect_detailed().await.unwrap();
        let engine = Arc::new(QueryEngine::new(session));
        let completion = engine
            .start_streamed(
                sql.into(),
                ResultLimits {
                    rows: 3,
                    memory_rows: 1,
                    ..Default::default()
                },
                None,
            )
            .unwrap()
            .wait_completion()
            .await
            .unwrap();
        assert!(matches!(
            completion.snapshot.phase,
            QueryPhase::Completed(_)
        ));
        assert!(
            completion.snapshot.plan.is_some(),
            "{sql}: {:?}",
            completion.snapshot.planning_error
        );
        let result = completion.result.unwrap();
        assert_eq!(result.capture.rows, 3);
        assert_eq!(
            result.capture.phase,
            ResultPhase::Finished(ResultTermination::Limited(ResultCutoff::Rows))
        );
        let page = result
            .capture
            .result
            .unwrap()
            .read_page(0, 3)
            .await
            .unwrap();
        assert_eq!(page.rows[0][0].display(), "1");
        assert_eq!(page.rows[2][0].display(), "3");
        engine.close().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires PostgreSQL 15 and ClickHouse from docker-compose.test.yml"]
async fn engine_cancel_during_native_draining_is_terminal_and_restartable() {
    for (session, sql, slow) in sessions() {
        session.connect_detailed().await.unwrap();
        let engine = Arc::new(QueryEngine::new(session));
        let query = engine
            .start_streamed(
                slow.into(),
                ResultLimits {
                    rows: 2,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let id = query.id;
        let mut updates = query.results.clone();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let phase = updates.borrow().phase.clone();
                if matches!(phase, ResultPhase::Draining(_)) {
                    break;
                }
                assert!(
                    !matches!(phase, ResultPhase::Finished(_)),
                    "ended before early rows: {phase:?}"
                );
                updates.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(
            engine.ping_detailed().await.unwrap_err(),
            ExecutionError::Busy
        );
        engine.cancel_detailed(id).await.unwrap();
        let completion = tokio::time::timeout(Duration::from_secs(5), query.wait_completion())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(completion.result, Err(ExecutionError::Canceled)));
        assert_eq!(completion.snapshot.phase, QueryPhase::Canceled(id));
        assert_eq!(
            completion.snapshot.capture.unwrap().phase,
            ResultPhase::Finished(ResultTermination::Canceled)
        );
        let restarted = engine
            .start_streamed(sql.into(), ResultLimits::default(), None)
            .unwrap()
            .wait_completion()
            .await
            .unwrap();
        assert_eq!(restarted.result.unwrap().capture.rows, 50);
        engine.close().await.unwrap();
    }
}
