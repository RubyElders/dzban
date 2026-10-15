import AppKit
import DzbanBridge
@testable import DzbanApplication
import XCTest

@MainActor
final class NavigationTests: XCTestCase {
    private func connected() async throws -> (Bridge, ApplicationStore, UUID) {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.loadProfiles()
        let result = await store.connect(profile: "fixture")
        XCTAssertTrue(result)
        return (bridge, store, try XCTUnwrap(store.selectedSession))
    }
    private func until(_ condition: () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(5)
        while !condition() {
            if ContinuousClock.now >= deadline { throw NSError(domain: "NavigationTests", code: 1) }
            try await Task.sleep(for: .milliseconds(5))
        }
    }
    func testHistoryRestoresSavedOutputWithoutSubmittingAndRefreshRetiresSelection() async throws {
        let (bridge, store, session) = try await connected()
        let original = try XCTUnwrap(store.document)
        let editor = store.editor(for: original)
        editor.edit(text: "SELECT 'saved 😀'", selection: NSRange(location: 0, length: 0))
        await store.performEditorIntent(.submit)
        try await until { store.document?.execution == "finished" && store.document?.rows == 500 }
        let model = HistoryBrowserModel(bridge: bridge)
        model.refresh(session: session)
        try await until { !model.loading }
        let list = try XCTUnwrap(model.list)
        let entry = try XCTUnwrap(list.entries.first)
        XCTAssertEqual(entry.sqlPreview, "SELECT 'saved 😀'")
        XCTAssertEqual(entry.rows, 500)
        let generation = try XCTUnwrap(store.document).generation
        editor.textView.string = "SELECT 'unsent draft'"
        editor.edit(text: editor.textView.string, selection: NSRange(location: 0, length: 0))
        let restored = await store.restoreHistory(session: session, revision: list.revision, index: entry.index)
        XCTAssertTrue(restored)
        let saved = try XCTUnwrap(store.document)
        XCTAssertEqual(saved.generation, generation + 1)
        XCTAssertEqual(saved.history, true)
        XCTAssertEqual(saved.rows, 200)
        XCTAssertEqual(saved.historyTotalRows, 500)
        XCTAssertEqual(editor.textView.string, "SELECT 'saved 😀'")
        XCTAssertEqual(saved.execution, "finished")
        model.refresh(session: session)
        try await until { !model.loading }
        XCTAssertNotEqual(model.list?.revision, list.revision)
        model.retire()
        XCTAssertNil(model.list)
        XCTAssertFalse(model.loading)
        await store.performEditorIntent(.submit)
        try await until { store.document?.execution == "finished" && store.document?.rows == 500 }
        XCTAssertNil(store.document?.history)
        try await store.shutdown()
    }

    func testCatalogListsDescribesFiltersAndRetires() async throws {
        let (bridge, store, session) = try await connected()
        let model = CatalogBrowserModel(bridge: bridge)
        await model.open(session: session)
        XCTAssertTrue(model.actions.contains { $0.id == "catalog.list.table" })
        XCTAssertFalse(model.actions.contains { $0.id == "catalog.list.schema" })
        model.read(.list(kind: "table"))
        try await until { !model.loading }
        let entry = try XCTUnwrap(model.entries.first)
        XCTAssertEqual(entry.identity.name, "žluť table")
        XCTAssertEqual(entry.attributes["comment"], .null)
        model.read(.describe(entry.identity))
        try await until { !model.loading }
        XCTAssertEqual(model.entries.first?.attributes["column_name"], .string("id"))
        model.read(.list(kind: "table", glob: "missing*"))
        try await until { !model.loading }
        XCTAssertTrue(model.entries.isEmpty)
        model.read(.list(kind: "schema"))
        try await until { !model.loading }
        XCTAssertTrue(model.failure?.contains("Unsupported") == true)
        model.read(.list(kind: "table"))
        try await until { !model.loading }
        XCTAssertNil(model.failure)
        model.read(.list(kind: "table"))
        model.retire()
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertNil(model.session)
        XCTAssertTrue(model.entries.isEmpty)
        try await store.shutdown()
    }
    func testPreviewSelectsCurrentStatementAndDropsDelayedOutputAfterNavigation() async throws {
        let (_, store, session) = try await connected()
        let document = try XCTUnwrap(store.document)
        let editor = store.editor(for: document)
        let sql = "SELECT 1; SELECT 'preview_delayed'"
        editor.edit(text: sql, selection: NSRange(location: 1, length: 0))
        store.editorChanged(session: session, document: document.document)
        await store.previewCurrent()
        guard case .object(let plan) = store.plan.plan else { return XCTFail("Missing plan") }
        XCTAssertEqual(plan["document"], .object(["fixture": .bool(true), "sql": .string("SELECT 1")]))
        store.plan.dismiss()
        editor.edit(text: sql, selection: NSRange(location: sql.utf16.count, length: 0))
        let delayed = Task { await store.previewCurrent() }
        try await until { store.plan.loading }
        await store.newDocument()
        await delayed.value
        XCTAssertFalse(store.plan.visible)
        XCTAssertNil(store.plan.plan)
        try await Task.sleep(for: .milliseconds(550))
        let current = try XCTUnwrap(store.document)
        let buffer = store.editor(for: current)
        buffer.edit(text: "SELECT 'preview_error'", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: session, document: current.document)
        await store.previewCurrent()
        XCTAssertEqual(store.plan.failure, "Fixture plan rejected")
        buffer.edit(text: "SELECT 42", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: session, document: current.document)
        await store.previewCurrent()
        XCTAssertNotNil(store.plan.plan)
        XCTAssertNil(store.plan.failure)
        store.plan.release(milliseconds: 10)
        try await until { store.plan.pinned }
        store.plan.reset()
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertTrue(store.plan.visible)
        store.plan.dismiss()
        await store.previewCurrent()
        store.plan.release(milliseconds: 500)
        try await until { !store.plan.visible }
        try await store.shutdown()
    }
    func testRowInspectionPagesColumnsAndUsesSharedTapHoldPolicy() async throws {
        let (bridge, store, session) = try await connected()
        let document = try XCTUnwrap(store.document)
        let editor = store.editor(for: document)
        editor.edit(text: "wide", selection: NSRange(location: 0, length: 0))
        await store.performEditorIntent(.submit)
        try await until { store.document?.execution == "finished" && store.document?.rows == 500 }
        let snapshot = try XCTUnwrap(store.document)
        let address = ResultAddress(scope: ResultScope(session: session, document: snapshot.document, generation: snapshot.generation), row: 499, column: 127)
        let row = RowInspectorModel(bridge: bridge, source: BridgeResultSource(bridge: bridge))
        row.show(address, pinned: true, columns: 128)
        try await until { row.values.count == 16 && !row.loading }
        XCTAssertEqual(row.columnStart, 112)
        XCTAssertEqual(row.values.last?.text, "r499c127")
        row.load(start: 0, columns: 128)
        try await until { row.values.count == 16 && !row.loading }
        XCTAssertEqual(row.values[0].text, "499")
        row.release(milliseconds: 500)
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertNotNil(row.address)
        row.close()
        row.show(address, pinned: false, columns: 128)
        row.release(milliseconds: 10)
        try await until { row.pinned }
        XCTAssertNotNil(row.address)
        row.close()
        row.show(address, pinned: false, columns: 128)
        try await until { row.address != nil && row.values.count == 16 }
        row.release(milliseconds: 500)
        try await until { row.address == nil && !row.loading }
        row.close()
        try await store.shutdown()
    }

    func testClosingWorkspaceRetiresPinnedPreviewAndControls() async throws {
        let (_, store, session) = try await connected()
        let document = try XCTUnwrap(store.document)
        let editor = store.editor(for: document)
        editor.edit(text: "SELECT 42", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: session, document: document.document)
        await store.previewCurrent(pinned: true)
        XCTAssertTrue(store.plan.visible)
        XCTAssertTrue(store.plan.pinned)
        await store.closeSession(session)
        XCTAssertNil(store.selectedSession)
        XCTAssertFalse(store.plan.visible)
        XCTAssertTrue(store.backendActions.isEmpty)
        try await store.shutdown()
    }
    func testApplicationDeactivationHidesTransientPlanAndPreservesPinnedPlan() async throws {
        let (_, store, session) = try await connected()
        let document = try XCTUnwrap(store.document)
        let editor = store.editor(for: document)
        editor.edit(text: "SELECT 42", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: session, document: document.document)
        let coordinator = WindowPolicy.Coordinator(store: store)
        let window = NSWindow()
        coordinator.attach(window)
        defer { coordinator.detach() }
        await store.previewCurrent()
        XCTAssertTrue(store.plan.visible)
        NotificationCenter.default.post(name: NSApplication.didResignActiveNotification, object: NSApplication.shared)
        try await until { !store.plan.visible }
        await store.previewCurrent(pinned: true)
        NotificationCenter.default.post(name: NSApplication.didResignActiveNotification, object: NSApplication.shared)
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertTrue(store.plan.visible)
        XCTAssertTrue(store.plan.pinned)
        await store.closeSession(session)
        try await store.shutdown()
    }

}
