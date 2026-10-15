import AppKit
import SwiftUI

public struct NativeCommands: Commands {
    @ObservedObject var store: ApplicationStore

    public init(store: ApplicationStore) { self.store = store }

    public var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Query") { Task { await store.newDocument() } }
                .keyboardShortcut("t", modifiers: .command)
                .disabled(store.selectedSession == nil || !store.navigationAvailable)
        }
        CommandGroup(after: .help) {
            Button("Keyboard Shortcuts") { store.shortcutsVisible = true }
                .keyboardShortcut("/", modifiers: [.command, .shift])
        }
        CommandMenu("Query") {
            Button("Close Query Tab") { if let document = store.document { Task { await store.closeDocument(document.document) } } }
                .keyboardShortcut("w", modifiers: .command)
                .disabled(store.document == nil || !store.navigationAvailable || store.shortcutsVisible)
            Button("Previous Query Tab") { Task { await store.cycleDocument(-1) } }
                .keyboardShortcut("[", modifiers: [.command, .shift])
                .disabled(store.document == nil || !store.navigationAvailable)
            Button("Next Query Tab") { Task { await store.cycleDocument(1) } }
                .keyboardShortcut("]", modifiers: [.command, .shift])
                .disabled(store.document == nil || !store.navigationAvailable)
            Button("Toggle Query History") { store.historyVisible.toggle() }
                .keyboardShortcut("b", modifiers: .command)
            Divider()
            Button("Run Current Query") {
                if NSApp.currentEvent?.type != .keyDown || NSApp.currentEvent?.isARepeat != true {
                    store.sendEditorIntent(.submit)
                }
            }
            .keyboardShortcut(.return, modifiers: .command)
            .disabled(!store.navigationAvailable || store.controls?.primaryAction != "run")
            Button("Preview Query Plan") { Task { await store.previewCurrent(pinned: true) } }
                .keyboardShortcut("p", modifiers: [.command, .shift])
                .disabled(!store.canPreview)
            Button("Cancel Query") { store.sendEditorIntent(.cancel) }
                .keyboardShortcut(".", modifiers: .command)
                .disabled(!store.navigationAvailable || store.controls?.primaryAction != "stop")
        }
        #if DZBAN_TEST_FIXTURES
        if ProcessInfo.processInfo.arguments.contains("--metrics-output") {
            CommandMenu("Testing") {
                Button("Reset Viewport Metrics") { store.resetResultMetrics() }
                Button("Record Viewport Metrics") { Task { await store.recordResultMetrics() } }
            }
        }
        #endif
        CommandMenu("Connection") {
            Button("Previous Connection") { Task { await store.cycleWorkspace(-1) } }
                .keyboardShortcut(.leftArrow, modifiers: [.command, .option])
                .disabled(store.workspaces.isEmpty || !store.navigationAvailable)
            Button("Next Connection") { Task { await store.cycleWorkspace(1) } }
                .keyboardShortcut(.rightArrow, modifiers: [.command, .option])
                .disabled(store.workspaces.isEmpty || !store.navigationAvailable)
            Button("Show Connections") { Task { await store.chooseConnections() } }
                .keyboardShortcut("k", modifiers: .command)
                .disabled(!store.navigationAvailable)
            Button("Disconnect") {
                if let session = store.selectedSession { Task { await store.closeSession(session) } }
            }
            .disabled(store.selectedSession == nil || !store.navigationAvailable)
        }
    }
}
