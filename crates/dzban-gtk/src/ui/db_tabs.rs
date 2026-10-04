use adw::{TabBar, TabPage, TabView};
use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use super::connection_state::ConnectionStateWidget;
use super::connections::{self, ConnectionManager};
use super::plan_inspector::{PlanScroll, PlanState};
use super::query_state::QueryExecution;
use super::query_tabs::QueryTabManager;
use super::statusbar::Statusbar;
use dzban_gtk::database::{
    AppCommand, AppEvent, AppState, Config, ConnectionOperation, ConnectionState, DbConfig,
    DbManager, InputKey, PlanPreviewState, RuntimeBridge,
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
        manager.handle_input_pressed(InputKey::Control, 0);
        manager.handle_input_pressed(InputKey::Shift, 1);
        *manager.query_preview_manager.borrow_mut() = Some(queries.clone());
        manager.dispatch(AppEvent::PreviewReady(dzban_gtk::database::QueryPlan {
            backend: dzban_gtk::database::DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate: dzban_gtk::database::QueryEstimate::default(),
        }));
        assert!(manager.handle_input_pressed(InputKey::Space, 10));
        assert!(widget.is_visible());
        assert!(manager.handle_input_pressed(InputKey::Space, 500));
        manager.handle_input_released(InputKey::Space, 600);
        assert!(!widget.is_visible());

        *manager.query_preview_manager.borrow_mut() = Some(queries);
        manager.dispatch(AppEvent::PreviewReady(dzban_gtk::database::QueryPlan {
            backend: dzban_gtk::database::DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate: dzban_gtk::database::QueryEstimate::default(),
        }));
        assert!(manager.handle_input_pressed(InputKey::Space, 700));
        manager.handle_input_released(InputKey::Space, 750);
        manager.handle_input_released(InputKey::Shift, 760);
        manager.handle_input_released(InputKey::Control, 770);
        assert!(widget.is_visible());
        manager.dispatch(AppEvent::QueryPlanning);
        assert!(!widget.is_visible());
    }
}

const QUERY_PREVIEW_DELAY: Duration = Duration::from_millis(250);

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
    app_state_callbacks: Rc<RefCell<Vec<AppStateCallback>>>,
    query_preview_generation: Rc<Cell<u64>>,
    query_preview_timer: Rc<RefCell<Option<glib::SourceId>>>,
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
        tab_bar.update_property(&[gtk4::accessible::Property::Label("Database tabs")]);

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
            app_state_callbacks: Rc::new(RefCell::new(Vec::new())),
            query_preview_generation: Rc::new(Cell::new(0)),
            query_preview_timer: Rc::new(RefCell::new(None)),
            query_preview_manager: Rc::new(RefCell::new(None)),
        };

        // Create connection manager as first (leftmost) pinned tab
        manager.create_connections_tab();

        // Handle tab close - if all DB tabs are closed, show connection manager
        let manager_clone = manager.clone();
        tab_view.connect_close_page(move |_view, page| {
            // Find and remove the database tab
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
                manager_clone.tabs.borrow_mut().remove(&name);
                manager_clone.db_manager.remove_config(&name);

                // Check if any database tabs remain
                let tabs_empty = manager_clone.tabs.borrow().is_empty();
                if tabs_empty {
                    // Select connection manager
                    if let Some(conn_page) = manager_clone.connections_page.borrow().as_ref() {
                        manager_clone.tab_view.set_selected_page(conn_page);
                    }
                    manager_clone.statusbar.unbind();
                }
            }

            gtk4::glib::Propagation::Proceed
        });

        // Handle tab selection changes to update statusbar
        let manager_clone = manager.clone();
        tab_view.connect_selected_page_notify(move |view| {
            if let Some(page) = view.selected_page() {
                // First, find if this is a database tab
                let connection = {
                    let tabs = manager_clone.tabs.borrow();
                    tabs.iter()
                        .find(|(_, (tab_page, _, _, _))| tab_page == &page)
                        .map(|(name, (_, query_manager, config, _))| {
                            (name.clone(), query_manager.clone(), config.connection())
                        })
                };

                if let Some((name, query_manager, connection)) = connection {
                    manager_clone
                        .statusbar
                        .bind_to_connection(&name, &connection);
                    let operation = connection.operation();
                    manager_clone.emit_query_availability(connection.operation_available());
                    match operation {
                        ConnectionOperation::PlanningQuery => {
                            manager_clone.dispatch(AppEvent::QueryPlanning)
                        }
                        ConnectionOperation::Query => manager_clone
                            .dispatch(AppEvent::QueryRunning(query_manager.running_plan())),
                        ConnectionOperation::Idle
                        | ConnectionOperation::Ping
                        | ConnectionOperation::Version
                        | ConnectionOperation::Catalog
                        | ConnectionOperation::Closing
                        | ConnectionOperation::Explain => {
                            manager_clone.dispatch(AppEvent::QueryFinished)
                        }
                    }
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
                    manager_clone.emit_query_availability(false);
                }
            }
        });

        manager
    }

    pub fn get_widget(&self) -> &gtk4::Box {
        &self.container
    }

    pub fn set_app_state_callback<F>(&self, callback: F)
    where
        F: Fn(AppState) + 'static,
    {
        self.app_state_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
        self.emit_app_state();
    }

    pub fn begin_query_preview(&self) {
        let Some(current_page) = self.tab_view.selected_page() else {
            return;
        };
        let preview_target = self
            .tabs
            .borrow()
            .values()
            .find(|(page, _, _, _)| page == &current_page)
            .map(|(_, query_manager, config, _)| (query_manager.clone(), config.connection()));
        let Some((query_manager, connection)) = preview_target else {
            return;
        };
        if self.query_preview_timer.borrow().is_some()
            || self.query_preview_manager.borrow().is_some()
        {
            return;
        }
        let Some((_, sql, _)) = query_manager.current_query() else {
            return;
        };

        let generation = self.query_preview_generation.get().wrapping_add(1);
        self.query_preview_generation.set(generation);

        let manager = self.clone();
        let timer = glib::timeout_add_local_once(QUERY_PREVIEW_DELAY, move || {
            manager.query_preview_timer.borrow_mut().take();
            if manager.query_preview_generation.get() != generation {
                return;
            }
            if connection.state() != ConnectionState::Connected {
                manager.dispatch(AppEvent::PreviewFailed(
                    "Database is not connected".to_string(),
                ));
                return;
            }
            let Ok(rx) = connection.explain_query(sql) else {
                return;
            };
            if query_manager.highlight_current_query().is_none() {
                return;
            }
            *manager.query_preview_manager.borrow_mut() = Some(query_manager);
            manager.dispatch(AppEvent::PreviewLoading);
            let manager_for_result = manager.clone();
            glib::spawn_future_local(async move {
                let event = match rx.await {
                    Ok(Ok(plan)) => AppEvent::PreviewReady(plan),
                    Ok(Err(error)) => AppEvent::PreviewFailed(error),
                    Err(_) => AppEvent::PreviewFailed("EXPLAIN was cancelled".to_string()),
                };
                if manager_for_result.query_preview_generation.get() == generation {
                    manager_for_result.dispatch(event);
                }
            });
        });
        *self.query_preview_timer.borrow_mut() = Some(timer);
    }

    pub fn cancel_query_preview(&self) {
        if let Some(timer) = self.query_preview_timer.borrow_mut().take() {
            timer.remove();
        }
        self.query_preview_generation
            .set(self.query_preview_generation.get().wrapping_add(1));
        let query_manager = self.query_preview_manager.borrow_mut().take();
        if let Some(query_manager) = query_manager {
            query_manager.clear_query_highlight();
            query_manager.close_plan();
        }
        self.dispatch(AppEvent::PreviewCancelled);
    }

    fn dispatch(&self, event: AppEvent) {
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

    pub fn handle_input_pressed(&self, key: InputKey, at_ms: u64) -> bool {
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

    pub fn handle_input_released(&self, key: InputKey, at_ms: u64) {
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

    fn emit_query_availability(&self, available: bool) {
        self.dispatch(AppEvent::QueryAvailabilityChanged(available));
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
        let Some((database_page, query_manager)) = self.current_database_tab() else {
            return false;
        };
        let query_tabs = query_manager.tab_count();
        if query_tabs > 0 && !query_manager.close_current_tab() {
            return false;
        }
        if query_tabs == 0 {
            self.cancel_query_preview();
            self.tab_view.close_page(&database_page);
        }
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
        self.current_query_manager()
            .is_some_and(|manager| manager.focus_tab_bar())
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
        self.current_query_manager()
            .is_some_and(|manager| manager.select_next_tab())
    }

    pub fn select_previous_query_tab(&self) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.select_previous_tab())
    }

    pub fn rename_current_query_tab(&self, title: &str) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.rename_current_tab(title))
    }

    pub fn current_query_tab_title(&self) -> Option<String> {
        self.current_query_manager()?.current_tab_title()
    }

    pub fn query_tabs(&self) -> Vec<(u32, String)> {
        self.current_query_manager()
            .map_or_else(Vec::new, |manager| manager.tab_titles())
    }

    pub fn select_query_tab(&self, query_num: u32) -> bool {
        self.current_query_manager()
            .is_some_and(|manager| manager.select_tab(query_num))
    }

    pub fn open_database_tabs(&self) -> Vec<String> {
        let mut tabs = self
            .tabs
            .borrow()
            .iter()
            .map(|(name, (page, _, _, _))| (self.tab_view.page_position(page), name.clone()))
            .collect::<Vec<_>>();
        tabs.sort_by_key(|(position, _)| *position);
        tabs.into_iter().map(|(_, name)| name).collect()
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
        // First check if tab already exists
        let existing_tab = {
            let tabs = self.tabs.borrow();
            tabs.get(db_name)
                .map(|(page, query_manager, _, _)| (page.clone(), query_manager.clone()))
        };

        if let Some((page, query_manager)) = existing_tab {
            // Tab already exists, just select it
            self.tab_view.set_selected_page(&page);
            // Focus the current query editor
            query_manager.focus_current_editor();
        } else {
            // Find database config
            let config = Config::load().ok();
            let db_config =
                config.and_then(|c| c.databases.iter().find(|db| db.name == db_name).cloned());

            if let Some(db_config) = db_config {
                if !self.db_manager.add_database_config(db_config) {
                    return;
                }
                let config = self
                    .db_manager
                    .get_config(db_name)
                    .expect("newly added database config is available");
                let connection = config.connection();
                let (page, query_manager, state_widget) = self.create_database_tab(db_name);

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
                    self.statusbar.bind_to_connection(db_name, &connection);
                    self.emit_query_availability(connection.operation_available());
                }

                let page_for_state = page.clone();
                let state_widget_for_state = state_widget.clone();
                let db_name_for_state = db_name.to_string();
                connection.connect_notify_local(Some("state"), move |connection, _| {
                    let state = connection.state();
                    let (css_class, label) = match state {
                        ConnectionState::Disconnected => ("dim-label", "Disconnected"),
                        ConnectionState::Connecting => ("warning", "Connecting"),
                        ConnectionState::Connected => ("success", "Connected"),
                        ConnectionState::Error => ("error", "Error"),
                    };
                    page_for_state.set_icon(Some(&create_colored_dot_icon(css_class)));
                    state_widget_for_state
                        .update_state(state, Some(&format!("{} - {}", db_name_for_state, label)));
                    if state == ConnectionState::Connected {
                        connection.ping();
                    }
                });

                let manager_for_availability = self.clone();
                let page_for_availability = page.clone();
                connection.connect_notify_local(
                    Some("operation-available"),
                    move |connection, _| {
                        if manager_for_availability.tab_view.selected_page().as_ref()
                            == Some(&page_for_availability)
                        {
                            manager_for_availability
                                .emit_query_availability(connection.operation_available());
                        }
                    },
                );

                let manager_for_operation = self.clone();
                let page_for_operation = page.clone();
                let query_manager_for_operation = query_manager.clone();
                let previous_operation = Rc::new(Cell::new(connection.operation()));
                connection.connect_notify_local(Some("operation"), move |connection, _| {
                    let operation = connection.operation();
                    let previous = previous_operation.replace(operation);
                    if manager_for_operation.tab_view.selected_page().as_ref()
                        != Some(&page_for_operation)
                    {
                        return;
                    }
                    match operation {
                        ConnectionOperation::PlanningQuery => {
                            manager_for_operation.dispatch(AppEvent::QueryPlanning)
                        }
                        ConnectionOperation::Query => manager_for_operation.dispatch(
                            AppEvent::QueryRunning(query_manager_for_operation.running_plan()),
                        ),
                        ConnectionOperation::Idle
                            if matches!(
                                previous,
                                ConnectionOperation::Query | ConnectionOperation::PlanningQuery
                            ) =>
                        {
                            manager_for_operation.dispatch(AppEvent::QueryFinished)
                        }
                        ConnectionOperation::Idle
                        | ConnectionOperation::Ping
                        | ConnectionOperation::Version
                        | ConnectionOperation::Catalog
                        | ConnectionOperation::Closing
                        | ConnectionOperation::Explain => {}
                    }
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
    }

    fn create_database_tab(
        &self,
        db_name: &str,
    ) -> (TabPage, QueryTabManager, ConnectionStateWidget) {
        // Create query tab manager for this database
        let query_manager = QueryTabManager::new(db_name);
        let content = query_manager.get_widget().clone();

        // Create connection state widget for tracking state (not displayed in tab)
        let state_widget = ConnectionStateWidget::new();
        state_widget.update_state(
            ConnectionState::Connecting,
            Some(&format!("{} - Connecting...", db_name)),
        );

        // Add page to TabView
        let page = self.tab_view.append(&content);

        // Set title - just the database name
        page.set_title(db_name);

        // Set orange/yellow dot icon for connecting state
        let icon = create_colored_dot_icon("warning");
        page.set_icon(Some(&icon));

        // Make tab closeable with animation
        self.tab_view.set_page_pinned(&page, false);

        let weak_tab_view = self.tab_view.downgrade();
        let database_page = page.clone();
        query_manager.set_empty_callback(move || {
            if let Some(tab_view) = weak_tab_view.upgrade() {
                tab_view.close_page(&database_page);
            }
        });

        // Open first query tab
        query_manager.new_query_tab();

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
        let Some((query_num, sql, state_manager)) = query_manager.current_query() else {
            tracing::debug!("No executable query in the selected tab");
            return;
        };
        let Ok((query_rx, explain_rx)) = connection.execute_recorded_query(sql) else {
            return;
        };
        let execution = QueryExecution::start(query_num, state_manager);
        let Some(output_generation) = query_manager.show_loading_for(execution.query_num()) else {
            execution.finish();
            return;
        };

        let statusbar = self.statusbar.clone();
        let tab_view = self.tab_view.clone();
        let manager_for_plan = self.clone();
        let database_page_for_plan = database_page.clone();
        let connection_for_plan = connection.clone();
        let query_manager_for_result = query_manager.clone();
        gtk4::glib::spawn_future_local(async move {
            let result = query_rx.await;
            execution.finish();
            query_manager_for_result.refresh_history();
            match result {
                Ok(Ok(result)) => {
                    let row_count = result.result_set.rows.len();
                    let duration = result.duration;
                    tracing::info!("Query returned {} rows", row_count);
                    let command = result.command;
                    if let Some(command) = &command {
                        query_manager_for_result
                            .display_command_for(execution.query_num(), command);
                    } else {
                        query_manager_for_result
                            .display_results_for(execution.query_num(), result.result_set);
                    };
                    if tab_view.selected_page().as_ref() == Some(&database_page)
                        && query_manager_for_result.is_query_selected(execution.query_num())
                    {
                        statusbar.update_rows(command.is_none().then_some(row_count));
                        statusbar.update_exec_time(Some(duration.as_secs_f64()));
                    }
                }
                Ok(Err(error)) => {
                    tracing::error!("Query failed: {}", error);
                    query_manager_for_result
                        .display_error_for(execution.query_num(), &error.to_string());
                }
                Err(_) => {
                    let error = "Query execution was cancelled";
                    tracing::error!("{}", error);
                    query_manager_for_result.display_error_for(execution.query_num(), error);
                }
            }
        });

        gtk4::glib::spawn_future_local(async move {
            let (plan, planning_error) = match explain_rx.await {
                Ok(Ok(plan)) => (Some(plan), None),
                Ok(Err(error)) => (None, Some(error)),
                Err(_) => (
                    None,
                    Some("EXPLAIN request was cancelled".to_string().into()),
                ),
            };
            let explain_error = planning_error.as_ref().map(ToString::to_string);
            query_manager.display_explain_for(
                query_num,
                output_generation,
                plan.as_ref(),
                explain_error.as_deref(),
            );
            if manager_for_plan.tab_view.selected_page().as_ref() == Some(&database_page_for_plan)
                && connection_for_plan.operation() == ConnectionOperation::Query
            {
                manager_for_plan.dispatch(AppEvent::QueryRunning(plan.clone()));
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
        let Some((_, state_manager)) = query_manager.running_query() else {
            return;
        };
        if !state_manager.cancel() {
            return;
        }
        self.dispatch(AppEvent::QueryCanceling);
        if connection.state() != ConnectionState::Connected {
            state_manager.finish();
            self.dispatch(AppEvent::QueryFinished);
            return;
        }
        let manager = self.clone();
        gtk4::glib::spawn_future_local(async move {
            match connection.cancel_query().await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!("Failed to cancel query: {}", error);
                    state_manager.cancel_failed();
                    manager.dispatch(AppEvent::QueryRunning(query_manager.running_plan()));
                }
                Err(_) => {
                    tracing::error!("Cancellation request was dropped");
                    state_manager.cancel_failed();
                    manager.dispatch(AppEvent::QueryRunning(query_manager.running_plan()));
                }
            }
        });
    }

    pub fn new_query_in_current_db(&self) {
        if let Some(current_page) = self.tab_view.selected_page() {
            let tabs = self.tabs.borrow();
            for (page, query_manager, _, _) in tabs.values() {
                if page == &current_page {
                    query_manager.new_query_tab();
                    return;
                }
            }
        }
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
