use gtk4::prelude::*;
use gtk4::{Box, Orientation, Label, ProgressBar};
use std::rc::Rc;
use std::cell::RefCell;

use super::constants::EXPLAIN_DELAY_MS;

/// Action bar that shows current action status with progress
#[derive(Clone)]
pub struct ActionBar {
    container: Box,
    label: Label,
    progress_bar: ProgressBar,
    timer_handle: Rc<RefCell<Option<glib::SourceId>>>,
}

impl ActionBar {
    pub fn new() -> Self {
        let container = Box::new(Orientation::Vertical, 0);
        container.set_halign(gtk4::Align::Center);
        
        // Label for action text
        let label = Label::new(Some("Actions: Ctrl+K…"));
        label.add_css_class("title-4");
        label.set_margin_top(4);
        label.set_margin_bottom(2);
        container.append(&label);
        
        // Progress bar at the bottom
        let progress_bar = ProgressBar::new();
        progress_bar.set_fraction(0.0);
        progress_bar.set_show_text(false);
        progress_bar.set_hexpand(true);
        progress_bar.set_visible(false); // Hidden by default
        container.append(&progress_bar);
        
        Self {
            container,
            label,
            progress_bar,
            timer_handle: Rc::new(RefCell::new(None)),
        }
    }
    
    pub fn get_widget(&self) -> &Box {
        &self.container
    }
    
    /// Start showing progress for an action
    pub fn start_progress(&self, action_text: &str) {
        // Cancel any existing timer
        if let Some(timer_id) = self.timer_handle.borrow_mut().take() {
            timer_id.remove();
        }
        
        self.label.set_text(action_text);
        self.progress_bar.set_fraction(0.0);
        self.progress_bar.set_visible(true);
        
        // Animate progress bar over EXPLAIN_DELAY_MS
        let progress_bar = self.progress_bar.clone();
        let timer_handle = self.timer_handle.clone();
        let steps = 20; // Number of animation steps
        let interval_ms = EXPLAIN_DELAY_MS / steps;
        let increment = 1.0 / steps as f64;
        
        let current_step = Rc::new(RefCell::new(0u64));
        
        let timer_id = glib::timeout_add_local(
            std::time::Duration::from_millis(interval_ms),
            {
                let current_step = current_step.clone();
                move || {
                    let mut step = current_step.borrow_mut();
                    *step += 1;
                    
                    let fraction = (*step as f64) * increment;
                    progress_bar.set_fraction(fraction.min(1.0));
                    
                    if *step >= steps {
                        glib::ControlFlow::Break
                    } else {
                        glib::ControlFlow::Continue
                    }
                }
            }
        );
        
        *timer_handle.borrow_mut() = Some(timer_id);
    }
    
    /// Stop progress and reset to default state
    pub fn stop_progress(&self) {
        // Cancel any existing timer
        if let Some(timer_id) = self.timer_handle.borrow_mut().take() {
            timer_id.remove();
        }
        
        self.label.set_text("Actions: Ctrl+K…");
        self.progress_bar.set_visible(false);
        self.progress_bar.set_fraction(0.0);
    }
    
    /// Update the label text without affecting progress
    pub fn set_text(&self, text: &str) {
        self.label.set_text(text);
    }
}
