use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use adw::prelude::*;
use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::ApplicationContext;
use dzban_core::commands::EditorIntent;
use dzban_core::error::ExecutionError;
use dzban_core::interaction::PrimaryAction;
use dzban_core::presentation::{ExecutionPhase, InputHint, ResultStatusReason};
use dzban_core::result_stream::ResultSink;
use dzban_core::tabs::TabPhase;
use dzban_core::{
    Config, DatabaseBackend, DatabaseConfig, QueryEstimate, QueryPlan, ResultCell, ResultColumn,
    ResultSet,
};
use dzban_gtk::runtime::RuntimeBridge;
use gtk4::{gdk, glib};
use libadwaita as adw;
use tokio::sync::Notify;

use super::app::App;
use super::shortcuts::ShortcutAction;
use super::tab_page::TabPage;
use super::window::Shell;

#[derive(Default)]
struct Fixture {
    sessions: AtomicUsize,
    executions: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
    hold_connect: AtomicBool,
    reject_connect: AtomicBool,
    disconnected: AtomicBool,
    connect_release: Notify,
    reject_close: AtomicBool,
    release: Notify,
}

struct Session {
    fixture: Arc<Fixture>,
    canceled: Notify,
}

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        if self.fixture.hold_connect.load(Ordering::SeqCst) {
            self.fixture.connect_release.notified().await;
        }
        if self.fixture.reject_connect.swap(false, Ordering::SeqCst) {
            return Err("Fixture connection refused".into());
        }
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        if self.fixture.reject_close.swap(false, Ordering::SeqCst) {
            return Err(ExecutionError::Message("Fixture cleanup failed".into()));
        }
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        if self.fixture.disconnected.load(Ordering::SeqCst) {
            Err("Fixture transport closed".into())
        } else {
            Ok(1)
        }
    }
    async fn reconnect_if_needed(
        &self,
        started: &(dyn Fn() + Send + Sync),
    ) -> Result<bool, ExecutionError> {
        if !self.fixture.disconnected.load(Ordering::SeqCst) {
            return Ok(false);
        }
        started();
        self.connect().await.map_err(ExecutionError::from)?;
        self.fixture.disconnected.store(false, Ordering::SeqCst);
        Ok(true)
    }
    async fn version(&self) -> Result<String, String> {
        Ok("Fixture 1.0 server".into())
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        Err("Use streaming".into())
    }
    async fn cancel(&self) -> Result<(), String> {
        self.canceled.notify_one();
        Ok(())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!([{"Plan": {"Node Type": "Fixture Scan"}}]),
            estimate: QueryEstimate {
                rows: Some(3),
                startup_cost: Some(0.0),
                total_cost: Some(1.5),
                ..Default::default()
            },
        })
    }
    async fn execute_streamed(
        &self,
        sql: &str,
        sink: &mut ResultSink,
    ) -> Result<Option<dzban_core::CommandAcknowledgment>, ExecutionError> {
        let fixture = &self.fixture;
        fixture.executions.fetch_add(1, Ordering::SeqCst);
        let active = fixture.active.fetch_add(1, Ordering::SeqCst) + 1;
        fixture.peak.fetch_max(active, Ordering::SeqCst);
        let result = async {
            if sql.starts_with("wide") {
                return stream_wide(sink, sql == "wide slowly").await;
            }
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
            for id in 0..3 {
                sink.row(vec![
                    ResultCell::Text(id.to_string()),
                    if sql == "long value" {
                        ResultCell::Text("x".repeat(80))
                    } else if id == 1 {
                        ResultCell::Null
                    } else {
                        ResultCell::Text(format!("row {id}"))
                    },
                ])
                .await?;
            }
            match sql {
                "wait" => {
                    self.canceled.notified().await;
                    Err(ExecutionError::Canceled)
                }
                "hold" => {
                    fixture.release.notified().await;
                    Ok(None)
                }
                _ => Ok(None),
            }
        }
        .await;
        fixture.active.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

async fn stream_wide(
    sink: &mut ResultSink,
    slowly: bool,
) -> Result<Option<dzban_core::CommandAcknowledgment>, ExecutionError> {
    let columns = (0..40)
        .map(|column| ResultColumn {
            name: format!("c{column}"),
            type_name: "text".into(),
        })
        .collect();
    sink.schema(columns).await?;
    for row in 0..500 {
        sink.row(
            (0..40)
                .map(|column| ResultCell::Text(format!("r{row}c{column}")))
                .collect(),
        )
        .await?;
        if slowly && row % 5 == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    Ok(None)
}

struct Factory(Arc<Fixture>);

impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        self.0.sessions.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Session {
            fixture: self.0.clone(),
            canceled: Notify::new(),
        }))
    }
}

struct Directory(std::path::PathBuf);

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    fixture: Arc<Fixture>,
    shell: Shell,
    window: adw::ApplicationWindow,
    directory: Directory,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.window.destroy();
    }
}

fn directory() -> Directory {
    let path = std::env::temp_dir().join(format!("dzban-gtk-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    Directory(path)
}

fn harness_in(directory: Directory) -> Harness {
    harness_with_fixture(directory, Arc::new(Fixture::default()))
}

fn harness_with_fixture(directory: Directory, fixture: Arc<Fixture>) -> Harness {
    adw::init().unwrap();
    super::icons::register();
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(Factory(fixture.clone())))
        .unwrap();
    let bridge = RuntimeBridge::with_context(Arc::new(ApplicationContext::new(registry))).unwrap();
    let config: Config = toml::from_str(
        r#"
[[databases]]
name = "fixture"
host = "fixture"
user = "fixture"
database = "fixture"

[[databases]]
name = "other"
host = "fixture"
user = "fixture"
database = "other"
"#,
    )
    .unwrap();
    let app = App::with_fixture(bridge, config, &directory.0);
    let window = adw::ApplicationWindow::builder()
        .default_width(1150)
        .default_height(760)
        .build();
    let shell = Shell::new(app, window.clone());
    window.present();
    shell.start();
    let harness = Harness {
        fixture,
        shell,
        window,
        directory,
    };
    wait_until("the initial chooser", || {
        harness.shell.workspace().selected().is_some() && harness.shell.app().config().is_some()
    });
    harness
}

fn harness() -> Harness {
    harness_in(directory())
}

#[track_caller]
fn wait_until(what: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "Timed out waiting for {what}");
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
}

impl Harness {
    fn capture(&self, name: &str) {
        let Some(directory) = std::env::var_os("DZBAN_TEST_SCREENSHOTS") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        fn settled(widget: &gtk4::Widget) -> bool {
            if let Some(stack) = widget.downcast_ref::<gtk4::Stack>()
                && stack.is_transition_running()
            {
                return false;
            }
            let mut child = widget.first_child();
            while let Some(current) = child {
                if !settled(&current) {
                    return false;
                }
                child = current.next_sibling();
            }
            true
        }
        wait_until("completed transitions", || {
            settled(self.window.upcast_ref())
        });
        let frames = std::rc::Rc::new(std::cell::Cell::new(0));
        let rendered = frames.clone();
        self.window.add_tick_callback(move |_, _| {
            rendered.set(rendered.get() + 1);
            if rendered.get() >= 2 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        wait_until("the updated frame", || frames.get() >= 2);
        let paintable = gtk4::WidgetPaintable::new(Some(&self.window));
        let mut node = None;
        wait_until("a rendered frame", || {
            let snapshot = gtk4::Snapshot::new();
            paintable.snapshot(
                &snapshot,
                self.window.width() as f64,
                self.window.height() as f64,
            );
            node = snapshot.to_node();
            node.is_some()
        });
        let node = node.unwrap();
        self.window
            .renderer()
            .unwrap()
            .render_texture(&node, None)
            .save_to_png(directory.join(format!("{name}.png")))
            .unwrap();
    }

    fn selected(&self) -> TabPage {
        self.shell.workspace().selected().unwrap()
    }

    #[track_caller]
    fn connect(&self, profile: &str) -> TabPage {
        let page = self.selected();
        page.connect(profile);
        wait_until("a ready connection", || page.is_ready());
        page
    }

    #[track_caller]
    fn run(&self, page: &TabPage, sql: &str) {
        page.editor().set_text(sql);
        let before = page.document().map_or(0, |document| document.generation);
        page.editor_command(EditorIntent::Primary);
        wait_until("the query to start", || {
            page.document()
                .is_some_and(|document| document.generation > before)
        });
    }

    #[track_caller]
    fn finished(&self, page: &TabPage) {
        wait_until("the query to finish", || {
            page.document()
                .is_some_and(|document| document.execution == ExecutionPhase::Finished)
        });
    }
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn two_tabs_on_one_profile_keep_independent_sessions_and_run_concurrently() {
    let harness = harness();
    let first = harness.connect("fixture");
    harness.shell.workspace().open_profile("fixture");
    let second = harness.selected();
    assert_ne!(first.id(), second.id());
    wait_until("the second session", || second.is_ready());
    assert_ne!(first.session(), second.session());
    assert_eq!(harness.fixture.sessions.load(Ordering::SeqCst), 2);
    let titles = harness
        .shell
        .workspace()
        .tab_titles()
        .into_iter()
        .map(|(_, title)| title)
        .collect::<Vec<_>>();
    assert_eq!(titles, ["fixture - Query 1", "fixture - Query 2"]);

    harness.shell.workspace().select(first.id());
    harness.run(&first, "hold");
    harness.shell.workspace().select(second.id());
    harness.run(&second, "hold");
    wait_until("two concurrent queries", || {
        harness.fixture.active.load(Ordering::SeqCst) == 2
    });
    assert_eq!(harness.fixture.peak.load(Ordering::SeqCst), 2);
    harness.fixture.release.notify_waiters();
    harness.finished(&first);
    harness.finished(&second);
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn pending_connection_can_be_canceled_and_retried_in_the_same_tab() {
    let harness = harness();
    let page = harness.selected();
    page.editor().set_text("kept draft");
    harness.fixture.hold_connect.store(true, Ordering::SeqCst);
    page.connect("fixture");
    wait_until("the pending connection", || {
        page.tab().phase == TabPhase::Connecting
    });
    page.cancel_chooser();
    harness.fixture.connect_release.notify_waiters();
    wait_until("the retained chooser", || {
        page.tab().phase == TabPhase::Choosing
    });
    assert_eq!(harness.shell.workspace().tab_count(), 1);
    assert!(!page.is_workspace());
    assert_eq!(page.editor().text(), "kept draft");

    harness.fixture.hold_connect.store(false, Ordering::SeqCst);
    page.connect("fixture");
    wait_until("the retried connection", || page.is_ready());
    assert_eq!(harness.selected().id(), page.id());
    assert_eq!(page.editor().text(), "kept draft");
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn failed_cleanup_keeps_the_tab_until_a_retry_succeeds() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.fixture.reject_close.store(true, Ordering::SeqCst);
    harness.shell.workspace().close_tab(page.id());
    wait_until("the failed cleanup", || {
        !harness.fixture.reject_close.load(Ordering::SeqCst) && page.tab().phase == TabPhase::Ready
    });
    assert_eq!(
        harness.shell.workspace().selected().unwrap().id(),
        page.id()
    );
    harness.shell.workspace().close_tab(page.id());
    wait_until("a fresh chooser", || {
        harness
            .shell
            .workspace()
            .selected()
            .is_some_and(|selected| selected.id() != page.id())
    });
    assert_eq!(harness.shell.workspace().tab_count(), 1);
    assert!(!harness.selected().is_workspace());
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn restored_workspace_reconnects_drafts_without_running_sql() {
    let directory = directory();
    std::fs::write(
        directory.0.join("gtk-workspace.json"),
        r#"{"version":1,"tabs":[{"profile":"fixture","draft":"SELECT restored"},{"draft":"scratch"}],"selected":1}"#,
    )
    .unwrap();
    let harness = harness_in(directory);
    wait_until("both restored tabs", || {
        harness.shell.workspace().tab_count() == 2
    });
    let restored = harness
        .shell
        .workspace()
        .pages()
        .into_iter()
        .find(|page| page.editor().text() == "SELECT restored")
        .unwrap();
    wait_until("the restored connection", || restored.is_ready());
    wait_until("the restored selection", || {
        harness.selected().editor().text() == "scratch"
    });
    assert_eq!(harness.fixture.executions.load(Ordering::SeqCst), 0);
    assert!(harness.directory.0.join("gtk-workspace.json").exists());
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn restoration_never_exposes_a_temporary_connection_chooser() {
    let directory = directory();
    std::fs::write(
        directory.0.join("gtk-workspace.json"),
        r#"{"version":1,"tabs":[{"profile":"fixture","draft":"SELECT saved"}],"selected":0}"#,
    )
    .unwrap();
    let fixture = Arc::new(Fixture::default());
    fixture.hold_connect.store(true, Ordering::SeqCst);
    let harness = harness_with_fixture(directory, fixture.clone());
    assert!(harness.shell.workspace().is_restoring());
    assert!(!harness.selected().is_workspace());
    assert!(!harness.selected().widget().is_mapped());
    harness.capture("restoring-workspace");
    fixture.hold_connect.store(false, Ordering::SeqCst);
    fixture.connect_release.notify_one();
    wait_until("restored workspace visibility", || {
        !harness.shell.workspace().is_restoring()
    });
    assert!(harness.selected().is_workspace());
    assert!(!harness.selected().is_transitioning());
    assert_eq!(harness.selected().editor().text(), "SELECT saved");
    assert_eq!(harness.fixture.executions.load(Ordering::SeqCst), 0);
    harness.capture("restored-workspace");
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn restoration_reconciles_pending_connection_updates_before_exposing_tabs() {
    let harness = harness();
    let workspace = harness.shell.workspace();
    let page = harness.selected();
    wait_until("the mapped chooser", || page.widget().is_mapped());
    workspace.set_restoring(true);
    let context = harness.shell.app().context();
    let _connection = {
        let _runtime = harness.shell.app().bridge().enter();
        context.start_connect_in_tab(
            page.id(),
            harness.shell.app().resolve_profile("fixture").unwrap(),
        )
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while context.tabs.find(page.id()).unwrap().engine.is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!page.is_workspace());
    workspace.set_restoring(false);
    assert!(page.is_workspace());
    assert!(!page.is_transitioning());
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn connection_recovery_updates_the_footer_without_replacing_results_or_editor() {
    let harness = harness();
    super::window::install_styles();
    let page = harness.connect("fixture");
    harness.run(&page, "rows");
    harness.finished(&page);
    wait_until("the original results", || {
        page.results().available_rows() == 3
    });
    let document = page.tab().document.id();
    let generation = page.document().unwrap().generation;
    page.editor().set_text("SELECT next");
    harness.fixture.hold_connect.store(true, Ordering::SeqCst);
    harness.fixture.disconnected.store(true, Ordering::SeqCst);
    let context = harness.shell.app().context().clone();
    let check = {
        let _runtime = harness.shell.app().bridge().enter();
        context.health.check_tab(&context, page.id())
    };
    let completed = harness.shell.app().run(check.wait());
    glib::spawn_future_local(async move { completed.await.unwrap().unwrap() });
    wait_until("the reconnecting indicator", || {
        harness
            .shell
            .footer()
            .indicator_has_class("status-connecting")
    });
    harness.capture("reconnecting-indicator");
    let pulse = Instant::now() + Duration::from_millis(400);
    wait_until("the next pulse phase", || Instant::now() >= pulse);
    harness.capture("reconnecting-indicator-pulse");
    harness.fixture.reject_connect.store(true, Ordering::SeqCst);
    harness.fixture.hold_connect.store(false, Ordering::SeqCst);
    harness.fixture.connect_release.notify_one();
    wait_until("the retry waiting indicator", || {
        harness.shell.footer().indicator_has_class("status-error")
    });
    harness.capture("retry-waiting-indicator");
    let retry = {
        let _runtime = harness.shell.app().bridge().enter();
        context.health.check_tab(&context, page.id())
    };
    let completed = harness.shell.app().run(retry.wait());
    glib::spawn_future_local(async move { completed.await.unwrap().unwrap() });
    wait_until("the recovery notice", || {
        harness
            .shell
            .footer()
            .indicator_has_class("status-connected")
    });
    assert_eq!(harness.shell.footer().connection_text(), "fixture");
    assert_eq!(harness.shell.workspace().tab_count(), 1);
    assert!(page.is_workspace());
    assert_eq!(page.tab().document.id(), document);
    assert_eq!(page.document().unwrap().generation, generation);
    assert_eq!(page.editor().text(), "SELECT next");
    assert_eq!(page.results().available_rows(), 3);
    assert_eq!(harness.fixture.executions.load(Ordering::SeqCst), 1);
    harness.capture("recovered-connection");
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn escape_closes_the_row_preview_before_canceling_a_running_query() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "rows");
    harness.finished(&page);
    wait_until("streamed rows", || page.results().available_rows() == 3);
    assert!(page.results().focus());
    assert!(page.results().show_row_preview());
    assert!(page.results().inspector_visible());
    assert_eq!(
        harness
            .shell
            .press(gdk::Key::Escape, gdk::ModifierType::empty()),
        glib::Propagation::Stop
    );
    assert!(!page.results().inspector_visible());

    harness.run(&page, "wait");
    wait_until("the stop action", || {
        harness.shell.run_button().tooltip_text().as_deref() == Some("Stop query (Escape)")
    });
    assert_eq!(
        harness
            .shell
            .press(gdk::Key::Escape, gdk::ModifierType::empty()),
        glib::Propagation::Stop
    );
    harness.shell.release(gdk::Key::Escape);
    harness.finished(&page);
    assert_eq!(
        page.document().unwrap().feedback.status,
        ResultStatusReason::Canceled
    );
    wait_until("the run action", || {
        harness.shell.app().context().tabs.controls().primary_action == PrimaryAction::Run
    });
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn copying_selected_rows_publishes_complete_markdown() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "rows");
    harness.finished(&page);
    wait_until("streamed rows", || page.results().available_rows() == 3);
    page.results().focus();
    page.results().toggle_result_rows();
    page.results().copy_selection();
    wait_until("the copy to finish", || !page.results().is_copying());
    let clipboard = page.results().widget().clipboard();
    let text = glib::MainContext::default()
        .block_on(clipboard.read_text_future())
        .unwrap()
        .unwrap();
    assert!(text.starts_with("| # | id | value |"), "{text}");
    assert!(text.contains("row 0"), "{text}");
}

fn result_key(view: &gtk4::ColumnView, key: gdk::Key, modifiers: gdk::ModifierType) {
    let controllers = view.observe_controllers();
    let keys = (0..controllers.n_items())
        .find_map(|index| {
            controllers
                .item(index)
                .and_downcast::<gtk4::EventControllerKey>()
        })
        .unwrap();
    assert!(keys.emit_by_name::<bool>("key-pressed", &[&key, &0_u32, &modifiers]));
}

fn copied_results(results: &super::results::ResultPanel) -> String {
    results.copy_selection();
    wait_until("result clipboard export", || !results.is_copying());
    glib::MainContext::default()
        .block_on(results.widget().clipboard().read_text_future())
        .unwrap()
        .unwrap()
        .to_string()
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn returning_to_results_keeps_native_focus_on_the_shared_cursor_row() {
    super::window::install_styles();
    fn has_label(widget: &gtk4::Widget, text: &str) -> bool {
        if widget
            .downcast_ref::<gtk4::Label>()
            .is_some_and(|label| label.text() == text)
        {
            return true;
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if has_label(&widget, text) {
                return true;
            }
            child = widget.next_sibling();
        }
        false
    }
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "rows");
    harness.finished(&page);
    wait_until("result rows", || page.results().available_rows() == 3);
    page.results().focus();
    let view = page.results().table().unwrap();
    for _ in 0..2 {
        result_key(&view, gdk::Key::Down, gdk::ModifierType::NO_MODIFIER_MASK);
    }
    assert_eq!(page.results().selected(), Some(2));
    assert!(page.editor().focus());
    assert!(page.results().focus());
    gtk4::test_widget_wait_for_draw(&view);
    let window = view.root().and_downcast::<gtk4::Window>().unwrap();
    wait_until("native focus on row three", || {
        let Some(focus) = gtk4::prelude::GtkWindowExt::focus(&window) else {
            return false;
        };
        has_label(&focus, "3")
    });
    assert_eq!(page.results().selected(), Some(2));
    assert_eq!(copied_results(page.results()), "2");
    wait_until("the cursor cell to render", || {
        has_label(view.upcast_ref(), "row 2")
    });
    harness.capture("grid-restored-cursor-focus");
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn result_cells_ranges_headers_and_row_numbers_have_distinct_copy_contracts() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "rows");
    harness.finished(&page);
    wait_until("result rows", || page.results().available_rows() == 3);
    page.results().focus();
    let view = page.results().table().unwrap();
    assert_eq!(copied_results(page.results()), "0");
    result_key(&view, gdk::Key::Right, gdk::ModifierType::SHIFT_MASK);
    assert!(copied_results(page.results()).starts_with("| id | value |\n"));
    result_key(&view, gdk::Key::Down, gdk::ModifierType::SHIFT_MASK);
    let rectangle = copied_results(page.results());
    assert!(
        rectangle.contains("| 0 | row 0 |\n") && rectangle.contains("| 1 | NULL |\n"),
        "{rectangle}"
    );
    result_key(&view, gdk::Key::Up, gdk::ModifierType::SHIFT_MASK);
    result_key(&view, gdk::Key::space, gdk::ModifierType::CONTROL_MASK);
    assert!(copied_results(page.results()).starts_with("| # | id | value |\n"));
    result_key(&view, gdk::Key::space, gdk::ModifierType::CONTROL_MASK);
    assert_eq!(copied_results(page.results()), "row 0");
    page.results().select_cell_for_test(0, 0, false);
    assert_eq!(copied_results(page.results()), "| # |\n| --- |\n| 1 |\n");
    let header = view
        .first_child()
        .unwrap()
        .first_child()
        .unwrap()
        .next_sibling()
        .unwrap();
    let controllers = header.observe_controllers();
    gtk4::test_widget_wait_for_draw(&view);
    wait_until("allocated result header", || header.width() > 32);
    let click = (0..controllers.n_items())
        .filter_map(|index| controllers.item(index).and_downcast::<gtk4::GestureClick>())
        .find(|click| click.name().as_deref() == Some("result-column-selection"))
        .unwrap();
    click.emit_by_name::<()>("pressed", &[&1_i32, &20.0_f64, &8.0_f64]);
    assert_eq!(
        copied_results(page.results()),
        "| id |\n| --- |\n| 0 |\n| 1 |\n| 2 |\n"
    );
    fn number(widget: &gtk4::Widget) -> Option<gtk4::Label> {
        if let Some(label) = widget.downcast_ref::<gtk4::Label>()
            && label.text() == "2"
            && label.has_css_class("dim-label")
        {
            return Some(label.clone());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if let Some(label) = number(&widget) {
                return Some(label);
            }
            child = widget.next_sibling();
        }
        None
    }
    let number = number(view.upcast_ref()).unwrap();
    let click = [Some(number.clone().upcast()), number.parent()]
        .into_iter()
        .flatten()
        .flat_map(|widget| {
            let controllers = widget.observe_controllers();
            (0..controllers.n_items()).filter_map(move |index| {
                controllers.item(index).and_downcast::<gtk4::GestureClick>()
            })
        })
        .find(|click| click.name().as_deref() == Some("result-row-selection"))
        .unwrap();
    click.emit_by_name::<()>("pressed", &[&1_i32, &20.0_f64, &8.0_f64]);
    assert!(copied_results(page.results()).contains("| 2 | 1 | NULL |\n"));
    harness.capture("grid-cell-column-selection");
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn sticky_result_columns_preserve_scroll_and_restore_with_matching_schema() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "wide");
    harness.finished(&page);
    wait_until("result rows", || page.results().available_rows() == 500);
    let view = page.results().table().unwrap();
    wait_until("result geometry", || {
        view.hadjustment().unwrap().upper() > 3000.0
    });
    let h = view.hadjustment().unwrap();
    let v = view.vadjustment().unwrap();
    h.set_value(1500.0);
    v.set_value(1200.0);
    gtk4::test_widget_wait_for_draw(&view);
    assert!(
        (h.value() - 1500.0).abs() < 1.0 && (v.value() - 1200.0).abs() < 1.0,
        "scroll before lock: {:?}",
        (h.value(), v.value())
    );
    page.results()
        .widget()
        .activate_action("results.freeze-40", None)
        .unwrap();
    let (left, right) = page.results().sticky_views();
    wait_until("sticky columns", || left.is_visible() && right.is_visible());
    gtk4::test_widget_wait_for_draw(&view);
    assert!(
        (h.value() - 1500.0).abs() < 1.0 && (v.value() - 1200.0).abs() < 1.0,
        "scroll after lock: {:?}",
        (h.value(), v.value())
    );
    assert!(
        left.width() >= 80 && left.width() < 100,
        "left width {}",
        left.width()
    );
    assert!(
        right.width() > 0 && right.width() < 400,
        "right width {}",
        right.width()
    );
    let right_view = right.child().and_downcast::<gtk4::ColumnView>().unwrap();
    assert!((right_view.vadjustment().unwrap().value() - v.value()).abs() < 1.0);
    let before = (h.value(), v.value());
    page.results().select_cell_for_test(50, 40, false);
    result_key(&view, gdk::Key::space, gdk::ModifierType::CONTROL_MASK);
    result_key(&view, gdk::Key::space, gdk::ModifierType::CONTROL_MASK);
    assert!((h.value() - before.0).abs() < 1.0);
    harness.capture("grid-frozen-both-edges");
    harness.run(&page, "wide");
    harness.finished(&page);
    wait_until("restored sticky columns", || {
        page.results().table().is_some_and(|current| {
            current != view && (current.hadjustment().unwrap().value() - before.0).abs() < 1.0
        }) && page.results().sticky_views().1.is_visible()
    });
    harness.capture("grid-frozen-restored");
    harness.run(&page, "rows");
    harness.finished(&page);
    wait_until("new result schema", || page.results().available_rows() == 3);
    assert!(!page.results().sticky_views().1.is_visible());
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn a_selected_row_survives_its_page_loading() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "wide");
    harness.finished(&page);
    wait_until("streamed rows", || page.results().available_rows() == 500);
    let table = page.results().table().unwrap();
    gtk4::test_widget_wait_for_draw(&table);
    wait_until("the first page", || !page.results().is_loading());
    table.model().unwrap().select_item(450, true);
    let vertical = table.vadjustment().unwrap();
    vertical.set_value(vertical.upper());
    gtk4::test_widget_wait_for_draw(&table);
    wait_until("the selected page", || !page.results().is_loading());
    assert_eq!(page.results().selected(), Some(450));
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn rerunning_the_same_query_returns_to_the_scrolled_view() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "wide slowly");
    harness.finished(&page);
    wait_until("streamed rows", || page.results().available_rows() == 500);
    let table = page.results().table().unwrap();
    let (horizontal, vertical) = (table.hadjustment().unwrap(), table.vadjustment().unwrap());
    wait_until("a scrollable table", || {
        horizontal.upper() > 2000.0 && vertical.upper() > 10000.0
    });
    horizontal.set_value(horizontal.upper() - horizontal.page_size() - 100.0);
    vertical.set_value(vertical.upper() - vertical.page_size() - 100.0);
    gtk4::test_widget_wait_for_draw(&table);
    let before = (horizontal.value(), vertical.value());
    harness.run(&page, "wide slowly");
    harness.finished(&page);
    wait_until("the scrolled view", || {
        page.results().table().is_some_and(|current| {
            current != table
                && (current.hadjustment().unwrap().value() - before.0).abs() < 1.0
                && (current.vadjustment().unwrap().value() - before.1).abs() < 1.0
        })
    });
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn repeated_fast_queries_keep_the_viewport_after_layout_settles() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "wide");
    harness.finished(&page);
    wait_until("streamed rows", || page.results().available_rows() == 500);
    let table = page.results().table().unwrap();
    let stack = table
        .ancestor(gtk4::Stack::static_type())
        .and_downcast::<gtk4::Stack>()
        .unwrap();
    wait_until("the workspace transition", || {
        !stack.is_transition_running()
    });
    wait_until("the first page", || !page.results().is_loading());
    let (horizontal, vertical) = (table.hadjustment().unwrap(), table.vadjustment().unwrap());
    wait_until("a scrollable table", || {
        horizontal.upper() > 2000.0 && vertical.upper() > 10000.0
    });
    horizontal.set_value(1700.0);
    vertical.set_value(8000.0);
    gtk4::test_widget_wait_for_draw(&table);
    wait_until("the visible page", || !page.results().is_loading());
    for _ in 0..3 {
        gtk4::test_widget_wait_for_draw(&table);
    }
    let before = (horizontal.value(), vertical.value());
    harness.capture("scroll-before");
    for repeat in 0..6 {
        harness.run(&page, "wide");
        harness.finished(&page);
        wait_until("the replacement table", || {
            page.results().available_rows() == 500
                && page
                    .results()
                    .table()
                    .is_some_and(|current| current != table)
        });
        let current = page.results().table().unwrap();
        assert!(
            page.results().issues().is_empty(),
            "a retired result reader reported an error in the new result"
        );
        let mut stable_since = None;
        let started = Instant::now();
        wait_until("the restored viewport", || {
            assert!(
                started.elapsed() < Duration::from_secs(4),
                "expected {before:?}, current {:?}, extent {:?}",
                (
                    current.hadjustment().unwrap().value(),
                    current.vadjustment().unwrap().value()
                ),
                (
                    current.hadjustment().unwrap().upper(),
                    current.vadjustment().unwrap().upper()
                )
            );
            let restored = (current.hadjustment().unwrap().value() - before.0).abs() < 1.0
                && (current.vadjustment().unwrap().value() - before.1).abs() < 1.0
                && !page.results().is_loading();
            if !restored {
                stable_since = None;
                return false;
            }
            stable_since.get_or_insert_with(Instant::now).elapsed() >= Duration::from_millis(200)
        });
        for frame in 0..12 {
            gtk4::test_widget_wait_for_draw(&current);
            assert!(
                (current.hadjustment().unwrap().value() - before.0).abs() <= 2.0
                    && (current.vadjustment().unwrap().value() - before.1).abs() <= 2.0,
                "repeat {repeat}, frame {frame}: expected {before:?}, got {:?}",
                (
                    current.hadjustment().unwrap().value(),
                    current.vadjustment().unwrap().value()
                )
            );
        }
    }
    harness.capture("scroll-after");
    let current = page.results().table().unwrap();
    let controllers = page.results().widget().observe_controllers();
    let wheel = (0..controllers.n_items())
        .find_map(|index| {
            controllers
                .item(index)
                .and_downcast::<gtk4::EventControllerScroll>()
        })
        .unwrap();
    wheel.emit_by_name::<bool>("scroll", &[&0.0_f64, &1.0_f64]);
    current.vadjustment().unwrap().set_value(4000.0);
    for _ in 0..12 {
        gtk4::test_widget_wait_for_draw(&current);
        assert!(
            current.vadjustment().unwrap().value() < 4500.0,
            "saved scroll overrode a user scroll"
        );
    }
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn long_values_use_spare_table_width_without_stretching_short_columns() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "long value");
    harness.finished(&page);
    wait_until("streamed rows", || page.results().available_rows() == 3);
    let table = page.results().table().unwrap();
    gtk4::test_widget_wait_for_draw(&table);
    wait_until("loaded values", || !page.results().is_loading());
    for _ in 0..3 {
        gtk4::test_widget_wait_for_draw(&table);
    }
    let columns = table.columns();
    let id = columns
        .item(1)
        .and_downcast::<gtk4::ColumnViewColumn>()
        .unwrap();
    let value = columns
        .item(2)
        .and_downcast::<gtk4::ColumnViewColumn>()
        .unwrap();
    assert!(id.fixed_width() < 100);
    assert!(
        value.fixed_width() > 600,
        "long value width: {}",
        value.fixed_width()
    );
    assert!(table.hadjustment().unwrap().upper() <= table.hadjustment().unwrap().page_size() + 1.0);
    let stack = table
        .ancestor(gtk4::Stack::static_type())
        .and_downcast::<gtk4::Stack>()
        .unwrap();
    wait_until("the workspace transition", || {
        !stack.is_transition_running()
    });
    gtk4::test_widget_wait_for_draw(&table);
    harness.capture("columns-wide");
    let wide = value.fixed_width();
    harness.window.set_default_size(700, 760);
    wait_until("columns fitting a smaller window", || {
        value.fixed_width() < wide
    });
    harness.window.set_default_size(1150, 760);
    wait_until("columns using the larger window", || {
        value.fixed_width() == wide
    });
    value.set_fixed_width(420);
    harness.window.set_default_size(900, 760);
    gtk4::test_widget_wait_for_draw(&table);
    assert_eq!(value.fixed_width(), 420);
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn toggling_history_keeps_the_header_controls_in_place() {
    let harness = harness();
    harness.connect("fixture");
    gtk4::test_widget_wait_for_draw(&harness.window);
    let position = |widget: &gtk4::Widget| {
        widget
            .compute_point(&harness.window, &gtk4::graphene::Point::new(0.0, 0.0))
            .map(|point| (point.x(), point.y()))
            .unwrap()
    };
    let toggle = harness
        .shell
        .history_button()
        .clone()
        .upcast::<gtk4::Widget>();
    let search = harness
        .shell
        .search()
        .widget()
        .clone()
        .upcast::<gtk4::Widget>();
    let before = (position(&toggle), position(&search));
    harness.shell.execute(ShortcutAction::ToggleHistory);
    gtk4::test_widget_wait_for_draw(&harness.window);
    let after = (position(&toggle), position(&search));
    assert!(harness.selected().history_visible());
    assert!((before.0.0 - after.0.0).abs() < 1.0 && (before.0.1 - after.0.1).abs() < 1.0);
    assert!((before.1.0 - after.1.0).abs() < 1.0 && (before.1.1 - after.1.1).abs() < 1.0);
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn history_restore_shows_the_saved_sample_without_executing() {
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "rows");
    harness.finished(&page);
    harness.shell.execute(ShortcutAction::FocusHistory);
    page.refresh_history();
    wait_until("the saved query", || page.history().entry_count() == 1);
    page.editor().set_text("draft");
    let executions = harness.fixture.executions.load(Ordering::SeqCst);
    page.history().activate(0);
    wait_until("the saved output", || {
        page.document()
            .is_some_and(|document| document.history == Some(true))
    });
    assert_eq!(page.editor().text(), "rows");
    assert_eq!(
        harness.fixture.executions.load(Ordering::SeqCst),
        executions
    );
    wait_until("saved rows", || page.results().available_rows() == 3);
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn plan_chord_estimates_then_pins_through_the_shared_input() {
    let harness = harness();
    let page = harness.connect("fixture");
    page.editor().set_text("rows");
    page.editor().focus();
    let control = gdk::ModifierType::CONTROL_MASK;
    let shift = gdk::ModifierType::SHIFT_MASK;
    harness
        .shell
        .press(gdk::Key::Control_L, gdk::ModifierType::empty());
    harness.shell.press(gdk::Key::Shift_L, control);
    wait_until("the estimate", || {
        page.interaction().is_some_and(|interaction| {
            matches!(interaction.input.hint, InputHint::EstimateReady { .. })
        })
    });
    assert_eq!(
        harness.shell.press(gdk::Key::space, control | shift),
        glib::Propagation::Stop
    );
    harness.shell.release(gdk::Key::space);
    harness.shell.release(gdk::Key::Shift_L);
    harness.shell.release(gdk::Key::Control_L);
    wait_until("the pinned plan", || {
        harness.shell.plan().is_visible() && harness.shell.plan().is_pinned()
    });
    assert_eq!(harness.fixture.executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        harness
            .shell
            .press(gdk::Key::Escape, gdk::ModifierType::empty()),
        glib::Propagation::Stop
    );
    assert!(!harness.shell.plan().is_visible());
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn closing_the_last_unconnected_chooser_saves_and_shuts_down() {
    let harness = harness();
    harness.selected().editor().set_text("unsent");
    harness.shell.execute(ShortcutAction::CloseTab);
    wait_until("the shared shutdown", || {
        *harness.shell.app().context().subscribe_phase().borrow()
            == dzban_core::application::ApplicationPhase::Closed
    });
    let saved = std::fs::read_to_string(harness.directory.0.join("gtk-workspace.json")).unwrap();
    assert!(saved.contains("unsent"), "{saved}");
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn vim_letters_navigate_lists_but_still_type_in_the_editor() {
    let harness = harness();
    let none = gdk::ModifierType::empty();
    let chooser = harness.selected().chooser().clone();
    assert!(chooser.focus());
    assert_eq!(chooser.selected_index(), Some(0));
    assert_eq!(
        harness.shell.press(gdk::Key::j, none),
        glib::Propagation::Stop
    );
    assert_eq!(chooser.selected_index(), Some(1));
    assert_eq!(
        harness.shell.press(gdk::Key::k, none),
        glib::Propagation::Stop
    );
    assert_eq!(chooser.selected_index(), Some(0));

    let page = harness.connect("fixture");
    harness.run(&page, "rows");
    harness.finished(&page);
    wait_until("streamed rows", || page.results().available_rows() == 3);
    assert!(page.results().focus());
    wait_until("focus in the results", || page.results().focused());
    assert_eq!(page.results().selected(), Some(0));
    assert_eq!(page.results().column(), 0);
    assert_eq!(
        harness.shell.press(gdk::Key::j, none),
        glib::Propagation::Stop
    );
    wait_until("the next row", || page.results().selected() == Some(1));
    assert_eq!(
        harness.shell.press(gdk::Key::l, none),
        glib::Propagation::Stop
    );
    assert_eq!(page.results().column(), 1);
    assert!(page.results().show_row_preview());
    assert_eq!(
        harness.shell.press(gdk::Key::l, none),
        glib::Propagation::Stop
    );
    wait_until("the previewed row to move", || {
        page.results().selected() == Some(2)
    });
    assert!(page.results().close_inspector());

    assert!(page.editor().focus());
    assert_eq!(
        harness.shell.press(gdk::Key::j, none),
        glib::Propagation::Proceed
    );
}

fn descendant(root: &gtk4::Widget, css: &str) -> Option<gtk4::Widget> {
    let mut child = root.first_child();
    while let Some(widget) = child {
        if widget.css_name() == css && widget.height() > 0 {
            return Some(widget);
        }
        if let Some(found) = descendant(&widget, css) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn layout_follows_the_native_mac_constants() {
    super::window::install_styles();
    let harness = harness();
    let page = harness.connect("fixture");
    harness.run(&page, "rows");
    harness.finished(&page);
    wait_until("streamed rows", || page.results().available_rows() == 3);
    harness.shell.execute(ShortcutAction::ToggleHistory);
    assert!(page.results().focus());
    assert!(page.results().show_row_preview());
    harness.shell.execute(ShortcutAction::OpenPlan);
    wait_until("the plan", || harness.shell.plan().is_visible());
    gtk4::test_widget_wait_for_draw(&harness.window);

    let window = harness.window.default_size();
    let search = harness.shell.search().widget().width();
    let history = page.history().widget().width();
    let tabs = harness.shell.workspace().bar().height();
    let footer = harness.shell.footer().widget().height();
    let editor = page.editor().widget().height();
    let results = page.results().widget().height();
    let plan = harness.shell.plan().widget();
    let plan = (plan.width(), plan.height());
    let inspector = page.results().inspector().widget();
    let inspector = (inspector.width(), inspector.height());
    let row = page
        .results()
        .table()
        .and_then(|table| descendant(table.upcast_ref(), "row"))
        .map_or(0, |row| row.height());
    let bounds = |widget: &gtk4::Widget| {
        widget
            .compute_bounds(&harness.window)
            .map_or(0, |bounds| bounds.height().round() as i32)
    };
    let buttons = [
        bounds(harness.shell.history_button().upcast_ref()),
        bounds(harness.shell.plan_button().upcast_ref()),
        bounds(harness.shell.run_button().upcast_ref()),
    ];
    let report = format!(
        "| Element | Mac | GTK |\n| --- | --- | --- |\n\
         | Default window | 1150x760 | {}x{} |\n\
         | Action search width | 380-600 | {search} |\n\
         | History pane width | 240-320 | {history} |\n\
         | Tab strip height | 40 | {tabs} |\n\
         | Footer height | 30 | {footer} |\n\
         | Editor min height | 120 | {editor} |\n\
         | Results min height | 160 | {results} |\n\
         | Result row height | 24 | {row} |\n\
         | Plan preview | 620x400 | {}x{} |\n\
         | Row preview | 640x460 | {}x{} |\n\
         | Toolbar buttons | 28 | {buttons:?} |\n",
        window.0, window.1, plan.0, plan.1, inspector.0, inspector.1,
    );
    if let Some(path) = std::env::var_os("DZBAN_LAYOUT_REPORT") {
        std::fs::write(path, &report).unwrap();
    }
    assert_eq!(window, (1150, 760), "{report}");
    assert!((380..=600).contains(&search), "{report}");
    assert!((240..=320).contains(&history), "{report}");
    assert!((30..=34).contains(&footer), "{report}");
    assert!(
        buttons.iter().all(|height| (28..=36).contains(height)),
        "{report}"
    );
    assert!(editor >= 120 && results >= 160, "{report}");
    assert!((24..=28).contains(&row), "{report}");
    assert_eq!(plan, (620, 400), "{report}");
    assert_eq!(inspector, (640, 460), "{report}");
}
