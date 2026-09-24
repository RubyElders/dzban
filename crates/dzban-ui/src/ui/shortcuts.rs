use adw::prelude::*;
use gtk4::prelude::*;
use gtk4::{EventControllerKey, gdk};
use libadwaita as adw;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    FocusSearch,
    ShowConnections,
    TestAllConnections,
    FocusDatabaseTabs,
    NextDatabaseTab,
    PreviousDatabaseTab,
    LocateDatabaseTab,
    ToggleSidebar,
    NewQueryTab,
    CloseQueryTab,
    NextQueryTab,
    PreviousQueryTab,
    RenameQueryTab,
    LocateQueryTab,
    FocusQueryTabs,
    FocusQueryEditor,
    FocusQueryResults,
    PreviewResultRow,
    FocusQueryHistory,
    RunQuery,
    StopQuery,
    PreviewQuery,
    PeekPlan,
    PinPlan,
    ClosePlan,
    ShowShortcuts,
}

#[derive(Debug, Clone)]
enum ShortcutTrigger {
    Key {
        key: gdk::Key,
        modifiers: gdk::ModifierType,
    },
    Chord(&'static str),
}

#[derive(Debug, Clone)]
pub struct Shortcut {
    pub action: ShortcutAction,
    pub name: &'static str,
    pub description: &'static str,
    pub group: &'static str,
    trigger: Option<ShortcutTrigger>,
    pub keywords: &'static [&'static str],
}

impl Shortcut {
    fn matches(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
        matches!(
            self.trigger,
            Some(
            ShortcutTrigger::Key {
                key: expected_key,
                modifiers: expected_modifiers,
            }) if key == expected_key
                && (modifiers == expected_modifiers
                    || (self.action == ShortcutAction::RunQuery
                        && modifiers
                            == expected_modifiers | gdk::ModifierType::SHIFT_MASK))
        )
    }

    pub fn accelerator(&self) -> String {
        let direct = match self.trigger {
            Some(ShortcutTrigger::Chord(label)) => Some(label.to_string()),
            Some(ShortcutTrigger::Key { key, modifiers }) => {
                let mut parts = Vec::new();
                if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                    parts.push("Ctrl".to_string());
                }
                if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                    parts.push("Shift".to_string());
                }
                if modifiers.contains(gdk::ModifierType::ALT_MASK) {
                    parts.push("Alt".to_string());
                }
                let key = match key {
                    gdk::Key::Return | gdk::Key::KP_Enter => "Enter".to_string(),
                    gdk::Key::space => "Space".to_string(),
                    gdk::Key::period => ".".to_string(),
                    gdk::Key::question => "?".to_string(),
                    _ => key.name().unwrap_or_default().to_string(),
                };
                parts.push(key);
                Some(parts.join("+"))
            }
            None => None,
        };
        direct.unwrap_or_default()
    }

    pub fn tooltip(&self) -> String {
        format!("{} ({})", self.name, self.accelerator())
    }
}

static SHORTCUTS: OnceLock<Vec<Shortcut>> = OnceLock::new();

fn key(
    action: ShortcutAction,
    key: gdk::Key,
    modifiers: gdk::ModifierType,
    name: &'static str,
    description: &'static str,
    group: &'static str,
) -> Shortcut {
    Shortcut {
        action,
        name,
        description,
        group,
        trigger: Some(ShortcutTrigger::Key { key, modifiers }),
        keywords: &[],
    }
}

fn chord(
    action: ShortcutAction,
    accelerator: &'static str,
    name: &'static str,
    description: &'static str,
    group: &'static str,
) -> Shortcut {
    Shortcut {
        action,
        name,
        description,
        group,
        trigger: Some(ShortcutTrigger::Chord(accelerator)),
        keywords: &[],
    }
}

fn action(
    action: ShortcutAction,
    name: &'static str,
    description: &'static str,
    group: &'static str,
    keywords: &'static [&'static str],
) -> Shortcut {
    Shortcut {
        action,
        name,
        description,
        group,
        trigger: None,
        keywords,
    }
}

fn init_shortcuts() -> Vec<Shortcut> {
    let control = gdk::ModifierType::CONTROL_MASK;
    let shift = gdk::ModifierType::SHIFT_MASK;
    vec![
        key(
            ShortcutAction::FocusSearch,
            gdk::Key::k,
            control,
            "Open Action Menu",
            "Search commands, databases, and open tabs",
            "Navigation",
        ),
        action(
            ShortcutAction::ShowConnections,
            "Show Connections",
            "Open the connection list and focus the selected database",
            "Connections",
            &["database", "open", "manage"],
        ),
        action(
            ShortcutAction::TestAllConnections,
            "Test All Connections",
            "Test every configured database connection",
            "Connections",
            &["database", "health", "ping"],
        ),
        action(
            ShortcutAction::FocusDatabaseTabs,
            "Focus Database Tabs",
            "Move keyboard focus to the database tab strip",
            "Database Tabs",
            &["jump", "workspace"],
        ),
        action(
            ShortcutAction::PreviousDatabaseTab,
            "Previous Database Tab",
            "Select the database workspace to the left",
            "Database Tabs",
            &["left", "previous"],
        ),
        action(
            ShortcutAction::NextDatabaseTab,
            "Next Database Tab",
            "Select the database workspace to the right",
            "Database Tabs",
            &["right", "next"],
        ),
        action(
            ShortcutAction::LocateDatabaseTab,
            "Locate Database Tab",
            "Find and select an open database workspace",
            "Database Tabs",
            &["find", "switch", "workspace"],
        ),
        key(
            ShortcutAction::ToggleSidebar,
            gdk::Key::b,
            control,
            "Toggle Query History",
            "Show or hide query history for the current database",
            "Navigation",
        ),
        key(
            ShortcutAction::NewQueryTab,
            gdk::Key::t,
            control,
            "New Query Tab",
            "Open a query tab in the current database",
            "Navigation",
        ),
        key(
            ShortcutAction::CloseQueryTab,
            gdk::Key::w,
            control,
            "Close Query Tab",
            "Close the selected query tab",
            "Navigation",
        ),
        key(
            ShortcutAction::PreviousQueryTab,
            gdk::Key::Page_Up,
            control,
            "Previous Query Tab",
            "Select the query tab to the left",
            "Query Tabs",
        ),
        key(
            ShortcutAction::NextQueryTab,
            gdk::Key::Page_Down,
            control,
            "Next Query Tab",
            "Select the query tab to the right",
            "Query Tabs",
        ),
        action(
            ShortcutAction::RenameQueryTab,
            "Rename Query Tab",
            "Rename the selected query tab",
            "Query Tabs",
            &["title", "name"],
        ),
        action(
            ShortcutAction::LocateQueryTab,
            "Locate Query Tab",
            "Find and select a query tab",
            "Query Tabs",
            &["find", "switch"],
        ),
        action(
            ShortcutAction::FocusQueryTabs,
            "Focus Query Tabs",
            "Move keyboard focus to the query tab strip",
            "Query Tabs",
            &["jump", "list"],
        ),
        action(
            ShortcutAction::FocusQueryEditor,
            "Focus Query Editor",
            "Move keyboard focus to the selected query editor",
            "Query",
            &["jump", "sql", "input"],
        ),
        action(
            ShortcutAction::FocusQueryResults,
            "Focus Query Results",
            "Move keyboard focus to the selected query output",
            "Query",
            &["jump", "output", "error"],
        ),
        chord(
            ShortcutAction::PreviewResultRow,
            "Tap or hold Space",
            "Preview Result Row",
            "Tap to keep the selected row open, or hold to peek until release",
            "Query",
        ),
        action(
            ShortcutAction::FocusQueryHistory,
            "Focus Query History",
            "Show query history and move keyboard focus to it",
            "Query",
            &["jump", "saved", "list"],
        ),
        chord(
            ShortcutAction::PreviewQuery,
            "Hold Ctrl",
            "Preview Query",
            "Highlight the current statement and estimate its rows and cost",
            "Query",
        ),
        chord(
            ShortcutAction::PeekPlan,
            "Hold Ctrl+Shift",
            "Peek at Query Plan",
            "Show the estimated plan until the modifiers are released",
            "Query",
        ),
        key(
            ShortcutAction::PinPlan,
            gdk::Key::space,
            control | shift,
            "Pin Query Plan",
            "Keep the temporary plan inspector open",
            "Query",
        ),
        key(
            ShortcutAction::RunQuery,
            gdk::Key::Return,
            control,
            "Run Query",
            "Execute the highlighted SQL statement",
            "Query",
        ),
        key(
            ShortcutAction::StopQuery,
            gdk::Key::period,
            control,
            "Stop Query",
            "Cancel the running query",
            "Query",
        ),
        key(
            ShortcutAction::ClosePlan,
            gdk::Key::Escape,
            gdk::ModifierType::empty(),
            "Close Query Plan",
            "Close a pinned query plan inspector",
            "Query",
        ),
        key(
            ShortcutAction::ShowShortcuts,
            gdk::Key::question,
            control | shift,
            "Keyboard Shortcuts",
            "Show this shortcut reference",
            "Help",
        ),
    ]
}

pub fn shortcuts() -> &'static [Shortcut] {
    SHORTCUTS.get_or_init(init_shortcuts)
}

pub fn shortcut(action: ShortcutAction) -> &'static Shortcut {
    shortcuts()
        .iter()
        .find(|shortcut| shortcut.action == action)
        .expect("every shortcut action is registered")
}

pub fn create_controller<F>(callback: F) -> EventControllerKey
where
    F: Fn(ShortcutAction) -> bool + 'static,
{
    let controller = EventControllerKey::new();
    controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        let modifiers = modifiers
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::ALT_MASK);
        for shortcut in shortcuts() {
            if shortcut.matches(key, modifiers) && callback(shortcut.action) {
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    controller
}

pub fn create_shortcuts_window() -> adw::Dialog {
    use gtk4::{Box, Label, ListBox, Orientation, ScrolledWindow};

    let content = Box::new(Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let title = Label::new(Some("Keyboard Shortcuts"));
    title.add_css_class("title-1");
    title.set_halign(gtk4::Align::Start);
    content.append(&title);

    let mut current_group = None;
    let mut list = None;
    for shortcut in shortcuts() {
        if shortcut.trigger.is_none() {
            continue;
        }
        if current_group != Some(shortcut.group) {
            let group_title = Label::new(Some(shortcut.group));
            group_title.add_css_class("title-4");
            group_title.set_halign(gtk4::Align::Start);
            group_title.set_margin_top(12);
            content.append(&group_title);

            let group_list = ListBox::new();
            group_list.add_css_class("boxed-list");
            content.append(&group_list);
            list = Some(group_list);
            current_group = Some(shortcut.group);
        }

        let row = adw::ActionRow::new();
        row.set_title(shortcut.name);
        row.set_subtitle(shortcut.description);
        let accelerator = Label::new(Some(&shortcut.accelerator()));
        accelerator.add_css_class("dim-label");
        accelerator.add_css_class("monospace");
        row.add_suffix(&accelerator);
        list.as_ref().unwrap().append(&row);
    }

    let scrolled = ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled.set_max_content_height(650);
    scrolled.set_propagate_natural_height(true);
    scrolled.set_child(Some(&content));

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(true);
    header.set_show_start_title_buttons(false);
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scrolled));

    let dialog = adw::Dialog::new();
    dialog.set_title("Keyboard Shortcuts");
    dialog.set_content_width(560);
    dialog.set_content_height(680);
    dialog.set_child(Some(&toolbar));
    dialog
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_action_has_one_name_and_description() {
        let mut actions = HashSet::new();
        for shortcut in shortcuts() {
            assert!(actions.insert(shortcut.action));
            assert!(!shortcut.name.is_empty());
            assert!(!shortcut.description.is_empty());
        }
    }

    #[test]
    fn plan_chords_are_registered_with_user_facing_labels() {
        assert_eq!(
            shortcut(ShortcutAction::PreviewQuery).accelerator(),
            "Hold Ctrl"
        );
        assert_eq!(
            shortcut(ShortcutAction::PeekPlan).accelerator(),
            "Hold Ctrl+Shift"
        );
        assert_eq!(
            shortcut(ShortcutAction::PinPlan).accelerator(),
            "Ctrl+Shift+Space"
        );
    }

    #[test]
    fn run_query_accepts_enter_while_plan_chord_is_held() {
        let run = shortcut(ShortcutAction::RunQuery);
        assert!(run.matches(
            gdk::Key::Return,
            gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK
        ));
    }

    #[test]
    fn navigation_actions_live_in_the_action_menu_without_hidden_shortcuts() {
        for action in [
            ShortcutAction::ShowConnections,
            ShortcutAction::FocusDatabaseTabs,
            ShortcutAction::FocusQueryTabs,
            ShortcutAction::FocusQueryEditor,
            ShortcutAction::FocusQueryResults,
            ShortcutAction::FocusQueryHistory,
        ] {
            assert!(shortcut(action).accelerator().is_empty());
        }
    }
}
