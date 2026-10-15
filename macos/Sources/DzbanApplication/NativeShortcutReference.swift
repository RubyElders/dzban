import SwiftUI

struct NativeShortcutReference: View {
    @Environment(\.dismiss) private var dismiss
    private let shortcuts = [
        ("Command-Return / Command-Shift-Return", "Run selected SQL or the current statement"),
        ("Escape / Command-period", "Cancel the running query"),
        ("Command-Shift-P", "Tap to pin the query plan; hold to peek until release"),
        ("Space in results", "Tap to pin the selected row; hold to peek"),
        ("Return in results", "Inspect the selected cell's original value"),
        ("Command-Shift-K", "Search actions, connections, and query tabs"),
        ("Up / Down / Return in action search", "Select and invoke an action"),
        ("Command-K", "Show connections"),
        ("Command-B", "Toggle query history"),
        ("Command-T / Command-W", "Open / close a query tab"),
        ("Command-Shift-[ / Command-Shift-]", "Previous / next query tab"),
        ("Command-Option-Left / Right", "Previous / next connection tab"),
        ("F1 / Command-Shift-/", "Show keyboard shortcuts"),
        ("Command-Q", "Quit after query and connection cleanup")
    ]
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Label("Keyboard Shortcuts", systemImage: "keyboard").font(.headline)
                Spacer()
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction).accessibilityIdentifier("shortcuts.close")
            }
            ScrollView {
                Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 12) {
                    ForEach(shortcuts, id: \.0) { key, action in
                        GridRow { Text(key).font(.system(.body, design: .monospaced)); Text(action) }
                    }
                }
            }
            Text("These are the native macOS controls. The GTK Ctrl+Shift modifier-only estimate chord is not implemented in this frontend; use Command-Shift-P for live plan preview.")
                .font(.caption).foregroundStyle(.secondary)
        }.padding(20).frame(width: 760, height: 530)
    }
}
