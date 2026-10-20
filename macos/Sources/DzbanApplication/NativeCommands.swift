import AppKit
import DzbanBridge
import SwiftUI

public struct NativeCommands: Commands {
    @ObservedObject var store: ApplicationStore

    public init(store: ApplicationStore) { self.store = store }

    public var body: some Commands {
        CommandGroup(replacing: .appTermination) {
            Button("Quit Dzban") {
                Task {
                    do { try await store.shutdown(); NSApp.terminate(nil) }
                    catch {}
                }
            }.keyboardShortcut("q", modifiers: .command)
        }
        CommandGroup(replacing: .newItem) {
            Button("New Tab") { Task { await store.newDocument() } }
                .keyboardShortcut("t", modifiers: .command)
                .disabled(!store.navigationAvailable)
        }
        CommandGroup(replacing: .help) {
            Button("Keyboard Shortcuts") { store.shortcutsVisible = true }
                .keyboardShortcut("/", modifiers: [.command, .shift])
        }
        CommandMenu("Query") {
            Button("Search Actions") { store.actionSearchRequest &+= 1 }
            .keyboardShortcut("l", modifiers: .command)
            .disabled(!store.navigationAvailable || store.shortcutsVisible)
            Button("Switch Query / Results Focus") {
                if NSApp.currentEvent?.isARepeat != true { store.toggleQueryResultsFocus() }
            }
            .keyboardShortcut("k", modifiers: .command)
            .disabled(!store.canToggleQueryResultsFocus)
            Button("Close Query Tab") { Task { await store.closeCurrentTab() } }
                .keyboardShortcut("w", modifiers: .command)
                .disabled(!store.navigationAvailable || store.shortcutsVisible)
            Button("Toggle Query History") { store.historyVisible.toggle() }
                .keyboardShortcut("b", modifiers: .command)
            Divider()
            Button(store.controls?.primaryAction == .stop ? "Stop Current Query" : "Run Current Query") {
                if NSApp.currentEvent?.type != .keyDown || NSApp.currentEvent?.isARepeat != true {
                    store.sendEditorIntent(.primary)
                }
            }
            .keyboardShortcut(.return, modifiers: .command)
            .disabled(!store.navigationAvailable || store.connectionsVisible || store.shortcutsVisible || ![PrimaryAction.run, .stop].contains(store.controls?.primaryAction ?? .unavailable))
            Button("Preview Query Plan") { Task { await store.openCurrentPlan() } }
                .keyboardShortcut(.space, modifiers: [.command, .shift])
                .disabled(!store.canPreview)
        }
        #if DZBAN_TEST_FIXTURES
        if ProcessInfo.processInfo.arguments.contains("--metrics-output") {
            CommandMenu("Testing") {
                Button("Reset Viewport Metrics") { store.resetResultMetrics() }
                Button("Record Viewport Metrics") { Task { await store.recordResultMetrics() } }
                ForEach([900, 960, 1030, 1150, 1200], id: \.self) { width in
                    Button("Resize Window to \(String(width))") {
                        guard let window = NSApp.keyWindow else { return }
                        let height = width == 900 ? 560 : width == 960 ? 620 : width == 1030 ? 700 : 760
                        var frame = window.frame
                        frame.size = NSSize(width: width, height: height)
                        window.setFrame(frame, display: true)
                    }
                }
            }
        }
        #endif
        CommandMenu("Connection") {
            Button("Show Connections") { Task { await store.chooseConnections() } }
                .disabled(!store.navigationAvailable)
            Button("Disconnect") {
                if let session = store.selectedSession { Task { await store.closeSession(session) } }
            }
            .disabled(store.selectedSession == nil || !store.navigationAvailable)
        }
    }
}
