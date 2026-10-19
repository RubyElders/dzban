import DzbanApplication
import DzbanBridge
import Foundation

#if DZBAN_TEST_FIXTURES
extension LifecycleScenarios {
    @MainActor
    public static func nativeEditorSelectsSQLPreservesDraftsAndScopesCancellation() async throws {
        func phase(_ name: String) {
            if ProcessInfo.processInfo.environment["DZBAN_TRACE_EDITOR_SCENARIO"] == "1" {
                FileHandle.standardError.write(Data("Editor: \(name)\n".utf8))
            }
        }
        phase("create and connect")
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.loadProfiles()
        let connected = await store.connect(profile: "fixture")
        try expect(connected)
        let session = store.selectedSession!
        let original = store.document!
        let editor = store.editor(for: original)
        let draft = "SELECT '😀e\u{301}'; fail; wait"
        editor.edit(text: draft, selection: (draft as NSString).range(of: "fail"))
        store.editorChanged(session: session, document: original.document)
        await store.performEditorIntent(.submit)
        phase("wait for selected failure")
        let failed = try await finished(bridge, session: session, document: original.document, generation: 1)
        try expect(failed.error != nil && failed.error != .null)
        try expect(failed.feedback.queryDiagnostic!.text == "Fixture query failed")

        editor.edit(text: draft, selection: NSRange(location: 2, length: 0))
        await store.performEditorIntent(.submit)
        phase("wait for recovered query")
        let recovered = try await finished(bridge, session: session, document: original.document, generation: 2)
        try expect(recovered.error == nil || recovered.error == .null)
        let history = try await bridge.request(.history(session: session), as: History.self)
        try expect(history.entries.contains { $0.sql == "SELECT '😀e\u{301}'" })
        try expect(!history.entries.contains { $0.sql == draft })

        phase("capture immutable submit")
        editor.edit(text: "SELECT 123", selection: NSRange(location: 0, length: 0))
        store.sendEditorIntent(.submit)
        editor.edit(text: "SELECT 456", selection: NSRange(location: 0, length: 0))
        _ = try await finished(bridge, session: session, document: original.document, generation: 3)
        let captured = try await bridge.request(.history(session: session), as: History.self)
        try expect(captured.entries.contains { $0.sql == "SELECT 123" })
        try expect(!captured.entries.contains { $0.sql == "SELECT 456" })

        phase("coalesce draft burst")
        for index in 0..<100 {
            editor.edit(text: "SELECT \(index)", selection: NSRange(location: 3, length: 0))
            store.editorChanged(session: session, document: original.document)
        }
        _ = try await snapshot(bridge, session: session, document: original.document, waitingFor: "latest coalesced draft") { $0.draft == "SELECT 99" }
        phase("connect second tab")
        _ = await store.connect(profile: "fixture")
        let secondSession = store.selectedSession!
        let second = store.document!
        let waiting = store.editor(for: second)
        waiting.edit(text: "wait", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: secondSession, document: second.document)
        await store.performEditorIntent(.submit)
        phase("wait for second query rows")
        try await receiving(bridge, session: secondSession, document: second.document)
        phase("select original and run")
        await store.selectDocument(original.document)
        try expect(store.editor(for: store.document!) === editor)
        try expect(editor.text == "SELECT 99" && editor.selection.location == 3)
        await store.performEditorIntent(.submit)
        _ = try await finished(bridge, session: session, document: original.document, generation: 4)
        phase("cancel second query")
        await store.selectSession(secondSession)
        await store.performEditorIntent(.cancel)
        let canceled = try await finished(bridge, session: secondSession, document: second.document, generation: 1)
        try expect(canceled.error != nil && canceled.error != .null)

        phase("restart second query")
        await store.selectDocument(second.document)
        await store.performEditorIntent(.submit)
        try await receiving(bridge, session: secondSession, document: second.document)
        await store.chooseConnections()
        phase("connect slow third tab")
        let otherConnected = await store.connect(profile: "fixture-slow")
        try expect(otherConnected)
        let otherSession = store.selectedSession!
        let otherDocument = store.document!
        let otherEditor = store.editor(for: otherDocument)
        otherEditor.edit(text: "wait", selection: NSRange(location: 0, length: 0))
        await store.performEditorIntent(.submit)
        try await receiving(bridge, session: otherSession, document: otherDocument.document)
        phase("reset and stop third query")
        await store.performEditorIntent(.reset)
        await store.refreshControls()
        try expect(store.controls?.primaryAction == .stop)
        await store.performEditorIntent(.primary)
        _ = try await finished(bridge, session: otherSession, document: otherDocument.document, generation: 1)
        phase("return and stop second query")
        await store.selectSession(secondSession)
        await store.refreshControls()
        try expect(store.controls?.primaryAction == .stop)
        await store.performEditorIntent(.cancel)
        _ = try await finished(bridge, session: secondSession, document: second.document, generation: 2)
        try expect(store.failure == nil, store.failure ?? "Unexpected editor failure")
        phase("shutdown")
        try await store.shutdown()
    }

    private static func snapshot(_ bridge: Bridge, session: UUID, document: UUID, waitingFor state: String, matching condition: @escaping @Sendable (DocumentSnapshot) -> Bool) async throws -> DocumentSnapshot {
        let stream = try await bridge.observeDocument(session: session, document: document)
        return try await withThrowingTaskGroup(of: DocumentSnapshot.self) { group in
            group.addTask {
                for try await snapshot in stream { if condition(snapshot) { return snapshot } }
                throw CheckFailure(message: "Document observation ended while waiting for \(state)")
            }
            group.addTask {
                try await Task.sleep(for: .seconds(5))
                let workspace = try await ApplicationClient(bridge: bridge).workspace()
                let latest = workspace.tabs.first { $0.session == session && $0.document == document }?.snapshot
                throw CheckFailure(message: "Timed out waiting for \(state): execution=\(latest?.execution.rawValue ?? "missing") generation=\(latest?.generation.description ?? "missing") rows=\(latest?.rows.description ?? "missing") draft=\(latest?.draft ?? "missing")")
            }
            defer { group.cancelAll() }
            guard let snapshot = try await group.next() else { throw CheckFailure(message: "No document observation") }
            return snapshot
        }
    }

    private static func finished(_ bridge: Bridge, session: UUID, document: UUID, generation: UInt64) async throws -> DocumentSnapshot {
        try await snapshot(bridge, session: session, document: document, waitingFor: "finished generation \(generation)") {
            $0.generation == generation && $0.execution == .finished
        }
    }

    private static func receiving(_ bridge: Bridge, session: UUID, document: UUID) async throws {
        _ = try await snapshot(bridge, session: session, document: document, waitingFor: "streaming rows") {
            $0.rows >= 5 && $0.execution != .finished
        }
    }
}
#endif
