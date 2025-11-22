use gtk4::prelude::*;
use gtk4::{Box, Label, Orientation, Separator};
use super::connection_state::ConnectionStateWidget;

pub struct Statusbar {
    container: Box,
    connection_widget: ConnectionStateWidget,
    exec_time: Label,
    rows_info: Label,
    server_version: Label,
    latency: Label,
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
        }
    }
    
    pub fn get_widget(&self) -> &Box {
        &self.container
    }
    
    pub fn update_connection(&self, db_name: Option<&str>, state: crate::db::ConnectionState) {
        if let Some(name) = db_name {
            let text = format!("{}", name);
            self.connection_widget.update_state(state, Some(&text));
        } else {
            self.connection_widget.update_state(crate::db::ConnectionState::Disconnected, Some("No database"));
        }
    }
    
    pub fn update_version(&self, version: Option<&str>) {
        if let Some(v) = version {
            self.server_version.set_text(v);
        } else {
            self.server_version.set_text("--");
        }
    }
    
    pub fn update_latency(&self, latency_ms: Option<u64>) {
        if let Some(ms) = latency_ms {
            self.latency.set_text(&format!("📡 {}ms", ms));
        } else {
            self.latency.set_text("📡 --");
        }
    }
    
    #[allow(dead_code)]
    pub fn update_exec_time(&self, seconds: Option<f64>) {
        if let Some(s) = seconds {
            self.exec_time.set_text(&format!("⏱️  {:.2}s", s));
        } else {
            self.exec_time.set_text("⏱️  --");
        }
    }
    
    #[allow(dead_code)]
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
        }
    }
}

pub fn create_statusbar() -> Statusbar {
    Statusbar::new()
}

// TODO: Add methods to update status bar with animated transitions
// - update_connection_status()
// - update_query_stats()
// - show_spinner() for running queries
