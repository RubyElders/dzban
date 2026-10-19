use std::cell::RefCell;
use std::rc::Rc;

use dzban_core::presentation::{HistoryOutcomeKind, HistoryPresentation};
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use libadwaita as adw;

use super::format;

type RestoreCallback = Rc<dyn Fn(uuid::Uuid, usize)>;
type Callback = Rc<dyn Fn()>;

#[derive(Default)]
struct Callbacks {
    restore: Option<RestoreCallback>,
    refresh: Option<Callback>,
    close: Option<Callback>,
}

struct Inner {
    root: gtk4::Box,
    stack: gtk4::Stack,
    list: gtk4::ListBox,
    spinner: gtk4::Spinner,
    placeholder: adw::StatusPage,
    presentation: RefCell<Option<HistoryPresentation>>,
    callbacks: RefCell<Callbacks>,
}

#[derive(Clone)]
pub struct HistoryPane(Rc<Inner>);

impl HistoryPane {
    pub fn new() -> Self {
        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .accessible_role(gtk4::AccessibleRole::Group)
            .build();
        root.add_css_class("query-history-pane");
        root.set_size_request(240, -1);
        root.update_property(&[gtk4::accessible::Property::Label("Query History")]);

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        header.add_css_class("workspace-section-header");
        header.set_margin_start(12);
        header.set_margin_end(6);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        let title = gtk4::Label::new(Some("Query History"));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        header.append(&title);
        let spinner = gtk4::Spinner::new();
        header.append(&spinner);
        let refresh = gtk4::Button::from_icon_name("view-refresh-symbolic");
        refresh.add_css_class("flat");
        refresh.set_tooltip_text(Some("Refresh history"));
        refresh.update_property(&[gtk4::accessible::Property::Label("Refresh history")]);
        header.append(&refresh);
        let close = gtk4::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.set_tooltip_text(Some("Close history"));
        close.update_property(&[gtk4::accessible::Property::Label("Close history")]);
        header.append(&close);
        root.append(&header);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

        let stack = gtk4::Stack::new();
        stack.set_vexpand(true);
        let placeholder = adw::StatusPage::new();
        placeholder.add_css_class("compact");
        placeholder.set_icon_name(Some("document-open-recent-symbolic"));
        placeholder.set_description(Some("Connect to a database to browse saved queries."));
        stack.add_named(&placeholder, Some("placeholder"));

        let list = gtk4::ListBox::new();
        list.add_css_class("workspace-list");
        list.add_css_class("query-history-list");
        list.set_selection_mode(gtk4::SelectionMode::Single);
        list.update_property(&[gtk4::accessible::Property::Label("Saved queries")]);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));
        stack.add_named(&scroll, Some("list"));
        root.append(&stack);

        let pane = Self(Rc::new(Inner {
            root,
            stack,
            list,
            spinner,
            placeholder,
            presentation: RefCell::new(None),
            callbacks: RefCell::new(Callbacks::default()),
        }));
        let weak = Rc::downgrade(&pane.0);
        refresh.connect_clicked(move |_| {
            if let Some(callback) = weak
                .upgrade()
                .and_then(|inner| inner.callbacks.borrow().refresh.clone())
            {
                callback();
            }
        });
        let weak = Rc::downgrade(&pane.0);
        close.connect_clicked(move |_| {
            if let Some(callback) = weak
                .upgrade()
                .and_then(|inner| inner.callbacks.borrow().close.clone())
            {
                callback();
            }
        });
        let weak = Rc::downgrade(&pane.0);
        pane.0.list.connect_row_activated(move |_, row| {
            if let Some(inner) = weak.upgrade() {
                HistoryPane(inner).restore(row.index());
            }
        });
        let keys = gtk4::EventControllerKey::new();
        let weak = Rc::downgrade(&pane.0);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if key == gdk::Key::Escape
                && modifiers.is_empty()
                && let Some(callback) = weak
                    .upgrade()
                    .and_then(|inner| inner.callbacks.borrow().close.clone())
            {
                callback();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        pane.0.root.add_controller(keys);
        pane.set_unavailable();
        pane
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.0.root
    }

    pub fn connect_restore(&self, callback: impl Fn(uuid::Uuid, usize) + 'static) {
        self.0.callbacks.borrow_mut().restore = Some(Rc::new(callback));
    }

    pub fn connect_refresh(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().refresh = Some(Rc::new(callback));
    }

    pub fn connect_close(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().close = Some(Rc::new(callback));
    }

    fn restore(&self, row: i32) {
        let target = self.0.presentation.borrow().as_ref().and_then(|history| {
            usize::try_from(row)
                .ok()
                .and_then(|row| history.entries.get(row))
                .map(|entry| (history.revision, entry.index))
        });
        let callback = self.0.callbacks.borrow().restore.clone();
        if let (Some((revision, index)), Some(callback)) = (target, callback) {
            callback(revision, index);
        }
    }

    pub fn set_loading(&self, loading: bool) {
        self.0.spinner.set_spinning(loading);
        self.0.spinner.set_visible(loading);
    }

    pub fn set_unavailable(&self) {
        self.0.presentation.replace(None);
        self.0.placeholder.set_title("");
        self.0
            .placeholder
            .set_description(Some("Connect to a database to browse saved queries."));
        self.0.stack.set_visible_child_name("placeholder");
        self.set_loading(false);
    }

    pub fn set_error(&self, message: &str) {
        self.0.placeholder.set_title("History unavailable");
        self.0.placeholder.set_description(Some(message));
        self.0.stack.set_visible_child_name("placeholder");
        self.set_loading(false);
    }

    pub fn set_presentation(&self, history: HistoryPresentation) {
        let selected = self.0.list.selected_row().map(|row| row.index());
        while let Some(child) = self.0.list.first_child() {
            self.0.list.remove(&child);
        }
        if history.entries.is_empty() {
            self.0.placeholder.set_title("");
            self.0
                .placeholder
                .set_description(Some("Queries you run on this connection appear here."));
            self.0.stack.set_visible_child_name("placeholder");
        } else {
            for entry in &history.entries {
                self.0.list.append(&history_row(entry));
            }
            self.0.stack.set_visible_child_name("list");
            if let Some(row) = selected.and_then(|index| self.0.list.row_at_index(index)) {
                self.0.list.select_row(Some(&row));
            }
        }
        self.0.presentation.replace(Some(history));
        self.set_loading(false);
    }

    pub fn navigate(&self, direction: super::navigation::Direction) -> bool {
        match direction.vertical() {
            Some(delta) if super::navigation::contains_focus(&self.0.root) => {
                super::navigation::move_list(&self.0.list, delta)
            }
            _ => false,
        }
    }

    pub fn focus(&self) -> bool {
        let row = self
            .0
            .list
            .selected_row()
            .or_else(|| self.0.list.row_at_index(0));
        match row {
            Some(row) => {
                self.0.list.select_row(Some(&row));
                row.grab_focus()
            }
            None => self.0.list.grab_focus(),
        }
    }

    pub fn has_focus(&self) -> bool {
        self.0
            .root
            .root()
            .and_then(|root| root.focus())
            .is_some_and(|focus| focus.is_ancestor(&self.0.root))
    }

    #[cfg(test)]
    pub fn entry_count(&self) -> usize {
        self.0
            .presentation
            .borrow()
            .as_ref()
            .map_or(0, |history| history.entries.len())
    }

    #[cfg(test)]
    pub fn activate(&self, row: i32) {
        self.restore(row);
    }
}

fn history_row(entry: &dzban_core::presentation::HistoryEntryPresentation) -> gtk4::ListBoxRow {
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 3);
    content.add_css_class("query-history-content");
    let sql = gtk4::Label::new(Some(entry.sql_preview.trim()));
    sql.add_css_class("monospace");
    sql.add_css_class("query-history-title");
    sql.set_xalign(0.0);
    sql.set_lines(2);
    sql.set_wrap(true);
    sql.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    sql.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    content.append(&sql);

    let date = gtk4::Label::new(Some(&format::executed_at(entry.executed_at)));
    date.add_css_class("caption");
    date.add_css_class("dim-label");
    date.set_xalign(0.0);
    content.append(&date);

    let mut status = match entry.status {
        HistoryOutcomeKind::Success => "Succeeded".to_string(),
        HistoryOutcomeKind::Failure => "Failed".to_string(),
    };
    if let Some(rows) = entry.rows {
        status.push_str(&format!(" - {rows} rows"));
    }
    if let Some(duration) = entry.duration_ms {
        status.push_str(&format!(" - {duration} ms"));
    }
    let outcome = gtk4::Label::new(Some(&status));
    outcome.add_css_class("caption");
    outcome.set_xalign(0.0);
    if entry.status == HistoryOutcomeKind::Failure {
        outcome.add_css_class("error");
    } else {
        outcome.add_css_class("dim-label");
    }
    content.append(&outcome);

    let row = gtk4::ListBoxRow::new();
    row.add_css_class("query-history-row");
    row.set_child(Some(&content));
    row.update_property(&[
        gtk4::accessible::Property::Label(entry.sql_preview.trim()),
        gtk4::accessible::Property::Description(&status),
    ]);
    row
}
