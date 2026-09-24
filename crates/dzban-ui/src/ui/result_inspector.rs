use gtk4::prelude::*;
use gtk4::{Box, Button, Label, Orientation, ScrolledWindow};
use std::cell::RefCell;
use std::rc::Rc;

use super::preview_overlay::PreviewOverlay;

#[derive(Clone)]
pub struct ResultInspector {
    container: Box,
    values: Box,
    current_values: Rc<RefCell<Vec<(String, String)>>>,
    pin_button: Button,
    visibility: PreviewOverlay,
}

impl ResultInspector {
    pub fn new() -> Self {
        let container = Box::new(Orientation::Vertical, 0);
        container.set_width_request(560);
        container.set_vexpand(true);
        container.set_halign(gtk4::Align::End);
        container.set_valign(gtk4::Align::Fill);
        container.set_margin_top(12);
        container.set_margin_bottom(12);
        container.set_margin_end(12);
        container.add_css_class("card");
        container.add_css_class("result-inspector");
        container.set_visible(false);
        container.update_property(&[
            gtk4::accessible::Property::Label("Result row preview"),
            gtk4::accessible::Property::Description("Complete values of the selected result row"),
        ]);

        let header = Box::new(Orientation::Horizontal, 8);
        header.set_margin_start(12);
        header.set_margin_end(8);
        header.set_margin_top(8);
        header.set_margin_bottom(8);
        let title = Label::new(Some("Result Row"));
        title.add_css_class("heading");
        title.set_halign(gtk4::Align::Start);
        title.set_hexpand(true);
        header.append(&title);

        let pin_button = Button::from_icon_name("view-pin-symbolic");
        pin_button.add_css_class("flat");
        pin_button.set_tooltip_text(Some("Keep result row open"));
        pin_button.update_property(&[gtk4::accessible::Property::Label("Keep result row open")]);
        header.append(&pin_button);

        let close_button = Button::from_icon_name("window-close-symbolic");
        close_button.add_css_class("flat");
        close_button.set_tooltip_text(Some("Close result row"));
        close_button.update_property(&[gtk4::accessible::Property::Label("Close result row")]);
        header.append(&close_button);
        container.append(&header);
        container.append(&gtk4::Separator::new(Orientation::Horizontal));

        let scrolled = ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hexpand(true);
        let values = Box::new(Orientation::Vertical, 0);
        values.set_focusable(true);
        values.update_property(&[gtk4::accessible::Property::Label("Complete result row")]);
        scrolled.set_child(Some(&values));
        container.append(&scrolled);

        let visibility = PreviewOverlay::new(&container);
        let inspector = Self {
            container,
            values,
            current_values: Rc::new(RefCell::new(Vec::new())),
            pin_button,
            visibility,
        };
        let inspector_for_pin = inspector.clone();
        inspector.pin_button.connect_clicked(move |_| {
            if inspector_for_pin.is_pinned() {
                inspector_for_pin.close();
            } else {
                inspector_for_pin.pin();
            }
        });
        let inspector_for_close = inspector.clone();
        close_button.connect_clicked(move |_| {
            inspector_for_close.close();
        });
        inspector
    }

    pub fn widget(&self) -> &Box {
        &self.container
    }

    pub fn show_row(&self, values: &[(String, String)]) {
        while let Some(child) = self.values.first_child() {
            self.values.remove(&child);
        }
        for (key, value) in values {
            let row = Box::new(Orientation::Horizontal, 16);
            row.set_margin_start(12);
            row.set_margin_end(12);
            row.set_margin_top(8);
            row.set_margin_bottom(8);

            let key_label = Label::new(Some(key));
            key_label.set_width_chars(18);
            key_label.set_max_width_chars(24);
            key_label.set_xalign(1.0);
            key_label.set_valign(gtk4::Align::Start);
            key_label.add_css_class("dim-label");
            row.append(&key_label);

            let value_label = Label::new(Some(value));
            value_label.set_xalign(0.0);
            value_label.set_halign(gtk4::Align::Fill);
            value_label.set_hexpand(true);
            value_label.set_selectable(true);
            value_label.set_wrap(true);
            value_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
            row.append(&value_label);
            self.values.append(&row);
            self.values
                .append(&gtk4::Separator::new(Orientation::Horizontal));
        }
        *self.current_values.borrow_mut() = values.to_vec();
        self.visibility.show_transient();
    }

    pub fn hide_transient(&self) {
        self.visibility.hide_transient();
    }

    pub fn pin(&self) -> bool {
        if !self.visibility.pin() {
            return false;
        }
        self.pin_button.set_tooltip_text(Some("Close result row"));
        self.values.grab_focus();
        true
    }

    pub fn close(&self) -> bool {
        let closed = self.visibility.close();
        if closed {
            self.pin_button
                .set_tooltip_text(Some("Keep result row open"));
        }
        closed
    }

    #[cfg(test)]
    pub fn is_visible(&self) -> bool {
        self.visibility.is_visible()
    }

    pub fn is_pinned(&self) -> bool {
        self.visibility.is_pinned()
    }

    #[cfg(test)]
    pub fn current_values(&self) -> Vec<(String, String)> {
        self.current_values.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn result_row_uses_transient_and_pinned_preview_states() {
        let inspector = ResultInspector::new();
        inspector.show_row(&[("payload".to_string(), "first line\nsecond line".to_string())]);
        assert!(inspector.is_visible());
        assert!(!inspector.is_pinned());
        inspector.hide_transient();
        assert!(!inspector.is_visible());

        inspector.show_row(&[("payload".to_string(), "full value".to_string())]);
        assert!(inspector.pin());
        inspector.hide_transient();
        assert!(inspector.is_visible());
        assert!(inspector.is_pinned());
        assert!(inspector.close());
        assert!(!inspector.is_visible());
    }
}
