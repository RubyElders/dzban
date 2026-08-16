use adw::prelude::*;
use adw::{ActionRow, StatusPage};
use gtk4::prelude::*;
use gtk4::{Box, Button, GestureClick, Label, ListBox, Orientation, ScrolledWindow, Spinner};
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

use dzban_db::{Config, DatabaseConfig, RuntimeBridge};

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum TestStatus {
    NotTested,
    Testing,
    Success,
    Failed,
}

pub fn create_connection_manager<F>(bridge: RuntimeBridge, on_database_open: F) -> Box
where
    F: Fn(&str) + 'static + Clone,
{
    let container = Box::new(Orientation::Vertical, 0);
    container.set_vexpand(true);

    // Load config
    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("Failed to load config: {}", e);
        Config {
            databases: Vec::new(),
        }
    });

    if config.databases.is_empty() {
        // Show empty state
        let status_page = StatusPage::new();
        status_page.set_icon_name(Some("network-server-symbolic"));
        status_page.set_title("No Connections Configured");
        status_page.set_description(Some("Create a dzban.toml file with database connections"));
        container.append(&status_page);
        return container;
    }

    // Header with Test All button
    let header = Box::new(Orientation::Horizontal, 12);
    header.set_margin_start(12);
    header.set_margin_end(12);
    header.set_margin_top(12);
    header.set_margin_bottom(6);

    let title = Label::new(Some("Database Connections"));
    title.add_css_class("title-2");
    title.set_halign(gtk4::Align::Start);
    title.set_hexpand(true);
    header.append(&title);

    let test_all_button = Button::with_label("Test All");
    test_all_button.add_css_class("suggested-action");
    header.append(&test_all_button);

    container.append(&header);

    // Scrolled window for connection list
    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);

    let listbox = ListBox::new();
    listbox.add_css_class("boxed-list");
    listbox.set_selection_mode(gtk4::SelectionMode::None);
    listbox.set_margin_start(12);
    listbox.set_margin_end(12);
    listbox.set_margin_top(6);
    listbox.set_margin_bottom(12);

    // Create rows for each database
    let mut test_buttons = Vec::new();
    for db_config in &config.databases {
        let (row, test_button, status_indicator) =
            create_connection_row(db_config, on_database_open.clone());
        listbox.append(&row);
        test_buttons.push((db_config.clone(), test_button, status_indicator));
    }

    // Wire up individual test buttons
    for (db_config, test_button, status_indicator) in test_buttons.iter() {
        let db_config = db_config.clone();
        let status_indicator = status_indicator.clone();
        let test_button_weak = test_button.clone();
        let bridge_clone = bridge.clone();

        // Use Rc<RefCell<bool>> to track if test is in progress
        let is_testing = Rc::new(RefCell::new(false));
        let is_testing_clone = is_testing.clone();

        test_button.connect_clicked(move |_btn| {
            // Check if already testing
            if *is_testing_clone.borrow() {
                return;
            }

            *is_testing_clone.borrow_mut() = true;
            test_button_weak.set_sensitive(false);

            let is_testing_reset = is_testing_clone.clone();
            let test_button_reset = test_button_weak.clone();

            test_connection(
                bridge_clone.clone(),
                db_config.clone(),
                status_indicator.clone(),
                move || {
                    // Reset state when test completes
                    *is_testing_reset.borrow_mut() = false;
                    test_button_reset.set_sensitive(true);
                },
            );
        });
    }

    // Wire up Test All button - just click all test buttons
    let test_buttons_clone = test_buttons.clone();
    test_all_button.connect_clicked(move |_| {
        for (_db_config, test_btn, _status_indicator) in &test_buttons_clone {
            // Only trigger if button is sensitive (not already testing)
            if test_btn.is_sensitive() {
                test_btn.emit_clicked();
            }
        }
    });

    scrolled.set_child(Some(&listbox));
    container.append(&scrolled);

    container
}

fn create_connection_row<F>(
    db_config: &DatabaseConfig,
    on_database_open: F,
) -> (ActionRow, Button, Box)
where
    F: Fn(&str) + 'static,
{
    let row = ActionRow::new();
    row.set_title(&db_config.name);

    // Build subtitle with connection details
    let details = if db_config.host.is_some() {
        format!(
            "{}@{}:{}/{}",
            db_config.user,
            db_config.host.as_ref().unwrap(),
            db_config.port.unwrap_or(5432),
            db_config.database
        )
    } else {
        format!("{}@unix/{}", db_config.user, db_config.database)
    };
    row.set_subtitle(&details);

    // Add double-click gesture to open database
    let gesture = GestureClick::new();
    gesture.set_button(1); // Left click
    let db_name = db_config.name.clone();
    gesture.connect_released(move |gesture, n_press, _x, _y| {
        if n_press == 2 {
            // Double click
            on_database_open(&db_name);
            gesture.set_state(gtk4::EventSequenceState::Claimed);
        }
    });
    row.add_controller(gesture);

    // Make row activatable (shows hover effect)
    row.set_activatable(true);

    // Status indicator and test button
    let suffix_box = Box::new(Orientation::Horizontal, 8);

    // Status indicator (circle with color)
    let status_box = Box::new(Orientation::Horizontal, 4);
    let status_indicator = create_status_indicator(TestStatus::NotTested);
    status_box.append(&status_indicator);

    // Test button
    let test_button = Button::with_label("Test");
    test_button.set_valign(gtk4::Align::Center);

    suffix_box.append(&status_box);
    suffix_box.append(&test_button);

    row.add_suffix(&suffix_box);

    (row, test_button, status_box)
}

fn create_status_indicator(status: TestStatus) -> Box {
    let box_widget = Box::new(Orientation::Horizontal, 4);
    box_widget.set_valign(gtk4::Align::Center);

    match status {
        TestStatus::NotTested => {
            let label = Label::new(Some("●"));
            label.add_css_class("dim-label");
            box_widget.append(&label);

            let text = Label::new(Some("Not tested"));
            text.add_css_class("caption");
            text.add_css_class("dim-label");
            box_widget.append(&text);
        }
        TestStatus::Testing => {
            let spinner = Spinner::new();
            spinner.start();
            box_widget.append(&spinner);

            let text = Label::new(Some("Testing..."));
            text.add_css_class("caption");
            box_widget.append(&text);
        }
        TestStatus::Success => {
            let label = Label::new(Some("●"));
            label.add_css_class("success");
            box_widget.append(&label);

            let text = Label::new(Some("OK"));
            text.add_css_class("caption");
            text.add_css_class("success");
            box_widget.append(&text);
        }
        TestStatus::Failed => {
            let label = Label::new(Some("●"));
            label.add_css_class("error");
            box_widget.append(&label);

            let text = Label::new(Some("Failed"));
            text.add_css_class("caption");
            text.add_css_class("error");
            box_widget.append(&text);
        }
    }

    box_widget
}

fn test_connection<F>(
    bridge: RuntimeBridge,
    db_config: DatabaseConfig,
    status_container: Box,
    on_complete: F,
) where
    F: Fn() + 'static,
{
    // Update to testing state
    update_status_indicator(&status_container, TestStatus::Testing);

    // Use RuntimeBridge to test connection on Tokio runtime
    let rx = bridge.test_connection(db_config);

    // Wait for result on GLib event loop
    gtk4::glib::spawn_future_local(async move {
        let result = rx.await;

        match result {
            Ok(Ok(_version)) => {
                update_status_indicator(&status_container, TestStatus::Success);
            }
            Ok(Err(e)) => {
                // Show error in status
                while let Some(child) = status_container.first_child() {
                    status_container.remove(&child);
                }

                let label = Label::new(Some("●"));
                label.add_css_class("error");
                status_container.append(&label);

                let text = Label::new(Some(&format!("Error: {}", e)));
                text.add_css_class("caption");
                text.add_css_class("error");
                text.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                text.set_max_width_chars(40);
                text.set_tooltip_text(Some(&e.to_string()));
                status_container.append(&text);
            }
            Err(_) => {
                // Channel closed, connection task was cancelled
                while let Some(child) = status_container.first_child() {
                    status_container.remove(&child);
                }

                let label = Label::new(Some("●"));
                label.add_css_class("error");
                status_container.append(&label);

                let text = Label::new(Some("Cancelled"));
                text.add_css_class("caption");
                text.add_css_class("error");
                text.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                text.set_max_width_chars(40);
                status_container.append(&text);
            }
        }

        // Call completion callback to re-enable button
        on_complete();
    });
}

fn update_status_indicator(container: &Box, status: TestStatus) {
    // Remove old children
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    // Add new indicator
    let new_indicator = create_status_indicator(status);
    while let Some(child) = new_indicator.first_child() {
        new_indicator.remove(&child);
        container.append(&child);
    }
}
