use std::ops::Range;

use gtk4::prelude::*;
use libadwaita as adw;
use sourceview5::prelude::*;
use sourceview5::{Buffer, LanguageManager, StyleSchemeManager, View};

#[derive(Clone)]
pub struct Editor {
    root: gtk4::Overlay,
    view: View,
    buffer: Buffer,
    placeholder: gtk4::Label,
}

fn apply_scheme(buffer: &Buffer, dark: bool) {
    let name = if dark { "Adwaita-dark" } else { "Adwaita" };
    if let Some(scheme) = StyleSchemeManager::default().scheme(name) {
        buffer.set_style_scheme(Some(&scheme));
    }
}

impl Editor {
    pub fn new() -> Self {
        let buffer = match LanguageManager::default().language("sql") {
            Some(language) => Buffer::with_language(&language),
            None => Buffer::new(None),
        };
        let style = adw::StyleManager::default();
        apply_scheme(&buffer, style.is_dark());
        let weak = buffer.downgrade();
        style.connect_dark_notify(move |style| {
            if let Some(buffer) = weak.upgrade() {
                apply_scheme(&buffer, style.is_dark());
            }
        });

        let view = View::with_buffer(&buffer);
        view.set_widget_name("sql-query-editor");
        view.update_property(&[
            gtk4::accessible::Property::Label("SQL query editor"),
            gtk4::accessible::Property::Description("Write SQL statements to preview or execute"),
            gtk4::accessible::Property::MultiLine(true),
        ]);
        view.set_monospace(true);
        view.set_left_margin(26);
        view.set_right_margin(26);
        view.set_top_margin(12);
        view.set_bottom_margin(12);
        view.set_wrap_mode(gtk4::WrapMode::WordChar);
        view.set_highlight_current_line(false);
        view.set_tab_width(4);
        view.set_insert_spaces_instead_of_tabs(true);
        view.set_auto_indent(true);
        view.add_css_class("query-editor");

        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_hexpand(true);
        scroll.set_child(Some(&view));

        let placeholder = gtk4::Label::new(Some("Write a SQL query..."));
        placeholder.add_css_class("dim-label");
        placeholder.add_css_class("monospace");
        placeholder.set_halign(gtk4::Align::Start);
        placeholder.set_valign(gtk4::Align::Start);
        placeholder.set_margin_start(26);
        placeholder.set_margin_top(12);
        placeholder.set_can_target(false);

        let root = gtk4::Overlay::new();
        root.set_child(Some(&scroll));
        root.add_overlay(&placeholder);
        root.set_size_request(-1, 120);

        let editor = Self {
            root,
            view,
            buffer,
            placeholder,
        };
        let placeholder = editor.placeholder.clone();
        editor.buffer.connect_changed(move |buffer| {
            placeholder.set_visible(buffer.char_count() == 0);
        });
        editor
    }

    pub fn widget(&self) -> &gtk4::Overlay {
        &self.root
    }

    pub fn text(&self) -> String {
        let (start, end) = self.buffer.bounds();
        self.buffer.text(&start, &end, true).to_string()
    }

    pub fn set_text(&self, text: &str) {
        if self.text() != text {
            self.buffer.set_text(text);
        }
        self.placeholder.set_visible(text.is_empty());
    }

    pub fn scalar_selection(&self) -> Range<usize> {
        let (start, end) = self.buffer.selection_bounds().unwrap_or_else(|| {
            let cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
            (cursor, cursor)
        });
        let (start, end) = (start.offset() as usize, end.offset() as usize);
        start.min(end)..start.max(end)
    }

    pub fn focus(&self) -> bool {
        self.view.grab_focus()
    }

    pub fn has_focus(&self) -> bool {
        self.view.has_focus()
    }

    pub fn connect_changed(&self, callback: impl Fn() + 'static) {
        self.buffer.connect_changed(move |_| callback());
    }

    pub fn connect_context_changed(&self, callback: impl Fn() + 'static) {
        self.buffer
            .connect_notify_local(Some("cursor-position"), move |_, _| callback());
    }
}
