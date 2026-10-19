import DzbanBridge
import XCTest

final class WorkspaceContractTests: XCTestCase {
    func testChooserAndConnectedTabKeepTheirSharedIdentityThroughRetryAndClose() async throws {
        let bridge = try Bridge.fixture()
        let created = try await bridge.request(.openTab(profile: "missing", draft: "SELECT 'žluť 😀';"), as: CreatedTab.self)
        let chooser = created.workspace
        let first = try XCTUnwrap(chooser.tabs.first)
        XCTAssertEqual(created.tab, first.id)
        XCTAssertEqual(first.phase, .choosing)
        XCTAssertNil(first.session)
        XCTAssertEqual(first.snapshot.draft, "SELECT 'žluť 😀';")
        do {
            let _: Connection = try await bridge.request(.connect(profile: "fixture-failure", tab: first.id), as: Connection.self)
            XCTFail("Fixture connection must fail")
        } catch let error as BridgeFailure { XCTAssertEqual(error.kind, "connection") }
        let failed = try await bridge.request(.tabs, as: TabInventory.self)
        XCTAssertEqual(failed.tabs.count, 1)
        XCTAssertEqual(failed.tabs[0].id, first.id)
        XCTAssertEqual(failed.tabs[0].document, first.document)
        XCTAssertEqual(failed.tabs[0].phase, .error)
        let connected = try await bridge.request(.connect(profile: "fixture", tab: first.id), as: Connection.self)
        XCTAssertEqual(connected.tab, first.id)
        let ready = try await bridge.request(.tabs, as: TabInventory.self)
        XCTAssertEqual(ready.tabs[0].snapshot.draft, "SELECT 'žluť 😀';")
        XCTAssertEqual(ready.tabs[0].document, first.document)
        XCTAssertEqual(ready.tabs[0].phase, .ready)
        let opened = try await bridge.request(.openTab(), as: CreatedTab.self)
        let added = opened.workspace
        XCTAssertEqual(added.tabs.count, 2)
        XCTAssertGreaterThan(added.revision, ready.revision)
        let second = opened.tab
        XCTAssertEqual(added.selected, second)
        let selected = try await bridge.request(.selectTab(first.id, revision: 20), as: TabInventory.self)
        XCTAssertEqual(selected.selected, first.id)
        do {
            let _: TabInventory = try await bridge.request(.selectTab(second, revision: 19), as: TabInventory.self)
            XCTFail("Older navigation must be rejected")
        } catch let error as BridgeFailure { XCTAssertEqual(error.kind, "selection_superseded") }
        let closed = try await bridge.request(.closeTab(first.id), as: TabInventory.self)
        XCTAssertEqual(closed.tabs.count, 1)
        XCTAssertEqual(closed.selected, second)
        let empty = try await bridge.request(.closeTab(second), as: TabInventory.self)
        XCTAssertTrue(empty.tabs.isEmpty)
        XCTAssertNil(empty.selected)
        try await bridge.shutdown()
    }
}
