use gtk4::prelude::*;
use libadwaita as adw;
use adw::{TabView, TabBar, TabPage};
use std::collections::HashMap;
use std::cell::RefCell;
use std::rc::Rc;

use super::query_tabs::QueryTabManager;
use super::connections;
use super::statusbar::Statusbar;
use crate::config::Config;
use crate::db::{ConnectionManager, DbConnection, ConnectionState};

#[derive(Clone)]
pub struct DatabaseTabManager {
    tab_view: TabView,
    _tab_bar: TabBar,
    container: gtk4::Box,
    tabs: Rc<RefCell<HashMap<String, (TabPage, QueryTabManager, Option<DbConnection>)>>>,
    connections_page: Rc<RefCell<Option<TabPage>>>,
    connection_manager: ConnectionManager,
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
        
        let manager = Self {
            tab_view: tab_view.clone(),
            _tab_bar: tab_bar,
            container,
            tabs: Rc::new(RefCell::new(HashMap::new())),
            connections_page: Rc::new(RefCell::new(None)),
            connection_manager: connection_manager.clone(),
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
                    .find(|(_, (p, _, _))| p == page)
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
                // First, find if this is a database tab and extract the data we need
                let db_info = {
                    let tabs = manager_clone.tabs.borrow();
                    tabs.iter()
                        .find(|(_, (tab_page, _, _))| tab_page == &page)
                        .and_then(|(db_name, (_, _, db_conn_opt))| {
                            db_conn_opt.as_ref().map(|conn| (db_name.clone(), conn.clone()))
                        })
                };
                
                // Now update statusbar without holding the borrow
                if let Some((db_name, db_conn)) = db_info {
                    let statusbar = manager_clone.statusbar.clone();
                    gtk4::glib::spawn_future_local(async move {
                        let state = db_conn.get_state().await;
                        let version = db_conn.get_version().await;
                        let latency = db_conn.get_latency().await;
                        
                        statusbar.update_connection(Some(&db_name), state);
                        statusbar.update_version(version.as_deref());
                        statusbar.update_latency(latency.map(|d| d.as_millis() as u64));
                    });
                    return;
                }
                
                // If it's the connections page, clear statusbar
                let is_connections_page = {
                    manager_clone.connections_page.borrow()
                        .as_ref()
                        .map(|conn_page| conn_page == &page)
                        .unwrap_or(false)
                };
                
                if is_connections_page {
                    manager_clone.statusbar.update_connection(None, ConnectionState::Disconnected);
                    manager_clone.statusbar.update_version(None);
                    manager_clone.statusbar.update_latency(None);
                }
            }
        });
        
        manager
    }
    
    pub fn get_widget(&self) -> &gtk4::Box {
        &self.container
    }
    
    pub fn open_database(&self, db_name: &str) {
        // First check if tab already exists
        let existing_tab = {
            let tabs = self.tabs.borrow();
            tabs.get(db_name).map(|(page, query_manager, _)| (page.clone(), query_manager.clone()))
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
                let (page, query_manager) = self.create_database_tab(db_name);
                
                // Insert with no connection initially
                self.tabs.borrow_mut().insert(db_name.to_string(), (page.clone(), query_manager, None));
                
                // Connect in background
                let conn_mgr = self.connection_manager.clone();
                let db_tabs_clone = self.clone();
                let db_name_clone = db_name.to_string();
                let page_clone = page.clone();
                
                gtk4::glib::spawn_future_local(async move {
                    // Update tab icon to connecting
                    let icon_connecting = gtk4::gio::ThemedIcon::new("document-open-recent-symbolic");
                    page_clone.set_icon(Some(&icon_connecting));
                    
                    match conn_mgr.connect(&db_config).await {
                        Ok(db_conn) => {
                            // Store connection
                            let mut tabs = db_tabs_clone.tabs.borrow_mut();
                            if let Some(entry) = tabs.get_mut(&db_name_clone) {
                                entry.2 = Some(db_conn.clone());
                            }
                            drop(tabs);
                            
                            // Update tab icon to connected (green dot)
                            let icon_connected = gtk4::gio::ThemedIcon::new("emblem-default-symbolic");
                            page_clone.set_icon(Some(&icon_connected));
                            
                            // Update statusbar if this is the selected tab
                            if db_tabs_clone.tab_view.selected_page().as_ref() == Some(&page_clone) {
                                let state = db_conn.get_state().await;
                                let version = db_conn.get_version().await;
                                let latency = db_conn.get_latency().await;
                                
                                db_tabs_clone.statusbar.update_connection(Some(&db_name_clone), state);
                                db_tabs_clone.statusbar.update_version(version.as_deref());
                                db_tabs_clone.statusbar.update_latency(latency.map(|d| d.as_millis() as u64));
                            }
                            
                            // Start keep-alive
                            conn_mgr.start_keepalive(db_name_clone.clone(), 5);
                            
                            // Setup periodic latency updates
                            let db_conn_ping = db_conn.clone();
                            let statusbar_ping = db_tabs_clone.statusbar.clone();
                            let page_ping = page_clone.clone();
                            let tab_view_ping = db_tabs_clone.tab_view.clone();
                            
                            gtk4::glib::spawn_future_local(async move {
                                let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
                                loop {
                                    interval.tick().await;
                                    
                                    // Update latency if this tab is selected
                                    if tab_view_ping.selected_page().as_ref() == Some(&page_ping) {
                                        if let Some(latency) = db_conn_ping.get_latency().await {
                                            statusbar_ping.update_latency(Some(latency.as_millis() as u64));
                                        }
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("Failed to connect to {}: {}", db_name_clone, e);
                            // Update tab icon to error
                            let icon_error = gtk4::gio::ThemedIcon::new("dialog-error-symbolic");
                            page_clone.set_icon(Some(&icon_error));
                        }
                    }
                });
            }
        }
    }
    
    fn create_database_tab(&self, db_name: &str) -> (TabPage, QueryTabManager) {
        // Create query tab manager for this database
        let query_manager = QueryTabManager::new(db_name);
        let content = query_manager.get_widget().clone();
        
        // Add page to TabView
        let page = self.tab_view.append(&content);
        
        // Configure the tab
        page.set_title(db_name);
        // Start with connecting icon (will be updated when connection completes)
        page.set_icon(Some(&gtk4::gio::ThemedIcon::new("emblem-synchronizing-symbolic")));
        
        // Make tab closeable with animation
        self.tab_view.set_page_pinned(&page, false);
        
        // Open first query tab
        query_manager.new_query_tab();
        
        // Select the newly created tab
        self.tab_view.set_selected_page(&page);
        
        (page, query_manager)
    }
    
    pub fn toggle_current_sidebar(&self) {
        if let Some(current_page) = self.tab_view.selected_page() {
            let tabs = self.tabs.borrow();
            // Find the QueryTabManager for the current page
            for (_db_name, (page, query_manager, _)) in tabs.iter() {
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
            for (_db_name, (page, query_manager, _)) in tabs.iter() {
                if page == &current_page {
                    return query_manager.is_sidebar_visible();
                }
            }
        }
        false
    }
    
    pub fn new_query_in_current_db(&self) {
        if let Some(current_page) = self.tab_view.selected_page() {
            let tabs = self.tabs.borrow();
            for (_db_name, (page, query_manager, _)) in tabs.iter() {
                if page == &current_page {
                    query_manager.new_query_tab();
                    break;
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
        let conn_manager = connections::create_connection_manager(move |db_name| {
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

