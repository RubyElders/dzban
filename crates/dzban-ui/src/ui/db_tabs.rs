use gtk4::prelude::*;
use gtk4::gdk;
use libadwaita as adw;
use adw::{TabView, TabBar, TabPage};
use std::collections::HashMap;
use std::cell::RefCell;
use std::rc::Rc;

use super::query_tabs::QueryTabManager;
use super::connections;
use super::statusbar::Statusbar;
use super::connection_state::ConnectionStateWidget;
use dzban_db::{Config, ConnectionState, DbConnection, RuntimeBridge};

type DatabaseTab = (TabPage, QueryTabManager, DbConnection, ConnectionStateWidget);
type DatabaseTabs = Rc<RefCell<HashMap<String, DatabaseTab>>>;

// Create a simple colored circle icon using Cairo with exact CSS color
#[allow(deprecated)]
fn create_colored_dot_icon(css_class: &str) -> gtk4::gio::Icon {
    // Create a temporary label with the CSS class to get the actual rendered color
    let label = gtk4::Label::new(Some("●"));
    label.add_css_class(css_class);
    
    // Get the actual color from the CSS (widget doesn't need to be realized)
    let context = label.style_context();
    let color = context.color();
    
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
            color.alpha() as f64
        );
        cr.arc(8.0, 8.0, 4.0, 0.0, 2.0 * std::f64::consts::PI);
        cr.fill().expect("Failed to fill circle");
    } // Drop cr to release borrow
    
    // Get stride before borrowing data mutably
    let stride = surface.stride() as usize;
    
    // Get surface data and create texture from memory
    let data = surface.data().expect("Failed to get surface data");
    let bytes = gtk4::glib::Bytes::from(&data[..]);
    let texture = gdk::MemoryTexture::new(
        16,
        16,
        gdk::MemoryFormat::B8g8r8a8,
        &bytes,
        stride
    );
    texture.upcast()
}

#[derive(Clone)]
pub struct DatabaseTabManager {
    tab_view: TabView,
    _tab_bar: TabBar,
    container: gtk4::Box,
    tabs: DatabaseTabs,
    connections_page: Rc<RefCell<Option<TabPage>>>,
    runtime_bridge: RuntimeBridge,
    statusbar: Statusbar,
}

impl DatabaseTabManager {
    pub fn new(statusbar: Statusbar) -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.set_vexpand(true);
        
        // Create TabView for modern Adwaita tabs with animations
        let tab_view = TabView::new();
        tab_view.set_vexpand(true);
        
        // Enable shortcuts (Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+W, etc.)
        tab_view.set_shortcuts(adw::TabViewShortcuts::all());
        
        // Create TabBar for the tab strip
        let tab_bar = TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);
        tab_bar.set_expand_tabs(false);
        
        container.append(&tab_bar);
        container.append(&tab_view);
        
        // Create runtime bridge for Tokio <-> GLib communication
        let runtime_bridge = RuntimeBridge::new()
            .expect("Failed to create RuntimeBridge");
        
        let manager = Self {
            tab_view: tab_view.clone(),
            _tab_bar: tab_bar,
            container,
            tabs: Rc::new(RefCell::new(HashMap::new())),
            connections_page: Rc::new(RefCell::new(None)),
            runtime_bridge,
            statusbar: statusbar.clone(),
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
                if let Some((_, _, connection, _)) = manager_clone.tabs.borrow_mut().remove(&name) {
                    connection.disconnect();
                }
                
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
                        .map(|(name, (_, _, connection, _))| (name.clone(), connection.clone()))
                };
                
                if let Some((name, connection)) = connection {
                    manager_clone.statusbar.bind_to_connection(&name, &connection);
                    return;
                }
                
                // If it's the connections page, unbind statusbar
                let is_connections_page = {
                    manager_clone.connections_page.borrow()
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
    
    /// Set the search entry for updating with action status
    pub fn set_search_entry(&self, search_entry: &gtk4::SearchEntry) {
        use super::constants::EXPLAIN_DELAY_MS;
        
        let search_entry_clone = search_entry.clone();
        
        // Find the progress bar in the search entry's parent container
        let progress_bar = if let Some(parent) = search_entry.parent() {
            if let Ok(container) = parent.downcast::<gtk4::Box>() {
                // The progress bar is the second child
                container.last_child()
                    .and_then(|child| child.downcast::<gtk4::ProgressBar>().ok())
            } else {
                None
            }
        } else {
            None
        };
        
        // Track the animation timer
        let timer_handle: std::rc::Rc<std::cell::RefCell<Option<gtk4::glib::SourceId>>> = 
            std::rc::Rc::new(std::cell::RefCell::new(None));
        
        // Set the callback on each existing tab's query manager
        let tabs = self.tabs.borrow();
        for (_, (_, query_manager, _, _)) in tabs.iter() {
            let se = search_entry_clone.clone();
            let pb = progress_bar.clone();
            let timer = timer_handle.clone();
            
            query_manager.set_action_bar_callback(move |text, show_progress| {
                se.set_placeholder_text(Some(text));
                
                // Cancel any existing timer
                if let Some(timer_id) = timer.borrow_mut().take() {
                    timer_id.remove();
                }
                
                if let Some(ref progress) = pb {
                    progress.set_visible(show_progress);
                    
                    if show_progress {
                        // Animate progress bar over EXPLAIN_DELAY_MS
                        progress.set_fraction(0.0);
                        
                        let progress_clone = progress.clone();
                        let timer_clone = timer.clone();
                        let steps = 20;
                        let interval_ms = EXPLAIN_DELAY_MS / steps;
                        let increment = 1.0 / steps as f64;
                        
                        let current_step = std::rc::Rc::new(std::cell::RefCell::new(0u64));
                        
                        let timer_id = gtk4::glib::timeout_add_local(
                            std::time::Duration::from_millis(interval_ms),
                            {
                                let current_step = current_step.clone();
                                move || {
                                    let mut step = current_step.borrow_mut();
                                    *step += 1;
                                    
                                    let fraction = (*step as f64) * increment;
                                    progress_clone.set_fraction(fraction.min(1.0));
                                    
                                    if *step >= steps {
                                        gtk4::glib::ControlFlow::Break
                                    } else {
                                        gtk4::glib::ControlFlow::Continue
                                    }
                                }
                            }
                        );
                        
                        *timer_clone.borrow_mut() = Some(timer_id);
                    } else {
                        progress.set_fraction(0.0);
                    }
                }
            });
        }
    }
    
    pub fn open_database(&self, db_name: &str) {
        // First check if tab already exists
        let existing_tab = {
            let tabs = self.tabs.borrow();
            tabs.get(db_name).map(|(page, query_manager, _, _)| (page.clone(), query_manager.clone()))
        };
        
        if let Some((page, query_manager)) = existing_tab {
            // Tab already exists, just select it
            self.tab_view.set_selected_page(&page);
            // Focus the current query editor
            query_manager.focus_current_editor();
        } else {
            // Find database config
            let config = Config::load().ok();
            let db_config = config.and_then(|c| {
                c.databases.iter().find(|db| db.name == db_name).cloned()
            });
            
            if let Some(db_config) = db_config {
                let connection = DbConnection::new(db_config.connection_string());
                let (page, query_manager, state_widget) =
                    self.create_database_tab(db_name, &connection);

                self.tabs.borrow_mut().insert(
                    db_name.to_string(),
                    (page.clone(), query_manager, connection.clone(), state_widget.clone()),
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
                    state_widget_for_state.update_state(
                        state,
                        Some(&format!("{} - {}", db_name_for_state, label)),
                    );
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
        connection: &DbConnection,
    ) -> (TabPage, QueryTabManager, ConnectionStateWidget) {
        // Create query tab manager for this database
        let query_manager = QueryTabManager::new(db_name);
        let content = query_manager.get_widget().clone();
        
        // Set up auto-EXPLAIN callback
        let connection_for_explain = connection.clone();
        let query_manager_for_explain = query_manager.clone();
        query_manager.set_explain_callback(move |explain_query| {
            tracing::info!("📊 Running auto-EXPLAIN query in background");
            let connection = connection_for_explain.clone();
            let query_mgr = query_manager_for_explain.clone();

            gtk4::glib::spawn_future_local(async move {
                let rx = connection.execute_query_rows(explain_query.clone());
                    
                    match rx.await {
                        Ok(Ok(result)) => {
                            if let Some(first_row) = result.rows.first() {
                                // Try to get the JSON column
                                if let Ok(Some(json_val)) = first_row.try_get::<_, Option<serde_json::Value>>(0) {
                                    let formatted = serde_json::to_string_pretty(&json_val)
                                        .unwrap_or_else(|_| json_val.to_string());
                                    tracing::info!("✅ Auto-EXPLAIN result:\n{}", formatted);
                                    
                                    // Show the results in the fancy slider
                                    query_mgr.show_explain_results(&json_val);
                                } else {
                                    tracing::warn!("Could not parse EXPLAIN result as JSON");
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::error!("❌ Auto-EXPLAIN failed: {}", e);
                        }
                        Err(_) => {
                            tracing::error!("❌ Auto-EXPLAIN was cancelled");
                        }
                    }
            });
        });
        
        // Create connection state widget for tracking state (not displayed in tab)
        let state_widget = ConnectionStateWidget::new();
        state_widget.update_state(ConnectionState::Connecting, Some(&format!("{} - Connecting...", db_name)));
        
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
            for (_db_name, (page, query_manager, _, _)) in tabs.iter() {
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
            for (_db_name, (page, query_manager, _, _)) in tabs.iter() {
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
            for (db_name, (page, query_manager, connection, _)) in tabs.iter() {
                if page == &current_page {
                    let (query_num, sql, state_manager) = match query_manager.current_query() {
                        Some(query) => query,
                        None => {
                            tracing::debug!("No executable query in the selected tab");
                            return;
                        }
                    };
                    
                    // Check if query is already running
                    if !state_manager.try_start() {
                        // Query is already running, ignore this request
                        tracing::debug!("Query already running, ignoring duplicate request");
                        return;
                    }
                    
                    query_manager.show_loading_for(query_num);

                    if connection.state() == ConnectionState::Connected {
                        let connection = connection.clone();
                        let query_manager = query_manager.clone();
                        let statusbar = self.statusbar.clone();
                        let tab_view = self.tab_view.clone();
                        let database_page = page.clone();

                        gtk4::glib::spawn_future_local(async move {
                            let rx = connection.execute_query_rows(sql);

                            match rx.await {
                                Ok(Ok(result)) => {
                                    let row_count = result.rows.len();
                                    tracing::info!("Query returned {} rows", row_count);
                                    query_manager.display_results_for(query_num, result.rows);
                                    if tab_view.selected_page().as_ref() == Some(&database_page) {
                                        statusbar.update_rows(Some(row_count));
                                        statusbar.update_exec_time(Some(result.duration.as_secs_f64()));
                                    }
                                }
                                Ok(Err(error)) => {
                                    tracing::error!("Query failed: {}", error);
                                    query_manager.display_error_for(query_num, &error);
                                }
                                Err(_) => {
                                    let error = "Query execution was cancelled";
                                    tracing::error!("{}", error);
                                    query_manager.display_error_for(query_num, error);
                                }
                            }
                        });
                    } else {
                        state_manager.finish();
                        query_manager.display_error_for(
                            query_num,
                            &format!("Not connected to {}", db_name),
                        );
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
        let Some((_, query_manager, connection, _)) = tabs
            .values()
            .find(|(page, _, _, _)| page == &current_page)
        else {
            return;
        };
        let Some((_, _, state_manager)) = query_manager.current_query() else {
            return;
        };
        if !state_manager.cancel() {
            return;
        }
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
            for (_db_name, (page, query_manager, _, _)) in tabs.iter() {
                if page == &current_page {
                    query_manager.new_query_tab();
                    return;
                }
            }
        }
    }
    
    pub fn open_connection_manager(&self) {
        if let Some(page) = self.connections_page.borrow().as_ref() {
            // Select the existing connection manager tab
            self.tab_view.set_selected_page(page);
        }
    }
    
    fn create_connections_tab(&self) {
        // Create connection manager as first pinned tab
        let db_manager = self.clone();
        let bridge = self.runtime_bridge.clone();
        let conn_manager = connections::create_connection_manager(bridge, move |db_name| {
            db_manager.open_database(db_name);
        });
        
        let page = self.tab_view.append(&conn_manager);
        page.set_title("Connections");
        page.set_icon(Some(&gtk4::gio::ThemedIcon::new("network-server-symbolic")));
        
        // Pin this tab so it stays at the left and can't be closed
        self.tab_view.set_page_pinned(&page, true);
        
        *self.connections_page.borrow_mut() = Some(page.clone());
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
