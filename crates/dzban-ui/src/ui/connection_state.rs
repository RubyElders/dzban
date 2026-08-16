use gtk4::prelude::*;
use gtk4::{Box, Orientation, Label};
use std::rc::Rc;
use std::cell::RefCell;
use dzban_db::ConnectionState;

/// A reusable widget showing connection state with a colored dot
pub struct ConnectionStateWidget {
    container: Box,
    status_dot: Label,
    status_text: Label,
    state: Rc<RefCell<ConnectionState>>,
}

impl ConnectionStateWidget {
    pub fn new() -> Self {
        let container = Box::new(Orientation::Horizontal, 4);
        container.set_valign(gtk4::Align::Center);
        
        // Colored dot indicator
        let status_dot = Label::new(Some("●"));
        status_dot.add_css_class("dim-label");
        
        // Status text
        let status_text = Label::new(Some("Not connected"));
        status_text.add_css_class("caption");
        status_text.add_css_class("dim-label");
        
        container.append(&status_dot);
        container.append(&status_text);
        
        Self {
            container,
            status_dot,
            status_text,
            state: Rc::new(RefCell::new(ConnectionState::Disconnected)),
        }
    }
    
    pub fn update_state(&self, state: ConnectionState, text: Option<&str>) {
        *self.state.borrow_mut() = state;
        
        // Remove all color classes
        self.status_dot.remove_css_class("success");
        self.status_dot.remove_css_class("warning");
        self.status_dot.remove_css_class("error");
        self.status_dot.remove_css_class("dim-label");
        
        self.status_text.remove_css_class("success");
        self.status_text.remove_css_class("warning");
        self.status_text.remove_css_class("error");
        self.status_text.remove_css_class("dim-label");
        
        match state {
            ConnectionState::Disconnected => {
                self.status_dot.add_css_class("dim-label");
                self.status_text.add_css_class("dim-label");
                self.status_text.set_text(text.unwrap_or("Not connected"));
            }
            ConnectionState::Connecting => {
                self.status_dot.add_css_class("warning");
                self.status_text.add_css_class("warning");
                self.status_text.set_text(text.unwrap_or("Connecting..."));
            }
            ConnectionState::Connected => {
                self.status_dot.add_css_class("success");
                self.status_text.add_css_class("success");
                self.status_text.set_text(text.unwrap_or("Connected"));
            }
            ConnectionState::Error => {
                self.status_dot.add_css_class("error");
                self.status_text.add_css_class("error");
                self.status_text.set_text(text.unwrap_or("Error"));
            }
        }
    }
    
    pub fn get_widget(&self) -> &Box {
        &self.container
    }

    #[cfg(test)]
    pub fn text(&self) -> gtk4::glib::GString {
        self.status_text.text()
    }
    
    #[allow(dead_code)]
    pub fn get_state(&self) -> ConnectionState {
        *self.state.borrow()
    }
}

impl Clone for ConnectionStateWidget {
    fn clone(&self) -> Self {
        Self {
            container: self.container.clone(),
            status_dot: self.status_dot.clone(),
            status_text: self.status_text.clone(),
            state: Rc::clone(&self.state),
        }
    }
}
