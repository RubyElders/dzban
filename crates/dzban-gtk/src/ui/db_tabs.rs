use adw::prelude::{AdwDialogExt, AlertDialogExt};
use adw::{TabBar, TabPage, TabView};
use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;
#[cfg(test)]
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
#[cfg(test)]
use std::time::Duration;

use super::connection_state::ConnectionStateWidget;
use super::connections::{self, ConnectionManager};
use super::plan_inspector::{PlanScroll, PlanState};
use super::query_tabs::QueryTabManager;
use super::statusbar::Statusbar;
use dzban_gtk::database::{
    AppCommand, AppEvent, AppState, Config, ConnectionState, DbConfig, DbManager, InputRole,
    PlanPreviewState, RuntimeBridge,
};

type DatabaseTab = (TabPage, QueryTabManager, DbConfig, ConnectionStateWidget);
type DatabaseTabs = Rc<RefCell<HashMap<String, DatabaseTab>>>;
type AppStateCallback = Rc<dyn Fn(AppState)>;

#[cfg(test)]
mod tests {
    use super::super::query_history::QueryResultSnapshot;
    use super::*;
    use dzban_core::history::{CompletedQuery, PendingQueryHistory, SuccessfulQuery};

    #[test]
    fn history_preserves_planning_diagnostics_and_records_once() {
        let diagnostic =
            dzban_core::error::ExecutionError::Database(dzban_core::error::DatabaseError {
                backend: dzban_core::DatabaseBackend::Postgresql,
                kind: dzban_core::error::ErrorKind::Server,
                message: "EXPLAIN unsupported".into(),
                code: Some(dzban_core::error::NativeErrorCode::SqlState("0A000".into())),
                detail: Some("Planning rejected".into()),
                hint: Some("Run without preview".into()),
            });
        let mut pending = PendingQueryHistory::new("SELECT 1".into());
        pending.complete(CompletedQuery::Success(SuccessfulQuery {
            command: None,
            duration: Duration::from_millis(1),
            rows: 1,
            result: QueryResultSnapshot::default(),
        }));
        assert!(pending.take_entry().is_none());
        pending.planned(Err(diagnostic.clone()));
        let entry = pending.take_entry().unwrap();
        assert_eq!(entry.planning_error, Some(diagnostic));
        assert!(matches!(
            entry.outcome,
            dzban_core::history::QueryHistoryOutcome::Success {
                explain_error: Some(_),
                ..
            }
        ));
        assert!(pending.take_entry().is_none());
    }

    fn plan_widget(widget: &gtk4::Widget) -> Option<gtk4::Widget> {
        if widget.widget_name() == "query-plan-inspector" {
            return Some(widget.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(found) = plan_widget(&current) {
                return Some(found);
            }
            child = current.next_sibling();
        }
        None
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn preview_adapter_hides_holds_and_pins_taps() {
        let manager = DatabaseTabManager::new(super::super::statusbar::create_statusbar());
        let queries = QueryTabManager::new("preview-adapter-test");
        queries.new_query_tab();
        let widget = plan_widget(queries.get_widget().upcast_ref()).unwrap();
        manager.dispatch(AppEvent::QueryAvailabilityChanged(true));
        manager.handle_input_pressed(InputRole::PrimaryModifier, 0);
        manager.handle_input_pressed(InputRole::PreviewModifier, 1);
        *manager.query_preview_manager.borrow_mut() = Some(queries.clone());
        manager.dispatch(AppEvent::PreviewReady(dzban_gtk::database::QueryPlan {
            backend: dzban_gtk::database::DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate: dzban_gtk::database::QueryEstimate::default(),
        }));
        assert!(manager.handle_input_pressed(InputRole::Inspect, 10));
        assert!(widget.is_visible());
        assert!(manager.handle_input_pressed(InputRole::Inspect, 500));
        manager.handle_input_released(InputRole::Inspect, 600);
        assert!(!widget.is_visible());

        *manager.query_preview_manager.borrow_mut() = Some(queries);
        manager.dispatch(AppEvent::PreviewReady(dzban_gtk::database::QueryPlan {
            backend: dzban_gtk::database::DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate: dzban_gtk::database::QueryEstimate::default(),
        }));
        assert!(manager.handle_input_pressed(InputRole::Inspect, 700));
        manager.handle_input_released(InputRole::Inspect, 750);
        manager.handle_input_released(InputRole::PreviewModifier, 760);
        manager.handle_input_released(InputRole::PrimaryModifier, 770);
        assert!(widget.is_visible());
        manager.dispatch(AppEvent::QueryPlanning);
        assert!(!widget.is_visible());
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn repeated_execution_snapshots_do_not_restart_action_bar_rendering() {
        let manager = DatabaseTabManager::new(super::super::statusbar::create_statusbar());
        let count = Rc::new(Cell::new(0));
        let observed = count.clone();
        manager.set_app_state_callback(move |_| observed.set(observed.get() + 1));
        assert_eq!(count.get(), 1);
        manager.dispatch(AppEvent::QueryRunning(None));
        assert_eq!(count.get(), 2);
        manager.dispatch(AppEvent::QueryRunning(None));
        assert_eq!(count.get(), 2);
        let second_count = Rc::new(Cell::new(0));
        let observed = second_count.clone();
        manager.set_app_state_callback(move |_| observed.set(observed.get() + 1));
        assert_eq!(second_count.get(), 1);
        assert_eq!(count.get(), 2);
        manager.dispatch(AppEvent::QueryRunning(None));
        assert_eq!(second_count.get(), 1);
        manager.dispatch(AppEvent::QueryCanceling);
        assert_eq!(count.get(), 3);
        assert_eq!(second_count.get(), 2);
        manager.dispatch(AppEvent::QueryFinished);
        assert_eq!(count.get(), 4);
        assert_eq!(second_count.get(), 3);
    }

    fn disconnected_workspace(manager: &DatabaseTabManager) -> (TabPage, QueryTabManager) {
        let name = "workspace-close-fixture";
        assert!(manager.db_manager.add_config(
            name.into(),
            "host=localhost user=fixture dbname=fixture".into()
        ));
        let config = manager.db_manager.get_config(name).unwrap();
        let (page, queries, state) = manager.create_database_tab(name);
        manager
            .tabs
            .borrow_mut()
            .insert(name.into(), (page.clone(), queries.clone(), config, state));
        (page, queries)
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn last_query_waits_for_workspace_close_and_chooser_clears_the_stop_action() {
        use dzban_core::interaction::{PrimaryAction, QueryExecutionState, SessionInteraction};
        let manager = DatabaseTabManager::new(super::super::statusbar::create_statusbar());
        let (page, queries) = disconnected_workspace(&manager);
        let weak_page = page.downgrade();
        manager.dispatch(AppEvent::SessionSelected(Some(SessionInteraction {
            scope: dzban_core::execution::QueryScope {
                session: dzban_core::execution::SessionId(
                    dzban_core::documents::QueryDocuments::default()
                        .open()
                        .id()
                        .0,
                ),
                query: None,
            },
            execution: QueryExecutionState::Running { plan: None },
            query_available: false,
        })));
        assert_eq!(
            manager.app_state.borrow().primary_action(),
            PrimaryAction::Stop
        );
        manager.open_connection_manager();
        assert_eq!(
            manager.app_state.borrow().primary_action(),
            PrimaryAction::Unavailable
        );
        manager.tab_view.set_selected_page(&page);
        assert!(manager.close_current_query_tab());
        assert_eq!(queries.tab_count(), 1);
        assert_eq!(manager.open_database_tabs().len(), 1);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !manager.open_database_tabs().is_empty() && std::time::Instant::now() < deadline {
            glib::MainContext::default().iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(manager.open_database_tabs().is_empty());
        assert_eq!(manager.db_manager.config_count(), 0);
        assert_eq!(manager.tab_view.n_pages(), 1);
        drop(page);
        assert!(weak_page.upgrade().is_none());
        assert_eq!(
            manager.app_state.borrow().primary_action(),
            PrimaryAction::Unavailable
        );
    }
}

fn create_colored_dot_icon(css_class: &str) -> gtk4::gio::Icon {
    let label = gtk4::Label::new(Some("●"));
    label.add_css_class(css_class);
    let color = label.color();

    // Create a 16x16 surface with a colored circle
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 16, 16)
        .expect("Failed to create surface");
    {
        let cr = cairo::Context::new(&surface).expect("Failed to create context");

        // Set transparent background
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        cr.paint().expect("Failed to paint background");

        // Draw colored circle using the color from CSS
        cr.set_source_rgba(
            color.red() as f64,
            color.green() as f64,
            color.blue() as f64,
            color.alpha() as f64,
        );
        cr.arc(8.0, 8.0, 4.0, 0.0, 2.0 * std::f64::consts::PI);
        cr.fill().expect("Failed to fill circle");
    } // Drop cr to release borrow

    // Get stride before borrowing data mutably
    let stride = surface.stride() as usize;

    // Get surface data and create texture from memory
    let data = surface.data().expect("Failed to get surface data");
    let bytes = gtk4::glib::Bytes::from(&data[..]);
    let texture = gdk::MemoryTexture::new(16, 16, gdk::MemoryFormat::B8g8r8a8, &bytes, stride);
    texture.upcast()
}

#[derive(Clone)]
pub struct DatabaseTabManager {
    tab_view: TabView,
    tab_bar: TabBar,
    container: gtk4::Box,
    tabs: DatabaseTabs,
    connections_page: Rc<RefCell<Option<TabPage>>>,
    connection_manager: Rc<RefCell<Option<ConnectionManager>>>,
    runtime_bridge: RuntimeBridge,
    db_manager: DbManager,
    statusbar: Statusbar,
    app_state: Rc<RefCell<AppState>>,
    published_app_state: Rc<RefCell<Option<AppState>>>,
    app_state_callbacks: Rc<RefCell<Vec<AppStateCallback>>>,
    plan_preview: dzban_core::preview::PlanPreviewService,
    query_preview_manager: Rc<RefCell<Option<QueryTabManager>>>,
}

impl DatabaseTabManager {
    pub fn runtime_bridge(&self) -> RuntimeBridge {
        self.runtime_bridge.clone()
    }

    pub fn new(statusbar: Statusbar) -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.set_vexpand(true);

        // Create TabView for modern Adwaita tabs with animations
        let tab_view = TabView::new();
        tab_view.set_vexpand(true);

        tab_view.set_shortcuts(adw::TabViewShortcuts::empty());

        // Create TabBar for the tab strip
        let tab_bar = TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);
        tab_bar.set_expand_tabs(false);
        tab_bar.set_focusable(true);
        tab_bar.update_property(&[gtk4::accessible::Property::Label("Query tabs")]);

        container.append(&tab_bar);
        container.append(&tab_view);

        // Create runtime bridge for Tokio <-> GLib communication
        let runtime_bridge = RuntimeBridge::new().expect("Failed to create RuntimeBridge");

        let manager = Self {
            tab_view: tab_view.clone(),
            tab_bar,
            container,
            tabs: Rc::new(RefCell::new(HashMap::new())),
            connections_page: Rc::new(RefCell::new(None)),
            connection_manager: Rc::new(RefCell::new(None)),
            runtime_bridge: runtime_bridge.clone(),
            db_manager: DbManager::with_runtime_bridge(runtime_bridge),
            statusbar: statusbar.clone(),
            app_state: Rc::new(RefCell::new(AppState::default())),
            published_app_state: Rc::new(RefCell::new(None)),
            app_state_callbacks: Rc::new(RefCell::new(Vec::new())),
            plan_preview: dzban_core::preview::PlanPreviewService::default(),
            query_preview_manager: Rc::new(RefCell::new(None)),
        };

        // Create connection manager as first (leftmost) pinned tab
        manager.create_connections_tab();

        // Handle tab close - if all DB tabs are closed, show connection manager
        let manager_clone = manager.clone();
        tab_view.connect_close_page(move |view, page| {
            let db_name_to_remove = {
                let tabs = manager_clone.tabs.borrow();
                tabs.iter()
                    .find(|(_, (p, _, _, _))| p == page)
                    .map(|(name, _)| name.clone())
            };

            if let Some(name) = db_name_to_remove {
                if manager_clone.tab_view.selected_page().as_ref() == Some(page) {
                    manager_clone.cancel_query_preview();
                }
                let retirement =
                    manager_clone
                        .tabs
                        .borrow()
                        .get(&name)
                        .map(|(_, queries, config, _)| {
                            (queries.clone(), config.connection().session_engine())
                        });
                let completion = match manager_clone.db_manager.request_remove_config(&name) {
                    Ok(completion) => completion,
                    Err(error) => {
                        view.close_page_finish(page, false);
                        manager_clone.report_close_error(&error.to_string());
                        return glib::Propagation::Stop;
                    }
                };
                let manager = manager_clone.clone();
                let page = page.clone();
                glib::spawn_future_local(async move {
                    let result = completion.await.unwrap_or_else(|_| {
                        Err(dzban_core::error::ExecutionError::Message(
                            "Connection close delivery was dropped".into(),
                        ))
                    });
                    match result {
                        Ok(true) => {
                            if let Some((queries, engine)) = retirement {
                                queries.retire_closed_session(engine.as_deref());
                            }
                            manager.tabs.borrow_mut().remove(&name);
                            manager.tab_view.close_page_finish(&page, true);
                            // Check if any database tabs remain
                            if manager.tabs.borrow().is_empty() {
                                // Select connection manager
                                manager.open_connection_manager();
                            }
                        }
                        Ok(false) => manager.tab_view.close_page_finish(&page, false),
                        Err(error) => {
                            manager.tab_view.close_page_finish(&page, false);
                            manager.report_close_error(&error.to_string());
                        }
                    }
                });
                return glib::Propagation::Stop;
            }

            gtk4::glib::Propagation::Proceed
        });

        // Handle tab selection changes to update statusbar
        let manager_clone = manager.clone();
        tab_view.connect_selected_page_notify(move |view| {
            manager_clone.cancel_query_preview();
            if let Some(page) = view.selected_page() {
                // First, find if this is a database tab
                let connection = {
                    let tabs = manager_clone.tabs.borrow();
                    tabs.iter()
                        .find(|(_, (tab_page, _, _, _))| tab_page == &page)
                        .map(|(name, (_, _, config, _))| (name.clone(), config.connection()))
                };

                if let Some((_name, connection)) = connection {
                    manager_clone
                        .statusbar
                        .bind_to_connection(&connection.profile_name(), &connection);
                    manager_clone
                        .dispatch(AppEvent::SessionSelected(connection.session_interaction()));
                    return;
                }

                // If it's the connections page, unbind statusbar
                let is_connections_page = {
                    manager_clone
                        .connections_page
                        .borrow()
                        .as_ref()
                        .map(|conn_page| conn_page == &page)
                        .unwrap_or(false)
                };

                if is_connections_page {
                    manager_clone.statusbar.unbind();
                    manager_clone.dispatch(AppEvent::SessionSelected(None));
                }
            }
        });

        manager
    }

    pub fn get_widget(&self) -> &gtk4::Box {
        &self.container
    }

    fn report_close_error(&self, error: &str) {
        tracing::error!("Could not close connection: {error}");
        if self.container.root().is_some() {
            let dialog = adw::AlertDialog::builder()
                .heading("Could not close connection")
                .body(error)
                .build();
            dialog.add_response("close", "Close");
            dialog.present(Some(&self.container));
        }
    }

    pub fn set_app_state_callback<F>(&self, callback: F)
    where
        F: Fn(AppState) + 'static,
    {
        let state = self.app_state.borrow().clone();
        let callback: AppStateCallback = Rc::new(callback);
        self.app_state_callbacks.borrow_mut().push(callback.clone());
        self.published_app_state.replace(Some(state.clone()));
        callback(state);
    }

    pub fn begin_query_preview(&self) {
        let Some(current_page) = self.tab_view.selected_page() else {
            return;
        };
        let target = self
            .tabs
            .borrow()
            .values()
            .find(|(page, _, _, _)| page == &current_page)
            .map(|(_, queries, config, _)| (queries.clone(), config.connection()));
        let Some((queries, connection)) = target else {
            self.dispatch(AppEvent::PreviewFailed("No query to explain".into()));
            return;
        };
        let Some((_, sql, document)) = queries.current_query() else {
            self.dispatch(AppEvent::PreviewFailed("No query to explain".into()));
            return;
        };
        if self.query_preview_manager.borrow().is_some() {
            return;
        }
        let mut snapshots = self.plan_preview.subscribe();
        let target =
            match connection.schedule_preview(self.plan_preview.clone(), document.clone(), sql) {
                Ok(target) => target,
                Err(error) => {
                    self.dispatch(AppEvent::PreviewFailed(error));
                    return;
                }
            };
        queries.highlight_current_query();
        *self.query_preview_manager.borrow_mut() = Some(queries);
        let manager = self.clone();
        glib::spawn_future_local(async move {
            loop {
                let snapshot = snapshots.borrow_and_update().clone();
                if snapshot.target != Some(target) {
                    return;
                }
                if snapshot.phase == dzban_core::preview::PreviewPhase::Empty {
                    manager.clear_preview_view();
                    return;
                }
                if manager.plan_preview.accepts(&snapshot, &document) {
                    let event = match snapshot.phase {
                        dzban_core::preview::PreviewPhase::Scheduled => None,
                        dzban_core::preview::PreviewPhase::Loading => {
                            Some(AppEvent::PreviewLoading)
                        }
                        dzban_core::preview::PreviewPhase::Ready(plan) => {
                            Some(AppEvent::PreviewReady(plan))
                        }
                        dzban_core::preview::PreviewPhase::Failed(error) => {
                            Some(AppEvent::PreviewFailed(error.to_string()))
                        }
                        dzban_core::preview::PreviewPhase::Empty => None,
                    };
                    if let Some(event) = event {
                        manager.dispatch(event);
                    }
                }
                if snapshots.changed().await.is_err() {
                    return;
                }
            }
        });
    }

    fn clear_preview_view(&self) {
        if let Some(queries) = self.query_preview_manager.borrow_mut().take() {
            queries.clear_query_highlight();
            queries.close_plan();
        }
        self.dispatch(AppEvent::PreviewCancelled);
    }

    pub fn cancel_query_preview(&self) {
        self.plan_preview.cancel();
        self.clear_preview_view();
    }

    fn dispatch(&self, event: AppEvent) {
        if let AppEvent::SessionSelected(session) = &event {
            self.runtime_bridge
                .select_session_tab(session.as_ref().map(|session| session.scope.session));
        }
        let update_plan = matches!(
            event,
            AppEvent::PreviewLoading | AppEvent::PreviewReady(_) | AppEvent::PreviewFailed(_)
        );
        let commands = self.app_state.borrow_mut().transition(event);
        let preview = self.app_state.borrow().preview.clone();
        if update_plan && let Some(query_manager) = self.query_preview_manager.borrow().as_ref() {
            match &preview {
                PlanPreviewState::Scheduled | PlanPreviewState::Loading => {
                    query_manager.set_plan_preview(PlanState::Loading)
                }
                PlanPreviewState::Ready(plan) => {
                    query_manager.set_plan_preview(PlanState::Ready(
                        serde_json::to_value(plan).unwrap_or_default(),
                    ));
                }
                PlanPreviewState::Error(error) => {
                    query_manager.set_plan_preview(PlanState::Error(error.clone()));
                }
                PlanPreviewState::Empty => {}
            }
        }
        self.emit_app_state();
        for command in commands {
            self.execute_app_command(command);
        }
    }

    fn emit_app_state(&self) {
        let state = self.app_state.borrow().clone();
        if self.published_app_state.borrow().as_ref() == Some(&state) {
            return;
        }
        self.published_app_state.replace(Some(state.clone()));
        for callback in self.app_state_callbacks.borrow().iter() {
            callback(state.clone());
        }
    }

    fn execute_app_command(&self, command: AppCommand) {
        match command {
            AppCommand::BeginPreview => self.begin_query_preview(),
            AppCommand::CancelPreview => self.cancel_query_preview(),
            AppCommand::ShowPlan => {
                self.show_query_plan();
            }
            AppCommand::HidePlan => {
                if let Some(manager) = self.query_preview_manager.borrow().as_ref() {
                    manager.close_plan();
                }
            }
            AppCommand::PinPlan => {
                self.pin_query_plan();
            }
            AppCommand::RunQuery => self.run_current_query(),
            AppCommand::CancelQuery => self.cancel_current_query(),
        }
    }

    pub fn handle_input_pressed(&self, key: InputRole, at_ms: u64) -> bool {
        let commands = self
            .app_state
            .borrow_mut()
            .transition(AppEvent::KeyPressed { key, at_ms });
        let handled = self.app_state.borrow().captures_key(key);
        self.emit_app_state();
        for command in commands {
            self.execute_app_command(command);
        }
        handled
    }

    pub fn handle_input_released(&self, key: InputRole, at_ms: u64) {
        let commands = self
            .app_state
            .borrow_mut()
            .transition(AppEvent::KeyReleased { key, at_ms });
        self.emit_app_state();
        for command in commands {
            self.execute_app_command(command);
        }
    }

    pub fn reset_input(&self) {
        self.dispatch(AppEvent::KeyboardReset);
    }

    pub fn activate_primary_action(&self) {
        self.dispatch(AppEvent::PrimaryActionRequested);
    }

    fn current_query_manager(&self) -> Option<QueryTabManager> {
        let current_page = self.tab_view.selected_page()?;
        self.tabs
            .borrow()
            .values()
            .find(|(page, _, _, _)| page == &current_page)
            .map(|(_, manager, _, _)| manager.clone())
    }

    pub fn show_query_plan(&self) -> bool {
        let Some(manager) = self.query_preview_manager.borrow().as_ref().cloned() else {
            return false;
        };
        manager.show_plan_transient();
        true
    }

    pub fn pin_query_plan(&self) -> bool {
        self.query_preview_manager
            .borrow()
            .as_ref()
            .is_some_and(QueryTabManager::pin_plan)
    }

    pub fn close_query_plan(&self) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.close_plan())
    }

    pub fn scroll_query_plan(&self, direction: PlanScroll) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.scroll_plan(direction))
    }

    pub fn close_current_query_tab(&self) -> bool {
        let Some((page, _)) = self.current_database_tab() else {
            return false;
        };
        self.cancel_query_preview();
        self.tab_view.close_page(&page);
        true
    }

    fn current_database_tab(&self) -> Option<(TabPage, QueryTabManager)> {
        let current_page = self.tab_view.selected_page()?;
        self.tabs
            .borrow()
            .values()
            .find(|(page, _, _, _)| page == &current_page)
            .map(|(page, manager, _, _)| (page.clone(), manager.clone()))
    }

    pub fn select_next_database_tab(&self) -> bool {
        if self.tab_view.select_next_page() {
            true
        } else if self.tab_view.n_pages() > 1 {
            self.tab_view.set_selected_page(&self.tab_view.nth_page(0));
            true
        } else {
            false
        }
    }

    pub fn focus_database_tabs(&self) -> bool {
        self.tab_bar.grab_focus()
    }

    pub fn focus_query_tabs(&self) -> bool {
        self.focus_database_tabs()
    }

    pub fn focus_query_editor(&self) -> bool {
        let Some(manager) = self.current_query_manager() else {
            return false;
        };
        manager.focus_current_editor();
        true
    }

    pub fn focus_query_results(&self) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.focus_current_results())
    }

    pub fn focus_query_history(&self) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.focus_history())
    }

    pub fn show_focused_result(&self) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.show_focused_result())
    }

    pub fn pin_result_preview(&self) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.pin_result())
    }

    pub fn close_result_preview(&self) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.close_result())
    }

    pub fn select_previous_database_tab(&self) -> bool {
        if self.tab_view.select_previous_page() {
            true
        } else if self.tab_view.n_pages() > 1 {
            self.tab_view
                .set_selected_page(&self.tab_view.nth_page(self.tab_view.n_pages() - 1));
            true
        } else {
            false
        }
    }

    pub fn select_next_query_tab(&self) -> bool {
        self.select_next_database_tab()
    }

    pub fn select_previous_query_tab(&self) -> bool {
        self.select_previous_database_tab()
    }

    pub fn rename_current_query_tab(&self, title: &str) -> bool {
        let Some((page, queries)) = self.current_database_tab() else {
            return false;
        };
        if !queries.rename_current_tab(title) {
            return false;
        }
        let profile = self
            .tabs
            .borrow()
            .values()
            .find(|(tab, _, _, _)| tab == &page)
            .and_then(|(_, _, config, _)| config.connection().session_engine())
            .and_then(|engine| self.runtime_bridge.query_tab(engine.id()))
            .and_then(|tab| tab.profile.map(|profile| profile.0));
        if let Some(profile) = profile {
            page.set_title(&format!("{profile} - {title}"));
        }
        true
    }

    pub fn current_query_tab_title(&self) -> Option<String> {
        self.current_query_manager()?.current_tab_title()
    }

    pub fn open_database_tabs(&self) -> Vec<(String, String)> {
        let mut tabs = self
            .tabs
            .borrow()
            .iter()
            .map(|(name, (page, _, _, _))| {
                (
                    self.tab_view.page_position(page),
                    name.clone(),
                    page.title().to_string(),
                )
            })
            .collect::<Vec<_>>();
        tabs.sort_by_key(|(position, _, _)| *position);
        tabs.into_iter().map(|(_, id, title)| (id, title)).collect()
    }

    pub fn select_database_tab(&self, name: &str) -> bool {
        let page = self
            .tabs
            .borrow()
            .get(name)
            .map(|(page, _, _, _)| page.clone());
        page.is_some_and(|page| {
            self.tab_view.set_selected_page(&page);
            true
        })
    }

    pub fn open_database(&self, db_name: &str) {
        // Find database config
        let config = Config::load().ok();
        let db_config =
            config.and_then(|c| c.databases.iter().find(|db| db.name == db_name).cloned());

        if let Some(db_config) = db_config {
            let profile_name = db_name.to_string();
            let tab_name = gtk4::glib::uuid_string_random().to_string();
            let db_name = tab_name.as_str();
            if !self
                .db_manager
                .add_query_tab_config(tab_name.clone(), db_config)
            {
                return;
            }
            let config = self
                .db_manager
                .get_config(db_name)
                .expect("newly added database config is available");
            let connection = config.connection();
            let (page, query_manager, state_widget) =
                self.create_database_tab_for_profile(db_name, &profile_name);
            let core_bridge = self.runtime_bridge.clone();
            let core_queries = query_manager.clone();
            let core_page = page.clone();
            let label_profile = profile_name.clone();
            connection.connect_notify_local(Some("state"), move |connection, _| {
                if connection.state() == ConnectionState::Connected
                    && let Some(tab) = connection
                        .session_engine()
                        .and_then(|engine| core_bridge.query_tab(engine.id()))
                {
                    core_page.set_title(&format!(
                        "{} - {}",
                        label_profile,
                        tab.document.snapshot().title
                    ));
                    core_queries.use_session_tab(tab.document);
                }
            });

            self.tabs.borrow_mut().insert(
                db_name.to_string(),
                (
                    page.clone(),
                    query_manager.clone(),
                    config,
                    state_widget.clone(),
                ),
            );

            if self.tab_view.selected_page().as_ref() == Some(&page) {
                self.statusbar
                    .bind_to_connection(&profile_name, &connection);
                self.dispatch(AppEvent::SessionSelected(connection.session_interaction()));
            }

            let page_for_state = page.clone();
            let state_widget_for_state = state_widget.clone();
            let db_name_for_state = profile_name.clone();
            connection.connect_notify_local(Some("state"), move |connection, _| {
                let state = connection.state();
                let (_css_class, label) = match state {
                    ConnectionState::Disconnected => ("dim-label", "Disconnected"),
                    ConnectionState::Connecting => ("warning", "Connecting"),
                    ConnectionState::Connected => ("success", "Connected"),
                    ConnectionState::Error => ("error", "Error"),
                };
                page_for_state.set_icon(Some(&gtk4::gio::ThemedIcon::new(
                    super::icons::database_backend(connection.backend()),
                )));
                page_for_state.set_loading(
                    state == ConnectionState::Connecting || connection.query_running(),
                );
                state_widget_for_state
                    .update_state(state, Some(&format!("{} - {}", db_name_for_state, label)));
                if state == ConnectionState::Connected {
                    connection.ping();
                }
            });

            let manager_for_availability = self.clone();
            let page_for_availability = page.clone();
            connection.connect_notify_local(Some("operation-available"), move |connection, _| {
                if manager_for_availability.tab_view.selected_page().as_ref()
                    == Some(&page_for_availability)
                {
                    manager_for_availability
                        .dispatch(AppEvent::SessionSelected(connection.session_interaction()));
                }
            });

            let manager_for_operation = self.clone();
            let page_for_operation = page.clone();
            connection.connect_notify_local(Some("operation"), move |connection, _| {
                if manager_for_operation.tab_view.selected_page().as_ref()
                    != Some(&page_for_operation)
                {
                    return;
                }
                page_for_operation.set_loading(connection.query_running());
                manager_for_operation
                    .dispatch(AppEvent::SessionSelected(connection.session_interaction()));
            });

            let weak_connection = connection.downgrade();
            gtk4::glib::timeout_add_seconds_local(5, move || {
                let Some(connection) = weak_connection.upgrade() else {
                    return gtk4::glib::ControlFlow::Break;
                };
                if connection.state() == ConnectionState::Disconnected {
                    return gtk4::glib::ControlFlow::Break;
                }
                if connection.state() == ConnectionState::Connected {
                    connection.ping();
                }
                gtk4::glib::ControlFlow::Continue
            });

            connection.connect();
        }
    }

    #[cfg(test)]
    fn create_database_tab(
        &self,
        db_name: &str,
    ) -> (TabPage, QueryTabManager, ConnectionStateWidget) {
        self.create_database_tab_for_profile(db_name, db_name)
    }

    fn create_database_tab_for_profile(
        &self,
        db_name: &str,
        profile_name: &str,
    ) -> (TabPage, QueryTabManager, ConnectionStateWidget) {
        // Create query tab manager for this database
        let query_manager = QueryTabManager::new(profile_name);
        query_manager.bind_preview_service(self.plan_preview.clone());
        let content = query_manager.get_widget().clone();

        // Create connection state widget for tracking state (not displayed in tab)
        let state_widget = ConnectionStateWidget::new();
        state_widget.update_state(
            ConnectionState::Connecting,
            Some(&format!("{} - Connecting...", db_name)),
        );

        // Add page to TabView
        let page = self.tab_view.append(&content);

        page.set_title(&format!("{profile_name} - Connecting"));

        // Set orange/yellow dot icon for connecting state
        let icon = create_colored_dot_icon("warning");
        page.set_icon(Some(&icon));

        // Make tab closeable with animation
        self.tab_view.set_page_pinned(&page, false);

        let weak_tab_view = self.tab_view.downgrade();
        let database_page = page.downgrade();
        query_manager.set_workspace_close_callback(move || {
            if let (Some(tab_view), Some(database_page)) =
                (weak_tab_view.upgrade(), database_page.upgrade())
            {
                tab_view.close_page(&database_page);
            }
        });

        // Open first query tab
        query_manager.new_query_tab();
        query_manager.set_single_tab();

        // Select the newly created tab
        self.tab_view.set_selected_page(&page);

        (page, query_manager, state_widget)
    }

    pub fn toggle_current_sidebar(&self) {
        if let Some(current_page) = self.tab_view.selected_page() {
            let tabs = self.tabs.borrow();
            // Find the QueryTabManager for the current page
            for (page, query_manager, _, _) in tabs.values() {
                if page == &current_page {
                    query_manager.toggle_sidebar();
                    break;
                }
            }
        }
    }

    pub fn is_current_sidebar_visible(&self) -> bool {
        if let Some(current_page) = self.tab_view.selected_page() {
            let tabs = self.tabs.borrow();
            for (page, query_manager, _, _) in tabs.values() {
                if page == &current_page {
                    return query_manager.is_sidebar_visible();
                }
            }
        }
        false
    }

    /// Execute the query in the currently active query tab
    pub fn run_current_query(&self) {
        self.close_query_plan();
        self.cancel_query_preview();
        let Some(current_page) = self.tab_view.selected_page() else {
            return;
        };
        let Some((database_page, query_manager, connection)) = self
            .tabs
            .borrow()
            .values()
            .find(|(page, _, _, _)| page == &current_page)
            .map(|(page, manager, config, _)| (page.clone(), manager.clone(), config.connection()))
        else {
            return;
        };
        let Some((query_num, sql, document)) = query_manager.current_query() else {
            tracing::debug!("No executable query in the selected tab");
            return;
        };
        let mut snapshots = document.subscribe();
        let Ok(delivery) = connection.execute_document_query(document.clone(), sql) else {
            return;
        };
        let query_rx = delivery.result;
        let engine_state = delivery.state;
        let query_id = delivery.id;
        let output_generation = delivery
            .generation
            .expect("document query has an output identity");
        let bridge = delivery.bridge;
        if !query_manager.show_loading_generation(query_num, output_generation) {
            return;
        }

        let statusbar = self.statusbar.clone();
        let tab_view = self.tab_view.clone();
        let manager_for_plan = self.clone();
        let database_page_for_plan = database_page.clone();
        let query_manager_for_result = query_manager.clone();
        let bridge_for_result = bridge.clone();
        gtk4::glib::spawn_future_local(async move {
            let result = query_rx.await;
            query_manager_for_result.refresh_history();
            if !document.accepts_output(output_generation) {
                return;
            }
            match result {
                Ok(Ok(result)) => {
                    let row_count = result.capture.rows;
                    let duration = result.duration;
                    tracing::info!("Query returned {} rows", row_count);
                    let command = result.command;
                    if let Some(command) = &command {
                        query_manager_for_result.display_command_for(query_num, command);
                    } else if row_count == 0 {
                        query_manager_for_result.display_results_for(
                            query_num,
                            dzban_gtk::database::ResultSet {
                                columns: vec![],
                                rows: vec![],
                            },
                        );
                    } else {
                        query_manager_for_result.display_stream_for(
                            query_num,
                            output_generation,
                            result.capture,
                            bridge_for_result,
                        );
                    };
                    if tab_view.selected_page().as_ref() == Some(&database_page)
                        && query_manager_for_result.is_query_selected(query_num)
                    {
                        statusbar.update_rows(command.is_none().then_some(row_count));
                        statusbar.update_exec_time(Some(duration.as_secs_f64()));
                    }
                }
                Ok(Err(error)) => {
                    tracing::error!("Query failed: {}", error);
                    query_manager_for_result.display_error_for(query_num, &error.to_string());
                }
                Err(_) => {
                    let error = "Query execution was cancelled";
                    tracing::error!("{}", error);
                    query_manager_for_result.display_error_for(query_num, error);
                }
            }
        });

        gtk4::glib::spawn_future_local(async move {
            let mut plan_rendered = false;
            loop {
                let document = snapshots.borrow_and_update().clone();
                if document.execution.query() != Some(query_id) {
                    break;
                }
                let scope = engine_state.borrow().scope();
                if manager_for_plan.tab_view.selected_page().as_ref()
                    == Some(&database_page_for_plan)
                    && let Some(event) = AppEvent::scoped_document_execution(&document, Some(scope))
                {
                    manager_for_plan.dispatch(event);
                }
                let Some(snapshot) = document.operation else {
                    if !document.execution.is_active() || snapshots.changed().await.is_err() {
                        break;
                    }
                    continue;
                };
                if !plan_rendered && (snapshot.plan.is_some() || snapshot.explain_error.is_some()) {
                    query_manager.display_explain_for(
                        query_num,
                        output_generation,
                        snapshot.plan.as_ref(),
                        snapshot.explain_error.as_deref(),
                    );
                    plan_rendered = true;
                }
                if let Some(capture) = snapshot.capture {
                    query_manager.display_stream_for(
                        query_num,
                        output_generation,
                        capture,
                        bridge.clone(),
                    );
                }
                if !document.execution.is_active() || snapshots.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    pub fn request_cancel_current_query(&self) {
        self.dispatch(AppEvent::CancelRequested);
    }

    fn cancel_current_query(&self) {
        let Some(current_page) = self.tab_view.selected_page() else {
            return;
        };
        let Some((query_manager, connection)) = self
            .tabs
            .borrow()
            .values()
            .find(|(page, _, _, _)| page == &current_page)
            .map(|(_, manager, config, _)| (manager.clone(), config.connection()))
        else {
            return;
        };
        let Some((_, document)) = query_manager.running_query() else {
            return;
        };
        let query = document.snapshot().execution.query();
        let Ok(cancellation) = connection.cancel_document(document.clone()) else {
            return;
        };
        if let Some(event) =
            AppEvent::scoped_document_execution(&document.snapshot(), connection.query_scope())
        {
            self.dispatch(event);
        }
        let manager = self.clone();
        gtk4::glib::spawn_future_local(async move {
            match cancellation.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::error!("Failed to cancel query: {error}"),
                Err(_) => tracing::error!("Cancellation delivery was dropped"),
            }
            let snapshot = document.snapshot();
            if snapshot.execution.query() == query
                && manager.tab_view.selected_page().as_ref() == Some(&current_page)
                && let Some(event) =
                    AppEvent::scoped_document_execution(&snapshot, connection.query_scope())
            {
                manager.dispatch(event);
            }
        });
    }

    pub fn new_query_in_current_db(&self) {
        self.open_connection_manager();
    }

    pub fn open_connection_manager(&self) {
        if let Some(page) = self.connections_page.borrow().as_ref() {
            self.tab_view.set_selected_page(page);
        }
        self.focus_connections();
    }

    pub fn focus_connections(&self) -> bool {
        self.connection_manager
            .borrow()
            .as_ref()
            .is_some_and(ConnectionManager::focus_selected)
    }

    pub fn focus_connection(&self, name: &str) -> bool {
        if let Some(page) = self.connections_page.borrow().as_ref() {
            self.tab_view.set_selected_page(page);
        }
        self.connection_manager
            .borrow()
            .as_ref()
            .is_some_and(|manager| manager.focus_database(name))
    }

    pub fn selected_connection_row(&self) -> Option<gtk4::ListBoxRow> {
        self.connection_manager.borrow().as_ref()?.selected_row()
    }

    pub fn test_all_connections(&self) -> bool {
        self.connection_manager
            .borrow()
            .as_ref()
            .is_some_and(ConnectionManager::test_all)
    }

    fn create_connections_tab(&self) {
        // Create connection manager as first pinned tab
        let db_manager = self.clone();
        let bridge = self.runtime_bridge.clone();
        let conn_manager = connections::create_connection_manager(bridge, move |db_name| {
            db_manager.open_database(db_name);
        });

        let page = self.tab_view.append(conn_manager.widget());
        page.set_title("Connections");
        page.set_icon(Some(&gtk4::gio::ThemedIcon::new("network-server-symbolic")));

        // Pin this tab so it stays at the left and can't be closed
        self.tab_view.set_page_pinned(&page, true);

        *self.connections_page.borrow_mut() = Some(page.clone());
        *self.connection_manager.borrow_mut() = Some(conn_manager);
        self.tab_view.set_selected_page(&page);
    }

    pub fn get_database_list(&self) -> Vec<String> {
        // Load config and return database names
        if let Ok(config) = Config::load() {
            config.databases.iter().map(|db| db.name.clone()).collect()
        } else {
            Vec::new()
        }
    }

    #[cfg(test)]
    pub(crate) fn open_test_database(&self, name: &str) {
        let connection_string = "postgresql://test@127.0.0.1/test";
        assert!(
            self.db_manager
                .add_config(name.to_string(), connection_string.to_string())
        );
        let config = self
            .db_manager
            .get_config(name)
            .expect("test database config is available");
        let (page, query_manager, state_widget) = self.create_database_tab(name);
        self.tabs.borrow_mut().insert(
            name.to_string(),
            (page, query_manager, config, state_widget),
        );
    }
}
