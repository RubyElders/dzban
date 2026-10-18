import AppKit
import DzbanBridge
@testable import DzbanApplication
import XCTest
import SwiftUI

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
    func testRepeatedRunWhileCommandHeldDoesNotNeedReturnKeyUp() async throws {
        let (_, store, _) = try await connected()
        let document = try XCTUnwrap(store.document)
        let editor = store.editor(for: document)
        editor.edit(text: "few", selection: NSRange(location: 3, length: 0))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 600, height: 300), styleMask: [.titled], backing: .buffered, defer: false)
        window.contentView?.addSubview(editor.textView)
        window.makeFirstResponder(editor.textView)
        let configuration = NativeToolbarConfiguration(history: AnyView(EmptyView()), plan: AnyView(EmptyView()), primary: AnyView(EmptyView()), placeholder: "", searchText: "", searchVisible: false, menuHeight: 0, focusRequest: 0, search: { _ in }, move: { _ in }, submit: {}, dismiss: { _ in })
        let coordinator = WindowPolicy.Coordinator(store: store, toolbar: configuration)
        coordinator.attach(window)
        defer { coordinator.detach() }
        func event(repeating: Bool = false) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil, characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: repeating, keyCode: 36))
        }
        for _ in 0..<2 {
            let generation = try XCTUnwrap(store.document).generation
            XCTAssertNil(coordinator.handle(try event()))
            try await until { (store.document?.generation ?? 0) > generation && store.document?.execution == "finished" }
        }
        let generation = try XCTUnwrap(store.document).generation
        XCTAssertNil(coordinator.handle(try event(repeating: true)))
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertEqual(store.document?.generation, generation)
        try await store.shutdown()
    }

    func testQuitRestoresOrderedTabsDraftsAndSelectionWithoutRunningQueries() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("workspace.json")
        let store = ApplicationStore(bridge: try Bridge.fixture(), restorationURL: url)
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let first = try XCTUnwrap(store.document)
        store.editor(for: first).edit(text: "SELECT 'unsaved 😀';", selection: NSRange(location: 0, length: 0))
        await store.newDocument()
        let secondConnected = await store.connect(profile: "fixture")
        XCTAssertTrue(secondConnected)
        let second = try XCTUnwrap(store.document)
        store.editor(for: second).edit(text: "wait", selection: NSRange(location: 0, length: 0))
        await store.performEditorIntent(.primary)
        try await until { store.document?.execution == "running" }
        await store.newDocument()
        await store.selectPage(store.pages[1].id)
        try await store.shutdown()

        let restored = ApplicationStore(bridge: try Bridge.fixture(), restorationURL: url)
        await restored.start()
        XCTAssertEqual(restored.pages.count, 3)
        XCTAssertEqual(restored.workspaces.count, 2)
        XCTAssertEqual(restored.selectedPage, restored.pages[1].id)
        XCTAssertEqual(restored.workspaces.map(\.profile), ["fixture", "fixture"])
        XCTAssertEqual(restored.workspaces.flatMap(\.documents).map(\.draft), ["SELECT 'unsaved 😀';", "wait"])
        XCTAssertFalse(restored.workspaces.flatMap(\.documents).contains { ["running", "planning"].contains($0.execution) })
        XCTAssertNil(restored.pages[2].session)
        await restored.closePage(restored.pages[0].id)
        try await restored.shutdown()

        let reopened = ApplicationStore(bridge: try Bridge.fixture(), restorationURL: url)
        await reopened.start()
        XCTAssertEqual(reopened.pages.count, 2)
        XCTAssertEqual(reopened.document?.draft, "wait")
        try await reopened.shutdown()
    }

    func testUnavailableRestoredConnectionKeepsItsDraftForReconnect() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("workspace.json")
        try Data(#"{"version":1,"tabs":[{"profile":"missing","draft":"SELECT 42;"}],"selected":0}"#.utf8).write(to: url)
        let store = ApplicationStore(bridge: try Bridge.fixture(), restorationURL: url)
        await store.start()
        XCTAssertEqual(store.pages.count, 1)
        XCTAssertNil(store.selectedSession)
        XCTAssertEqual(store.pages[0].savedDraft, "SELECT 42;")
        let reconnected = await store.connect(profile: "fixture")
        XCTAssertTrue(reconnected)
        XCTAssertEqual(store.document?.draft, "SELECT 42;")
        try await store.shutdown()
    }

    func testClosingTheLastConnectionPageShutsDown() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        await store.closeCurrentTab()
        XCTAssertTrue(store.isClosed)
        XCTAssertTrue(store.pages.isEmpty)
    }

    func testConnectionPickerPagesBecomeSessionsWithoutAddingAnotherTab() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.loadProfiles()
        let firstPage = store.selectedPage
        XCTAssertEqual(store.pages.count, 1)
        XCTAssertNil(store.selectedSession)
        let firstConnected = await store.connect(profile: "fixture")
        XCTAssertTrue(firstConnected)
        let firstSession = try XCTUnwrap(store.selectedSession)
        XCTAssertEqual(store.selectedPage, firstPage)
        XCTAssertEqual(store.pages.count, 1)
        await store.newDocument()
        let secondPage = store.selectedPage
        XCTAssertNil(store.selectedSession)
        XCTAssertTrue(store.connectionsVisible)
        XCTAssertEqual(store.pages.count, 2)
        XCTAssertEqual(store.workspaces.count, 1)
        await store.cycleWorkspace(-1)
        XCTAssertEqual(store.selectedSession, firstSession)
        await store.cycleWorkspace(1)
        XCTAssertEqual(store.selectedPage, secondPage)
        XCTAssertNil(store.selectedSession)
        let secondConnected = await store.connect(profile: "fixture")
        XCTAssertTrue(secondConnected)
        XCTAssertEqual(store.selectedPage, secondPage)
        XCTAssertEqual(store.pages.count, 2)
        XCTAssertNotEqual(store.selectedSession, firstSession)
        await store.newDocument()
        await store.closeCurrentTab()
        XCTAssertEqual(store.pages.count, 2)
        XCTAssertNotNil(store.selectedSession)
        try await store.shutdown()
        XCTAssertTrue(store.pages.isEmpty)
    }

    func testAwayCompletionMarksSelectedTabAndCancellationDoesNotNotify() async throws {
        let (_, store, session) = try await connected()
        var updates: [QueryAttentionUpdate] = []
        store.attentionChanged = { updates.append($0) }
        store.updateApplicationVisibility(false)
        let document = try XCTUnwrap(store.document)
        store.editor(for: document).edit(text: "fail", selection: NSRange(location: 0, length: 0))
        await store.performEditorIntent(.submit)
        try await until { store.document?.execution == "finished" && store.unseenCompletionCount == 1 }
        XCTAssertTrue(try XCTUnwrap(store.workspace).unseenResultFailed)
        XCTAssertEqual(try XCTUnwrap(store.workspace).completionRevision, 1)
        XCTAssertEqual(updates.filter(\.requestAttention).count, 1)
        XCTAssertEqual(store.selectedSession, session)
        await store.selectSession(session)
        XCTAssertEqual(store.unseenCompletionCount, 1)
        store.updateApplicationVisibility(true)
        XCTAssertEqual(store.unseenCompletionCount, 0)
        XCTAssertTrue(try XCTUnwrap(updates.last).cancelAttention)
        XCTAssertFalse(try XCTUnwrap(store.workspace).hasUnseenResult)
        store.editor(for: try XCTUnwrap(store.document)).edit(text: "wait", selection: NSRange(location: 0, length: 0))
        await store.performEditorIntent(.submit)
        try await until { store.document?.execution == "running" }
        store.updateApplicationVisibility(false)
        await store.performEditorIntent(.cancel)
        try await until { store.document?.execution == "finished" }
        XCTAssertEqual(store.unseenCompletionCount, 0)
        XCTAssertEqual(updates.filter(\.requestAttention).count, 1)
        try await store.shutdown()
        XCTAssertEqual(try XCTUnwrap(updates.last).unreadCount, 0)
    }

    func testBackgroundCompletionMarksOnlyItsTabUntilSelected() async throws {
        let (bridge, store, first) = try await connected()
        let document = try XCTUnwrap(store.document)
        let _: Submission = try await bridge.request(.submit(session: first, document: document.document, sql: "delayed_result"), as: Submission.self)
        try await until { store.document?.execution == "running" }
        let opened = await store.connect(profile: "fixture")
        XCTAssertTrue(opened)
        let second = try XCTUnwrap(store.selectedSession)
        try await until { store.workspaces.first(where: { $0.id == first })?.hasUnseenResult == true }
        XCTAssertFalse(try XCTUnwrap(store.workspaces.first(where: { $0.id == second })).hasUnseenResult)
        await store.selectSession(first)
        XCTAssertFalse(try XCTUnwrap(store.workspace).hasUnseenResult)
        await store.selectSession(second)
        XCTAssertFalse(try XCTUnwrap(store.workspaces.first(where: { $0.id == first })).hasUnseenResult)
        try await store.shutdown()
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
        _ = await store.connect(profile: "fixture")
        await delayed.value
        XCTAssertFalse(store.plan.visible)
        XCTAssertNil(store.plan.plan)
        try await Task.sleep(for: .milliseconds(550))
        let current = try XCTUnwrap(store.document)
        let buffer = store.editor(for: current)
        buffer.edit(text: "SELECT 'preview_error'", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: store.selectedSession!, document: current.document)
        await store.previewCurrent()
        XCTAssertEqual(store.plan.failure, "Fixture plan rejected")
        buffer.edit(text: "SELECT 42", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: store.selectedSession!, document: current.document)
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
    func testRowInspectionAppendsColumnsAndUsesSharedTapHoldPolicy() async throws {
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
        XCTAssertEqual(row.values[0].text, "499")
        for count in stride(from: 32, through: 128, by: 16) {
            row.loadMore(columns: 128)
            try await until { row.values.count == count && !row.loading }
        }
        XCTAssertEqual(row.values.last?.text, "r499c127")
        XCTAssertNil(row.moveRow(1, rows: 500, columns: 128, column: 127))
        XCTAssertEqual(row.moveRow(-1, rows: 500, columns: 128, column: 127)?.row, 498)
        _ = row.moveRow(-1, rows: 500, columns: 128, column: 127)
        _ = row.moveRow(1, rows: 500, columns: 128, column: 127)
        try await until { row.values.count == 16 && !row.loading }
        XCTAssertEqual(row.values[0].text, "498")
        XCTAssertEqual(row.address?.column, 127)
        XCTAssertEqual(row.moveRow(1, rows: 500, columns: 128, column: 127)?.row, 499)
        try await until { row.values.count == 16 && !row.loading }
        XCTAssertEqual(row.values[0].text, "499")
        row.inspectedCell = address
        row.backOrClose()
        XCTAssertNil(row.inspectedCell)
        XCTAssertNotNil(row.address)
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
        let coordinator = WindowPolicy.Coordinator(store: store, toolbar: NativeToolbarConfiguration(history: AnyView(EmptyView()), plan: AnyView(EmptyView()), primary: AnyView(EmptyView()), placeholder: "", searchText: "", searchVisible: false, menuHeight: 360, focusRequest: 0, search: { _ in }, move: { _ in }, submit: {}, dismiss: { _ in }))
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

    func testProbesReportLatencyAndVersionWithoutOpeningAWorkspace() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.loadProfiles()
        await store.probeProfile("fixture")
        XCTAssertEqual(store.profileHealth["fixture"]?.state, .connected)
        XCTAssertEqual(store.profileHealth["fixture"]?.version, "fixture")
        XCTAssertNotNil(store.profileHealth["fixture"]?.latencyMicros)
        XCTAssertTrue(store.workspaces.isEmpty)
        XCTAssertNil(store.selectedSession)
        await store.probeProfile("fixture-failure")
        XCTAssertEqual(store.profileHealth["fixture-failure"]?.state, .error)
        XCTAssertEqual(store.profileHealth["fixture-failure"]?.failure, "Fixture connection rejected")
        XCTAssertTrue(store.workspaces.isEmpty)
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let session = try XCTUnwrap(store.selectedSession)
        try await until { store.health[session]?.latencyMicros != nil }
        XCTAssertEqual(store.health[session]?.state, .connected)
        XCTAssertEqual(ConnectionHealth.latency(999), "999µs")
        XCTAssertEqual(ConnectionHealth.latency(1250), "1ms")
        XCTAssertEqual(ConnectionHealth.latency(1_250_000), "1.25s")
        try await store.shutdown()
    }

    func testFailedShutdownRetainsOnlyTheTabNeedingCleanup() async throws {
        let (bridge, store, firstSession) = try await connected()
        let first = try XCTUnwrap(store.document)
        let _: Submission = try await bridge.request(.submit(session: firstSession, document: first.document, sql: "reject_close"), as: Submission.self)
        try await until { store.document?.execution == "finished" }
        let connectedSecond = await store.connect(profile: "fixture")
        XCTAssertTrue(connectedSecond)
        do {
            try await store.shutdown()
            XCTFail("Cleanup rejection was lost")
        } catch let error as BridgeFailure { XCTAssertEqual(error.kind, "shutdown") }
        XCTAssertEqual(store.workspaces.map(\.id), [firstSession])
        XCTAssertEqual(store.selectedSession, firstSession)
        XCTAssertTrue(store.navigationAvailable)
        try await store.shutdown()
        XCTAssertTrue(store.isClosed)
        XCTAssertTrue(store.workspaces.isEmpty)
    }

    func testOpenPlanPinsLiveEstimateAndSurvivesModifierRelease() async throws {
        let (_, store, session) = try await connected()
        let document = try XCTUnwrap(store.document)
        store.editor(for: document).edit(text: "SELECT 42", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: session, document: document.document)
        store.sendSharedKey(.primary, pressed: true, at: 1)
        store.sendSharedKey(.preview, pressed: true, at: 2)
        try await until { store.sharedInput?.hint.kind == "estimate_ready" }
        await store.openCurrentPlan()
        store.sendSharedKey(.preview, pressed: false, at: 420)
        store.sendSharedKey(.primary, pressed: false, at: 421)
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertTrue(store.plan.visible)
        XCTAssertTrue(store.plan.pinned)
        XCTAssertNotNil(store.plan.plan)
        XCTAssertEqual(store.document?.generation, 0)
        try await store.shutdown()
    }

    func testSharedChordEstimatesWithoutExecutingThenPinsAndRunsOnce() async throws {
        let (_, store, session) = try await connected()
        let document = try XCTUnwrap(store.document)
        store.editor(for: document).edit(text: "SELECT 42", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: session, document: document.document)
        store.sendSharedKey(.primary, pressed: true, at: 1)
        store.sendSharedKey(.preview, pressed: true, at: 2)
        try await until { store.sharedInput?.hint.kind == "estimate_ready" }
        XCTAssertEqual(store.document?.generation, 0)
        XCTAssertFalse(store.plan.visible)
        store.sendSharedKey(.inspect, pressed: true, at: 400)
        try await until { store.plan.visible }
        store.sendSharedKey(.inspect, pressed: false, at: 410)
        try await until { store.plan.pinned }
        store.sendSharedKey(.preview, pressed: false, at: 420)
        store.sendSharedKey(.primary, pressed: false, at: 421)
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertTrue(store.plan.pinned)
        store.plan.close()
        store.sendSharedKey(.primary, pressed: true, at: 500)
        store.sendSharedKey(.submit, pressed: true, at: 501)
        store.sendSharedKey(.submit, pressed: true, at: 502)
        store.sendSharedKey(.submit, pressed: false, at: 503)
        store.sendSharedKey(.primary, pressed: false, at: 504)
        try await until { store.document?.execution == "finished" }
        XCTAssertEqual(store.document?.generation, 1)
        XCTAssertNotNil(store.document?.executionSeconds)
        try await store.shutdown()
    }

    func testSubmitDuringDelayedEstimateWaitsAndNavigationDropsLatePlan() async throws {
        let (_, store, session) = try await connected()
        let document = try XCTUnwrap(store.document)
        store.editor(for: document).edit(text: "SELECT 'preview_delayed'", selection: NSRange(location: 0, length: 0))
        store.editorChanged(session: session, document: document.document)
        store.sendSharedKey(.primary, pressed: true, at: 1)
        store.sendSharedKey(.preview, pressed: true, at: 2)
        try await until { store.plan.loading }
        store.sendSharedKey(.submit, pressed: true, at: 3)
        store.sendSharedKey(.submit, pressed: false, at: 4)
        try await until { store.document?.execution == "finished" }
        XCTAssertEqual(store.document?.generation, 1)
        XCTAssertNil(store.failure)
        store.clearSharedInput()
        store.sendSharedKey(.primary, pressed: true, at: 600)
        store.sendSharedKey(.preview, pressed: true, at: 601)
        try await until { store.plan.loading }
        _ = await store.connect(profile: "fixture")
        try await Task.sleep(for: .milliseconds(650))
        XCTAssertFalse(store.plan.visible)
        XCTAssertNil(store.plan.plan)
        XCTAssertEqual(store.document?.generation, 0)
        try await store.shutdown()
    }

}
