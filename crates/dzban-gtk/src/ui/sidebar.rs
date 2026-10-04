use gtk4::prelude::*;
use gtk4::{Box, Label, ListBox, ListBoxRow, Orientation, Overlay, ScrolledWindow};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::query_history::{QueryHistoryEntry, QueryHistoryOutcome, QueryHistoryStore};

type OpenCallback = Rc<dyn Fn(&QueryHistoryEntry)>;
type PlanCallback = Rc<dyn Fn(&QueryHistoryEntry, HistoryPlanAction)>;

#[derive(Clone, Copy)]
pub enum HistoryPlanAction {
    ShowTransient,
    HideTransient,
    Pin,
}

#[derive(Clone)]
pub struct QueryHistory {
    container: Box,
    listbox: ListBox,
    open_callback: Rc<RefCell<Option<OpenCallback>>>,
    plan_callback: Rc<RefCell<Option<PlanCallback>>>,
    store: Rc<RefCell<Option<QueryHistoryStore>>>,
    entry_count: Rc<Cell<usize>>,
}

impl QueryHistory {
    pub fn new(db_name: &str) -> Self {
        let container = Box::new(Orientation::Vertical, 0);
        container.set_vexpand(true);
        container.set_widget_name("query-history");
        container.add_css_class("query-history-pane");
        container.update_property(&[
            gtk4::accessible::Property::Label("Query History"),
            gtk4::accessible::Property::Description(
                "Saved queries, results, and estimated plans for this database",
            ),
        ]);

        let scrolled = ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);

        let listbox = ListBox::new();
        listbox.add_css_class("query-history-list");
        listbox.add_css_class("workspace-list");
        listbox.update_property(&[
            gtk4::accessible::Property::Label("Query History"),
            gtk4::accessible::Property::Description(
                "Saved queries, results, and estimated plans for this database",
            ),
        ]);
        listbox.set_selection_mode(gtk4::SelectionMode::Browse);
        listbox.set_focusable(true);
        listbox.set_show_separators(true);

        let empty_page = adw::StatusPage::new();
        empty_page.add_css_class("compact");
        empty_page.set_icon_name(Some("document-open-recent-symbolic"));
        empty_page.set_title("No Query History");
        empty_page.set_description(Some("Run a query to save it here"));
        listbox.set_placeholder(Some(&empty_page));

        scrolled.set_child(Some(&listbox));
        container.append(&scrolled);

        let history = Self {
            container,
            listbox,
            open_callback: Rc::new(RefCell::new(None)),
            plan_callback: Rc::new(RefCell::new(None)),
            store: Rc::new(RefCell::new(None)),
            entry_count: Rc::new(Cell::new(0)),
        };

        match QueryHistoryStore::load(db_name) {
            Ok(store) => {
                for entry in store.entries().iter().rev() {
                    history.render_entry(entry);
                }
                *history.store.borrow_mut() = Some(store);
            }
            Err(error) => tracing::warn!("Query history is not persistent: {}", error),
        }

        history
    }

    pub fn widget(&self) -> &Box {
        &self.container
    }

    pub fn focus(&self) -> bool {
        self.refresh();
        let row = self
            .listbox
            .selected_row()
            .or_else(|| self.listbox.row_at_index(0));
        row.map_or_else(
            || self.listbox.grab_focus(),
            |row| {
                self.listbox.select_row(Some(&row));
                row.grab_focus()
            },
        )
    }

    pub fn set_open_callback<F>(&self, callback: F)
    where
        F: Fn(&QueryHistoryEntry) + 'static,
    {
        *self.open_callback.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn set_plan_callback<F>(&self, callback: F)
    where
        F: Fn(&QueryHistoryEntry, HistoryPlanAction) + 'static,
    {
        *self.plan_callback.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn refresh(&self) {
        let updated = {
            let mut store = self.store.borrow_mut();
            let Some(store) = store.as_mut() else {
                return;
            };
            let previous = store.entries().to_vec();
            if let Err(error) = store.refresh() {
                tracing::warn!("Failed to refresh query history: {}", error);
                return;
            }
            if previous == store.entries() {
                return;
            }
            let selected = self
                .listbox
                .selected_row()
                .and_then(|row| previous.get(row.index() as usize).cloned());
            (store.entries().to_vec(), selected)
        };
        while let Some(row) = self.listbox.row_at_index(0) {
            self.listbox.remove(&row);
        }
        self.entry_count.set(0);
        for entry in updated.0.iter().rev() {
            self.render_entry(entry);
        }
        if let Some(selected) = updated.1
            && let Some(index) = updated.0.iter().position(|entry| entry == &selected)
            && let Some(row) = self.listbox.row_at_index(index as i32)
        {
            self.listbox.select_row(Some(&row));
        }
    }

    fn render_entry(&self, entry: &QueryHistoryEntry) {
        let row = ListBoxRow::new();
        row.add_css_class("query-history-row");
        row.set_activatable(true);
        row.set_selectable(true);

        let content = Box::new(Orientation::Vertical, 4);
        content.add_css_class("query-history-content");

        let title = Label::new(Some(&query_title(&entry.sql)));
        title.add_css_class("query-history-title");
        title.set_halign(gtk4::Align::Fill);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title.set_single_line_mode(true);
        content.append(&title);

        let details = Box::new(Orientation::Horizontal, 7);
        let executed_at = format_timestamp(entry.executed_at);
        let status_marker = Box::new(Orientation::Vertical, 0);
        status_marker.set_size_request(7, 7);
        status_marker.set_valign(gtk4::Align::Center);
        status_marker.add_css_class("query-history-status");
        details.append(&status_marker);

        let metadata = Label::new(None);
        metadata.add_css_class("caption");
        metadata.add_css_class("dim-label");
        metadata.set_halign(gtk4::Align::Fill);
        metadata.set_xalign(0.0);
        metadata.set_hexpand(true);
        metadata.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        metadata.set_single_line_mode(true);

        match &entry.outcome {
            QueryHistoryOutcome::Success {
                duration_ms,
                rows,
                explain_error,
                ..
            } => {
                status_marker.add_css_class("success");
                metadata.set_text(&format!(
                    "{}  |  {}  |  {} {}",
                    executed_at,
                    format_duration(*duration_ms),
                    rows,
                    if *rows == 1 { "row" } else { "rows" }
                ));
                if let Some(error) = explain_error {
                    row.set_tooltip_text(Some(error));
                }
            }
            QueryHistoryOutcome::Failure { error, .. } => {
                status_marker.add_css_class("error");
                metadata.set_text(&format!("{}  |  Failed  |  {}", executed_at, error));
                row.set_tooltip_text(Some(error));
            }
        }
        details.append(&metadata);
        let details_overlay = Overlay::new();
        details_overlay.set_child(Some(&details));

        let entry = Rc::new(entry.clone());
        if matches!(
            &entry.outcome,
            QueryHistoryOutcome::Success {
                explain: Some(_),
                ..
            } | QueryHistoryOutcome::Failure {
                explain: Some(_),
                ..
            }
        ) {
            let explain_button = gtk4::Button::from_icon_name("documentinfo-symbolic");
            explain_button.set_valign(gtk4::Align::Center);
            explain_button.set_tooltip_text(Some("Show saved query plan"));
            explain_button.update_property(&[
                gtk4::accessible::Property::Label("Show saved query plan"),
                gtk4::accessible::Property::Description(
                    "Hover to preview the saved plan or press to keep it open",
                ),
            ]);
            explain_button.add_css_class("flat");
            explain_button.add_css_class("circular");
            explain_button.add_css_class("history-row-action");
            explain_button.set_halign(gtk4::Align::End);
            metadata.set_margin_end(24);
            details_overlay.add_overlay(&explain_button);

            let callback = self.plan_callback.clone();
            let entry_for_explain = Rc::clone(&entry);
            explain_button.connect_clicked(move |_| {
                if let Some(callback) = callback.borrow().as_ref() {
                    callback(&entry_for_explain, HistoryPlanAction::Pin);
                }
            });

            let hover = gtk4::EventControllerMotion::new();
            let callback = self.plan_callback.clone();
            let entry_for_hover = Rc::clone(&entry);
            hover.connect_enter(move |_, _, _| {
                if let Some(callback) = callback.borrow().as_ref() {
                    callback(&entry_for_hover, HistoryPlanAction::ShowTransient);
                }
            });
            let callback = self.plan_callback.clone();
            let entry_for_leave = Rc::clone(&entry);
            hover.connect_leave(move |_| {
                if let Some(callback) = callback.borrow().as_ref() {
                    callback(&entry_for_leave, HistoryPlanAction::HideTransient);
                }
            });
            explain_button.add_controller(hover);
        }
        content.append(&details_overlay);
        row.set_child(Some(&content));

        let callback = self.open_callback.clone();
        let entry_for_row = Rc::clone(&entry);
        row.connect_activate(move |_| {
            if let Some(callback) = callback.borrow().as_ref() {
                callback(&entry_for_row);
            }
        });

        self.listbox.prepend(&row);
        self.listbox.select_row(Some(&row));
        let entry_count = self.entry_count.get() + 1;
        self.entry_count.set(entry_count);
        if entry_count > 100 {
            if let Some(oldest) = self.listbox.last_child() {
                self.listbox.remove(&oldest);
            }
            self.entry_count.set(100);
        }
    }
}

fn query_title(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_duration(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{} ms", duration_ms)
    } else {
        format!("{:.2} s", duration_ms as f64 / 1_000.0)
    }
}

fn format_timestamp(timestamp: u64) -> String {
    let timestamp = timestamp.min(i64::MAX as u64) as i64;
    glib::DateTime::from_unix_local(timestamp)
        .and_then(|date_time| date_time.format("%Y-%m-%d %H:%M:%S"))
        .map(String::from)
        .unwrap_or_else(|_| "Unknown time".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_for_widget_test() -> QueryHistory {
        let container = Box::new(Orientation::Vertical, 0);
        let listbox = ListBox::new();
        container.append(&listbox);
        QueryHistory {
            container,
            listbox,
            open_callback: Rc::new(RefCell::new(None)),
            plan_callback: Rc::new(RefCell::new(None)),
            store: Rc::new(RefCell::new(None)),
            entry_count: Rc::new(Cell::new(0)),
        }
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn refreshing_history_shows_other_writers_and_preserves_selection() {
        let directory = std::env::temp_dir().join(format!(
            "dzban-sidebar-history-{}",
            glib::uuid_string_random()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("history.json");
        let history = history_for_widget_test();
        let placeholder = Label::new(Some("No Query History"));
        history.listbox.set_placeholder(Some(&placeholder));
        *history.store.borrow_mut() =
            Some(QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap());
        let mut writer = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
        writer
            .add(QueryHistoryEntry::failure(
                "first".into(),
                "fixture".into(),
                None,
                None,
            ))
            .unwrap();
        history.refresh();
        history
            .listbox
            .select_row(history.listbox.row_at_index(0).as_ref());
        writer
            .add(QueryHistoryEntry::failure(
                "external".into(),
                "fixture".into(),
                None,
                None,
            ))
            .unwrap();
        history.focus();
        assert_eq!(history.entry_count.get(), 2);
        assert_eq!(placeholder.parent(), Some(history.listbox.clone().upcast()));
        assert_eq!(history.listbox.selected_row().unwrap().index(), 1);
        assert_eq!(
            history.store.borrow().as_ref().unwrap().entries()[0].sql,
            "external"
        );
        history.focus();
        assert_eq!(history.entry_count.get(), 2);
        writer
            .add(QueryHistoryEntry::failure(
                "local".into(),
                "fixture".into(),
                None,
                None,
            ))
            .unwrap();
        history.refresh();
        assert_eq!(history.entry_count.get(), 3);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn query_titles_collapse_whitespace() {
        assert_eq!(
            query_title(" SELECT  *\nFROM users "),
            "SELECT * FROM users"
        );
        assert_eq!(
            query_title("SELECT * FROM pg_stat_activity WHERE state <> 'idle'"),
            "SELECT * FROM pg_stat_activity WHERE state <> 'idle'"
        );
    }

    #[test]
    fn durations_use_readable_units() {
        assert_eq!(format_duration(42), "42 ms");
        assert_eq!(format_duration(1_250), "1.25 s");
    }

    #[test]
    fn timestamps_are_displayable() {
        assert_ne!(format_timestamp(0), "Unknown time");
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn successful_and_failed_history_rows_have_the_same_height() {
        let history = history_for_widget_test();
        history.render_entry(&QueryHistoryEntry {
            planning_error: None,
            sql: "SELECT successful_query".to_string(),
            executed_at: 0,
            outcome: QueryHistoryOutcome::Success {
                command: None,
                duration_ms: 42,
                rows: 1,
                result: None,
                explain: Some(serde_json::json!({})),
                explain_error: None,
            },
        });
        history.render_entry(&QueryHistoryEntry {
            planning_error: None,
            sql: "SELECT failed_query".to_string(),
            executed_at: 0,
            outcome: QueryHistoryOutcome::Failure {
                diagnostic: None,
                error: "column does not exist".to_string(),
                explain: Some(serde_json::json!({})),
                explain_error: None,
            },
        });

        let window = gtk4::Window::new();
        window.set_default_size(360, 240);
        window.set_child(Some(history.widget()));
        window.present();
        gtk4::test_widget_wait_for_draw(history.widget());

        let failed = history.listbox.row_at_index(0).unwrap();
        let successful = history.listbox.row_at_index(1).unwrap();
        assert_eq!(successful.height(), failed.height());
        window.close();
    }
}
