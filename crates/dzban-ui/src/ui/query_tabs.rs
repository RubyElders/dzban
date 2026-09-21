use adw::{OverlaySplitView, TabBar, TabPage, TabView};
use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{
    Box, ColumnView, ColumnViewColumn, Label, ListItem, Orientation, Paned, ScrolledWindow,
    SignalListItemFactory, SingleSelection, StringList,
};
use libadwaita as adw;
use sourceview5::prelude::*;
use sourceview5::{
    Buffer as SourceBuffer, LanguageManager, StyleSchemeManager, View as SourceView,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use super::query_history::{QueryHistoryEntry, QueryHistoryOutcome, QueryResultSnapshot};
use super::query_state::QueryStateManager;
use super::sidebar::QueryHistory;
use dzban_db::Row;

// Stores UI elements for each query tab
struct QueryTab {
    page: TabPage,
    editor: SourceView,
    results_scroll: ScrolledWindow,
    state_manager: QueryStateManager,
    output_generation: Cell<u64>,
}

#[derive(Clone)]
pub struct QueryTabManager {
    _db_name: String,
    tab_view: TabView,
    container: OverlaySplitView,
    query_counter: Rc<RefCell<u32>>,
    tabs: Rc<RefCell<HashMap<u32, QueryTab>>>, // Map query number to tab data
    history: QueryHistory,
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

        let history = QueryHistory::new(db_name);
        split_view.set_sidebar(Some(history.widget()));

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

        let manager = Self {
            _db_name: db_name.to_string(),
            tab_view,
            container: split_view,
            query_counter: Rc::new(RefCell::new(0)),
            tabs: Rc::new(RefCell::new(HashMap::new())),
            history,
        };

        let weak_tab_view = manager.tab_view.downgrade();
        let weak_tabs = Rc::downgrade(&manager.tabs);
        manager.history.set_open_callback(move |entry| {
            let Some(tab_view) = weak_tab_view.upgrade() else {
                return;
            };
            let Some(selected_page) = tab_view.selected_page() else {
                return;
            };
            let Some(tabs) = weak_tabs.upgrade() else {
                return;
            };
            let tabs = tabs.borrow();
            let Some(tab) = tabs.values().find(|tab| tab.page == selected_page) else {
                return;
            };
            tab.output_generation
                .set(tab.output_generation.get().wrapping_add(1));
            tab.editor.buffer().set_text(&entry.sql);
            match &entry.outcome {
                QueryHistoryOutcome::Success {
                    result,
                    explain,
                    explain_error,
                    ..
                } => {
                    tab.results_scroll.set_child(Some(&create_query_output(
                        result.as_ref(),
                        explain.as_ref(),
                        explain_error.as_deref(),
                    )));
                }
                QueryHistoryOutcome::Failure { error } => {
                    tab.results_scroll
                        .set_child(Some(&create_error_page(error)));
                }
            }
            tab.editor.grab_focus();
        });

        let weak_tabs = Rc::downgrade(&manager.tabs);
        manager.tab_view.connect_close_page(move |_view, page| {
            if let Some(tabs) = weak_tabs.upgrade() {
                let query_num = tabs
                    .borrow()
                    .iter()
                    .find(|(_, tab)| tab.page == *page)
                    .map(|(query_num, _)| *query_num);
                if let Some(query_num) = query_num {
                    tabs.borrow_mut().remove(&query_num);
                }
            }
            glib::Propagation::Proceed
        });

        manager
    }

    pub fn get_widget(&self) -> &OverlaySplitView {
        &self.container
    }

    pub fn new_query_tab(&self) {
        let mut counter = self.query_counter.borrow_mut();
        *counter += 1;
        let query_num = *counter;

        let (content, editor, results_scroll) = create_query_editor_pane(query_num);

        // Create state manager for this tab
        let state_manager = QueryStateManager::new();

        // Add to TabView
        let page = self.tab_view.append(&content);
        page.set_title(&format!("Query {}", query_num));
        page.set_icon(Some(&gtk4::gio::ThemedIcon::new("text-editor-symbolic")));

        // Store the tab data (without revealer)
        self.tabs.borrow_mut().insert(
            query_num,
            QueryTab {
                page: page.clone(),
                editor: editor.clone(),
                results_scroll,
                state_manager,
                output_generation: Cell::new(0),
            },
        );

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
        let selected_page = self.tab_view.selected_page()?;
        self.tabs
            .borrow()
            .iter()
            .find(|(_, tab)| tab.page == selected_page)
            .map(|(query_num, _)| *query_num)
    }

    pub fn is_query_selected(&self, query_num: u32) -> bool {
        self.current_query_id() == Some(query_num)
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

    pub fn highlight_current_query(&self) -> Option<String> {
        let query_num = self.current_query_id()?;
        let tabs = self.tabs.borrow();
        let tab = tabs.get(&query_num)?;
        if !tab.editor.has_focus() {
            return None;
        }

        let buffer = tab.editor.buffer();
        let (cursor_iter, _) = buffer.selection_bounds().unwrap_or_else(|| {
            let iter = buffer.iter_at_offset(buffer.cursor_position());
            (iter, iter)
        });
        let full_text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let (query, start_offset, end_offset) =
            extract_query_at_cursor(&full_text, cursor_iter.offset() as usize)?;

        let tag_table = buffer.tag_table();
        let tag = tag_table.lookup("query-highlight").unwrap_or_else(|| {
            let tag = gtk4::TextTag::new(Some("query-highlight"));
            tag.set_background_rgba(Some(&gdk::RGBA::new(0.39, 0.59, 1.0, 0.18)));
            tag_table.add(&tag);
            tag
        });
        buffer.remove_tag(&tag, &buffer.start_iter(), &buffer.end_iter());
        buffer.apply_tag(
            &tag,
            &buffer.iter_at_offset(start_offset as i32),
            &buffer.iter_at_offset(end_offset as i32),
        );

        Some(query)
    }

    pub fn clear_query_highlight(&self) {
        let tabs = self.tabs.borrow();
        for tab in tabs.values() {
            let buffer = tab.editor.buffer();
            if let Some(tag) = buffer.tag_table().lookup("query-highlight") {
                buffer.remove_tag(&tag, &buffer.start_iter(), &buffer.end_iter());
            }
        }
    }

    pub fn display_results_for(&self, query_num: u32, rows: Vec<Row>) -> QueryResultSnapshot {
        let result = result_snapshot(&rows);
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            let results_widget = create_results_grid(&result);
            tab.results_scroll.set_child(Some(&results_widget));
        }
        bounded_result_snapshot(&result)
    }

    pub fn display_explain_for(
        &self,
        query_num: u32,
        output_generation: u64,
        explain: Option<&serde_json::Value>,
        explain_error: Option<&str>,
    ) {
        let Some(results_scroll) = self
            .tabs
            .borrow()
            .get(&query_num)
            .filter(|tab| tab.output_generation.get() == output_generation)
            .map(|tab| tab.results_scroll.clone())
        else {
            return;
        };
        let Some(content) = results_scroll
            .child()
            .and_then(|child| child.downcast::<Box>().ok())
        else {
            return;
        };
        append_explain(&content, explain, explain_error);
    }

    /// Show error message in results area
    pub fn display_error_for(&self, query_num: u32, error: &str) {
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            tab.results_scroll
                .set_child(Some(&create_error_page(error)));
        }
    }

    pub fn record_query_success(
        &self,
        sql: String,
        duration: Duration,
        rows: usize,
        result: QueryResultSnapshot,
        explain: Option<serde_json::Value>,
        explain_error: Option<String>,
    ) {
        self.history.add(QueryHistoryEntry::success(
            sql,
            duration,
            rows,
            result,
            explain,
            explain_error,
        ));
    }

    pub fn record_query_failure(&self, sql: String, error: String) {
        self.history.add(QueryHistoryEntry::failure(sql, error));
    }

    /// Show loading state in results area
    pub fn show_loading_for(&self, query_num: u32) -> Option<u64> {
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            let output_generation = tab.output_generation.get().wrapping_add(1);
            tab.output_generation.set(output_generation);
            let loading_page = adw::StatusPage::new();
            loading_page.set_icon_name(Some("content-loading-symbolic"));
            loading_page.set_title("Running Query...");
            loading_page.set_description(Some("Please wait"));
            let spinner = gtk4::Spinner::new();
            spinner.set_spinning(true);
            spinner.set_size_request(48, 48);
            loading_page.set_child(Some(&spinner));
            tab.results_scroll.set_child(Some(&loading_page));
            Some(output_generation)
        } else {
            None
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

fn create_query_output(
    result: Option<&QueryResultSnapshot>,
    explain: Option<&serde_json::Value>,
    explain_error: Option<&str>,
) -> Box {
    let content = result.map_or_else(
        || {
            let content = Box::new(Orientation::Vertical, 0);
            content.append(&create_results_unavailable());
            content
        },
        create_results_grid,
    );
    append_explain(&content, explain, explain_error);
    content
}

fn append_explain(content: &Box, explain: Option<&serde_json::Value>, explain_error: Option<&str>) {
    if let Some(explain) = explain {
        content.append(&create_explain_section(explain));
    } else if let Some(error) = explain_error {
        content.append(&create_explain_error(error));
    }
}

fn create_explain_section(json_value: &serde_json::Value) -> Box {
    let (rows_estimate, first_cost, last_cost) = extract_explain_stats(json_value);

    let section = Box::new(Orientation::Vertical, 8);
    section.set_margin_start(12);
    section.set_margin_end(12);
    section.set_margin_top(12);
    section.set_margin_bottom(12);
    section.add_css_class("card");

    let info_box = Box::new(Orientation::Horizontal, 12);
    info_box.set_margin_start(12);
    info_box.set_margin_end(12);
    info_box.set_margin_top(8);
    info_box.set_margin_bottom(4);

    let explain_icon = Label::new(Some("🔍"));
    explain_icon.add_css_class("title-3");
    info_box.append(&explain_icon);

    let title_label = Label::new(Some("Query Plan"));
    title_label.add_css_class("title-4");
    title_label.set_halign(gtk4::Align::Start);
    info_box.append(&title_label);

    let spacer = Box::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    info_box.append(&spacer);

    let stats_box = Box::new(Orientation::Horizontal, 20);

    let rows_label = Label::new(Some(&format!("📊 ~{} rows", rows_estimate)));
    rows_label.add_css_class("dim-label");
    stats_box.append(&rows_label);

    let cost_label = Label::new(Some(&format!(
        "💰 Cost: {:.2}..{:.2}",
        first_cost, last_cost
    )));
    cost_label.add_css_class("dim-label");
    stats_box.append(&cost_label);

    info_box.append(&stats_box);

    let plan = serde_json::to_string_pretty(json_value).unwrap_or_else(|_| json_value.to_string());
    let plan_label = Label::new(Some(&plan));
    plan_label.set_halign(gtk4::Align::Fill);
    plan_label.set_xalign(0.0);
    plan_label.set_selectable(true);
    plan_label.add_css_class("monospace");
    plan_label.set_margin_start(12);
    plan_label.set_margin_end(12);
    plan_label.set_margin_bottom(8);

    section.append(&info_box);
    section.append(&gtk4::Separator::new(Orientation::Horizontal));
    section.append(&plan_label);
    section
}

fn create_explain_error(error: &str) -> Box {
    let section = Box::new(Orientation::Vertical, 4);
    section.set_margin_start(12);
    section.set_margin_end(12);
    section.set_margin_top(12);
    section.set_margin_bottom(12);
    section.add_css_class("card");

    let title = Label::new(Some("Query Plan Unavailable"));
    title.set_halign(gtk4::Align::Start);
    title.add_css_class("heading");
    title.set_margin_start(12);
    title.set_margin_end(12);
    title.set_margin_top(8);

    let detail = Label::new(Some(error));
    detail.set_halign(gtk4::Align::Start);
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.add_css_class("dim-label");
    detail.set_margin_start(12);
    detail.set_margin_end(12);
    detail.set_margin_bottom(8);

    section.append(&title);
    section.append(&detail);
    section
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
        let trim_end = query_text
            .rfind(|c: char| !c.is_whitespace())
            .map(|pos| pos + 1)
            .unwrap_or(query_text.len());

        let content_start = start + trim_start;
        let content_end = start + trim_end;

        // If cursor is within the actual content, this is our query
        if cursor_pos >= content_start && cursor_pos < content_end {
            return Some((trimmed.to_string(), content_start, content_end));
        }
    }

    // 2. Cursor is not within content - prefer the query before cursor
    for (start, _end, query_text) in queries.iter().rev() {
        let trimmed = query_text.trim();
        if trimmed.is_empty() {
            continue;
        }

        let trim_start = query_text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        let trim_end = query_text
            .rfind(|c: char| !c.is_whitespace())
            .map(|pos| pos + 1)
            .unwrap_or(query_text.len());

        let content_start = start + trim_start;
        let content_end = start + trim_end;

        // Use the last query whose content ends before or at cursor
        if content_end <= cursor_pos {
            return Some((trimmed.to_string(), content_start, content_end));
        }
    }

    // 3. Still no match? Return the first non-empty query
    for (start, _end, query_text) in &queries {
        let trimmed = query_text.trim();
        if !trimmed.is_empty() {
            let trim_start = query_text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
            let trim_end = query_text
                .rfind(|c: char| !c.is_whitespace())
                .map(|pos| pos + 1)
                .unwrap_or(query_text.len());

            return Some((trimmed.to_string(), start + trim_start, start + trim_end));
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

fn create_results_unavailable() -> adw::StatusPage {
    let status_page = adw::StatusPage::new();
    status_page.set_icon_name(Some("document-open-recent-symbolic"));
    status_page.set_title("Results Not Stored");
    status_page.set_description(Some("This history entry predates stored result snapshots"));
    status_page
}

fn create_error_page(error: &str) -> adw::StatusPage {
    let error_page = adw::StatusPage::new();
    error_page.set_icon_name(Some("dialog-error-symbolic"));
    error_page.set_title("Query Failed");
    error_page.set_description(Some(error));
    error_page
}

const MAX_HISTORY_RESULT_ROWS: usize = 200;
const MAX_HISTORY_RESULT_BYTES: usize = 1_000_000;

fn result_snapshot(rows: &[Row]) -> QueryResultSnapshot {
    let columns = rows
        .first()
        .map(|row| {
            row.columns()
                .iter()
                .map(|column| column.name().to_string())
                .collect()
        })
        .unwrap_or_default();
    let data = rows
        .iter()
        .map(|row| {
            row.columns()
                .iter()
                .enumerate()
                .map(|(index, column)| display_value(row, index, column.type_().name()))
                .collect()
        })
        .collect();
    QueryResultSnapshot {
        columns,
        rows: data,
        total_rows: rows.len(),
    }
}

fn display_value(row: &Row, index: usize, type_name: &str) -> String {
    if let Ok(Some(value)) = row.try_get::<_, Option<String>>(index) {
        value
    } else if let Ok(None) = row.try_get::<_, Option<String>>(index) {
        "NULL".to_string()
    } else if let Ok(value) = row.try_get::<_, Option<i32>>(index) {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(value) = row.try_get::<_, Option<i64>>(index) {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(value) = row.try_get::<_, Option<f64>>(index) {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(value) = row.try_get::<_, Option<bool>>(index) {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(value) = row.try_get::<_, Option<serde_json::Value>>(index) {
        value
            .map(|json| serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string()))
            .unwrap_or_else(|| "NULL".to_string())
    } else {
        format!("<{}>", type_name)
    }
}

fn bounded_result_snapshot(result: &QueryResultSnapshot) -> QueryResultSnapshot {
    let mut stored_bytes = result.columns.iter().map(String::len).sum::<usize>();
    let rows = result
        .rows
        .iter()
        .take(MAX_HISTORY_RESULT_ROWS)
        .take_while(|row| {
            let row_bytes = row.iter().map(String::len).sum::<usize>();
            if stored_bytes.saturating_add(row_bytes) > MAX_HISTORY_RESULT_BYTES {
                false
            } else {
                stored_bytes += row_bytes;
                true
            }
        })
        .cloned()
        .collect();
    QueryResultSnapshot {
        columns: result.columns.clone(),
        rows,
        total_rows: result.total_rows,
    }
}

fn create_results_grid(result: &QueryResultSnapshot) -> Box {
    let container = Box::new(Orientation::Vertical, 0);

    if result.total_rows == 0 {
        let empty_page = adw::StatusPage::new();
        empty_page.set_icon_name(Some("emblem-ok-symbolic"));
        empty_page.set_title("Query Successful");
        empty_page.set_description(Some("0 rows returned"));
        container.append(&empty_page);
        return container;
    }

    let string_list = StringList::new(&[]);
    for row in &result.rows {
        string_list.append(&serde_json::to_string(row).unwrap_or_default());
    }

    let selection_model = SingleSelection::new(Some(string_list));
    let column_view = ColumnView::new(Some(selection_model));
    column_view.set_show_column_separators(true);
    column_view.set_show_row_separators(true);

    // Add columns
    for (col_idx, column) in result.columns.iter().enumerate() {
        let factory = SignalListItemFactory::new();

        factory.connect_setup(move |_, list_item| {
            let label = Label::new(None);
            label.set_xalign(0.0);
            label.set_margin_start(8);
            label.set_margin_end(8);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            list_item
                .downcast_ref::<ListItem>()
                .unwrap()
                .set_child(Some(&label));
        });

        factory.connect_bind(move |_, list_item| {
            let item = list_item.downcast_ref::<ListItem>().unwrap();
            if let Some(string_obj) = item
                .item()
                .and_then(|o| o.downcast::<gtk4::StringObject>().ok())
            {
                let full_text = string_obj.string().to_string();
                let row: Vec<String> = serde_json::from_str(&full_text).unwrap_or_default();
                if let Some(cell_text) = row.get(col_idx)
                    && let Some(label) = item.child().and_then(|c| c.downcast::<Label>().ok())
                {
                    label.set_text(cell_text);
                }
            }
        });

        let col = ColumnViewColumn::new(Some(column), Some(factory));
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

    let row_count = if result.rows.len() == result.total_rows {
        format!("📊 {} rows returned", result.total_rows)
    } else {
        format!(
            "📊 {} of {} rows stored",
            result.rows.len(),
            result.total_rows
        )
    };
    let row_count_label = Label::new(Some(&row_count));
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
LIMIT 100;"
            .to_string(),
        2 => "-- Query 2: Recent Orders
SELECT * 
FROM orders 
WHERE created_at > CURRENT_DATE - INTERVAL '7 days'
ORDER BY created_at DESC;"
            .to_string(),
        _ => "-- New Query\n\n".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_query_at_the_cursor() {
        let sql = "SELECT 1;\n\nSELECT 2;";

        assert_eq!(extract_query_at_cursor(sql, 3).unwrap().0, "SELECT 1");
        assert_eq!(extract_query_at_cursor(sql, 13).unwrap().0, "SELECT 2");
        assert_eq!(extract_query_at_cursor(sql, 9).unwrap().0, "SELECT 1");
    }

    #[test]
    fn stored_results_are_bounded() {
        let result = QueryResultSnapshot {
            columns: vec!["value".to_string()],
            rows: (0..MAX_HISTORY_RESULT_ROWS + 1)
                .map(|index| vec![index.to_string()])
                .collect(),
            total_rows: MAX_HISTORY_RESULT_ROWS + 1,
        };

        let stored = bounded_result_snapshot(&result);

        assert_eq!(stored.rows.len(), MAX_HISTORY_RESULT_ROWS);
        assert_eq!(stored.total_rows, MAX_HISTORY_RESULT_ROWS + 1);
    }

    #[test]
    fn stored_results_respect_the_size_limit() {
        let result = QueryResultSnapshot {
            columns: vec!["value".to_string()],
            rows: vec![
                vec!["a".repeat(MAX_HISTORY_RESULT_BYTES)],
                vec!["second".to_string()],
            ],
            total_rows: 2,
        };

        let stored = bounded_result_snapshot(&result);

        assert!(stored.rows.is_empty());
        assert_eq!(stored.total_rows, 2);
    }
}
