use std::sync::Arc;

use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use dzban_core::error::ExecutionError;
use dzban_core::execution::{AuxiliaryHandle, OperationState, QueryEngine, SessionPhase};
use dzban_core::{DatabaseBackend, QueryPlan, ResultSet};

#[tokio::test]
async fn recorded_query_publishes_after_its_caller_drops_the_handle() {
    use dzban_core::execution::HistoryPublication;
    use dzban_core::history::QueryHistoryStore;
    let directory =
        std::env::temp_dir().join(format!("dzban-owned-history-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let history = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    let engine = Arc::new(QueryEngine::new(Arc::new(Session {
        fast_preview: true,
        ..Default::default()
    })));
    let mut publication = engine.subscribe_history();
    let query = engine
        .start_recorded_detailed("SELECT 1".into(), history)
        .unwrap();
    let id = query.id;
    assert_eq!(*publication.borrow(), HistoryPublication::Pending(id));
    drop(query);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while *publication.borrow_and_update() != HistoryPublication::Published(id) {
            publication.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    engine.close().await.unwrap();
    let history = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    assert_eq!(history.entries().len(), 1);
    assert_eq!(history.entries()[0].sql, "SELECT 1");
    assert!(matches!(
        history.entries()[0].outcome,
        dzban_core::history::QueryHistoryOutcome::Success { .. }
    ));
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn history_failure_does_not_replace_the_query_outcome() {
    use dzban_core::execution::HistoryPublication;
    use dzban_core::history::QueryHistoryStore;
    let directory =
        std::env::temp_dir().join(format!("dzban-history-failure-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let history = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    std::fs::write(path.with_extension("entries"), b"not a directory").unwrap();
    let engine = Arc::new(QueryEngine::new(Arc::new(Session {
        fast_preview: true,
        ..Default::default()
    })));
    let query = engine
        .start_recorded_detailed("SELECT 1".into(), history)
        .unwrap();
    let id = query.id;
    assert!(query.wait_detailed().await.is_ok());
    assert!(
        matches!(&*engine.subscribe_history().borrow(), HistoryPublication::Failed(active, _) if *active == id)
    );
    let publication = engine.subscribe_history().borrow().clone();
    assert!(publication.error_for(id).is_some());
    assert!(
        publication
            .error_for(dzban_core::execution::QueryId(uuid::Uuid::new_v4()))
            .is_none()
    );
    assert!(HistoryPublication::Pending(id).error_for(id).is_none());
    assert!(HistoryPublication::Published(id).error_for(id).is_none());
    assert_eq!(*engine.subscribe_operation().borrow(), OperationState::Idle);
    engine.close().await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn publication_keeps_session_admission_until_the_record_is_saved() {
    use dzban_core::execution::{HistoryPublication, QueryPhase};
    use dzban_core::history::QueryHistoryStore;
    let directory =
        std::env::temp_dir().join(format!("dzban-history-admission-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (release, blocked) = std::sync::mpsc::channel();
        let (entered, ready) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            blocked
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        });
        ready
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        let engine = Arc::new(QueryEngine::new(Arc::new(Session {
            fast_preview: true,
            ..Default::default()
        })));
        let mut state = engine.subscribe();
        let query = engine
            .start_recorded_detailed(
                "first".into(),
                QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap(),
            )
            .unwrap();
        let id = query.id;
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while state.borrow_and_update().phase != QueryPhase::Completed(id) {
                state.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(
            *engine.subscribe_history().borrow(),
            HistoryPublication::Pending(id)
        );
        assert!(!query.is_finished());
        assert!(matches!(
            engine.start_detailed("overlap".into()),
            Err(ExecutionError::Busy)
        ));
        assert_eq!(engine.close().await, Err(ExecutionError::Busy));
        release.send(()).unwrap();
        blocker.await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(3), query.wait_detailed())
                .await
                .unwrap()
                .is_ok()
        );
        assert_eq!(
            *engine.subscribe_history().borrow(),
            HistoryPublication::Published(id)
        );
        let history = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
        assert_eq!(history.entries().len(), 1);
        assert_eq!(history.entries()[0].sql, "first");
        assert!(engine.execute_detailed("restart").await.is_ok());
        engine.close().await.unwrap();
    });
    std::fs::remove_dir_all(directory).unwrap();
}

#[derive(Default)]
struct Session {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    panic_preview: bool,
    fast_preview: bool,
    transaction_status: Option<dzban_core::adapter::TransactionStatus>,
    panic_transaction_status: bool,
}

#[tokio::test]
async fn recorded_planning_cancellation_preserves_diagnostics_and_allows_restart() {
    use dzban_core::execution::{HistoryPublication, QueryPhase};
    use dzban_core::history::{QueryHistoryOutcome, QueryHistoryStore};
    let directory =
        std::env::temp_dir().join(format!("dzban-canceled-history-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let query = engine
        .start_recorded_detailed(
            "cancel".into(),
            QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap(),
        )
        .unwrap();
    let id = query.id;
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        session.entered.notified(),
    )
    .await
    .unwrap();
    engine.cancel_detailed(id).await.unwrap();
    session.release.notify_one();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(3), query.wait_detailed())
            .await
            .unwrap()
            .unwrap_err(),
        ExecutionError::Canceled
    );
    assert_eq!(
        *engine.subscribe_history().borrow(),
        HistoryPublication::Published(id)
    );
    let history = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    assert!(matches!(
        &history.entries()[0].outcome,
        QueryHistoryOutcome::Failure {
            diagnostic: Some(ExecutionError::Canceled),
            explain: Some(_),
            ..
        }
    ));
    let query = engine
        .start_recorded_detailed("restart".into(), history)
        .unwrap();
    assert!(matches!(
        engine.subscribe().borrow().phase,
        QueryPhase::Queued(_)
    ));
    assert!(engine.subscribe().borrow().execution_error.is_none());
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        session.entered.notified(),
    )
    .await
    .unwrap();
    session.release.notify_one();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(3), query.wait_detailed())
            .await
            .unwrap()
            .is_ok()
    );
    let history = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    assert_eq!(history.entries().len(), 2);
    assert_eq!(history.entries()[0].sql, "restart");
    let query = engine
        .start_recorded_detailed("cancel before planning".into(), history)
        .unwrap();
    engine.cancel_detailed(query.id).await.unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(3), query.wait_detailed())
            .await
            .unwrap()
            .unwrap_err(),
        ExecutionError::Canceled
    );
    let history =
        QueryHistoryStore::load_from_path(directory.join("history.json"), "fixture").unwrap();
    assert_eq!(history.entries().len(), 3);
    assert!(matches!(
        &history.entries()[0].outcome,
        QueryHistoryOutcome::Failure {
            diagnostic: Some(ExecutionError::Canceled),
            explain: None,
            ..
        }
    ));
    engine.close().await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn recorded_worker_panic_publishes_a_failure_and_releases_admission() {
    use dzban_core::execution::HistoryPublication;
    use dzban_core::history::{QueryHistoryOutcome, QueryHistoryStore};
    let directory =
        std::env::temp_dir().join(format!("dzban-panic-history-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let engine = Arc::new(QueryEngine::new(Arc::new(Session {
        fast_preview: true,
        panic_preview: true,
        ..Default::default()
    })));
    let query = engine
        .start_recorded_detailed(
            "panic".into(),
            QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap(),
        )
        .unwrap();
    let id = query.id;
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(3), query.wait_detailed())
            .await
            .unwrap(),
        Err(ExecutionError::Message(_))
    ));
    assert_eq!(
        *engine.subscribe_history().borrow(),
        HistoryPublication::Published(id)
    );
    assert_eq!(*engine.subscribe_operation().borrow(), OperationState::Idle);
    let history = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    assert_eq!(history.entries().len(), 1);
    assert!(
        matches!(&history.entries()[0].outcome, QueryHistoryOutcome::Failure { diagnostic: Some(ExecutionError::Message(error)), .. } if error.contains("Query worker failed"))
    );
    engine.close().await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

impl Session {
    async fn wait(&self) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}

#[async_trait]
impl DatabaseSession for Session {
    fn transaction_status(&self) -> Option<dzban_core::adapter::TransactionStatus> {
        assert!(!self.panic_transaction_status, "Fixture status panic");
        self.transaction_status
    }
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        Ok(())
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        Ok(Default::default())
    }
    async fn cancel(&self) -> Result<(), String> {
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        self.wait().await;
        Ok(1)
    }
    async fn version(&self) -> Result<String, String> {
        self.wait().await;
        Ok("fixture".into())
    }
    async fn explain(&self, sql: &str) -> Result<QueryPlan, String> {
        if !self.fast_preview {
            self.wait().await;
        }
        assert!(!self.panic_preview, "Fixture preview panic");
        if sql == "unavailable" {
            return Err("Fixture planning unavailable".into());
        }
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!({"sql": sql}),
            estimate: Default::default(),
        })
    }
}

#[tokio::test]
async fn delayed_consumers_receive_their_own_terminal_plan_and_diagnostics() {
    use dzban_core::execution::QueryPhase;
    let engine = Arc::new(QueryEngine::new(Arc::new(Session {
        fast_preview: true,
        ..Default::default()
    })));
    for sql in ["first", "unavailable"] {
        let first = engine.start_detailed(sql.into()).unwrap();
        let first_id = first.id;
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !first.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let next = engine.start_detailed("next".into()).unwrap();
        let next_id = next.id;
        let next = next.wait_completion().await.unwrap();
        let first = first.wait_completion().await.unwrap();
        assert!(first.result.is_ok());
        assert_eq!(first.snapshot.phase, QueryPhase::Completed(first_id));
        assert_eq!(next.snapshot.phase, QueryPhase::Completed(next_id));
        assert_eq!(
            engine.subscribe().borrow().phase,
            QueryPhase::Completed(next_id)
        );
        assert_eq!(
            next.snapshot.plan.unwrap().document,
            serde_json::json!({"sql": "next"})
        );
        assert!(next.snapshot.planning_error.is_none());
        if sql == "unavailable" {
            assert!(first.snapshot.plan.is_none());
            assert_eq!(
                first.snapshot.planning_error,
                Some(ExecutionError::Message(
                    "Fixture planning unavailable".into()
                ))
            );
        } else {
            assert_eq!(
                first.snapshot.plan.unwrap().document,
                serde_json::json!({"sql": sql})
            );
        }
        assert!(first.history.is_none());
    }
    engine.close().await.unwrap();
}

#[tokio::test]
async fn transaction_status_panic_preserves_query_result_and_history_publication() {
    use dzban_core::execution::HistoryPublication;
    use dzban_core::history::{QueryHistoryOutcome, QueryHistoryStore};
    let directory =
        std::env::temp_dir().join(format!("dzban-status-history-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let engine = Arc::new(QueryEngine::new(Arc::new(Session {
        fast_preview: true,
        panic_transaction_status: true,
        ..Default::default()
    })));
    let query = engine
        .start_recorded_detailed(
            "fixture".into(),
            QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap(),
        )
        .unwrap();
    let id = query.id;
    let completion = query.wait_completion().await.unwrap();
    assert!(completion.result.is_ok());
    assert_eq!(completion.transaction_status, None);
    assert_eq!(completion.history, Some(HistoryPublication::Published(id)));
    assert_eq!(*engine.subscribe_operation().borrow(), OperationState::Idle);
    let history = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    assert_eq!(history.entries().len(), 1);
    assert!(matches!(
        history.entries()[0].outcome,
        QueryHistoryOutcome::Success { .. }
    ));
    assert!(engine.execute_detailed("restart").await.is_ok());
    engine.close().await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn completion_preserves_known_transaction_status_without_inventing_one() {
    use dzban_core::adapter::TransactionStatus;
    for (status, failure) in [
        (None, false),
        (Some(TransactionStatus::Idle), false),
        (Some(TransactionStatus::InTransaction), false),
        (Some(TransactionStatus::FailedTransaction), true),
    ] {
        let engine = Arc::new(QueryEngine::new(Arc::new(Session {
            fast_preview: true,
            panic_preview: failure,
            transaction_status: status,
            ..Default::default()
        })));
        let completion = engine
            .start_detailed("fixture".into())
            .unwrap()
            .wait_completion()
            .await
            .unwrap();
        assert_eq!(completion.transaction_status, status);
        assert_eq!(completion.result.is_err(), failure);
        engine.close().await.unwrap();
    }
}

#[tokio::test]
async fn auxiliary_worker_panic_releases_the_observable_operation_state() {
    let session = Arc::new(Session {
        panic_preview: true,
        ..Default::default()
    });
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let state = engine.subscribe_operation();
    let preview = engine.start_preview("SELECT 1".into()).unwrap();
    assert_eq!(*state.borrow(), OperationState::Preview);
    session.entered.notified().await;
    session.release.notify_one();
    assert!(matches!(
        preview.wait().await,
        Err(ExecutionError::Message(_))
    ));
    assert_eq!(*state.borrow(), OperationState::Idle);
    engine.close().await.unwrap();
}

async fn assert_owned_admission<T>(
    engine: Arc<QueryEngine>,
    session: Arc<Session>,
    request: AuxiliaryHandle<T>,
) {
    let operation = engine.subscribe_operation();
    let active = *operation.borrow();
    assert!(matches!(
        active,
        OperationState::Preview | OperationState::Ping | OperationState::Version
    ));
    assert!(matches!(engine.start_ping(), Err(ExecutionError::Busy)));
    assert!(matches!(engine.start_version(), Err(ExecutionError::Busy)));
    assert!(matches!(
        engine.start_preview("SELECT 1".into()),
        Err(ExecutionError::Busy)
    ));
    assert!(matches!(
        engine.start_detailed("SELECT 1".into()),
        Err(ExecutionError::Busy)
    ));
    assert!(matches!(engine.start_close(), Err(ExecutionError::Busy)));
    session.entered.notified().await;
    drop(request);
    assert_eq!(*operation.borrow(), active);
    assert!(matches!(engine.start_close(), Err(ExecutionError::Busy)));
    session.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            match engine.start_close() {
                Ok(close) => {
                    close.wait().await.unwrap();
                    break;
                }
                Err(ExecutionError::Busy) => tokio::task::yield_now().await,
                Err(error) => panic!("Unexpected close failure: {error}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(*engine.subscribe_session().borrow(), SessionPhase::Closed);
    assert_eq!(*operation.borrow(), OperationState::Idle);
    assert!(matches!(engine.start_ping(), Err(ExecutionError::Closed)));
    assert!(matches!(
        engine.start_version(),
        Err(ExecutionError::Closed)
    ));
    assert!(matches!(
        engine.start_preview("SELECT 1".into()),
        Err(ExecutionError::Closed)
    ));
}

#[tokio::test]
async fn query_operation_state_retains_identity_until_owned_work_finishes() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let mut state = engine.subscribe_operation();
    assert_eq!(*state.borrow(), OperationState::Idle);
    let query = engine.start_detailed("SELECT 1".into()).unwrap();
    let id = query.id;
    assert_eq!(*state.borrow(), OperationState::Query(id));
    session.entered.notified().await;
    drop(query);
    assert_eq!(*state.borrow(), OperationState::Query(id));
    assert_eq!(
        *engine.subscribe_operation().borrow(),
        OperationState::Query(id)
    );
    session.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while *state.borrow_and_update() != OperationState::Idle {
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    engine.close().await.unwrap();
    assert_eq!(*state.borrow(), OperationState::Idle);
}

#[tokio::test]
async fn auxiliary_start_handles_admit_synchronously_and_retain_owned_work() {
    for kind in ["plan", "ping", "version"] {
        let session = Arc::new(Session::default());
        let engine = Arc::new(QueryEngine::new(session.clone()));
        match kind {
            "plan" => {
                assert_owned_admission(
                    engine.clone(),
                    session,
                    engine.start_preview("SELECT 1".into()).unwrap(),
                )
                .await
            }
            "ping" => {
                assert_owned_admission(engine.clone(), session, engine.start_ping().unwrap()).await
            }
            _ => {
                assert_owned_admission(engine.clone(), session, engine.start_version().unwrap())
                    .await
            }
        }
    }
}
