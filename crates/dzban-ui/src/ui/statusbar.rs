use gtk4::prelude::*;
use gtk4::{Box, Label, Orientation, Separator};
use std::cell::RefCell;
use std::rc::Rc;
use super::connection_state::ConnectionStateWidget;
use dzban_db::DbConnection;

struct StatusbarBinding {
    connection: DbConnection,
    handlers: Vec<gtk4::glib::SignalHandlerId>,
}

pub struct Statusbar {
    container: Box,
    connection_widget: ConnectionStateWidget,
    exec_time: Label,
    rows_info: Label,
    server_version: Label,
    latency: Label,
    binding: Rc<RefCell<Option<StatusbarBinding>>>,
}

impl Statusbar {
    pub fn new() -> Self {
        let statusbar = Box::new(Orientation::Horizontal, 12);
        statusbar.set_margin_start(12);
        statusbar.set_margin_end(12);
        statusbar.set_margin_top(6);
        statusbar.set_margin_bottom(6);
        statusbar.add_css_class("toolbar");
        
        // Connection status with state widget
        let connection_widget = ConnectionStateWidget::new();
        statusbar.append(connection_widget.get_widget());
        
        // Separator
        let sep1 = Separator::new(Orientation::Vertical);
        statusbar.append(&sep1);
        
        // Query execution time
        let exec_time = Label::new(Some("⏱️  --"));
        exec_time.add_css_class("dim-label");
        exec_time.add_css_class("caption");
        statusbar.append(&exec_time);
        
        // Separator
        let sep2 = Separator::new(Orientation::Vertical);
        statusbar.append(&sep2);
        
        // Rows affected/returned
        let rows_info = Label::new(Some("📊 -- rows"));
        rows_info.add_css_class("dim-label");
        rows_info.add_css_class("caption");
        statusbar.append(&rows_info);
        
        // Spacer to push next items to the right
        let spacer = Box::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        statusbar.append(&spacer);
        
        // Latency
        let latency = Label::new(Some("📡 --"));
        latency.add_css_class("dim-label");
        latency.add_css_class("caption");
        statusbar.append(&latency);
        
        // Separator
        let sep3 = Separator::new(Orientation::Vertical);
        statusbar.append(&sep3);
        
        // Server version
        let server_version = Label::new(Some("--"));
        server_version.add_css_class("dim-label");
        server_version.add_css_class("caption");
        statusbar.append(&server_version);
        
        Self {
            container: statusbar,
            connection_widget,
            exec_time,
            rows_info,
            server_version,
            latency,
            binding: Rc::new(RefCell::new(None)),
        }
    }
    
    pub fn get_widget(&self) -> &Box {
        &self.container
    }
    
    pub fn bind_to_connection(&self, db_name: &str, connection: &DbConnection) {
        self.clear_binding();
        let db_name = db_name.to_string();
        let statusbar = self.clone();
        let latency_handler = connection.connect_notify_local(Some("latency-micros"), move |connection, _| {
            let latency = connection.latency();
            statusbar.update_latency(latency);
        });
        
        let statusbar = self.clone();
        let version_handler = connection.connect_notify_local(Some("version"), move |connection, _| {
            let version = connection.version();
            statusbar.update_version(version.as_deref());
        });
        
        let statusbar = self.clone();
        let db_name_for_state = db_name.clone();
        let state_handler = connection.connect_notify_local(Some("state"), move |connection, _| {
            let state = connection.state();
            let db_name = db_name_for_state.clone();
            statusbar.update_connection(Some(&db_name), state);
        });

        *self.binding.borrow_mut() = Some(StatusbarBinding {
            connection: connection.clone(),
            handlers: vec![latency_handler, version_handler, state_handler],
        });
        
        // Set initial values
        self.update_latency(connection.latency());
        self.update_version(connection.version().as_deref());
        self.update_connection(Some(&db_name), connection.state());
    }
    
    /// Unbind from current model (if switching tabs or closing connection)
    pub fn unbind(&self) {
        self.clear_binding();
        self.update_connection(None, dzban_db::ConnectionState::Disconnected);
        self.update_version(None);
        self.update_latency(None);
        self.update_exec_time(None);
        self.update_rows(None);
    }

    fn clear_binding(&self) {
        if let Some(binding) = self.binding.borrow_mut().take() {
            for handler in binding.handlers {
                gtk4::glib::signal::signal_handler_disconnect(&binding.connection, handler);
            }
        }
    }
    
    pub fn update_connection(&self, db_name: Option<&str>, state: dzban_db::ConnectionState) {
        if let Some(name) = db_name {
            let text = name.to_string();
            self.connection_widget.update_state(state, Some(&text));
        } else {
            self.connection_widget.update_state(dzban_db::ConnectionState::Disconnected, Some("No database"));
        }
    }
    
    pub fn update_version(&self, version: Option<&str>) {
        if let Some(v) = version {
            self.server_version.set_text(v);
        } else {
            self.server_version.set_text("--");
        }
    }
    
    pub fn update_latency(&self, latency: Option<std::time::Duration>) {
        if let Some(duration) = latency {
            let formatted = format_latency(duration);
            self.latency.set_text(&format!("📡 {}", formatted));
        } else {
            self.latency.set_text("📡 --");
        }
    }
    
    pub fn update_exec_time(&self, seconds: Option<f64>) {
        if let Some(s) = seconds {
            self.exec_time.set_text(&format!("⏱️  {:.2}s", s));
        } else {
            self.exec_time.set_text("⏱️  --");
        }
    }
    
    pub fn update_rows(&self, count: Option<usize>) {
        if let Some(n) = count {
            self.rows_info.set_text(&format!("📊 {} rows", n));
        } else {
            self.rows_info.set_text("📊 -- rows");
        }
    }
}

impl Clone for Statusbar {
    fn clone(&self) -> Self {
        Self {
            container: self.container.clone(),
            connection_widget: self.connection_widget.clone(),
            exec_time: self.exec_time.clone(),
            rows_info: self.rows_info.clone(),
            server_version: self.server_version.clone(),
            latency: self.latency.clone(),
            binding: self.binding.clone(),
        }
    }
}

pub fn create_statusbar() -> Statusbar {
    Statusbar::new()
}

/// Format latency with dynamic units
/// - < 1ms: show in microseconds (µs)
/// - < 1000ms: show in milliseconds (ms)
/// - >= 1000ms: show in seconds (s)
fn format_latency(duration: std::time::Duration) -> String {
    let micros = duration.as_micros();
    
    if micros < 1000 {
        // Less than 1ms - show in microseconds
        format!("{}µs", micros)
    } else if micros < 1_000_000 {
        // Less than 1 second - show in milliseconds
        let ms = duration.as_millis();
        format!("{}ms", ms)
    } else {
        // 1 second or more - show in seconds with 2 decimal places
        let seconds = duration.as_secs_f64();
        format!("{:.2}s", seconds)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn rebinding_disconnects_the_previous_connection() {
        let statusbar = Statusbar::new();
        let first = DbConnection::new("host=first".to_string());
        let second = DbConnection::new("host=second".to_string());

        statusbar.bind_to_connection("first", &first);
        statusbar.bind_to_connection("second", &second);
        first.notify("state");

        assert_eq!(statusbar.connection_widget.text(), "second");
    }
}
