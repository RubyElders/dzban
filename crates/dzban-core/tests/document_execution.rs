use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use dzban_core::documents::{CloseDocument, DocumentExecution, QueryDocuments};
use dzban_core::error::ExecutionError;
use dzban_core::execution::QueryEngine;
use dzban_core::result_store::ResultLimits;
use dzban_core::result_stream::ResultSink;
use dzban_core::{
    CommandAcknowledgment, DatabaseBackend, QueryPlan, ResultCell, ResultColumn, ResultSet,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Session {
    ready: Notify,
    release: Notify,
    denied: AtomicBool,
    plans: AtomicUsize,
    cancellations: AtomicUsize,
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
        self.cancellations.fetch_add(1, Ordering::Relaxed);
        if self.denied.load(Ordering::Relaxed) {
            return Err("Fixture cancel denied".into());
        }
        self.release.notify_one();
        Ok(())
    }
    async fn explain(&self, sql: &str) -> Result<QueryPlan, String> {
        self.plans.fetch_add(1, Ordering::Relaxed);
        if sql == "planning" {
            self.ready.notify_one();
            self.release.notified().await;
        }
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate: Default::default(),
        })
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        panic!("document execution must stream");
    }
    async fn execute_streamed(
        &self,
        sql: &str,
        sink: &mut ResultSink,
    ) -> Result<Option<CommandAcknowledgment>, ExecutionError> {
        assert_ne!(sql, "panic", "Fixture worker panic");
        if sql == "failure" {
            return Err(ExecutionError::Message("Fixture failure".into()));
        }
        if sql == "command" {
            return Ok(Some(CommandAcknowledgment {
                affected_rows: Some(4),
            }));
        }
        sink.schema(vec![ResultColumn {
            name: "value".into(),
            type_name: "text".into(),
        }])
        .await?;
        if sql != "zero" {
            sink.row(vec![ResultCell::Text("žluť\nfirst".into())])
                .await?;
            sink.row(vec![ResultCell::Null]).await?;
        }
        if sql == "wait" {
            self.ready.notify_one();
            self.release.notified().await;
        }
        Ok(None)
    }
}

fn limits() -> ResultLimits {
    ResultLimits {
        memory_rows: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn empty_submission_does_not_begin_planning_or_replace_output() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let mut documents = QueryDocuments::default();
    let document = documents.open();
    assert!(
        document
            .submit(engine, " \n\t".into(), limits(), None)
            .is_err()
    );
    assert_eq!(session.plans.load(Ordering::Relaxed), 0);
    assert_eq!(document.snapshot().output_generation, 0);
    assert!(!document.is_active());
    assert!(document.snapshot().completion.is_none());
}

#[tokio::test]
async fn core_retains_early_and_completed_results_after_all_frontend_observers_drop() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let mut documents = QueryDocuments::default();
    let document = documents.open();
    let directory =
        std::env::temp_dir().join(format!("dzban-document-history-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let history =
        dzban_core::history::QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    let handle = document
        .submit(engine.clone(), "wait".into(), limits(), Some(history))
        .unwrap();
    let id = handle.query;
    let mut state = document.subscribe();
    session.ready.notified().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while state
            .borrow()
            .operation
            .as_ref()
            .and_then(|op| op.capture.as_ref())
            .is_none_or(|capture| capture.rows != 2)
        {
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let capture = state
        .borrow()
        .operation
        .as_ref()
        .unwrap()
        .capture
        .clone()
        .unwrap();
    assert_eq!(
        capture
            .result
            .as_ref()
            .unwrap()
            .read_page(0, 2)
            .await
            .unwrap()
            .rows[1][0],
        ResultCell::Null
    );
    assert_eq!(documents.close(document.id()), CloseDocument::Busy);
    assert!(matches!(
        document.submit(engine.clone(), "overlap".into(), limits(), None),
        Err(ExecutionError::Busy)
    ));
    drop(state);
    drop(handle);
    session.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while document.is_active() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let completed = document.snapshot();
    assert_eq!(completed.execution, DocumentExecution::Finished(id));
    assert_eq!(completed.session, Some(engine.id()));
    let completion = completed.completion.unwrap().unwrap();
    assert_eq!(completion.result.as_ref().unwrap().capture.rows, 2);
    assert_eq!(session.plans.load(Ordering::Relaxed), 1);
    let history = dzban_core::history::QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    assert_eq!(history.entries().len(), 1);
    assert!(matches!(
        history.entries()[0].outcome,
        dzban_core::history::QueryHistoryOutcome::Success {
            explain: Some(_),
            ..
        }
    ));
    let reader = completion
        .result
        .as_ref()
        .unwrap()
        .capture
        .result
        .clone()
        .unwrap();
    assert_eq!(
        documents.close(document.id()),
        CloseDocument::Closed { last: true }
    );
    engine.close().await.unwrap();
    assert_eq!(
        reader.read_page(0, 2).await.unwrap().rows[0][0],
        ResultCell::Text("žluť\nfirst".into())
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn planning_cancel_survives_dropped_waiter_and_allows_restart() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let mut documents = QueryDocuments::default();
    let document = documents.open();
    let query = document
        .submit(engine.clone(), "planning".into(), limits(), None)
        .unwrap();
    session.ready.notified().await;
    drop(document.request_cancel(engine.clone()).unwrap());
    let completion = query.wait().await.unwrap();
    assert!(matches!(completion.result, Err(ExecutionError::Canceled)));
    assert!(!document.is_active());
    let restarted = document
        .submit(engine, "command".into(), limits(), None)
        .unwrap();
    let completion = restarted.wait().await.unwrap();
    assert_eq!(
        completion.result.unwrap().command.unwrap().affected_rows,
        Some(4)
    );
}

#[tokio::test]
async fn cancel_denial_and_wrong_session_do_not_release_the_running_document() {
    let session = Arc::new(Session::default());
    session.denied.store(true, Ordering::Relaxed);
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let mut documents = QueryDocuments::default();
    let document = documents.open();
    let query = document
        .submit(engine.clone(), "wait".into(), limits(), None)
        .unwrap();
    session.ready.notified().await;
    let other = Arc::new(QueryEngine::new(Arc::new(Session::default())));
    assert!(document.request_cancel(other).is_err());
    assert_eq!(session.cancellations.load(Ordering::Relaxed), 0);
    assert!(
        document
            .request_cancel(engine)
            .unwrap()
            .wait()
            .await
            .is_err()
    );
    assert!(document.is_active());
    assert_eq!(documents.close(document.id()), CloseDocument::Busy);
    session.release.notify_one();
    query.wait().await.unwrap().result.unwrap();
    assert!(!document.is_active());
}

#[tokio::test]
async fn older_document_completion_cannot_reset_a_newer_session_operation_hint() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let mut documents = QueryDocuments::default();
    let first = documents.open();
    first
        .submit(engine.clone(), "zero".into(), limits(), None)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let old = first.snapshot();
    let second = documents.open();
    let running = second
        .submit(engine.clone(), "wait".into(), limits(), None)
        .unwrap();
    session.ready.notified().await;
    let current = engine.subscribe().borrow().clone();
    assert!(
        dzban_core::interaction::AppEvent::scoped_document_execution(&old, Some(current.scope()))
            .is_none()
    );
    assert!(
        dzban_core::interaction::AppEvent::scoped_document_execution(
            &second.snapshot(),
            Some(current.scope())
        )
        .is_some()
    );
    session.release.notify_one();
    running.wait().await.unwrap();
}

#[tokio::test]
async fn failure_panic_zero_rows_and_replaced_output_keep_terminal_metadata() {
    let engine = Arc::new(QueryEngine::new(Arc::new(Session::default())));
    let mut documents = QueryDocuments::default();
    let document = documents.open();
    for sql in ["failure", "panic", "zero", "command"] {
        let query = document
            .submit(engine.clone(), sql.into(), limits(), None)
            .unwrap();
        let generation = query.generation;
        document.replace_output();
        let completion = query.wait().await.unwrap();
        assert!(!document.accepts_output(generation));
        assert!(!document.is_active());
        let snapshot = document.snapshot();
        assert_eq!(snapshot.execution.query(), completion.snapshot.phase.id());
        let retained = snapshot.completion.unwrap().unwrap();
        assert_eq!(retained.snapshot.phase, completion.snapshot.phase);
        match sql {
            "zero" => assert_eq!(retained.result.as_ref().unwrap().capture.rows, 0),
            "command" => assert_eq!(
                retained
                    .result
                    .as_ref()
                    .unwrap()
                    .command
                    .as_ref()
                    .unwrap()
                    .affected_rows,
                Some(4)
            ),
            _ => assert!(retained.result.is_err()),
        }
    }
}
