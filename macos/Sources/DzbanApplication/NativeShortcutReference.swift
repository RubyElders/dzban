import SwiftUI

struct NativeShortcutReference: View {
    @Environment(\.dismiss) private var dismiss

    private struct Shortcut: Identifiable {
        let keys: String
        let action: String
        let context: String
        let spokenKeys: String
        var id: String { action }
    }

    private let shortcuts = [
        Shortcut(keys: "⌘↩", action: "Run or stop query", context: "Query", spokenKeys: "Command Return"),
        Shortcut(keys: "⌘⇧↩", action: "Estimate and run query", context: "Query", spokenKeys: "Command Shift Return"),
        Shortcut(keys: "⌘⇧␣", action: "Open query plan", context: "Query", spokenKeys: "Command Shift Space"),
        Shortcut(keys: "⌘⇧", action: "Hold for live estimate", context: "Query", spokenKeys: "Hold Command Shift"),
        Shortcut(keys: "␣", action: "Tap to pin row; hold to peek", context: "Results", spokenKeys: "Space"),
        Shortcut(keys: "␣ / ↩", action: "Open selected cell value", context: "Row preview", spokenKeys: "Space or Return"),
        Shortcut(keys: "← / →", action: "Previous / next result row", context: "Row preview", spokenKeys: "Left or Right"),
        Shortcut(keys: "⎋", action: "Go back, dismiss, or stop query", context: "Current view", spokenKeys: "Escape"),
        Shortcut(keys: "⌘K", action: "Switch query / results focus", context: "Window", spokenKeys: "Command K"),
        Shortcut(keys: "⌘L", action: "Search actions and tabs", context: "Window", spokenKeys: "Command L"),
        Shortcut(keys: "H J K L", action: "Move left / down / up / right", context: "Outside text", spokenKeys: "H, J, K, L"),
        Shortcut(keys: "↑ ↓ / ↩", action: "Select / open item", context: "Lists", spokenKeys: "Up, Down, Return"),
        Shortcut(keys: "␣", action: "Probe selected connection", context: "Connections", spokenKeys: "Space"),
        Shortcut(keys: "⌘B", action: "Toggle history sidebar", context: "Window", spokenKeys: "Command B"),
        Shortcut(keys: "⌘T / ⌘W", action: "New tab / close tab", context: "Window", spokenKeys: "Command T, Command W"),
        Shortcut(keys: "⌃⇥ / ⌃⇧⇥", action: "Next / previous tab", context: "Window", spokenKeys: "Control Tab, Control Shift Tab"),
        Shortcut(keys: "⌘A / C / V / X", action: "Select all / copy / paste / cut", context: "Text", spokenKeys: "Command A, C, V, X"),
        Shortcut(keys: "⌘Z / ⌘⇧Z", action: "Undo / redo", context: "Text", spokenKeys: "Command Z, Command Shift Z"),
        Shortcut(keys: "⌘?", action: "Show keyboard shortcuts", context: "Window", spokenKeys: "Command Question Mark"),
        Shortcut(keys: "⌘Q", action: "Quit after query cleanup", context: "App", spokenKeys: "Command Q")
    ]

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Keyboard Shortcuts").font(.headline)
                Spacer()
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction).accessibilityIdentifier("shortcuts.close")
            }
            .padding(16)
            HStack(spacing: 16) {
                Text("Where").frame(width: 110, alignment: .leading)
                Text("Action").frame(maxWidth: .infinity, alignment: .leading)
                Text("Shortcut").frame(width: 170, alignment: .trailing)
            }
            .font(.system(size: 14, weight: .medium))
            .padding(.horizontal, 16)
            .frame(height: 28)
            .background(Color(nsColor: .controlBackgroundColor))
            Divider()
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(shortcuts.enumerated()), id: \.element.id) { index, shortcut in
                        HStack(spacing: 16) {
                            Text(shortcut.context).foregroundStyle(.secondary)
                                .frame(width: 110, alignment: .leading)
                            Text(shortcut.action).frame(maxWidth: .infinity, alignment: .leading)
                            Text(shortcut.keys).font(.system(size: 14, weight: .medium, design: .monospaced))
                                .frame(width: 170, alignment: .trailing)
                                .accessibilityLabel(shortcut.spokenKeys)
                                .accessibilityIdentifier("shortcut.\(shortcut.spokenKeys)")
                        }
                        .font(.system(size: 14))
                        .padding(.horizontal, 16)
                        .frame(height: 26)
                        .background(Color(nsColor: NSColor.alternatingContentBackgroundColors[index % 2]))
                    }
                }
            }
            .background(Color(nsColor: .textBackgroundColor))
            .accessibilityIdentifier("shortcuts.table")
        }
        .frame(width: 720, height: 578)
    }
}
