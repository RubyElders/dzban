# Keyboard action system

Shared interaction roles and frontend ownership are documented in [Frontend integration](frontend-integration.md). Physical bindings below belong to GTK. They follow the native macOS bindings with Control in place of Command.

Dzban has one action catalog in `crates/dzban-gtk/src/ui/shortcuts.rs`. Every user-facing command has a name, description, group, search keywords, and optional direct shortcuts.

The catalog drives:

- the action field opened with `Ctrl+L`;
- the keyboard shortcut reference (`Ctrl+?` or `F1`);
- button tooltips;
- action and shortcut metadata tests.

Do not add a key handler without adding its command to the catalog. Context-specific entries, one per configured connection and one per open tab, are added to the action field by the provider in `window.rs`.

## Bindings

| Binding | Action |
| --- | --- |
| `Ctrl+Enter` | Run or stop the query under the cursor or the selection |
| `Ctrl+Shift+Enter` | Estimate, then run |
| Hold `Ctrl+Shift` | Live estimate in the action field |
| `Ctrl+Shift+Space` | Plan preview: tap to pin, hold to peek |
| `Escape` | Close the row preview, leave history, stop the query, or close the plan, in that order |
| `Space` in results | Row preview: tap to pin, hold to peek; arrows move between rows and columns |
| `Enter` or double-click in results | Cell detail with a full-value copy |
| `Ctrl+C` in results | Copy the selected rows as Markdown |
| `H` `J` `K` `L` outside text fields | Left, Down, Up and Right in the chooser, history, results and row preview |
| `Ctrl+K` | Move focus between the editor and the results |
| `Ctrl+L` | Search actions |
| `Ctrl+B` | Toggle query history |
| `Ctrl+T` / `Ctrl+W` | New tab with a connection chooser / close tab |
| `Ctrl+Tab`, `Ctrl+Page Down` / `Ctrl+Shift+Tab`, `Ctrl+Page Up` | Next / previous tab |
| `F11` | Toggle fullscreen |
| `Ctrl+Q` | Save open tabs and quit |

Modifier and submit keys are sent to the selected tab's shared input owner as `InputCommand` events, awaited in order. The window only decides which physical keys it consumes, from its own modifier tracking and the shared controls projection.

## Focus rules

A chooser tab focuses its selected connection row. Up and Down move through connections, Enter connects, Space tests the connection, and Escape cancels a pending connection or closes an extra chooser tab. Connecting moves focus to the editor.

Closing the last tab opens a fresh chooser. Closing the last tab while it is still an unconnected chooser quits after saving the workspace.

## Adding an action

1. Add a `ShortcutAction` variant and exactly one catalog entry.
2. Implement it in `Shell::execute`.
3. Call tab or workspace methods instead of traversing GTK child widgets.
4. Add a catalog or behavior test.
5. Add a Dogtail scenario when the behavior changes screen navigation, focus, dialogs, or keyboard routing.
