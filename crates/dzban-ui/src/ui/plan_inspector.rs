use gtk4::prelude::*;
use gtk4::{Box, Button, Label, Orientation, ScrolledWindow, Spinner, Stack, TextView};

use super::preview_overlay::PreviewOverlay;
use dzban_db::{QueryEstimate, QueryPlan};

#[derive(Clone, Debug, PartialEq)]
pub enum PlanState {
    Loading,
    Ready(serde_json::Value),
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanScroll {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
}

#[derive(Clone)]
pub struct PlanInspector {
    container: Box,
    stack: Stack,
    summary: Label,
    plan_view: TextView,
    error: Label,
    scrolled: ScrolledWindow,
    pin_button: Button,
    visibility: PreviewOverlay,
}

impl PlanInspector {
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
        container.add_css_class("plan-inspector");
        container.set_widget_name("query-plan-inspector");
        container.update_property(&[
            gtk4::accessible::Property::Label("Query plan inspector"),
            gtk4::accessible::Property::Description(
                "Estimated rows, cost, and database query plan",
            ),
        ]);
        container.set_visible(false);

        let header = Box::new(Orientation::Horizontal, 8);
        header.set_margin_start(12);
        header.set_margin_end(8);
        header.set_margin_top(8);
        header.set_margin_bottom(8);

        let title = Label::new(Some("Query Plan"));
        title.add_css_class("heading");
        title.set_halign(gtk4::Align::Start);
        title.set_hexpand(true);
        header.append(&title);

        let pin_button = Button::from_icon_name("view-pin-symbolic");
        pin_button.add_css_class("flat");
        pin_button.set_widget_name("query-plan-pin");
        pin_button.set_tooltip_text(Some("Pin query plan"));
        pin_button.update_property(&[
            gtk4::accessible::Property::Label("Pin query plan"),
            gtk4::accessible::Property::Description(
                "Keep the detailed query plan open after releasing Ctrl and Shift",
            ),
            gtk4::accessible::Property::KeyShortcuts("Control+Shift+Space"),
        ]);
        header.append(&pin_button);

        let close_button = Button::from_icon_name("window-close-symbolic");
        close_button.add_css_class("flat");
        close_button.set_widget_name("query-plan-close");
        close_button.set_tooltip_text(Some("Close query plan"));
        close_button.update_property(&[
            gtk4::accessible::Property::Label("Close query plan"),
            gtk4::accessible::Property::KeyShortcuts("Escape"),
        ]);
        header.append(&close_button);
        container.append(&header);
        container.append(&gtk4::Separator::new(Orientation::Horizontal));

        let stack = Stack::new();
        stack.set_vexpand(true);
        stack.set_transition_type(gtk4::StackTransitionType::Crossfade);

        let loading = Box::new(Orientation::Vertical, 12);
        loading.set_valign(gtk4::Align::Center);
        let spinner = Spinner::new();
        spinner.set_spinning(true);
        spinner.set_size_request(32, 32);
        let loading_label = Label::new(Some("Estimating query..."));
        loading_label.add_css_class("dim-label");
        loading.append(&spinner);
        loading.append(&loading_label);
        stack.add_named(&loading, Some("loading"));

        let ready = Box::new(Orientation::Vertical, 8);
        let summary = Label::new(None);
        summary.set_halign(gtk4::Align::Start);
        summary.set_margin_start(12);
        summary.set_margin_end(12);
        summary.set_margin_top(10);
        summary.add_css_class("dim-label");
        ready.append(&summary);

        let scrolled = ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hexpand(true);
        scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
        let plan_view = TextView::new();
        plan_view.set_widget_name("query-plan-content");
        plan_view.set_editable(false);
        plan_view.set_cursor_visible(false);
        plan_view.set_monospace(true);
        plan_view.set_wrap_mode(gtk4::WrapMode::None);
        plan_view.set_left_margin(12);
        plan_view.set_right_margin(12);
        plan_view.set_top_margin(4);
        plan_view.set_bottom_margin(12);
        plan_view.update_property(&[
            gtk4::accessible::Property::Label("Database query plan"),
            gtk4::accessible::Property::ReadOnly(true),
        ]);
        scrolled.set_child(Some(&plan_view));
        ready.append(&scrolled);
        stack.add_named(&ready, Some("ready"));

        let error = Label::new(None);
        error.set_wrap(true);
        error.set_xalign(0.0);
        error.set_valign(gtk4::Align::Start);
        error.set_margin_start(16);
        error.set_margin_end(16);
        error.set_margin_top(16);
        error.add_css_class("error");
        stack.add_named(&error, Some("error"));

        container.append(&stack);

        let visibility = PreviewOverlay::new(&container);
        let inspector = Self {
            container,
            stack,
            summary,
            plan_view,
            error,
            scrolled,
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

    pub fn set_state(&self, state: &PlanState) {
        match state {
            PlanState::Loading => self.stack.set_visible_child_name("loading"),
            PlanState::Ready(plan) => {
                self.summary.set_text(&plan_summary(plan));
                let document = serde_json::from_value::<QueryPlan>(plan.clone())
                    .map(|plan| plan.document)
                    .unwrap_or_else(|_| plan.clone());
                self.plan_view.buffer().set_text(
                    &serde_json::to_string_pretty(&document)
                        .unwrap_or_else(|_| document.to_string()),
                );
                self.stack.set_visible_child_name("ready");
            }
            PlanState::Error(error) => {
                self.error.set_text(error);
                self.stack.set_visible_child_name("error");
            }
        }
    }

    pub fn show_transient(&self) {
        self.visibility.show_transient();
    }

    pub fn hide_transient(&self) {
        self.visibility.hide_transient();
    }

    pub fn pin(&self) -> bool {
        if !self.visibility.pin() {
            return false;
        }
        self.pin_button.set_icon_name("view-pin-symbolic");
        self.pin_button.set_tooltip_text(Some("Unpin query plan"));
        self.pin_button
            .update_property(&[gtk4::accessible::Property::Label("Unpin query plan")]);
        self.plan_view.grab_focus();
        true
    }

    pub fn close(&self) -> bool {
        if !self.visibility.close() {
            return false;
        }
        self.pin_button.set_tooltip_text(Some("Pin query plan"));
        self.pin_button
            .update_property(&[gtk4::accessible::Property::Label("Pin query plan")]);
        true
    }

    pub fn show_saved(&self, plan: &serde_json::Value, pinned: bool) {
        if self.is_pinned() && !pinned {
            return;
        }
        self.set_state(&PlanState::Ready(plan.clone()));
        self.show_transient();
        if pinned {
            self.pin();
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visibility.is_visible()
    }

    pub fn is_pinned(&self) -> bool {
        self.visibility.is_pinned()
    }

    pub fn scroll(&self, direction: PlanScroll) -> bool {
        if !self.is_visible() {
            return false;
        }
        let adjustment = self.scrolled.vadjustment();
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
}

fn plan_summary(plan: &serde_json::Value) -> String {
    if let Ok(plan) = serde_json::from_value::<QueryPlan>(plan.clone()) {
        return query_estimate_summary(&plan.estimate)
            .unwrap_or_else(|| "Estimated query plan".to_string());
    }
    let root = plan
        .as_array()
        .and_then(|plans| plans.first())
        .and_then(|root| root.get("Plan"));
    let rows = root
        .and_then(|plan| plan.get("Plan Rows"))
        .and_then(serde_json::Value::as_i64);
    let startup = root
        .and_then(|plan| plan.get("Startup Cost"))
        .and_then(serde_json::Value::as_f64);
    let total = root
        .and_then(|plan| plan.get("Total Cost"))
        .and_then(serde_json::Value::as_f64);
    match (rows, startup, total) {
        (Some(rows), Some(startup), Some(total)) => {
            format!(
                "Estimated rows: {}    Cost: {:.2}..{:.2}",
                rows, startup, total
            )
        }
        _ => "Estimated query plan".to_string(),
    }
}

pub fn query_estimate_summary(estimate: &QueryEstimate) -> Option<String> {
    match (
        estimate.rows,
        estimate.startup_cost,
        estimate.total_cost,
        estimate.parts,
        estimate.marks,
    ) {
        (Some(rows), Some(startup), Some(total), _, _) => Some(format!(
            "Estimated rows: {rows}    Cost: {startup:.2}..{total:.2}"
        )),
        (Some(rows), _, _, Some(parts), Some(marks)) => Some(format!(
            "Estimated rows: {rows}    Parts: {parts}    Marks: {marks}"
        )),
        (Some(rows), _, _, _, _) => Some(format!("Estimated rows: {rows}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_summary_uses_root_estimates() {
        let plan = serde_json::json!([{
            "Plan": {
                "Plan Rows": 42,
                "Startup Cost": 1.25,
                "Total Cost": 9.5
            }
        }]);
        assert_eq!(
            plan_summary(&plan),
            "Estimated rows: 42    Cost: 1.25..9.50"
        );
    }

    #[test]
    fn plan_summary_uses_clickhouse_scan_estimates() {
        let plan = serde_json::to_value(QueryPlan {
            backend: dzban_db::DatabaseBackend::Clickhouse,
            document: serde_json::json!({"Plan": {}}),
            estimate: dzban_db::QueryEstimate {
                rows: Some(42),
                startup_cost: None,
                total_cost: None,
                parts: Some(2),
                marks: Some(7),
            },
        })
        .unwrap();

        assert_eq!(
            plan_summary(&plan),
            "Estimated rows: 42    Parts: 2    Marks: 7"
        );
    }

    #[test]
    fn running_summary_includes_rows_and_cost() {
        assert_eq!(
            query_estimate_summary(&QueryEstimate {
                rows: Some(42),
                startup_cost: Some(1.25),
                total_cost: Some(9.5),
                ..QueryEstimate::default()
            }),
            Some("Estimated rows: 42    Cost: 1.25..9.50".to_string())
        );
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn transient_plan_can_be_pinned_and_closed() {
        let inspector = PlanInspector::new();
        inspector.set_state(&PlanState::Ready(serde_json::json!([{
            "Plan": {"Plan Rows": 1, "Startup Cost": 0.0, "Total Cost": 0.01}
        }])));
        inspector.show_transient();
        assert!(inspector.is_visible());
        assert!(inspector.pin());
        inspector.hide_transient();
        assert!(inspector.is_visible());
        assert!(inspector.is_pinned());
        assert!(inspector.close());
        assert!(!inspector.is_visible());
    }
}
