use adw::{OverlaySplitView, TabBar, TabPage, TabView};
use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{
    Box, ColumnView, ColumnViewColumn, Label, ListItem, Orientation, Overlay, Paned,
    ScrolledWindow, SignalListItemFactory, SingleSelection, StringList,
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

use super::plan_inspector::{PlanInspector, PlanScroll, PlanState};
use super::preview_overlay::{PreviewRelease, preview_release};
use super::query_history::{QueryHistoryEntry, QueryHistoryOutcome, QueryResultSnapshot};
use super::query_state::QueryStateManager;
use super::result_inspector::ResultInspector;
use super::sidebar::{HistoryPlanAction, QueryHistory};

type EmptyCallback = Rc<dyn Fn()>;
use dzban_db::ResultSet;

const MAX_RESULT_COLUMN_WIDTH: i32 = 360;
const RESULT_CELL_HORIZONTAL_SPACE: i32 = 16;
const RESULT_HEADER_HORIZONTAL_SPACE: i32 = 48;
const MAX_RESULT_CELL_PREVIEW_CHARS: usize = 512;

// Stores UI elements for each query tab
struct QueryTab {
    page: TabPage,
    editor: SourceView,
    results_scroll: ScrolledWindow,
    state_manager: QueryStateManager,
    output_generation: Cell<u64>,
    result_scroll_position: Cell<Option<ResultScrollPosition>>,
    result_scroll_restore_pending: Rc<Cell<bool>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResultScrollPosition {
    horizontal: f64,
    vertical: f64,
}

#[derive(Clone)]
pub struct QueryTabManager {
    _db_name: String,
    tab_view: TabView,
    tab_bar: TabBar,
    container: OverlaySplitView,
    query_counter: Rc<RefCell<u32>>,
    tabs: Rc<RefCell<HashMap<u32, QueryTab>>>, // Map query number to tab data
    history: QueryHistory,
    plan_inspector: PlanInspector,
    result_inspector: ResultInspector,
    empty_callback: Rc<RefCell<Option<EmptyCallback>>>,
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
        tab_view.set_shortcuts(adw::TabViewShortcuts::empty());

        // Create TabBar
        let tab_bar = TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);
        tab_bar.set_focusable(true);
        tab_bar.update_property(&[gtk4::accessible::Property::Label("Query tabs")]);

        content_box.append(&tab_bar);
        content_box.append(&tab_view);

        let content_overlay = Overlay::new();
        content_overlay.set_child(Some(&content_box));
        let plan_inspector = PlanInspector::new();
        content_overlay.add_overlay(plan_inspector.widget());
        let result_inspector = ResultInspector::new();
        content_overlay.add_overlay(result_inspector.widget());
        split_view.set_content(Some(&content_overlay));

        let manager = Self {
            _db_name: db_name.to_string(),
            tab_view,
            tab_bar,
            container: split_view,
            query_counter: Rc::new(RefCell::new(0)),
            tabs: Rc::new(RefCell::new(HashMap::new())),
            history,
            plan_inspector,
            result_inspector,
            empty_callback: Rc::new(RefCell::new(None)),
        };

        let weak_tab_view = manager.tab_view.downgrade();
        let weak_tabs = Rc::downgrade(&manager.tabs);
        let result_inspector = manager.result_inspector.clone();
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
            tab.result_scroll_position.set(None);
            tab.result_scroll_restore_pending.set(false);
            tab.editor.buffer().set_text(&entry.sql);
            match &entry.outcome {
                QueryHistoryOutcome::Success { result, .. } => {
                    tab.results_scroll.set_child(Some(&create_query_output(
                        result.as_ref(),
                        Some(&result_inspector),
                    )));
                }
                QueryHistoryOutcome::Failure { error } => {
                    tab.results_scroll
                        .set_child(Some(&create_error_page(error)));
                }
            }
            tab.editor.grab_focus();
        });

        let plan_inspector = manager.plan_inspector.clone();
        manager.history.set_plan_callback(move |entry, action| {
            let QueryHistoryOutcome::Success {
                explain: Some(plan),
                ..
            } = &entry.outcome
            else {
                return;
            };
            match action {
                HistoryPlanAction::ShowTransient => {
                    plan_inspector.show_saved(plan, false);
                }
                HistoryPlanAction::HideTransient => plan_inspector.hide_transient(),
                HistoryPlanAction::Pin => plan_inspector.show_saved(plan, true),
            }
        });

        let weak_tabs = Rc::downgrade(&manager.tabs);
        let empty_callback = manager.empty_callback.clone();
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
                if tabs.borrow().is_empty()
                    && let Some(callback) = empty_callback.borrow().as_ref()
                {
                    callback();
                }
            }
            glib::Propagation::Proceed
        });

        let plan_inspector = manager.plan_inspector.clone();
        manager.tab_view.connect_selected_page_notify(move |_| {
            plan_inspector.close();
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

        let (content, editor, results_scroll) = create_query_editor_pane();

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
                result_scroll_position: Cell::new(None),
                result_scroll_restore_pending: Rc::new(Cell::new(false)),
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

    pub fn focus_tab_bar(&self) -> bool {
        self.tab_bar.grab_focus()
    }

    pub fn focus_current_results(&self) -> bool {
        let Some(query_num) = self.current_query_id() else {
            return false;
        };
        let tabs = self.tabs.borrow();
        let Some(tab) = tabs.get(&query_num) else {
            return false;
        };
        let Some(column_view) = find_result_view(tab.results_scroll.upcast_ref()) else {
            return tab.results_scroll.grab_focus();
        };
        column_view.grab_focus()
    }

    pub fn focus_history(&self) -> bool {
        self.container.set_show_sidebar(true);
        self.history.focus()
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

    pub fn display_results_for(
        &self,
        query_num: u32,
        result_set: ResultSet,
    ) -> QueryResultSnapshot {
        let result = result_snapshot(&result_set);
        self.display_result_snapshot_for(query_num, &result);
        bounded_result_snapshot(&result)
    }

    fn display_result_snapshot_for(&self, query_num: u32, result: &QueryResultSnapshot) {
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            let results_widget = create_results_grid(result, Some(&self.result_inspector));
            tab.results_scroll.set_child(Some(&results_widget));
            if let Some(position) = tab.result_scroll_position.get() {
                tab.result_scroll_restore_pending.set(true);
                restore_result_scroll_position(
                    &tab.results_scroll,
                    position,
                    tab.result_scroll_restore_pending.clone(),
                );
            }
        }
    }

    pub fn display_explain_for(
        &self,
        query_num: u32,
        output_generation: u64,
        explain: Option<&serde_json::Value>,
        explain_error: Option<&str>,
    ) {
        let is_current = self
            .tabs
            .borrow()
            .get(&query_num)
            .filter(|tab| tab.output_generation.get() == output_generation)
            .is_some()
            && self.is_query_selected(query_num);
        if !is_current {
            return;
        }
        if let Some(explain) = explain {
            self.plan_inspector
                .set_state(&PlanState::Ready(explain.clone()));
        } else if let Some(error) = explain_error {
            self.plan_inspector
                .set_state(&PlanState::Error(error.to_string()));
        }
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
            let has_results = find_result_view(tab.results_scroll.upcast_ref()).is_some();
            if has_results && !tab.result_scroll_restore_pending.get() {
                tab.result_scroll_position
                    .set(Some(result_scroll_position(&tab.results_scroll)));
            }
            let output_generation = tab.output_generation.get().wrapping_add(1);
            tab.output_generation.set(output_generation);
            let loading_page = adw::StatusPage::new();
            loading_page.set_widget_name("query-running-status");
            loading_page.set_icon_name(Some("content-loading-symbolic"));
            loading_page.set_title("Running Query");
            loading_page.set_description(Some("Waiting for results"));
            let spinner = gtk4::Spinner::new();
            spinner.set_spinning(true);
            spinner.set_size_request(48, 48);
            loading_page.set_child(Some(&spinner));
            if !has_results {
                tab.results_scroll.set_child(Some(&loading_page));
            }
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

    pub fn close_current_tab(&self) -> bool {
        let Some(page) = self.tab_view.selected_page() else {
            return false;
        };
        self.tab_view.close_page(&page);
        true
    }

    pub fn tab_count(&self) -> i32 {
        self.tab_view.n_pages()
    }

    pub fn set_empty_callback<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        *self.empty_callback.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn select_next_tab(&self) -> bool {
        if self.tab_view.select_next_page() {
            true
        } else if self.tab_view.n_pages() > 1 {
            self.tab_view.set_selected_page(&self.tab_view.nth_page(0));
            true
        } else {
            false
        }
    }

    pub fn select_previous_tab(&self) -> bool {
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

    pub fn current_tab_title(&self) -> Option<String> {
        let query_num = self.current_query_id()?;
        self.tabs
            .borrow()
            .get(&query_num)
            .map(|tab| tab.page.title().to_string())
    }

    pub fn rename_current_tab(&self, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        let Some(query_num) = self.current_query_id() else {
            return false;
        };
        let tabs = self.tabs.borrow();
        let Some(tab) = tabs.get(&query_num) else {
            return false;
        };
        tab.page.set_title(title);
        true
    }

    pub fn tab_titles(&self) -> Vec<(u32, String)> {
        let mut tabs = self
            .tabs
            .borrow()
            .iter()
            .map(|(query_num, tab)| {
                (
                    self.tab_view.page_position(&tab.page),
                    *query_num,
                    tab.page.title().to_string(),
                )
            })
            .collect::<Vec<_>>();
        tabs.sort_by_key(|(position, _, _)| *position);
        tabs.into_iter()
            .map(|(_, query_num, title)| (query_num, title))
            .collect()
    }

    pub fn select_tab(&self, query_num: u32) -> bool {
        let page = self
            .tabs
            .borrow()
            .get(&query_num)
            .map(|tab| tab.page.clone());
        page.is_some_and(|page| {
            self.tab_view.set_selected_page(&page);
            self.focus_current_editor();
            true
        })
    }

    pub fn set_plan_preview(&self, state: PlanState) {
        self.plan_inspector.set_state(&state);
    }

    pub fn show_plan_transient(&self) {
        self.result_inspector.close();
        self.plan_inspector.show_transient();
    }

    pub fn hide_plan_transient(&self) {
        self.plan_inspector.hide_transient();
    }

    pub fn pin_plan(&self) -> bool {
        self.plan_inspector.pin()
    }

    pub fn is_plan_pinned(&self) -> bool {
        self.plan_inspector.is_pinned()
    }

    pub fn close_plan(&self) -> bool {
        let closed = self.plan_inspector.close();
        if closed {
            self.focus_current_editor();
        }
        closed
    }

    pub fn scroll_plan(&self, direction: PlanScroll) -> bool {
        self.plan_inspector.scroll(direction)
    }

    pub fn show_focused_result(&self) -> bool {
        let Some(values) = self.focused_result_values() else {
            return false;
        };
        self.plan_inspector.close();
        self.result_inspector.show_row(&values);
        true
    }

    pub fn pin_result(&self) -> bool {
        self.result_inspector.pin()
    }

    pub fn close_result(&self) -> bool {
        let closed = self.result_inspector.close();
        if closed {
            self.focus_current_results();
        }
        closed
    }

    fn focused_result_values(&self) -> Option<Vec<(String, String)>> {
        let query_num = self.current_query_id()?;
        let tabs = self.tabs.borrow();
        let tab = tabs.get(&query_num)?;
        let window = self.container.root()?.downcast::<gtk4::Window>().ok()?;
        let focus = gtk4::prelude::GtkWindowExt::focus(&window)?;
        if !focus.is_ancestor(&tab.results_scroll) {
            return None;
        }
        selected_result_values(&find_result_view(tab.results_scroll.upcast_ref())?)
    }
}

fn result_scroll_position(scroll: &ScrolledWindow) -> ResultScrollPosition {
    ResultScrollPosition {
        horizontal: scroll.hadjustment().value(),
        vertical: scroll.vadjustment().value(),
    }
}

fn restore_result_scroll_position(
    scroll: &ScrolledWindow,
    position: ResultScrollPosition,
    pending: Rc<Cell<bool>>,
) {
    let horizontal = scroll.hadjustment();
    let vertical = scroll.vadjustment();
    restore_adjustment(&horizontal, position.horizontal);
    restore_adjustment(&vertical, position.vertical);
    let frames_remaining = Rc::new(Cell::new(4_u8));
    scroll.add_tick_callback(move |_, _| {
        restore_adjustment(&horizontal, position.horizontal);
        restore_adjustment(&vertical, position.vertical);
        let remaining = frames_remaining.get() - 1;
        frames_remaining.set(remaining);
        if remaining == 0 {
            pending.set(false);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn restore_adjustment(adjustment: &gtk4::Adjustment, value: f64) {
    let maximum = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
    adjustment.set_value(value.clamp(adjustment.lower(), maximum));
}

fn find_result_view(root: &gtk4::Widget) -> Option<ColumnView> {
    let mut widgets = vec![root.clone()];
    while let Some(widget) = widgets.pop() {
        if let Some(column_view) = widget.downcast_ref::<ColumnView>() {
            return Some(column_view.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            widgets.push(current);
        }
    }
    None
}

#[cfg(test)]
fn find_status_page(root: &gtk4::Widget) -> Option<adw::StatusPage> {
    let mut widgets = vec![root.clone()];
    while let Some(widget) = widgets.pop() {
        if let Some(status_page) = widget.downcast_ref::<adw::StatusPage>() {
            return Some(status_page.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            widgets.push(current);
        }
    }
    None
}

fn create_query_output(
    result: Option<&QueryResultSnapshot>,
    result_inspector: Option<&ResultInspector>,
) -> Box {
    result.map_or_else(
        || {
            let content = Box::new(Orientation::Vertical, 0);
            content.append(&create_results_unavailable());
            content
        },
        |result| create_results_grid(result, result_inspector),
    )
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

fn create_query_editor_pane() -> (Box, SourceView, ScrolledWindow) {
    let container = Box::new(Orientation::Vertical, 0);
    container.set_vexpand(true);

    // Vertical paned: SQL Editor (top) | Results (bottom)
    let paned = Paned::new(Orientation::Vertical);
    paned.add_css_class("query-split");
    paned.set_vexpand(true);
    paned.set_position(400);
    paned.set_wide_handle(false);
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(false);

    // SQL Editor with syntax highlighting
    let editor_scroll = ScrolledWindow::new();
    editor_scroll.set_vexpand(true);
    editor_scroll.set_hexpand(true);

    // Create SourceView for SQL
    let sql_editor = SourceView::new();
    sql_editor.set_widget_name("sql-query-editor");
    sql_editor.update_property(&[
        gtk4::accessible::Property::Label("SQL query editor"),
        gtk4::accessible::Property::Description("Write SQL statements to preview or execute"),
        gtk4::accessible::Property::MultiLine(true),
    ]);

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
    results_scroll.set_focusable(true);
    results_scroll.update_property(&[
        gtk4::accessible::Property::Label("Query results"),
        gtk4::accessible::Property::Description("Output, status, or error from the selected query"),
    ]);

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

fn result_snapshot(result_set: &ResultSet) -> QueryResultSnapshot {
    let columns = result_set
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect();
    let data = result_set
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| value.display().to_string())
                .collect()
        })
        .collect();
    QueryResultSnapshot {
        columns,
        rows: data,
        total_rows: result_set.rows.len(),
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

fn create_results_grid(
    result: &QueryResultSnapshot,
    result_inspector: Option<&ResultInspector>,
) -> Box {
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
    selection_model.set_autoselect(true);
    selection_model.set_can_unselect(false);
    let column_view = ColumnView::new(Some(selection_model));
    column_view.set_focusable(true);
    column_view.set_show_column_separators(true);
    column_view.set_show_row_separators(true);

    for (col_idx, column) in result.columns.iter().enumerate() {
        let factory = SignalListItemFactory::new();
        let column_width = result_column_width(&column_view, column, &result.rows, col_idx);

        factory.connect_setup(move |_, list_item| {
            let cell = create_result_cell();
            let item = list_item.downcast_ref::<ListItem>().unwrap();
            item.set_child(Some(&cell));
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
                    && let Some(cell) = item.child().and_then(|c| c.downcast::<Label>().ok())
                {
                    set_result_cell_value(&cell, cell_text);
                }
            }
        });

        let col = ColumnViewColumn::new(Some(column), Some(factory));
        col.set_resizable(true);
        col.set_expand(false);
        col.set_fixed_width(column_width);
        column_view.append_column(&col);
    }
    if let Some(inspector) = result_inspector.cloned() {
        install_result_preview_controller(&column_view, inspector);
    }

    container.append(&column_view);

    container
}

fn create_result_cell() -> Label {
    let label = Label::new(None);
    label.set_xalign(0.0);
    label.set_halign(gtk4::Align::Fill);
    label.set_hexpand(true);
    label.set_wrap(false);
    label.set_lines(1);
    label.set_single_line_mode(true);
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.set_margin_end(8);
    label.set_margin_top(4);
    label.set_margin_bottom(4);
    label
}

fn install_result_preview_controller(column_view: &ColumnView, inspector: ResultInspector) {
    let controller = gtk4::EventControllerKey::new();
    controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let started = Rc::new(Cell::new(None::<std::time::Instant>));
    let started_for_press = started.clone();
    let weak_view = column_view.downgrade();
    let inspector_for_press = inspector.clone();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        let modifiers = modifiers
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::ALT_MASK);
        if key != gdk::Key::space || !modifiers.is_empty() {
            return glib::Propagation::Proceed;
        }
        if inspector_for_press.is_pinned() {
            inspector_for_press.close();
            started_for_press.set(None);
            return glib::Propagation::Stop;
        }
        let Some(values) = weak_view
            .upgrade()
            .and_then(|view| selected_result_values(&view))
        else {
            return glib::Propagation::Proceed;
        };
        inspector_for_press.show_row(&values);
        if started_for_press.get().is_none() {
            started_for_press.set(Some(std::time::Instant::now()));
        }
        glib::Propagation::Stop
    });
    let inspector_for_release = inspector;
    controller.connect_key_released(move |_, key, _, _| {
        if key != gdk::Key::space {
            return;
        }
        let Some(started) = started.take() else {
            return;
        };
        match preview_release(started.elapsed()) {
            PreviewRelease::Pin => {
                inspector_for_release.pin();
            }
            PreviewRelease::Hide => inspector_for_release.hide_transient(),
        }
    });
    column_view.add_controller(controller);
}

fn selected_result_values(column_view: &ColumnView) -> Option<Vec<(String, String)>> {
    let selection = column_view.model()?.downcast::<SingleSelection>().ok()?;
    let item = selection
        .selected_item()?
        .downcast::<gtk4::StringObject>()
        .ok()?;
    let row: Vec<String> = serde_json::from_str(&item.string()).ok()?;
    let columns = column_view.columns();
    Some(
        row.into_iter()
            .enumerate()
            .map(|(index, value)| {
                let name = columns
                    .item(index as u32)
                    .and_then(|column| column.downcast::<ColumnViewColumn>().ok())
                    .and_then(|column| column.title())
                    .map(Into::into)
                    .unwrap_or_else(|| format!("Column {}", index + 1));
                (name, value)
            })
            .collect(),
    )
}

fn result_column_width(
    column_view: &ColumnView,
    column_name: &str,
    rows: &[Vec<String>],
    column_index: usize,
) -> i32 {
    let layout = column_view.create_pango_layout(None);
    layout.set_text(column_name);
    let (header_width, _) = layout.pixel_size();
    let mut width = header_width + RESULT_HEADER_HORIZONTAL_SPACE;
    let measurement_cell = create_result_cell();

    for row in rows {
        let Some(value) = row.get(column_index) else {
            continue;
        };
        set_result_cell_value(&measurement_cell, value);
        let (_, natural_width, _, _) = measurement_cell.measure(Orientation::Horizontal, -1);
        width = width.max(natural_width + RESULT_CELL_HORIZONTAL_SPACE);
    }

    width.min(MAX_RESULT_COLUMN_WIDTH)
}

fn set_result_cell_value(cell: &Label, value: &str) {
    cell.set_text(&result_cell_preview(value));
    cell.set_tooltip_text(Some(value));
}

fn result_cell_preview(value: &str) -> String {
    let mut preview = String::with_capacity(value.len().min(MAX_RESULT_CELL_PREVIEW_CHARS));
    let mut character_count = 0;

    for character in value.chars() {
        if matches!(character, '\n' | '\r' | '\t') {
            if !preview.is_empty() && !preview.ends_with(' ') {
                preview.push(' ');
                character_count += 1;
            }
            continue;
        }
        if character.is_control() {
            continue;
        }
        preview.push(character);
        character_count += 1;
        if character_count > MAX_RESULT_CELL_PREVIEW_CHARS {
            break;
        }
    }

    if character_count <= MAX_RESULT_CELL_PREVIEW_CHARS {
        preview
    } else {
        preview
            .chars()
            .take(MAX_RESULT_CELL_PREVIEW_CHARS - 3)
            .collect::<String>()
            + "..."
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

    #[test]
    fn result_cell_previews_hide_line_breaks_and_limit_large_values() {
        assert_eq!(
            result_cell_preview("first\r\nsecond\n\tthird"),
            "first second third"
        );
        assert_eq!(
            result_cell_preview("2026-09-23 12:34:56.123456+02"),
            "2026-09-23 12:34:56.123456+02"
        );

        let preview = result_cell_preview(&"ž".repeat(MAX_RESULT_CELL_PREVIEW_CHARS + 1));
        assert_eq!(preview.chars().count(), MAX_RESULT_CELL_PREVIEW_CHARS);
        assert!(preview.ends_with("..."));
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn running_query_status_does_not_duplicate_plan_estimates() {
        let manager = QueryTabManager::new("running-status-test");
        manager.new_query_tab();
        manager.show_loading_for(1).unwrap();

        let results_scroll = manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .results_scroll
            .clone();
        let status = find_status_page(results_scroll.upcast_ref()).unwrap();
        assert_eq!(status.title(), "Running Query");
        assert_eq!(status.description().as_deref(), Some("Waiting for results"));
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn result_cells_are_single_line_and_keep_the_full_value() {
        let cell = create_result_cell();
        let value = "first line\nsecond line with the complete value";

        set_result_cell_value(&cell, value);

        assert_eq!(cell.xalign(), 0.0);
        assert_eq!(cell.halign(), gtk4::Align::Fill);
        assert!(cell.hexpands());
        assert!(!cell.wraps());
        assert_eq!(cell.lines(), 1);
        assert!(cell.is_single_line_mode());
        assert_eq!(cell.ellipsize(), gtk4::pango::EllipsizeMode::End);
        assert_eq!(
            cell.text(),
            "first line second line with the complete value"
        );
        assert_eq!(cell.tooltip_text().as_deref(), Some(value));
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn result_columns_fit_their_values_with_a_maximum_width() {
        let column_view = ColumnView::new(None::<SingleSelection>);
        let short = result_column_width(&column_view, "id", &[vec!["42".to_string()]], 0);
        let timestamp = result_column_width(
            &column_view,
            "created_at",
            &[vec!["2026-09-23 12:34:56.123456+02".to_string()]],
            0,
        );
        let oversized = result_column_width(
            &column_view,
            "payload",
            &[vec!["wide value ".repeat(100)]],
            0,
        );

        assert!(timestamp > short);
        assert!(timestamp < MAX_RESULT_COLUMN_WIDTH);
        assert_eq!(oversized, MAX_RESULT_COLUMN_WIDTH);
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn pg_stat_activity_basic_values_are_not_ellipsized() {
        let result = QueryResultSnapshot {
            columns: vec![
                "datid".to_string(),
                "datname".to_string(),
                "pid".to_string(),
                "query_start".to_string(),
                "state".to_string(),
                "query_id".to_string(),
                "backend_type".to_string(),
            ],
            rows: vec![vec![
                "16384".to_string(),
                "production".to_string(),
                "91234".to_string(),
                "2026-09-23 12:34:56.123456+02".to_string(),
                "active".to_string(),
                "781234567890".to_string(),
                "client backend".to_string(),
            ]],
            total_rows: 1,
        };
        let grid = create_results_grid(&result, None);
        let window = gtk4::Window::new();
        window.set_default_size(1600, 400);
        window.set_child(Some(&grid));
        window.present();
        gtk4::test_widget_wait_for_draw(&grid);

        for value in &result.rows[0] {
            let label = find_descendant_label(grid.upcast_ref(), value)
                .unwrap_or_else(|| panic!("did not render result value {value:?}"));
            assert!(
                !label.layout().is_ellipsized(),
                "result value {value:?} was ellipsized at {} px",
                label.width()
            );
        }
        window.close();
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn result_values_align_with_their_column_titles() {
        let result = QueryResultSnapshot {
            columns: vec!["aligned_title".to_string()],
            rows: vec![vec!["aligned value".to_string()]],
            total_rows: 1,
        };
        let grid = create_results_grid(&result, None);
        let window = gtk4::Window::new();
        window.set_default_size(600, 300);
        window.set_child(Some(&grid));
        window.present();
        gtk4::test_widget_wait_for_draw(&grid);

        let title = find_descendant_label(grid.upcast_ref(), "aligned_title").unwrap();
        let value = find_descendant_label(grid.upcast_ref(), "aligned value").unwrap();
        let origin = gtk4::graphene::Point::zero();
        let title_x = title.compute_point(&grid, &origin).unwrap().x();
        let value_x = value.compute_point(&grid, &origin).unwrap().x();

        assert!(
            (title_x - value_x).abs() <= 1.0,
            "column title starts at {title_x}px but its values start at {value_x}px"
        );
        window.close();
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn rerunning_a_query_preserves_both_result_scroll_positions() {
        let manager = QueryTabManager::new("result-scroll-test");
        manager.new_query_tab();
        let result = QueryResultSnapshot {
            columns: (0..12).map(|index| format!("column_{index}")).collect(),
            rows: (0..100)
                .map(|row| {
                    (0..12)
                        .map(|column| format!("row {row} column {column} long value"))
                        .collect()
                })
                .collect(),
            total_rows: 100,
        };
        manager.display_result_snapshot_for(1, &result);

        let window = gtk4::Window::new();
        window.set_default_size(700, 500);
        window.set_child(Some(manager.get_widget()));
        window.present();
        gtk4::test_widget_wait_for_draw(manager.get_widget());

        let scroll = manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .results_scroll
            .clone();
        let horizontal = scroll.hadjustment();
        let vertical = scroll.vadjustment();
        horizontal.set_value((horizontal.upper() - horizontal.page_size()) * 0.6);
        vertical.set_value((vertical.upper() - vertical.page_size()) * 0.6);
        let expected = result_scroll_position(&scroll);
        assert!(expected.horizontal > 0.0);
        assert!(expected.vertical > 0.0);

        for rerun in 1..=3 {
            manager.show_loading_for(1).unwrap();
            assert!(find_result_view(scroll.upcast_ref()).is_some());
            assert_eq!(result_scroll_position(&scroll), expected);

            manager.display_result_snapshot_for(1, &result);
            gtk4::test_widget_wait_for_draw(manager.get_widget());
            while glib::MainContext::default().iteration(false) {}

            let restored = result_scroll_position(&scroll);
            assert!(
                (restored.horizontal - expected.horizontal).abs() <= 1.0,
                "horizontal result position changed on rerun {rerun}"
            );
            assert!(
                (restored.vertical - expected.vertical).abs() <= 1.0,
                "vertical result position changed on rerun {rerun}"
            );
        }
        window.close();
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn focused_result_row_opens_all_values_in_the_preview() {
        let manager = QueryTabManager::new("result-preview-test");
        manager.new_query_tab();
        let result = QueryResultSnapshot {
            columns: vec!["id".to_string(), "payload".to_string(), "state".to_string()],
            rows: vec![vec![
                "42".to_string(),
                "first line\nsecond line".to_string(),
                "active".to_string(),
            ]],
            total_rows: 1,
        };
        let grid = create_results_grid(&result, Some(&manager.result_inspector));
        manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .results_scroll
            .set_child(Some(&grid));

        let window = gtk4::Window::new();
        window.set_default_size(900, 600);
        window.set_child(Some(manager.get_widget()));
        window.present();
        gtk4::test_widget_wait_for_draw(&grid);

        let results_scroll = manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .results_scroll
            .clone();
        let column_view =
            find_result_view(results_scroll.upcast_ref()).expect("result view was not created");
        assert!(column_view.is_focusable());
        assert!(column_view.is_mapped());
        assert!(manager.focus_current_results());
        let focus = gtk4::prelude::GtkWindowExt::focus(&window).unwrap();
        assert!(focus == column_view || focus.is_ancestor(&column_view));
        let selection = column_view
            .model()
            .and_downcast::<SingleSelection>()
            .unwrap();
        assert_eq!(selection.selected(), 0);
        let controllers = column_view.observe_controllers();
        let key_controller = (0..controllers.n_items())
            .find_map(|index| {
                controllers
                    .item(index)
                    .and_then(|controller| controller.downcast::<gtk4::EventControllerKey>().ok())
            })
            .expect("result cell key controller was not installed");
        let handled = key_controller.emit_by_name::<bool>(
            "key-pressed",
            &[&gdk::Key::space, &0_u32, &gdk::ModifierType::empty()],
        );
        assert!(handled);
        assert!(manager.result_inspector.is_visible());
        assert_eq!(
            manager.result_inspector.current_values(),
            vec![
                ("id".to_string(), "42".to_string()),
                ("payload".to_string(), "first line\nsecond line".to_string()),
                ("state".to_string(), "active".to_string()),
            ]
        );
        key_controller.emit_by_name::<()>(
            "key-released",
            &[&gdk::Key::space, &0_u32, &gdk::ModifierType::empty()],
        );
        assert!(manager.result_inspector.is_pinned());
        key_controller.emit_by_name::<bool>(
            "key-pressed",
            &[&gdk::Key::space, &0_u32, &gdk::ModifierType::empty()],
        );
        assert!(!manager.result_inspector.is_visible());
        window.close();
    }

    fn find_descendant_label(root: &gtk4::Widget, text: &str) -> Option<Label> {
        let mut widgets = vec![root.clone()];
        while let Some(widget) = widgets.pop() {
            if let Some(label) = widget.downcast_ref::<Label>()
                && label.text() == text
            {
                return Some(label.clone());
            }
            let mut child = widget.first_child();
            while let Some(current) = child {
                child = current.next_sibling();
                widgets.push(current);
            }
        }
        None
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn query_tabs_can_be_navigated_located_and_renamed() {
        let manager = QueryTabManager::new("tab-actions-test");
        manager.new_query_tab();
        let first_editor = manager.tabs.borrow().get(&1).unwrap().editor.clone();
        assert_eq!(
            first_editor
                .buffer()
                .text(
                    &first_editor.buffer().start_iter(),
                    &first_editor.buffer().end_iter(),
                    false,
                )
                .as_str(),
            ""
        );
        manager.new_query_tab();

        let window = gtk4::Window::new();
        window.set_child(Some(manager.get_widget()));
        window.present();
        gtk4::test_widget_wait_for_draw(manager.get_widget());

        assert_eq!(manager.current_tab_title().as_deref(), Some("Query 2"));
        assert!(manager.select_previous_tab());
        assert_eq!(manager.current_tab_title().as_deref(), Some("Query 1"));
        assert!(manager.select_previous_tab());
        assert_eq!(manager.current_tab_title().as_deref(), Some("Query 2"));
        assert!(manager.rename_current_tab("Analysis"));
        assert_eq!(manager.current_tab_title().as_deref(), Some("Analysis"));
        assert_eq!(
            manager.tab_titles(),
            vec![(1, "Query 1".to_string()), (2, "Analysis".to_string())]
        );
        assert!(manager.select_tab(1));
        assert_eq!(manager.current_tab_title().as_deref(), Some("Query 1"));
        assert!(manager.focus_tab_bar());
        let focus = gtk4::prelude::GtkWindowExt::focus(&window).unwrap();
        assert!(focus == manager.tab_bar || focus.is_ancestor(&manager.tab_bar));
        manager.focus_current_editor();
        let focus = gtk4::prelude::GtkWindowExt::focus(&window).unwrap();
        assert!(focus == first_editor || focus.is_ancestor(&first_editor));
        assert!(manager.focus_current_results());
        let results_scroll = manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .results_scroll
            .clone();
        let focus = gtk4::prelude::GtkWindowExt::focus(&window).unwrap();
        assert!(focus == results_scroll || focus.is_ancestor(&results_scroll));
        assert!(manager.focus_history());
        assert!(manager.is_sidebar_visible());
        window.close();
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn closing_the_last_query_tab_reports_that_no_tab_remains() {
        let manager = QueryTabManager::new("tab-close-test");
        manager.new_query_tab();
        assert!(manager.close_current_tab());
        assert!(!manager.close_current_tab());
    }
}
