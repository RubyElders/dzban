use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk4::{gdk, glib};
use libadwaita as adw;

#[derive(Clone)]
pub struct ActionItem {
    pub title: String,
    pub detail: String,
    pub icon: &'static str,
    pub keywords: Vec<String>,
    pub enabled: bool,
    pub run: Rc<dyn Fn()>,
}

impl ActionItem {
    pub fn new(
        title: impl Into<String>,
        detail: impl Into<String>,
        icon: &'static str,
        run: impl Fn() + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            icon,
            keywords: Vec::new(),
            enabled: true,
            run: Rc::new(run),
        }
    }

    pub fn keywords(mut self, keywords: &[&str]) -> Self {
        self.keywords = keywords.iter().map(|keyword| keyword.to_string()).collect();
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let query = query.to_lowercase();
        std::iter::once(&self.title)
            .chain(std::iter::once(&self.detail))
            .chain(self.keywords.iter())
            .any(|value| value.to_lowercase().contains(&query))
    }
}

type Provider = Rc<dyn Fn() -> Vec<ActionItem>>;

struct Inner {
    entry: gtk4::Entry,
    popover: gtk4::Popover,
    list: gtk4::ListBox,
    items: RefCell<Vec<ActionItem>>,
    provider: RefCell<Option<Provider>>,
    restore_focus: RefCell<Option<glib::WeakRef<gtk4::Widget>>>,
    highlighted: Cell<i32>,
    running: Cell<bool>,
}

#[derive(Clone)]
pub struct ActionSearch(Rc<Inner>);

impl ActionSearch {
    pub fn new() -> Self {
        let entry = gtk4::Entry::new();
        entry.set_widget_name("database-search-action");
        entry.add_css_class("query-action-field");
        entry.set_hexpand(true);
        entry.set_width_chars(48);
        entry.set_max_width_chars(80);
        entry.set_icon_from_icon_name(
            gtk4::EntryIconPosition::Primary,
            Some("system-search-symbolic"),
        );
        entry.update_property(&[
            gtk4::accessible::Property::Label("Database search and query action"),
            gtk4::accessible::Property::Description("Search actions or display query estimates"),
            gtk4::accessible::Property::KeyShortcuts("Control+l"),
        ]);

        let list = gtk4::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_selection_mode(gtk4::SelectionMode::Single);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_max_content_height(360);
        scroll.set_propagate_natural_height(true);
        scroll.set_child(Some(&list));
        let popover = gtk4::Popover::new();
        popover.set_parent(&entry);
        popover.set_autohide(false);
        popover.set_has_arrow(false);
        popover.set_can_focus(false);
        popover.set_position(gtk4::PositionType::Bottom);
        popover.add_css_class("action-menu");
        popover.set_child(Some(&scroll));
        let weak_popover = popover.downgrade();
        entry.connect_destroy(move |_| {
            if let Some(popover) = weak_popover.upgrade() {
                popover.unparent();
            }
        });

        let search = Self(Rc::new(Inner {
            entry: entry.clone(),
            popover,
            list,
            items: RefCell::new(Vec::new()),
            provider: RefCell::new(None),
            restore_focus: RefCell::new(None),
            highlighted: Cell::new(0),
            running: Cell::new(false),
        }));

        let weak = Rc::downgrade(&search.0);
        entry.connect_changed(move |entry| {
            if let Some(inner) = weak.upgrade() {
                entry.set_icon_from_icon_name(
                    gtk4::EntryIconPosition::Secondary,
                    (!entry.text().is_empty()).then_some("edit-clear-symbolic"),
                );
                let search = ActionSearch(inner);
                if search.focused() || !entry.text().is_empty() {
                    search.refresh();
                }
            }
        });
        entry.connect_icon_release(|entry, position| {
            if position == gtk4::EntryIconPosition::Secondary {
                entry.set_text("");
            }
        });
        let focus = gtk4::EventControllerFocus::new();
        let weak = Rc::downgrade(&search.0);
        focus.connect_enter(move |_| {
            if let Some(inner) = weak.upgrade() {
                ActionSearch(inner).refresh();
            }
        });
        let weak = Rc::downgrade(&search.0);
        focus.connect_leave(move |_| {
            if let Some(inner) = weak.upgrade() {
                let popover = inner.popover.clone();
                let entry = inner.entry.clone();
                glib::idle_add_local_once(move || {
                    let focused = entry
                        .root()
                        .and_then(|root| root.focus())
                        .is_some_and(|focus| focus.is_ancestor(&entry));
                    if !focused {
                        popover.popdown();
                    }
                });
            }
        });
        entry.add_controller(focus);
        let weak = Rc::downgrade(&search.0);
        entry.connect_activate(move |_| {
            if let Some(inner) = weak.upgrade() {
                let search = ActionSearch(inner);
                search.run(search.0.highlighted.get());
            }
        });
        let weak = Rc::downgrade(&search.0);
        search.0.list.connect_row_activated(move |_, row| {
            if let Some(inner) = weak.upgrade() {
                ActionSearch(inner).run(row.index());
            }
        });

        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak = Rc::downgrade(&search.0);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(inner) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let search = ActionSearch(inner);
            match key {
                gdk::Key::Down => {
                    search.highlight(search.0.highlighted.get() + 1);
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    search.highlight(search.0.highlighted.get() - 1);
                    glib::Propagation::Stop
                }
                gdk::Key::Escape => {
                    search.dismiss();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        entry.add_controller(keys);
        search
    }

    fn focused(&self) -> bool {
        self.0
            .entry
            .root()
            .and_then(|root| root.focus())
            .is_some_and(|focus| focus.is_ancestor(&self.0.entry))
    }

    pub fn widget(&self) -> &gtk4::Entry {
        &self.0.entry
    }

    pub fn set_provider(&self, provider: impl Fn() -> Vec<ActionItem> + 'static) {
        self.0.provider.replace(Some(Rc::new(provider)));
    }

    pub fn set_hint(&self, hint: &str, class: Option<&str>) {
        self.0.entry.set_placeholder_text(Some(hint));
        self.0
            .entry
            .update_property(&[gtk4::accessible::Property::Description(hint)]);
        for name in ["guidance", "progress", "ready", "error"] {
            self.0.entry.remove_css_class(name);
        }
        if let Some(class) = class {
            self.0.entry.add_css_class(class);
        }
    }

    pub fn open(&self, text: &str, restore: Option<gtk4::Widget>) -> bool {
        self.0
            .restore_focus
            .replace(restore.map(|widget| widget.downgrade()));
        self.0.entry.set_text(text);
        let focused = self.0.entry.grab_focus();
        self.0.entry.set_position(-1);
        self.refresh();
        focused
    }

    pub fn dismiss(&self) {
        self.0.popover.popdown();
        self.0.entry.set_text("");
        let target = self
            .0
            .restore_focus
            .take()
            .and_then(|widget| widget.upgrade());
        match target {
            Some(widget) if widget.root().is_some() => {
                widget.grab_focus();
            }
            _ => {
                if let Some(window) = self.0.entry.root().and_downcast::<gtk4::Window>() {
                    gtk4::prelude::GtkWindowExt::set_focus(&window, None::<&gtk4::Widget>);
                }
            }
        }
    }

    pub fn refresh(&self) {
        if self.0.running.get() {
            return;
        }
        let provider = self.0.provider.borrow().clone();
        let Some(provider) = provider else {
            return;
        };
        let query = self.0.entry.text().trim().to_string();
        let items = provider()
            .into_iter()
            .filter(|item| item.matches(&query))
            .collect::<Vec<_>>();
        while let Some(child) = self.0.list.first_child() {
            self.0.list.remove(&child);
        }
        for item in &items {
            self.0.list.append(&action_row(self, item, &query));
        }
        self.0.items.replace(items);
        if self.0.items.borrow().is_empty() {
            self.0.popover.popdown();
            return;
        }
        let width = self.0.entry.width().clamp(380, 600);
        self.0.popover.set_size_request(width, -1);
        self.highlight(0);
        self.0.popover.popup();
    }

    fn highlight(&self, index: i32) {
        let count = self.0.items.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let index = index.clamp(0, count - 1);
        self.0.highlighted.set(index);
        if let Some(row) = self.0.list.row_at_index(index) {
            self.0.list.select_row(Some(&row));
        }
    }

    fn run(&self, index: i32) {
        let item = usize::try_from(index)
            .ok()
            .and_then(|index| self.0.items.borrow().get(index).cloned());
        let Some(item) = item else {
            return;
        };
        if !item.enabled {
            return;
        }
        self.0.running.set(true);
        self.0.popover.popdown();
        self.0.entry.set_text("");
        let restore = self
            .0
            .restore_focus
            .take()
            .and_then(|widget| widget.upgrade());
        if let Some(widget) = restore.filter(|widget| widget.root().is_some()) {
            widget.grab_focus();
        } else if let Some(window) = self.0.entry.root().and_downcast::<gtk4::Window>() {
            gtk4::prelude::GtkWindowExt::set_focus(&window, None::<&gtk4::Widget>);
        }
        self.0.running.set(false);
        (item.run)();
    }
}

fn markup(text: &str, query: &str) -> String {
    if query.is_empty() {
        return glib::markup_escape_text(text).to_string();
    }
    let lower = text.to_lowercase();
    match lower.find(&query.to_lowercase()) {
        Some(start)
            if text.is_char_boundary(start) && text.is_char_boundary(start + query.len()) =>
        {
            format!(
                "{}<b>{}</b>{}",
                glib::markup_escape_text(&text[..start]),
                glib::markup_escape_text(&text[start..start + query.len()]),
                glib::markup_escape_text(&text[start + query.len()..])
            )
        }
        _ => glib::markup_escape_text(text).to_string(),
    }
}

fn action_row(search: &ActionSearch, item: &ActionItem, query: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_use_markup(true);
    row.set_title(&markup(&item.title, query));
    row.set_subtitle(&markup(&item.detail, query));
    row.set_activatable(item.enabled);
    row.set_sensitive(item.enabled);
    row.update_property(&[
        gtk4::accessible::Property::Label(&item.title),
        gtk4::accessible::Property::Description(&item.detail),
    ]);
    row.add_prefix(&gtk4::Image::from_icon_name(item.icon));
    let button = gtk4::Button::from_icon_name("go-next-symbolic");
    button.add_css_class("flat");
    button.set_valign(gtk4::Align::Center);
    button.set_can_focus(false);
    button.update_property(&[gtk4::accessible::Property::Label(&format!(
        "Run action: {}",
        item.title
    ))]);
    let weak = Rc::downgrade(&search.0);
    let title = item.title.clone();
    button.connect_clicked(move |_| {
        if let Some(inner) = weak.upgrade() {
            let search = ActionSearch(inner);
            let index = search
                .0
                .items
                .borrow()
                .iter()
                .position(|candidate| candidate.title == title);
            if let Some(index) = index {
                search.run(index as i32);
            }
        }
    });
    row.add_suffix(&button);
    row
}
