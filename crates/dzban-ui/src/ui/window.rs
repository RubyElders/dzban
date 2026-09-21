use adw::prelude::*;
use adw::{Application, ApplicationWindow, HeaderBar, Toast, ToastOverlay, ToolbarView};
use gtk4::prelude::*;
use libadwaita as adw;

use super::db_tabs::{DatabaseTabManager, QueryPreviewState};
use super::shortcuts;
use super::statusbar::create_statusbar;

pub fn build_ui(app: &Application) {
    install_styles();

    // Create main application window
    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(1400)
        .default_height(900)
        .title("Dzban - PostgreSQL Client")
        .build();

    // Use ToolbarView for modern header/content/bottom layout (v1_4+)
    let toolbar_view = ToolbarView::new();

    // Create toast overlay for notifications
    let toast_overlay = ToastOverlay::new();

    // Create status bar at the bottom
    let statusbar = create_statusbar();

    // Create database tab manager (main content)
    let db_tab_manager = DatabaseTabManager::new(statusbar.clone());
    let content = db_tab_manager.get_widget().clone();

    // Create header bar with search/action bar and controls
    let (header, search_entry) = create_header_bar(&db_tab_manager, &toast_overlay);
    toolbar_view.add_top_bar(&header);

    let query_preview_controller = gtk4::EventControllerKey::new();
    query_preview_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let db_manager_for_preview = db_tab_manager.clone();
    query_preview_controller.connect_key_pressed(move |_, key, _, modifiers| {
        if key == gtk4::gdk::Key::Control_L || key == gtk4::gdk::Key::Control_R {
            db_manager_for_preview.begin_query_preview();
        } else if modifiers.contains(gtk4::gdk::ModifierType::CONTROL_MASK)
            && key != gtk4::gdk::Key::Return
            && key != gtk4::gdk::Key::KP_Enter
        {
            db_manager_for_preview.cancel_query_preview();
        }
        gtk4::glib::Propagation::Proceed
    });
    let db_manager_for_preview = db_tab_manager.clone();
    query_preview_controller.connect_key_released(move |_, key, _, _| {
        if key == gtk4::gdk::Key::Control_L || key == gtk4::gdk::Key::Control_R {
            db_manager_for_preview.cancel_query_preview();
        }
    });
    window.add_controller(query_preview_controller);

    // Create shortcuts dialog (persistent, shown/hidden on demand)
    let shortcuts_dialog = shortcuts::create_shortcuts_window();

    // Setup keyboard shortcuts using unified system
    let search_entry_for_shortcuts = search_entry.clone();
    let db_manager_for_shortcuts = db_tab_manager.clone();
    let window_for_shortcuts = window.clone();
    let shortcuts_dialog_for_shortcut = shortcuts_dialog.clone();

    let key_controller = shortcuts::create_controller(move |shortcut_id| match shortcut_id {
        "search_focus" => {
            db_manager_for_shortcuts.cancel_query_preview();
            search_entry_for_shortcuts.grab_focus();
            true
        }
        "run_query" => {
            db_manager_for_shortcuts.cancel_query_preview();
            db_manager_for_shortcuts.run_current_query();
            true
        }
        "stop_query" => {
            db_manager_for_shortcuts.cancel_current_query();
            true
        }
        "new_query_tab" => {
            db_manager_for_shortcuts.new_query_in_current_db();
            true
        }
        "show_shortcuts" => {
            shortcuts_dialog_for_shortcut.present(Some(&window_for_shortcuts));
            true
        }
        _ => false,
    });
    window.add_controller(key_controller);

    // Add keyboard icon button to header bar to show shortcuts
    let shortcuts_button = gtk4::Button::from_icon_name("input-keyboard-symbolic");
    shortcuts_button.set_tooltip_text(Some("Keyboard Shortcuts (Ctrl+?)"));
    let shortcuts_dialog_for_button = shortcuts_dialog.clone();
    let window_for_button = window.clone();
    shortcuts_button.connect_clicked(move |_| {
        shortcuts_dialog_for_button.present(Some(&window_for_button));
    });
    header.pack_end(&shortcuts_button);

    toast_overlay.set_child(Some(&content));
    toolbar_view.set_content(Some(&toast_overlay));

    toolbar_view.add_bottom_bar(statusbar.get_widget());

    window.set_content(Some(&toolbar_view));

    // Setup toast example
    setup_toasts(&toast_overlay);

    window.present();
}

fn install_styles() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(
        "progressbar.query-action-wave trough {
            min-height: 30px;
            border-radius: 9px;
            background-color: transparent;
        }
        progressbar.query-action-wave progress {
            min-height: 30px;
            border-radius: 9px;
            background-image: linear-gradient(
                90deg,
                alpha(@accent_bg_color, 0.05),
                alpha(@accent_bg_color, 0.32),
                alpha(@accent_bg_color, 0.05)
            );
        }",
    );
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().expect("a display is available after GTK initialization"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn create_header_bar(
    db_tab_manager: &DatabaseTabManager,
    toast_overlay: &ToastOverlay,
) -> (HeaderBar, gtk4::Entry) {
    use gtk4::{
        Button, Entry, EntryIconPosition, EventControllerKey, Label, ListBox, MenuButton, Popover,
        ScrolledWindow, gdk,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    let header = HeaderBar::new();

    // Left side - sidebar toggle
    let sidebar_toggle = Button::from_icon_name("sidebar-show-symbolic");
    sidebar_toggle.set_tooltip_text(Some("Toggle Sidebar (Ctrl+B)"));

    // Wire up sidebar toggle
    let db_manager_clone = db_tab_manager.clone();
    sidebar_toggle.connect_clicked(move |btn| {
        db_manager_clone.toggle_current_sidebar();
        // Update icon based on state
        let is_showing = db_manager_clone.is_current_sidebar_visible();
        if is_showing {
            btn.set_icon_name("sidebar-hide-symbolic");
        } else {
            btn.set_icon_name("sidebar-show-symbolic");
        }
    });

    header.pack_start(&sidebar_toggle);

    let (search_container, db_search, progress_bar) = create_query_action_field();

    let pulse_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let search_for_preview = db_search.clone();
    let progress_for_preview = progress_bar.clone();
    let pulse_timer_for_preview = Rc::clone(&pulse_timer);
    db_tab_manager.set_query_preview_callback(move |state| {
        if let Some(timer) = pulse_timer_for_preview.borrow_mut().take() {
            timer.remove();
        }

        search_for_preview.set_placeholder_text(Some(&query_preview_message(&state)));
        match &state {
            QueryPreviewState::Loading => {
                search_for_preview.set_tooltip_text(Some("Release Ctrl to dismiss"));
                progress_for_preview.set_visible(true);
                progress_for_preview.set_fraction(0.2);
                let weak_progress = progress_for_preview.downgrade();
                *pulse_timer_for_preview.borrow_mut() = Some(glib::timeout_add_local(
                    std::time::Duration::from_millis(80),
                    move || {
                        let Some(progress) = weak_progress.upgrade() else {
                            return glib::ControlFlow::Break;
                        };
                        progress.pulse();
                        glib::ControlFlow::Continue
                    },
                ));
            }
            QueryPreviewState::Error(error) => {
                search_for_preview.set_tooltip_text(Some(error));
                clear_progress(&progress_for_preview);
            }
            QueryPreviewState::Idle | QueryPreviewState::Ready { .. } => {
                search_for_preview.set_tooltip_text(None);
                clear_progress(&progress_for_preview);
            }
        }
    });

    let suggestions_popover = Popover::new();
    suggestions_popover.set_parent(&db_search);
    suggestions_popover.set_autohide(false); // Don't auto-hide to keep focus in search
    suggestions_popover.set_has_arrow(false);
    suggestions_popover.set_width_request(500); // Fixed wider width

    let suggestions_scroll = ScrolledWindow::new();
    suggestions_scroll.set_max_content_height(400);
    suggestions_scroll.set_propagate_natural_height(true);
    suggestions_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);

    let suggestions_list = ListBox::new();
    suggestions_list.add_css_class("navigation-sidebar");
    suggestions_scroll.set_child(Some(&suggestions_list));
    suggestions_popover.set_child(Some(&suggestions_scroll));

    // Track selected row index
    let selected_index: Rc<RefCell<i32>> = Rc::new(RefCell::new(0));

    // Get real database list
    let db_list = db_tab_manager.get_database_list();

    // Define search actions
    #[derive(Clone)]
    struct SearchAction {
        title: String,
        subtitle: String,
        icon: String,
        action: String, // "db:name" or "connections"
    }

    let create_search_actions = move |search_text: &str| -> Vec<SearchAction> {
        let mut actions = Vec::new();
        let search_lower = search_text.to_lowercase();

        // Add "Connections" if it matches title or subtitle
        let conn_title = "Connections";
        let conn_subtitle = "Manage database connections";
        if search_text.is_empty()
            || conn_title.to_lowercase().contains(&search_lower)
            || conn_subtitle.to_lowercase().contains(&search_lower)
        {
            actions.push(SearchAction {
                title: conn_title.to_string(),
                subtitle: conn_subtitle.to_string(),
                icon: "network-server-symbolic".to_string(),
                action: "connections".to_string(),
            });
        }

        // Add matching databases (search in both name and subtitle)
        for db_name in &db_list {
            let subtitle = "Open database connection";
            if search_text.is_empty()
                || db_name.to_lowercase().contains(&search_lower)
                || subtitle.to_lowercase().contains(&search_lower)
            {
                actions.push(SearchAction {
                    title: db_name.clone(),
                    subtitle: subtitle.to_string(),
                    icon: "server-database-symbolic".to_string(),
                    action: format!("db:{}", db_name),
                });
            }
        }

        actions
    };

    let execute_action = {
        let db_manager = db_tab_manager.clone();
        let toast = toast_overlay.clone();
        move |action: &str, entry: &Entry| {
            if action == "connections" {
                db_manager.open_connection_manager();
                entry.set_text("");
                let toast_msg = Toast::new("Showing database connections");
                toast_msg.set_timeout(2);
                toast.add_toast(toast_msg);
            } else if let Some(db_name) = action.strip_prefix("db:") {
                db_manager.open_database(db_name);
                entry.set_text("");
                let toast_msg = Toast::new(&format!("Opened: {}", db_name));
                toast_msg.set_timeout(2);
                toast.add_toast(toast_msg);
            }
        }
    };

    // Helper to create highlighted text markup
    let create_highlighted_markup = |text: &str, search: &str| -> String {
        if search.is_empty() {
            return gtk4::glib::markup_escape_text(text).to_string();
        }

        // Create markup with bold matched parts
        let search_lower = search.to_lowercase();
        let text_lower = text.to_lowercase();

        if let Some(pos) = text_lower.find(&search_lower) {
            let before = &text[..pos];
            let matched = &text[pos..pos + search.len()];
            let after = &text[pos + search.len()..];

            format!(
                "{}<b>{}</b>{}",
                gtk4::glib::markup_escape_text(before),
                gtk4::glib::markup_escape_text(matched),
                gtk4::glib::markup_escape_text(after)
            )
        } else {
            gtk4::glib::markup_escape_text(text).to_string()
        }
    };

    let update_suggestions = {
        let suggestions_list = suggestions_list.clone();
        let suggestions_popover = suggestions_popover.clone();
        let selected_index = selected_index.clone();
        let db_search = db_search.clone();
        let execute_action = execute_action.clone();

        move |search_text: &str| {
            // Clear previous suggestions
            while let Some(child) = suggestions_list.first_child() {
                suggestions_list.remove(&child);
            }

            *selected_index.borrow_mut() = 0;

            let actions = create_search_actions(search_text);

            if actions.is_empty() {
                suggestions_popover.popdown();
                return;
            }

            // Add action rows
            for (idx, action) in actions.iter().enumerate() {
                let row = adw::ActionRow::new();

                // Use markup for title with highlighting
                let title_markup = create_highlighted_markup(&action.title, search_text);
                let subtitle_markup = create_highlighted_markup(&action.subtitle, search_text);

                // ActionRow doesn't support markup directly, so we need to use a workaround
                // Set title and subtitle as plain text first
                row.set_title(&action.title);
                row.set_subtitle(&action.subtitle);

                // Then replace with markup by accessing the internal labels
                if let Some(title_widget) = row.first_child()
                    && let Ok(box_widget) = title_widget.downcast::<gtk4::Box>()
                {
                    // Find both title and subtitle labels in the box
                    let mut child = box_widget.first_child();
                    while let Some(widget) = child {
                        if let Ok(label) = widget.clone().downcast::<Label>() {
                            let css_classes = label.css_classes();
                            let has_title = css_classes.iter().any(|c| c.as_str() == "title");
                            let has_subtitle = css_classes
                                .iter()
                                .any(|c| c.as_str() == "subtitle" || c.as_str() == "caption");

                            if has_title {
                                label.set_markup(&title_markup);
                            } else if has_subtitle {
                                label.set_markup(&subtitle_markup);
                            }
                        }
                        child = widget.next_sibling();
                    }
                }

                // Add icon as prefix
                let icon = gtk4::Image::from_icon_name(&action.icon);
                row.add_prefix(&icon);

                row.set_activatable(true);

                let action_str = action.action.clone();
                let execute_clone = execute_action.clone();
                let entry_clone = db_search.clone();
                let popover_clone = suggestions_popover.clone();

                row.connect_activated(move |_| {
                    execute_clone(&action_str, &entry_clone);
                    popover_clone.popdown();
                });

                suggestions_list.append(&row);

                // Select first row by default
                if idx == 0 {
                    suggestions_list.select_row(Some(&row));
                }
            }

            suggestions_popover.popup();
        }
    };

    // Wire up search changed to update suggestions
    let update_suggestions_clone = update_suggestions.clone();
    db_search.connect_changed(move |entry| {
        let text = entry.text();
        entry.set_icon_from_icon_name(
            EntryIconPosition::Secondary,
            (!text.is_empty()).then_some("edit-clear-symbolic"),
        );
        update_suggestions_clone(&text);
    });
    db_search.connect_icon_release(|entry, position| {
        if position == EntryIconPosition::Secondary {
            entry.set_text("");
        }
    });

    // Wire up keyboard navigation (Up/Down arrows)
    let key_controller = EventControllerKey::new();
    let suggestions_list_keys = suggestions_list.clone();
    let selected_index_keys = selected_index.clone();
    let suggestions_popover_keys = suggestions_popover.clone();
    key_controller.connect_key_pressed(move |_controller, keyval, _keycode, _state| {
        match keyval {
            gdk::Key::Down => {
                // Move selection down
                let mut idx = selected_index_keys.borrow_mut();
                let n_items = suggestions_list_keys.observe_children().n_items();
                if n_items > 0 {
                    *idx = (*idx + 1).min(n_items as i32 - 1);
                    if let Some(row) = suggestions_list_keys.row_at_index(*idx) {
                        suggestions_list_keys.select_row(Some(&row));
                        row.grab_focus();
                    }
                }
                gtk4::glib::Propagation::Stop
            }
            gdk::Key::Up => {
                // Move selection up
                let mut idx = selected_index_keys.borrow_mut();
                *idx = (*idx - 1).max(0);
                if let Some(row) = suggestions_list_keys.row_at_index(*idx) {
                    suggestions_list_keys.select_row(Some(&row));
                    row.grab_focus();
                }
                gtk4::glib::Propagation::Stop
            }
            gdk::Key::Escape => {
                // Close popover on Escape
                suggestions_popover_keys.popdown();
                gtk4::glib::Propagation::Stop
            }
            _ => gtk4::glib::Propagation::Proceed,
        }
    });
    db_search.add_controller(key_controller);

    // Wire up Enter key to activate selected row or first row
    let suggestions_list_enter = suggestions_list.clone();
    let suggestions_popover_enter = suggestions_popover.clone();
    let selected_index_enter = selected_index.clone();
    let toast_enter = toast_overlay.clone();
    db_search.connect_activate(move |entry| {
        let text = entry.text();
        let search_text = text.trim();

        // Check if we have a selected row in the popover
        if suggestions_popover_enter.is_visible() {
            let idx = *selected_index_enter.borrow();
            if let Some(row) = suggestions_list_enter.row_at_index(idx) {
                // Activate the selected row
                row.activate();
                suggestions_popover_enter.popdown();
                return;
            }
        }

        // Fallback: no popover or no selection, show error
        if !search_text.is_empty() {
            let toast_msg = Toast::new(&format!("Unknown action: {}", search_text));
            toast_msg.set_timeout(2);
            toast_enter.add_toast(toast_msg);
            entry.set_text("");
        }

        suggestions_popover_enter.popdown();
    });

    header.set_title_widget(Some(&search_container));

    // Right side controls

    // Run query button (primary action)
    let run_button = Button::from_icon_name("media-playback-start-symbolic");
    run_button.set_tooltip_text(Some("Run Query (Ctrl+Enter)"));
    run_button.add_css_class("suggested-action");

    let db_manager_run = db_tab_manager.clone();
    run_button.connect_clicked(move |_| {
        db_manager_run.run_current_query();
    });

    header.pack_end(&run_button);

    // New query tab button
    let new_query_button = Button::from_icon_name("tab-new-symbolic");
    new_query_button.set_tooltip_text(Some("New Query Tab (Ctrl+T)"));

    let db_manager_clone = db_tab_manager.clone();
    new_query_button.connect_clicked(move |_| {
        db_manager_clone.new_query_in_current_db();
    });

    header.pack_end(&new_query_button);

    // Settings menu
    let menu_button = MenuButton::new();
    menu_button.set_icon_name("open-menu-symbolic");
    menu_button.set_tooltip_text(Some("Main Menu"));
    header.pack_end(&menu_button);

    (header, db_search)
}

fn query_preview_message(state: &QueryPreviewState) -> String {
    match state {
        QueryPreviewState::Idle => "Search databases... (Ctrl+K)".to_string(),
        QueryPreviewState::Loading => {
            "Enter to run query - estimating rows and cost...".to_string()
        }
        QueryPreviewState::Ready {
            rows,
            startup_cost,
            total_cost,
        } => format!(
            "Enter to run query - ~{} rows - cost {:.2}..{:.2}",
            rows, startup_cost, total_cost
        ),
        QueryPreviewState::Error(_) => {
            "Enter to run query - estimate unavailable (hover for details)".to_string()
        }
    }
}

fn create_query_action_field() -> (gtk4::Overlay, gtk4::Entry, gtk4::ProgressBar) {
    let overlay = gtk4::Overlay::new();
    overlay.set_halign(gtk4::Align::Center);

    let entry = gtk4::Entry::new();
    entry.set_placeholder_text(Some("Search databases... (Ctrl+K)"));
    entry.set_width_chars(64);
    entry.add_css_class("search");
    entry.set_icon_from_icon_name(
        gtk4::EntryIconPosition::Primary,
        Some("system-search-symbolic"),
    );
    overlay.set_child(Some(&entry));

    let progress = gtk4::ProgressBar::new();
    progress.add_css_class("query-action-wave");
    progress.set_halign(gtk4::Align::Fill);
    progress.set_valign(gtk4::Align::Fill);
    progress.set_pulse_step(0.08);
    progress.set_can_target(false);
    progress.set_opacity(0.25);
    progress.set_visible(false);
    overlay.add_overlay(&progress);

    (overlay, entry, progress)
}

fn clear_progress(progress: &gtk4::ProgressBar) {
    progress.set_fraction(0.0);
    progress.set_visible(false);
}

fn setup_toasts(toast_overlay: &ToastOverlay) {
    // Example: Show a toast notification
    let toast = Toast::new("Welcome to Dzban!");
    toast.set_timeout(3);
    toast_overlay.add_toast(toast);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_query_preview_prompts_for_execution_with_estimates() {
        let message = query_preview_message(&QueryPreviewState::Ready {
            rows: 42,
            startup_cost: 1.25,
            total_cost: 9.5,
        });

        assert_eq!(message, "Enter to run query - ~42 rows - cost 1.25..9.50");
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn query_action_progress_preserves_layout_and_can_restart() {
        install_styles();
        let (field, entry, progress) = create_query_action_field();
        let window = gtk4::Window::new();
        window.set_child(Some(&field));
        window.present();
        gtk4::test_widget_wait_for_draw(&field);
        let baseline = (field.width(), field.height());

        progress.set_visible(true);
        progress.pulse();
        gtk4::test_widget_wait_for_draw(&field);
        assert!(progress.is_visible());
        assert!((progress.opacity() - 0.25).abs() < 0.01);
        assert_eq!((progress.width(), progress.height()), baseline);
        assert_eq!((field.width(), field.height()), baseline);

        clear_progress(&progress);
        gtk4::test_widget_wait_for_draw(&field);
        assert!(!progress.is_visible());
        assert_eq!(progress.fraction(), 0.0);
        assert_eq!((field.width(), field.height()), baseline);

        progress.set_visible(true);
        progress.pulse();
        gtk4::test_widget_wait_for_draw(&field);
        assert!(progress.is_visible());
        assert_eq!((progress.width(), progress.height()), baseline);
        assert_eq!((field.width(), field.height()), baseline);

        clear_progress(&progress);
        assert_eq!(
            entry.placeholder_text().as_deref(),
            Some("Search databases... (Ctrl+K)")
        );
        window.close();
    }
}
