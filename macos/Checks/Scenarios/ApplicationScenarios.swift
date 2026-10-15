import DzbanApplication
import DzbanBridge
import Foundation

#if DZBAN_TEST_FIXTURES
extension LifecycleScenarios {
    @MainActor
    public static func nativeStoreNavigatesClosesAndRejectsStaleConnections() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.loadProfiles()
        try expect(store.profiles.contains { $0.name == "fixture" })
        let rejected = await store.connect(profile: "fixture-failure")
        try expect(!rejected)
        try expect(store.failure == "Fixture connection rejected")
        store.failure = nil
        let connected = await store.connect(profile: "fixture")
        try expect(connected)
        let session = store.selectedSession!
        let original = store.document!.document
        await store.newDocument()
        let second = store.document!.document
        try expect(second != original)
        await store.selectDocument(original)
        try expect(store.document?.document == original)
        await store.closeDocument(second)
        try expect(store.workspace?.documents.count == 1)
        try expect(store.selectedSession == session)
        await store.chooseConnections()
        try expect(store.selectedSession == nil)
        try expect(store.workspaces.count == 1)
        await store.selectSession(session)
        try expect(store.controls?.primaryAction == "run")
        await store.closeDocument(original)
        try expect(store.workspaces.isEmpty)
        try expect(store.selectedSession == nil)

        let connecting = Task { await store.connect(profile: "fixture-slow") }
        let deadline = ContinuousClock.now + .seconds(5)
        while true {
            let pending = try await bridge.request(.connections, as: Connections.self)
            if pending.connections.contains(where: { $0.connecting && $0.profile == "fixture-slow" }) { break }
            try expect(ContinuousClock.now < deadline, "Connection was never admitted")
            try await Task.sleep(for: .milliseconds(5))
        }
        await store.chooseConnections()
        let stale = await connecting.value
        try expect(!stale)
        try expect(store.selectedSession == nil)
        try expect(store.workspaces.isEmpty)
        try expect(store.failure == nil, store.failure ?? "Unexpected native store error")
        try await store.shutdown()
        try expect(store.isClosed)
    }

    @MainActor
    public static func nativeStoreKeepsBusyAndFailedCleanupAvailableForRetry() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.loadProfiles()
        _ = await store.connect(profile: "fixture")
        let session = store.selectedSession!
        let document = store.document!.document
        let _: Submission = try await bridge.request(.submit(session: session, document: document, sql: "wait"), as: Submission.self)
        let active = try await bridge.observeDocument(session: session, document: document)
        for try await snapshot in active { if snapshot.rows >= 5 { break } }
        await store.closeDocument(document)
        try expect(store.workspace?.documents.count == 1)
        try expect(store.failure != nil)
        store.failure = nil
        await store.chooseConnections()
        try expect(store.controls == nil)
        await store.selectSession(session)
        await store.refreshControls()
        try expect(store.controls?.primaryAction == "stop")
        do { try await store.shutdown(); throw CheckFailure(message: "Busy store shut down") }
        catch let error as BridgeFailure { try expect(error.kind == "shutdown") }
        try expect(!store.isClosed && !store.isShuttingDown)
        let _: Ack = try await bridge.request(.cancel(session: session, document: document), as: Ack.self)
        let completed = try await bridge.observeDocument(session: session, document: document)
        for try await snapshot in completed { if snapshot.execution == "finished" { break } }
        let _: Submission = try await bridge.request(.submit(session: session, document: document, sql: "reject_close"), as: Submission.self)
        let rejecting = try await bridge.observeDocument(session: session, document: document)
        for try await snapshot in rejecting { if snapshot.execution == "finished" { break } }
        do { try await store.shutdown(); throw CheckFailure(message: "Rejected cleanup was lost") }
        catch let error as BridgeFailure { try expect(error.kind == "shutdown") }
        try expect(store.workspaces.count == 1)
        try expect(!store.isClosed && store.navigationAvailable)
        async let first: Void = store.shutdown()
        async let second: Void = store.shutdown()
        _ = try await (first, second)
        try expect(store.isClosed && store.workspaces.isEmpty)
    }
}
#endif
