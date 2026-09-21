use adw::ActionRow;
use adw::prelude::*;
use gtk4::prelude::*;
use gtk4::{Box, Label, ListBox, Orientation, ScrolledWindow};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::query_history::{QueryHistoryEntry, QueryHistoryOutcome, QueryHistoryStore};

type OpenCallback = Rc<dyn Fn(&QueryHistoryEntry)>;

#[derive(Clone)]
pub struct QueryHistory {
    container: Box,
    listbox: ListBox,
    empty_label: Label,
    open_callback: Rc<RefCell<Option<OpenCallback>>>,
    store: Rc<RefCell<Option<QueryHistoryStore>>>,
    entry_count: Rc<Cell<usize>>,
}

impl QueryHistory {
    pub fn new(db_name: &str) -> Self {
        let container = Box::new(Orientation::Vertical, 0);
        container.set_vexpand(true);

        let header = Box::new(Orientation::Vertical, 2);
        header.set_margin_start(12);
        header.set_margin_end(12);
        header.set_margin_top(12);
        header.set_margin_bottom(6);

        let title = Label::new(Some("Query History"));
        title.add_css_class("title-4");
        title.set_halign(gtk4::Align::Start);
        header.append(&title);

        let database = Label::new(Some(db_name));
        database.add_css_class("dim-label");
        database.set_halign(gtk4::Align::Start);
        database.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        header.append(&database);

        container.append(&header);

        let scrolled = ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);

        let listbox = ListBox::new();
        listbox.add_css_class("navigation-sidebar");
        listbox.set_selection_mode(gtk4::SelectionMode::None);

        let empty_label = Label::new(Some("Run a query to build history"));
        empty_label.add_css_class("dim-label");
        empty_label.set_margin_start(12);
        empty_label.set_margin_end(12);
        empty_label.set_margin_top(24);
        empty_label.set_wrap(true);
        listbox.append(&empty_label);

        scrolled.set_child(Some(&listbox));
        container.append(&scrolled);

        let history = Self {
            container,
            listbox,
            empty_label,
            open_callback: Rc::new(RefCell::new(None)),
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

    pub fn set_open_callback<F>(&self, callback: F)
    where
        F: Fn(&QueryHistoryEntry) + 'static,
    {
        *self.open_callback.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn add(&self, entry: QueryHistoryEntry) {
        self.render_entry(&entry);
        if let Some(store) = self.store.borrow_mut().as_mut()
            && let Err(error) = store.add(entry)
        {
            tracing::warn!("Failed to save query history: {}", error);
        }
    }

    fn render_entry(&self, entry: &QueryHistoryEntry) {
        self.empty_label.set_visible(false);

        let row = ActionRow::new();
        row.set_use_markup(false);
        row.set_title(&query_title(&entry.sql));
        row.set_title_lines(2);
        row.set_subtitle_lines(1);
        row.set_activatable(true);
        let executed_at = format_timestamp(entry.executed_at);

        match &entry.outcome {
            QueryHistoryOutcome::Success {
                duration_ms,
                rows,
                explain,
                explain_error,
                ..
            } => {
                let plan_status = if explain.is_some() {
                    "plan saved"
                } else if explain_error.is_some() {
                    "plan unavailable"
                } else {
                    "no plan"
                };
                row.set_subtitle(&format!(
                    "{} - {} - {} rows - {}",
                    executed_at,
                    format_duration(*duration_ms),
                    rows,
                    plan_status
                ));
                if let Some(error) = explain_error {
                    row.set_tooltip_text(Some(error));
                }
            }
            QueryHistoryOutcome::Failure { error } => {
                row.set_subtitle(&format!("{} - Failed - {}", executed_at, error));
                row.set_tooltip_text(Some(error));
            }
        }

        let open_button = gtk4::Button::from_icon_name("document-open-symbolic");
        open_button.set_valign(gtk4::Align::Center);
        open_button.set_tooltip_text(Some("Open in editor"));
        open_button.add_css_class("flat");
        open_button.add_css_class("circular");
        row.add_suffix(&open_button);

        let entry = Rc::new(entry.clone());
        if matches!(
            &entry.outcome,
            QueryHistoryOutcome::Success {
                explain: Some(_),
                ..
            }
        ) {
            let explain_button = gtk4::Button::from_icon_name("documentinfo-symbolic");
            explain_button.set_valign(gtk4::Align::Center);
            explain_button.set_tooltip_text(Some("Open results and saved query plan"));
            explain_button.add_css_class("flat");
            explain_button.add_css_class("circular");
            row.add_suffix(&explain_button);

            let callback = self.open_callback.clone();
            let entry_for_explain = Rc::clone(&entry);
            explain_button.connect_clicked(move |_| {
                if let Some(callback) = callback.borrow().as_ref() {
                    callback(&entry_for_explain);
                }
            });
        }

        let callback = self.open_callback.clone();
        let entry_for_row = Rc::clone(&entry);
        row.connect_activated(move |_| {
            if let Some(callback) = callback.borrow().as_ref() {
                callback(&entry_for_row);
            }
        });

        let callback = self.open_callback.clone();
        let entry_for_button = Rc::clone(&entry);
        open_button.connect_clicked(move |_| {
            if let Some(callback) = callback.borrow().as_ref() {
                callback(&entry_for_button);
            }
        });

        self.listbox.prepend(&row);
        let entry_count = self.entry_count.get() + 1;
        self.entry_count.set(entry_count);
        if entry_count > 100 {
            if let Some(oldest) = self.empty_label.prev_sibling() {
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
}
