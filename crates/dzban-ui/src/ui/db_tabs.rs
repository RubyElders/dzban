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
use dzban_db::{Config, ConnectionState, DbConfig, DbManager, RuntimeBridge};

type DatabaseTab = (TabPage, QueryTabManager, DbConfig, ConnectionStateWidget);
type DatabaseTabs = Rc<RefCell<HashMap<String, DatabaseTab>>>;
type QueryPreviewCallback = Rc<dyn Fn(QueryPreviewState)>;

const QUERY_PREVIEW_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub enum QueryPreviewState {
    Idle,
    Loading,
    Ready {
        rows: i64,
        startup_cost: f64,
        total_cost: f64,
        plan: serde_json::Value,
    },
    Error(String),
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
    query_preview_callback: Rc<RefCell<Option<QueryPreviewCallback>>>,
    query_preview_generation: Rc<Cell<u64>>,
    query_preview_timer: Rc<RefCell<Option<glib::SourceId>>>,
    query_preview_manager: Rc<RefCell<Option<QueryTabManager>>>,
}

impl DatabaseTabManager {
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
            runtime_bridge,
            db_manager: DbManager::new(),
            statusbar: statusbar.clone(),
            query_preview_callback: Rc::new(RefCell::new(None)),
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
                        .map(|(name, (_, _, config, _))| (name.clone(), config.connection()))
                };

                if let Some((name, connection)) = connection {
                    manager_clone
                        .statusbar
                        .bind_to_connection(&name, &connection);
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
                }
            }
        });

        manager
    }

    pub fn get_widget(&self) -> &gtk4::Box {
        &self.container
    }

    pub fn set_query_preview_callback<F>(&self, callback: F)
    where
        F: Fn(QueryPreviewState) + 'static,
    {
        *self.query_preview_callback.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn begin_query_preview(&self) {
        self.cancel_query_preview();

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
        let Some(sql) = query_manager.highlight_current_query() else {
            return;
        };

        let generation = self.query_preview_generation.get().wrapping_add(1);
        self.query_preview_generation.set(generation);
        *self.query_preview_manager.borrow_mut() = Some(query_manager);
        self.emit_query_preview(QueryPreviewState::Loading);

        let manager = self.clone();
        let timer = glib::timeout_add_local_once(QUERY_PREVIEW_DELAY, move || {
            manager.query_preview_timer.borrow_mut().take();
            if manager.query_preview_generation.get() != generation {
                return;
            }
            if connection.state() != ConnectionState::Connected {
                manager.emit_query_preview(QueryPreviewState::Error(
                    "Database is not connected".to_string(),
                ));
                return;
            }
            let Some(explain_sql) = explain_statement(&sql) else {
                manager.emit_query_preview(QueryPreviewState::Error(
                    "Current statement is already EXPLAIN".to_string(),
                ));
                return;
            };

            let manager_for_result = manager.clone();
            let rx = connection.execute_query_rows(explain_sql);
            glib::spawn_future_local(async move {
                let state = match rx.await {
                    Ok(Ok(result)) => extract_explain_plan(&result.rows)
                        .and_then(|plan| {
                            extract_query_estimate(&plan).map(|(rows, startup_cost, total_cost)| {
                                QueryPreviewState::Ready {
                                    rows,
                                    startup_cost,
                                    total_cost,
                                    plan,
                                }
                            })
                        })
                        .unwrap_or_else(QueryPreviewState::Error),
                    Ok(Err(error)) => QueryPreviewState::Error(error),
                    Err(_) => QueryPreviewState::Error("EXPLAIN was cancelled".to_string()),
                };
                if manager_for_result.query_preview_generation.get() == generation {
                    manager_for_result.emit_query_preview(state);
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
            query_manager.hide_plan_transient();
            self.emit_query_preview(QueryPreviewState::Idle);
        }
    }

    fn emit_query_preview(&self, state: QueryPreviewState) {
        if let Some(query_manager) = self.query_preview_manager.borrow().as_ref() {
            match &state {
                QueryPreviewState::Loading => query_manager.set_plan_preview(PlanState::Loading),
                QueryPreviewState::Ready { plan, .. } => {
                    query_manager.set_plan_preview(PlanState::Ready(plan.clone()));
                }
                QueryPreviewState::Error(error) => {
                    query_manager.set_plan_preview(PlanState::Error(error.clone()));
                }
                QueryPreviewState::Idle => {}
            }
        }
        if let Some(callback) = self.query_preview_callback.borrow().as_ref() {
            callback(state);
        }
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

    pub fn hide_query_plan(&self) {
        if let Some(manager) = self.query_preview_manager.borrow().as_ref() {
            manager.hide_plan_transient();
        }
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
        self.current_query_manager()
            .is_some_and(|manager| manager.close_current_tab())
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
                if !self
                    .db_manager
                    .add_config(db_name.to_string(), db_config.connection_string())
                {
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
                    (page.clone(), query_manager, config, state_widget.clone()),
                );

                if self.tab_view.selected_page().as_ref() == Some(&page) {
                    self.statusbar.bind_to_connection(db_name, &connection);
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
        // Get the current database tab
        if let Some(current_page) = self.tab_view.selected_page() {
            let tabs = self.tabs.borrow();

            // Find which database tab this is
            for (db_name, (page, query_manager, config, _)) in tabs.iter() {
                if page == &current_page {
                    let connection = config.connection();
                    let (query_num, sql, state_manager) = match query_manager.current_query() {
                        Some(query) => query,
                        None => {
                            tracing::debug!("No executable query in the selected tab");
                            return;
                        }
                    };

                    let Some(execution) = QueryExecution::start(query_num, state_manager) else {
                        // Query is already running, ignore this request
                        tracing::debug!("Query already running, ignoring duplicate request");
                        return;
                    };

                    let Some(output_generation) =
                        query_manager.show_loading_for(execution.query_num())
                    else {
                        execution.finish();
                        return;
                    };

                    if connection.state() == ConnectionState::Connected {
                        let connection = connection.clone();
                        let query_manager = query_manager.clone();
                        let statusbar = self.statusbar.clone();
                        let tab_view = self.tab_view.clone();
                        let database_page = page.clone();
                        let history_sql = sql.clone();

                        gtk4::glib::spawn_future_local(async move {
                            let rx = connection.execute_query_rows(sql);

                            match rx.await {
                                Ok(Ok(result)) => {
                                    execution.finish();
                                    let row_count = result.rows.len();
                                    let duration = result.duration;
                                    let explain_sql = explain_statement(&history_sql);
                                    let embedded_plan = if explain_sql.is_none() {
                                        extract_explain_plan(&result.rows).ok()
                                    } else {
                                        None
                                    };
                                    tracing::info!("Query returned {} rows", row_count);
                                    let stored_result = query_manager
                                        .display_results_for(execution.query_num(), result.rows);
                                    if tab_view.selected_page().as_ref() == Some(&database_page)
                                        && query_manager.is_query_selected(execution.query_num())
                                    {
                                        statusbar.update_rows(Some(row_count));
                                        statusbar.update_exec_time(Some(duration.as_secs_f64()));
                                    }

                                    let (explain, explain_error) = match explain_sql {
                                        Some(explain_sql) => {
                                            match connection.execute_query_rows(explain_sql).await {
                                                Ok(Ok(explain_result)) => {
                                                    match extract_explain_plan(&explain_result.rows) {
                                                        Ok(plan) => (Some(plan), None),
                                                        Err(error) => (None, Some(error)),
                                                    }
                                                }
                                                Ok(Err(error)) => (None, Some(error)),
                                                Err(_) => (
                                                    None,
                                                    Some("EXPLAIN request was cancelled".to_string()),
                                                ),
                                            }
                                        }
                                        None => embedded_plan.map_or_else(
                                            || {
                                                (
                                                    None,
                                                    Some(
                                                        "The existing EXPLAIN did not return a JSON plan"
                                                            .to_string(),
                                                    ),
                                                )
                                            },
                                            |plan| (Some(plan), None),
                                        ),
                                    };
                                    query_manager.display_explain_for(
                                        execution.query_num(),
                                        output_generation,
                                        explain.as_ref(),
                                        explain_error.as_deref(),
                                    );
                                    query_manager.record_query_success(
                                        history_sql,
                                        duration,
                                        row_count,
                                        stored_result,
                                        explain,
                                        explain_error,
                                    );
                                }
                                Ok(Err(error)) => {
                                    execution.finish();
                                    tracing::error!("Query failed: {}", error);
                                    query_manager.display_error_for(execution.query_num(), &error);
                                    query_manager.record_query_failure(history_sql, error);
                                }
                                Err(_) => {
                                    execution.finish();
                                    let error = "Query execution was cancelled";
                                    tracing::error!("{}", error);
                                    query_manager.display_error_for(execution.query_num(), error);
                                    query_manager
                                        .record_query_failure(history_sql, error.to_string());
                                }
                            }
                        });
                    } else {
                        execution.finish();
                        let error = format!("Not connected to {}", db_name);
                        query_manager.display_error_for(execution.query_num(), &error);
                        query_manager.record_query_failure(sql, error);
                    }
                    return;
                }
            }
        }
    }

    pub fn cancel_current_query(&self) {
        let Some(current_page) = self.tab_view.selected_page() else {
            return;
        };
        let tabs = self.tabs.borrow();
        let Some((_, query_manager, config, _)) =
            tabs.values().find(|(page, _, _, _)| page == &current_page)
        else {
            return;
        };
        let Some((_, _, state_manager)) = query_manager.current_query() else {
            return;
        };
        if !state_manager.cancel() {
            return;
        }
        let connection = config.connection();
        if connection.state() != ConnectionState::Connected {
            state_manager.finish();
            return;
        }
        let connection = connection.clone();
        gtk4::glib::spawn_future_local(async move {
            match connection.cancel_query().await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!("Failed to cancel query: {}", error);
                    state_manager.cancel_failed();
                }
                Err(_) => {
                    tracing::error!("Cancellation request was dropped");
                    state_manager.cancel_failed();
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
}

fn explain_statement(sql: &str) -> Option<String> {
    let sql = sql.trim();
    if sql
        .split_whitespace()
        .next()
        .is_some_and(|keyword| keyword.eq_ignore_ascii_case("EXPLAIN"))
    {
        None
    } else {
        Some(format!("EXPLAIN (FORMAT JSON) {}", sql))
    }
}

fn extract_explain_plan(rows: &[dzban_db::Row]) -> Result<serde_json::Value, String> {
    rows.first()
        .ok_or_else(|| "EXPLAIN returned no rows".to_string())?
        .try_get::<_, serde_json::Value>(0)
        .map_err(|error| format!("EXPLAIN returned an invalid JSON plan: {}", error))
}

fn extract_query_estimate(plan: &serde_json::Value) -> Result<(i64, f64, f64), String> {
    let plan = plan
        .as_array()
        .and_then(|plans| plans.first())
        .and_then(|root| root.get("Plan"))
        .ok_or_else(|| "EXPLAIN returned no query plan".to_string())?;
    let rows = plan
        .get("Plan Rows")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| "EXPLAIN returned no row estimate".to_string())?;
    let startup_cost = plan
        .get("Startup Cost")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| "EXPLAIN returned no startup cost".to_string())?;
    let total_cost = plan
        .get("Total Cost")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| "EXPLAIN returned no total cost".to_string())?;
    Ok((rows, startup_cost, total_cost))
}

#[cfg(test)]
mod tests {
    use super::{explain_statement, extract_query_estimate};

    #[test]
    fn explain_is_plain_and_json_formatted() {
        assert_eq!(
            explain_statement("select * from users"),
            Some("EXPLAIN (FORMAT JSON) select * from users".to_string())
        );
        assert!(
            !explain_statement("select * from users")
                .unwrap()
                .contains("ANALYZE")
        );
    }

    #[test]
    fn existing_explain_is_not_wrapped() {
        assert_eq!(explain_statement("  explain select 1"), None);
    }

    #[test]
    fn query_estimate_uses_the_root_plan() {
        let plan = serde_json::json!([{
            "Plan": {
                "Plan Rows": 42,
                "Startup Cost": 1.25,
                "Total Cost": 9.5
            }
        }]);

        assert_eq!(extract_query_estimate(&plan), Ok((42, 1.25, 9.5)));
    }
}
