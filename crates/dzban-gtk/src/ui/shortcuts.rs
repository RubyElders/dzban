use adw::prelude::*;
use gtk4::gdk;
use libadwaita as adw;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    FocusSearch,
    FocusEditor,
    FocusResults,
    CycleFocus,
    FocusHistory,
    FocusTabs,
    ToggleHistory,
    ShowConnections,
    TestAllConnections,
    NewTab,
    CloseTab,
    NextTab,
    PreviousTab,
    RenameTab,
    Disconnect,
    RunQuery,
    EstimateAndRun,
    Estimate,
    OpenPlan,
    StopQuery,
    PreviewRow,
    CopyResults,
    ToggleResultRows,
    ToggleFrozenColumn,
    ShowShortcuts,
    Fullscreen,
    Quit,
}

#[derive(Debug, Clone)]
enum Trigger {
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
    triggers: Vec<Trigger>,
    pub keywords: &'static [&'static str],
}

impl Shortcut {
    pub fn domain_action(&self) -> Option<&'static dzban_core::action::ActionDescriptor> {
        let id = match self.action {
            ShortcutAction::RunQuery => "query.submit",
            ShortcutAction::StopQuery => "query.cancel",
            ShortcutAction::Estimate => "query.plan",
            _ => return None,
        };
        dzban_core::action::ActionRegistry::shared_portable().get(id)
    }

    pub fn matches(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
        self.triggers.iter().any(|trigger| {
            matches!(
                trigger,
                Trigger::Key {
                    key: expected,
                    modifiers: expected_modifiers,
                } if key.to_lower() == expected.to_lower() && modifiers == *expected_modifiers
            )
        })
    }

    pub fn has_trigger(&self) -> bool {
        !self.triggers.is_empty()
    }

    pub fn accelerator(&self) -> String {
        self.triggers
            .iter()
            .map(|trigger| match trigger {
                Trigger::Chord(label) => label.to_string(),
                Trigger::Key { key, modifiers } => {
                    let mut parts = Vec::new();
                    if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                        parts.push("Ctrl".to_string());
                    }
                    if modifiers.contains(gdk::ModifierType::SHIFT_MASK)
                        && *key != gdk::Key::question
                    {
                        parts.push("Shift".to_string());
                    }
                    if modifiers.contains(gdk::ModifierType::ALT_MASK) {
                        parts.push("Alt".to_string());
                    }
                    parts.push(match *key {
                        gdk::Key::Return | gdk::Key::KP_Enter => "Enter".into(),
                        gdk::Key::space => "Space".into(),
                        gdk::Key::question => "?".into(),
                        gdk::Key::Page_Up => "Page Up".into(),
                        gdk::Key::Page_Down => "Page Down".into(),
                        gdk::Key::ISO_Left_Tab => "Tab".into(),
                        key => key
                            .to_upper()
                            .name()
                            .map(|name| name.to_string())
                            .unwrap_or_default(),
                    });
                    parts.join("+")
                }
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }

    pub fn tooltip(&self) -> String {
        if self.triggers.is_empty() {
            self.name.to_string()
        } else {
            format!("{} ({})", self.name, self.accelerator())
        }
    }
}

const CONTROL: gdk::ModifierType = gdk::ModifierType::CONTROL_MASK;
const SHIFT: gdk::ModifierType = gdk::ModifierType::SHIFT_MASK;
const NONE: gdk::ModifierType = gdk::ModifierType::empty();

fn entry(
    action: ShortcutAction,
    name: &'static str,
    description: &'static str,
    group: &'static str,
    triggers: Vec<Trigger>,
    keywords: &'static [&'static str],
) -> Shortcut {
    Shortcut {
        action,
        name,
        description,
        group,
        triggers,
        keywords,
    }
}

fn key(key: gdk::Key, modifiers: gdk::ModifierType) -> Trigger {
    Trigger::Key { key, modifiers }
}

fn init() -> Vec<Shortcut> {
    use ShortcutAction as Action;
    use Trigger::Chord;
    vec![
        entry(
            Action::RunQuery,
            "Run Query",
            "",
            "Query",
            vec![key(gdk::Key::Return, CONTROL)],
            &["execute", "stop"],
        ),
        entry(
            Action::EstimateAndRun,
            "Estimate and Run",
            "Estimate the query, then run it",
            "Query",
            vec![key(gdk::Key::Return, CONTROL | SHIFT)],
            &["plan", "execute"],
        ),
        entry(
            Action::Estimate,
            "Estimate Query",
            "",
            "Query",
            vec![Chord("Hold Ctrl+Shift")],
            &["estimate", "plan"],
        ),
        entry(
            Action::OpenPlan,
            "Preview Query Plan",
            "Open the plan; tap Space to keep it open or hold to peek",
            "Query",
            vec![key(gdk::Key::space, CONTROL | SHIFT)],
            &["explain", "plan", "estimate"],
        ),
        entry(
            Action::StopQuery,
            "Stop Query",
            "",
            "Query",
            vec![Chord("Escape")],
            &["cancel"],
        ),
        entry(
            Action::PreviewRow,
            "Preview Result Row",
            "Tap Space to keep the row open, or hold to peek; arrows move between rows and columns",
            "Results",
            vec![Chord("Space")],
            &["row", "inspect", "peek"],
        ),
        entry(
            Action::CopyResults,
            "Copy Results",
            "Copy one value as text or a multi-selection as Markdown",
            "Results",
            vec![key(gdk::Key::c, CONTROL)],
            &["clipboard", "markdown", "export"],
        ),
        entry(
            Action::ToggleResultRows,
            "Toggle Whole Result Rows",
            "Switch between cells and whole rows without moving the viewport",
            "Results",
            vec![key(gdk::Key::space, CONTROL)],
            &["select", "row", "cells"],
        ),
        entry(
            Action::ToggleFrozenColumn,
            "Freeze or Unfreeze Result Column",
            "Keep the current column visible at the nearest table edge",
            "Results",
            vec![Chord("X")],
            &["lock", "sticky", "pin"],
        ),
        entry(
            Action::CycleFocus,
            "Toggle Editor and Results",
            "Move focus between the SQL editor and its results",
            "Navigation",
            vec![key(gdk::Key::k, CONTROL)],
            &["focus", "switch"],
        ),
        entry(
            Action::FocusSearch,
            "Search Actions",
            "Search commands, connections and open tabs",
            "Navigation",
            vec![key(gdk::Key::l, CONTROL)],
            &["command", "palette", "find"],
        ),
        entry(
            Action::ToggleHistory,
            "Query History",
            "Show or hide saved queries on the left",
            "Navigation",
            vec![key(gdk::Key::b, CONTROL)],
            &["sidebar", "saved"],
        ),
        entry(
            Action::NewTab,
            "New Query Tab",
            "Choose a connection for a new query",
            "Tabs",
            vec![key(gdk::Key::t, CONTROL)],
            &["open", "connection"],
        ),
        entry(
            Action::CloseTab,
            "Close Query Tab",
            "Close the selected tab",
            "Tabs",
            vec![key(gdk::Key::w, CONTROL)],
            &["close"],
        ),
        entry(
            Action::NextTab,
            "Next Query Tab",
            "Select the next tab",
            "Tabs",
            vec![
                key(gdk::Key::Tab, CONTROL),
                key(gdk::Key::Page_Down, CONTROL),
            ],
            &["right", "switch"],
        ),
        entry(
            Action::PreviousTab,
            "Previous Query Tab",
            "Select the previous tab",
            "Tabs",
            vec![
                key(gdk::Key::ISO_Left_Tab, CONTROL | SHIFT),
                key(gdk::Key::Page_Up, CONTROL),
            ],
            &["left", "switch"],
        ),
        entry(
            Action::RenameTab,
            "Rename Query Tab",
            "Rename the selected query",
            "Tabs",
            Vec::new(),
            &["title", "name"],
        ),
        entry(
            Action::FocusEditor,
            "Focus Query Editor",
            "Return to SQL at the current cursor position",
            "Navigation",
            Vec::new(),
            &["jump", "sql"],
        ),
        entry(
            Action::FocusResults,
            "Focus Results",
            "Navigate result rows and cells",
            "Navigation",
            Vec::new(),
            &["jump", "output", "table"],
        ),
        entry(
            Action::FocusHistory,
            "Focus Query History",
            "Show query history and move keyboard focus to it",
            "Navigation",
            Vec::new(),
            &["jump", "saved"],
        ),
        entry(
            Action::FocusTabs,
            "Focus Query Tabs",
            "Move keyboard focus to the tab strip",
            "Navigation",
            Vec::new(),
            &["jump"],
        ),
        entry(
            Action::ShowConnections,
            "Show Connections",
            "Choose a database connection",
            "Connections",
            Vec::new(),
            &["database", "open", "chooser"],
        ),
        entry(
            Action::TestAllConnections,
            "Test All Connections",
            "Probe configured servers and measure delay",
            "Connections",
            Vec::new(),
            &["health", "ping", "probe"],
        ),
        entry(
            Action::Disconnect,
            "Disconnect",
            "Close the current workspace",
            "Connections",
            Vec::new(),
            &["close", "session"],
        ),
        entry(
            Action::ShowShortcuts,
            "Keyboard Shortcuts",
            "Show the keyboard reference",
            "Help",
            vec![
                key(gdk::Key::question, CONTROL | SHIFT),
                key(gdk::Key::F1, NONE),
            ],
            &["help", "keys"],
        ),
        entry(
            Action::Fullscreen,
            "Fullscreen",
            "Toggle fullscreen",
            "Help",
            vec![key(gdk::Key::F11, NONE)],
            &["window", "maximize"],
        ),
        entry(
            Action::Quit,
            "Quit",
            "Save open tabs and close Dzban",
            "Help",
            vec![key(gdk::Key::q, CONTROL)],
            &["exit", "close"],
        ),
    ]
}

pub fn shortcuts() -> &'static [Shortcut] {
    static SHORTCUTS: OnceLock<Vec<Shortcut>> = OnceLock::new();
    SHORTCUTS.get_or_init(|| {
        let mut shortcuts = init();
        for shortcut in &mut shortcuts {
            if let Some(action) = shortcut.domain_action() {
                shortcut.description = &action.description;
            }
        }
        shortcuts
    })
}

pub fn shortcut(action: ShortcutAction) -> &'static Shortcut {
    shortcuts()
        .iter()
        .find(|shortcut| shortcut.action == action)
        .expect("every shortcut action is registered")
}

pub fn matching(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<ShortcutAction> {
    let modifiers = modifiers & (CONTROL | SHIFT | gdk::ModifierType::ALT_MASK);
    shortcuts()
        .iter()
        .find(|shortcut| shortcut.matches(key, modifiers))
        .map(|shortcut| shortcut.action)
}

pub fn create_dialog() -> adw::Dialog {
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let mut group = None;
    let mut list = None;
    for shortcut in shortcuts() {
        if !shortcut.has_trigger() {
            continue;
        }
        if group != Some(shortcut.group) {
            let title = gtk4::Label::new(Some(shortcut.group));
            title.add_css_class("heading");
            title.set_halign(gtk4::Align::Start);
            title.set_margin_top(12);
            content.append(&title);
            let rows = gtk4::ListBox::new();
            rows.add_css_class("boxed-list");
            rows.set_selection_mode(gtk4::SelectionMode::None);
            content.append(&rows);
            list = Some(rows);
            group = Some(shortcut.group);
        }
        let row = adw::ActionRow::new();
        row.set_title(shortcut.name);
        row.set_subtitle(shortcut.description);
        let accelerator = gtk4::Label::new(Some(&shortcut.accelerator()));
        accelerator.add_css_class("dim-label");
        accelerator.add_css_class("monospace");
        row.add_suffix(&accelerator);
        list.as_ref().unwrap().append(&row);
    }

    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scroll.set_propagate_natural_height(true);
    scroll.set_max_content_height(578);
    scroll.set_child(Some(&content));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&scroll));
    let dialog = adw::Dialog::new();
    dialog.set_title("Keyboard Shortcuts");
    dialog.set_content_width(720);
    dialog.set_content_height(578);
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
            assert!(!shortcut.description.is_empty(), "{}", shortcut.name);
        }
    }

    #[test]
    fn domain_shortcuts_use_the_shared_action_descriptions() {
        for (action, id) in [
            (ShortcutAction::RunQuery, "query.submit"),
            (ShortcutAction::StopQuery, "query.cancel"),
            (ShortcutAction::Estimate, "query.plan"),
        ] {
            let shortcut = shortcut(action);
            let descriptor = shortcut.domain_action().unwrap();
            assert_eq!(descriptor.id, id);
            assert_eq!(shortcut.description, descriptor.description);
        }
    }

    #[test]
    fn the_native_mac_bindings_map_command_to_control() {
        for (action, accelerator) in [
            (ShortcutAction::RunQuery, "Ctrl+Enter"),
            (ShortcutAction::EstimateAndRun, "Ctrl+Shift+Enter"),
            (ShortcutAction::OpenPlan, "Ctrl+Shift+Space"),
            (ShortcutAction::CycleFocus, "Ctrl+K"),
            (ShortcutAction::FocusSearch, "Ctrl+L"),
            (ShortcutAction::ToggleHistory, "Ctrl+B"),
            (ShortcutAction::NewTab, "Ctrl+T"),
            (ShortcutAction::CloseTab, "Ctrl+W"),
            (ShortcutAction::ShowShortcuts, "Ctrl+? / F1"),
        ] {
            assert_eq!(shortcut(action).accelerator(), accelerator);
        }
        assert_eq!(
            shortcut(ShortcutAction::Estimate).accelerator(),
            "Hold Ctrl+Shift"
        );
    }

    #[test]
    fn shifted_letters_and_tab_aliases_match_their_actions() {
        assert_eq!(
            matching(gdk::Key::L, CONTROL),
            Some(ShortcutAction::FocusSearch)
        );
        assert_eq!(
            matching(gdk::Key::ISO_Left_Tab, CONTROL | SHIFT),
            Some(ShortcutAction::PreviousTab)
        );
        assert_eq!(
            matching(gdk::Key::Tab, CONTROL),
            Some(ShortcutAction::NextTab)
        );
        assert_eq!(
            matching(gdk::Key::question, CONTROL | SHIFT),
            Some(ShortcutAction::ShowShortcuts)
        );
    }

    #[test]
    fn navigation_actions_live_in_the_action_menu_without_hidden_shortcuts() {
        for action in [
            ShortcutAction::ShowConnections,
            ShortcutAction::FocusTabs,
            ShortcutAction::FocusEditor,
            ShortcutAction::FocusResults,
            ShortcutAction::FocusHistory,
            ShortcutAction::Disconnect,
        ] {
            assert!(shortcut(action).accelerator().is_empty());
        }
    }
}
