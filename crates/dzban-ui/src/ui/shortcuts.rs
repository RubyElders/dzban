use adw::prelude::*;
use gtk4::prelude::*;
use gtk4::{EventControllerKey, gdk};
use libadwaita as adw;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Keyboard shortcut definition
#[derive(Debug, Clone)]
pub struct Shortcut {
    pub id: &'static str,
    pub key: gdk::Key,
    pub modifiers: gdk::ModifierType,
    pub title: &'static str,
    pub description: &'static str,
    pub group: &'static str,
}

impl Shortcut {
    /// Check if a key event matches this shortcut
    pub fn matches(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
        self.key == key && self.modifiers == modifiers
    }

    /// Get the accelerator string (e.g., "Ctrl+K")
    pub fn accelerator(&self) -> String {
        let mut parts = Vec::new();

        if self.modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            parts.push("Ctrl");
        }
        if self.modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
            parts.push("Shift");
        }
        if self.modifiers.contains(gdk::ModifierType::ALT_MASK) {
            parts.push("Alt");
        }

        // Add the key
        let key_name = self.key.name().unwrap_or_default();
        parts.push(key_name.as_str());

        parts.join("+")
    }
}

/// Global shortcuts registry - single source of truth
static SHORTCUTS: OnceLock<HashMap<&'static str, Shortcut>> = OnceLock::new();

/// Initialize shortcuts registry
fn init_shortcuts() -> HashMap<&'static str, Shortcut> {
    let mut map = HashMap::new();

    // Navigation shortcuts
    map.insert(
        "search_focus",
        Shortcut {
            id: "search_focus",
            key: gdk::Key::k,
            modifiers: gdk::ModifierType::CONTROL_MASK,
            title: "Focus Search",
            description: "Focus the database search field",
            group: "Navigation",
        },
    );

    map.insert(
        "new_query_tab",
        Shortcut {
            id: "new_query_tab",
            key: gdk::Key::t,
            modifiers: gdk::ModifierType::CONTROL_MASK,
            title: "New Query Tab",
            description: "Open a new query tab in the current database",
            group: "Navigation",
        },
    );

    map.insert(
        "close_tab",
        Shortcut {
            id: "close_tab",
            key: gdk::Key::w,
            modifiers: gdk::ModifierType::CONTROL_MASK,
            title: "Close Tab",
            description: "Close the current query tab",
            group: "Navigation",
        },
    );

    // Query execution shortcuts
    map.insert(
        "run_query",
        Shortcut {
            id: "run_query",
            key: gdk::Key::Return,
            modifiers: gdk::ModifierType::CONTROL_MASK,
            title: "Run Query",
            description: "Execute the current SQL query",
            group: "Query Execution",
        },
    );

    map.insert(
        "stop_query",
        Shortcut {
            id: "stop_query",
            key: gdk::Key::period,
            modifiers: gdk::ModifierType::CONTROL_MASK,
            title: "Stop Query",
            description: "Cancel the running query",
            group: "Query Execution",
        },
    );

    // Help shortcuts
    map.insert(
        "show_shortcuts",
        Shortcut {
            id: "show_shortcuts",
            key: gdk::Key::question,
            modifiers: gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
            title: "Keyboard Shortcuts",
            description: "Show this help dialog",
            group: "Help",
        },
    );

    map
}

/// Get shortcuts registry
pub fn shortcuts() -> &'static HashMap<&'static str, Shortcut> {
    SHORTCUTS.get_or_init(init_shortcuts)
}

/// Create a keyboard event handler that dispatches to shortcut callbacks
pub fn create_controller<F>(callback: F) -> EventControllerKey
where
    F: Fn(&str) -> bool + 'static,
{
    let controller = EventControllerKey::new();

    // Set propagation phase to CAPTURE so we get events before text widgets
    controller.set_propagation_phase(gtk4::PropagationPhase::Capture);

    controller.connect_key_pressed(move |_controller, key, _code, modifiers| {
        // Normalize modifiers (ignore locks)
        let modifiers = modifiers
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::ALT_MASK);

        tracing::debug!(
            "Key pressed: {:?} with modifiers: {:?}",
            key.name(),
            modifiers
        );

        // Check all shortcuts for a match
        for shortcut in shortcuts().values() {
            if shortcut.matches(key, modifiers) {
                tracing::info!("Shortcut matched: {}", shortcut.id);
                if callback(shortcut.id) {
                    return glib::Propagation::Stop;
                }
            }
        }

        glib::Propagation::Proceed
    });

    controller
}

/// Create the shortcuts help dialog
pub fn create_shortcuts_window() -> adw::Dialog {
    use gtk4::{Box, Label, ListBox, Orientation, ScrolledWindow};

    // Group shortcuts by category
    let mut groups: HashMap<&str, Vec<&Shortcut>> = HashMap::new();
    for shortcut in shortcuts().values() {
        groups.entry(shortcut.group).or_default().push(shortcut);
    }

    let content = Box::new(Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    // Title
    let title = Label::new(Some("Keyboard Shortcuts"));
    title.add_css_class("title-1");
    title.set_halign(gtk4::Align::Start);
    content.append(&title);

    // Create sections for each group
    for (group_name, shortcuts_list) in groups.iter() {
        // Group title
        let group_title = Label::new(Some(group_name));
        group_title.add_css_class("title-4");
        group_title.set_halign(gtk4::Align::Start);
        group_title.set_margin_top(12);
        content.append(&group_title);

        // List of shortcuts
        let list = ListBox::new();
        list.add_css_class("boxed-list");

        for shortcut in shortcuts_list {
            let row = adw::ActionRow::new();
            row.set_title(shortcut.title);
            row.set_subtitle(shortcut.description);

            let accel_label = Label::new(Some(&shortcut.accelerator()));
            accel_label.add_css_class("dim-label");
            accel_label.add_css_class("monospace");
            row.add_suffix(&accel_label);

            list.append(&row);
        }

        content.append(&list);
    }

    let scrolled = ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled.set_max_content_height(600);
    scrolled.set_propagate_natural_height(true);
    scrolled.set_child(Some(&content));

    // Create toolbar with header bar (same close button style as main window)
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    // Use system default close button (same as main window)
    header.set_show_end_title_buttons(true);
    header.set_show_start_title_buttons(false);

    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scrolled));

    let dialog = adw::Dialog::new();
    dialog.set_title("Keyboard Shortcuts");
    dialog.set_content_width(500);
    dialog.set_content_height(600);
    dialog.set_child(Some(&toolbar));

    dialog
}
