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
        CommandMenu("Query") {
            Button("Run Current Query") {
                if NSApp.currentEvent?.type != .keyDown || NSApp.currentEvent?.isARepeat != true {
                    store.sendEditorIntent(.submit)
                }
            }
            .keyboardShortcut(.return, modifiers: .command)
            .disabled(!store.navigationAvailable || store.controls?.primaryAction != "run")
            Button("Cancel Query") { store.sendEditorIntent(.cancel) }
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
