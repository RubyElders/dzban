use gtk4::prelude::*;
use gtk4::{Box, Orientation, Paned, ScrolledWindow, TextView};
use libadwaita as adw;
use adw::{TabView, TabBar, OverlaySplitView};
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashMap;

use super::sidebar::create_query_sidebar;

#[derive(Clone)]
pub struct QueryTabManager {
    _db_name: String,
    tab_view: TabView,
    container: OverlaySplitView,
    query_counter: Rc<RefCell<u32>>,
    editors: Rc<RefCell<HashMap<u32, TextView>>>, // Map page ID to TextView
}

impl QueryTabManager {
    pub fn new(db_name: &str) -> Self {
        // Use OverlaySplitView for adaptive sidebar (v1_4+)
        let split_view = OverlaySplitView::new();
        split_view.set_vexpand(true);
        split_view.set_show_sidebar(true);
        split_view.set_pin_sidebar(true); // Pin sidebar to prevent auto-hide on narrow width
        split_view.set_enable_show_gesture(true);
        split_view.set_enable_hide_gesture(true);
        split_view.set_min_sidebar_width(250.0);
        split_view.set_max_sidebar_width(450.0);
        split_view.set_sidebar_width_fraction(0.20);
        
        // Create sidebar with query history
        let sidebar = create_query_sidebar(db_name);
        split_view.set_sidebar(Some(&sidebar));
        
        // Create main content area with query tabs
        let content_box = Box::new(Orientation::Vertical, 0);
        content_box.set_vexpand(true);
        
        // Create TabView for query tabs
        let tab_view = TabView::new();
        tab_view.set_vexpand(true);
        tab_view.set_shortcuts(adw::TabViewShortcuts::all());
        
        // Create TabBar
        let tab_bar = TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);
        
        content_box.append(&tab_bar);
        content_box.append(&tab_view);
        
        split_view.set_content(Some(&content_box));
        
        Self {
            _db_name: db_name.to_string(),
            tab_view,
            container: split_view,
            query_counter: Rc::new(RefCell::new(0)),
            editors: Rc::new(RefCell::new(HashMap::new())),
        }
    }
    
    pub fn get_widget(&self) -> &OverlaySplitView {
        &self.container
    }
    
    pub fn new_query_tab(&self) {
        let mut counter = self.query_counter.borrow_mut();
        *counter += 1;
        let query_num = *counter;
        
        // Create query editor with results pane
        let (content, editor) = create_query_editor_pane(query_num);
        
        // Add to TabView
        let page = self.tab_view.append(&content);
        page.set_title(&format!("Query {}", query_num));
        page.set_icon(Some(&gtk4::gio::ThemedIcon::new("text-editor-symbolic")));
        
        // Store the editor reference
        self.editors.borrow_mut().insert(query_num, editor);
        
        // Select the new tab
        self.tab_view.set_selected_page(&page);
        
        // Focus the editor
        self.focus_current_editor();
    }
    
    pub fn focus_current_editor(&self) {
        if let Some(page) = self.tab_view.selected_page() {
            // Find the editor for this page
            let editors = self.editors.borrow();
            
            // Get page title to extract query number
            let title = page.title();
            if let Some(num_str) = title.strip_prefix("Query ") {
                if let Ok(query_num) = num_str.parse::<u32>() {
                    if let Some(editor) = editors.get(&query_num) {
                        editor.grab_focus();
                    }
                }
            }
        }
    }
    
    pub fn toggle_sidebar(&self) {
        let is_showing = self.container.shows_sidebar();
        self.container.set_show_sidebar(!is_showing);
    }
    
    pub fn is_sidebar_visible(&self) -> bool {
        self.container.shows_sidebar()
    }
}

fn create_query_editor_pane(query_num: u32) -> (Box, TextView) {
    let container = Box::new(Orientation::Vertical, 0);
    container.set_vexpand(true);
    
    // Vertical paned: SQL Editor (top) | Results (bottom)
    let paned = Paned::new(Orientation::Vertical);
    paned.set_vexpand(true);
    paned.set_position(400);
    paned.set_wide_handle(true);
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(false);
    
    // SQL Editor
    let editor_scroll = ScrolledWindow::new();
    editor_scroll.set_vexpand(true);
    editor_scroll.set_hexpand(true);
    
    let sql_editor = TextView::new();
    sql_editor.set_monospace(true);
    sql_editor.set_left_margin(16);
    sql_editor.set_right_margin(16);
    sql_editor.set_top_margin(16);
    sql_editor.set_bottom_margin(16);
    sql_editor.set_wrap_mode(gtk4::WrapMode::Word);
    
    // Sample query
    let buffer = sql_editor.buffer();
    buffer.set_text(&get_sample_query(query_num));
    
    editor_scroll.set_child(Some(&sql_editor));
    paned.set_start_child(Some(&editor_scroll));
    
    // Results area (will use ColumnView for modern table in next iteration)
    let results_scroll = ScrolledWindow::new();
    results_scroll.set_vexpand(true);
    results_scroll.set_hexpand(true);
    
    let results_placeholder = create_results_placeholder();
    results_scroll.set_child(Some(&results_placeholder));
    
    paned.set_end_child(Some(&results_scroll));
    
    container.append(&paned);
    
    (container, sql_editor)
}

fn create_results_placeholder() -> adw::StatusPage {
    let status_page = adw::StatusPage::new();
    status_page.set_icon_name(Some("system-run-symbolic"));
    status_page.set_title("No Results");
    status_page.set_description(Some("Run a query to see results here"));
    status_page
}

fn get_sample_query(query_num: u32) -> String {
    match query_num {
        1 => "-- Query 1: User Statistics
SELECT 
    users.id,
    users.name,
    users.email,
    COUNT(orders.id) as order_count,
    SUM(orders.total) as total_spent
FROM users
LEFT JOIN orders ON users.id = orders.user_id
WHERE users.created_at > '2024-01-01'
GROUP BY users.id, users.name, users.email
ORDER BY total_spent DESC
LIMIT 100;".to_string(),
        2 => "-- Query 2: Recent Orders
SELECT * 
FROM orders 
WHERE created_at > CURRENT_DATE - INTERVAL '7 days'
ORDER BY created_at DESC;".to_string(),
        _ => "-- New Query\n\n".to_string(),
    }
}
