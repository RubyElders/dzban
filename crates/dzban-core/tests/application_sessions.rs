use std::sync::Arc;

use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::{
    ApplicationContext, ApplicationError, ApplicationPhase, ConnectionPhase,
};
use dzban_core::error::ExecutionError;
use dzban_core::execution::SessionPhase;
use dzban_core::{DatabaseBackend, DatabaseConfig, QueryPlan, ResultSet};

#[derive(Default)]
struct Session {
    canceled: tokio::sync::Notify,
    connection_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    fail_connect: bool,
    panic_connect: bool,
    fail_disconnect_once: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        assert!(!self.panic_connect, "Fixture connection panic");
        if let Some((entered, release)) = &self.connection_gate {
            entered.notify_one();
            release.notified().await;
        }
        if self.fail_connect {
            return Err("Fixture connection failure".into());
        }
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        if self
            .fail_disconnect_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(ExecutionError::Message("Fixture disconnect failure".into()));
        }
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(0)
    }
    async fn execute(&self, sql: &str) -> Result<ResultSet, String> {
        if sql == "wait" {
            self.canceled.notified().await;
            return Err("Canceled by fixture".into());
        }
        Ok(ResultSet::default())
    }
    async fn cancel(&self) -> Result<(), String> {
        self.canceled.notify_one();
        Ok(())
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        Err("No fixture plan".into())
    }
}

struct Factory;

impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(Arc::new(Session::default()))
    }
}

struct SessionFactory(Arc<Session>);

#[tokio::test]
async fn failed_request_cleanup_keeps_other_pending_connections_visible() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(SessionFactory(Arc::new(Session {
            connection_gate: Some((entered.clone(), release.clone())),
            ..Default::default()
        }))))
        .unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let profile =
        DatabaseConfig::from_connection_string("host=localhost user=fixture dbname=fixture");
    let connecting = context.start_connect(profile.clone());
    entered.notified().await;
    let mut unsupported = profile;
    unsupported.backend = DatabaseBackend::Clickhouse;
    let failed = context.start_connect(unsupported);
    let pending = context.subscribe_pending_connections();
    assert_eq!(pending.borrow().len(), 2);
    assert!(failed.wait().await.is_err());
    assert_eq!(pending.borrow().len(), 1);
    assert_eq!(pending.borrow()[0].request, connecting.id);
    assert_eq!(pending.borrow()[0].phase, ConnectionPhase::Connecting);
    release.notify_one();
    let engine = connecting.wait().await.unwrap();
    assert!(pending.borrow().is_empty());
    context.close(engine.id()).await.unwrap();
}

impl AdapterFactory for SessionFactory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn dropped_connection_waiter_does_not_lose_the_owned_request_or_session() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let session = Arc::new(Session {
        connection_gate: Some((entered.clone(), release.clone())),
        ..Default::default()
    });
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(SessionFactory(session)))
        .unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let handle = context.start_connect(DatabaseConfig::from_connection_string(
        "host=localhost user=fixture dbname=fixture",
    ));
    let request = handle.id;
    let mut state = handle.subscribe();
    let pending = context.subscribe_pending_connections();
    assert_eq!(pending.borrow().len(), 1);
    assert_eq!(pending.borrow()[0], *state.borrow());
    assert_eq!(state.borrow().request, request);
    assert_eq!(state.borrow().phase, ConnectionPhase::Connecting);
    tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    let waiter = tokio::spawn(handle.wait());
    waiter.abort();
    assert!(matches!(waiter.await, Err(error) if error.is_cancelled()));
    assert!(context.session_ids().await.is_empty());
    assert_eq!(pending.borrow().len(), 1);
    release.notify_one();
    let id = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let ConnectionPhase::Connected(id) = state.borrow_and_update().phase {
                break id;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(state.borrow().request, request);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        let mut pending = pending.clone();
        while !pending.borrow_and_update().is_empty() {
            pending.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(context.session_ids().await, [id]);
    context
        .session(id)
        .await
        .unwrap()
        .execute_detailed("SELECT 1")
        .await
        .unwrap();
    context.close(id).await.unwrap();
    assert!(context.session_ids().await.is_empty());
}

#[tokio::test]
async fn connection_failures_and_panics_publish_terminal_state_without_registering_sessions() {
    for panic_connect in [false, true] {
        let mut registry = AdapterRegistry::default();
        registry
            .register(Arc::new(SessionFactory(Arc::new(Session {
                panic_connect,
                fail_connect: !panic_connect,
                ..Default::default()
            }))))
            .unwrap();
        let context = Arc::new(ApplicationContext::new(registry));
        let handle = context.start_connect(DatabaseConfig::from_connection_string(
            "host=localhost user=fixture dbname=fixture",
        ));
        let request = handle.id;
        let state = handle.subscribe();
        let Err(error) = handle.wait().await else {
            panic!("Failed connection must not return a session");
        };
        assert_eq!(state.borrow().request, request);
        assert_eq!(state.borrow().phase, ConnectionPhase::Failed(error));
        assert!(context.session_ids().await.is_empty());
        assert!(context.subscribe_pending_connections().borrow().is_empty());
    }
}

#[tokio::test]
async fn context_owns_sessions_until_successful_close_and_rejects_unknown_ids() {
    let mut adapters = AdapterRegistry::default();
    adapters.register(Arc::new(Factory)).unwrap();
    let context = Arc::new(ApplicationContext::new(adapters));
    let profile =
        DatabaseConfig::from_connection_string("host=localhost user=fixture dbname=fixture");
    let first_request = context.start_connect(profile.clone());
    let second_request = context.start_connect(profile);
    let pending = context.subscribe_pending_connections();
    assert_eq!(pending.borrow().len(), 2);
    assert_ne!(first_request.id, second_request.id);
    let first_state = first_request.subscribe();
    let second_state = second_request.subscribe();
    let first = first_request.wait().await.unwrap();
    let second = second_request.wait().await.unwrap();
    assert!(pending.borrow().is_empty());
    assert_eq!(
        first_state.borrow().phase,
        ConnectionPhase::Connected(first.id())
    );
    assert_eq!(
        second_state.borrow().phase,
        ConnectionPhase::Connected(second.id())
    );
    assert_ne!(first.id(), second.id());
    assert!(Arc::ptr_eq(
        &context.session(first.id()).await.unwrap(),
        &first
    ));
    context.close(first.id()).await.unwrap();
    assert_eq!(*first.subscribe_session().borrow(), SessionPhase::Closed);
    assert!(
        matches!(context.session(first.id()).await, Err(ApplicationError::UnknownSession(id)) if id == first.id())
    );
    assert!(matches!(
        context.close(first.id()).await,
        Err(ApplicationError::UnknownSession(_))
    ));
    second.execute_detailed("SELECT 1").await.unwrap();
    context.close(second.id()).await.unwrap();
}

#[tokio::test]
async fn busy_close_retains_the_registered_session_and_query() {
    let mut adapters = AdapterRegistry::default();
    adapters.register(Arc::new(Factory)).unwrap();
    let context = Arc::new(ApplicationContext::new(adapters));
    let engine = context
        .connect_profile(DatabaseConfig::from_connection_string(
            "host=localhost user=fixture dbname=fixture",
        ))
        .await
        .unwrap();
    let query = engine.start_detailed("wait".into()).unwrap();
    let mut state = engine.subscribe();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if matches!(
                state.borrow_and_update().phase,
                dzban_core::execution::QueryPhase::Executing(_)
            ) {
                break;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        context.close(engine.id()).await,
        Err(ApplicationError::Operation(ExecutionError::Busy))
    ));
    assert!(Arc::ptr_eq(
        &context.session(engine.id()).await.unwrap(),
        &engine
    ));
    assert!(!query.is_finished());
    engine.cancel(query.id).await.unwrap();
    assert_eq!(
        query.wait_detailed().await.unwrap_err(),
        ExecutionError::Canceled
    );
    context.close(engine.id()).await.unwrap();
    assert!(matches!(
        context.session(engine.id()).await,
        Err(ApplicationError::UnknownSession(_))
    ));
}

#[tokio::test]
async fn shutdown_drains_pending_connections_even_when_its_waiter_is_dropped() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(SessionFactory(Arc::new(Session {
            connection_gate: Some((entered.clone(), release.clone())),
            ..Default::default()
        }))))
        .unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let profile =
        DatabaseConfig::from_connection_string("host=localhost user=fixture dbname=fixture");
    let connection = context.start_connect(profile.clone());
    entered.notified().await;
    let shutdown = context.start_shutdown().unwrap();
    let mut phase = context.subscribe_phase();
    assert_eq!(*phase.borrow(), ApplicationPhase::Draining);
    assert!(matches!(
        context.start_shutdown(),
        Err(ApplicationError::Operation(ExecutionError::Busy))
    ));
    assert!(matches!(
        context.start_connect(profile.clone()).wait().await,
        Err(ApplicationError::Operation(ExecutionError::Busy))
    ));
    assert_eq!(context.subscribe_pending_connections().borrow().len(), 1);
    drop(shutdown);
    release.notify_one();
    let engine = connection.wait().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while *phase.borrow_and_update() != ApplicationPhase::Closed {
            phase.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(context.session_ids().await.is_empty());
    assert!(context.subscribe_pending_connections().borrow().is_empty());
    assert_eq!(*engine.subscribe_session().borrow(), SessionPhase::Closed);
    assert!(matches!(
        context.start_connect(profile).wait().await,
        Err(ApplicationError::Operation(ExecutionError::Closed))
    ));
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn busy_shutdown_restores_admission_and_can_be_retried_after_cancellation() {
    let mut registry = AdapterRegistry::default();
    registry.register(Arc::new(Factory)).unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let engine = context
        .connect_profile(DatabaseConfig::from_connection_string(
            "host=localhost user=fixture dbname=fixture",
        ))
        .await
        .unwrap();
    let query = engine.start_detailed("wait".into()).unwrap();
    let mut state = engine.subscribe();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if matches!(
                state.borrow_and_update().phase,
                dzban_core::execution::QueryPhase::Executing(_)
            ) {
                break;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let interaction = dzban_core::interaction::SessionInteraction::from_engine(&engine);
    assert_eq!(
        interaction.execution,
        dzban_core::interaction::QueryExecutionState::Running { plan: None }
    );
    assert!(!interaction.query_available);
    let mut view = dzban_core::interaction::AppState::default();
    view.transition(dzban_core::interaction::AppEvent::SessionSelected(Some(
        interaction,
    )));
    view.transition(dzban_core::interaction::AppEvent::SessionSelected(None));
    assert_eq!(
        view.primary_action(),
        dzban_core::interaction::PrimaryAction::Unavailable
    );
    assert!(!query.is_finished());
    assert!(matches!(
        context.start_shutdown().unwrap().wait().await,
        Err(ApplicationError::Operation(ExecutionError::Busy))
    ));
    assert_eq!(
        *context.subscribe_phase().borrow(),
        ApplicationPhase::Running
    );
    assert_eq!(context.session_ids().await, [engine.id()]);
    assert!(!query.is_finished());
    engine.cancel_detailed(query.id).await.unwrap();
    assert_eq!(
        query.wait_detailed().await.unwrap_err(),
        ExecutionError::Canceled
    );
    context.start_shutdown().unwrap().wait().await.unwrap();
    assert_eq!(
        *context.subscribe_phase().borrow(),
        ApplicationPhase::Closed
    );
    assert!(context.session_ids().await.is_empty());
}

#[tokio::test]
async fn failed_shutdown_keeps_the_failed_session_available_for_retry() {
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(SessionFactory(Arc::new(Session {
            fail_disconnect_once: std::sync::atomic::AtomicBool::new(true),
            ..Default::default()
        }))))
        .unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let engine = context
        .connect_profile(DatabaseConfig::from_connection_string(
            "host=localhost user=fixture dbname=fixture",
        ))
        .await
        .unwrap();
    assert_eq!(
        context.start_shutdown().unwrap().wait().await.unwrap_err(),
        ApplicationError::Operation(ExecutionError::Message("Fixture disconnect failure".into()))
    );
    assert_eq!(
        *context.subscribe_phase().borrow(),
        ApplicationPhase::Running
    );
    assert_eq!(*engine.subscribe_session().borrow(), SessionPhase::Open);
    assert_eq!(context.session_ids().await, [engine.id()]);
    engine.execute_detailed("SELECT 1").await.unwrap();
    context.start_shutdown().unwrap().wait().await.unwrap();
    assert_eq!(
        *context.subscribe_phase().borrow(),
        ApplicationPhase::Closed
    );
    assert!(context.session_ids().await.is_empty());
}

#[tokio::test]
async fn workspace_retirement_requires_successful_session_cleanup() {
    use dzban_core::documents::{CloseDocument, DocumentLifecycle, QueryDocuments};
    use dzban_core::interaction::SessionInteraction;
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(SessionFactory(Arc::new(Session {
            fail_disconnect_once: std::sync::atomic::AtomicBool::new(true),
            ..Default::default()
        }))))
        .unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let engine = context
        .connect_profile(DatabaseConfig::from_connection_string(
            "host=localhost user=fixture dbname=fixture",
        ))
        .await
        .unwrap();
    let mut documents = QueryDocuments::default();
    let document = documents.open();
    document.set_draft("SELECT 1;".into());
    assert_eq!(
        documents.close_in_workspace(document.id()),
        CloseDocument::CloseWorkspace
    );
    assert_eq!(
        documents.retire_closed_session(&engine),
        Err(ExecutionError::Busy)
    );
    assert!(context.close(engine.id()).await.is_err());
    assert_eq!(document.snapshot().lifecycle, DocumentLifecycle::Open);
    assert_eq!(document.draft(), "SELECT 1;");
    assert!(SessionInteraction::from_engine(&engine).query_available);
    context.close(engine.id()).await.unwrap();
    assert!(!SessionInteraction::from_engine(&engine).query_available);
    documents.retire_closed_session(&engine).unwrap();
    assert!(documents.selected().is_none());
    assert!(documents.documents().is_empty());
    assert_eq!(document.snapshot().lifecycle, DocumentLifecycle::Closed);
    assert!(!document.set_draft("SELECT 2;".into()));
    assert!(context.session_ids().await.is_empty());
}
