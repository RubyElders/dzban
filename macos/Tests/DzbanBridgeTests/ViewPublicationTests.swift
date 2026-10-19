import AppKit
import DzbanBridge
@testable import DzbanApplication
import SwiftUI
import XCTest

@MainActor
final class ViewPublicationTests: XCTestCase {
    private func until(details: @autoclosure () -> String = "", file: StaticString = #filePath, line: UInt = #line, _ condition: () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(8)
        while !condition() {
            if ContinuousClock.now >= deadline { throw ViewportFailure("Hosted workspace did not reach its expected state at \(file):\(line) \(details())") }
            try await Task.sleep(for: .milliseconds(5))
        }
    }

    private func resultTable(in view: NSView?) -> NativeResultTable? {
        guard let view else { return nil }
        if let table = view as? NativeResultTable { return table }
        return view.subviews.lazy.compactMap { self.resultTable(in: $0) }.first
    }

    func testNativePopoverDismissalPublishesAfterViewUpdate() async throws {
        let bridge = try Bridge.fixture()
        let model = PlanPreviewModel()
        model.present(visible: true, pinned: true)
        ViewUpdateDiagnostics.reset()
        ViewUpdateDiagnostics.enabled = true
        defer { ViewUpdateDiagnostics.reset() }
        ViewUpdateDiagnostics.watch(model, name: "plan")
        ViewUpdateDiagnostics.frames.append("popover dismissal")
        let dismissal = model.presentationDismissed()
        ViewUpdateDiagnostics.frames.removeLast()
        XCTAssertTrue(model.visible)
        XCTAssertTrue(ViewUpdateDiagnostics.publications.isEmpty)
        await dismissal?.value
        XCTAssertFalse(model.visible)
        try await bridge.shutdown()
    }

    func testRetiredPopoverDismissalCannotCloseNewPresentation() async throws {
        let bridge = try Bridge.fixture()
        let model = PlanPreviewModel()
        model.present(visible: true)
        let dismissal = model.presentationDismissed()
        model.present(visible: false)
        model.present(visible: true, pinned: true)
        await dismissal?.value
        XCTAssertTrue(model.visible)
        XCTAssertTrue(model.pinned)
        model.dismiss()
        try await bridge.shutdown()
    }

    func testHistoryDismissalPublishesAfterViewUpdate() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let session = try XCTUnwrap(store.selectedSession)
        let model = HistoryBrowserModel(application: store.application)
        model.refresh(session: session)
        try await until { model.list != nil && !model.loading }
        ViewUpdateDiagnostics.reset()
        ViewUpdateDiagnostics.enabled = true
        defer { ViewUpdateDiagnostics.reset() }
        ViewUpdateDiagnostics.watch(model, name: "history")
        ViewUpdateDiagnostics.frames.append("history dismissal")
        let dismissal = model.presentationEnded()
        ViewUpdateDiagnostics.frames.removeLast()
        XCTAssertNotNil(model.list)
        XCTAssertTrue(ViewUpdateDiagnostics.publications.isEmpty)
        await dismissal.value
        XCTAssertNil(model.list)
        XCTAssertFalse(model.loading)
        try await store.shutdown()
    }

    func testCellDismissalPreservesNewPresentation() async throws {
        let bridge = try Bridge.fixture()
        let model = CellInspectorModel(source: BridgeResultSource(application: ApplicationClient(bridge: bridge)))
        let scope = ResultScope(session: UUID(), document: UUID(), generation: 1)
        let first = ResultAddress(scope: scope, row: 0, column: 0)
        let second = ResultAddress(scope: scope, row: 1, column: 0)
        model.openEntire(first)
        ViewUpdateDiagnostics.reset()
        ViewUpdateDiagnostics.enabled = true
        defer { ViewUpdateDiagnostics.reset() }
        ViewUpdateDiagnostics.watch(model, name: "cell")
        ViewUpdateDiagnostics.frames.append("cell dismissal")
        let dismissal = model.presentationEnded()
        ViewUpdateDiagnostics.frames.removeLast()
        XCTAssertEqual(model.address, first)
        XCTAssertTrue(ViewUpdateDiagnostics.publications.isEmpty)
        model.openEntire(second)
        await dismissal.value
        XCTAssertEqual(model.address, second)
        model.cancel()
        try await bridge.shutdown()
    }

    func testRerunAndInspectorDoNotPublishDuringNativeViewUpdates() async throws {
        ViewUpdateDiagnostics.reset()
        ViewUpdateDiagnostics.enabled = true
        defer { ViewUpdateDiagnostics.reset() }
        let store = ApplicationStore(bridge: try Bridge.fixture())
        ViewUpdateDiagnostics.watch(store, name: "workspace")
        ViewUpdateDiagnostics.watch(store.results, name: "results")
        ViewUpdateDiagnostics.watch(store.plan, name: "plan")
        ViewUpdateDiagnostics.watch(store.rowInspector, name: "row inspector")
        ViewUpdateDiagnostics.watch(store.cellInspector, name: "cell inspector")
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1150, height: 760), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.contentView = NSHostingView(rootView: RootView(store: store))
        window.makeKeyAndOrderFront(nil)
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let document = try XCTUnwrap(store.document)
        let editor = store.editor(for: document)
        ViewUpdateDiagnostics.watch(editor, name: "editor")
        editor.restore(text: "wide")
        await store.openCurrentPlan()
        try await until { store.plan.plan != nil && !store.plan.loading }
        await Task.yield()
        await store.performEditorIntent(.submit)
        try await until { store.document?.execution == .finished && store.results.columns.count == 128 }
        try await until(details: "columns=\(self.resultTable(in: window.contentView)?.numberOfColumns.description ?? "missing") visible=\(window.isVisible)") { self.resultTable(in: window.contentView)?.numberOfColumns == 128 }
        let table = try XCTUnwrap(resultTable(in: window.contentView))
        table.focusedColumn = 127
        table.selectRowIndexes(IndexSet(integer: 499), byExtendingSelection: false)
        table.scrollRowToVisible(499)
        table.scrollColumnToVisible(127)
        let generation = try XCTUnwrap(store.document).generation
        await store.performEditorIntent(.submit)
        try await until { (store.document?.generation ?? 0) > generation && store.document?.execution == .finished && store.results.selected?.row == 499 }
        try await until {
            table.selectedRow == 499 && table.focusedColumn == 127
                && store.results.value(row: 499, column: 127) != nil
        }
        let address = try XCTUnwrap(store.results.selected)
        store.rowInspector.show(address, pinned: true, columns: 128)
        try await until { store.rowInspector.values.count > 0 }
        store.rowInspector.inspectedCell = address
        try await until { store.cellInspector.loaded }
        store.rowInspector.backOrClose()
        store.rowInspector.close()
        editor.restore(text: "single")
        let previousGeneration = try XCTUnwrap(store.document).generation
        await store.performEditorIntent(.submit)
        try await until {
            (store.document?.generation ?? 0) > previousGeneration
                && store.document?.execution == .finished && store.results.columns.count == 1
        }
        try await until { table.numberOfColumns == 1 && table.numberOfRows == 3 }
        await Task.yield()
        window.contentView = NSView()
        window.orderOut(nil)
        try await store.shutdown()
        XCTAssertTrue(ViewUpdateDiagnostics.publications.isEmpty, ViewUpdateDiagnostics.publications.joined(separator: "\n\n"))
    }
}
