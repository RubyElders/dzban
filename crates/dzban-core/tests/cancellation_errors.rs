use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use dzban_core::error::{DatabaseError, ErrorKind, ExecutionError, NativeErrorCode};
use dzban_core::execution::{QueryEngine, QueryPhase};
use dzban_core::{DatabaseBackend, QueryEstimate, QueryPlan, ResultSet};
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Default)]
struct Session {
    started: Notify,
    finish: Notify,
    cancel_started: Notify,
    cancel_finish: Notify,
    cancel_panics: bool,
    cancel_succeeds: bool,
    cancel_calls: std::sync::atomic::AtomicUsize,
}

fn diagnostic() -> ExecutionError {
    ExecutionError::Database(DatabaseError {
        backend: DatabaseBackend::Postgresql,
        kind: ErrorKind::Server,
        message: "Cancellation refused".into(),
        code: Some(NativeErrorCode::SqlState("42501".into())),
        detail: Some("Fixture detail".into()),
        hint: None,
    })
}

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(0)
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate: QueryEstimate::default(),
        })
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        self.started.notify_one();
        self.finish.notified().await;
        Ok(Default::default())
    }
    async fn cancel(&self) -> Result<(), String> {
        panic!("Legacy cancellation path")
    }
    async fn cancel_detailed(&self) -> Result<(), ExecutionError> {
        self.cancel_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.cancel_started.notify_one();
        assert!(!self.cancel_panics, "Fixture cancellation panic");
        self.cancel_finish.notified().await;
        if self.cancel_succeeds {
            Ok(())
        } else {
            Err(diagnostic())
        }
    }
}

#[tokio::test]
async fn cancellation_for_a_completed_query_cannot_cancel_its_successor() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let first = engine.start_detailed("first".into()).unwrap();
    let old_id = first.id;
    session.started.notified().await;
    session.finish.notify_one();
    first.wait_detailed().await.unwrap();

    let second = engine.start_detailed("second".into()).unwrap();
    session.started.notified().await;
    engine.cancel_detailed(old_id).await.unwrap();
    assert_eq!(
        session
            .cancel_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        engine.subscribe().borrow().phase,
        QueryPhase::Executing(second.id)
    );
    assert!(!second.is_finished());
    session.finish.notify_one();
    assert!(second.wait_detailed().await.is_ok());
}

#[tokio::test]
async fn canceled_execution_retains_its_execution_duration() {
    let session = Arc::new(Session {
        cancel_succeeds: true,
        ..Default::default()
    });
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let query = engine.start_detailed("query".into()).unwrap();
    session.started.notified().await;
    let id = query.id;
    session.cancel_finish.notify_one();
    engine.cancel_detailed(id).await.unwrap();
    session.finish.notify_one();
    assert!(matches!(
        query.wait_detailed().await,
        Err(ExecutionError::Canceled)
    ));
    let state = engine.subscribe().borrow().clone();
    assert_eq!(state.phase, QueryPhase::Canceled(id));
    assert!(state.execution_duration > std::time::Duration::ZERO);
    assert!(state.elapsed() >= state.planning_duration + state.execution_duration);
}

#[tokio::test]
async fn cancellation_worker_panic_recovers_the_active_phase_and_admission() {
    let session = Arc::new(Session {
        cancel_panics: true,
        ..Default::default()
    });
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let query = engine.start_detailed("query".into()).unwrap();
    session.started.notified().await;
    let error = engine.cancel_detailed(query.id).await.unwrap_err();
    assert!(
        matches!(error, ExecutionError::Message(message) if message.contains("Cancellation worker failed"))
    );
    assert_eq!(
        engine.subscribe().borrow().phase,
        QueryPhase::Executing(query.id)
    );
    session.finish.notify_one();
    assert!(query.wait_detailed().await.is_ok());
    let restart = engine.start_detailed("restart".into()).unwrap();
    session.started.notified().await;
    session.finish.notify_one();
    assert!(restart.wait_detailed().await.is_ok());
}

#[tokio::test]
async fn dropping_the_cancel_waiter_does_not_drop_control_delivery_or_recovery() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let query = engine.start_detailed("query".into()).unwrap();
    session.started.notified().await;
    let id = query.id;
    let observer_engine = engine.clone();
    let observer = tokio::spawn(async move { observer_engine.cancel_detailed(id).await });
    session.cancel_started.notified().await;
    observer.abort();
    assert!(observer.await.unwrap_err().is_cancelled());
    session.cancel_finish.notify_one();
    let mut state = engine.subscribe();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if state.borrow().phase == QueryPhase::Executing(id) {
                break;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .expect("Core must finish cancellation recovery after the observer is dropped");
    session.finish.notify_one();
    assert!(query.wait_detailed().await.is_ok());
}

#[tokio::test]
async fn cancel_failure_finishes_before_admitting_a_new_query() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let first = engine.start_detailed("first".into()).unwrap();
    session.started.notified().await;
    let id = first.id;
    let cancel_engine = engine.clone();
    let cancel = tokio::spawn(async move { cancel_engine.cancel_detailed(id).await });
    session.cancel_started.notified().await;
    session.finish.notify_one();
    assert!(matches!(
        engine.start_detailed("overlap".into()),
        Err(ExecutionError::Busy)
    ));
    session.cancel_finish.notify_one();
    assert_eq!(cancel.await.unwrap().unwrap_err(), diagnostic());
    assert!(first.wait_detailed().await.is_ok());
    let second = engine.start_detailed("second".into()).unwrap();
    session.started.notified().await;
    assert_eq!(
        engine.subscribe().borrow().phase,
        QueryPhase::Executing(second.id)
    );
    session.finish.notify_one();
    assert!(second.wait_detailed().await.is_ok());
}

#[tokio::test]
async fn failed_cancellation_restores_only_the_matching_active_query() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let query = engine.start_detailed("query".into()).unwrap();
    session.started.notified().await;
    session.cancel_finish.notify_one();
    assert_eq!(
        engine.cancel_detailed(query.id).await.unwrap_err(),
        diagnostic()
    );
    assert_eq!(
        engine.subscribe().borrow().phase,
        QueryPhase::Executing(query.id)
    );
    session.finish.notify_one();
    assert!(query.wait_detailed().await.is_ok());
}
