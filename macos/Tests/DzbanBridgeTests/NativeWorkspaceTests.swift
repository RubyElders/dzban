import DzbanBridge
@testable import DzbanApplication
import XCTest

@MainActor
final class NativeWorkspaceTests: XCTestCase {
    func testSharedRestoreConnectionCanBeCanceledAndRetriedFromTheNativeChooser() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("workspace.json")
        try Data(#"{"version":1,"tabs":[{"profile":"fixture-slow","draft":"keep draft"}],"selected":0}"#.utf8).write(to: url)
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge, restorationURL: url)
        let starting = Task { await store.start() }
        let deadline = ContinuousClock.now + .seconds(5)
        while store.pages.first?.phase != .connecting {
            if ContinuousClock.now >= deadline { throw ViewportFailure("Restoration was not admitted") }
            try await Task.sleep(for: .milliseconds(5))
        }
        store.cancelConnectionPicker()
        await starting.value
        while store.pages.first?.phase != .choosing {
            if ContinuousClock.now >= deadline { throw ViewportFailure("Restoration cancellation did not finish") }
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertNil(store.failure)
        XCTAssertNil(store.selectedSession)
        XCTAssertEqual(store.pages.first?.snapshot.draft, "keep draft")
        let retried = await store.connect(profile: "fixture")
        XCTAssertTrue(retried)
        XCTAssertEqual(store.document?.draft, "keep draft")
        try await store.shutdown()
    }

    func testFailedSharedWorkspaceSaveKeepsTheNativeWorkspaceOpenForRetry() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("workspace.json")
        let store = ApplicationStore(bridge: try Bridge.fixture(), restorationURL: url)
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let document = try XCTUnwrap(store.document)
        store.editor(for: document).edit(text: "unsent 😀", selection: NSRange(location: 0, length: 0))
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: false)
        do { try await store.shutdown(); XCTFail("A directory cannot be replaced by a saved workspace") }
        catch let error as BridgeFailure { XCTAssertEqual(error.kind, "workspace") }
        XCTAssertFalse(store.isClosed)
        XCTAssertTrue(store.navigationAvailable)
        XCTAssertEqual(store.editor(for: document).text, "unsent 😀")
        XCTAssertEqual(store.document?.document, document.document)
        try FileManager.default.removeItem(at: url)
        async let first: Void = store.shutdown()
        async let second: Void = store.shutdown()
        _ = try await (first, second)
        XCTAssertTrue(store.isClosed)
        let saved = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        let tabs = try XCTUnwrap(saved["tabs"] as? [[String: Any]])
        XCTAssertEqual(tabs.first?["draft"] as? String, "unsent 😀")
    }

    private func pending(_ bridge: Bridge) async throws {
        let deadline = ContinuousClock.now + .seconds(5)
        while true {
            let pending = try await bridge.request(.connections, as: Connections.self)
            if pending.connections.contains(where: { $0.connecting }) { return }
            if ContinuousClock.now >= deadline { throw ViewportFailure("Connection was not admitted") }
            try await Task.sleep(for: .milliseconds(5))
        }
    }

    private func assertSharedInventory(_ store: ApplicationStore) async throws {
        let shared = try await store.bridge.request(.tabs, as: TabInventory.self)
        XCTAssertEqual(store.pages.map(\.id), shared.tabs.map(\.id))
        XCTAssertEqual(store.pages.map(\.document), shared.tabs.map(\.document))
        XCTAssertEqual(store.selectedPage, shared.selected)
    }

    func testNativeTabsKeepSharedTabAndDocumentIDsAcrossConnectionAndClose() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        await store.start()
        try await assertSharedInventory(store)
        let first = try XCTUnwrap(store.pages.first)
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        XCTAssertEqual(store.document?.document, first.document)
        XCTAssertEqual(store.selectedPage, first.id)
        await store.newDocument()
        let second = try XCTUnwrap(store.selectedPage)
        XCTAssertNotEqual(second, first.id)
        try await assertSharedInventory(store)
        await store.cycleWorkspace(-1)
        XCTAssertEqual(store.selectedPage, first.id)
        await store.closePage(second)
        XCTAssertEqual(store.selectedPage, first.id)
        try await assertSharedInventory(store)
        try await store.shutdown()
    }

    func testCancelingLastPendingConnectionKeepsSharedDraftForRetry() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.start()
        let first = try XCTUnwrap(store.pages.first)
        let _: TabInventory = try await bridge.request(.setTabDraft(first.id, draft: "SELECT 'žluť 😀';"), as: TabInventory.self)
        await store.selectPage(first.id)
        let connecting = Task { await store.connect(profile: "fixture-slow") }
        try await pending(bridge)
        store.cancelConnectionPicker()
        let connected = await connecting.value
        XCTAssertFalse(connected)
        XCTAssertNil(store.failure)
        XCTAssertNil(store.selectedSession)
        XCTAssertEqual(store.pages.count, 1)
        XCTAssertEqual(store.pages[0].id, first.id)
        XCTAssertEqual(store.pages[0].document, first.document)
        XCTAssertEqual(store.pages[0].snapshot.draft, "SELECT 'žluť 😀';")
        XCTAssertEqual(store.pages[0].phase, .choosing)
        try await assertSharedInventory(store)
        let retried = await store.connect(profile: "fixture")
        XCTAssertTrue(retried)
        XCTAssertEqual(store.document?.document, first.document)
        XCTAssertEqual(store.document?.draft, "SELECT 'žluť 😀';")
        try await store.shutdown()
    }

    func testBackgroundConnectionCompletesOnItsOriginWithoutChangingSelection() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.start()
        let first = try XCTUnwrap(store.selectedPage)
        let connecting = Task { await store.connect(profile: "fixture-slow") }
        try await pending(bridge)
        await store.newDocument()
        let second = try XCTUnwrap(store.selectedPage)
        let connected = await connecting.value
        XCTAssertTrue(connected)
        XCTAssertEqual(store.selectedPage, second)
        XCTAssertNil(store.selectedSession)
        XCTAssertEqual(store.workspaces.count, 1)
        XCTAssertEqual(store.workspaces[0].tab, first)
        await store.newDocument()
        let third = try XCTUnwrap(store.selectedPage)
        let secondConnected = await store.connect(profile: "fixture", tab: second)
        XCTAssertTrue(secondConnected)
        XCTAssertEqual(store.selectedPage, third)
        XCTAssertEqual(store.pages.count, 3)
        XCTAssertEqual(store.workspaces.count, 2)
        XCTAssertTrue(store.workspaces.contains(where: { $0.tab == second }))
        try await assertSharedInventory(store)
        try await store.shutdown()
    }

    func testClosingPendingTabDrainsItsConnectionWithoutPresentingAnError() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let first = try XCTUnwrap(store.selectedPage)
        await store.newDocument()
        let second = try XCTUnwrap(store.selectedPage)
        let connecting = Task { await store.connect(profile: "fixture-slow") }
        try await pending(bridge)
        await store.closePage(second)
        let completed = await connecting.value
        XCTAssertFalse(completed)
        XCTAssertNil(store.failure)
        XCTAssertEqual(store.selectedPage, first)
        XCTAssertEqual(store.pages.count, 1)
        try await assertSharedInventory(store)
        try await store.shutdown()
    }
}
