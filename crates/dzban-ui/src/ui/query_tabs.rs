use gtk4::prelude::*;
use gtk4::{Box, Orientation, Paned, ScrolledWindow, ColumnView, ColumnViewColumn, SignalListItemFactory, Label, ListItem, SingleSelection, StringList, EventControllerKey, Revealer, Overlay, gdk};
use sourceview5::{View as SourceView, Buffer as SourceBuffer, LanguageManager, StyleSchemeManager};
use sourceview5::prelude::*;
use libadwaita as adw;
use adw::{TabView, TabBar, OverlaySplitView};
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashMap;

use super::sidebar::create_query_sidebar;
use super::query_state::QueryStateManager;
use super::constants::{EXPLAIN_DELAY_MS, EXPLAIN_ANIMATION_MS, EXPLAIN_TOP_MARGIN};

// Stores UI elements for each query tab
struct QueryTab {
    editor: SourceView,
    results_scroll: ScrolledWindow,
    state_manager: QueryStateManager,
}

type ExplainCallback = Rc<dyn Fn(String)>;
type ActionBarCallback = Rc<dyn Fn(&str, bool)>;

#[derive(Clone)]
pub struct QueryTabManager {
    _db_name: String,
    tab_view: TabView,
    container: OverlaySplitView,
    query_counter: Rc<RefCell<u32>>,
    tabs: Rc<RefCell<HashMap<u32, QueryTab>>>, // Map query number to tab data
    explain_callback: Rc<RefCell<Option<ExplainCallback>>>,
    explain_revealer: Revealer, // Single revealer for all tabs, positioned at top
    action_bar_callback: Rc<RefCell<Option<ActionBarCallback>>>,
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
        
        // Create main content area with overlay for floating revealer
        let content_overlay = Overlay::new();
        content_overlay.set_vexpand(true);
        
        // Content box with TabBar and TabView (base layer)
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
        
        // Set content_box as the base child (bottom layer)
        content_overlay.set_child(Some(&content_box));
        
        // Create revealer for EXPLAIN info - floats on top as overlay
        let explain_revealer = Revealer::new();
        explain_revealer.set_transition_type(gtk4::RevealerTransitionType::SlideDown);
        explain_revealer.set_transition_duration(EXPLAIN_ANIMATION_MS);
        explain_revealer.set_reveal_child(false);
        explain_revealer.set_valign(gtk4::Align::Start);
        explain_revealer.set_halign(gtk4::Align::Fill);
        explain_revealer.set_margin_start(12);
        explain_revealer.set_margin_end(12);
        explain_revealer.set_margin_top(EXPLAIN_TOP_MARGIN);
        
        // Don't let the revealer receive input events when hidden
        explain_revealer.set_can_target(false);
        
        // Add revealer as overlay (top layer) - this floats on top without affecting layout
        content_overlay.add_overlay(&explain_revealer);
        
        split_view.set_content(Some(&content_overlay));
        
        Self {
            _db_name: db_name.to_string(),
            tab_view,
            container: split_view,
            query_counter: Rc::new(RefCell::new(0)),
            tabs: Rc::new(RefCell::new(HashMap::new())),
            explain_callback: Rc::new(RefCell::new(None)),
            explain_revealer,
            action_bar_callback: Rc::new(RefCell::new(None)),
        }
    }
    
    pub fn get_widget(&self) -> &OverlaySplitView {
        &self.container
    }
    
    /// Set callback for auto-EXPLAIN queries
    pub fn set_explain_callback<F>(&self, callback: F)
    where
        F: Fn(String) + 'static,
    {
        *self.explain_callback.borrow_mut() = Some(Rc::new(callback));
    }
    
    /// Set callback for action bar updates (text, show_progress)
    pub fn set_action_bar_callback<F>(&self, callback: F)
    where
        F: Fn(&str, bool) + 'static,
    {
        *self.action_bar_callback.borrow_mut() = Some(Rc::new(callback));
    }
    
    fn update_action_bar(&self, text: &str, show_progress: bool) {
        if let Some(callback) = self.action_bar_callback.borrow().as_ref() {
            callback(text, show_progress);
        }
    }
    
    pub fn new_query_tab(&self) {
        let mut counter = self.query_counter.borrow_mut();
        *counter += 1;
        let query_num = *counter;
        
        // Create query editor with results pane (no revealer per tab anymore)
        let (content, editor, results_scroll) = create_query_editor_pane(query_num);
        
        // Create state manager for this tab
        let state_manager = QueryStateManager::new();
        
        // Add to TabView
        let page = self.tab_view.append(&content);
        page.set_title(&format!("Query {}", query_num));
        page.set_icon(Some(&gtk4::gio::ThemedIcon::new("text-editor-symbolic")));
        
        // Store the tab data (without revealer)
        self.tabs.borrow_mut().insert(query_num, QueryTab {
            editor: editor.clone(),
            results_scroll,
            state_manager,
        });
        
        // Add keyboard event controller to highlight query on Ctrl press
        let key_controller = EventControllerKey::new();
        let manager_for_highlight = self.clone();
        
        // Store timer handle to cancel if Ctrl is released early
        let explain_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let explain_timer_for_press = explain_timer.clone();
        
        key_controller.connect_key_pressed(move |_controller, key, _code, _modifiers| {
            // Highlight when Ctrl key itself is pressed
            if key == gdk::Key::Control_L || key == gdk::Key::Control_R {
                manager_for_highlight.highlight_query_at_cursor();
                
                // Update action bar to show preparing EXPLAIN with progress
                manager_for_highlight.update_action_bar("Preparing EXPLAIN…", true);
                
                // Start 1-second timer for auto-EXPLAIN
                let manager_for_timer = manager_for_highlight.clone();
                let timer_handle_for_callback = explain_timer_for_press.clone();
                let timer_id = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(EXPLAIN_DELAY_MS),
                    move || {
                        // Clear the timer ID since it has fired
                        *timer_handle_for_callback.borrow_mut() = None;
                        manager_for_timer.run_auto_explain();
                    }
                );
                *explain_timer_for_press.borrow_mut() = Some(timer_id);
            }
            glib::Propagation::Proceed
        });
        
        let manager_for_clear = self.clone();
        let explain_timer_for_release = explain_timer.clone();
        key_controller.connect_key_released(move |_controller, key, _code, _modifiers| {
            // Clear highlight when Ctrl is released
            if key == gdk::Key::Control_L || key == gdk::Key::Control_R {
                manager_for_clear.clear_query_highlight();
                
                // Hide explain revealer when Ctrl is released
                manager_for_clear.hide_explain_results();
                
                // Reset action bar to default state
                manager_for_clear.update_action_bar("Search databases... (Ctrl+K)", false);
                
                // Cancel auto-EXPLAIN timer if Ctrl released early and timer still pending
                if let Some(timer_id) = explain_timer_for_release.borrow_mut().take() {
                    // Only try to remove if the timer hasn't fired yet
                    // If remove() returns false, the timer already executed
                    timer_id.remove();
                }
            }
        });
        
        editor.add_controller(key_controller);
        
        // Select the new tab
        self.tab_view.set_selected_page(&page);
        
        // Focus the editor
        self.focus_current_editor();
    }
    
    pub fn focus_current_editor(&self) {
        if let Some(query_num) = self.current_query_id()
            && let Some(tab) = self.tabs.borrow().get(&query_num)
        {
            tab.editor.grab_focus();
        }
    }

    fn current_query_id(&self) -> Option<u32> {
        self.tab_view
            .selected_page()?
            .title()
            .strip_prefix("Query ")?
            .parse()
            .ok()
    }

    pub fn current_query(&self) -> Option<(u32, String, QueryStateManager)> {
        let query_num = self.current_query_id()?;
        let tabs = self.tabs.borrow();
        let tab = tabs.get(&query_num)?;
        let buffer = tab.editor.buffer();
        let (cursor_iter, _) = buffer.selection_bounds().unwrap_or_else(|| {
            let iter = buffer.iter_at_offset(buffer.cursor_position());
            (iter, iter)
        });
        let full_text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let query = extract_query_at_cursor(&full_text, cursor_iter.offset() as usize)?.0;
        Some((query_num, query, tab.state_manager.clone()))
    }
    
    /// Get the SQL query text from the currently selected tab
    /// Returns the query at cursor position (split by semicolons)
    pub fn get_current_query_text(&self) -> Option<String> {
        self.current_query().map(|(_, query, _)| query)
    }
    
    /// Highlight the query that will be executed at the current cursor position
    pub fn highlight_query_at_cursor(&self) {
        if let Some(query_num) = self.current_query_id() {
            let tabs = self.tabs.borrow();
            if let Some(tab) = tabs.get(&query_num) {
                        let buffer = tab.editor.buffer();
                        
                        // Get cursor position
                        let (cursor_iter, _) = buffer.selection_bounds()
                            .unwrap_or_else(|| {
                                let iter = buffer.iter_at_offset(buffer.cursor_position());
                                (iter, iter)
                            });
                        let cursor_offset = cursor_iter.offset();
                        
                        // Get full text
                        let start = buffer.start_iter();
                        let end = buffer.end_iter();
                        let full_text = buffer.text(&start, &end, false).to_string();
                        
                        // Extract query at cursor position with range
                        if let Some((_query, start_offset, end_offset)) = 
                            extract_query_at_cursor(&full_text, cursor_offset as usize) {
                            
                            // Create or get the highlight tag
                            let tag_table = buffer.tag_table();
                            let tag = if let Some(existing_tag) = tag_table.lookup("query-highlight") {
                                existing_tag
                            } else {
                                let new_tag = gtk4::TextTag::new(Some("query-highlight"));
                                // Use a subtle background color for highlighting
                                new_tag.set_background(Some("rgba(100, 150, 255, 0.15)"));
                                tag_table.add(&new_tag);
                                new_tag
                            };
                            
                            // Remove any existing highlights first
                            let buffer_start = buffer.start_iter();
                            let buffer_end = buffer.end_iter();
                            buffer.remove_tag(&tag, &buffer_start, &buffer_end);
                            
                            // Apply highlight to the query range
                            let query_start = buffer.iter_at_offset(start_offset as i32);
                            let query_end = buffer.iter_at_offset(end_offset as i32);
                            buffer.apply_tag(&tag, &query_start, &query_end);
                        }
            }
        }
    }
    
    /// Clear the query highlight
    pub fn clear_query_highlight(&self) {
        if let Some(query_num) = self.current_query_id() {
            let tabs = self.tabs.borrow();
            if let Some(tab) = tabs.get(&query_num) {
                        let buffer = tab.editor.buffer();
                        
                        // Remove highlight tag from entire buffer
                        let tag_table = buffer.tag_table();
                        if let Some(tag) = tag_table.lookup("query-highlight") {
                            let start = buffer.start_iter();
                            let end = buffer.end_iter();
                            buffer.remove_tag(&tag, &start, &end);
                        }
            }
        }
    }
    
    /// Run auto-EXPLAIN after holding Ctrl for 2 seconds
    fn run_auto_explain(&self) {
        if let Some(query_text) = self.get_current_query_text() {
            // Check if query already starts with EXPLAIN
            let trimmed = query_text.trim();
            if trimmed.to_uppercase().starts_with("EXPLAIN") {
                tracing::info!("Query already contains EXPLAIN, skipping auto-EXPLAIN");
                return;
            }
            
            // Prepend EXPLAIN (FORMAT JSON)
            let explain_query = format!("EXPLAIN (FORMAT JSON) {}", query_text);
            
            tracing::info!("🔍 Auto-EXPLAIN triggered for query: {}", query_text.lines().next().unwrap_or(""));
            
            // Call the explain callback if set
            if let Some(callback) = self.explain_callback.borrow().as_ref() {
                callback(explain_query);
            } else {
                tracing::warn!("Auto-EXPLAIN callback not set");
            }
        }
    }
    
    /// Display query results in a grid (GTK4 ColumnView)
    pub fn display_results_for(&self, query_num: u32, rows: Vec<tokio_postgres::Row>) {
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            tab.state_manager.finish();
            let results_widget = create_results_grid(rows);
            tab.results_scroll.set_child(Some(&results_widget));
        }
    }
    
    /// Show error message in results area
    pub fn display_error_for(&self, query_num: u32, error: &str) {
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            tab.state_manager.finish();
            let error_page = adw::StatusPage::new();
            error_page.set_icon_name(Some("dialog-error-symbolic"));
            error_page.set_title("Query Failed");
            error_page.set_description(Some(error));
            tab.results_scroll.set_child(Some(&error_page));
        }
    }
    
    /// Show loading state in results area
    pub fn show_loading_for(&self, query_num: u32) {
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            let loading_page = adw::StatusPage::new();
            loading_page.set_icon_name(Some("content-loading-symbolic"));
            loading_page.set_title("Running Query...");
            loading_page.set_description(Some("Please wait"));
            let spinner = gtk4::Spinner::new();
            spinner.set_spinning(true);
            spinner.set_size_request(48, 48);
            loading_page.set_child(Some(&spinner));
            tab.results_scroll.set_child(Some(&loading_page));
        }
    }
    
    pub fn toggle_sidebar(&self) {
        let is_showing = self.container.shows_sidebar();
        self.container.set_show_sidebar(!is_showing);
    }
    
    pub fn is_sidebar_visible(&self) -> bool {
        self.container.shows_sidebar()
    }
    
    /// Display EXPLAIN results in the fancy top slider
    pub fn show_explain_results(&self, json_value: &serde_json::Value) {
        // Extract EXPLAIN stats
        let (rows_estimate, first_cost, last_cost) = extract_explain_stats(json_value);
        
        // Create info box with stats
        let info_box = Box::new(Orientation::Horizontal, 12);
        info_box.set_margin_start(12);
        info_box.set_margin_end(12);
        info_box.set_margin_top(8);
        info_box.set_margin_bottom(8);
        info_box.add_css_class("card");
        info_box.add_css_class("toolbar");
        
        let explain_icon = Label::new(Some("🔍"));
        explain_icon.add_css_class("title-3");
        info_box.append(&explain_icon);
        
        let title_label = Label::new(Some("Query Plan"));
        title_label.add_css_class("title-4");
        title_label.set_halign(gtk4::Align::Start);
        info_box.append(&title_label);
        
        // Add spacer
        let spacer = Box::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        info_box.append(&spacer);
        
        // Stats
        let stats_box = Box::new(Orientation::Horizontal, 20);
        
        let rows_label = Label::new(Some(&format!("📊 ~{} rows", rows_estimate)));
        rows_label.add_css_class("dim-label");
        stats_box.append(&rows_label);
        
        let cost_label = Label::new(Some(&format!("💰 Cost: {:.2}..{:.2}", first_cost, last_cost)));
        cost_label.add_css_class("dim-label");
        stats_box.append(&cost_label);
        
        info_box.append(&stats_box);
        
        // Set the info box as the revealer's child
        self.explain_revealer.set_child(Some(&info_box));
        
        // Reveal with animation
        self.explain_revealer.set_reveal_child(true);
    }
    
    /// Hide EXPLAIN results slider
    pub fn hide_explain_results(&self) {
        self.explain_revealer.set_reveal_child(false);
    }
}

/// Extract key statistics from EXPLAIN JSON output
fn extract_explain_stats(json_value: &serde_json::Value) -> (i64, f64, f64) {
    // Default values
    let mut rows_estimate = 0i64;
    let mut first_cost = 0.0f64;
    let mut last_cost = 0.0f64;
    
    // EXPLAIN (FORMAT JSON) returns an array with one element containing the plan
    if let Some(plan_array) = json_value.as_array()
        && let Some(plan_obj) = plan_array.first()
        && let Some(plan) = plan_obj.get("Plan")
    {
                // Get row estimate
                if let Some(rows) = plan.get("Plan Rows") {
                    rows_estimate = rows.as_i64().unwrap_or(0);
                }
                
                // Get startup cost (first cost)
                if let Some(cost) = plan.get("Startup Cost") {
                    first_cost = cost.as_f64().unwrap_or(0.0);
                }
                
                // Get total cost (last cost)
                if let Some(cost) = plan.get("Total Cost") {
                    last_cost = cost.as_f64().unwrap_or(0.0);
                }
    }
    
    (rows_estimate, first_cost, last_cost)
}

/// Extract the SQL query at the cursor position
/// Splits by semicolons and returns the query containing the cursor
/// Returns (query_text, start_offset, end_offset)
fn extract_query_at_cursor(text: &str, cursor_pos: usize) -> Option<(String, usize, usize)> {
    // If cursor is beyond text, return None
    if cursor_pos > text.len() {
        return None;
    }
    
    // Split by semicolons and track positions
    let mut queries = Vec::new();
    let mut current_pos = 0;
    
    for part in text.split(';') {
        let part_len = part.len();
        let query_start = current_pos;
        let query_end = current_pos + part_len;
        
        queries.push((query_start, query_end, part));
        current_pos = query_end + 1; // +1 for the semicolon
    }
    
    // Strategy: Prefer the query before the cursor when at boundaries
    // 1. First, check if cursor is within a query's content (not just whitespace)
    for (start, _end, query_text) in &queries {
        let trimmed = query_text.trim();
        if trimmed.is_empty() {
            continue;
        }
        
        // Find actual content boundaries (skip leading/trailing whitespace)
        let trim_start = query_text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        let trim_end = query_text.rfind(|c: char| !c.is_whitespace())
            .map(|pos| pos + 1)
            .unwrap_or(query_text.len());
        
        let content_start = start + trim_start;
        let content_end = start + trim_end;
        
        // If cursor is within the actual content, this is our query
        if cursor_pos >= content_start && cursor_pos < content_end {
            return Some((
                trimmed.to_string(),
                content_start,
                content_end,
            ));
        }
    }
    
    // 2. Cursor is not within content - prefer the query before cursor
    for (start, _end, query_text) in queries.iter().rev() {
        let trimmed = query_text.trim();
        if trimmed.is_empty() {
            continue;
        }
        
        let trim_start = query_text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        let trim_end = query_text.rfind(|c: char| !c.is_whitespace())
            .map(|pos| pos + 1)
            .unwrap_or(query_text.len());
        
        let content_start = start + trim_start;
        let content_end = start + trim_end;
        
        // Use the last query whose content ends before or at cursor
        if content_end <= cursor_pos {
            return Some((
                trimmed.to_string(),
                content_start,
                content_end,
            ));
        }
    }
    
    // 3. Still no match? Return the first non-empty query
    for (start, _end, query_text) in &queries {
        let trimmed = query_text.trim();
        if !trimmed.is_empty() {
            let trim_start = query_text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
            let trim_end = query_text.rfind(|c: char| !c.is_whitespace())
                .map(|pos| pos + 1)
                .unwrap_or(query_text.len());
            
            return Some((
                trimmed.to_string(),
                start + trim_start,
                start + trim_end,
            ));
        }
    }
    
    None
}

fn create_query_editor_pane(query_num: u32) -> (Box, SourceView, ScrolledWindow) {
    let container = Box::new(Orientation::Vertical, 0);
    container.set_vexpand(true);
    
    // Vertical paned: SQL Editor (top) | Results (bottom)
    let paned = Paned::new(Orientation::Vertical);
    paned.set_vexpand(true);
    paned.set_position(400);
    paned.set_wide_handle(true);
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(false);
    
    // SQL Editor with syntax highlighting
    let editor_scroll = ScrolledWindow::new();
    editor_scroll.set_vexpand(true);
    editor_scroll.set_hexpand(true);
    
    // Create SourceView for SQL
    let sql_editor = SourceView::new();
    
    // Get SQL language definition
    let lang_manager = LanguageManager::default();
    if let Some(sql_lang) = lang_manager.language("sql") {
        let buffer = SourceBuffer::with_language(&sql_lang);
        
        // Apply color scheme based on system theme preference
        let scheme_manager = StyleSchemeManager::default();
        
        // Get the current style manager to detect dark mode
        let style_manager = adw::StyleManager::default();
        let is_dark = style_manager.is_dark();
        
        let scheme_name = if is_dark { "Adwaita-dark" } else { "Adwaita" };
        if let Some(scheme) = scheme_manager.scheme(scheme_name) {
            buffer.set_style_scheme(Some(&scheme));
        }
        
        // Watch for theme changes and update scheme
        let buffer_for_theme = buffer.clone();
        style_manager.connect_dark_notify(move |style_manager| {
            let is_dark = style_manager.is_dark();
            let scheme_name = if is_dark { "Adwaita-dark" } else { "Adwaita" };
            let scheme_manager = StyleSchemeManager::default();
            if let Some(scheme) = scheme_manager.scheme(scheme_name) {
                buffer_for_theme.set_style_scheme(Some(&scheme));
            }
        });
        
        // Set sample query
        buffer.set_text(&get_sample_query(query_num));
        
        sql_editor.set_buffer(Some(&buffer));
    }
    
    // Configure editor
    sql_editor.set_monospace(true);
    sql_editor.set_left_margin(16);
    sql_editor.set_right_margin(16);
    sql_editor.set_top_margin(16);
    sql_editor.set_bottom_margin(16);
    sql_editor.set_wrap_mode(gtk4::WrapMode::Word);
    sql_editor.set_show_line_numbers(true);
    sql_editor.set_highlight_current_line(true);
    sql_editor.set_tab_width(4);
    sql_editor.set_insert_spaces_instead_of_tabs(true);
    sql_editor.set_auto_indent(true);
    
    editor_scroll.set_child(Some(&sql_editor));
    paned.set_start_child(Some(&editor_scroll));
    
    // Results area
    let results_scroll = ScrolledWindow::new();
    results_scroll.set_vexpand(true);
    results_scroll.set_hexpand(true);
    
    let results_placeholder = create_results_placeholder();
    results_scroll.set_child(Some(&results_placeholder));
    
    paned.set_end_child(Some(&results_scroll));
    
    container.append(&paned);
    
    (container, sql_editor, results_scroll)
}

fn create_results_placeholder() -> adw::StatusPage {
    let status_page = adw::StatusPage::new();
    status_page.set_icon_name(Some("system-run-symbolic"));
    status_page.set_title("No Results");
    status_page.set_description(Some("Run a query to see results here"));
    status_page
}

/// Create a ColumnView grid to display PostgreSQL query results
fn create_results_grid(rows: Vec<tokio_postgres::Row>) -> Box {
    let container = Box::new(Orientation::Vertical, 0);
    
    if rows.is_empty() {
        let empty_page = adw::StatusPage::new();
        empty_page.set_icon_name(Some("emblem-ok-symbolic"));
        empty_page.set_title("Query Successful");
        empty_page.set_description(Some("0 rows returned"));
        container.append(&empty_page);
        return container;
    }
    
    // Get column names from first row
    let columns = rows[0].columns();
    
    // Convert rows to Vec<Vec<String>> for display
    let mut data: Vec<Vec<String>> = Vec::new();
    for row in &rows {
        let mut row_data = Vec::new();
        for (i, column) in columns.iter().enumerate() {
            let display_value = if let Ok(Some(s)) = row.try_get::<_, Option<String>>(i) {
                s
            } else if let Ok(None) = row.try_get::<_, Option<String>>(i) {
                "NULL".to_string()
            } else if let Ok(v) = row.try_get::<_, Option<i32>>(i) {
                v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string())
            } else if let Ok(v) = row.try_get::<_, Option<i64>>(i) {
                v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string())
            } else if let Ok(v) = row.try_get::<_, Option<f64>>(i) {
                v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string())
            } else if let Ok(v) = row.try_get::<_, Option<bool>>(i) {
                v.map(|b| b.to_string()).unwrap_or_else(|| "NULL".to_string())
            } else if let Ok(v) = row.try_get::<_, Option<serde_json::Value>>(i) {
                // Handle JSON - pretty print it
                v.map(|json| serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string()))
                    .unwrap_or_else(|| "NULL".to_string())
            } else {
                // Fallback for unknown types
                format!("<{}>", column.type_().name())
            };
            row_data.push(display_value);
        }
        data.push(row_data);
    }
    
    // Create model with all data as strings (simple approach)
    let string_list = StringList::new(&[]);
    for row_data in &data {
        string_list.append(&row_data.join("\t"));
    }
    
    let selection_model = SingleSelection::new(Some(string_list.clone()));
    
    // Create ColumnView
    let column_view = ColumnView::new(Some(selection_model));
    column_view.set_show_column_separators(true);
    column_view.set_show_row_separators(true);
    
    let selection_model = SingleSelection::new(Some(string_list));
    column_view.set_model(Some(&selection_model));
    
    // Add columns
    for (col_idx, column) in columns.iter().enumerate() {
        let factory = SignalListItemFactory::new();
        
        factory.connect_setup(move |_, list_item| {
            let label = Label::new(None);
            label.set_xalign(0.0);
            label.set_margin_start(8);
            label.set_margin_end(8);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            list_item.downcast_ref::<ListItem>().unwrap().set_child(Some(&label));
        });
        
        factory.connect_bind(move |_, list_item| {
            let item = list_item.downcast_ref::<ListItem>().unwrap();
            if let Some(string_obj) = item.item().and_then(|o| o.downcast::<gtk4::StringObject>().ok()) {
                let full_text = string_obj.string().to_string();
                let parts: Vec<&str> = full_text.split('\t').collect();
                if let Some(cell_text) = parts.get(col_idx)
                    && let Some(label) = item.child().and_then(|c| c.downcast::<Label>().ok())
                {
                    label.set_text(cell_text);
                }
            }
        });
        
        let col = ColumnViewColumn::new(Some(column.name()), Some(factory));
        col.set_resizable(true);
        col.set_expand(true);
        column_view.append_column(&col);
    }
    
    // Add header with row count
    let header = Box::new(Orientation::Horizontal, 12);
    header.set_margin_start(12);
    header.set_margin_end(12);
    header.set_margin_top(6);
    header.set_margin_bottom(6);
    header.add_css_class("toolbar");
    
    let row_count_label = Label::new(Some(&format!("📊 {} rows returned", rows.len())));
    row_count_label.add_css_class("dim-label");
    row_count_label.add_css_class("caption");
    header.append(&row_count_label);
    
    container.append(&header);
    container.append(&column_view);
    
    container
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

#[cfg(test)]
mod tests {
    use super::*;

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn completion_is_applied_to_the_query_that_started_it() {
        let manager = QueryTabManager::new("test");
        manager.new_query_tab();
        let first_state = manager.tabs.borrow().get(&1).unwrap().state_manager.clone();
        assert!(first_state.try_start());

        manager.new_query_tab();
        assert_eq!(manager.current_query_id(), Some(2));
        manager.display_error_for(1, "failed");

        assert!(first_state.try_start());
        assert_eq!(manager.current_query_id(), Some(2));
    }
}
