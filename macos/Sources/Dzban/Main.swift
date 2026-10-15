import AppKit
import DzbanApplication
import DzbanBridge
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {
    let store: ApplicationStore?
    let initializationError: String?
    private var terminationPending = false

    override init() {
        do {
            #if DZBAN_TEST_FIXTURES
            let bridge = try ProcessInfo.processInfo.arguments.contains("--empty-config") ? Bridge() : Bridge.fixture()
            store = ApplicationStore(bridge: bridge, resultSource: fixtureResultSource(bridge))
            #else
            let bridge = try Bridge()
            store = ApplicationStore(bridge: bridge)
            #endif
            initializationError = nil
        } catch {
            store = nil
            initializationError = error.localizedDescription
        }
        super.init()
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let store, !store.isClosed else { return .terminateNow }
        if !terminationPending {
            terminationPending = true
            Task {
                do {
                    try await store.shutdown()
                    sender.reply(toApplicationShouldTerminate: true)
                } catch {
                    terminationPending = false
                    sender.reply(toApplicationShouldTerminate: false)
                }
            }
        }
        return .terminateLater
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

@main
struct DzbanApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        Window("Dzban", id: "main") {
            if let store = delegate.store {
                RootView(store: store)
                    .task {
                        #if DZBAN_TEST_FIXTURES
                        let arguments = ProcessInfo.processInfo.arguments
                        if let index = arguments.firstIndex(of: "--fixture-query"), arguments.indices.contains(index + 1) {
                            await store.loadProfiles()
                            if await store.connect(profile: "fixture"), let session = store.selectedSession, let document = store.document {
                                do {
                                    let _: Submission = try await store.bridge.request(.submit(session: session, document: document.document, sql: arguments[index + 1]), as: Submission.self)
                                } catch { store.failure = error.localizedDescription }
                            }
                        }
                        #endif
                    }
            } else {
                ContentUnavailableView("Cannot start Dzban", systemImage: "exclamationmark.triangle", description: Text(delegate.initializationError ?? "Cannot initialize the database runtime"))
                    .frame(width: 600, height: 400)
            }
        }
        .defaultSize(width: 1150, height: 760)
        .commands {
            if let store = delegate.store { NativeCommands(store: store) }
        }
    }
}
