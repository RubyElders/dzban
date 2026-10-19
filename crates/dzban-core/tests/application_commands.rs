use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::{ApplicationContext, ApplicationError};
use dzban_core::commands::{CommandError, EditorCommand, EditorIntent, HistoryLocation};
use dzban_core::error::ExecutionError;
use dzban_core::result_stream::ResultSink;
use dzban_core::{
    CommandAcknowledgment, DatabaseBackend, DatabaseConfig, QueryPlan, ResultCell, ResultColumn,
    ResultSet,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
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

struct Factory(Arc<Session>);
impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(self.0.clone())
    }
}
struct Fixture {
    context: Arc<ApplicationContext>,
    session: Arc<Session>,
    directory: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let session = Arc::new(Session::default());
        let mut registry = AdapterRegistry::default();
        registry
            .register(Arc::new(Factory(session.clone())))
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("dzban-command-tests-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        Self {
            context: Arc::new(ApplicationContext::new(registry)),
            session,
            directory,
        }
    }
    fn history(&self) -> HistoryLocation {
        HistoryLocation::File(self.directory.join("history.json"))
    }
    async fn connect(&self) -> dzban_core::tabs::QueryTab {
        let engine = self
            .context
            .start_connect_tab(DatabaseConfig::from_connection_string(
                "host=localhost user=fixture dbname=fixture",
            ))
            .wait()
            .await
            .unwrap();
        self.context.tabs.session(engine.id()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}
fn command(tab: &dzban_core::tabs::QueryTab, draft: &str, intent: EditorIntent) -> EditorCommand {
    EditorCommand {
        tab: tab.id,
        document: tab.document.id(),
        draft: draft.into(),
        selection: 0..0,
        intent,
    }
}
async fn finished(tab: &dzban_core::tabs::QueryTab) {
    let mut snapshots = tab.document.subscribe();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if snapshots.borrow_and_update().completion.is_some() {
                break;
            }
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}
async fn reached(signal: &Notify) {
    tokio::time::timeout(Duration::from_secs(3), signal.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn shared_editor_command_selects_unicode_sql_and_publishes_profile_history() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let text = "SELECT '😀é'; SELECT 2";
    let request = command(&tab, text, EditorIntent::Submit);
    let outcome = fixture
        .context
        .start_editor_command(request, fixture.history())
        .wait()
        .await
        .unwrap();
    let submission = outcome.submission.unwrap();
    assert_eq!(submission.document, tab.document.id().0);
    assert_eq!(submission.session, tab.engine.as_ref().unwrap().id().0);
    assert_eq!(submission.generation, 1);
    finished(&tab).await;
    let history = fixture
        .history()
        .load(tab.profile.as_ref().unwrap().0.clone())
        .await
        .unwrap();
    assert_eq!(history.entries()[0].sql, "SELECT '😀é'");
    assert_eq!(fixture.session.plans.load(Ordering::SeqCst), 1);
    let mut invalid = command(&tab, text, EditorIntent::Primary);
    invalid.selection = 1000..1001;
    assert!(matches!(
        fixture
            .context
            .start_editor_command(invalid, fixture.history())
            .wait()
            .await,
        Err(CommandError::Selection(_))
    ));
    assert_eq!(tab.document.snapshot().output_generation, 1);
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}
#[tokio::test]
async fn primary_command_stops_the_retained_query_without_starting_another_one() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    fixture
        .context
        .start_editor_command(
            command(&tab, "wait", EditorIntent::Primary),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    reached(&fixture.session.ready).await;
    let mut stop = command(&tab, "ignored", EditorIntent::Primary);
    stop.selection = 1000..1001;
    let outcome = fixture
        .context
        .start_editor_command(stop, fixture.history())
        .wait()
        .await
        .unwrap();
    assert!(outcome.submission.is_none());
    assert!(outcome.cancellation_requested);
    finished(&tab).await;
    assert_eq!(fixture.session.plans.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.session.cancellations.load(Ordering::SeqCst), 1);
    assert_eq!(tab.document.snapshot().output_generation, 1);
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn pending_command_cannot_follow_selection_to_a_new_tab_after_preview_cleanup() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let preview = fixture
        .context
        .tabs
        .start_preview(
            tab.id,
            tab.document.id(),
            tab.document.snapshot().draft_revision,
            "planning".into(),
            Some(1),
        )
        .unwrap();
    reached(&fixture.session.ready).await;
    let pending = fixture.context.start_editor_command(
        command(&tab, "SELECT 1", EditorIntent::Primary),
        fixture.history(),
    );
    assert!(preview.wait().await.is_err());
    fixture.context.open_tab(None, String::new()).unwrap();
    fixture.session.release.notify_one();
    assert!(matches!(
        pending.wait().await,
        Err(CommandError::Application(
            ApplicationError::SelectionSuperseded
        ))
    ));
    assert_eq!(tab.document.snapshot().output_generation, 0);
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn dropping_a_command_waiter_does_not_cancel_the_query_or_its_history() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let admitted = fixture.context.start_editor_command(
        command(&tab, "wait", EditorIntent::Primary),
        fixture.history(),
    );
    reached(&fixture.session.ready).await;
    drop(admitted);
    assert!(tab.document.is_active());
    fixture.session.release.notify_one();
    finished(&tab).await;
    assert_eq!(
        fixture
            .history()
            .load(tab.profile.as_ref().unwrap().0.clone())
            .await
            .unwrap()
            .entries()
            .len(),
        1
    );
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

fn input_command(
    tab: &dzban_core::tabs::QueryTab,
    draft: &str,
    event: dzban_core::interaction::InputEvent,
) -> dzban_core::commands::InputCommand {
    dzban_core::commands::InputCommand {
        tab: tab.id,
        document: tab.document.id(),
        revision: tab.document.snapshot().draft_revision,
        draft: draft.into(),
        selection: 0..0,
        event,
    }
}

async fn interaction_until(
    snapshots: &mut tokio::sync::watch::Receiver<
        Option<dzban_core::presentation::InteractionPresentation>,
    >,
    predicate: impl Fn(&dzban_core::presentation::InteractionPresentation) -> bool,
) -> dzban_core::presentation::InteractionPresentation {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(snapshot) = snapshots.borrow_and_update().clone()
                && predicate(&snapshot)
            {
                return snapshot;
            }
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn shared_input_executor_estimates_pins_and_executes_without_frontend_effects() {
    use dzban_core::interaction::{InputEvent, InputKey};
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let mut snapshots = fixture.context.tabs.subscribe_interaction(tab.id).unwrap();
    for (key, at_ms) in [(InputKey::Primary, 1), (InputKey::Preview, 2)] {
        fixture
            .context
            .start_input_command(
                input_command(&tab, "SELECT 42", InputEvent::Press { key, at_ms }),
                fixture.history(),
            )
            .wait()
            .await
            .unwrap();
    }
    let estimated = interaction_until(&mut snapshots, |snapshot| snapshot.plan.is_some()).await;
    assert!(!estimated.visible);
    assert!(estimated.input.commands.is_empty());
    assert_eq!(tab.document.snapshot().output_generation, 0);
    for event in [
        InputEvent::Press {
            key: InputKey::Inspect,
            at_ms: 3,
        },
        InputEvent::Release {
            key: InputKey::Inspect,
            at_ms: 4,
        },
        InputEvent::Release {
            key: InputKey::Preview,
            at_ms: 5,
        },
        InputEvent::Release {
            key: InputKey::Primary,
            at_ms: 6,
        },
    ] {
        fixture
            .context
            .start_input_command(input_command(&tab, "SELECT 42", event), fixture.history())
            .wait()
            .await
            .unwrap();
    }
    let pinned = snapshots.borrow().clone().unwrap();
    assert!(pinned.visible && pinned.input.pinned);
    let run = fixture
        .context
        .start_input_command(
            input_command(&tab, "SELECT 42", InputEvent::PrimaryAction),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    assert!(run.outcome.submission.is_some());
    assert!(!run.interaction.visible);
    assert!(run.interaction.plan.is_none());
    assert!(!run.interaction.input.pinned);
    finished(&tab).await;
    interaction_until(&mut snapshots, |snapshot| {
        matches!(
            snapshot.input.hint,
            dzban_core::presentation::InputHint::Default
        )
    })
    .await;
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn shared_input_submit_drains_live_preview_and_rejects_navigation_before_admission() {
    use dzban_core::interaction::{InputEvent, InputKey};
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    fixture
        .context
        .start_input_command(
            input_command(&tab, "planning", InputEvent::OpenPlan),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    reached(&fixture.session.ready).await;
    let mut snapshots = fixture.context.tabs.subscribe_interaction(tab.id).unwrap();
    fixture
        .context
        .start_input_command(
            input_command(
                &tab,
                "SELECT 1",
                InputEvent::Press {
                    key: InputKey::Primary,
                    at_ms: 1,
                },
            ),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    let submission = fixture.context.start_input_command(
        input_command(
            &tab,
            "SELECT 1",
            InputEvent::Press {
                key: InputKey::Submit,
                at_ms: 2,
            },
        ),
        fixture.history(),
    );
    interaction_until(&mut snapshots, |snapshot| !snapshot.loading).await;
    assert!(matches!(
        tab.engine.as_ref().unwrap().start_ping(),
        Err(ExecutionError::Busy)
    ));
    fixture.context.open_tab(None, String::new()).unwrap();
    fixture.session.release.notify_one();
    assert!(matches!(
        submission.wait().await,
        Err(CommandError::Application(
            ApplicationError::SelectionSuperseded
        ))
    ));
    assert_eq!(tab.document.snapshot().output_generation, 0);
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn shared_input_repeat_is_suppressed_and_the_next_submit_stops_the_query() {
    use dzban_core::interaction::{InputEvent, InputKey};
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    fixture
        .context
        .start_input_command(
            input_command(
                &tab,
                "wait",
                InputEvent::Press {
                    key: InputKey::Primary,
                    at_ms: 1,
                },
            ),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    let submitted = fixture
        .context
        .start_input_command(
            input_command(
                &tab,
                "wait",
                InputEvent::Press {
                    key: InputKey::Submit,
                    at_ms: 2,
                },
            ),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    assert!(submitted.outcome.submission.is_some());
    reached(&fixture.session.ready).await;
    let repeated = fixture
        .context
        .start_input_command(
            input_command(
                &tab,
                "wait",
                InputEvent::Press {
                    key: InputKey::Submit,
                    at_ms: 3,
                },
            ),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    assert_eq!(repeated.outcome, Default::default());
    fixture
        .context
        .start_input_command(
            input_command(
                &tab,
                "wait",
                InputEvent::Release {
                    key: InputKey::Submit,
                    at_ms: 4,
                },
            ),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    let stopped = fixture
        .context
        .start_input_command(
            input_command(
                &tab,
                "wait",
                InputEvent::Press {
                    key: InputKey::Submit,
                    at_ms: 5,
                },
            ),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    assert!(stopped.outcome.cancellation_requested);
    assert!(stopped.outcome.submission.is_none());
    finished(&tab).await;
    assert_eq!(tab.document.snapshot().output_generation, 1);
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn shared_input_empty_preview_reports_failure_and_context_change_clears_it() {
    use dzban_core::interaction::InputEvent;
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let failed = fixture
        .context
        .start_input_command(
            input_command(&tab, "  ", InputEvent::OpenPlan),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    assert_eq!(failed.interaction.error.as_deref(), Some("Empty query"));
    assert!(!failed.interaction.loading);
    assert!(failed.interaction.visible);
    let reset = fixture
        .context
        .start_input_command(
            input_command(&tab, "SELECT 1", InputEvent::ContextChanged),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    assert!(!reset.interaction.visible);
    assert!(reset.interaction.error.is_none());
    assert_eq!(fixture.session.plans.load(Ordering::SeqCst), 0);
    let snapshots = fixture.context.tabs.subscribe_interaction(tab.id).unwrap();
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
    let retired = snapshots.borrow().clone().unwrap();
    assert!(retired.retired && !retired.loading && !retired.visible);
}

#[tokio::test]
async fn plan_waiter_observes_retirement_after_backend_cleanup() {
    use dzban_core::interaction::InputEvent;
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let admitted = fixture
        .context
        .start_input_command(
            input_command(&tab, "planning", InputEvent::OpenPlan),
            fixture.history(),
        )
        .wait()
        .await
        .unwrap();
    reached(&fixture.session.ready).await;
    let pending = fixture
        .context
        .tabs
        .await_plan(tab.id, admitted.interaction.revision);
    tokio::pin!(pending);
    tokio::select! {
        biased;
        _ = &mut pending => panic!("Plan transport is still active"),
        _ = std::future::ready(()) => (),
    }
    fixture.session.release.notify_one();
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
    let retired = pending.await.unwrap();
    assert!(retired.retired);
    assert!(!retired.visible && !retired.loading);
    assert!(retired.plan.is_none());
    assert_eq!(fixture.session.plans.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captured_editor_commands_survive_concurrent_draft_delivery_without_losing_submissions() {
    struct DraftDelivery {
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for DraftDelivery {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.worker.take().unwrap().join().unwrap();
        }
    }
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let stop = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(std::sync::Barrier::new(2));
    let writer_stop = stop.clone();
    let writer_ready = ready.clone();
    let document = tab.document.clone();
    let delivery = DraftDelivery {
        stop,
        worker: Some(std::thread::spawn(move || {
            writer_ready.wait();
            while !writer_stop.load(Ordering::SeqCst) {
                document.set_draft("newer native buffer".into());
                std::thread::yield_now();
            }
        })),
    };
    ready.wait();
    for generation in 1..=32 {
        let outcome = fixture
            .context
            .start_editor_command(
                command(&tab, "SELECT 'captured source'", EditorIntent::Submit),
                fixture.history(),
            )
            .wait()
            .await
            .unwrap();
        assert_eq!(outcome.submission.unwrap().generation, generation);
        finished(&tab).await;
    }
    drop(delivery);
    let history = fixture
        .history()
        .load(tab.profile.as_ref().unwrap().0.clone())
        .await
        .unwrap();
    assert_eq!(history.entries().len(), 32);
    assert!(
        history
            .entries()
            .iter()
            .all(|entry| entry.sql == "SELECT 'captured source'")
    );
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn queued_plan_does_not_relabel_captured_sql_after_the_draft_changes() {
    use dzban_core::interaction::InputEvent;
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    tab.document.set_draft("SELECT original".into());
    let previous = fixture
        .context
        .tabs
        .start_preview(
            tab.id,
            tab.document.id(),
            tab.document.snapshot().draft_revision,
            "planning".into(),
            None,
        )
        .unwrap();
    reached(&fixture.session.ready).await;
    let mut snapshots = fixture.context.tabs.subscribe_interaction(tab.id).unwrap();
    let queued = fixture.context.start_input_command(
        input_command(&tab, "SELECT original", InputEvent::OpenPlan),
        fixture.history(),
    );
    interaction_until(&mut snapshots, |snapshot| snapshot.loading).await;
    tab.document.set_draft("SELECT replacement".into());
    fixture.session.release.notify_one();
    let admitted = queued.wait().await;
    let revision = snapshots.borrow().as_ref().unwrap().revision;
    let settled = tokio::time::timeout(
        Duration::from_secs(3),
        fixture.context.tabs.await_plan(tab.id, revision),
    )
    .await;
    let plans = fixture.session.plans.load(Ordering::SeqCst);
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(previous.wait().await.is_err());
    assert_eq!(
        plans, 1,
        "A stale captured plan must not be attached to a newer draft"
    );
    assert!(matches!(
        admitted,
        Err(CommandError::Preview(
            dzban_core::preview::PreviewError::Invalidated(_)
        ))
    ));
    let settled = settled.unwrap().unwrap();
    assert!(!settled.loading && settled.plan.is_none());
    assert_eq!(
        settled.draft_revision,
        tab.document.snapshot().draft_revision
    );
}

#[tokio::test]
async fn stale_queued_plan_cannot_clear_a_newer_draft_preview() {
    use dzban_core::interaction::InputEvent;
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    tab.document.set_draft("SELECT original".into());
    let previous = fixture
        .context
        .tabs
        .start_preview(
            tab.id,
            tab.document.id(),
            tab.document.snapshot().draft_revision,
            "planning".into(),
            None,
        )
        .unwrap();
    reached(&fixture.session.ready).await;
    let mut snapshots = fixture.context.tabs.subscribe_interaction(tab.id).unwrap();
    let stale = fixture.context.start_input_command(
        input_command(&tab, "SELECT original", InputEvent::OpenPlan),
        fixture.history(),
    );
    let first = interaction_until(&mut snapshots, |snapshot| snapshot.loading).await;
    tab.document.set_draft("SELECT replacement".into());
    let current = fixture.context.start_input_command(
        input_command(&tab, "SELECT replacement", InputEvent::OpenPlan),
        fixture.history(),
    );
    interaction_until(&mut snapshots, |snapshot| {
        snapshot.loading && snapshot.draft_revision > first.draft_revision
    })
    .await;
    fixture.session.release.notify_one();
    let stale = stale.wait().await;
    let current = current.wait().await;
    let revision = snapshots.borrow().as_ref().unwrap().revision;
    let settled = tokio::time::timeout(
        Duration::from_secs(3),
        fixture.context.tabs.await_plan(tab.id, revision),
    )
    .await;
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(previous.wait().await.is_err());
    assert!(matches!(
        stale,
        Err(CommandError::Preview(
            dzban_core::preview::PreviewError::Invalidated(_)
        ))
    ));
    assert!(current.is_ok());
    let settled = settled.unwrap().unwrap();
    assert!(!settled.loading && settled.plan.is_some());
    assert_eq!(
        settled.draft_revision,
        tab.document.snapshot().draft_revision
    );
    assert_eq!(fixture.session.plans.load(Ordering::SeqCst), 2);
}
