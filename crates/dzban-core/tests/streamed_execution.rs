use std::sync::Arc;

use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use dzban_core::error::ExecutionError;
use dzban_core::execution::{HistoryPublication, QueryEngine, QueryPhase};
use dzban_core::history::{QueryHistoryOutcome, QueryHistoryStore};
use dzban_core::result_store::{ResultCutoff, ResultLimits};
use dzban_core::result_stream::{Delivery, ResultPhase, ResultSink, ResultTermination};
use dzban_core::{
    CommandAcknowledgment, DatabaseBackend, QueryEstimate, QueryPlan, ResultCell, ResultColumn,
    ResultSet,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Session {
    ready: Notify,
    finish: Notify,
    plans: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(1)
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn cancel(&self) -> Result<(), String> {
        self.finish.notify_one();
        Ok(())
    }
    async fn explain(&self, sql: &str) -> Result<QueryPlan, String> {
        self.plans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if sql == "planning" {
            self.ready.notify_one();
            self.finish.notified().await;
        }
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!({"fixture": true}),
            estimate: QueryEstimate::default(),
        })
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        panic!("collected execution must not be used");
    }
    async fn execute_streamed(
        &self,
        sql: &str,
        results: &mut ResultSink,
    ) -> Result<Option<CommandAcknowledgment>, ExecutionError> {
        assert_ne!(sql, "panic", "stream fixture panic");
        if sql == "command" {
            return Ok(Some(CommandAcknowledgment {
                affected_rows: Some(5),
            }));
        }
        results
            .schema(vec![ResultColumn {
                name: "value".into(),
                type_name: "text".into(),
            }])
            .await?;
        if sql == "invalid" {
            let _ = results.row(vec![]).await;
            return Ok(None);
        }
        for index in 0..5 {
            if matches!(
                results
                    .row(vec![if index == 1 {
                        ResultCell::Null
                    } else {
                        ResultCell::Text(format!("žluť\n{index}"))
                    }])
                    .await?,
                Delivery::Stop(_)
            ) {
                break;
            }
        }
        if sql == "wait" {
            self.ready.notify_one();
            self.finish.notified().await;
        }
        if sql == "late" {
            return Err(ExecutionError::Message("late native error".into()));
        }
        Ok(None)
    }
}

fn limits() -> ResultLimits {
    ResultLimits {
        rows: 3,
        memory_rows: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn early_pages_keep_admission_until_cleanup_and_preserve_terminal_state() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let query = engine
        .start_streamed("wait".into(), limits(), None)
        .unwrap();
    session.ready.notified().await;
    assert_eq!(
        query.results.borrow().phase,
        ResultPhase::Draining(ResultCutoff::Rows)
    );
    let reader = query.results.borrow().result.clone().unwrap();
    assert_eq!(
        reader.read_page(0, 3).await.unwrap().rows[1][0],
        ResultCell::Null
    );
    assert!(!query.is_finished());
    assert!(matches!(
        engine.start_streamed("overlap".into(), limits(), None),
        Err(ExecutionError::Busy)
    ));
    session.finish.notify_one();
    let completion = query.wait_completion().await.unwrap();
    assert!(matches!(
        completion.snapshot.phase,
        QueryPhase::Completed(_)
    ));
    assert_eq!(
        completion.snapshot.capture.unwrap().phase,
        ResultPhase::Finished(ResultTermination::Limited(ResultCutoff::Rows))
    );
    assert_eq!(completion.result.unwrap().capture.rows, 3);
    assert_eq!(session.plans.load(std::sync::atomic::Ordering::Relaxed), 1);
    engine.ping_detailed().await.unwrap();
    engine.close().await.unwrap();
    assert!(engine.subscribe().borrow().capture.is_none());
    assert_eq!(reader.read_page(2, 1).await.unwrap().rows.len(), 1);
}

#[tokio::test]
async fn canceled_observers_do_not_release_running_queries_and_restart_is_clean() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let query = engine
        .start_streamed("wait".into(), limits(), None)
        .unwrap();
    let id = query.id;
    let mut result_state = query.results.clone();
    drop(query);
    session.ready.notified().await;
    assert_eq!(
        engine.ping_detailed().await.unwrap_err(),
        ExecutionError::Busy
    );
    engine.cancel_detailed(id).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !matches!(
            result_state.borrow().phase,
            ResultPhase::Finished(ResultTermination::Canceled)
        ) {
            result_state.changed().await.unwrap();
        }
        while engine.ping_detailed().await.is_err() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let completion = engine
        .start_streamed("restart".into(), limits(), None)
        .unwrap()
        .wait_completion()
        .await
        .unwrap();
    assert_eq!(completion.result.unwrap().capture.rows, 3);
}

#[tokio::test]
async fn planning_cancel_worker_panic_storage_failure_and_late_errors_are_terminal() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let query = engine
        .start_streamed("planning".into(), limits(), None)
        .unwrap();
    session.ready.notified().await;
    engine.cancel_detailed(query.id).await.unwrap();
    let completion = query.wait_completion().await.unwrap();
    assert!(matches!(completion.result, Err(ExecutionError::Canceled)));
    assert_eq!(completion.snapshot.capture.unwrap().rows, 0);
    for sql in ["panic", "invalid", "late"] {
        let completion = engine
            .start_streamed(sql.into(), limits(), None)
            .unwrap()
            .wait_completion()
            .await
            .unwrap();
        assert!(completion.result.is_err(), "{sql}");
        assert!(matches!(
            completion.snapshot.phase,
            QueryPhase::Failed(_, _)
        ));
        assert!(matches!(
            completion.snapshot.capture.unwrap().phase,
            ResultPhase::Finished(ResultTermination::Failed(_))
        ));
        engine.ping_detailed().await.unwrap();
    }
}

#[tokio::test]
async fn shared_history_records_bounded_sample_plan_cutoff_and_commands() {
    let directory =
        std::env::temp_dir().join(format!("dzban-stream-history-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let engine = Arc::new(QueryEngine::new(Arc::new(Session::default())));
    for sql in ["rows", "command"] {
        let store = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
        let completion = engine
            .start_streamed(sql.into(), limits(), Some(store))
            .unwrap()
            .wait_completion()
            .await
            .unwrap();
        assert!(completion.result.is_ok());
        assert!(matches!(
            completion.history,
            Some(HistoryPublication::Published(_))
        ));
    }
    let store = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    let rows = store
        .entries()
        .iter()
        .find(|entry| entry.sql == "rows")
        .unwrap();
    let QueryHistoryOutcome::Success {
        cutoff,
        result: Some(sample),
        explain,
        ..
    } = &rows.outcome
    else {
        panic!("expected stored rows");
    };
    assert_eq!(*cutoff, Some(ResultCutoff::Rows));
    assert_eq!(sample.total_rows, 3);
    assert_eq!(sample.rows.len(), 3);
    assert!(explain.is_some());
    let command = store
        .entries()
        .iter()
        .find(|entry| entry.sql == "command")
        .unwrap();
    assert!(matches!(
        command.outcome,
        QueryHistoryOutcome::Success {
            command: Some(CommandAcknowledgment {
                affected_rows: Some(5)
            }),
            ..
        }
    ));
    std::fs::remove_dir_all(directory).unwrap();
}
