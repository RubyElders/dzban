import AppKit
import DzbanBridge
@testable import DzbanApplication
import XCTest

private struct AccessibilityPress: @unchecked Sendable {
    private let element: NSAccessibilityElement
    init(_ element: NSAccessibilityElement) { self.element = element }
    func perform() -> Bool { element.accessibilityPerformPress() }
}

@MainActor
final class WindowActionsTests: XCTestCase {
    func testSearchNavigationAndSubmissionKeepFocusAndEnabledSelection() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        await store.loadProfiles()
        let actions = WindowActionsModel(store: store)
        actions.move(1)
        XCTAssertTrue(actions.visible)
        XCTAssertEqual(actions.highlighted, "connections")
        actions.move(1)
        XCTAssertEqual(actions.highlighted, "history")
        actions.move(-1)
        XCTAssertEqual(actions.highlighted, "connections")
        actions.search("keyboard shortcuts")
        XCTAssertEqual(actions.items.map(\.id), ["shortcuts"])
        var focusRestorations = 0
        actions.submit { focusRestorations += 1 }
        XCTAssertTrue(store.shortcutsVisible)
        XCTAssertFalse(actions.visible)
        XCTAssertEqual(actions.searchText, "")
        XCTAssertEqual(focusRestorations, 1)
        actions.search("no such action fixture")
        actions.move(1)
        actions.submit { focusRestorations += 1 }
        XCTAssertNil(actions.highlighted)
        XCTAssertTrue(actions.visible)
        XCTAssertEqual(focusRestorations, 1)
        actions.dismiss()
        try await store.shutdown()
    }

    func testDisabledActionsAreNotSubmittedAndToolbarUsesTheSameMenuState() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        await store.loadProfiles()
        let actions = WindowActionsModel(store: store)
        let configuration = actions.configuration(restoreFocus: {})
        configuration.search("Run Query")
        XCTAssertEqual(actions.items.map(\.id), ["run"])
        XCTAssertFalse(try XCTUnwrap(actions.items.first).enabled)
        configuration.submit()
        XCTAssertTrue(actions.visible)
        XCTAssertNil(actions.highlighted)
        configuration.dismiss(false)
        XCTAssertFalse(actions.visible)
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        actions.search("Run Query")
        XCTAssertEqual(actions.highlighted, "run")
        XCTAssertTrue(try XCTUnwrap(actions.items.first).enabled)
        let updated = actions.configuration(restoreFocus: {})
        XCTAssertEqual(updated.searchText, "Run Query")
        XCTAssertTrue(updated.searchVisible)
        try await store.shutdown()
    }

    func testRowNumberAccessibilityPressRunsOnTheMainActorFromEitherThread() async throws {
        let element = RowNumberElement()
        var presses = 0
        element.press = { XCTAssertTrue(Thread.isMainThread); presses += 1 }
        XCTAssertTrue(element.accessibilityPerformPress())
        XCTAssertEqual(presses, 1)
        let press = AccessibilityPress(element)
        let accepted = await Task.detached { press.perform() }.value
        XCTAssertTrue(accepted)
        let deadline = ContinuousClock.now + .seconds(2)
        while presses < 2 {
            guard ContinuousClock.now < deadline else { XCTFail("Background accessibility press did not reach the main actor"); return }
            try await Task.sleep(for: .milliseconds(1))
        }
        XCTAssertEqual(presses, 2)
    }

}
