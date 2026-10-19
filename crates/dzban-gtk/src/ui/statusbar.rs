use std::cell::RefCell;
use std::rc::Rc;

use dzban_core::DatabaseBackend;
use dzban_core::health::{HealthState, HealthStatus};
use dzban_core::presentation::DocumentPresentation;
use gtk4::prelude::*;

use super::format;
use super::icons;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionDisplay {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

impl ConnectionDisplay {
    fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Error => "Error",
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Disconnected => "status-disconnected",
            Self::Connecting => "status-connecting",
            Self::Connected => "status-connected",
            Self::Error => "status-error",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FooterState {
    pub profile: Option<String>,
    pub backend: Option<DatabaseBackend>,
    pub connecting: bool,
    pub health: Option<HealthStatus>,
    pub failure: Option<String>,
    pub document: Option<DocumentPresentation>,
    pub loading: bool,
    pub copying: bool,
    pub ping_available: bool,
}

impl FooterState {
    pub fn connection(&self) -> ConnectionDisplay {
        if self.connecting {
            return ConnectionDisplay::Connecting;
        }
        if self.failure.is_some() {
            return ConnectionDisplay::Error;
        }
        match self.health.as_ref().map(|health| health.state) {
            Some(HealthState::Connected) => ConnectionDisplay::Connected,
            Some(HealthState::Error) => ConnectionDisplay::Error,
            _ if self.profile.is_some() && self.ping_available => ConnectionDisplay::Connected,
            _ => ConnectionDisplay::Disconnected,
        }
    }

    pub fn connection_text(&self) -> String {
        let name = self.profile.clone().unwrap_or_else(|| "No database".into());
        if self.connection() == ConnectionDisplay::Error {
            let cause = self
                .failure
                .clone()
                .or_else(|| self.health.as_ref().and_then(|health| health.error.clone()))
                .unwrap_or_else(|| "connection failed".into());
            format!("{name}: {cause}")
        } else {
            name
        }
    }
}

type Callback = Rc<dyn Fn()>;

#[derive(Default)]
struct Callbacks {
    ping: Option<Callback>,
    saved_plan: Option<Callback>,
    cancel_copy: Option<Callback>,
    shortcuts: Option<Callback>,
}

struct Inner {
    root: gtk4::Box,
    dot: gtk4::Box,
    connection: gtk4::Label,
    duration: gtk4::Label,
    status: gtk4::Label,
    estimate: gtk4::Label,
    cost: gtk4::Label,
    sample: gtk4::Label,
    saved_plan: gtk4::Button,
    loading: gtk4::Spinner,
    cancel_copy: gtk4::Button,
    ping: gtk4::Button,
    ping_spinner: gtk4::Spinner,
    ping_icon: gtk4::Image,
    latency: gtk4::Label,
    server_icon: gtk4::Image,
    version: gtk4::Label,
    callbacks: RefCell<Callbacks>,
}

#[derive(Clone)]
pub struct Statusbar(Rc<Inner>);

fn divider() -> gtk4::Separator {
    let separator = gtk4::Separator::new(gtk4::Orientation::Vertical);
    separator.set_size_request(-1, 14);
    separator.set_valign(gtk4::Align::Center);
    separator
}

fn caption(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("caption");
    label.add_css_class("numeric");
    label
}

fn icon(name: &str, label: &str) -> gtk4::Image {
    let image = gtk4::Image::from_icon_name(name);
    image.set_pixel_size(14);
    image.add_css_class("dim-label");
    image.update_property(&[gtk4::accessible::Property::Label(label)]);
    image
}

fn connect_button(
    button: &gtk4::Button,
    inner: &Rc<Inner>,
    pick: fn(&Callbacks) -> Option<Callback>,
) {
    let weak = Rc::downgrade(inner);
    button.connect_clicked(move |_| {
        if let Some(callback) = weak
            .upgrade()
            .and_then(|inner| pick(&inner.callbacks.borrow()))
        {
            callback();
        }
    });
}

impl Statusbar {
    pub fn new() -> Self {
        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(12)
            .accessible_role(gtk4::AccessibleRole::Group)
            .build();
        root.add_css_class("workspace-statusbar");
        root.set_size_request(-1, 30);
        root.update_property(&[gtk4::accessible::Property::Label("Workspace status")]);

        let connection_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
        let dot = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        dot.add_css_class("status-dot");
        dot.set_valign(gtk4::Align::Center);
        connection_box.append(&dot);
        let connection = caption("No database");
        connection.set_max_width_chars(28);
        connection.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        connection_box.append(&connection);
        root.append(&connection_box);
        root.append(&divider());

        let duration_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
        duration_box.set_tooltip_text(Some("Query execution time"));
        duration_box.append(&icon(icons::QUERY_TIME, "Query execution time"));
        let duration = caption("--");
        duration_box.append(&duration);
        root.append(&duration_box);
        root.append(&divider());

        let result_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        result_box.append(&icon(icons::RESULT_ROWS, "Returned rows"));
        let status = caption("Run a query to see results");
        status.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        status.update_property(&[gtk4::accessible::Property::Description("Result status")]);
        result_box.append(&status);
        let estimate = caption("");
        estimate.add_css_class("dim-label");
        result_box.append(&estimate);
        let cost = caption("");
        cost.add_css_class("dim-label");
        result_box.append(&cost);
        let sample = caption("Text sample");
        sample.add_css_class("dim-label");
        sample.set_tooltip_text(Some(
            "Saved history keeps text values without SQL types or NULL identity",
        ));
        result_box.append(&sample);
        let saved_plan = gtk4::Button::with_label("Saved Plan");
        saved_plan.add_css_class("flat");
        saved_plan.add_css_class("caption");
        saved_plan.set_tooltip_text(Some("Show the saved query plan"));
        result_box.append(&saved_plan);
        let loading = gtk4::Spinner::new();
        result_box.append(&loading);
        let cancel_copy = gtk4::Button::with_label("Cancel Copy");
        cancel_copy.add_css_class("flat");
        cancel_copy.add_css_class("caption");
        result_box.append(&cancel_copy);
        root.append(&result_box);

        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        root.append(&spacer);

        let ping = gtk4::Button::new();
        ping.add_css_class("flat");
        ping.set_tooltip_text(Some("Database ping latency. Click to probe now."));
        ping.update_property(&[gtk4::accessible::Property::Label("Database ping latency")]);
        let ping_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
        let ping_spinner = gtk4::Spinner::new();
        ping_spinner.set_visible(false);
        ping_content.append(&ping_spinner);
        let ping_icon = icon(icons::PING, "Database ping latency");
        ping_content.append(&ping_icon);
        let latency = caption("--");
        ping_content.append(&latency);
        ping.set_child(Some(&ping_content));
        root.append(&ping);
        root.append(&divider());

        let server_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
        let server_icon = icon(icons::POSTGRESQL, "PostgreSQL database server");
        server_icon.remove_css_class("dim-label");
        server_icon.set_visible(false);
        server_box.append(&server_icon);
        let version = caption("--");
        server_box.append(&version);
        root.append(&server_box);

        let shortcuts = gtk4::Button::from_icon_name("input-keyboard-symbolic");
        shortcuts.add_css_class("flat");
        shortcuts.set_tooltip_text(Some("Keyboard Shortcuts (Ctrl+?)"));
        shortcuts.update_property(&[gtk4::accessible::Property::Label("Keyboard shortcuts")]);
        root.append(&shortcuts);

        let statusbar = Self(Rc::new(Inner {
            root,
            dot,
            connection,
            duration,
            status,
            estimate,
            cost,
            sample,
            saved_plan: saved_plan.clone(),
            loading,
            cancel_copy: cancel_copy.clone(),
            ping: ping.clone(),
            ping_spinner,
            ping_icon,
            latency,
            server_icon,
            version,
            callbacks: RefCell::new(Callbacks::default()),
        }));
        connect_button(&ping, &statusbar.0, |callbacks| callbacks.ping.clone());
        connect_button(&saved_plan, &statusbar.0, |callbacks| {
            callbacks.saved_plan.clone()
        });
        connect_button(&cancel_copy, &statusbar.0, |callbacks| {
            callbacks.cancel_copy.clone()
        });
        connect_button(&shortcuts, &statusbar.0, |callbacks| {
            callbacks.shortcuts.clone()
        });
        statusbar.update(&FooterState::default());
        statusbar
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.0.root
    }

    pub fn connect_ping(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().ping = Some(Rc::new(callback));
    }

    pub fn connect_saved_plan(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().saved_plan = Some(Rc::new(callback));
    }

    pub fn connect_cancel_copy(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().cancel_copy = Some(Rc::new(callback));
    }

    pub fn connect_shortcuts(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().shortcuts = Some(Rc::new(callback));
    }

    pub fn update(&self, state: &FooterState) {
        let connection = state.connection();
        for display in [
            ConnectionDisplay::Disconnected,
            ConnectionDisplay::Connecting,
            ConnectionDisplay::Connected,
            ConnectionDisplay::Error,
        ] {
            self.0.dot.remove_css_class(display.class());
        }
        self.0.dot.add_css_class(connection.class());
        self.0
            .dot
            .update_property(&[gtk4::accessible::Property::Label(connection.label())]);
        let text = state.connection_text();
        self.0.connection.set_text(&text);
        self.0
            .connection
            .set_tooltip_text(Some(&format!("{}: {text}", connection.label())));

        let document = state.document.as_ref();
        self.0.duration.set_text(
            &document
                .and_then(|document| document.execution_seconds)
                .map(|seconds| format!("{seconds:.2}s"))
                .unwrap_or_else(|| "--".into()),
        );
        self.0.status.set_text(
            &document
                .map(format::result_status)
                .unwrap_or_else(|| "Run a query to see results".into()),
        );
        let plan = document.and_then(|document| document.execution_plan.as_ref());
        let estimate = plan
            .and_then(|plan| {
                plan.estimate
                    .rows
                    .map(|rows| format::row_estimate(plan.backend, rows))
            })
            .unwrap_or_default();
        self.0.estimate.set_text(&estimate);
        self.0.estimate.set_visible(!estimate.is_empty());
        let finished = document.is_some_and(|document| document.feedback.progress.is_none());
        let cost = plan
            .filter(|_| finished)
            .and_then(
                |plan| match (plan.estimate.startup_cost, plan.estimate.total_cost) {
                    (Some(startup), Some(total)) if startup.is_finite() && total.is_finite() => {
                        Some(format!("Cost {startup:.2}..{total:.2}"))
                    }
                    (None, Some(total)) if total.is_finite() => Some(format!("Cost {total:.2}")),
                    _ => None,
                },
            )
            .unwrap_or_default();
        self.0.cost.set_text(&cost);
        self.0.cost.set_visible(!cost.is_empty());
        let saved = document.is_some_and(|document| document.history == Some(true));
        self.0
            .sample
            .set_visible(saved && document.is_some_and(|document| document.rows > 0));
        self.0.saved_plan.set_visible(
            saved && document.is_some_and(|document| document.history_plan_available == Some(true)),
        );
        self.0.loading.set_spinning(state.loading);
        self.0.loading.set_visible(state.loading);
        self.0.cancel_copy.set_visible(state.copying);

        let probing = state.health.as_ref().is_some_and(|health| health.probing);
        self.0.ping_spinner.set_visible(probing);
        self.0.ping_spinner.set_spinning(probing);
        self.0.ping_icon.set_visible(!probing);
        let latency = format::latency(
            state
                .health
                .as_ref()
                .and_then(|health| health.latency_micros),
        );
        self.0.latency.set_text(&latency);
        self.0
            .ping
            .update_property(&[gtk4::accessible::Property::Description(&latency)]);
        self.0.ping.set_sensitive(state.ping_available && !probing);

        match state.backend {
            Some(backend) => {
                self.0
                    .server_icon
                    .set_icon_name(Some(icons::database_backend(backend)));
                self.0
                    .server_icon
                    .update_property(&[gtk4::accessible::Property::Label(match backend {
                        DatabaseBackend::Postgresql => "PostgreSQL database server",
                        DatabaseBackend::Clickhouse => "ClickHouse database server",
                    })]);
                self.0.server_icon.set_visible(true);
            }
            None => self.0.server_icon.set_visible(false),
        }
        let version = state
            .health
            .as_ref()
            .and_then(|health| health.version.clone());
        self.0
            .version
            .set_text(&format::short_version(version.as_deref()));
        self.0.version.set_tooltip_text(Some(
            version.as_deref().unwrap_or("Database server version"),
        ));
    }

    #[cfg(test)]
    pub fn connection_text(&self) -> String {
        self.0.connection.text().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_errors_include_the_database_and_cause() {
        let state = FooterState {
            profile: Some("reporting".into()),
            failure: Some("password authentication failed".into()),
            ..Default::default()
        };
        assert_eq!(state.connection(), ConnectionDisplay::Error);
        assert_eq!(
            state.connection_text(),
            "reporting: password authentication failed"
        );
    }

    #[test]
    fn connecting_takes_precedence_over_stale_health() {
        let state = FooterState {
            profile: Some("reporting".into()),
            connecting: true,
            health: Some(HealthStatus {
                state: HealthState::Error,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(state.connection(), ConnectionDisplay::Connecting);
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn footer_switches_the_server_icon_with_the_backend() {
        let statusbar = Statusbar::new();
        for backend in [DatabaseBackend::Postgresql, DatabaseBackend::Clickhouse] {
            statusbar.update(&FooterState {
                backend: Some(backend),
                ..Default::default()
            });
            assert_eq!(
                statusbar.0.server_icon.icon_name().as_deref(),
                Some(icons::database_backend(backend))
            );
            assert!(statusbar.0.server_icon.is_visible());
        }
        statusbar.update(&FooterState::default());
        assert!(!statusbar.0.server_icon.is_visible());
        assert_eq!(statusbar.connection_text(), "No database");
    }
}
