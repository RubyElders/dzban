import XCTest

@MainActor
final class ShellTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = false
        await MainActor.run { app = XCUIApplication() }
    }

    override func tearDown() async throws {
        await MainActor.run { if app.state != .notRunning { app.terminate() } }
    }

    func testConnectionQueryMenuAndQuit() throws {
        app.launch()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        XCTAssertTrue(app.buttons["Query 1"].waitForExistence(timeout: 5))
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(app.buttons["Query 2"].waitForExistence(timeout: 5))
        app.buttons["Close Query 2"].click()
        XCTAssertTrue(app.buttons["Query 1"].exists)
        XCTAssertFalse(app.buttons["Query 2"].exists)
        app.typeKey("k", modifierFlags: .command)
        XCTAssertTrue(connect.waitForExistence(timeout: 5))
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testFailedQuitKeepsWindowAvailableForRetry() throws {
        app.launchArguments = ["--fixture-query", "reject_close"]
        app.launch()
        let phase = app.staticTexts["query.execution"]
        XCTAssertTrue(phase.waitForExistence(timeout: 10))
        let finished = expectation(for: NSPredicate(format: "value == %@", "Finished"), evaluatedWith: phase)
        wait(for: [finished], timeout: 10)
        app.typeKey("q", modifierFlags: .command)
        let acknowledgment = app.windows.buttons["OK"]
        XCTAssertTrue(acknowledgment.waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Fixture cleanup rejected once"].exists)
        acknowledgment.click()
        XCTAssertTrue(app.windows.firstMatch.exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testFinalQueryCloseReturnsToConnections() throws {
        app.launch()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        let close = app.buttons["Close Query 1"]
        XCTAssertTrue(close.waitForExistence(timeout: 5))
        close.click()
        XCTAssertTrue(connect.waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Query 1"].exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testWindowCloseWaitsForCleanup() throws {
        app.launch()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        XCTAssertTrue(app.buttons["Query 1"].waitForExistence(timeout: 5))
        app.typeKey("w", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testWorkspaceSelectionRestoresQueryTabs() throws {
        app.launch()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        XCTAssertTrue(app.buttons["Query 1"].waitForExistence(timeout: 5))
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(app.buttons["Query 2"].waitForExistence(timeout: 5))
        app.typeKey("k", modifierFlags: .command)
        let second = app.buttons["connection.fixture-slow"]
        XCTAssertTrue(second.waitForExistence(timeout: 5))
        second.click()
        XCTAssertTrue(app.buttons["Query 1"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Query 2"].exists)
        let original = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@ AND label CONTAINS %@ AND NOT label CONTAINS %@", "workspace.", "fixture", "slow")).firstMatch
        XCTAssertTrue(original.waitForExistence(timeout: 5))
        original.click()
        XCTAssertTrue(app.buttons["Query 2"].waitForExistence(timeout: 5))
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testEmptyConfigurationLaunch() throws {
        app.launchArguments = ["--empty-config"]
        app.launchEnvironment["DZBAN_CONFIG"] = "/dev/null"
        app.launch()
        XCTAssertTrue(app.staticTexts["connections.empty"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["connection.fixture"].exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }
}
