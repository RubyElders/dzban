use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dzban_core::presentation::PlanPresentation;
use gtk4::prelude::*;

use super::format;

#[derive(Clone, Debug, PartialEq)]
pub enum PlanContent {
    Loading,
    Ready(Box<PlanPresentation>),
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanScroll {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
}

type Callback = Rc<dyn Fn()>;

#[derive(Default)]
struct Callbacks {
    pin: Option<Callback>,
    close: Option<Callback>,
}

struct Inner {
    container: gtk4::Box,
    stack: gtk4::Stack,
    summary: gtk4::Label,
    text: gtk4::TextView,
    error: gtk4::Label,
    scroll: gtk4::ScrolledWindow,
    pin: gtk4::Button,
    pinned: Cell<bool>,
    content: RefCell<Option<PlanContent>>,
    callbacks: RefCell<Callbacks>,
}

#[derive(Clone)]
pub struct PlanView(Rc<Inner>);

impl PlanView {
    pub fn new() -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.set_size_request(620, 400);
        container.set_halign(gtk4::Align::End);
        container.set_valign(gtk4::Align::Start);
        container.set_margin_top(8);
        container.set_margin_end(12);
        container.add_css_class("card");
        container.add_css_class("floating-inspector");
        container.set_widget_name("query-plan-inspector");
        container.set_visible(false);
        container.update_property(&[
            gtk4::accessible::Property::Label("Query plan inspector"),
            gtk4::accessible::Property::Description(
                "Estimated rows, cost, and database query plan",
            ),
        ]);

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        header.set_margin_start(16);
        header.set_margin_end(8);
        header.set_margin_top(8);
        header.set_margin_bottom(8);
        let title = gtk4::Label::new(Some("Query Plan"));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        header.append(&title);
        let pin = gtk4::Button::from_icon_name("view-pin-symbolic");
        pin.add_css_class("flat");
        header.append(&pin);
        let close = gtk4::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.set_tooltip_text(Some("Close query plan"));
        close.update_property(&[
            gtk4::accessible::Property::Label("Close query plan"),
            gtk4::accessible::Property::KeyShortcuts("Escape"),
        ]);
        header.append(&close);
        container.append(&header);
        container.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

        let stack = gtk4::Stack::new();
        stack.set_vexpand(true);
        let loading = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        loading.set_valign(gtk4::Align::Center);
        let spinner = gtk4::Spinner::new();
        spinner.set_spinning(true);
        spinner.set_size_request(24, 24);
        loading.append(&spinner);
        let loading_label = gtk4::Label::new(Some("Loading plan"));
        loading_label.add_css_class("dim-label");
        loading.append(&loading_label);
        stack.add_named(&loading, Some("loading"));

        let ready = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        let summary = gtk4::Label::new(None);
        summary.set_xalign(0.0);
        summary.set_wrap(true);
        summary.set_margin_start(16);
        summary.set_margin_end(16);
        summary.set_margin_top(10);
        summary.add_css_class("dim-label");
        summary.add_css_class("caption");
        ready.append(&summary);
        let text = gtk4::TextView::new();
        text.set_widget_name("query-plan-content");
        text.set_editable(false);
        text.set_cursor_visible(false);
        text.set_monospace(true);
        text.set_wrap_mode(gtk4::WrapMode::None);
        text.set_left_margin(16);
        text.set_right_margin(16);
        text.set_top_margin(4);
        text.set_bottom_margin(12);
        text.update_property(&[
            gtk4::accessible::Property::Label("Database query plan"),
            gtk4::accessible::Property::ReadOnly(true),
        ]);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_child(Some(&text));
        ready.append(&scroll);
        stack.add_named(&ready, Some("ready"));

        let error = gtk4::Label::new(None);
        error.set_wrap(true);
        error.set_selectable(true);
        error.set_xalign(0.0);
        error.set_valign(gtk4::Align::Start);
        error.set_margin_start(16);
        error.set_margin_end(16);
        error.set_margin_top(16);
        error.add_css_class("error");
        stack.add_named(&error, Some("error"));
        container.append(&stack);

        let view = Self(Rc::new(Inner {
            container,
            stack,
            summary,
            text,
            error,
            scroll,
            pin: pin.clone(),
            pinned: Cell::new(false),
            content: RefCell::new(None),
            callbacks: RefCell::new(Callbacks::default()),
        }));
        let weak = Rc::downgrade(&view.0);
        pin.connect_clicked(move |_| {
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
        let weak = Rc::downgrade(&view.0);
        close.connect_clicked(move |_| {
            if let Some(callback) = weak
                .upgrade()
                .and_then(|inner| inner.callbacks.borrow().close.clone())
            {
                callback();
            }
        });
        view.set_pinned(false);
        view
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.0.container
    }

    pub fn connect_pin(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().pin = Some(Rc::new(callback));
    }

    pub fn connect_close(&self, callback: impl Fn() + 'static) {
        self.0.callbacks.borrow_mut().close = Some(Rc::new(callback));
    }

    fn set_pinned(&self, pinned: bool) {
        self.0.pinned.set(pinned);
        let label = if pinned {
            "Unpin query plan"
        } else {
            "Pin query plan"
        };
        self.0.pin.set_tooltip_text(Some(label));
        self.0
            .pin
            .update_property(&[gtk4::accessible::Property::Label(label)]);
    }

    pub fn show(&self, content: PlanContent, pinned: bool) {
        if self.0.content.borrow().as_ref() != Some(&content) {
            match &content {
                PlanContent::Loading => self.0.stack.set_visible_child_name("loading"),
                PlanContent::Ready(plan) => {
                    self.0.summary.set_text(&format::plan_summary(plan));
                    self.0.text.buffer().set_text(&plan.text);
                    self.0.stack.set_visible_child_name("ready");
                }
                PlanContent::Error(error) => {
                    self.0.error.set_text(error);
                    self.0.stack.set_visible_child_name("error");
                }
            }
            self.0.content.replace(Some(content));
        }
        self.set_pinned(pinned);
        self.0.container.set_visible(true);
    }

    pub fn hide(&self) {
        self.0.container.set_visible(false);
        self.0.content.replace(None);
        self.set_pinned(false);
    }

    pub fn is_visible(&self) -> bool {
        self.0.container.is_visible()
    }

    #[cfg(test)]
    pub fn is_pinned(&self) -> bool {
        self.is_visible() && self.0.pinned.get()
    }

    pub fn scroll(&self, direction: PlanScroll) -> bool {
        if !self.is_visible() {
            return false;
        }
        let adjustment = self.0.scroll.vadjustment();
        let delta = match direction {
            PlanScroll::LineUp => -adjustment.step_increment(),
            PlanScroll::LineDown => adjustment.step_increment(),
            PlanScroll::PageUp => -adjustment.page_increment(),
            PlanScroll::PageDown => adjustment.page_increment(),
        };
        let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        adjustment.set_value((adjustment.value() + delta).clamp(adjustment.lower(), upper));
        true
    }

    #[cfg(test)]
    pub fn summary(&self) -> String {
        self.0.summary.text().to_string()
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        let buffer = self.0.text.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dzban_core::{DatabaseBackend, QueryEstimate, QueryPlan};

    fn plan(estimate: QueryEstimate) -> PlanPresentation {
        PlanPresentation::from(&QueryPlan {
            backend: DatabaseBackend::Clickhouse,
            document: serde_json::json!({"Plan": "ReadFromMergeTree"}),
            estimate,
        })
    }

    #[test]
    fn summaries_include_estimates_and_diagnostics() {
        assert_eq!(
            format::plan_summary(&plan(QueryEstimate {
                rows: Some(42),
                parts: Some(2),
                marks: Some(7),
                ..Default::default()
            })),
            "Estimated rows: 42 | Parts: 2 | Marks: 7"
        );
        let summary = format::plan_summary(&plan(QueryEstimate {
            diagnostic: Some(dzban_core::error::ExecutionError::Message(
                "Not enough privileges".into(),
            )),
            ..Default::default()
        }));
        assert!(summary.starts_with("Estimate unavailable:"));
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn plan_text_is_rendered_and_pin_state_is_exposed() {
        let view = PlanView::new();
        view.show(PlanContent::Loading, false);
        assert!(view.is_visible());
        assert!(!view.is_pinned());
        let plan = plan(QueryEstimate {
            rows: Some(42),
            ..Default::default()
        });
        view.show(PlanContent::Ready(Box::new(plan.clone())), true);
        assert!(view.is_pinned());
        assert_eq!(view.text(), plan.text);
        assert_eq!(view.summary(), "Estimated rows: 42");
        view.hide();
        assert!(!view.is_visible());
        assert!(!view.is_pinned());
    }
}
