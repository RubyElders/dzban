use std::cell::RefCell;
use std::rc::Rc;

use dzban_core::DatabaseBackend;
use dzban_core::health::{HealthSnapshot, HealthState, HealthStatus};
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use libadwaita as adw;

use super::app::ConfigState;
use super::format;
use super::icons;

type NameCallback = Rc<dyn Fn(String)>;
type Callback = Rc<dyn Fn()>;

struct Row {
    name: String,
    row: gtk4::ListBoxRow,
    status: gtk4::Label,
    open: BusyButton,
    test: BusyButton,
}

struct BusyButton {
    button: gtk4::Button,
    stack: gtk4::Stack,
    spinner: gtk4::Spinner,
}

impl BusyButton {
    fn new(label: &str, description: &str) -> Self {
        let stack = gtk4::Stack::new();
        stack.set_hhomogeneous(true);
        stack.add_named(&gtk4::Label::new(Some(label)), Some("label"));
        let spinner = gtk4::Spinner::new();
        spinner.set_halign(gtk4::Align::Center);
        stack.add_named(&spinner, Some("busy"));
        let button = gtk4::Button::new();
        button.set_child(Some(&stack));
        button.set_valign(gtk4::Align::Center);
        button.update_property(&[
            gtk4::accessible::Property::Label(label),
            gtk4::accessible::Property::Description(description),
        ]);
        Self {
            button,
            stack,
            spinner,
        }
    }

    fn set_busy(&self, busy: bool) {
        self.spinner.set_spinning(busy);
        self.stack
            .set_visible_child_name(if busy { "busy" } else { "label" });
        self.button
            .update_state(&[gtk4::accessible::State::Busy(busy)]);
    }
}

#[derive(Default)]
struct Callbacks {
    open: Option<NameCallback>,
    test: Option<NameCallback>,
    test_all: Option<Callback>,
    reload: Option<Callback>,
    cancel: Option<Callback>,
}

struct Inner {
    root: gtk4::Box,
    stack: gtk4::Stack,
    list: gtk4::ListBox,
    test_all: gtk4::Button,
    empty: adw::StatusPage,
    rows: RefCell<Vec<Row>>,
    callbacks: RefCell<Callbacks>,
    pending: RefCell<Option<String>>,
    health: RefCell<HealthSnapshot>,
    failures: RefCell<Vec<(String, String)>>,
    available: std::cell::Cell<bool>,
}

#[derive(Clone)]
pub struct Chooser(Rc<Inner>);

impl Chooser {
    pub fn new() -> Self {
        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(6)
            .accessible_role(gtk4::AccessibleRole::Group)
            .build();
        root.set_margin_top(12);
        root.set_margin_bottom(12);
        root.set_margin_start(19);
        root.set_margin_end(19);
        root.set_vexpand(true);
        root.set_widget_name("connection-chooser");
        root.update_property(&[gtk4::accessible::Property::Label("Database Connections")]);

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        header.set_margin_bottom(4);
        let title = gtk4::Label::new(Some("Connections"));
        title.add_css_class("heading");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        header.append(&title);
        let test_all = gtk4::Button::with_label("Test All");
        test_all.update_property(&[gtk4::accessible::Property::Label("Test All")]);
        header.append(&test_all);
        root.append(&header);

        let stack = gtk4::Stack::new();
        stack.set_vexpand(true);

        let loading = gtk4::Spinner::new();
        loading.set_spinning(true);
        loading.set_halign(gtk4::Align::Center);
        loading.set_valign(gtk4::Align::Center);
        loading.set_size_request(24, 24);
        stack.add_named(&loading, Some("loading"));

        let list = gtk4::ListBox::new();
        list.add_css_class("boxed-list");
        list.add_css_class("connection-list");
        list.set_selection_mode(gtk4::SelectionMode::Single);
        list.set_valign(gtk4::Align::Start);
        list.update_property(&[gtk4::accessible::Property::Label("Connection profiles")]);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));
        stack.add_named(&scroll, Some("list"));

        let empty = adw::StatusPage::new();
        empty.set_icon_name(Some("network-server-symbolic"));
        empty.set_title("No connection profiles found");
        let reload = gtk4::Button::with_label("Reload Profiles");
        reload.set_halign(gtk4::Align::Center);
        reload.add_css_class("pill");
        empty.set_child(Some(&reload));
        stack.add_named(&empty, Some("empty"));
        root.append(&stack);

        let chooser = Self(Rc::new(Inner {
            root,
            stack,
            list,
            test_all: test_all.clone(),
            empty,
            rows: RefCell::new(Vec::new()),
            callbacks: RefCell::new(Callbacks::default()),
            pending: RefCell::new(None),
            health: RefCell::new(HealthSnapshot::default()),
            failures: RefCell::new(Vec::new()),
            available: std::cell::Cell::new(true),
        }));

        let weak = Rc::downgrade(&chooser.0);
        test_all.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade()
                && let Some(callback) = inner.callbacks.borrow().test_all.clone()
            {
                callback();
            }
        });
        let weak = Rc::downgrade(&chooser.0);
        reload.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade()
                && let Some(callback) = inner.callbacks.borrow().reload.clone()
            {
                callback();
            }
        });
        let weak = Rc::downgrade(&chooser.0);
        chooser.0.list.connect_row_activated(move |_, row| {
            if let Some(inner) = weak.upgrade() {
                Chooser(inner).activate(row, true);
            }
        });

        let keys = gtk4::EventControllerKey::new();
        let weak = Rc::downgrade(&chooser.0);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(inner) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let chooser = Chooser(inner);
            if !modifiers.is_empty() && modifiers != gdk::ModifierType::SHIFT_MASK {
                return glib::Propagation::Proceed;
            }
            match key {
                gdk::Key::space => {
                    if let Some(row) = chooser.0.list.selected_row() {
                        chooser.activate(&row, false);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Escape => {
                    let callback = chooser.0.callbacks.borrow().cancel.clone();
                    if let Some(callback) = callback {
                        callback();
                        return glib::Propagation::Stop;
                    }
                    glib::Propagation::Proceed
                }
                _ => glib::Propagation::Proceed,
            }
        });
        chooser.0.root.add_controller(keys);
        chooser.0.stack.set_visible_child_name("loading");
        chooser
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.0.root
    }

    pub fn connect_open(&self, callback: impl Fn(String) + 'static) {
        self.0.callbacks.borrow_mut().open = Some(Rc::new(callback));
    }

    pub fn connect_test(&self, callback: impl Fn(String) + 'static) {
        self.0.callbacks.borrow_mut().test = Some(Rc::new(callback));
    }

    pub fn connect_test_all(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().test_all = Some(Rc::new(callback));
    }

    pub fn connect_reload(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().reload = Some(Rc::new(callback));
    }

    pub fn connect_cancel(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().cancel = Some(Rc::new(callback));
    }

    fn activate(&self, row: &gtk4::ListBoxRow, open: bool) {
        let name = self
            .0
            .rows
            .borrow()
            .iter()
            .find(|candidate| &candidate.row == row)
            .map(|row| row.name.clone());
        let Some(name) = name else {
            return;
        };
        if open {
            self.open(name);
        } else {
            self.test(name);
        }
    }

    fn open(&self, name: String) {
        if self.0.pending.borrow().is_some() || !self.0.available.get() {
            return;
        }
        let callback = self.0.callbacks.borrow().open.clone();
        if let Some(callback) = callback {
            callback(name);
        }
    }

    fn test(&self, name: String) {
        if self
            .profile_status(&name)
            .is_some_and(|status| status.probing)
            || !self.0.available.get()
        {
            return;
        }
        let callback = self.0.callbacks.borrow().test.clone();
        if let Some(callback) = callback {
            callback(name);
        }
    }

    pub fn set_profiles(&self, state: &ConfigState, profiles: &[(String, DatabaseBackend)]) {
        let selected = self
            .0
            .list
            .selected_row()
            .and_then(|row| self.row_name(&row));
        while let Some(child) = self.0.list.first_child() {
            self.0.list.remove(&child);
        }
        self.0.rows.borrow_mut().clear();
        match state {
            ConfigState::Loading => {
                self.0.stack.set_visible_child_name("loading");
                return;
            }
            ConfigState::Failed(error) => {
                self.0
                    .empty
                    .set_title("Connection profiles are unavailable");
                self.0.empty.set_description(Some(error));
                self.0.stack.set_visible_child_name("empty");
                return;
            }
            ConfigState::Ready(_) if profiles.is_empty() => {
                self.0.empty.set_title("No connection profiles found");
                self.0
                    .empty
                    .set_description(Some("Add database profiles to config.toml, then reload."));
                self.0.stack.set_visible_child_name("empty");
                return;
            }
            ConfigState::Ready(_) => {}
        }
        for (name, backend) in profiles {
            let row = self.create_row(name, *backend);
            self.0.list.append(&row.row);
            self.0.rows.borrow_mut().push(row);
        }
        let rows = self.0.rows.borrow();
        let row = selected
            .and_then(|selected| rows.iter().find(|row| row.name == selected))
            .or_else(|| rows.first());
        if let Some(row) = row {
            self.0.list.select_row(Some(&row.row));
        }
        drop(rows);
        self.0.stack.set_visible_child_name("list");
        self.refresh();
    }

    fn create_row(&self, name: &str, backend: DatabaseBackend) -> Row {
        let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_size_request(-1, 38);

        let icon = gtk4::Image::from_icon_name(icons::database_backend(backend));
        icon.set_pixel_size(18);
        icon.update_property(&[gtk4::accessible::Property::Label(
            icons::database_backend_label(backend),
        )]);
        content.append(&icon);

        let label = gtk4::Label::new(Some(name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        content.append(&label);

        let status = gtk4::Label::new(None);
        status.set_xalign(1.0);
        status.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        content.append(&status);

        let open = BusyButton::new("Open", &format!("Connect to {name}"));
        content.append(&open.button);
        let test = BusyButton::new("Test", &format!("Test the {name} connection"));
        content.append(&test.button);

        let row = gtk4::ListBoxRow::new();
        row.set_child(Some(&content));
        row.update_property(&[gtk4::accessible::Property::Label(name)]);

        let weak = Rc::downgrade(&self.0);
        let profile = name.to_string();
        open.button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Chooser(inner).open(profile.clone());
            }
        });
        let weak = Rc::downgrade(&self.0);
        let profile = name.to_string();
        test.button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Chooser(inner).test(profile.clone());
            }
        });
        Row {
            name: name.to_string(),
            row,
            status,
            open,
            test,
        }
    }

    fn row_name(&self, row: &gtk4::ListBoxRow) -> Option<String> {
        self.0
            .rows
            .borrow()
            .iter()
            .find(|candidate| &candidate.row == row)
            .map(|row| row.name.clone())
    }

    fn profile_status(&self, name: &str) -> Option<HealthStatus> {
        self.0
            .health
            .borrow()
            .profiles
            .iter()
            .find(|profile| profile.profile == name)
            .map(|profile| profile.status.clone())
    }

    pub fn apply_health(&self, snapshot: &HealthSnapshot) {
        self.0.health.replace(snapshot.clone());
        self.refresh();
    }

    pub fn set_pending(&self, profile: Option<&str>) {
        self.0.pending.replace(profile.map(str::to_string));
        self.refresh();
    }

    pub fn set_available(&self, available: bool) {
        self.0.available.set(available);
        self.refresh();
    }

    pub fn record_failure(&self, profile: &str, message: &str) {
        let mut failures = self.0.failures.borrow_mut();
        failures.retain(|(name, _)| name != profile);
        failures.push((profile.to_string(), message.to_string()));
        drop(failures);
        self.refresh();
    }

    fn refresh(&self) {
        let pending = self.0.pending.borrow().clone();
        let available = self.0.available.get();
        self.0.test_all.set_sensitive(available);
        for row in self.0.rows.borrow().iter() {
            let status = self.profile_status(&row.name);
            let failure = self
                .0
                .failures
                .borrow()
                .iter()
                .find(|(name, _)| name == &row.name)
                .map(|(_, message)| message.clone())
                .filter(|_| {
                    status
                        .as_ref()
                        .is_none_or(|status| status.state != HealthState::Connected)
                });
            let probing = status.as_ref().is_some_and(|status| status.probing);
            let connecting = pending.as_deref() == Some(row.name.as_str());
            row.open.set_busy(connecting);
            row.test.set_busy(probing);
            let (text, error, tooltip) = match (&status, &failure) {
                _ if connecting => ("Connecting".into(), false, None),
                (Some(status), _) if status.probing => (String::new(), false, None),
                (_, Some(message)) => (message.clone(), true, Some(message.clone())),
                (Some(status), None) => match &status.error {
                    Some(error) => (error.clone(), true, Some(error.clone())),
                    None => (
                        format!(
                            "{} - {}",
                            format::health_label(status.state),
                            format::latency(status.latency_micros)
                        ),
                        false,
                        status.version.clone(),
                    ),
                },
                (None, None) => (String::new(), false, None),
            };
            row.status.set_text(&text);
            row.status.set_tooltip_text(tooltip.as_deref());
            if error {
                row.status.add_css_class("error");
            } else {
                row.status.remove_css_class("error");
            }
            row.open
                .button
                .set_sensitive(available && (pending.is_none() || connecting));
            row.test.button.set_sensitive(available);
            row.row.set_activatable(pending.is_none() && available);
        }
    }

    pub fn navigate(&self, direction: super::navigation::Direction) -> bool {
        match direction.vertical() {
            Some(delta) if super::navigation::contains_focus(&self.0.root) => {
                super::navigation::move_list(&self.0.list, delta)
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub fn selected_index(&self) -> Option<i32> {
        self.0.list.selected_row().map(|row| row.index())
    }

    pub fn focus(&self) -> bool {
        if let Some(row) = self
            .0
            .list
            .selected_row()
            .or_else(|| self.0.list.row_at_index(0))
        {
            self.0.list.select_row(Some(&row));
            return row.grab_focus();
        }
        false
    }
}
