use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::{ApplicationContext, ApplicationPhase, ConnectionSelection};
use dzban_core::documents::{CloseDocument, DocumentLifecycle, QueryDocument, QueryDocuments};
use dzban_core::error::ExecutionError;
use dzban_core::history::QueryHistoryStore;
use dzban_core::interaction::{
    AppCommand, AppEvent, AppState, InputRole, PrimaryAction, SessionInteraction,
};
use dzban_core::preview::{PlanPreviewService, PreviewPhase};
use dzban_core::result_store::ResultLimits;
use dzban_core::result_stream::ResultSink;
use dzban_core::{DatabaseBackend, DatabaseConfig, QueryPlan, ResultCell, ResultColumn, ResultSet};
use tokio::sync::Notify;

#[derive(Default)]
struct Fixture {
    ready: Notify,
    release: Notify,
    canceled: Notify,
    plans: AtomicUsize,
}

#[async_trait]
impl DatabaseSession for Fixture {
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
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        Err("Use streaming".into())
    }
    async fn cancel(&self) -> Result<(), String> {
        self.canceled.notify_one();
        Ok(())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        self.plans.fetch_add(1, Ordering::SeqCst);
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!({"fixture": true}),
            estimate: dzban_core::QueryEstimate {
                rows: Some(500),
                ..Default::default()
            },
        })
    }
    async fn execute_streamed(
        &self,
        sql: &str,
        sink: &mut ResultSink,
    ) -> Result<Option<dzban_core::CommandAcknowledgment>, ExecutionError> {
        sink.schema(vec![
            ResultColumn {
                name: "id".into(),
                type_name: "integer".into(),
            },
            ResultColumn {
                name: "value".into(),
                type_name: "text".into(),
            },
        ])
        .await?;
        for id in 0..500 {
            sink.row(vec![
                ResultCell::Text(id.to_string()),
                if id == 1 {
                    ResultCell::Null
                } else {
                    ResultCell::Text("žluť\nNULL".into())
                },
            ])
            .await?;
            if id == 4 && sql != "rows" {
                self.ready.notify_one();
                if sql == "wait" {
                    self.canceled.notified().await;
                    return Err(ExecutionError::Canceled);
                }
                self.release.notified().await;
            }
        }
        Ok(None)
    }
}

struct Factory(Arc<Fixture>);
impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(self.0.clone())
    }
}

async fn finished(document: &QueryDocument) {
    let mut snapshots = document.subscribe();
    tokio::time::timeout(Duration::from_secs(3), async {
        while snapshots.borrow_and_update().execution.is_active() {
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[tokio::test]
async fn frontend_can_connect_preview_stream_navigate_cancel_restart_and_close_without_native_ui() {
    let directory = std::env::temp_dir().join(format!("dzban-frontend-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let directory = Directory(directory);
    let path = directory.0.join("history.json");
    let history = || QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    let fixture = Arc::new(Fixture::default());
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(Factory(fixture.clone())))
        .unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let connection = context.start_connect(DatabaseConfig::from_connection_string(
        "host=localhost user=fixture dbname=fixture",
    ));
    let mut connection_state = ConnectionSelection::default();
    connection_state.begin(connection.id);
    let connection_snapshots = connection.subscribe();
    let engine = connection.wait().await.unwrap();
    assert!(connection_state.apply(&connection_snapshots.borrow()));
    assert_eq!(connection_state, ConnectionSelection::Ready(engine.id()));

    let mut documents = QueryDocuments::default();
    let document = documents.open();
    document.set_draft("stream".into());
    let mut view = AppState::default();
    view.transition(AppEvent::SessionSelected(Some(
        SessionInteraction::from_engine(&engine),
    )));
    view.transition(AppEvent::KeyPressed {
        key: InputRole::PrimaryModifier,
        at_ms: 0,
    });
    assert_eq!(
        view.transition(AppEvent::KeyPressed {
            key: InputRole::PreviewModifier,
            at_ms: 1
        }),
        [AppCommand::BeginPreview]
    );
    let preview = PlanPreviewService::default();
    preview
        .schedule(
            engine.clone(),
            document.clone(),
            document.draft(),
            Duration::ZERO,
        )
        .unwrap();
    let mut plans = preview.subscribe();
    let plan = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let PreviewPhase::Ready(plan) = plans.borrow_and_update().phase.clone() {
                break plan;
            }
            plans.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(preview.accepts(&preview.snapshot(), &document));
    view.transition(AppEvent::PreviewReady(plan));
    assert_eq!(
        view.transition(AppEvent::KeyPressed {
            key: InputRole::Submit,
            at_ms: 10
        }),
        [AppCommand::RunQuery]
    );

    let limits = ResultLimits {
        memory_rows: 2,
        ..Default::default()
    };
    let query = document
        .submit(engine.clone(), document.draft(), limits, Some(history()))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), fixture.ready.notified())
        .await
        .unwrap();
    let mut snapshots = document.subscribe();
    let early = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(capture) = snapshots
                .borrow_and_update()
                .operation
                .as_ref()
                .and_then(|operation| operation.capture.clone())
                && capture.rows >= 5
            {
                break capture;
            }
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let first_page = early.result.unwrap().read_page(0, 5).await.unwrap();
    assert_eq!(first_page.rows[1][1], ResultCell::Null);
    assert_eq!(first_page.rows[0][1], ResultCell::Text("žluť\nNULL".into()));
    let other = documents.open();
    assert!(document.is_active());
    assert_eq!(documents.selected().unwrap().id(), other.id());
    view.transition(AppEvent::SessionSelected(None));
    assert_eq!(view.primary_action(), PrimaryAction::Unavailable);
    assert_eq!(documents.close(document.id()), CloseDocument::Busy);
    assert!(context.close(engine.id()).await.is_err());
    assert_eq!(
        documents.close_in_workspace(other.id()),
        CloseDocument::Closed { last: false }
    );
    drop(snapshots);
    drop(query);
    fixture.release.notify_one();
    finished(&document).await;
    let completion = document.snapshot().completion.unwrap().unwrap();
    let capture = &completion.result.as_ref().unwrap().capture;
    assert_eq!(capture.rows, 500);
    assert_eq!(capture.cutoff, None);
    let page = capture
        .result
        .as_ref()
        .unwrap()
        .read_page(200, 200)
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 200);
    assert_eq!(page.rows[0][0], ResultCell::Text("200".into()));
    assert_eq!(page.rows[199][0], ResultCell::Text("399".into()));
    assert_eq!(fixture.plans.load(Ordering::SeqCst), 2);

    document.set_draft("wait".into());
    let query = document
        .submit(engine.clone(), document.draft(), limits, Some(history()))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), fixture.ready.notified())
        .await
        .unwrap();
    document
        .request_cancel(engine.clone())
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(query.wait().await.unwrap().result.is_err());
    finished(&document).await;
    view.transition(AppEvent::SessionSelected(Some(
        SessionInteraction::from_engine(&engine),
    )));
    assert_eq!(view.primary_action(), PrimaryAction::Run);
    document.set_draft("rows".into());
    document
        .submit(engine.clone(), document.draft(), limits, Some(history()))
        .unwrap()
        .wait()
        .await
        .unwrap()
        .result
        .unwrap();
    let saved = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    assert_eq!(saved.entries().len(), 3);
    assert!(
        saved
            .entries()
            .iter()
            .all(|entry| entry.planning_error.is_none())
    );
    assert_eq!(
        documents.close_in_workspace(document.id()),
        CloseDocument::CloseWorkspace
    );
    assert_eq!(document.draft(), "rows");
    assert!(connection_state.begin_close(engine.id()));
    let closed = context
        .close(engine.id())
        .await
        .map_err(|error| match error {
            dzban_core::application::ApplicationError::Operation(error) => error,
            error => ExecutionError::Message(error.to_string()),
        });
    assert!(connection_state.finish_close(engine.id(), &closed));
    closed.unwrap();
    documents.retire_closed_session(&engine).unwrap();
    assert_eq!(document.snapshot().lifecycle, DocumentLifecycle::Closed);
    assert!(documents.documents().is_empty());
    assert!(connection_state.can_forget());
    view.transition(AppEvent::SessionSelected(None));
    assert_eq!(view.primary_action(), PrimaryAction::Unavailable);
    context.start_shutdown().unwrap().wait().await.unwrap();
    assert_eq!(
        *context.subscribe_phase().borrow(),
        ApplicationPhase::Closed
    );
}

struct IndependentFactory;
impl AdapterFactory for IndependentFactory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(Arc::new(Fixture::default()))
    }
}

#[tokio::test]
async fn query_tabs_have_independent_sessions_and_close_cancels_only_the_owning_query() {
    let mut registry = AdapterRegistry::default();
    registry.register(Arc::new(IndependentFactory)).unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let profile =
        DatabaseConfig::from_connection_string("host=localhost user=fixture dbname=fixture");
    let first_request = context.start_connect_tab(profile.clone());
    let first_id = first_request.tab.unwrap();
    let first_engine = first_request.wait().await.unwrap();
    let second_engine = context.start_connect_tab(profile).wait().await.unwrap();
    let first = context.tabs.session(first_engine.id()).unwrap();
    let second = context.tabs.session(second_engine.id()).unwrap();
    assert_eq!(first.id, first_id);
    assert_ne!(first.id, second.id);
    assert_ne!(first_engine.id(), second_engine.id());
    assert_ne!(first.document.id(), second.document.id());
    assert_eq!(context.tabs.snapshot().selected, Some(second.id));
    assert!(context.tabs.select(Some(first.id)));
    first.document.set_draft("wait".into());
    second.document.set_draft("rows".into());
    let slow = first
        .document
        .submit(
            first_engine.clone(),
            "wait".into(),
            ResultLimits::default(),
            None,
        )
        .unwrap();
    let mut progress = first.document.subscribe();
    tokio::time::timeout(Duration::from_secs(3), async {
        while progress
            .borrow_and_update()
            .operation
            .as_ref()
            .and_then(|operation| operation.capture.as_ref())
            .map_or(0, |capture| capture.rows)
            < 5
        {
            progress.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let quick = second
        .document
        .submit(
            second_engine.clone(),
            "rows".into(),
            ResultLimits::default(),
            None,
        )
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), quick.wait())
            .await
            .unwrap()
            .unwrap()
            .result
            .is_ok()
    );
    assert!(first.document.is_active());
    assert_eq!(first.document.draft(), "wait");
    assert_eq!(second.document.draft(), "rows");
    let waiting = second
        .submit("wait".into(), ResultLimits::default(), None)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), context.close(first_engine.id()))
        .await
        .unwrap()
        .unwrap();
    assert!(slow.wait().await.unwrap().result.is_err());
    assert!(second.document.is_active());
    assert!(context.tabs.session(first_engine.id()).is_none());
    assert!(context.tabs.session(second_engine.id()).is_some());
    assert_eq!(context.tabs.snapshot().selected, Some(second.id));
    tokio::time::timeout(
        Duration::from_secs(3),
        context.start_shutdown().unwrap().wait(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(waiting.wait().await.unwrap().result.is_err());
    assert!(context.tabs.snapshot().tabs.is_empty());
    assert!(context.session_ids().await.is_empty());
}

#[tokio::test]
async fn failed_tab_keeps_its_identity_and_draft_until_shutdown() {
    struct Rejected;
    impl AdapterFactory for Rejected {
        fn backend(&self) -> DatabaseBackend {
            DatabaseBackend::Postgresql
        }
        fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
            Err("connection rejected".into())
        }
    }
    let mut registry = AdapterRegistry::default();
    registry.register(Arc::new(Rejected)).unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let request = context.start_connect_tab(DatabaseConfig::from_connection_string(
        "host=localhost user=fixture dbname=fixture",
    ));
    let tab = request.tab.unwrap();
    let document = context.tabs.snapshot().tabs[0].document.clone();
    document.set_draft("SELECT 42".into());
    assert!(request.wait().await.is_err());
    let snapshot = context.tabs.snapshot();
    assert_eq!(snapshot.tabs[0].id, tab);
    assert_eq!(snapshot.tabs[0].document.draft(), "SELECT 42");
    assert!(matches!(
        snapshot.tabs[0].phase,
        dzban_core::tabs::TabPhase::Failed(_)
    ));
    context.start_shutdown().unwrap().wait().await.unwrap();
    assert!(context.tabs.snapshot().tabs.is_empty());
}
