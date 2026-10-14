# Keyboard action system

Shared interaction roles and frontend ownership are documented in
[Frontend integration](frontend-integration.md). Physical bindings below belong to GTK.

Dzban has one action catalog in `crates/dzban-gtk/src/ui/shortcuts.rs`. Every user-facing command has an identifier, name, description, group, search keywords, and an optional direct shortcut.

The catalog drives:

- the action field opened with `Ctrl+K`;
- the keyboard shortcut reference;
- button tooltips;
- action and shortcut metadata tests.

Do not add a key handler without adding its command to the catalog. Context-specific data such as configured databases and open database or query tabs is added to the action field by a dynamic provider in `window.rs`.

## Interaction model

The action menu is the complete navigation and command surface. It contains application actions plus dynamic entries for configured connections, open database tabs, and query tabs. Search for destinations such as:

- `Focus Query Editor`, `Focus Query Results`, or `Focus Query History`;
- `Focus Database Tabs` or `Focus Query Tabs`;
- `Connection: NAME`, `Database tab: NAME`, or `Query tab: NAME`.

Direct shortcuts are reserved for frequent editing and execution operations. They are optional aliases, not a second command hierarchy. The shortcut reference only renders actions that have a direct binding.

## Focus rules

The Connections screen selects and focuses its first database row when it opens. Arrow keys move through databases and Enter activates the selected row. Opening the Connections action returns focus to that list.

Query editors retain focus after tab selection. Dialog actions, such as renaming a query tab, return through an explicit confirmation button or Enter.

## Adding an action

1. Add a `ShortcutAction` variant and exactly one catalog entry.
2. Implement it in `execute_shortcut_action`.
3. Add manager-level behavior instead of traversing GTK child widgets.
4. Add a catalog or behavior test.
5. Add a Dogtail scenario when the behavior changes screen navigation, focus, dialogs, or keyboard routing.
