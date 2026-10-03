use adw::prelude::*;
use adw::{Application, ApplicationWindow, HeaderBar, Toast, ToastOverlay, ToolbarView};
use gtk4::prelude::*;
use libadwaita as adw;

use crate::APP_NAME;

use super::db_tabs::{DatabaseTabManager, QueryActionState};
use super::plan_inspector::PlanScroll;
use super::shortcuts;
use super::shortcuts::ShortcutAction;
use super::statusbar::create_statusbar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewKeyAction {
    Begin,
    Cancel,
    Scroll(PlanScroll),
}

fn relevant_modifiers(modifiers: gtk4::gdk::ModifierType) -> gtk4::gdk::ModifierType {
    modifiers
        & (gtk4::gdk::ModifierType::CONTROL_MASK
            | gtk4::gdk::ModifierType::SHIFT_MASK
            | gtk4::gdk::ModifierType::ALT_MASK)
}

fn plan_scroll(key: gtk4::gdk::Key) -> Option<PlanScroll> {
    match key {
        gtk4::gdk::Key::Up => Some(PlanScroll::LineUp),
        gtk4::gdk::Key::Down => Some(PlanScroll::LineDown),
        gtk4::gdk::Key::Page_Up => Some(PlanScroll::PageUp),
        gtk4::gdk::Key::Page_Down => Some(PlanScroll::PageDown),
        _ => None,
    }
}

fn preview_key_pressed(
    key: gtk4::gdk::Key,
    modifiers: gtk4::gdk::ModifierType,
) -> Option<PreviewKeyAction> {
    let modifiers = relevant_modifiers(modifiers);
    let chord = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK;
    let control_pressed = key == gtk4::gdk::Key::Control_L || key == gtk4::gdk::Key::Control_R;
    let shift_pressed = key == gtk4::gdk::Key::Shift_L || key == gtk4::gdk::Key::Shift_R;
    if (control_pressed && (modifiers == gtk4::gdk::ModifierType::SHIFT_MASK || modifiers == chord))
        || (shift_pressed
            && (modifiers == gtk4::gdk::ModifierType::CONTROL_MASK || modifiers == chord))
    {
        return Some(PreviewKeyAction::Begin);
    }
    if modifiers == chord || modifiers.is_empty() {
        return plan_scroll(key).map(PreviewKeyAction::Scroll);
    }
    None
}

fn preview_key_released(key: gtk4::gdk::Key) -> Option<PreviewKeyAction> {
    if key == gtk4::gdk::Key::Shift_L
        || key == gtk4::gdk::Key::Shift_R
        || key == gtk4::gdk::Key::Control_L
        || key == gtk4::gdk::Key::Control_R
    {
        Some(PreviewKeyAction::Cancel)
    } else {
        None
    }
}

pub fn build_ui(app: &Application) {
    install_styles();

    // Create main application window
    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(1400)
        .default_height(900)
        .title(APP_NAME)
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

    let shortcuts_dialog = shortcuts::create_shortcuts_window();

    // Create header bar with search/action bar and controls
    let (header, search_entry) =
        create_header_bar(&db_tab_manager, &toast_overlay, &window, &shortcuts_dialog);
    toolbar_view.add_top_bar(&header);

    let query_preview_controller = gtk4::EventControllerKey::new();
    query_preview_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let db_manager_for_preview = db_tab_manager.clone();
    query_preview_controller.connect_key_pressed(move |_, key, _, modifiers| {
        match preview_key_pressed(key, modifiers) {
            Some(PreviewKeyAction::Begin) => {
                db_manager_for_preview.begin_query_preview();
            }
            Some(PreviewKeyAction::Scroll(direction))
                if db_manager_for_preview.scroll_query_plan(direction) =>
            {
                return gtk4::glib::Propagation::Stop;
            }
            _ => {}
        }
        gtk4::glib::Propagation::Proceed
    });
    let db_manager_for_preview = db_tab_manager.clone();
    query_preview_controller.connect_key_released(move |_, key, _, _| {
        if let Some(PreviewKeyAction::Cancel) = preview_key_released(key) {
            db_manager_for_preview.finish_query_preview_chord();
        }
    });
    window.add_controller(query_preview_controller);

    // Setup keyboard shortcuts using unified system
    let search_entry_for_shortcuts = search_entry.clone();
    let db_manager_for_shortcuts = db_tab_manager.clone();
    let window_for_shortcuts = window.clone();
    let shortcuts_dialog_for_shortcut = shortcuts_dialog.clone();

    let key_controller = shortcuts::create_controller(move |action| {
        execute_shortcut_action(
            action,
            &db_manager_for_shortcuts,
            &search_entry_for_shortcuts,
            &window_for_shortcuts,
            &shortcuts_dialog_for_shortcut,
        )
    });
    window.add_controller(key_controller);

    toast_overlay.set_child(Some(&content));
    toast_overlay.set_vexpand(true);

    let workspace = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    workspace.set_vexpand(true);
    workspace.append(&toast_overlay);
    workspace.append(statusbar.get_widget());
    toolbar_view.set_content(Some(&workspace));

    window.set_content(Some(&toolbar_view));

    // Setup toast example
    setup_toasts(&toast_overlay);

    window.present();
    let manager_for_initial_focus = db_tab_manager;
    let window_for_initial_focus = window.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
        if let Some(row) = manager_for_initial_focus.selected_connection_row() {
            gtk4::prelude::GtkWindowExt::set_focus(&window_for_initial_focus, Some(&row));
        }
    });
}

fn execute_shortcut_action(
    action: ShortcutAction,
    db_manager: &DatabaseTabManager,
    search_entry: &gtk4::Entry,
    window: &ApplicationWindow,
    shortcuts_dialog: &adw::Dialog,
) -> bool {
    match action {
        ShortcutAction::FocusSearch => {
            db_manager.cancel_query_preview();
            search_entry.grab_focus()
        }
        ShortcutAction::ShowConnections => {
            db_manager.open_connection_manager();
            true
        }
        ShortcutAction::TestAllConnections => {
            db_manager.open_connection_manager();
            db_manager.test_all_connections()
        }
        ShortcutAction::FocusDatabaseTabs => db_manager.focus_database_tabs(),
        ShortcutAction::NextDatabaseTab => db_manager.select_next_database_tab(),
        ShortcutAction::PreviousDatabaseTab => db_manager.select_previous_database_tab(),
        ShortcutAction::LocateDatabaseTab => {
            search_entry.set_text("select open database tab");
            search_entry.grab_focus()
        }
        ShortcutAction::ToggleSidebar => {
            db_manager.toggle_current_sidebar();
            true
        }
        ShortcutAction::NewQueryTab => {
            db_manager.new_query_in_current_db();
            true
        }
        ShortcutAction::CloseQueryTab => {
            if !db_manager.close_current_query_tab() {
                window.close();
            }
            true
        }
        ShortcutAction::NextQueryTab => db_manager.select_next_query_tab(),
        ShortcutAction::PreviousQueryTab => db_manager.select_previous_query_tab(),
        ShortcutAction::RenameQueryTab => show_rename_query_dialog(db_manager, window),
        ShortcutAction::LocateQueryTab => {
            search_entry.set_text("select query tab");
            search_entry.grab_focus()
        }
        ShortcutAction::FocusQueryTabs => db_manager.focus_query_tabs(),
        ShortcutAction::FocusQueryEditor => db_manager.focus_query_editor(),
        ShortcutAction::FocusQueryResults => db_manager.focus_query_results(),
        ShortcutAction::PreviewResultRow => {
            db_manager.focus_query_results()
                && db_manager.show_focused_result()
                && db_manager.pin_result_preview()
        }
        ShortcutAction::FocusQueryHistory => db_manager.focus_query_history(),
        ShortcutAction::RunQuery => {
            db_manager.close_query_plan();
            db_manager.cancel_query_preview();
            db_manager.run_current_query();
            true
        }
        ShortcutAction::StopQuery => {
            db_manager.cancel_current_query();
            true
        }
        ShortcutAction::PreviewQuery => {
            db_manager.begin_query_preview();
            true
        }
        ShortcutAction::PeekPlan => {
            if !db_manager.show_query_plan() {
                db_manager.begin_query_preview();
            }
            db_manager.show_query_plan() && db_manager.pin_query_plan()
        }
        ShortcutAction::ClosePlan => {
            if db_manager.close_result_preview() {
                true
            } else if db_manager.close_query_plan() {
                db_manager.cancel_query_preview();
                true
            } else {
                false
            }
        }
        ShortcutAction::ShowShortcuts => {
            shortcuts_dialog.present(Some(window));
            true
        }
    }
}

fn show_rename_query_dialog(db_manager: &DatabaseTabManager, window: &ApplicationWindow) -> bool {
    let Some(current_title) = db_manager.current_query_tab_title() else {
        return false;
    };
    let dialog = adw::Dialog::new();
    dialog.set_title("Rename Query Tab");
    dialog.set_content_width(420);

    let toolbar = ToolbarView::new();
    let header = HeaderBar::new();
    header.set_show_start_title_buttons(true);
    toolbar.add_top_bar(&header);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    let entry = gtk4::Entry::new();
    entry.set_text(&current_title);
    entry.set_activates_default(true);
    entry.update_property(&[gtk4::accessible::Property::Label("Query tab name")]);
    let rename = gtk4::Button::with_label("Rename");
    rename.add_css_class("suggested-action");
    rename.set_halign(gtk4::Align::End);
    rename.set_receives_default(true);
    content.append(&entry);
    content.append(&rename);
    toolbar.set_content(Some(&content));
    dialog.set_child(Some(&toolbar));

    let manager_for_rename = db_manager.clone();
    let entry_for_rename = entry.clone();
    let dialog_for_rename = dialog.clone();
    rename.connect_clicked(move |_| {
        if manager_for_rename.rename_current_query_tab(&entry_for_rename.text()) {
            dialog_for_rename.close();
        }
    });
    let rename_for_activate = rename.clone();
    entry.connect_activate(move |_| rename_for_activate.emit_clicked());
    dialog.present(Some(window));
    entry.grab_focus();
    true
}

fn install_styles() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(
        "entry.query-action-field {
            min-height: 36px;
            border-color: transparent;
            background-color: transparent;
            box-shadow: none;
        }
        entry.query-action-field:focus {
            border-color: alpha(@accent_color, 0.55);
            background-color: alpha(@view_bg_color, 0.82);
        }
        progressbar.query-action-progress {
            min-width: 0;
            min-height: 2px;
            margin: 0;
            padding: 0;
        }
        progressbar.query-action-progress trough {
            min-width: 0;
            min-height: 2px;
            margin: 0;
            padding: 0;
            border: none;
            border-radius: 0;
            background-color: transparent;
        }
        progressbar.query-action-progress progress {
            min-width: 0;
            min-height: 2px;
            border-radius: 0;
            background-color: @accent_bg_color;
        }
        .plan-inspector {
            background-color: @window_bg_color;
            box-shadow: 0 6px 24px alpha(black, 0.28);
        }
        .query-history-pane {
            background-color: @view_bg_color;
            border-right: 1px solid alpha(@window_fg_color, 0.18);
        }
        .workspace-section-header {
            background-color: @headerbar_bg_color;
            border-bottom: 1px solid alpha(@window_fg_color, 0.18);
        }
        .workspace-list {
            background-color: @view_bg_color;
        }
        .workspace-list row {
            border-radius: 0;
        }
        .workspace-list row:hover {
            background-color: alpha(@view_fg_color, 0.06);
        }
        .workspace-list row:selected {
            background-color: alpha(@accent_bg_color, 0.16);
            color: @view_fg_color;
        }
        .query-history-list row {
            min-height: 54px;
            padding: 0;
            transition: background-color 140ms ease;
        }
        .query-history-content {
            padding: 9px 12px;
        }
        .query-history-title {
            font-weight: 600;
        }
        .query-history-status {
            border-radius: 999px;
        }
        .query-history-status.success {
            background-color: @success_color;
        }
        .query-history-status.error {
            background-color: @error_color;
        }
        .query-history-row .history-row-action {
            min-width: 20px;
            min-height: 20px;
            padding: 0;
            opacity: 0;
            transition: opacity 140ms ease;
        }
        .query-history-row:hover .history-row-action,
        .query-history-row:selected .history-row-action,
        .query-history-row:focus-within .history-row-action {
            opacity: 1;
        }
        paned.query-split > separator {
            min-height: 1px;
            background-color: alpha(@window_fg_color, 0.18);
        }
        .workspace-statusbar {
            border-top: 1px solid alpha(@window_fg_color, 0.18);
            background-color: @headerbar_bg_color;
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
    window: &ApplicationWindow,
    shortcuts_dialog: &adw::Dialog,
) -> (HeaderBar, gtk4::Entry) {
    use gtk4::{
        Button, Entry, EntryIconPosition, EventControllerKey, Label, ListBox, Popover,
        ScrolledWindow, gdk,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    let header = HeaderBar::new();

    // Left side - sidebar toggle
    let sidebar_toggle = Button::from_icon_name("sidebar-show-symbolic");
    sidebar_toggle.set_tooltip_text(Some(
        &shortcuts::shortcut(ShortcutAction::ToggleSidebar).tooltip(),
    ));

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
    db_tab_manager.set_query_action_callback(move |state| {
        if let Some(timer) = pulse_timer_for_preview.borrow_mut().take() {
            timer.remove();
        }

        search_for_preview.set_placeholder_text(Some(&query_action_message(&state)));
        match &state {
            QueryActionState::Loading
            | QueryActionState::Running
            | QueryActionState::SavingPlan => {
                let tooltip = match &state {
                    QueryActionState::Loading => "Release Ctrl+Shift to dismiss",
                    QueryActionState::Running => "Query is running",
                    QueryActionState::SavingPlan => "Saving the estimated query plan",
                    _ => unreachable!(),
                };
                search_for_preview.set_tooltip_text(Some(tooltip));
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
            QueryActionState::Error(error) => {
                search_for_preview.set_tooltip_text(Some(error));
                clear_progress(&progress_for_preview);
            }
            QueryActionState::Idle | QueryActionState::Ready { .. } => {
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

    #[derive(Clone)]
    enum SearchActionKind {
        Shortcut(ShortcutAction),
        OpenDatabase(String),
        FocusConnection(String),
        SelectDatabase(String),
        SelectQuery(u32),
    }

    #[derive(Clone)]
    struct SearchAction {
        title: String,
        subtitle: String,
        icon: String,
        action: SearchActionKind,
    }

    let manager_for_actions = db_tab_manager.clone();
    let create_search_actions = move |search_text: &str| -> Vec<SearchAction> {
        let mut actions = Vec::new();
        let search_lower = search_text.to_lowercase();

        for action in shortcuts::shortcuts() {
            let searchable = std::iter::once(action.name)
                .chain(std::iter::once(action.description))
                .chain(action.keywords.iter().copied())
                .any(|value| value.to_lowercase().contains(&search_lower));
            if search_text.is_empty() || searchable {
                actions.push(SearchAction {
                    title: action.name.to_string(),
                    subtitle: action.description.to_string(),
                    icon: "system-run-symbolic".to_string(),
                    action: SearchActionKind::Shortcut(action.action),
                });
            }
        }

        let databases = manager_for_actions.get_database_list();
        for db_name in &databases {
            let subtitle = "Open database connection";
            if search_text.is_empty()
                || db_name.to_lowercase().contains(&search_lower)
                || subtitle.to_lowercase().contains(&search_lower)
            {
                actions.push(SearchAction {
                    title: db_name.clone(),
                    subtitle: subtitle.to_string(),
                    icon: "server-database-symbolic".to_string(),
                    action: SearchActionKind::OpenDatabase(db_name.clone()),
                });
            }
        }

        for db_name in databases {
            let subtitle = "Focus database in the connection list";
            let searchable = format!("connection database {} {}", db_name, subtitle).to_lowercase();
            if search_text.is_empty() || searchable.contains(&search_lower) {
                actions.push(SearchAction {
                    title: format!("Connection: {}", db_name),
                    subtitle: subtitle.to_string(),
                    icon: "network-server-symbolic".to_string(),
                    action: SearchActionKind::FocusConnection(db_name),
                });
            }
        }

        for db_name in manager_for_actions.open_database_tabs() {
            let subtitle = "Select open database tab";
            let searchable = format!("database tab {} {}", db_name, subtitle).to_lowercase();
            if search_text.is_empty() || searchable.contains(&search_lower) {
                actions.push(SearchAction {
                    title: format!("Database tab: {}", db_name),
                    subtitle: subtitle.to_string(),
                    icon: "go-jump-symbolic".to_string(),
                    action: SearchActionKind::SelectDatabase(db_name),
                });
            }
        }

        for (query_num, title) in manager_for_actions.query_tabs() {
            let subtitle = "Select query tab";
            let searchable = format!("query tab {} {}", title, subtitle).to_lowercase();
            if search_text.is_empty() || searchable.contains(&search_lower) {
                actions.push(SearchAction {
                    title: format!("Query tab: {}", title),
                    subtitle: subtitle.to_string(),
                    icon: "go-jump-symbolic".to_string(),
                    action: SearchActionKind::SelectQuery(query_num),
                });
            }
        }

        actions
    };

    let execute_action = {
        let db_manager = db_tab_manager.clone();
        let toast = toast_overlay.clone();
        let window = window.clone();
        let shortcuts_dialog = shortcuts_dialog.clone();
        move |action: &SearchActionKind, entry: &Entry| {
            let message = match action {
                SearchActionKind::Shortcut(action) => {
                    execute_shortcut_action(
                        *action,
                        &db_manager,
                        entry,
                        &window,
                        &shortcuts_dialog,
                    );
                    None
                }
                SearchActionKind::OpenDatabase(db_name) => {
                    db_manager.open_database(db_name);
                    Some(format!("Opened: {}", db_name))
                }
                SearchActionKind::FocusConnection(db_name) => {
                    db_manager.focus_connection(db_name);
                    None
                }
                SearchActionKind::SelectDatabase(db_name) => {
                    db_manager.select_database_tab(db_name);
                    Some(format!("Selected database tab: {}", db_name))
                }
                SearchActionKind::SelectQuery(query_num) => {
                    db_manager.select_query_tab(*query_num);
                    None
                }
            };
            entry.set_text("");
            if let Some(message) = message {
                let toast_msg = Toast::new(&message);
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
                row.update_property(&[
                    gtk4::accessible::Property::Label(&action.title),
                    gtk4::accessible::Property::Description(&action.subtitle),
                ]);

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

                let action_kind = action.action.clone();
                let execute_clone = execute_action.clone();
                let entry_clone = db_search.clone();
                let popover_clone = suggestions_popover.clone();

                let action_button = Button::from_icon_name("go-next-symbolic");
                action_button.add_css_class("flat");
                action_button.update_property(&[gtk4::accessible::Property::Label(&format!(
                    "Run action: {}",
                    action.title
                ))]);
                let action_for_button = action_kind.clone();
                let execute_for_button = execute_action.clone();
                let entry_for_button = db_search.clone();
                let popover_for_button = suggestions_popover.clone();
                action_button.connect_clicked(move |_| {
                    execute_for_button(&action_for_button, &entry_for_button);
                    popover_for_button.popdown();
                });
                row.add_suffix(&action_button);

                row.connect_activated(move |_| {
                    execute_clone(&action_kind, &entry_clone);
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
    let update_suggestions_on_focus = update_suggestions.clone();
    db_search.connect_has_focus_notify(move |entry| {
        if entry.has_focus() {
            update_suggestions_on_focus(&entry.text());
        }
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
    run_button.set_tooltip_text(Some(
        &shortcuts::shortcut(ShortcutAction::RunQuery).tooltip(),
    ));
    run_button.update_property(&[
        gtk4::accessible::Property::Label("Run query"),
        gtk4::accessible::Property::KeyShortcuts("Control+Return Control+Shift+Return"),
    ]);
    run_button.add_css_class("suggested-action");

    let run_button_for_operation = run_button.clone();
    db_tab_manager.set_query_availability_callback(move |available| {
        run_button_for_operation.set_sensitive(available);
    });

    let db_manager_run = db_tab_manager.clone();
    run_button.connect_clicked(move |_| {
        db_manager_run.run_current_query();
    });

    header.pack_end(&run_button);

    // New query tab button
    let new_query_button = Button::from_icon_name("tab-new-symbolic");
    new_query_button.set_tooltip_text(Some(
        &shortcuts::shortcut(ShortcutAction::NewQueryTab).tooltip(),
    ));
    new_query_button.update_property(&[
        gtk4::accessible::Property::Label("New query tab"),
        gtk4::accessible::Property::KeyShortcuts("Control+t"),
    ]);

    let db_manager_clone = db_tab_manager.clone();
    new_query_button.connect_clicked(move |_| {
        db_manager_clone.new_query_in_current_db();
    });

    header.pack_end(&new_query_button);

    // Settings menu
    let menu_button = Button::from_icon_name("open-menu-symbolic");
    menu_button.set_tooltip_text(Some("Main Menu"));
    menu_button.update_property(&[gtk4::accessible::Property::Label("Main Menu")]);

    let menu_popover = Popover::new();
    let menu_content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    menu_content.set_margin_top(6);
    menu_content.set_margin_bottom(6);
    menu_content.set_margin_start(6);
    menu_content.set_margin_end(6);
    let shortcuts_button = Button::new();
    shortcuts_button.add_css_class("flat");
    shortcuts_button.set_halign(gtk4::Align::Fill);
    let shortcuts_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    shortcuts_content.append(&gtk4::Image::from_icon_name("input-keyboard-symbolic"));
    shortcuts_content.append(&Label::new(Some("Keyboard Shortcuts")));
    shortcuts_button.set_child(Some(&shortcuts_content));
    shortcuts_button.set_tooltip_text(Some(
        &shortcuts::shortcut(ShortcutAction::ShowShortcuts).tooltip(),
    ));
    shortcuts_button.update_property(&[
        gtk4::accessible::Property::Label("Keyboard shortcuts"),
        gtk4::accessible::Property::KeyShortcuts("F1"),
    ]);
    let shortcuts_dialog_for_button = shortcuts_dialog.clone();
    let window_for_button = window.clone();
    let menu_popover_for_button = menu_popover.clone();
    shortcuts_button.connect_clicked(move |_| {
        menu_popover_for_button.popdown();
        shortcuts_dialog_for_button.present(Some(&window_for_button));
    });
    menu_content.append(&shortcuts_button);
    menu_popover.set_child(Some(&menu_content));
    menu_popover.set_parent(&menu_button);
    let menu_popover_for_menu = menu_popover.clone();
    menu_button.connect_clicked(move |_| menu_popover_for_menu.popup());
    header.pack_end(&menu_button);

    (header, db_search)
}

fn query_action_message(state: &QueryActionState) -> String {
    match state {
        QueryActionState::Idle => default_action_hint(),
        QueryActionState::Loading => {
            "Enter to run - Space for plan - estimating rows and cost...".to_string()
        }
        QueryActionState::Ready { plan } => match (
            plan.estimate.rows,
            plan.estimate.startup_cost,
            plan.estimate.total_cost,
            plan.estimate.parts,
            plan.estimate.marks,
        ) {
            (Some(rows), Some(startup), Some(total), _, _) => {
                format!(
                    "Enter to run - Space for plan - ~{rows} rows - cost {startup:.2}..{total:.2}"
                )
            }
            (Some(rows), _, _, Some(parts), Some(marks)) => {
                format!(
                    "Enter to run - Space for plan - ~{rows} rows - {parts} parts - {marks} marks"
                )
            }
            (Some(rows), _, _, _, _) => {
                format!("Enter to run - Space for plan - ~{rows} rows")
            }
            _ => "Enter to run - Space for plan - estimate ready".to_string(),
        },
        QueryActionState::Running => "Running query...".to_string(),
        QueryActionState::SavingPlan => "Query complete - saving estimated plan...".to_string(),
        QueryActionState::Error(error) => {
            format!("Enter to run - estimate unavailable: {}", error)
        }
    }
}

fn default_action_hint() -> String {
    format!(
        "Actions and databases ({})",
        shortcuts::shortcut(ShortcutAction::FocusSearch).accelerator()
    )
}

fn create_query_action_field() -> (gtk4::Overlay, gtk4::Entry, gtk4::ProgressBar) {
    let overlay = gtk4::Overlay::new();
    overlay.set_halign(gtk4::Align::Center);
    overlay.set_width_request(440);
    overlay.set_overflow(gtk4::Overflow::Hidden);
    overlay.set_widget_name("query-action-control");

    let entry = gtk4::Entry::new();
    entry.set_widget_name("database-search-action");
    entry.update_property(&[
        gtk4::accessible::Property::Label("Database search and query action"),
        gtk4::accessible::Property::Description("Search actions or display query estimates"),
        gtk4::accessible::Property::KeyShortcuts("Control+k"),
    ]);
    entry.set_placeholder_text(Some(&default_action_hint()));
    entry.set_width_chars(50);
    gtk4::prelude::EntryExt::set_alignment(&entry, 0.5);
    entry.add_css_class("query-action-field");
    entry.connect_has_focus_notify(|entry| {
        if entry.has_focus() {
            gtk4::prelude::EntryExt::set_alignment(entry, 0.0);
            entry.set_icon_from_icon_name(
                gtk4::EntryIconPosition::Primary,
                Some("system-search-symbolic"),
            );
        } else {
            gtk4::prelude::EntryExt::set_alignment(entry, 0.5);
            entry.set_icon_from_icon_name(gtk4::EntryIconPosition::Primary, None);
        }
    });
    overlay.set_child(Some(&entry));

    let progress = gtk4::ProgressBar::new();
    progress.add_css_class("query-action-progress");
    progress.set_halign(gtk4::Align::Fill);
    progress.set_valign(gtk4::Align::End);
    progress.set_pulse_step(0.08);
    progress.set_can_target(false);
    progress.set_visible(false);
    overlay.add_overlay(&progress);
    overlay.set_measure_overlay(&progress, false);

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
    fn preview_chord_transitions_are_interpreted_without_a_display() {
        let control = gtk4::gdk::ModifierType::CONTROL_MASK;
        let shift = gtk4::gdk::ModifierType::SHIFT_MASK;

        assert_eq!(
            preview_key_pressed(gtk4::gdk::Key::Control_L, control),
            None
        );
        assert_eq!(
            preview_key_pressed(gtk4::gdk::Key::Shift_L, control),
            Some(PreviewKeyAction::Begin)
        );
        assert_eq!(
            preview_key_pressed(gtk4::gdk::Key::Control_L, shift),
            Some(PreviewKeyAction::Begin)
        );
        assert_eq!(
            preview_key_pressed(gtk4::gdk::Key::Shift_L, control | shift),
            Some(PreviewKeyAction::Begin)
        );
        assert_eq!(
            preview_key_released(gtk4::gdk::Key::Shift_L),
            Some(PreviewKeyAction::Cancel)
        );
        assert_eq!(
            preview_key_released(gtk4::gdk::Key::Control_L),
            Some(PreviewKeyAction::Cancel)
        );
    }

    #[test]
    fn plan_scroll_keys_require_the_plan_chord_or_no_modifiers() {
        let control = gtk4::gdk::ModifierType::CONTROL_MASK;
        let shift = gtk4::gdk::ModifierType::SHIFT_MASK;

        assert_eq!(
            preview_key_pressed(gtk4::gdk::Key::Page_Down, control | shift),
            Some(PreviewKeyAction::Scroll(PlanScroll::PageDown))
        );
        assert_eq!(
            preview_key_pressed(gtk4::gdk::Key::Down, gtk4::gdk::ModifierType::empty()),
            Some(PreviewKeyAction::Scroll(PlanScroll::LineDown))
        );
        assert_eq!(preview_key_pressed(gtk4::gdk::Key::Down, control), None);
        assert_eq!(
            preview_key_pressed(
                gtk4::gdk::Key::Down,
                control | shift | gtk4::gdk::ModifierType::ALT_MASK
            ),
            None
        );
    }

    #[test]
    fn ready_query_preview_prompts_for_execution_with_estimates() {
        let message = query_action_message(&QueryActionState::Ready {
            plan: dzban_db::QueryPlan {
                backend: dzban_db::DatabaseBackend::Postgresql,
                document: serde_json::json!([]),
                estimate: dzban_db::QueryEstimate {
                    rows: Some(42),
                    startup_cost: Some(1.25),
                    total_cost: Some(9.5),
                    parts: None,
                    marks: None,
                },
            },
        });

        assert_eq!(
            message,
            "Enter to run - Space for plan - ~42 rows - cost 1.25..9.50"
        );
    }

    #[test]
    fn clickhouse_preview_shows_scan_estimates_without_fake_cost() {
        let message = query_action_message(&QueryActionState::Ready {
            plan: dzban_db::QueryPlan {
                backend: dzban_db::DatabaseBackend::Clickhouse,
                document: serde_json::json!({}),
                estimate: dzban_db::QueryEstimate {
                    rows: Some(42),
                    startup_cost: None,
                    total_cost: None,
                    parts: Some(2),
                    marks: Some(7),
                },
            },
        });

        assert_eq!(
            message,
            "Enter to run - Space for plan - ~42 rows - 2 parts - 7 marks"
        );
    }

    #[test]
    fn running_query_and_plan_storage_are_distinct_states() {
        assert_eq!(
            query_action_message(&QueryActionState::Running),
            "Running query..."
        );
        assert_eq!(
            query_action_message(&QueryActionState::SavingPlan),
            "Query complete - saving estimated plan..."
        );
    }

    #[test]
    fn query_preview_errors_are_visible_without_a_tooltip() {
        let message = query_action_message(&QueryActionState::Error(
            "Database is not connected".to_string(),
        ));

        assert_eq!(
            message,
            "Enter to run - estimate unavailable: Database is not connected"
        );
        assert!(!message.contains("hover"));
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
        assert!(progress.height() <= 2);
        assert_eq!((field.width(), field.height()), baseline);
        assert_eq!(field.overflow(), gtk4::Overflow::Hidden);

        clear_progress(&progress);
        gtk4::test_widget_wait_for_draw(&field);
        assert!(!progress.is_visible());
        assert_eq!(progress.fraction(), 0.0);
        assert_eq!((field.width(), field.height()), baseline);

        progress.set_visible(true);
        progress.pulse();
        gtk4::test_widget_wait_for_draw(&field);
        assert!(progress.is_visible());
        assert!(progress.height() <= 2);
        assert_eq!((field.width(), field.height()), baseline);

        clear_progress(&progress);
        assert_eq!(
            entry.placeholder_text().as_deref(),
            Some(default_action_hint().as_str())
        );
        window.close();
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn close_query_action_returns_to_connections_before_closing_the_window() {
        let statusbar = create_statusbar();
        let manager = DatabaseTabManager::new(statusbar);
        manager.open_test_database("Close Test");
        let window = ApplicationWindow::builder().build();
        let search = gtk4::Entry::new();
        let shortcuts = shortcuts::create_shortcuts_window();
        window.present();
        gtk4::test_widget_wait_for_draw(&window);

        assert!(execute_shortcut_action(
            ShortcutAction::CloseQueryTab,
            &manager,
            &search,
            &window,
            &shortcuts,
        ));
        assert!(window.is_visible());
        assert!(manager.open_database_tabs().is_empty());

        assert!(execute_shortcut_action(
            ShortcutAction::CloseQueryTab,
            &manager,
            &search,
            &window,
            &shortcuts,
        ));
        assert!(!window.is_visible());
    }
}
