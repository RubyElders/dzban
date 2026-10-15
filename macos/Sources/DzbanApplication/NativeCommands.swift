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
