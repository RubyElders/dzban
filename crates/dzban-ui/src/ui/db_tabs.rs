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
use super::ConnectionModel;
use dzban_db::{Config, ConnectionManager, ConnectionState, RuntimeBridge};

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
    tabs: Rc<RefCell<HashMap<String, (TabPage, QueryTabManager, ConnectionModel, ConnectionStateWidget)>>>,
    connections_page: Rc<RefCell<Option<TabPage>>>,
    connection_manager: ConnectionManager,
    runtime_bridge: RuntimeBridge,
    statusbar: Statusbar,
}

impl DatabaseTabManager {
    pub fn new(connection_manager: ConnectionManager, statusbar: Statusbar) -> Self {
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
            connection_manager: connection_manager.clone(),
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
                // Remove from tabs
                manager_clone.tabs.borrow_mut().remove(&name);
                
                // Disconnect in background
                let conn_mgr = manager_clone.connection_manager.clone();
                gtk4::glib::spawn_future_local(async move {
                    conn_mgr.disconnect(&name).await;
                });
                
                // Check if any database tabs remain
                let tabs_empty = manager_clone.tabs.borrow().is_empty();
                if tabs_empty {
                    // Select connection manager
                    if let Some(conn_page) = manager_clone.connections_page.borrow().as_ref() {
                        manager_clone.tab_view.set_selected_page(conn_page);
                    }
                    // Update statusbar
                    manager_clone.statusbar.update_connection(None, ConnectionState::Disconnected);
                    manager_clone.statusbar.update_version(None);
                    manager_clone.statusbar.update_latency(None);
                }
            }
            
            gtk4::glib::Propagation::Proceed
        });
        
        // Handle tab selection changes to update statusbar
        let manager_clone = manager.clone();
        tab_view.connect_selected_page_notify(move |view| {
            if let Some(page) = view.selected_page() {
                // First, find if this is a database tab
                let model_opt = {
                    let tabs = manager_clone.tabs.borrow();
                    tabs.iter()
                        .find(|(_, (tab_page, _, _, _))| tab_page == &page)
                        .map(|(_, (_, _, model, _))| model.clone())
                };
                
                // Bind statusbar to the model (reactive!)
                if let Some(model) = model_opt {
                    manager_clone.statusbar.bind_to_model(&model);
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
                // Create new database tab (this will trigger set_selected_page)
                let (page, query_manager, connection_model, state_widget) = self.create_database_tab(db_name);
                
                // Insert with connection model
                self.tabs.borrow_mut().insert(db_name.to_string(), (page.clone(), query_manager, connection_model, state_widget.clone()));
                
                // Connect in background using RuntimeBridge
                let bridge = self.runtime_bridge.clone();
                let db_tabs_clone = self.clone();
                let db_name_clone = db_name.to_string();
                let page_clone = page.clone();
                
                // Get connection result channel
                let rx = bridge.connect(db_config);
                
                // Wait for result on GLib's event loop
                gtk4::glib::spawn_future_local(async move {
                    match rx.await {
                        Ok(Ok(db_conn)) => {
                            tracing::info!("✅ Successfully connected to '{}'", db_name_clone);
                            
                            // Update ConnectionModel with the connection
                            let tabs = db_tabs_clone.tabs.borrow();
                            if let Some(entry) = tabs.get(&db_name_clone) {
                                let model = &entry.2;
                                
                                // Set the connection in the model
                                model.set_connection(db_conn.clone());
                                model.update_state(ConnectionState::Connected);
                                
                                // Start monitoring (reactive updates)
                                model.start_monitoring();
                                
                                // Send initial ping to populate latency immediately
                                let conn_ping = db_conn.clone();
                                let model_ping = model.clone();
                                gtk4::glib::spawn_future_local(async move {
                                    if let Ok(latency) = conn_ping.ping().await {
                                        model_ping.update_latency(Some(latency));
                                    }
                                });
                                
                                // Update state widget (still used for internal tracking)
                                entry.3.update_state(ConnectionState::Connected, Some(&format!("{} - Connected", db_name_clone)));
                            }
                            drop(tabs);
                            
                            // Update icon to show connected state (green dot)
                            let icon = create_colored_dot_icon("success");
                            page_clone.set_icon(Some(&icon));
                            
                            // Bind statusbar to model if this is the selected tab
                            if db_tabs_clone.tab_view.selected_page().as_ref() == Some(&page_clone) {
                                let tabs = db_tabs_clone.tabs.borrow();
                                if let Some(entry) = tabs.get(&db_name_clone) {
                                    db_tabs_clone.statusbar.bind_to_model(&entry.2);
                                }
                            }
                            
                            // Setup periodic latency updates using RuntimeBridge
                            let bridge_ping = db_tabs_clone.runtime_bridge.clone();
                            let db_conn_ping = db_conn.clone();
                            let db_name_for_ping = db_name_clone.clone();
                            let tabs_for_ping = db_tabs_clone.tabs.clone();
                            
                            // Spawn periodic ping on GLib event loop
                            gtk4::glib::timeout_add_seconds_local(5, move || {
                                let bridge_clone = bridge_ping.clone();
                                let conn_clone = db_conn_ping.clone();
                                let db_name_ping = db_name_for_ping.clone();
                                let tabs_ping = tabs_for_ping.clone();
                                
                                gtk4::glib::spawn_future_local(async move {
                                    // Ping and update model (which automatically updates UI via property binding)
                                    let ping_rx = bridge_clone.ping(conn_clone);
                                    if let Ok(Ok(latency)) = ping_rx.await {
                                        let tabs = tabs_ping.borrow();
                                        if let Some(entry) = tabs.get(&db_name_ping) {
                                            entry.2.update_latency(Some(latency));
                                        }
                                    }
                                });
                                
                                gtk4::glib::ControlFlow::Continue
                            });
                        }
                        Ok(Err(e)) => {
                            tracing::error!("Failed to connect to '{}': {}", db_name_clone, e);
                            // Update state widget to show error
                            let tabs = db_tabs_clone.tabs.borrow();
                            if let Some(entry) = tabs.get(&db_name_clone) {
                                entry.3.update_state(ConnectionState::Error, Some(&format!("{} - Error", db_name_clone)));
                            }
                            // Update icon to show error state (red dot)
                            let icon = create_colored_dot_icon("error");
                            page_clone.set_icon(Some(&icon));
                        }
                        Err(_) => {
                            tracing::error!("Connection task was cancelled for '{}'", db_name_clone);
                            // Update state widget to show error
                            let tabs = db_tabs_clone.tabs.borrow();
                            if let Some(entry) = tabs.get(&db_name_clone) {
                                entry.3.update_state(ConnectionState::Error, Some(&format!("{} - Cancelled", db_name_clone)));
                            }
                            // Update icon to show error state (red dot)
                            let icon = create_colored_dot_icon("error");
                            page_clone.set_icon(Some(&icon));
                        }
                    }
                });
            }
        }
    }
    
    fn create_database_tab(&self, db_name: &str) -> (TabPage, QueryTabManager, ConnectionModel, ConnectionStateWidget) {
        // Create query tab manager for this database
        let query_manager = QueryTabManager::new(db_name);
        let content = query_manager.get_widget().clone();
        
        // Create connection model (GObject for reactive updates)
        let connection_model = ConnectionModel::new(db_name.to_string());
        connection_model.update_state(ConnectionState::Connecting);
        
        // Set up auto-EXPLAIN callback
        let bridge_for_explain = self.runtime_bridge.clone();
        let connection_model_for_explain = connection_model.clone();
        let query_manager_for_explain = query_manager.clone();
        query_manager.set_explain_callback(move |explain_query| {
            tracing::info!("📊 Running auto-EXPLAIN query in background");
            
            if let Some(connection) = connection_model_for_explain.get_connection() {
                let bridge = bridge_for_explain.clone();
                let query_mgr = query_manager_for_explain.clone();
                
                gtk4::glib::spawn_future_local(async move {
                    let rx = bridge.execute_query(connection, explain_query.clone());
                    
                    match rx.await {
                        Ok(Ok(rows)) => {
                            if let Some(first_row) = rows.first() {
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
            } else {
                tracing::warn!("No database connection available for auto-EXPLAIN");
            }
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
        
        (page, query_manager, connection_model, state_widget)
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
            for (db_name, (page, query_manager, connection_model, _)) in tabs.iter() {
                if page == &current_page {
                    // Get the query state manager
                    let state_manager = match query_manager.get_current_state_manager() {
                        Some(sm) => sm,
                        None => {
                            query_manager.display_error("No query tab selected");
                            return;
                        }
                    };
                    
                    // Check if query is already running
                    if !state_manager.try_start() {
                        // Query is already running, ignore this request
                        tracing::debug!("Query already running, ignoring duplicate request");
                        return;
                    }
                    
                    // Get the SQL query text
                    if let Some(sql) = query_manager.get_current_query_text() {
                        let sql = sql.trim();
                        if sql.is_empty() {
                            state_manager.finish();
                            query_manager.display_error("No query to execute");
                            return;
                        }
                        
                        // Show loading state
                        query_manager.show_loading();
                        
                        // Get the connection
                        if let Some(connection) = connection_model.get_connection() {
                            let bridge = self.runtime_bridge.clone();
                            let query_manager_clone = query_manager.clone();
                            let sql_string = sql.to_string();
                            let state_manager_clone = state_manager.clone();
                            
                            gtk4::glib::spawn_future_local(async move {
                                // Execute query on Tokio runtime via bridge
                                let rx = bridge.execute_query(connection, sql_string);
                                
                                match rx.await {
                                    Ok(Ok(rows)) => {
                                        tracing::info!("Query returned {} rows", rows.len());
                                        query_manager_clone.display_results(rows);
                                        // state_manager.finish() is called in display_results
                                    }
                                    Ok(Err(e)) => {
                                        tracing::error!("Query failed: {}", e);
                                        query_manager_clone.display_error(&e);
                                        // state_manager.finish() is called in display_error
                                    }
                                    Err(_) => {
                                        tracing::error!("Query execution was cancelled");
                                        state_manager_clone.finish();
                                        query_manager_clone.display_error("Query execution was cancelled");
                                    }
                                }
                            });
                        } else {
                            state_manager.finish();
                            query_manager.display_error(&format!("Not connected to {}", db_name));
                        }
                    } else {
                        state_manager.finish();
                    }
                    return;
                }
            }
        }
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

