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

use super::plan_inspector::{PlanInspector, PlanScroll, PlanState};
use super::preview_overlay::{PreviewRelease, preview_release};
use super::query_history::{QueryHistoryOutcome, QueryResultSnapshot};
use super::result_inspector::ResultInspector;
use super::sidebar::{HistoryPlanAction, QueryHistory};
use dzban_core::documents::{CloseDocument, QueryDocument, QueryDocuments};
use dzban_core::query_selection::query_at_cursor as extract_query_at_cursor;

type WorkspaceCloseCallback = Rc<dyn Fn()>;
use dzban_core::result_stream::{ResultPhase, ResultSnapshot, ResultTermination};
use dzban_gtk::database::RuntimeBridge;
use dzban_gtk::database::{QueryPlan, ResultSet};
use dzban_gtk::result_model::ResultListModel;

enum StreamView {
    Empty,
    Loading(ResultSnapshot),
    Ready(ResultListModel, Label),
    Unreadable,
}

const MAX_RESULT_COLUMN_WIDTH: i32 = 360;
const RESULT_CELL_HORIZONTAL_SPACE: i32 = 16;
const RESULT_HEADER_HORIZONTAL_SPACE: i32 = 48;
const MAX_RESULT_CELL_PREVIEW_CHARS: usize = 512;

// Stores UI elements for each query tab
struct QueryTab {
    page: TabPage,
    editor: SourceView,
    results_scroll: ScrolledWindow,
    stream_status: Label,
    document: QueryDocument,
    result_scroll_position: Cell<Option<ResultScrollPosition>>,
    result_scroll_restore_pending: Rc<Cell<bool>>,
    stream_view: RefCell<StreamView>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResultScrollPosition {
    horizontal: f64,
    vertical: f64,
    row: u32,
}

#[derive(Clone)]
pub struct QueryTabManager {
    _db_name: String,
    tab_view: TabView,
    tab_bar: TabBar,
    container: OverlaySplitView,
    documents: Rc<RefCell<QueryDocuments>>,
    tabs: Rc<RefCell<HashMap<u32, QueryTab>>>, // Map query number to tab data
    history: QueryHistory,
    plan_inspector: PlanInspector,
    result_inspector: ResultInspector,
    workspace_close_callback: Rc<RefCell<Option<WorkspaceCloseCallback>>>,
    preview_service: Rc<RefCell<Option<dzban_core::preview::PlanPreviewService>>>,
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
            documents: Rc::new(RefCell::new(QueryDocuments::default())),
            tabs: Rc::new(RefCell::new(HashMap::new())),
            history,
            plan_inspector,
            result_inspector,
            workspace_close_callback: Rc::new(RefCell::new(None)),
            preview_service: Rc::new(RefCell::new(None)),
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
            tab.document.replace_output();
            tab.result_scroll_position.set(None);
            tab.result_scroll_restore_pending.set(false);
            tab.editor.buffer().set_text(&entry.sql);
            match &entry.outcome {
                QueryHistoryOutcome::Success {
                    command: Some(command),
                    ..
                } => {
                    tab.results_scroll
                        .set_child(Some(&create_command_page(command)));
                }
                QueryHistoryOutcome::Success { result, .. } => {
                    tab.results_scroll.set_child(Some(&create_query_output(
                        result.as_ref(),
                        Some(&result_inspector),
                    )));
                }
                QueryHistoryOutcome::Failure { error, .. } => {
                    tab.results_scroll
                        .set_child(Some(&create_error_page(error)));
                }
            }
            tab.editor.grab_focus();
        });

        let plan_inspector = manager.plan_inspector.clone();
        manager.history.set_plan_callback(move |entry, action| {
            let plan = match &entry.outcome {
                QueryHistoryOutcome::Success {
                    explain: Some(plan),
                    ..
                }
                | QueryHistoryOutcome::Failure {
                    explain: Some(plan),
                    ..
                } => plan,
                _ => return,
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
        let documents = manager.documents.clone();
        let workspace_close_callback = manager.workspace_close_callback.clone();
        manager.tab_view.connect_close_page(move |view, page| {
            if let Some(tabs) = weak_tabs.upgrade() {
                let query_num = {
                    let tabs = tabs.borrow();
                    let Some((query_num, tab)) = tabs.iter().find(|(_, tab)| tab.page == *page)
                    else {
                        return glib::Propagation::Proceed;
                    };
                    let outcome = if workspace_close_callback.borrow().is_some() {
                        documents.borrow_mut().close_in_workspace(tab.document.id())
                    } else {
                        documents.borrow_mut().close(tab.document.id())
                    };
                    match outcome {
                        CloseDocument::Busy => {
                            view.close_page_finish(page, false);
                            return glib::Propagation::Stop;
                        }
                        CloseDocument::CloseWorkspace => {
                            view.close_page_finish(page, false);
                            if let Some(callback) = workspace_close_callback.borrow().as_ref() {
                                callback();
                            }
                            return glib::Propagation::Stop;
                        }
                        CloseDocument::Closed { .. } => *query_num,
                        CloseDocument::Missing => return glib::Propagation::Proceed,
                    }
                };
                tabs.borrow_mut().remove(&query_num);
            }
            glib::Propagation::Proceed
        });

        let plan_inspector = manager.plan_inspector.clone();
        let weak_tabs = Rc::downgrade(&manager.tabs);
        let documents = manager.documents.clone();
        let preview_service = manager.preview_service.clone();
        manager.tab_view.connect_selected_page_notify(move |view| {
            if let Some(service) = preview_service.borrow().as_ref() {
                service.workspace_changed(&documents.borrow());
            }
            if let Some(tabs) = weak_tabs.upgrade()
                && let Some(page) = view.selected_page()
                && let Some(tab) = tabs.borrow().values().find(|tab| tab.page == page)
            {
                documents.borrow_mut().select(tab.document.id());
            }
            plan_inspector.close();
        });
        let weak_tabs = Rc::downgrade(&manager.tabs);
        let documents = manager.documents.clone();
        manager
            .tab_view
            .connect_page_reordered(move |_, page, position| {
                if let Some(tabs) = weak_tabs.upgrade()
                    && let Some(tab) = tabs.borrow().values().find(|tab| tab.page == *page)
                {
                    documents
                        .borrow_mut()
                        .reorder(tab.document.id(), position as usize);
                }
            });

        manager
    }

    pub fn get_widget(&self) -> &OverlaySplitView {
        &self.container
    }

    pub fn use_session_tab(&self, document: QueryDocument) {
        self.tab_bar.set_visible(false);
        if let Some(number) = self.current_query_id() {
            let mut tabs = self.tabs.borrow_mut();
            if let Some(tab) = tabs.get_mut(&number) {
                document.set_draft(
                    tab.editor
                        .buffer()
                        .text(
                            &tab.editor.buffer().start_iter(),
                            &tab.editor.buffer().end_iter(),
                            false,
                        )
                        .to_string(),
                );
                tab.document = document.clone();
            }
        }
        *self.documents.borrow_mut() = QueryDocuments::from_document(document);
        self.focus_current_editor();
    }

    pub fn set_single_tab(&self) {
        self.tab_bar.set_visible(false);
    }

    pub fn new_query_tab(&self) {
        let document = self.documents.borrow_mut().open();
        let query_num = document.number();

        let (content, editor, results_scroll, stream_status) = create_query_editor_pane();

        // Add to TabView
        let page = self.tab_view.append(&content);
        page.set_title(&document.snapshot().title);
        page.set_icon(Some(&gtk4::gio::ThemedIcon::new("text-editor-symbolic")));

        // Store the tab data (without revealer)
        self.tabs.borrow_mut().insert(
            query_num,
            QueryTab {
                page: page.clone(),
                editor: editor.clone(),
                results_scroll,
                stream_status,
                document: document.clone(),
                result_scroll_position: Cell::new(None),
                result_scroll_restore_pending: Rc::new(Cell::new(false)),
                stream_view: RefCell::new(StreamView::Empty),
            },
        );

        let cursor_tabs = Rc::downgrade(&self.tabs);
        editor
            .buffer()
            .connect_notify_local(Some("cursor-position"), move |_, _| {
                if let Some(tabs) = cursor_tabs.upgrade()
                    && let Some(tab) = tabs.borrow().get(&query_num)
                {
                    tab.document.query_context_changed();
                }
            });
        let draft_tabs = Rc::downgrade(&self.tabs);
        editor.buffer().connect_changed(move |buffer| {
            if let Some(tabs) = draft_tabs.upgrade()
                && let Some(tab) = tabs.borrow().get(&query_num)
            {
                tab.document.set_draft(
                    buffer
                        .text(&buffer.start_iter(), &buffer.end_iter(), false)
                        .to_string(),
                );
            }
        });

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

    #[cfg(test)]
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
        self.documents
            .borrow()
            .selected()
            .map(|document| document.number())
    }

    pub fn is_query_selected(&self, query_num: u32) -> bool {
        self.current_query_id() == Some(query_num)
    }

    pub fn current_query(&self) -> Option<(u32, String, QueryDocument)> {
        let query_num = self.current_query_id()?;
        let tabs = self.tabs.borrow();
        let tab = tabs.get(&query_num)?;
        let buffer = tab.editor.buffer();
        let (cursor_iter, _) = buffer.selection_bounds().unwrap_or_else(|| {
            let iter = buffer.iter_at_offset(buffer.cursor_position());
            (iter, iter)
        });
        let full_text = tab.document.draft();
        let query = extract_query_at_cursor(&full_text, cursor_iter.offset() as usize)?.0;
        Some((query_num, query, tab.document.clone()))
    }

    pub fn running_query(&self) -> Option<(u32, QueryDocument)> {
        self.tabs
            .borrow()
            .iter()
            .find(|(_, tab)| tab.document.is_active())
            .map(|(query_num, tab)| (*query_num, tab.document.clone()))
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

    pub fn display_command_for(&self, query_num: u32, command: &dzban_core::CommandAcknowledgment) {
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            tab.stream_status.set_visible(false);
            tab.results_scroll
                .set_child(Some(&create_command_page(command)));
        }
    }

    pub fn display_stream_for(
        &self,
        query_num: u32,
        generation: u64,
        snapshot: ResultSnapshot,
        bridge: RuntimeBridge,
    ) {
        if matches!(
            snapshot.phase,
            ResultPhase::FailedAwaitingCleanup(_)
                | ResultPhase::Finished(ResultTermination::Failed(_) | ResultTermination::Canceled)
        ) {
            return;
        }
        let tabs = self.tabs.borrow();
        let Some(tab) = tabs
            .get(&query_num)
            .filter(|tab| tab.document.accepts_output(generation))
        else {
            return;
        };
        let mut state = tab.stream_view.borrow_mut();
        match &mut *state {
            StreamView::Ready(model, status) => {
                model.set_rows(snapshot.rows);
                status.set_text(&stream_status(&snapshot));
                status.set_visible(!matches!(
                    snapshot.phase,
                    ResultPhase::Finished(ResultTermination::Complete)
                ));
                restore_stream_position(tab, &snapshot);
                return;
            }
            StreamView::Loading(current) => {
                *current = snapshot;
                return;
            }
            StreamView::Empty => {}
            StreamView::Unreadable => return,
        }
        let Some(result) = snapshot.result.clone().filter(|_| snapshot.rows > 0) else {
            return;
        };
        *state = StreamView::Loading(snapshot);
        drop(state);
        drop(tabs);
        let manager = self.clone();
        let page = bridge.read_result_page(result.clone(), 0, 0);
        let rows = bridge.read_result_rows(result.clone(), 0);
        glib::spawn_future_local(async move {
            let mut sample = match page.await {
                Ok(Ok(sample)) => result_snapshot(&sample),
                Ok(Err(error)) => {
                    if manager
                        .tabs
                        .borrow()
                        .get(&query_num)
                        .is_some_and(|tab| tab.document.accepts_output(generation))
                    {
                        manager.display_result_read_error(
                            query_num,
                            generation,
                            &error.to_string(),
                        );
                    }
                    return;
                }
                Err(_) => {
                    manager.display_result_read_error(
                        query_num,
                        generation,
                        "Result schema delivery closed",
                    );
                    return;
                }
            };
            let rows = match rows.await {
                Ok(Ok(rows)) => rows,
                Ok(Err(error)) => {
                    manager.display_result_read_error(query_num, generation, &error.to_string());
                    return;
                }
                Err(_) => {
                    manager.display_result_read_error(
                        query_num,
                        generation,
                        "Result page delivery closed",
                    );
                    return;
                }
            };
            sample.rows = rows
                .iter()
                .filter_map(|row| serde_json::from_str(row).ok())
                .collect();
            let tabs = manager.tabs.borrow();
            let Some(tab) = tabs
                .get(&query_num)
                .filter(|tab| tab.document.accepts_output(generation))
            else {
                return;
            };
            let mut state = tab.stream_view.borrow_mut();
            let StreamView::Loading(snapshot) = &*state else {
                return;
            };
            let model = ResultListModel::new(result, bridge, sample.columns.len(), snapshot.rows);
            let weak_tabs = Rc::downgrade(&manager.tabs);
            model.connect_local("load-error", false, move |values| {
                if let Some(tabs) = weak_tabs.upgrade() {
                    let tabs = tabs.borrow();
                    if let Some(tab) = tabs
                        .get(&query_num)
                        .filter(|tab| tab.document.accepts_output(generation))
                    {
                        let error = values[1].get::<String>().unwrap();
                        tab.stream_view.replace(StreamView::Unreadable);
                        tab.stream_status
                            .set_text(&format!("Cannot read result page: {error}"));
                        tab.stream_status.set_visible(true);
                    }
                }
                None
            });
            let view = create_result_column_view(
                model.clone(),
                &sample.columns,
                &sample.rows,
                Some(&manager.result_inspector),
            );
            let status = tab.stream_status.clone();
            status.set_text(&stream_status(snapshot));
            status.set_visible(!matches!(
                snapshot.phase,
                ResultPhase::Finished(ResultTermination::Complete)
            ));
            tab.results_scroll.set_child(Some(&view));
            let snapshot = snapshot.clone();
            *state = StreamView::Ready(model, status);
            restore_stream_position(tab, &snapshot);
        });
    }

    fn display_result_read_error(&self, query_num: u32, generation: u64, error: &str) {
        let tabs = self.tabs.borrow();
        if let Some(tab) = tabs
            .get(&query_num)
            .filter(|tab| tab.document.accepts_output(generation))
        {
            tab.stream_view.replace(StreamView::Unreadable);
            tab.results_scroll
                .set_child(Some(&create_error_page(&format!(
                    "Cannot read result page: {error}"
                ))));
        }
    }

    fn display_result_snapshot_for(&self, query_num: u32, result: &QueryResultSnapshot) {
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            tab.stream_status.set_visible(false);
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
        plan: Option<&QueryPlan>,
        explain_error: Option<&str>,
    ) {
        let tabs = self.tabs.borrow();
        if !tabs
            .get(&query_num)
            .is_some_and(|tab| tab.document.accepts_output(output_generation))
        {
            return;
        }
        drop(tabs);
        if !self.is_query_selected(query_num) {
            return;
        }
        if let Some(plan) = plan {
            self.plan_inspector.set_state(&PlanState::Ready(
                serde_json::to_value(plan).unwrap_or_default(),
            ));
        } else if let Some(error) = explain_error {
            self.plan_inspector
                .set_state(&PlanState::Error(error.to_string()));
        }
    }

    /// Show error message in results area
    pub fn display_error_for(&self, query_num: u32, error: &str) {
        if let Some(tab) = self.tabs.borrow().get(&query_num) {
            tab.stream_status.set_visible(false);
            tab.stream_view.replace(StreamView::Empty);
            tab.results_scroll
                .set_child(Some(&create_error_page(error)));
        }
    }

    pub fn refresh_history(&self) {
        self.history.refresh();
    }

    /// Show loading state in results area
    #[cfg(test)]
    pub fn show_loading_for(&self, query_num: u32) -> Option<u64> {
        let generation = self
            .tabs
            .borrow()
            .get(&query_num)?
            .document
            .replace_output();
        self.show_loading_generation(query_num, generation)
            .then_some(generation)
    }

    pub fn show_loading_generation(&self, query_num: u32, output_generation: u64) -> bool {
        if let Some(tab) = self
            .tabs
            .borrow()
            .get(&query_num)
            .filter(|tab| tab.document.accepts_output(output_generation))
        {
            let has_results = find_result_view(tab.results_scroll.upcast_ref()).is_some();
            if has_results && !tab.result_scroll_restore_pending.get() {
                tab.result_scroll_position
                    .set(Some(result_scroll_position(&tab.results_scroll)));
            }
            tab.stream_status.set_visible(false);
            tab.stream_view.replace(StreamView::Empty);
            let loading_page = adw::StatusPage::new();
            loading_page.set_widget_name("query-running-status");
            loading_page.add_css_class("compact");
            loading_page.set_icon_name(Some("content-loading-symbolic"));
            loading_page.set_title("Running Query");
            loading_page.set_description(Some("Waiting for results"));
            let spinner = gtk4::Spinner::new();
            spinner.set_spinning(true);
            spinner.set_size_request(48, 48);
            loading_page.set_child(Some(&spinner));
            self.result_inspector.close();
            tab.results_scroll.set_child(Some(&loading_page));
            let horizontal = tab.results_scroll.hadjustment();
            let vertical = tab.results_scroll.vadjustment();
            horizontal.set_value(horizontal.lower());
            vertical.set_value(vertical.lower());
            let scroll = tab.results_scroll.clone();
            let loading_page_for_idle = loading_page.clone();
            glib::idle_add_local_once(move || {
                if scroll.child().as_ref() != Some(loading_page_for_idle.upcast_ref()) {
                    return;
                }
                let horizontal = scroll.hadjustment();
                let vertical = scroll.vadjustment();
                horizontal.set_value(horizontal.lower());
                vertical.set_value(vertical.lower());
            });
            true
        } else {
            false
        }
    }

    pub fn toggle_sidebar(&self) {
        let is_showing = self.container.shows_sidebar();
        self.container.set_show_sidebar(!is_showing);
    }

    pub fn is_sidebar_visible(&self) -> bool {
        self.container.shows_sidebar()
    }

    #[cfg(test)]
    pub fn close_current_tab(&self) -> bool {
        let Some(page) = self.tab_view.selected_page() else {
            return false;
        };
        self.tab_view.close_page(&page);
        true
    }

    #[cfg(test)]
    pub fn tab_count(&self) -> i32 {
        self.documents.borrow().documents().len() as i32
    }

    pub fn set_workspace_close_callback<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        *self.workspace_close_callback.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn retire_closed_session(&self, engine: Option<&dzban_core::execution::QueryEngine>) {
        let mut documents = self.documents.borrow_mut();
        if let Some(engine) = engine {
            if let Err(error) = documents.retire_closed_session(engine) {
                tracing::error!("Could not retire query documents: {error}");
            }
        } else {
            let ids = documents
                .documents()
                .iter()
                .map(QueryDocument::id)
                .collect::<Vec<_>>();
            for id in ids {
                documents.close(id);
            }
        }
    }

    pub fn bind_preview_service(&self, service: dzban_core::preview::PlanPreviewService) {
        self.preview_service.replace(Some(service));
    }

    #[cfg(test)]
    pub fn select_next_tab(&self) -> bool {
        let next = self.documents.borrow_mut().select_relative(true);
        next.is_some_and(|document| self.select_tab(document.number()))
    }

    #[cfg(test)]
    pub fn select_previous_tab(&self) -> bool {
        let next = self.documents.borrow_mut().select_relative(false);
        next.is_some_and(|document| self.select_tab(document.number()))
    }

    pub fn current_tab_title(&self) -> Option<String> {
        let query_num = self.current_query_id()?;
        self.tabs
            .borrow()
            .get(&query_num)
            .map(|tab| tab.document.snapshot().title)
    }

    pub fn rename_current_tab(&self, title: &str) -> bool {
        let Some(query_num) = self.current_query_id() else {
            return false;
        };
        let tabs = self.tabs.borrow();
        let Some(tab) = tabs.get(&query_num) else {
            return false;
        };
        if !tab.document.rename(title) {
            return false;
        }
        tab.page.set_title(&tab.document.snapshot().title);
        true
    }

    #[cfg(test)]
    pub fn tab_titles(&self) -> Vec<(u32, String)> {
        self.documents
            .borrow()
            .documents()
            .iter()
            .map(|document| {
                let snapshot = document.snapshot();
                (snapshot.number, snapshot.title)
            })
            .collect()
    }

    #[cfg(test)]
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

    pub fn pin_plan(&self) -> bool {
        self.plan_inspector.pin()
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
    let rows = find_result_view(scroll.upcast_ref())
        .and_then(|view| view.model())
        .map_or(0, |model| model.n_items());
    let height = scroll.vadjustment().upper().max(1.0);
    ResultScrollPosition {
        horizontal: scroll.hadjustment().value(),
        vertical: scroll.vadjustment().value(),
        row: ((scroll.vadjustment().value() / height * f64::from(rows)) as u32)
            .min(rows.saturating_sub(1)),
    }
}

fn restore_stream_position(tab: &QueryTab, snapshot: &ResultSnapshot) {
    let Some(position) = tab.result_scroll_position.get() else {
        return;
    };
    if snapshot.rows <= position.row as usize && !matches!(snapshot.phase, ResultPhase::Finished(_))
    {
        return;
    }
    tab.result_scroll_position.set(None);
    tab.result_scroll_restore_pending.set(true);
    restore_result_scroll_position(
        &tab.results_scroll,
        position,
        tab.result_scroll_restore_pending.clone(),
    );
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
    let view = find_result_view(scroll.upcast_ref());
    scroll.add_tick_callback(move |_, _| {
        if let Some(view) = &view
            && let Some(model) = view.model()
            && model.n_items() > 0
        {
            view.scroll_to(
                position.row.min(model.n_items() - 1),
                None,
                gtk4::ListScrollFlags::NONE,
                None,
            );
        }
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

fn create_query_editor_pane() -> (Box, SourceView, ScrolledWindow, Label) {
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

    let output = Box::new(Orientation::Vertical, 0);
    let status = Label::new(None);
    status.set_xalign(0.0);
    status.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    status.set_single_line_mode(true);
    status.set_visible(false);
    output.append(&status);
    output.append(&results_scroll);
    paned.set_end_child(Some(&output));

    container.append(&paned);

    (container, sql_editor, results_scroll, status)
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

fn create_command_page(command: &dzban_core::CommandAcknowledgment) -> adw::StatusPage {
    let page = adw::StatusPage::new();
    page.set_title("Command completed");
    let description = command.affected_rows.map_or_else(
        || "Affected row count unavailable".into(),
        |rows| format!("{rows} affected rows"),
    );
    page.set_description(Some(&description));
    page
}

#[cfg(test)]
use dzban_core::history::{MAX_HISTORY_RESULT_BYTES, MAX_HISTORY_RESULT_ROWS};

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
    result.bounded()
}

fn stream_status(snapshot: &ResultSnapshot) -> String {
    match &snapshot.phase {
        ResultPhase::WaitingForSchema => "Waiting for results".into(),
        ResultPhase::Receiving => format!("Receiving results - {} rows", snapshot.rows),
        ResultPhase::Draining(limit) => format!(
            "{} reached - {} rows kept; waiting for query completion",
            result_limit_name(*limit),
            snapshot.rows
        ),
        ResultPhase::FailedAwaitingCleanup(error) => {
            format!("{} - waiting for query cleanup", error)
        }
        ResultPhase::Finished(ResultTermination::Limited(limit)) => {
            format!(
                "Result truncated - {} rows kept ({})",
                snapshot.rows,
                result_limit_name(*limit)
            )
        }
        ResultPhase::Finished(ResultTermination::Canceled) => "Query canceled".into(),
        ResultPhase::Finished(ResultTermination::Failed(error)) => error.to_string(),
        ResultPhase::Finished(ResultTermination::Complete) => String::new(),
    }
}

fn result_limit_name(limit: dzban_core::result_store::ResultCutoff) -> &'static str {
    match limit {
        dzban_core::result_store::ResultCutoff::Rows => "10,000-row limit",
        dzban_core::result_store::ResultCutoff::Bytes => "32 MiB limit",
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

    let column_view =
        create_result_column_view(string_list, &result.columns, &result.rows, result_inspector);
    container.append(&column_view);
    container
}

fn create_result_column_view(
    model: impl IsA<gtk4::gio::ListModel>,
    columns: &[String],
    sample: &[Vec<String>],
    result_inspector: Option<&ResultInspector>,
) -> ColumnView {
    let model = model.upcast::<gtk4::gio::ListModel>();
    let streamed_model = model.clone().downcast::<ResultListModel>().ok();
    let tooltip_value = Rc::new(RefCell::new(None::<(u32, usize, String)>));
    let tooltip_request = Rc::new(Cell::new(None::<(u32, usize)>));
    let selection_model = SingleSelection::new(Some(model));
    selection_model.set_autoselect(true);
    selection_model.set_can_unselect(false);
    let column_view = ColumnView::new(Some(selection_model));
    column_view.set_focusable(true);
    column_view.update_property(&[gtk4::accessible::Property::Label("Query results")]);
    column_view.set_show_column_separators(true);
    column_view.set_show_row_separators(true);

    for (col_idx, column) in columns.iter().enumerate() {
        let factory = SignalListItemFactory::new();
        let column_width = result_column_width(&column_view, column, sample, col_idx);
        let streamed = streamed_model.clone();
        let value = tooltip_value.clone();
        let request = tooltip_request.clone();

        factory.connect_setup(move |_, list_item| {
            let cell = create_result_cell();
            let item = list_item.downcast_ref::<ListItem>().unwrap();
            if let Some(model) = streamed.clone() {
                let weak_item = item.downgrade();
                let value = value.clone();
                let request = request.clone();
                cell.connect_query_tooltip(move |cell, _, _, _, tooltip| {
                    let Some(item) = weak_item.upgrade() else {
                        return false;
                    };
                    let position = item.position();
                    if let Some((row, column, text)) = &*value.borrow()
                        && (*row, *column) == (position, col_idx)
                    {
                        tooltip.set_text(Some(text));
                        return true;
                    }
                    if request.get() == Some((position, col_idx)) {
                        return false;
                    }
                    request.set(Some((position, col_idx)));
                    let response = model.read_row(position);
                    let request = request.clone();
                    let value = value.clone();
                    let weak_cell = cell.downgrade();
                    let weak_item = item.downgrade();
                    glib::spawn_future_local(async move {
                        let response = response.await;
                        if request.get() != Some((position, col_idx)) {
                            return;
                        }
                        request.set(None);
                        let Ok(Ok(row)) = response else {
                            return;
                        };
                        let Some(text) = row.rows.first().and_then(|row| row.get(col_idx)) else {
                            return;
                        };
                        value.replace(Some((position, col_idx, text.display().to_string())));
                        if weak_item
                            .upgrade()
                            .is_some_and(|item| item.position() == position)
                            && let Some(cell) = weak_cell.upgrade()
                        {
                            cell.trigger_tooltip_query();
                        }
                    });
                    false
                });
            }
            item.set_child(Some(&cell));
        });

        let streamed = streamed_model.is_some();
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
                    if streamed {
                        cell.set_tooltip_text(None);
                        cell.set_has_tooltip(true);
                    }
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
    if let Some(model) = streamed_model {
        let weak_view = column_view.downgrade();
        let widths = Rc::new(RefCell::new(
            (0..column_view.columns().n_items())
                .map(|index| {
                    column_view
                        .columns()
                        .item(index)
                        .and_downcast::<ColumnViewColumn>()
                        .unwrap()
                        .fixed_width()
                })
                .collect::<Vec<_>>(),
        ));
        let weak_model = model.downgrade();
        model.connect_local("page-ready", false, move |_| {
            let (Some(view), Some(model)) = (weak_view.upgrade(), weak_model.upgrade()) else {
                return None;
            };
            let rows = model.cached_values();
            let mut widths = widths.borrow_mut();
            for (index, width) in widths.iter_mut().enumerate() {
                let column = view
                    .columns()
                    .item(index as u32)
                    .and_downcast::<ColumnViewColumn>()
                    .unwrap();
                if column.fixed_width() != *width {
                    continue;
                }
                let required = result_column_width(
                    &view,
                    column.title().as_deref().unwrap_or_default(),
                    &rows,
                    index,
                );
                if required > *width {
                    *width = required;
                    column.set_fixed_width(required);
                }
            }
            None
        });
    }

    column_view
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
    let request = Rc::new(Cell::new(0u64));
    let request_for_press = request.clone();
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
        if let Some(view) = weak_view.upgrade()
            && let Some(selection) = view
                .model()
                .and_then(|model| model.downcast::<SingleSelection>().ok())
            && let Some(model) = selection
                .model()
                .and_then(|model| model.downcast::<ResultListModel>().ok())
        {
            let generation = request_for_press.get().wrapping_add(1);
            request_for_press.set(generation);
            let position = selection.selected();
            let row = model.read_row(position);
            let request = request_for_press.clone();
            let inspector = inspector_for_press.clone();
            let weak_view = view.downgrade();
            glib::spawn_future_local(async move {
                let Ok(Ok(row)) = row.await else {
                    return;
                };
                if request.get() != generation || !inspector.widget().is_visible() {
                    return;
                }
                let Some(view) = weak_view.upgrade().filter(|view| view.root().is_some()) else {
                    return;
                };
                if view
                    .model()
                    .and_then(|model| model.downcast::<SingleSelection>().ok())
                    .is_none_or(|selection| selection.selected() != position)
                {
                    return;
                }
                let pinned = inspector.is_pinned();
                let values = row
                    .rows
                    .first()
                    .map(|values| {
                        row.columns
                            .iter()
                            .zip(values)
                            .map(|(column, value)| {
                                (column.name.clone(), value.display().to_string())
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                inspector.show_row(&values);
                if pinned {
                    inspector.pin();
                }
            });
        }
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
            PreviewRelease::Hide => {
                request.set(request.get().wrapping_add(1));
                inspector_for_release.hide_transient();
            }
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

    fn test_query_id() -> dzban_core::execution::QueryId {
        let mut documents = QueryDocuments::default();
        let document = documents.open();
        dzban_core::execution::QueryId(document.id().0)
    }

    #[gtk4::test]
    fn session_tab_binding_keeps_editor_events_on_the_shared_document() {
        let manager = QueryTabManager::new("fixture");
        manager.new_query_tab();
        let original = manager.tabs.borrow().get(&1).unwrap().document.clone();
        let shared = QueryDocuments::default().open();
        manager.use_session_tab(shared.clone());
        let editor = manager.tabs.borrow().get(&1).unwrap().editor.clone();
        editor.buffer().set_text("SELECT 42");
        assert_eq!(shared.draft(), "SELECT 42");
        assert_eq!(original.draft(), "");
        let revision = shared.snapshot().draft_revision;
        editor.buffer().place_cursor(&editor.buffer().start_iter());
        assert!(shared.snapshot().draft_revision > revision);
        assert_eq!(manager.current_query().unwrap().2.id(), shared.id());
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn shared_documents_follow_editor_selection_rename_and_reorder() {
        let manager = QueryTabManager::new("document-projection-test");
        manager.new_query_tab();
        let first = manager.documents.borrow().selected().unwrap();
        manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .editor
            .buffer()
            .set_text("SELECT 1;");
        assert_eq!(first.draft(), "SELECT 1;");
        let revision = first.snapshot().draft_revision;
        let buffer = manager.tabs.borrow().get(&1).unwrap().editor.buffer();
        buffer.place_cursor(&buffer.start_iter());
        assert!(first.snapshot().draft_revision > revision);
        assert_eq!(first.draft(), "SELECT 1;");
        assert!(manager.rename_current_tab("  First query  "));
        assert_eq!(first.snapshot().title, "First query");
        manager.new_query_tab();
        let second = manager.documents.borrow().selected().unwrap();
        assert_eq!(second.draft(), "");
        assert!(manager.select_previous_tab());
        assert_eq!(manager.current_query().unwrap().1, "SELECT 1");
        assert_eq!(manager.current_tab_title().as_deref(), Some("First query"));
        let second_page = manager.tabs.borrow().get(&2).unwrap().page.clone();
        assert!(manager.tab_view.reorder_page(&second_page, 0));
        assert_eq!(manager.tab_titles()[0].0, 2);
        assert!(manager.select_next_tab());
        assert_eq!(
            manager.documents.borrow().selected().unwrap().id(),
            second.id()
        );
        assert!(manager.close_current_tab());
        assert_eq!(
            manager.documents.borrow().selected().unwrap().id(),
            first.id()
        );
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn byte_cutoff_notice_stays_outside_the_virtual_table() {
        adw::init().unwrap();
        let snapshot = tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (mut sink, reader) =
                dzban_core::result_stream::start_results(dzban_core::result_store::ResultLimits {
                    memory_rows: 1,
                    ..Default::default()
                });
            sink.schema(vec![dzban_core::ResultColumn {
                name: "payload".into(),
                type_name: "text".into(),
            }])
            .await
            .unwrap();
            for _ in 0..40 {
                if matches!(
                    sink.row(vec![dzban_core::ResultCell::Text("x".repeat(1024 * 1024))])
                        .await
                        .unwrap(),
                    dzban_core::result_stream::Delivery::Stop(_)
                ) {
                    break;
                }
            }
            sink.finish(Ok(())).await.unwrap();
            reader.borrow().clone()
        });
        assert_eq!(snapshot.rows, 31);
        assert_eq!(
            snapshot.phase,
            ResultPhase::Finished(ResultTermination::Limited(
                dzban_core::result_store::ResultCutoff::Bytes
            ))
        );
        let manager = QueryTabManager::new("byte-cutoff-screen-regression");
        manager.new_query_tab();
        let window = gtk4::Window::new();
        window.set_default_size(1000, 700);
        window.set_child(Some(manager.get_widget()));
        window.present();
        let generation = manager.show_loading_for(1).unwrap();
        manager.display_stream_for(1, generation, snapshot, RuntimeBridge::new().unwrap());
        let scroll = manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .results_scroll
            .clone();
        wait_for_stream_widget(|| find_result_view(scroll.upcast_ref()).is_some());
        wait_for_stream_widget(|| {
            find_descendant_label(scroll.upcast_ref(), &"x".repeat(512)).is_some()
        });
        wait_for_stream_widget(|| {
            let tabs = manager.tabs.borrow();
            matches!(&*tabs.get(&1).unwrap().stream_view.borrow(), StreamView::Ready(model, _) if model.phase() == dzban_gtk::result_model::ResultModelPhase::Idle)
        });
        let tabs = manager.tabs.borrow();
        let tab = tabs.get(&1).unwrap();
        assert_eq!(
            tab.stream_status.text(),
            "Result truncated - 31 rows kept (32 MiB limit)"
        );
        assert!(!tab.stream_status.is_ancestor(&scroll));
        gtk4::test_widget_wait_for_draw(manager.get_widget());
        assert!(tab.stream_status.is_mapped());
        assert_eq!(
            tab.stream_status.ellipsize(),
            gtk4::pango::EllipsizeMode::End
        );
        if let Some(path) = std::env::var_os("DZBAN_BYTE_SCREENSHOT") {
            assert!(
                std::process::Command::new("import")
                    .args(["-window", "root"])
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        drop(tabs);
        window.close();
    }

    fn wait_for_stream_widget(mut ready: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "streamed widget did not settle"
            );
            glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn streamed_rerun_restores_scroll_after_the_previous_row_arrives() {
        adw::init().unwrap();
        let mut snapshot = tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (mut sink, reader) =
                dzban_core::result_stream::start_results(dzban_core::result_store::ResultLimits {
                    memory_rows: 1,
                    ..Default::default()
                });
            sink.schema(
                (0..12)
                    .map(|index| dzban_core::ResultColumn {
                        name: format!("column_{index}"),
                        type_name: "text".into(),
                    })
                    .collect(),
            )
            .await
            .unwrap();
            for index in 0..2000 {
                sink.row(
                    (0..12)
                        .map(|column| {
                            dzban_core::ResultCell::Text(format!("row {index} column {column}"))
                        })
                        .collect(),
                )
                .await
                .unwrap();
            }
            sink.finish(Ok(())).await.unwrap();
            reader.borrow().clone()
        });
        let manager = QueryTabManager::new("streamed-scroll-regression");
        manager.new_query_tab();
        let window = gtk4::Window::new();
        window.set_default_size(1000, 700);
        window.set_child(Some(manager.get_widget()));
        window.present();
        let bridge = RuntimeBridge::new().unwrap();
        let generation = manager.show_loading_for(1).unwrap();
        manager.display_stream_for(1, generation, snapshot.clone(), bridge.clone());
        let scroll = manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .results_scroll
            .clone();
        wait_for_stream_widget(|| find_result_view(scroll.upcast_ref()).is_some());
        let view = find_result_view(scroll.upcast_ref()).unwrap();
        gtk4::test_widget_wait_for_draw(&view);
        assert_eq!(view.vadjustment().unwrap(), scroll.vadjustment());
        assert_eq!(view.hadjustment().unwrap(), scroll.hadjustment());
        assert!(view.height() <= scroll.height());
        view.scroll_to(1500, None, gtk4::ListScrollFlags::NONE, None);
        wait_for_stream_widget(|| {
            find_descendant_label(view.upcast_ref(), "row 1500 column 0").is_some()
        });
        scroll.hadjustment().set_value(200.0);
        gtk4::test_widget_wait_for_draw(&view);
        let position = result_scroll_position(&scroll);
        assert!(
            position.row > 1000,
            "position={position:?}, upper={}, page={}",
            scroll.vadjustment().upper(),
            scroll.vadjustment().page_size()
        );
        assert_eq!(position.horizontal, 200.0);
        for _ in 0..3 {
            let generation = manager.show_loading_for(1).unwrap();
            snapshot.rows = 1;
            snapshot.phase = ResultPhase::Receiving;
            manager.display_stream_for(1, generation, snapshot.clone(), bridge.clone());
            wait_for_stream_widget(|| find_result_view(scroll.upcast_ref()).is_some());
            assert!(
                manager
                    .tabs
                    .borrow()
                    .get(&1)
                    .unwrap()
                    .result_scroll_position
                    .get()
                    .is_some()
            );
            snapshot.rows = 1000;
            manager.display_stream_for(1, generation, snapshot.clone(), bridge.clone());
            assert!(
                manager
                    .tabs
                    .borrow()
                    .get(&1)
                    .unwrap()
                    .result_scroll_position
                    .get()
                    .is_some()
            );
            snapshot.rows = 2000;
            snapshot.phase = ResultPhase::Finished(ResultTermination::Complete);
            manager.display_stream_for(1, generation, snapshot.clone(), bridge.clone());
            wait_for_stream_widget(|| {
                !manager
                    .tabs
                    .borrow()
                    .get(&1)
                    .unwrap()
                    .result_scroll_restore_pending
                    .get()
            });
            let view = find_result_view(scroll.upcast_ref()).unwrap();
            wait_for_stream_widget(|| {
                find_descendant_label(view.upcast_ref(), "row 1500 column 0").is_some()
            });
            gtk4::test_widget_wait_for_draw(&view);
            let column = view
                .columns()
                .item(2)
                .and_downcast::<ColumnViewColumn>()
                .unwrap();
            assert!(
                column.fixed_width()
                    >= result_column_width(
                        &view,
                        "column_2",
                        &[vec![
                            String::new(),
                            String::new(),
                            "row 1500 column 2".into()
                        ]],
                        2
                    )
            );
            assert!((scroll.hadjustment().value() - position.horizontal).abs() < 1.0);
            assert!((scroll.vadjustment().value() - position.vertical).abs() < 1.0);
        }
        if let Some(path) = std::env::var_os("DZBAN_SCROLL_SCREENSHOT") {
            assert!(
                std::process::Command::new("import")
                    .args(["-window", "root"])
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        window.close();
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn streamed_preview_keeps_full_values_and_does_not_reopen_after_release() {
        adw::init().unwrap();
        let full_value = "full-value-".repeat(100);
        let snapshot = tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (mut sink, reader) = dzban_core::result_stream::start_results(Default::default());
            sink.schema(vec![dzban_core::ResultColumn {
                name: "payload".into(),
                type_name: "text".into(),
            }])
            .await
            .unwrap();
            sink.row(vec![dzban_core::ResultCell::Text(full_value.clone())])
                .await
                .unwrap();
            sink.finish(Ok(())).await.unwrap();
            reader.borrow().clone()
        });
        let manager = QueryTabManager::new("streamed-preview-regression");
        manager.new_query_tab();
        let window = gtk4::Window::new();
        window.set_default_size(1000, 700);
        window.set_child(Some(manager.get_widget()));
        window.present();
        let generation = manager.show_loading_for(1).unwrap();
        manager.display_stream_for(1, generation, snapshot, RuntimeBridge::new().unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let view = loop {
            let scroll = manager
                .tabs
                .borrow()
                .get(&1)
                .unwrap()
                .results_scroll
                .clone();
            if let Some(view) = find_result_view(scroll.upcast_ref()) {
                break view;
            }
            assert!(std::time::Instant::now() < deadline);
            glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        gtk4::test_widget_wait_for_draw(&view);
        let controllers = view.observe_controllers();
        let key = (0..controllers.n_items())
            .find_map(|index| {
                controllers
                    .item(index)
                    .and_downcast::<gtk4::EventControllerKey>()
            })
            .unwrap();
        key.emit_by_name::<bool>(
            "key-pressed",
            &[&gdk::Key::space, &0_u32, &gdk::ModifierType::empty()],
        );
        key.emit_by_name::<()>(
            "key-released",
            &[&gdk::Key::space, &0_u32, &gdk::ModifierType::empty()],
        );
        while manager.result_inspector.current_values()
            != vec![("payload".into(), full_value.clone())]
        {
            assert!(std::time::Instant::now() < deadline);
            glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(manager.result_inspector.is_pinned());
        if let Some(path) = std::env::var_os("DZBAN_STREAM_SCREENSHOT") {
            gtk4::test_widget_wait_for_draw(manager.get_widget());
            assert!(
                std::process::Command::new("import")
                    .args(["-window", "root"])
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        manager.result_inspector.close();
        key.emit_by_name::<bool>(
            "key-pressed",
            &[&gdk::Key::space, &0_u32, &gdk::ModifierType::empty()],
        );
        std::thread::sleep(std::time::Duration::from_millis(400));
        key.emit_by_name::<()>(
            "key-released",
            &[&gdk::Key::space, &0_u32, &gdk::ModifierType::empty()],
        );
        for _ in 0..50 {
            glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!manager.result_inspector.is_visible());
        window.close();
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn streamed_tabs_grow_without_rebuilding_and_discard_stale_pages() {
        adw::init().unwrap();
        let mut snapshot = tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (mut sink, reader) = dzban_core::result_stream::start_results(Default::default());
            sink.schema(vec![dzban_core::ResultColumn {
                name: "value".into(),
                type_name: "text".into(),
            }])
            .await
            .unwrap();
            for index in 0..1000 {
                sink.row(vec![dzban_core::ResultCell::Text(format!("row {index}"))])
                    .await
                    .unwrap();
            }
            sink.finish(Ok(())).await.unwrap();
            reader.borrow().clone()
        });
        let manager = QueryTabManager::new("streamed-tab-regression");
        manager.new_query_tab();
        let query_num = *manager.tabs.borrow().keys().next().unwrap();
        let generation = manager.show_loading_for(query_num).unwrap();
        let bridge = RuntimeBridge::new().unwrap();
        snapshot.rows = 1;
        snapshot.phase = ResultPhase::Receiving;
        manager.display_stream_for(query_num, generation, snapshot.clone(), bridge.clone());
        snapshot.rows = 1000;
        manager.display_stream_for(query_num, generation, snapshot.clone(), bridge.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if matches!(
                *manager
                    .tabs
                    .borrow()
                    .get(&query_num)
                    .unwrap()
                    .stream_view
                    .borrow(),
                StreamView::Ready(_, _)
            ) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let tabs = manager.tabs.borrow();
        let tab = tabs.get(&query_num).unwrap();
        let view = find_result_view(tab.results_scroll.upcast_ref()).unwrap();
        let selection = view.model().unwrap().downcast::<SingleSelection>().unwrap();
        let model = selection
            .model()
            .unwrap()
            .downcast::<ResultListModel>()
            .unwrap();
        assert_eq!(model.n_items(), 1000);
        assert!(model.cached_rows() <= 200);
        drop(tabs);
        snapshot.phase = ResultPhase::Finished(ResultTermination::Limited(
            dzban_core::result_store::ResultCutoff::Rows,
        ));
        manager.display_stream_for(query_num, generation, snapshot.clone(), bridge.clone());
        let tabs = manager.tabs.borrow();
        let tab = tabs.get(&query_num).unwrap();
        assert_eq!(
            find_result_view(tab.results_scroll.upcast_ref()).unwrap(),
            view
        );
        let state = tab.stream_view.borrow();
        let StreamView::Ready(_, status) = &*state else {
            panic!("stream view missing");
        };
        assert!(status.is_visible());
        assert_eq!(
            status.text(),
            "Result truncated - 1000 rows kept (10,000-row limit)"
        );
        drop(state);
        drop(tabs);
        model.set_rows(1001);
        model.item(1000);
        wait_for_stream_widget(|| {
            model.phase() == dzban_gtk::result_model::ResultModelPhase::Failed
        });
        manager.display_stream_for(query_num, generation, snapshot.clone(), bridge.clone());
        let tabs = manager.tabs.borrow();
        let tab = tabs.get(&query_num).unwrap();
        assert!(matches!(*tab.stream_view.borrow(), StreamView::Unreadable));
        assert_eq!(
            tab.stream_status.text(),
            "Cannot read result page: Result page is missing"
        );
        assert!(tab.stream_status.is_visible());
        drop(tabs);
        let next = manager.show_loading_for(query_num).unwrap();
        manager.display_stream_for(query_num, next, snapshot.clone(), bridge.clone());
        manager.display_error_for(query_num, "test failure");
        for _ in 0..50 {
            glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let tabs = manager.tabs.borrow();
        let tab = tabs.get(&query_num).unwrap();
        assert!(find_result_view(tab.results_scroll.upcast_ref()).is_none());
        assert_eq!(
            find_status_page(tab.results_scroll.upcast_ref())
                .unwrap()
                .title(),
            "Query Failed"
        );
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn command_completion_displays_known_and_unknown_affected_counts() {
        let known = create_command_page(&dzban_core::CommandAcknowledgment {
            affected_rows: Some(2),
        });
        assert_eq!(known.title(), "Command completed");
        assert_eq!(known.description().as_deref(), Some("2 affected rows"));
        let unknown = create_command_page(&dzban_core::CommandAcknowledgment {
            affected_rows: None,
        });
        assert_eq!(unknown.title(), "Command completed");
        assert_eq!(
            unknown.description().as_deref(),
            Some("Affected row count unavailable")
        );
    }

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
    fn running_query_replaces_stale_results_with_progress() {
        let manager = QueryTabManager::new("running-status-test");
        manager.new_query_tab();
        let window = gtk4::Window::new();
        window.set_default_size(1200, 800);
        window.set_child(Some(manager.get_widget()));
        window.present();
        manager.display_result_snapshot_for(
            1,
            &QueryResultSnapshot {
                columns: vec!["value".to_string()],
                rows: vec![vec!["stale".to_string()]],
                total_rows: 1,
            },
        );
        manager.show_loading_for(1).unwrap();
        gtk4::test_widget_wait_for_draw(manager.get_widget());

        let results_scroll = manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .results_scroll
            .clone();
        let status = find_status_page(results_scroll.upcast_ref()).unwrap();
        assert!(find_result_view(results_scroll.upcast_ref()).is_none());
        assert_eq!(status.title(), "Running Query");
        assert_eq!(status.description().as_deref(), Some("Waiting for results"));
        assert_eq!(results_scroll.hadjustment().value(), 0.0);
        assert_eq!(results_scroll.vadjustment().value(), 0.0);
        let vertical = results_scroll.vadjustment();
        assert!(vertical.upper() <= vertical.page_size());
        window.close();
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn running_query_remains_addressable_after_selecting_another_tab() {
        let manager = QueryTabManager::new("background-query-test");
        manager.new_query_tab();
        manager
            .tabs
            .borrow()
            .get(&1)
            .unwrap()
            .document
            .start(test_query_id());
        manager.new_query_tab();

        assert!(manager.is_query_selected(2));
        assert_eq!(manager.running_query().unwrap().0, 1);
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn query_history_sidebar_can_be_hidden_and_restored() {
        let manager = QueryTabManager::new("sidebar-toggle-test");
        assert!(manager.is_sidebar_visible());
        manager.toggle_sidebar();
        assert!(!manager.is_sidebar_visible());
        manager.toggle_sidebar();
        assert!(manager.is_sidebar_visible());
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
            assert!(find_result_view(scroll.upcast_ref()).is_none());
            assert!(find_status_page(scroll.upcast_ref()).is_some());
            while glib::MainContext::default().iteration(false) {}
            assert_eq!(scroll.hadjustment().value(), scroll.hadjustment().lower());
            assert_eq!(scroll.vadjustment().value(), scroll.vadjustment().lower());

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

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn running_query_tab_stays_open_until_the_operation_finishes() {
        let manager = QueryTabManager::new("running-tab-close-test");
        manager.new_query_tab();
        let state = manager.tabs.borrow().get(&1).unwrap().document.clone();
        let query = test_query_id();
        state.start(query);

        assert!(manager.close_current_tab());
        assert_eq!(manager.tab_count(), 1);

        state.finish(query);
        assert!(manager.close_current_tab());
        assert_eq!(manager.tab_count(), 0);
    }
}
