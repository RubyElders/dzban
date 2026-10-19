use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{gdk, glib};

use super::navigation::Direction;

type Callback = Rc<dyn Fn()>;
type DirectionCallback = Rc<dyn Fn(i64)>;
type ColumnCallback = Rc<dyn Fn(usize)>;

#[derive(Default)]
struct Callbacks {
    close: Option<Callback>,
    pin: Option<Callback>,
    move_row: Option<DirectionCallback>,
    open_cell: Option<ColumnCallback>,
}

struct Inner {
    container: gtk4::Box,
    title: gtk4::Label,
    stack: gtk4::Stack,
    list: gtk4::ListBox,
    cell_title: gtk4::Label,
    cell_view: gtk4::TextView,
    cell_detail: gtk4::Label,
    pin_button: gtk4::Button,
    row: Cell<Option<usize>>,
    values: RefCell<Vec<(String, Option<String>)>>,
    pinned: Cell<bool>,
    callbacks: RefCell<Callbacks>,
}

#[derive(Clone)]
pub struct RowInspector(Rc<Inner>);

impl RowInspector {
    pub fn new() -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.set_size_request(640, 460);
        container.set_halign(gtk4::Align::End);
        container.set_valign(gtk4::Align::Start);
        container.set_margin_top(12);
        container.set_margin_end(12);
        container.set_margin_bottom(12);
        container.add_css_class("card");
        container.add_css_class("floating-inspector");
        container.set_visible(false);
        container.update_property(&[
            gtk4::accessible::Property::Label("Result row preview"),
            gtk4::accessible::Property::Description("Complete values of the selected result row"),
        ]);

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        header.set_margin_start(12);
        header.set_margin_end(8);
        header.set_margin_top(8);
        header.set_margin_bottom(8);
        let title = gtk4::Label::new(Some("Result Row"));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        header.append(&title);
        let pin_button = gtk4::Button::from_icon_name("view-pin-symbolic");
        pin_button.add_css_class("flat");
        pin_button.set_tooltip_text(Some("Keep result row open"));
        pin_button.update_property(&[gtk4::accessible::Property::Label("Keep result row open")]);
        header.append(&pin_button);
        let close = gtk4::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.set_tooltip_text(Some("Close result row"));
        close.update_property(&[
            gtk4::accessible::Property::Label("Close result row"),
            gtk4::accessible::Property::KeyShortcuts("Escape"),
        ]);
        header.append(&close);
        container.append(&header);
        container.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

        let stack = gtk4::Stack::new();
        stack.set_vexpand(true);
        let list = gtk4::ListBox::new();
        list.add_css_class("workspace-list");
        list.set_selection_mode(gtk4::SelectionMode::Single);
        list.update_property(&[gtk4::accessible::Property::Label("Complete result row")]);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroll.set_child(Some(&list));
        stack.add_named(&scroll, Some("row"));

        let cell = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        cell.set_margin_start(12);
        cell.set_margin_end(12);
        cell.set_margin_top(8);
        cell.set_margin_bottom(8);
        let cell_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let back = gtk4::Button::with_label("Back to Row");
        back.add_css_class("flat");
        cell_header.append(&back);
        let cell_title = gtk4::Label::new(None);
        cell_title.add_css_class("heading");
        cell_title.set_xalign(0.0);
        cell_title.set_hexpand(true);
        cell_header.append(&cell_title);
        cell.append(&cell_header);
        let cell_view = gtk4::TextView::new();
        cell_view.set_editable(false);
        cell_view.set_monospace(true);
        cell_view.set_wrap_mode(gtk4::WrapMode::WordChar);
        cell_view.update_property(&[gtk4::accessible::Property::Label("Cell value")]);
        let cell_scroll = gtk4::ScrolledWindow::new();
        cell_scroll.set_vexpand(true);
        cell_scroll.set_child(Some(&cell_view));
        cell.append(&cell_scroll);
        let cell_detail = gtk4::Label::new(None);
        cell_detail.add_css_class("caption");
        cell_detail.add_css_class("dim-label");
        cell_detail.set_xalign(0.0);
        cell.append(&cell_detail);
        stack.add_named(&cell, Some("cell"));
        container.append(&stack);

        let inspector = Self(Rc::new(Inner {
            container,
            title,
            stack,
            list,
            cell_title,
            cell_view,
            cell_detail,
            pin_button: pin_button.clone(),
            row: Cell::new(None),
            values: RefCell::new(Vec::new()),
            pinned: Cell::new(false),
            callbacks: RefCell::new(Callbacks::default()),
        }));

        let weak = Rc::downgrade(&inspector.0);
        pin_button.connect_clicked(move |_| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let callback = if inner.pinned.get() {
                inner.callbacks.borrow().close.clone()
            } else {
                inner.callbacks.borrow().pin.clone()
            };
            if let Some(callback) = callback {
                callback();
            }
        });
        let weak = Rc::downgrade(&inspector.0);
        close.connect_clicked(move |_| {
            if let Some(callback) = weak
                .upgrade()
                .and_then(|inner| inner.callbacks.borrow().close.clone())
            {
                callback();
            }
        });
        let weak = Rc::downgrade(&inspector.0);
        back.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                RowInspector(inner).back();
            }
        });
        let weak = Rc::downgrade(&inspector.0);
        inspector.0.list.connect_row_activated(move |_, row| {
            if let Some(inner) = weak.upgrade() {
                RowInspector(inner).open(row.index());
            }
        });

        let keys = gtk4::EventControllerKey::new();
        let weak = Rc::downgrade(&inspector.0);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(inner) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !modifiers.is_empty() {
                return glib::Propagation::Proceed;
            }
            let inspector = RowInspector(inner);
            match key {
                gdk::Key::Left | gdk::Key::Right if inspector.showing_row() => {
                    inspector.navigate(if key == gdk::Key::Left {
                        Direction::Left
                    } else {
                        Direction::Right
                    });
                    glib::Propagation::Stop
                }
                gdk::Key::space if inspector.showing_row() => {
                    if let Some(row) = inspector.0.list.selected_row() {
                        inspector.open(row.index());
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Escape => {
                    if inspector.back() {
                        return glib::Propagation::Stop;
                    }
                    if let Some(callback) = inspector.0.callbacks.borrow().close.clone() {
                        callback();
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        inspector.0.container.add_controller(keys);
        inspector
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.0.container
    }

    pub fn connect_close(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().close = Some(Rc::new(callback));
    }

    pub fn connect_pin(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().pin = Some(Rc::new(callback));
    }

    pub fn connect_move_row(&self, callback: impl Fn(i64) + 'static) {
        self.0.callbacks.borrow_mut().move_row = Some(Rc::new(callback));
    }

    pub fn connect_open_cell(&self, callback: impl Fn(usize) + 'static) {
        self.0.callbacks.borrow_mut().open_cell = Some(Rc::new(callback));
    }

    pub fn row(&self) -> Option<usize> {
        self.0.row.get()
    }

    pub fn selected_column(&self) -> usize {
        self.0
            .list
            .selected_row()
            .map_or(0, |row| row.index().max(0) as usize)
    }

    fn showing_row(&self) -> bool {
        self.0.stack.visible_child_name().as_deref() == Some("row")
    }

    pub fn show_row(&self, row: usize, values: Vec<(String, Option<String>)>) {
        let column = self.selected_column();
        let changed_row = self.0.row.replace(Some(row)) != Some(row);
        self.0.title.set_text(&format!("Result Row {}", row + 1));
        self.0
            .title
            .update_property(&[gtk4::accessible::Property::Label("Result Row")]);
        while let Some(child) = self.0.list.first_child() {
            self.0.list.remove(&child);
        }
        for (name, value) in &values {
            self.0.list.append(&value_row(name, value.as_deref()));
        }
        let column = column.min(values.len().saturating_sub(1));
        if let Some(target) = self.0.list.row_at_index(column as i32) {
            self.0.list.select_row(Some(&target));
            if self.0.pinned.get() && self.0.container.is_visible() {
                target.grab_focus();
            }
        }
        self.0.values.replace(values);
        if changed_row {
            self.0.stack.set_visible_child_name("row");
        }
    }

    pub fn navigate(&self, direction: Direction) -> bool {
        if !self.is_visible() || !self.showing_row() {
            return false;
        }
        if let Some(delta) = direction.horizontal() {
            if let Some(callback) = self.0.callbacks.borrow().move_row.clone() {
                callback(delta);
            }
        } else if let Some(delta) = direction.vertical() {
            self.move_column(delta);
        }
        true
    }

    pub fn select_column(&self, column: usize) {
        if let Some(row) = self.0.list.row_at_index(column as i32) {
            self.0.list.select_row(Some(&row));
        }
    }

    pub fn move_column(&self, direction: i64) {
        let count = self.0.values.borrow().len() as i64;
        if count == 0 {
            return;
        }
        let next = (self.selected_column() as i64 + direction).clamp(0, count - 1);
        if let Some(row) = self.0.list.row_at_index(next as i32) {
            self.0.list.select_row(Some(&row));
            row.grab_focus();
        }
    }

    fn open(&self, index: i32) {
        let Ok(column) = usize::try_from(index) else {
            return;
        };
        if let Some(callback) = self.0.callbacks.borrow().open_cell.clone() {
            callback(column);
        }
    }

    pub fn show_cell(&self, column: usize, value: Option<&str>, total: usize) {
        let name = self
            .0
            .values
            .borrow()
            .get(column)
            .map(|(name, _)| name.clone())
            .unwrap_or_default();
        self.0.cell_title.set_text(&name);
        self.0.cell_view.buffer().set_text(value.unwrap_or("NULL"));
        let shown = value.map_or(0, |value| value.chars().count());
        self.0.cell_detail.set_text(&match value {
            None => "SQL NULL".into(),
            Some(_) if total == 0 => "Empty value".into(),
            Some(_) => format!("Characters 1-{shown} of {total}"),
        });
        self.0.stack.set_visible_child_name("cell");
        self.0.cell_view.grab_focus();
    }

    pub fn back(&self) -> bool {
        if self.showing_row() {
            return false;
        }
        self.0.stack.set_visible_child_name("row");
        if let Some(row) = self.0.list.selected_row() {
            row.grab_focus();
        }
        true
    }

    pub fn set_visibility(&self, visible: bool, pinned: bool) {
        self.0.pinned.set(visible && pinned);
        self.0.container.set_visible(visible);
        let label = if pinned {
            "Unpin result row"
        } else {
            "Keep result row open"
        };
        self.0.pin_button.set_tooltip_text(Some(label));
        self.0
            .pin_button
            .update_property(&[gtk4::accessible::Property::Label(label)]);
        if !visible {
            self.0.stack.set_visible_child_name("row");
            self.0.row.set(None);
        } else if pinned && let Some(row) = self.0.list.selected_row() {
            row.grab_focus();
        }
    }

    pub fn is_visible(&self) -> bool {
        self.0.container.is_visible()
    }

    pub fn is_pinned(&self) -> bool {
        self.0.pinned.get()
    }
}

fn value_row(name: &str, value: Option<&str>) -> gtk4::ListBoxRow {
    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    let key = gtk4::Label::new(Some(name));
    key.set_width_chars(16);
    key.set_max_width_chars(24);
    key.set_xalign(1.0);
    key.set_valign(gtk4::Align::Start);
    key.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    key.add_css_class("dim-label");
    content.append(&key);
    let label = gtk4::Label::new(Some(value.unwrap_or("NULL")));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_wrap(true);
    label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    label.set_lines(6);
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.add_css_class("monospace");
    if value.is_none() {
        label.add_css_class("dim-label");
    }
    content.append(&label);
    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&content));
    row.update_property(&[
        gtk4::accessible::Property::Label(name),
        gtk4::accessible::Property::Description(value.unwrap_or("SQL NULL")),
    ]);
    row
}
