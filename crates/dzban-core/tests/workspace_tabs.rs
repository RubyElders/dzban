use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::{ApplicationContext, ApplicationError};
use dzban_core::documents::DocumentLifecycle;
use dzban_core::error::ExecutionError;
use dzban_core::tabs::{TabId, TabPhase};
use dzban_core::{ConnectionProfileId, DatabaseBackend, DatabaseConfig, QueryPlan, ResultSet};
use tokio::sync::Notify;

#[derive(Default)]
struct Session {
    connect_entered: Notify,
    connect_release: Notify,
    close_entered: Notify,
    close_release: Notify,
    block_connect: AtomicBool,
    block_close: AtomicBool,
    reject_connect: AtomicBool,
    reject_close: AtomicBool,
    closes: AtomicUsize,
    explain_entered: Notify,
    explain_release: Notify,
    explains: AtomicUsize,
}

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        self.connect_entered.notify_one();
        if self.block_connect.load(Ordering::SeqCst) {
            self.connect_release.notified().await;
        }
        if self.reject_connect.swap(false, Ordering::SeqCst) {
            Err("connection rejected once".into())
        } else {
            Ok(())
        }
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.close_entered.notify_one();
        if self.block_close.load(Ordering::SeqCst) {
            self.close_release.notified().await;
        }
        if self.reject_close.swap(false, Ordering::SeqCst) {
            Err(ExecutionError::Message("cleanup rejected once".into()))
        } else {
            Ok(())
        }
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(1)
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        Err("unused".into())
    }
    async fn cancel(&self) -> Result<(), String> {
        Ok(())
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn explain(&self, sql: &str) -> Result<QueryPlan, String> {
        self.explains.fetch_add(1, Ordering::SeqCst);
        if sql == "wait" {
            self.explain_entered.notify_one();
            self.explain_release.notified().await;
        }
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!({"sql": sql}),
            estimate: Default::default(),
        })
    }
}

struct Factory(Arc<Session>);
impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(self.0.clone())
    }
}

fn fixture() -> (Arc<ApplicationContext>, Arc<Session>, DatabaseConfig) {
    let session = Arc::new(Session::default());
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(Factory(session.clone())))
        .unwrap();
    (
        Arc::new(ApplicationContext::new(registry)),
        session,
        DatabaseConfig::from_connection_string("host=localhost user=fixture dbname=fixture"),
    )
}

async fn reached(signal: &Notify) {
    tokio::time::timeout(Duration::from_secs(3), signal.notified())
        .await
        .unwrap();
}

struct WorkspaceFile(std::path::PathBuf);
impl WorkspaceFile {
    async fn new(tabs: Vec<(Option<&str>, &str)>, selected: i64) -> Self {
        let file = Self(
            std::env::temp_dir().join(format!("dzban-restore-test-{}.json", uuid::Uuid::new_v4())),
        );
        file.location()
            .save(dzban_core::workspace::SavedWorkspace {
                version: 1,
                selected,
                tabs: tabs
                    .into_iter()
                    .map(|(profile, draft)| dzban_core::workspace::SavedTab {
                        profile: profile.map(str::to_owned),
                        draft: draft.into(),
                    })
                    .collect(),
            })
            .await
            .unwrap();
        file
    }
    fn location(&self) -> dzban_core::workspace::WorkspaceLocation {
        dzban_core::workspace::WorkspaceLocation {
            host: dzban_core::workspace::WorkspaceHost::Macos,
            path: Some(self.0.clone()),
        }
    }
}
impl Drop for WorkspaceFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}

#[tokio::test]
async fn shared_restore_keeps_order_profiles_drafts_selection_and_missing_connections_without_sql()
{
    let (context, session, mut profile) = fixture();
    profile.password = Some("fixture-only-not-persisted".into());
    let file = WorkspaceFile::new(
        vec![
            (Some("fixture"), "SELECT 'žluť 😀';"),
            (None, "chooser"),
            (Some("missing"), "keep"),
            (Some("fixture"), "last"),
        ],
        i64::MAX,
    )
    .await;
    let result = context
        .start_restore_workspace(
            file.location(),
            Some(dzban_core::Config {
                databases: vec![profile],
            }),
        )
        .wait()
        .await;
    assert!(result.finished && result.restored && result.error.is_none());
    assert_eq!(result.issues.len(), 1);
    assert_eq!(result.issues[0].profile.as_deref(), Some("missing"));
    let tabs = context.tabs.snapshot();
    assert_eq!(
        tabs.tabs
            .iter()
            .map(|tab| tab.document.draft())
            .collect::<Vec<_>>(),
        ["SELECT 'žluť 😀';", "chooser", "keep", "last"]
    );
    assert_eq!(
        tabs.tabs
            .iter()
            .map(|tab| tab.phase.clone())
            .collect::<Vec<_>>(),
        [
            TabPhase::Ready,
            TabPhase::Choosing,
            TabPhase::Choosing,
            TabPhase::Ready
        ]
    );
    assert_eq!(tabs.selected, Some(tabs.tabs[3].id));
    assert_ne!(
        tabs.tabs[0].engine.as_ref().unwrap().id(),
        tabs.tabs[3].engine.as_ref().unwrap().id()
    );
    assert_eq!(session.explains.load(Ordering::SeqCst), 0);
    context
        .save_workspace(file.location(), Default::default())
        .await
        .unwrap();
    let saved = std::fs::read_to_string(&file.0).unwrap();
    assert!(!saved.contains("fixture-only-not-persisted"));
    assert!(!saved.contains("session"));
    assert!(!saved.contains("result"));
    context.start_shutdown().unwrap().wait().await.unwrap();
    assert_eq!(session.closes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn shutdown_joins_retained_restore_without_admitting_later_saved_connections() {
    let (context, session, profile) = fixture();
    session.block_connect.store(true, Ordering::SeqCst);
    let file = WorkspaceFile::new(
        vec![(Some("fixture"), "first"), (Some("fixture"), "second")],
        0,
    )
    .await;
    let restore = context.start_restore_workspace(
        file.location(),
        Some(dzban_core::Config {
            databases: vec![profile],
        }),
    );
    reached(&session.connect_entered).await;
    let joined = context.start_restore_workspace(file.location(), None);
    drop(restore);
    let shutdown = context.start_shutdown().unwrap();
    session.connect_release.notify_one();
    let restored = tokio::time::timeout(Duration::from_secs(3), joined.wait())
        .await
        .unwrap();
    assert!(restored.finished);
    tokio::time::timeout(Duration::from_secs(3), shutdown.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(context.tabs.snapshot().tabs.is_empty());
    assert!(context.session_ids().await.is_empty());
    assert_eq!(session.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn canceling_a_restored_connection_preserves_its_draft_without_reporting_failure() {
    let (context, session, profile) = fixture();
    session.block_connect.store(true, Ordering::SeqCst);
    let file =
        WorkspaceFile::new(vec![(Some("fixture"), "retry draft"), (None, "chooser")], 0).await;
    let restore = context.start_restore_workspace(
        file.location(),
        Some(dzban_core::Config {
            databases: vec![profile],
        }),
    );
    reached(&session.connect_entered).await;
    let tab = context.tabs.snapshot().tabs[0].id;
    let cancel = context.start_cancel_connection(tab).unwrap();
    session.connect_release.notify_one();
    cancel.wait().await.unwrap();
    let restored = restore.wait().await;
    assert!(restored.issues.is_empty());
    assert_eq!(
        context.tabs.find(tab).unwrap().document.draft(),
        "retry draft"
    );
    assert_eq!(context.tabs.find(tab).unwrap().phase, TabPhase::Choosing);
    assert_eq!(context.tabs.snapshot().selected, Some(tab));
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn restored_connection_and_shutdown_failures_keep_shared_ownership_retryable() {
    let (context, session, profile) = fixture();
    session.reject_connect.store(true, Ordering::SeqCst);
    let file = WorkspaceFile::new(vec![(Some("fixture"), "retry draft")], -4).await;
    let restore = context
        .start_restore_workspace(
            file.location(),
            Some(dzban_core::Config {
                databases: vec![profile.clone()],
            }),
        )
        .wait()
        .await;
    assert_eq!(restore.issues.len(), 1);
    let tab = context.tabs.snapshot().tabs[0].id;
    assert!(matches!(
        context.tabs.find(tab).unwrap().phase,
        TabPhase::Failed(_)
    ));
    context
        .start_connect_in_tab(tab, profile)
        .wait()
        .await
        .unwrap();
    assert_eq!(
        context.tabs.find(tab).unwrap().document.draft(),
        "retry draft"
    );
    session.reject_close.store(true, Ordering::SeqCst);
    assert!(context.start_shutdown().unwrap().wait().await.is_err());
    assert_eq!(context.tabs.snapshot().tabs.len(), 1);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn failed_shutdown_resumes_attention_for_the_next_query() {
    use dzban_core::attention::AttentionInput;
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    context.attention().input(AttentionInput::Visibility {
        visible: false,
        viewed: None,
    });
    session.reject_close.store(true, Ordering::SeqCst);
    assert!(context.start_shutdown().unwrap().wait().await.is_err());
    let tab = context.tabs.session(engine.id()).unwrap();
    let completed = tab
        .submit("SELECT 1".into(), Default::default(), None)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(completed.result.is_err());
    let attention = context.attention().snapshot();
    assert_eq!(attention.request_revision, 1);
    assert!(attention.tabs[0].unseen.is_some());
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn chooser_keeps_one_document_through_connection_failure_and_retry() {
    let (context, session, profile) = fixture();
    let tab = context
        .open_tab(
            Some(ConnectionProfileId("unavailable".into())),
            "SELECT 'žluť 😀';".into(),
        )
        .unwrap();
    let chooser = context.tabs.find(tab).unwrap();
    let document = chooser.document.id();
    assert_eq!(chooser.phase, TabPhase::Choosing);
    assert!(chooser.engine.is_none());
    assert!(chooser.backend.is_none());
    session.reject_connect.store(true, Ordering::SeqCst);
    let failed = context.start_connect_in_tab(tab, profile.clone());
    assert_eq!(failed.tab, Some(tab));
    assert!(failed.wait().await.is_err());
    assert!(matches!(
        context.tabs.find(tab).unwrap().phase,
        TabPhase::Failed(_)
    ));
    let connected = context
        .start_connect_in_tab(tab, profile)
        .wait()
        .await
        .unwrap();
    let ready = context.tabs.find(tab).unwrap();
    assert_eq!(ready.phase, TabPhase::Ready);
    assert_eq!(ready.document.id(), document);
    assert_eq!(ready.document.draft(), "SELECT 'žluť 😀';");
    assert_eq!(ready.engine.as_ref().unwrap().id(), connected.id());
    assert_eq!(context.tabs.snapshot().tabs.len(), 1);
    context.start_shutdown().unwrap().wait().await.unwrap();
    assert_eq!(
        chooser.document.snapshot().lifecycle,
        DocumentLifecycle::Closed
    );
}

#[tokio::test]
async fn closing_pending_connection_retains_ownership_and_shutdown_joins_cleanup() {
    let (context, session, profile) = fixture();
    session.block_connect.store(true, Ordering::SeqCst);
    session.block_close.store(true, Ordering::SeqCst);
    let tab = context.open_tab(None, "draft".into()).unwrap();
    let document = context.tabs.find(tab).unwrap().document;
    let connecting = context.start_connect_in_tab(tab, profile.clone());
    reached(&session.connect_entered).await;
    let request = context.tabs.find(tab).unwrap().connection_request;
    assert!(matches!(
        context.start_connect_in_tab(tab, profile).wait().await,
        Err(ApplicationError::Operation(ExecutionError::Busy))
    ));
    assert_eq!(context.tabs.find(tab).unwrap().connection_request, request);
    let closing = context.start_close_tab(tab).unwrap();
    assert_eq!(context.tabs.find(tab).unwrap().phase, TabPhase::Closing);
    drop(connecting);
    let other = context.open_tab(None, String::new()).unwrap();
    session.connect_release.notify_one();
    reached(&session.close_entered).await;
    let engine = context.tabs.find(tab).unwrap().engine.unwrap();
    let joined = context.begin_close(engine).unwrap();
    assert_eq!(context.tabs.find(tab).unwrap().phase, TabPhase::Closing);
    assert_eq!(context.tabs.snapshot().selected, Some(other));
    let shutdown = context.start_shutdown().unwrap();
    drop(closing);
    session.close_release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), shutdown.wait())
        .await
        .unwrap()
        .unwrap();
    joined.wait().await.unwrap();
    assert!(context.tabs.snapshot().tabs.is_empty());
    assert!(context.session_ids().await.is_empty());
    assert_eq!(session.closes.load(Ordering::SeqCst), 1);
    assert_eq!(document.snapshot().lifecycle, DocumentLifecycle::Closed);
}

#[tokio::test]
async fn failed_cleanup_retains_the_tab_session_and_draft_for_retry() {
    let (context, session, profile) = fixture();
    let tab = context.open_tab(None, "keep this draft".into()).unwrap();
    let engine = context
        .start_connect_in_tab(tab, profile)
        .wait()
        .await
        .unwrap();
    let document = context.tabs.find(tab).unwrap().document;
    session.reject_close.store(true, Ordering::SeqCst);
    assert!(context.start_close_tab(tab).unwrap().wait().await.is_err());
    let retained = context.tabs.find(tab).unwrap();
    assert_eq!(retained.phase, TabPhase::Ready);
    assert_eq!(retained.document.id(), document.id());
    assert_eq!(retained.document.draft(), "keep this draft");
    assert_eq!(context.session_ids().await, [engine.id()]);
    context.start_close_tab(tab).unwrap().wait().await.unwrap();
    assert!(context.tabs.find(tab).is_none());
    assert!(context.session_ids().await.is_empty());
    assert_eq!(session.closes.load(Ordering::SeqCst), 2);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn chooser_selection_rejects_stale_navigation_and_last_close_leaves_no_tab() {
    let (context, _, _) = fixture();
    let first = context.open_tab(None, "first".into()).unwrap();
    let second = context.open_tab(None, "second".into()).unwrap();
    assert!(context.tabs.select_at(Some(first), Some(10)));
    assert!(context.tabs.select_at(Some(second), Some(20)));
    assert!(!context.tabs.select_at(Some(first), Some(11)));
    assert!(
        !context
            .tabs
            .select_at(Some(TabId(uuid::Uuid::new_v4())), Some(100))
    );
    assert_eq!(context.tabs.snapshot().selected, Some(second));
    context
        .start_close_tab(first)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(context.tabs.snapshot().selected, Some(second));
    assert!(context.tabs.select_at(Some(second), Some(21)));
    context
        .start_close_tab(second)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(context.tabs.snapshot().selected, None);
    assert!(context.tabs.snapshot().tabs.is_empty());
    context.start_shutdown().unwrap().wait().await.unwrap();
    assert!(context.open_tab(None, String::new()).is_err());
}

#[tokio::test]
async fn canceling_connection_retains_chooser_identity_and_rejects_the_late_success() {
    let (context, session, profile) = fixture();
    session.block_connect.store(true, Ordering::SeqCst);
    let tab = context.open_tab(None, "retained draft".into()).unwrap();
    let document = context.tabs.find(tab).unwrap().document;
    let connecting = context.start_connect_in_tab(tab, profile.clone());
    reached(&session.connect_entered).await;
    let canceling = context.start_cancel_connection(tab).unwrap();
    session.connect_release.notify_one();
    assert!(connecting.wait().await.is_err());
    canceling.wait().await.unwrap();
    let chooser = context.tabs.find(tab).unwrap();
    assert_eq!(chooser.phase, TabPhase::Choosing);
    assert_eq!(chooser.document.id(), document.id());
    assert_eq!(chooser.document.draft(), "retained draft");
    assert_eq!(
        chooser.document.snapshot().lifecycle,
        DocumentLifecycle::Open
    );
    assert!(chooser.engine.is_none());
    assert!(context.session_ids().await.is_empty());
    session.block_connect.store(false, Ordering::SeqCst);
    context
        .start_connect_in_tab(tab, profile)
        .wait()
        .await
        .unwrap();
    assert_eq!(context.tabs.find(tab).unwrap().document.id(), document.id());
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn tab_creation_and_cycle_share_navigation_revision_ordering() {
    let (context, _, _) = fixture();
    let first = context.open_tab_at(None, "first".into(), Some(10)).unwrap();
    let second = context
        .open_tab_at(None, "second".into(), Some(20))
        .unwrap();
    assert_eq!(
        context.open_tab_at(None, "stale".into(), Some(19)),
        Err(ApplicationError::SelectionSuperseded)
    );
    assert_eq!(context.tabs.snapshot().tabs.len(), 2);
    assert!(context.tabs.cycle(1, Some(21)));
    assert_eq!(context.tabs.snapshot().selected, Some(first));
    assert!(context.tabs.cycle(-1, Some(22)));
    assert_eq!(context.tabs.snapshot().selected, Some(second));
    assert!(!context.tabs.cycle(1, Some(21)));
    assert_eq!(context.tabs.snapshot().selected, Some(second));
    assert!(context.tabs.cycle(i64::MIN, Some(23)));
    assert_eq!(context.tabs.snapshot().selected, Some(second));
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn failed_connection_cancellation_keeps_cleanup_retryable_without_losing_the_draft() {
    let (context, session, profile) = fixture();
    session.block_connect.store(true, Ordering::SeqCst);
    session.reject_close.store(true, Ordering::SeqCst);
    let tab = context.open_tab(None, "keep draft".into()).unwrap();
    let document = context.tabs.find(tab).unwrap().document;
    let connecting = context.start_connect_in_tab(tab, profile);
    reached(&session.connect_entered).await;
    let canceling = context.start_cancel_connection(tab).unwrap();
    session.connect_release.notify_one();
    assert!(connecting.wait().await.is_err());
    assert!(canceling.wait().await.is_err());
    let retained = context.tabs.find(tab).unwrap();
    assert_eq!(retained.phase, TabPhase::Ready);
    assert!(retained.engine.is_some());
    assert_eq!(retained.document.id(), document.id());
    assert_eq!(retained.document.draft(), "keep draft");
    context
        .start_cancel_connection(tab)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let chooser = context.tabs.find(tab).unwrap();
    assert_eq!(chooser.phase, TabPhase::Choosing);
    assert_eq!(chooser.document.id(), document.id());
    assert!(context.session_ids().await.is_empty());
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn shared_tab_input_rejects_retired_targets_and_resets_after_draft_changes() {
    use dzban_core::interaction::{AppCommand, InputEvent, InputKey};
    let (context, _, profile) = fixture();
    let connected = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(connected.id()).unwrap();
    let revision = tab.document.snapshot().draft_revision;
    let press = |key, at_ms| InputEvent::Press { key, at_ms };
    let input = |event| {
        context
            .tabs
            .input(tab.id, tab.document.id(), revision, event)
    };
    input(press(InputKey::Primary, 1)).unwrap();
    assert_eq!(
        input(press(InputKey::Preview, 2)).unwrap().commands,
        vec![AppCommand::BeginPreview]
    );
    assert_eq!(
        input(press(InputKey::Submit, 3)).unwrap().commands,
        vec![AppCommand::RunQuery]
    );
    assert!(
        input(press(InputKey::Submit, 4))
            .unwrap()
            .commands
            .is_empty()
    );
    let chooser = context.open_tab(None, String::new()).unwrap();
    assert!(matches!(
        input(InputEvent::Reset),
        Err(ApplicationError::SelectionSuperseded)
    ));
    assert!(context.tabs.select(Some(tab.id)));
    assert!(
        input(press(InputKey::Submit, 5))
            .unwrap()
            .commands
            .is_empty()
    );
    tab.document.set_draft("SELECT 2".into());
    assert!(matches!(
        input(InputEvent::Reset),
        Err(ApplicationError::SelectionSuperseded)
    ));
    let next = context
        .tabs
        .input(
            tab.id,
            tab.document.id(),
            tab.document.snapshot().draft_revision,
            press(InputKey::Submit, 5),
        )
        .unwrap();
    assert!(next.commands.is_empty());
    assert!(context.tabs.find(chooser).is_some());
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn shared_tab_input_keeps_a_pinned_plan_after_modifier_release_and_keyboard_reset() {
    use dzban_core::interaction::{InputEvent, InputKey};
    let (context, _, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    let input = |event| {
        context
            .tabs
            .input(
                tab.id,
                tab.document.id(),
                tab.document.snapshot().draft_revision,
                event,
            )
            .unwrap()
    };
    input(InputEvent::Press {
        key: InputKey::Primary,
        at_ms: 1,
    });
    input(InputEvent::Press {
        key: InputKey::Preview,
        at_ms: 2,
    });
    assert!(input(InputEvent::Pin).pinned);
    assert!(
        input(InputEvent::Release {
            key: InputKey::Preview,
            at_ms: 3
        })
        .pinned
    );
    assert!(
        input(InputEvent::Release {
            key: InputKey::Primary,
            at_ms: 4
        })
        .pinned
    );
    assert!(input(InputEvent::Reset).pinned);
    assert!(!input(InputEvent::ContextChanged).pinned);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn shared_preview_orders_requests_and_rejects_stale_cancellation() {
    use dzban_core::preview::PreviewError;
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    let revision = tab.document.snapshot().draft_revision;
    context.tabs.cancel_preview(tab.id, Some(10)).unwrap();
    assert!(matches!(
        context
            .tabs
            .start_preview(tab.id, tab.document.id(), revision, "old".into(), Some(9)),
        Err(PreviewError::Invalidated(_))
    ));
    let current = context
        .tabs
        .start_preview(
            tab.id,
            tab.document.id(),
            revision,
            "current".into(),
            Some(11),
        )
        .unwrap();
    assert!(context.tabs.cancel_preview(tab.id, Some(10)).is_err());
    let result = current.wait().await.unwrap();
    assert_eq!(result.target.session, engine.id());
    assert_eq!(result.target.document, tab.document.id());
    assert_eq!(result.plan.document["sql"], "current");
    assert_eq!(session.explains.load(Ordering::SeqCst), 1);
    tab.document.set_draft("changed".into());
    assert!(matches!(
        context
            .tabs
            .start_preview(tab.id, tab.document.id(), revision, "old".into(), Some(12)),
        Err(PreviewError::Invalidated(_))
    ));
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn leaving_a_tab_retires_preview_visibility_without_releasing_transport_admission() {
    use dzban_core::preview::{PeekEvent, PreviewError, PreviewSurface};
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    let visibility = context
        .tabs
        .peek(
            tab.id,
            PreviewSurface::Plan,
            PeekEvent::Open,
            Duration::ZERO,
        )
        .unwrap();
    assert!(visibility.visible && visibility.pinned);
    let pending = context
        .tabs
        .start_preview(
            tab.id,
            tab.document.id(),
            tab.document.snapshot().draft_revision,
            "wait".into(),
            Some(1),
        )
        .unwrap();
    reached(&session.explain_entered).await;
    context.open_tab(None, String::new()).unwrap();
    assert!(matches!(
        pending.wait().await,
        Err(PreviewError::Invalidated(_))
    ));
    assert!(matches!(engine.start_ping(), Err(ExecutionError::Busy)));
    let visibility = context
        .tabs
        .peek(
            tab.id,
            PreviewSurface::Plan,
            PeekEvent::Reset,
            Duration::ZERO,
        )
        .unwrap();
    assert!(!visibility.visible && !visibility.pinned);
    assert!(matches!(
        context.tabs.start_preview(
            tab.id,
            tab.document.id(),
            tab.document.snapshot().draft_revision,
            "late".into(),
            Some(2)
        ),
        Err(PreviewError::Invalidated(_))
    ));
    session.explain_release.notify_one();
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn shared_peek_surfaces_keep_independent_pin_and_hold_state() {
    use dzban_core::preview::{PREVIEW_HOLD, PeekEvent, PreviewSurface};
    let (context, _, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    let peek =
        |surface, event, elapsed| context.tabs.peek(tab.id, surface, event, elapsed).unwrap();
    peek(PreviewSurface::Plan, PeekEvent::Open, Duration::ZERO);
    peek(PreviewSurface::Row, PeekEvent::Show, Duration::ZERO);
    let row = peek(PreviewSurface::Row, PeekEvent::Release, PREVIEW_HOLD);
    assert!(!row.visible && !row.pinned);
    let plan = peek(PreviewSurface::Plan, PeekEvent::Reset, Duration::ZERO);
    assert!(plan.visible && plan.pinned);
    peek(PreviewSurface::Row, PeekEvent::Show, Duration::ZERO);
    let row = peek(PreviewSurface::Row, PeekEvent::Release, Duration::ZERO);
    assert!(row.visible && row.pinned);
    peek(PreviewSurface::Plan, PeekEvent::Close, Duration::ZERO);
    let row = peek(PreviewSurface::Row, PeekEvent::Reset, Duration::ZERO);
    assert!(row.visible && row.pinned);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn moving_a_tab_reorders_the_inventory_without_changing_selection() {
    let (context, _, _) = fixture();
    let first = context.open_tab(None, "first".into()).unwrap();
    let second = context.open_tab(None, "second".into()).unwrap();
    let third = context.open_tab(None, "third".into()).unwrap();
    let revision = context.tabs.snapshot().revision;
    assert!(context.tabs.move_tab(third, 0));
    let snapshot = context.tabs.snapshot();
    assert!(snapshot.revision > revision);
    assert_eq!(
        snapshot.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        vec![third, first, second]
    );
    assert_eq!(snapshot.selected, Some(third));
    assert!(context.tabs.move_tab(first, usize::MAX));
    assert_eq!(
        context
            .tabs
            .snapshot()
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        vec![third, second, first]
    );
    assert!(!context.tabs.move_tab(TabId(uuid::Uuid::new_v4()), 0));
}
