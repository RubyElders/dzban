use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::ApplicationContext;
use dzban_core::error::ExecutionError;
use dzban_core::{DatabaseBackend, DatabaseConfig, QueryPlan, ResultSet};
use dzban_gtk::database::{ConnectionState, DbConnection, RuntimeBridge};
use glib::prelude::*;
use libadwaita::prelude::AdwApplicationWindowExt;

#[derive(Clone, Copy)]
enum Stage {
    Ready,
    Ping,
    Query,
    Connect,
    Version,
    DisconnectFailure,
}

#[derive(Default)]
struct Gate {
    entered: AtomicBool,
    release: tokio::sync::Notify,
    closed: AtomicBool,
    cancellations: AtomicUsize,
}

impl Gate {
    async fn wait(&self) {
        self.entered.store(true, Ordering::SeqCst);
        self.release.notified().await;
    }
}

struct Session {
    number: usize,
    stage: Stage,
    gate: Arc<Gate>,
}

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        if self.number == 1 && matches!(self.stage, Stage::Connect) {
            self.gate.wait().await;
        }
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        if matches!(self.stage, Stage::DisconnectFailure)
            && !self.gate.closed.swap(true, Ordering::SeqCst)
        {
            return Err(ExecutionError::Message("Fixture disconnect failed".into()));
        }
        if self.number == 1 {
            self.gate.closed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        if matches!(self.stage, Stage::Ping) {
            self.gate.wait().await;
        }
        Ok(0)
    }
    async fn execute(&self, sql: &str) -> Result<ResultSet, String> {
        if matches!(self.stage, Stage::Query) && sql == "external-wait" {
            self.gate.wait().await;
        }
        Ok(Default::default())
    }
    async fn execute_detailed(
        &self,
        sql: &str,
    ) -> Result<dzban_core::ExecutionOutcome, ExecutionError> {
        if sql == "native-failure" {
            return Err(ExecutionError::Database(dzban_core::error::DatabaseError {
                backend: DatabaseBackend::Postgresql,
                kind: dzban_core::error::ErrorKind::Server,
                message: "Fixture missing column".into(),
                code: Some(dzban_core::error::NativeErrorCode::SqlState("42703".into())),
                detail: Some("Fixture lookup detail".into()),
                hint: Some("Fixture lookup hint".into()),
            }));
        }
        Ok(dzban_core::ExecutionOutcome::Rows(self.execute(sql).await?))
    }
    async fn cancel(&self) -> Result<(), String> {
        self.gate.cancellations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn version(&self) -> Result<String, String> {
        if self.number == 1 && matches!(self.stage, Stage::Version) {
            self.gate.wait().await;
        }
        Ok(format!("Fixture{} 1.0", self.number))
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        Err("No fixture plan".into())
    }
}

struct Factory {
    created: AtomicUsize,
    stage: Stage,
    gate: Arc<Gate>,
}

impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(Arc::new(Session {
            number: self.created.fetch_add(1, Ordering::SeqCst) + 1,
            stage: self.stage,
            gate: self.gate.clone(),
        }))
    }
}

fn fixture(stage: Stage) -> (DbConnection, Arc<ApplicationContext>, Arc<Gate>) {
    let gate = Arc::new(Gate::default());
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(Factory {
            created: AtomicUsize::new(0),
            stage,
            gate: gate.clone(),
        }))
        .unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let bridge = RuntimeBridge::with_context(context.clone()).unwrap();
    let connection = DbConnection::from_config_with_bridge(
        DatabaseConfig::from_connection_string("host=localhost user=fixture dbname=fixture"),
        bridge,
    );
    (connection, context, gate)
}

async fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "Connection lifecycle did not reach the expected state"
        );
        glib::timeout_future(Duration::from_millis(5)).await;
    }
}

async fn until_session_count(context: &ApplicationContext, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while context.session_ids().await.len() != count {
        assert!(
            Instant::now() < deadline,
            "Core session registry did not reach the expected count"
        );
        glib::timeout_future(Duration::from_millis(5)).await;
    }
}

#[glib::async_test]
async fn idle_gobject_cancel_cannot_target_a_later_query() {
    let (connection, _, gate) = fixture(Stage::Query);
    connection.connect();
    until(|| connection.state() == ConnectionState::Connected).await;
    let cancellation = connection.cancel_query();
    let result = connection.execute_query_detailed("external-wait".into());
    cancellation.await.unwrap().unwrap();
    gate.release.notify_one();
    assert!(result.await.unwrap().is_ok());
    connection
        .disconnect_with_completion()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
}

#[glib::async_test]
async fn identified_gobject_cancel_cannot_target_a_restarted_query() {
    let (connection, context, gate) = fixture(Stage::Query);
    connection.connect();
    until(|| connection.state() == ConnectionState::Connected).await;
    let first = connection.execute_query_detailed("first".into());
    first.await.unwrap().unwrap();
    let engine = context
        .session(context.session_ids().await[0])
        .await
        .unwrap();
    let previous_query = engine.subscribe().borrow().phase.id().unwrap();
    let result = connection.execute_query_detailed("external-wait".into());
    until(|| gate.entered.load(Ordering::SeqCst)).await;
    let running_query = engine.subscribe().borrow().phase.id().unwrap();
    assert_ne!(previous_query, running_query);
    connection
        .cancel_query_identified(previous_query)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(gate.cancellations.load(Ordering::SeqCst), 0);
    assert_eq!(
        engine.subscribe().borrow().phase,
        dzban_core::execution::QueryPhase::Executing(running_query)
    );
    gate.release.notify_one();
    result.await.unwrap().unwrap();
    connection
        .disconnect_with_completion()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
}

#[glib::async_test]
async fn detailed_query_delivery_preserves_native_diagnostics_and_display_signals() {
    let (connection, context, _) = fixture(Stage::Ready);
    let signals = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let observed = signals.clone();
    connection.connect_closure(
        "query-error",
        false,
        glib::closure_local!(move |_connection: DbConnection, message: String| {
            observed.borrow_mut().push(message);
        }),
    );
    connection.connect();
    until(|| connection.state() == ConnectionState::Connected).await;
    let error = connection
        .execute_query_detailed("native-failure".into())
        .await
        .unwrap()
        .unwrap_err();
    let ExecutionError::Database(diagnostic) = &error else {
        panic!("Expected native diagnostic")
    };
    assert_eq!(
        diagnostic.code,
        Some(dzban_core::error::NativeErrorCode::SqlState("42703".into()))
    );
    assert_eq!(diagnostic.detail.as_deref(), Some("Fixture lookup detail"));
    assert_eq!(diagnostic.hint.as_deref(), Some("Fixture lookup hint"));
    assert_eq!(connection.last_error(), error.to_string());
    assert_eq!(signals.borrow().as_slice(), [error.to_string()]);
    assert!(connection.operation_available());
    let legacy = connection
        .execute_query_rows("native-failure".into())
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(legacy, error.to_string());
    connection
        .execute_query_detailed("restart".into())
        .await
        .unwrap()
        .unwrap();
    assert!(connection.last_error().is_empty());
    connection
        .disconnect_with_completion()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(context.session_ids().await.is_empty());
}

#[glib::async_test]
async fn core_shutdown_updates_the_connected_frontend_projection() {
    let (connection, context, _) = fixture(Stage::Ready);
    connection.connect();
    until(|| connection.state() == ConnectionState::Connected).await;
    let bridge = RuntimeBridge::with_context(context.clone()).unwrap();
    bridge.start_shutdown().unwrap().await.unwrap().unwrap();
    until(|| connection.state() == ConnectionState::Disconnected).await;
    assert!(!connection.operation_available());
    assert_eq!(connection.version(), None);
    assert_eq!(
        connection
            .explain_query("SELECT 1".into())
            .unwrap()
            .await
            .unwrap()
            .unwrap_err(),
        "Not connected"
    );
    assert_eq!(
        connection
            .execute_query_detailed("SELECT 1".into())
            .await
            .unwrap()
            .unwrap_err(),
        ExecutionError::Message("Not connected".into())
    );
    assert_eq!(
        connection
            .execute_query_rows_with_explain("SELECT 1".into())
            .unwrap_err(),
        "Not connected"
    );
    connection.ping();
    assert_eq!(
        connection.operation(),
        dzban_gtk::database::ConnectionOperation::Idle
    );
    assert!(context.session_ids().await.is_empty());
}

#[glib::async_test]
async fn recorded_bridge_query_saves_history_after_frontend_receivers_are_dropped() {
    use dzban_core::execution::HistoryPublication;
    use dzban_core::history::QueryHistoryStore;
    let directory = std::env::temp_dir().join(format!(
        "dzban-bridge-history-{}",
        glib::uuid_string_random()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let history = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    let (connection, context, _) = fixture(Stage::Ready);
    connection.connect();
    until(|| connection.state() == ConnectionState::Connected).await;
    let engine = context
        .session(context.session_ids().await[0])
        .await
        .unwrap();
    let bridge = RuntimeBridge::with_context(context.clone()).unwrap();
    let (query_id, (result, plan)) = bridge
        .start_planned_identified(
            dzban_gtk::database::InternalConnection::new(
                engine.clone(),
                dzban_core::DatabaseBackend::Postgresql,
            ),
            "SELECT 1".into(),
            Some(history),
        )
        .unwrap();
    drop(result);
    drop(plan);
    until(|| *engine.subscribe_history().borrow() == HistoryPublication::Published(query_id)).await;
    let history = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    assert_eq!(history.entries().len(), 1);
    assert_eq!(history.entries()[0].sql, "SELECT 1");
    assert_eq!(
        history.entries()[0].planning_error,
        Some(ExecutionError::Message("No fixture plan".into()))
    );
    connection.disconnect();
    until_session_count(&context, 0).await;
    std::fs::remove_dir_all(directory).unwrap();
}

#[glib::async_test]
async fn core_busy_operation_updates_the_frontend_and_preserves_admission() {
    let (connection, context, gate) = fixture(Stage::Ping);
    connection.connect();
    until(|| connection.state() == ConnectionState::Connected).await;
    let engine = context
        .session(context.session_ids().await[0])
        .await
        .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let ping = {
        let _runtime = runtime.enter();
        engine.start_ping().unwrap()
    };
    until(|| gate.entered.load(Ordering::SeqCst)).await;
    until(|| connection.operation() == dzban_gtk::database::ConnectionOperation::Ping).await;
    assert!(!connection.operation_available());
    assert_eq!(
        connection
            .execute_query_detailed("SELECT 1".into())
            .await
            .unwrap()
            .unwrap_err(),
        ExecutionError::Busy
    );
    assert_eq!(
        connection
            .execute_query_rows_with_explain("SELECT 1".into())
            .unwrap_err(),
        ExecutionError::Busy.to_string()
    );
    assert_eq!(
        connection.explain_query("SELECT 1".into()).unwrap_err(),
        ExecutionError::Busy.to_string()
    );
    connection.ping();
    assert_eq!(
        connection.operation(),
        dzban_gtk::database::ConnectionOperation::Ping
    );
    gate.release.notify_one();
    ping.wait().await.unwrap();
    until(|| connection.operation_available()).await;
    connection
        .execute_query_detailed("restart".into())
        .await
        .unwrap()
        .unwrap();
    connection
        .disconnect_with_completion()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(context.session_ids().await.is_empty());
}

#[glib::async_test]
async fn externally_started_queries_drive_frontend_running_and_idle_state() {
    let (connection, context, gate) = fixture(Stage::Query);
    connection.connect();
    until(|| connection.state() == ConnectionState::Connected).await;
    let engine = context
        .session(context.session_ids().await[0])
        .await
        .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let query = {
        let _runtime = runtime.enter();
        engine.start_detailed("external-wait".into()).unwrap()
    };
    until(|| gate.entered.load(Ordering::SeqCst)).await;
    until(|| connection.operation() == dzban_gtk::database::ConnectionOperation::Query).await;
    assert!(connection.query_running());
    assert!(!connection.operation_available());
    gate.release.notify_one();
    query.wait_detailed().await.unwrap();
    until(|| connection.operation_available()).await;
    assert!(!connection.query_running());
    connection
        .disconnect_with_completion()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(context.session_ids().await.is_empty());
}

#[glib::async_test]
async fn disposing_the_frontend_removes_observers_without_abandoning_core_work() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for _ in 0..5 {
        let (connection, context, gate) = fixture(Stage::Ping);
        connection.connect();
        until(|| connection.state() == ConnectionState::Connected).await;
        let engine = context
            .session(context.session_ids().await[0])
            .await
            .unwrap();
        let ping = {
            let _runtime = runtime.enter();
            engine.start_ping().unwrap()
        };
        until(|| connection.operation() == dzban_gtk::database::ConnectionOperation::Ping).await;
        let weak = connection.downgrade();
        drop(connection);
        assert!(weak.upgrade().is_none());
        assert!(matches!(
            *engine.subscribe_operation().borrow(),
            dzban_core::execution::OperationState::Ping
        ));
        gate.release.notify_one();
        ping.wait().await.unwrap();
        let shutdown = {
            let _runtime = runtime.enter();
            context.start_shutdown().unwrap()
        };
        shutdown.wait().await.unwrap();
        assert!(context.session_ids().await.is_empty());
    }
}

#[glib::async_test]
async fn bridge_shutdown_drains_pending_work_without_a_tokio_caller() {
    use dzban_core::application::{ApplicationError, ApplicationPhase};
    let gate = Arc::new(Gate::default());
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(Factory {
            created: AtomicUsize::new(0),
            stage: Stage::Connect,
            gate: gate.clone(),
        }))
        .unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let bridge = RuntimeBridge::with_context(context.clone()).unwrap();
    let connection = bridge.start_connect(DatabaseConfig::from_connection_string(
        "host=localhost user=fixture dbname=fixture",
    ));
    until(|| gate.entered.load(Ordering::SeqCst)).await;
    let completion = bridge.start_shutdown().unwrap();
    assert_eq!(
        *context.subscribe_phase().borrow(),
        ApplicationPhase::Draining
    );
    assert!(matches!(
        bridge.start_shutdown(),
        Err(ApplicationError::Operation(ExecutionError::Busy))
    ));
    gate.release.notify_one();
    let connected = connection.result.await.unwrap().unwrap();
    completion.await.unwrap().unwrap();
    assert_eq!(
        *context.subscribe_phase().borrow(),
        ApplicationPhase::Closed
    );
    assert!(context.session_ids().await.is_empty());
    assert!(gate.closed.load(Ordering::SeqCst));
    drop(connected);
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn window_close_waits_for_pending_connections_and_backend_cleanup() {
    use gtk4::prelude::*;
    glib::MainContext::ref_thread_default().block_on(async {
        libadwaita::init().unwrap();
        let (connection, context, gate) = fixture(Stage::Connect);
        let bridge = RuntimeBridge::with_context(context.clone()).unwrap();
        let window = libadwaita::ApplicationWindow::builder().build();
        let overlay = libadwaita::ToastOverlay::new();
        window.set_content(Some(&overlay));
        dzban_gtk::shutdown::bind_window_shutdown(&window, bridge, &overlay);
        window.present();
        gtk4::test_widget_wait_for_draw(&window);
        connection.connect();
        until(|| gate.entered.load(Ordering::SeqCst)).await;
        window.close();
        assert!(window.is_visible());
        assert_eq!(
            *context.subscribe_phase().borrow(),
            dzban_core::application::ApplicationPhase::Draining
        );
        window.close();
        assert!(window.is_visible());
        gate.release.notify_one();
        until(|| !window.is_visible()).await;
        assert!(context.session_ids().await.is_empty());
        assert!(gate.closed.load(Ordering::SeqCst));
        assert_eq!(
            *context.subscribe_phase().borrow(),
            dzban_core::application::ApplicationPhase::Closed
        );
    });
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn window_close_failure_keeps_the_window_open_until_successful_retry() {
    use gtk4::prelude::*;
    glib::MainContext::ref_thread_default().block_on(async {
        libadwaita::init().unwrap();
        let (connection, context, _) = fixture(Stage::DisconnectFailure);
        let bridge = RuntimeBridge::with_context(context.clone()).unwrap();
        let window = libadwaita::ApplicationWindow::builder().build();
        let overlay = libadwaita::ToastOverlay::new();
        window.set_content(Some(&overlay));
        dzban_gtk::shutdown::bind_window_shutdown(&window, bridge, &overlay);
        window.present();
        gtk4::test_widget_wait_for_draw(&window);
        connection.connect();
        until(|| connection.state() == ConnectionState::Connected).await;
        window.close();
        until(|| {
            *context.subscribe_phase().borrow()
                == dzban_core::application::ApplicationPhase::Running
        })
        .await;
        assert!(window.is_visible());
        assert_eq!(context.session_ids().await.len(), 1);
        window.close();
        until(|| !window.is_visible()).await;
        assert!(context.session_ids().await.is_empty());
        assert_eq!(
            *context.subscribe_phase().borrow(),
            dzban_core::application::ApplicationPhase::Closed
        );
    });
}

#[glib::async_test]
async fn failed_disconnect_completion_restores_the_open_selection() {
    let (connection, context, _) = fixture(Stage::DisconnectFailure);
    connection.connect();
    until(|| connection.state() == ConnectionState::Connected).await;
    let original = context.session_ids().await;
    let completion = connection.disconnect_with_completion().unwrap();
    assert_eq!(
        completion.await.unwrap().unwrap_err(),
        ExecutionError::Message("Fixture disconnect failed".into())
    );
    assert_eq!(connection.state(), ConnectionState::Connected);
    assert_eq!(connection.version().as_deref(), Some("Fixture1 1.0"));
    connection.connect();
    assert_eq!(context.session_ids().await, original);
    assert_eq!(connection.state(), ConnectionState::Connected);
    connection
        .disconnect_with_completion()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(context.session_ids().await.is_empty());
}

#[glib::async_test]
async fn disconnect_completion_drains_core_and_gtk_before_context_teardown() {
    for _ in 0..20 {
        let (connection, context, gate) = fixture(Stage::Connect);
        gate.release.notify_one();
        connection.connect();
        until(|| connection.state() == ConnectionState::Connected).await;
        let completion = connection.disconnect_with_completion().unwrap();
        assert_eq!(connection.state(), ConnectionState::Disconnected);
        completion.await.unwrap().unwrap();
        assert!(gate.closed.load(Ordering::SeqCst));
        assert!(context.session_ids().await.is_empty());
        assert_eq!(connection.state(), ConnectionState::Disconnected);
        connection
            .disconnect_with_completion()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn connecting_an_open_selection_keeps_the_existing_session() {
    glib::MainContext::ref_thread_default().block_on(async {
        let (connection, context, gate) = fixture(Stage::Connect);
        connection.connect();
        until(|| gate.entered.load(Ordering::SeqCst)).await;
        gate.release.notify_one();
        until(|| connection.state() == ConnectionState::Connected).await;
        let original = context.session_ids().await;
        assert_eq!(original.len(), 1);
        connection.connect();
        assert_eq!(connection.state(), ConnectionState::Connected);
        assert_eq!(connection.version().as_deref(), Some("Fixture1 1.0"));
        assert_eq!(context.session_ids().await, original);
        connection.disconnect();
        until_session_count(&context, 0).await;
    });
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn disconnect_from_connecting_signal_clears_the_admitted_request() {
    glib::MainContext::ref_thread_default().block_on(async {
        let (connection, context, gate) = fixture(Stage::Connect);
        connection.connect_closure(
            "state-changed",
            false,
            glib::closure_local!(move |connection: DbConnection,
                                       _old: ConnectionState,
                                       new: ConnectionState| {
                if new == ConnectionState::Connecting {
                    connection.disconnect();
                }
            }),
        );
        connection.connect();
        assert_eq!(connection.state(), ConnectionState::Disconnected);
        until(|| gate.entered.load(Ordering::SeqCst)).await;
        gate.release.notify_one();
        until(|| gate.closed.load(Ordering::SeqCst)).await;
        until_session_count(&context, 0).await;
        assert_eq!(connection.state(), ConnectionState::Disconnected);
        assert_eq!(connection.version(), None);
    });
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn disconnect_during_connect_or_version_lookup_rejects_late_completion() {
    glib::MainContext::ref_thread_default().block_on(async {
        for stage in [Stage::Connect, Stage::Version] {
            let (connection, context, gate) = fixture(stage);
            connection.connect();
            until(|| gate.entered.load(Ordering::SeqCst)).await;
            assert_eq!(connection.state(), ConnectionState::Connecting);
            connection.disconnect();
            gate.release.notify_one();
            until(|| gate.closed.load(Ordering::SeqCst)).await;
            until_session_count(&context, 0).await;
            assert_eq!(connection.state(), ConnectionState::Disconnected);
            assert_eq!(connection.version(), None);
            assert!(context.session_ids().await.is_empty());
        }
    });
}

#[gtk4::test]
#[ignore = "requires a graphical display"]
fn replacement_request_keeps_its_session_and_version_after_old_completion() {
    glib::MainContext::ref_thread_default().block_on(async {
        for stage in [Stage::Connect, Stage::Version] {
            let (connection, context, gate) = fixture(stage);
            connection.connect();
            until(|| gate.entered.load(Ordering::SeqCst)).await;
            connection.connect();
            until(|| connection.state() == ConnectionState::Connected).await;
            assert_eq!(connection.version().as_deref(), Some("Fixture2 1.0"));
            gate.release.notify_one();
            until(|| gate.closed.load(Ordering::SeqCst)).await;
            until_session_count(&context, 1).await;
            assert_eq!(connection.state(), ConnectionState::Connected);
            assert_eq!(connection.version().as_deref(), Some("Fixture2 1.0"));
            assert_eq!(context.session_ids().await.len(), 1);
            connection.disconnect();
            until_session_count(&context, 0).await;
        }
    });
}
