import DzbanApplication
import DzbanBridge
import Foundation

#if DZBAN_TEST_FIXTURES
extension LifecycleScenarios {
    @MainActor
    public static func nativeEditorSelectsSQLPreservesDraftsAndScopesCancellation() async throws {
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
        let failed = try await finished(bridge, session: session, document: original.document, generation: 1)
        try expect(failed.error != nil && failed.error != .null)
        try expect(QueryDiagnostic.describe(failed.error!) == "Fixture query failed")

        editor.edit(text: draft, selection: NSRange(location: 2, length: 0))
        await store.performEditorIntent(.submit)
        let recovered = try await finished(bridge, session: session, document: original.document, generation: 2)
        try expect(recovered.error == nil || recovered.error == .null)
        let history = try await bridge.request(.history(session: session), as: History.self)
        try expect(history.entries.contains { $0.sql == "SELECT '😀e\u{301}'" })
        try expect(!history.entries.contains { $0.sql == draft })

        editor.edit(text: "SELECT 123", selection: NSRange(location: 0, length: 0))
        store.sendEditorIntent(.submit)
        editor.edit(text: "SELECT 456", selection: NSRange(location: 0, length: 0))
        _ = try await finished(bridge, session: session, document: original.document, generation: 3)
        let captured = try await bridge.request(.history(session: session), as: History.self)
        try expect(captured.entries.contains { $0.sql == "SELECT 123" })
        try expect(!captured.entries.contains { $0.sql == "SELECT 456" })

        for index in 0..<100 {
            editor.edit(text: "SELECT \(index)", selection: NSRange(location: 3, length: 0))
            store.editorChanged(session: session, document: original.document)
        }
        let drafts = try await bridge.observeDocument(session: session, document: original.document)
        for try await snapshot in drafts { if snapshot.draft == "SELECT 99" { break } }
        await store.newDocument()
        let second = store.document!
        let waiting = store.editor(for: second)
        waiting.edit(text: "wait", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: session, document: second.document)
        await store.performEditorIntent(.submit)
        try await receiving(bridge, session: session, document: second.document)
        await store.selectDocument(original.document)
        try expect(store.editor(for: store.document!) === editor)
        try expect(editor.text == "SELECT 99" && editor.selection.location == 3)
        await store.performEditorIntent(.submit)
        let unchanged = try await bridge.observeDocument(session: session, document: original.document)
        for try await snapshot in unchanged { try expect(snapshot.generation == 3); break }
        await store.performEditorIntent(.cancel)
        let canceled = try await finished(bridge, session: session, document: second.document, generation: 1)
        try expect(canceled.error != nil && canceled.error != .null)

        await store.selectDocument(second.document)
        await store.performEditorIntent(.submit)
        try await receiving(bridge, session: session, document: second.document)
        await store.chooseConnections()
        let otherConnected = await store.connect(profile: "fixture-slow")
        try expect(otherConnected)
        let otherSession = store.selectedSession!
        let otherDocument = store.document!
        let otherEditor = store.editor(for: otherDocument)
        otherEditor.edit(text: "wait", selection: NSRange(location: 0, length: 0))
        await store.performEditorIntent(.submit)
        try await receiving(bridge, session: otherSession, document: otherDocument.document)
        await store.performEditorIntent(.reset)
        await store.refreshControls()
        try expect(store.controls?.primaryAction == "stop")
        await store.performEditorIntent(.primary)
        _ = try await finished(bridge, session: otherSession, document: otherDocument.document, generation: 1)
        await store.selectSession(session)
        await store.refreshControls()
        try expect(store.controls?.primaryAction == "stop")
        await store.performEditorIntent(.cancel)
        _ = try await finished(bridge, session: session, document: second.document, generation: 2)
        try expect(store.failure == nil, store.failure ?? "Unexpected editor failure")
        try await store.shutdown()
    }

    private static func finished(_ bridge: Bridge, session: UUID, document: UUID, generation: UInt64) async throws -> DocumentSnapshot {
        let stream = try await bridge.observeDocument(session: session, document: document)
        for try await snapshot in stream {
            if snapshot.generation == generation && snapshot.execution == "finished" { return snapshot }
        }
        throw CheckFailure(message: "Document did not finish")
    }

    private static func receiving(_ bridge: Bridge, session: UUID, document: UUID) async throws {
        let stream = try await bridge.observeDocument(session: session, document: document)
        for try await snapshot in stream { if snapshot.rows >= 5 && snapshot.execution != "finished" { return } }
        throw CheckFailure(message: "Document did not stream")
    }
}
#endif
