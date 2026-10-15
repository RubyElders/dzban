import AppKit
import XCTest

@MainActor
final class ShellTests: XCTestCase {
    private var app: XCUIApplication!
    private var fixtureDirectory: URL!

    override func setUp() async throws {
        continueAfterFailure = false
        try await MainActor.run {
            app = XCUIApplication()
            fixtureDirectory = FileManager.default.temporaryDirectory.appendingPathComponent("dzban-ui-\(UUID())", isDirectory: true)
            let temporary = fixtureDirectory.appendingPathComponent("tmp", isDirectory: true)
            try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: true)
            app.launchEnvironment["TMPDIR"] = temporary.path + "/"
            app.launchEnvironment["DZBAN_CONFIG"] = "/dev/null"
            app.launchEnvironment["XDG_CONFIG_HOME"] = fixtureDirectory.appendingPathComponent("config").path
            app.launchEnvironment["XDG_CONFIG_DIRS"] = fixtureDirectory.appendingPathComponent("system-config").path
            app.launchEnvironment["XDG_DATA_HOME"] = fixtureDirectory.appendingPathComponent("data").path
        }
    }

    override func tearDown() async throws {
        await MainActor.run {
            if app.state != .notRunning { app.terminate() }
            if let fixtureDirectory {
                let layout = fixtureDirectory.appendingPathComponent("viewport.json.layout")
                if FileManager.default.fileExists(atPath: layout.path) {
                    let attachment = XCTAttachment(contentsOfFile: layout)
                    attachment.name = "Native viewport layout"
                    attachment.lifetime = .keepAlways
                    add(attachment)
                }
                if app.state == .notRunning { try? FileManager.default.removeItem(at: fixtureDirectory) }
                NSPasteboard(name: NSPasteboard.Name(fixtureDirectory.lastPathComponent)).releaseGlobally()
            }
        }
    }

    func testNativeConnectionsStatusAndShortcuts() throws {
        app.launchArguments = ["--fixture-query", "SELECT 42"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let tabs = app.descendants(matching: .any)["connection.tabs"]
        let status = app.descendants(matching: .any)["workspace.statusbar"]
        XCTAssertTrue(tabs.exists)
        XCTAssertTrue(status.exists)
        let editor = app.textViews["query.editor"]
        XCTAssertLessThan(tabs.frame.maxY, editor.frame.minY)
        XCTAssertGreaterThan(status.frame.minY, editor.frame.maxY)
        app.buttons["shortcuts.open"].click()
        XCTAssertTrue(app.sheets.buttons["shortcuts.close"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.sheets.staticTexts["Command-Shift-P"].exists)
        app.sheets.buttons["shortcuts.close"].click()
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(app.buttons["Query 2"].waitForExistence(timeout: 5))
        app.typeKey("[", modifierFlags: [.command, .shift])
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        app.typeKey("]", modifierFlags: [.command, .shift])
        waitForValue("Run a query to see results", in: app.staticTexts["results.status"])
        app.typeKey("w", modifierFlags: .command)
        let closed = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: app.buttons["Query 2"])
        wait(for: [closed], timeout: 10)
        XCTAssertTrue(app.buttons["Query 1"].exists)
        app.typeKey("b", modifierFlags: .command)
        XCTAssertFalse(app.buttons["history.close"].exists)
        app.typeKey("b", modifierFlags: .command)
        XCTAssertTrue(app.buttons["history.close"].waitForExistence(timeout: 5))
        app.typeKey("k", modifierFlags: .command)
        app.buttons["connection.fixture-slow"].click()
        XCTAssertTrue(app.buttons["Query 1"].waitForExistence(timeout: 5))
        app.typeKey(.leftArrow, modifierFlags: [.command, .option])
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
        attachment.name = "Native connections and status bar"
        attachment.lifetime = .keepAlways
        add(attachment)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testCenteredActionsAndLeftHistory() throws {
        app.launchArguments = ["--fixture-query", "SELECT 42"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let restore = app.buttons["history.restore.0"]
        XCTAssertTrue(restore.waitForExistence(timeout: 10))
        let editor = app.textViews["query.editor"]
        XCTAssertLessThan(restore.frame.maxX, editor.frame.minX)
        XCTAssertTrue(app.windows.firstMatch.buttons["_XCUI:CloseWindow"].exists)
        XCTAssertTrue(app.toolbars.buttons["history.open"].exists)
        XCTAssertTrue(app.toolbars.buttons["query.primary"].exists)
        let actions = app.toolbars.searchFields["actions.search"]
        let run = app.buttons["query.primary"]
        XCTAssertLessThan(app.buttons["history.open"].frame.maxX, actions.frame.minX)
        XCTAssertLessThan(actions.frame.maxX, run.frame.minX)
        app.typeKey("k", modifierFlags: [.command, .shift])
        let search = app.toolbars.searchFields["actions.search"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["action.new"].waitForExistence(timeout: 5))
        search.click()
        search.typeText("query")
        XCTAssertTrue(app.buttons["action.new"].exists)
        XCTAssertFalse(app.buttons["action.disconnect"].exists)
        search.typeKey(.downArrow, modifierFlags: [])
        search.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.buttons["Query 2"].waitForExistence(timeout: 5))
        restore.click()
        waitForValue("Saved - 200 of 500 rows", in: app.staticTexts["results.status"])
        XCTAssertTrue(app.textViews["query.editor"].isHittable)
        app.buttons["history.close"].click()
        XCTAssertFalse(app.buttons["history.close"].exists)
        app.buttons["history.open"].click()
        XCTAssertTrue(app.buttons["history.restore.0"].waitForExistence(timeout: 5))
        let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
        attachment.name = "Native centered actions and left history"
        attachment.lifetime = .keepAlways
        add(attachment)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testConnectionQueryMenuAndQuit() throws {
        launchApp()
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
        launchApp()
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
        launchApp()
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
        launchApp()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        XCTAssertTrue(app.buttons["Query 1"].waitForExistence(timeout: 5))
        app.typeKey("w", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testWorkspaceSelectionRestoresQueryTabs() throws {
        launchApp()
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

    func testNativeEditingExecutionCancellationAndRestart() throws {
        launchApp()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        let editor = app.textViews["query.editor"]
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        editor.click()
        editor.typeText("wait")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Run 1", in: app.staticTexts["query.generation"])
        let primary = app.buttons["query.primary"]
        waitForLabel("Stop", in: primary)
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(app.buttons["Query 2"].waitForExistence(timeout: 5))
        editor.click()
        editor.typeText("SELECT 2")
        app.typeKey(.return, modifierFlags: .command)
        XCTAssertFalse(app.staticTexts["query.generation"].exists)
        app.typeKey(.escape, modifierFlags: [])
        waitForLabel("Run", in: primary)
        app.buttons["Query 1"].click()
        waitForValue("wait", in: editor)
        editor.click()
        app.typeKey(.return, modifierFlags: [.command, .shift])
        waitForValue("Run 2", in: app.staticTexts["query.generation"])
        waitForLabel("Stop", in: primary)
        app.typeKey(".", modifierFlags: .command)
        waitForLabel("Run", in: primary)
        editor.click()
        app.typeKey("a", modifierFlags: .command)
        editor.typeText("wait; fail")
        app.typeKey(.rightArrow, modifierFlags: .command)
        app.typeKey(.leftArrow, modifierFlags: [.option, .shift])
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Run 3", in: app.staticTexts["query.generation"])
        let diagnostic = app.staticTexts["query.error"]
        waitForValue("Fixture query failed", in: diagnostic)
        editor.click()
        app.typeKey("a", modifierFlags: .command)
        editor.typeText("SELECT 1")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Run 4", in: app.staticTexts["query.generation"])
        waitForValue("Finished", in: app.staticTexts["query.execution"])
        XCTAssertFalse(diagnostic.exists)
        waitForValue("SELECT 1", in: editor)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testNativeUndoRedoSurviveExecutionAndTabSwitching() throws {
        launchApp()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        let editor = app.textViews["query.editor"]
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        editor.click()
        editor.typeText("SELECT 1")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Finished", in: app.staticTexts["query.execution"])
        app.typeKey("z", modifierFlags: .command)
        waitForValue("", in: editor)
        app.typeKey("z", modifierFlags: [.command, .shift])
        waitForValue("SELECT 1", in: editor)
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(app.buttons["Query 2"].waitForExistence(timeout: 5))
        editor.click()
        editor.typeText("SELECT 2")
        app.typeKey(.return, modifierFlags: [])
        editor.typeText("SELECT 3")
        waitForValue("SELECT 2\nSELECT 3", in: editor)
        XCTAssertFalse(app.staticTexts["query.generation"].exists)
        app.buttons["Query 1"].click()
        editor.click()
        app.typeKey("z", modifierFlags: .command)
        waitForValue("", in: editor)
        app.buttons["Query 2"].click()
        waitForValue("SELECT 2\nSELECT 3", in: editor)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testEarlyRowsOriginalCellInspectionAndCancellation() throws {
        app.launchArguments = ["--fixture-query", "wait"]
        launchApp()
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        waitForValue("4", in: app.descendants(matching: .any)["result.cell.4.0"])
        waitForValue("Running", in: app.staticTexts["query.execution"])
        let null = app.descendants(matching: .any)["result.cell.1.1"]
        waitForValue("NULL", in: null)
        null.click()
        app.buttons["results.inspect"].click()
        waitForValue("SQL NULL", in: app.sheets.staticTexts["cell.null"])
        app.sheets.buttons["cell.close"].click()
        app.descendants(matching: .any)["result.cell.0.1"].click()
        app.buttons["results.inspect"].click()
        let value = app.sheets.textViews["cell.value"]
        waitForValue("žluť\nNULL😀", in: value)
        app.sheets.buttons["cell.close"].click()
        app.buttons["query.primary"].click()
        waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
        waitForValue("4", in: app.descendants(matching: .any)["result.cell.4.0"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testHugeCellChunksAndWideResultScrolling() throws {
        app.launchArguments = ["--fixture-query", "huge"]
        launchApp()
        waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
        app.menuBars.menuBarItems["Testing"].click()
        app.menuItems["Reset Viewport Metrics"].click()
        let large = app.descendants(matching: .any)["result.cell.0.1"]
        XCTAssertTrue(large.waitForExistence(timeout: 5))
        XCTAssertTrue((large.value as? String)?.hasSuffix("...") == true)
        large.click()
        app.buttons["results.inspect"].click()
        waitForValue("Characters 1-2048 of 2500000", in: app.sheets.staticTexts["cell.range"])
        app.sheets.buttons["cell.next"].click()
        waitForValue("Characters 2049-4096 of 2500000", in: app.sheets.staticTexts["cell.range"])
        app.sheets.buttons["cell.last"].click()
        waitForValue("Characters 2498561-2500000 of 2500000", in: app.sheets.staticTexts["cell.range"])
        app.sheets.buttons["cell.close"].click()
        let editor = app.textViews["query.editor"]
        editor.click()
        editor.typeText("wide")
        waitForValue("wide", in: editor)
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        waitForValue("r0c2", in: app.descendants(matching: .any)["result.cell.0.2"])
        app.tables["results.table"].scroll(byDeltaX: -25000, deltaY: -13000)
        waitForValue("r499c127", in: app.descendants(matching: .any)["result.cell.499.127"])
        try recordViewportMetrics("Huge cell and wide replacement measurements")
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testWideViewportResourceMeasurements() throws {
        app.launchArguments = ["--fixture-query", "wide"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        app.menuBars.menuBarItems["Testing"].click()
        app.menuItems["Reset Viewport Metrics"].click()
        let table = app.tables["results.table"]
        table.scroll(byDeltaX: -25000, deltaY: -13000)
        waitForValue("r499c127", in: app.descendants(matching: .any)["result.cell.499.127"])
        table.scroll(byDeltaX: 25000, deltaY: 13000)
        waitForValue("r0c2", in: app.descendants(matching: .any)["result.cell.0.2"])
        try recordViewportMetrics("Wide result resource measurements")
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testResultWaitingCutoffFailureCleanupAndEmptyCommandStates() throws {
        app.launchArguments = ["--fixture-query", "waiting"]
        launchApp()
        waitForValue("Waiting for schema", in: app.staticTexts["results.status"])
        app.buttons["query.primary"].click()
        waitForValue("Canceled - 0 rows", in: app.staticTexts["results.status"])
        submitResultQuery("limit")
        waitForValue("Truncated - 10000 rows; waiting for cleanup", in: app.staticTexts["results.status"])
        waitForValue("0", in: app.descendants(matching: .any)["result.cell.0.0"])
        app.buttons["query.primary"].click()
        waitForValue("Finished", in: app.staticTexts["query.execution"])
        XCTAssertTrue((app.staticTexts["results.status"].value as? String)?.hasPrefix("Truncated - 10000 rows") == true)
        submitResultQuery("cleanup_failure")
        waitForValue("Result capture failed; waiting for cleanup", in: app.staticTexts["results.status"])
        XCTAssertTrue(app.staticTexts["results.capture-error"].exists)
        waitForValue("4", in: app.descendants(matching: .any)["result.cell.4.0"])
        app.buttons["query.primary"].click()
        waitForValue("Finished", in: app.staticTexts["query.execution"])
        submitResultQuery("command")
        waitForValue("7 rows affected", in: app.staticTexts["results.status"])
        XCTAssertFalse(app.descendants(matching: .any)["result.cell.0.0"].exists)
        XCTAssertFalse(app.staticTexts["results.read-error"].exists)
        submitResultQuery("empty")
        waitForValue("Complete - 0 rows", in: app.staticTexts["results.status"])
        XCTAssertFalse(app.staticTexts["results.read-error"].exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testResultCopyUsesOriginalValuesAndPreservesNullIdentity() throws {
        app.launchArguments = ["--fixture-query", "wait"]
        launchApp()
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        let pasteboard = NSPasteboard(name: NSPasteboard.Name(fixtureDirectory.lastPathComponent))
        app.descendants(matching: .any)["result.cell.0.1"].click()
        app.typeKey("c", modifierFlags: .command)
        let copied = expectation(for: NSPredicate { _, _ in pasteboard.string(forType: .string) == "žluť\nNULL😀" }, evaluatedWith: nil)
        wait(for: [copied], timeout: 5)
        let type = NSPasteboard.PasteboardType("com.rubyelders.dzban.cell")
        let text = try XCTUnwrap(pasteboard.data(forType: type))
        XCTAssertEqual((try JSONSerialization.jsonObject(with: text) as? [String: Any])?["isNull"] as? Bool, false)
        app.descendants(matching: .any)["result.cell.1.1"].click()
        app.buttons["results.copy"].click()
        let nullCopied = expectation(for: NSPredicate { _, _ in pasteboard.string(forType: .string) == "NULL" }, evaluatedWith: nil)
        wait(for: [nullCopied], timeout: 5)
        let null = try XCTUnwrap(pasteboard.data(forType: type))
        XCTAssertEqual((try JSONSerialization.jsonObject(with: null) as? [String: Any])?["isNull"] as? Bool, true)
        app.buttons["query.primary"].click()
        waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testResultReadFailureRetriesWithoutReplacingOutput() throws {
        app.launchArguments = ["--fixture-query", "wide", "--fail-first-page", "YES"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        waitForValue("Fixture page read failed", in: app.staticTexts["results.read-error"])
        app.buttons["results.retry"].click()
        waitForValue("r0c2", in: app.descendants(matching: .any)["result.cell.0.2"])
        XCTAssertFalse(app.staticTexts["results.read-error"].exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testCatalogNavigationAndPreviewFailureRestart() throws {
        launchApp()
        app.buttons["connection.fixture"].click()
        XCTAssertTrue(app.textViews["query.editor"].waitForExistence(timeout: 10))
        app.toolbars.searchFields["actions.search"].click()
        let actionSearch = app.toolbars.searchFields["actions.search"]
        actionSearch.click()
        actionSearch.typeText("catalog")
        app.buttons["action.catalog"].click()
        XCTAssertTrue(app.sheets.buttons["catalog.describe.žluť table"].waitForExistence(timeout: 5))
        capture("Native catalog list")
        app.sheets.buttons["catalog.describe.žluť table"].click()
        XCTAssertTrue(app.sheets.staticTexts["column_name: id"].waitForExistence(timeout: 5))
        let pattern = app.sheets.textFields["catalog.pattern"]
        pattern.click()
        pattern.typeText("missing*")
        app.sheets.buttons["catalog.list"].click()
        XCTAssertTrue(app.sheets.staticTexts["No catalog entries"].waitForExistence(timeout: 5))
        app.sheets.buttons["catalog.close"].click()
        let editor = app.textViews["query.editor"]
        editor.click()
        editor.typeText("SELECT 'preview_error'")
        app.buttons["plan.open"].click()
        XCTAssertTrue(app.staticTexts["plan.error"].waitForExistence(timeout: 5))
        app.buttons["plan.close"].click()
        editor.click()
        app.typeKey("a", modifierFlags: .command)
        editor.typeText("SELECT 42")
        app.buttons["plan.open"].click()
        XCTAssertTrue(app.textViews["plan.value"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["plan.pin"].exists)
        capture("Native query plan")
        app.buttons["plan.close"].click()
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testNativeRowInspectionPreservesValuesAndOpensFullCell() throws {
        app.launchArguments = ["--fixture-query", "wait"]
        launchApp()
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.descendants(matching: .any)["result.cell.0.1"].click()
        XCTAssertTrue(app.buttons["results.inspect-row"].isEnabled)
        app.buttons["results.inspect-row"].click()
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        waitForValue("žluť\nNULL😀", in: app.staticTexts["row.value.1"])
        capture("Native row inspection")
        app.buttons["row.inspect.1"].click()
        waitForValue("žluť\nNULL😀", in: app.sheets.textViews["cell.value"])
        app.sheets.buttons["cell.close"].click()
        app.buttons["query.primary"].click()
        waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testPlanAndRowTapUseNativeKeyEvents() throws {
        app.launchArguments = ["--fixture-query", "wide"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let editor = app.textViews["query.editor"]
        editor.click()
        editor.typeText("SELECT 42")
        app.typeKey("p", modifierFlags: [.command, .shift])
        XCTAssertTrue(app.textViews["plan.value"].waitForExistence(timeout: 5))
        waitForLabel("Pinned", in: app.buttons["plan.pin"])
        app.buttons["plan.close"].click()
        app.descendants(matching: .any)["result.cell.0.1"].click()
        app.typeKey(" ", modifierFlags: [])
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        waitForLabel("Pinned", in: app.buttons["row.pin"])
        app.buttons["row.close"].click()
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    private func capture(_ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
    private func recordViewportMetrics(_ name: String) throws {
        app.menuBars.menuBarItems["Testing"].click()
        app.menuItems["Record Viewport Metrics"].click()
        let output = fixtureDirectory.appendingPathComponent("viewport.json")
        let written = expectation(for: NSPredicate { _, _ in FileManager.default.fileExists(atPath: output.path) }, evaluatedWith: nil)
        wait(for: [written], timeout: 5)
        let data = try Data(contentsOf: output)
        let report = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: NSNumber])
        XCTAssertLessThanOrEqual(try XCTUnwrap(report["peakCacheBytes"]).intValue, 8 * 1024 * 1024)
        XCTAssertLessThanOrEqual(try XCTUnwrap(report["cachePages"]).intValue, 8)
        XCTAssertLessThanOrEqual(try XCTUnwrap(report["peakPageSchemaReads"]).intValue, 2)
        XCTAssertGreaterThan(try XCTUnwrap(report["mainThreadSamples"]).intValue, 0)
        let attachment = XCTAttachment(data: data, uniformTypeIdentifier: "public.json")
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        print("Native viewport measurements: \(String(decoding: data, as: UTF8.self))")
    }

    func testHistoryRestoresSampleAndSavedPlanWithoutRunning() throws {
        app.launchArguments = ["--fixture-query", "SELECT 42"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        if !app.buttons["history.close"].exists { app.buttons["history.open"].click() }
        let restore = app.buttons["history.restore.0"]
        XCTAssertTrue(restore.waitForExistence(timeout: 10))
        let historyAttachment = XCTAttachment(screenshot: app.screenshot())
        historyAttachment.name = "Native history list"
        historyAttachment.lifetime = .keepAlways
        add(historyAttachment)
        restore.click()
        waitForValue("Saved - 200 of 500 rows", in: app.staticTexts["results.status"])
        waitForValue("SELECT 42", in: app.textViews["query.editor"])
        XCTAssertTrue(app.staticTexts["history.sample-note"].exists)
        app.buttons["history.plan"].click()
        XCTAssertTrue(app.textViews["history.plan.value"].waitForExistence(timeout: 5))
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = "Saved history plan"
        attachment.lifetime = .keepAlways
        add(attachment)
        app.buttons["history.plan.close"].click()
        app.textViews["query.editor"].click()
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        XCTAssertFalse(app.staticTexts["history.sample-note"].exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testLivePostgreSQLWorkflow() throws { try liveWorkflow(postgres: true) }
    func testLiveClickHouseWorkflow() throws { try liveWorkflow(postgres: false) }

    private func liveWorkflow(postgres: Bool) throws {
        guard ProcessInfo.processInfo.environment["DZBAN_TEST_DATABASES"] == "1" else {
            throw XCTSkip("Requires disposable services from scripts/test-macos-live.sh")
        }
        let configuration = fixtureDirectory.appendingPathComponent("live.toml")
        let profile = postgres ? "native-pg" : "native-ch"
        let contents = postgres ? """
        [[databases]]
        name = "native-pg"
        host = "localhost"
        port = 5415
        user = "testuser"
        password = "testpass"
        database = "testdb"
        """ : """
        [[databases]]
        name = "native-ch"
        backend = "clickhouse"
        url = "http://127.0.0.1:58123"
        user = "dzban"
        password = "dzban-test"
        database = "dzban_test"
        """
        try contents.write(to: configuration, atomically: true, encoding: .utf8)
        app.launchEnvironment["DZBAN_CONFIG"] = configuration.path
        app.launchArguments = ["--empty-config", "YES"]
        launchApp()
        let connect = app.buttons["connection.\(profile)"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        XCTAssertTrue(app.textViews["query.editor"].waitForExistence(timeout: 10))
        let sql = postgres
            ? "SELECT 42 AS id, NULL::text AS missing, chr(382)||chr(10)||'value' AS value"
            : "SELECT 42 AS id, CAST(NULL, 'Nullable(String)') AS missing, concat(char(197,190),char(10),'value') AS value"
        submitResultQuery(sql)
        waitForValue("Complete - 1 rows", in: app.staticTexts["results.status"])
        waitForValue("42", in: app.descendants(matching: .any)["result.cell.0.0"])
        let null = app.descendants(matching: .any)["result.cell.0.1"]
        waitForValue("NULL", in: null)
        null.click()
        app.buttons["results.inspect"].click()
        waitForValue("SQL NULL", in: app.sheets.staticTexts["cell.null"])
        app.sheets.buttons["cell.close"].click()
        app.descendants(matching: .any)["result.cell.0.2"].click()
        app.buttons["results.inspect"].click()
        waitForValue("ž\nvalue", in: app.sheets.textViews["cell.value"])
        app.sheets.buttons["cell.close"].click()
        if postgres {
            submitResultQuery("BEGIN")
            waitForValue("0 rows affected", in: app.staticTexts["results.status"])
            submitResultQuery("SELECT 43")
            waitForValue("Complete - 1 rows", in: app.staticTexts["results.status"])
            waitForValue("43", in: app.descendants(matching: .any)["result.cell.0.0"])
            submitResultQuery("ROLLBACK")
            waitForValue("0 rows affected", in: app.staticTexts["results.status"])
        } else {
            submitResultQuery("CREATE TEMPORARY TABLE dzban_native_command (id UInt64)")
            waitForValue("Command completed", in: app.staticTexts["results.status"])
        }
        submitResultQuery("SELECT * FROM dzban_native_missing_table")
        waitForValue("Failed - 0 rows", in: app.staticTexts["results.status"])
        let diagnostic = app.staticTexts["query.error"]
        XCTAssertTrue(diagnostic.waitForExistence(timeout: 5))
        XCTAssertTrue((diagnostic.value as? String)?.contains(postgres ? "42P01" : "60") == true)
        submitResultQuery(postgres ? "SELECT pg_sleep(30)" : "SELECT number, sleepEachRow(0.1) FROM numbers(300) SETTINGS max_block_size=1, max_threads=1")
        waitForValue("Running", in: app.staticTexts["query.execution"])
        app.typeKey("q", modifierFlags: .command)
        let acknowledgment = app.windows.buttons["OK"]
        XCTAssertTrue(acknowledgment.waitForExistence(timeout: 5))
        acknowledgment.click()
        XCTAssertTrue(app.windows.firstMatch.exists)
        app.buttons["query.primary"].click()
        let canceled = expectation(for: NSPredicate(format: "value BEGINSWITH %@", "Canceled -"), evaluatedWith: app.staticTexts["results.status"])
        wait(for: [canceled], timeout: 10)
        submitResultQuery(postgres ? "SELECT i FROM generate_series(1,10005) i" : "SELECT number FROM numbers(10005)")
        waitForValue("Truncated - 10000 rows (row limit)", in: app.staticTexts["results.status"])
        waitForValue("Finished", in: app.staticTexts["query.execution"])
        submitResultQuery("SELECT 44")
        waitForValue("Complete - 1 rows", in: app.staticTexts["results.status"])
        waitForValue("44", in: app.descendants(matching: .any)["result.cell.0.0"])
        if !app.buttons["history.close"].exists { app.buttons["history.open"].click() }
        let restore = app.buttons["history.restore.0"]
        XCTAssertTrue(restore.waitForExistence(timeout: 5))
        restore.click()
        waitForValue("Saved - 1 of 1 rows", in: app.staticTexts["results.status"])
        waitForValue("SELECT 44", in: app.textViews["query.editor"])
        let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
        attachment.name = "Live \(profile) restored result"
        attachment.lifetime = .keepAlways
        add(attachment)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testNativeAppearanceResizeAndAccessibleInspection() throws {
        for appearance in ["Light", "Dark"] {
            app.launchArguments = ["--fixture-query", "wide", "--test-appearance", appearance.lowercased()]
            launchApp()
            waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
            let window = app.windows.firstMatch
            if window.frame.width < 1000 {
                let corner = window.coordinate(withNormalizedOffset: CGVector(dx: 1, dy: 1)).withOffset(CGVector(dx: -2, dy: -2))
                corner.press(forDuration: 0.1, thenDragTo: corner.withOffset(CGVector(dx: 240, dy: 0)))
            }
            let before = window.frame
            let corner = window.coordinate(withNormalizedOffset: CGVector(dx: 1, dy: 1)).withOffset(CGVector(dx: -2, dy: -2))
            corner.press(forDuration: 0.1, thenDragTo: corner.withOffset(CGVector(dx: -120, dy: -60)))
            XCTAssertLessThan(window.frame.width, before.width)
            print("Native \(appearance) window points: \(window.frame.size); main display backing scale: \(NSScreen.main?.backingScaleFactor ?? 0)")
            XCTAssertTrue(app.textViews["query.editor"].isHittable)
            let first = app.descendants(matching: .any)["result.cell.0.0"]
            XCTAssertTrue(first.waitForExistence(timeout: 5))
            first.click()
            app.buttons["results.inspect-row"].click()
            XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
            XCTAssertTrue(app.buttons["row.close"].isHittable)
            let row = XCTAttachment(screenshot: app.screenshot())
            row.name = "Native \(appearance) resized row inspector"
            row.lifetime = .keepAlways
            add(row)
            app.buttons["row.close"].click()
            if !app.buttons["history.close"].exists { app.buttons["history.open"].click() }
            XCTAssertTrue(app.buttons["history.restore.0"].waitForExistence(timeout: 5))
            let history = XCTAttachment(screenshot: window.screenshot())
            history.name = "Native \(appearance) resized history"
            history.lifetime = .keepAlways
            add(history)
            app.buttons["history.close"].click()
            app.typeKey("q", modifierFlags: .command)
            XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
        }
    }

    private func submitResultQuery(_ sql: String) {
        let editor = app.textViews["query.editor"]
        editor.click()
        app.typeKey("a", modifierFlags: .command)
        editor.typeText(sql)
        waitForValue(sql, in: editor)
        app.typeKey(.return, modifierFlags: .command)
    }

    private func launchApp() {
        app.launchArguments += ["-ApplePersistenceIgnoreState", "YES", "--metrics-output", fixtureDirectory.appendingPathComponent("viewport.json").path]
        app.launchArguments += ["--test-pasteboard", fixtureDirectory.lastPathComponent]
        app.launch()
    }

    private func waitForValue(_ value: String, in element: XCUIElement) {
        XCTAssertTrue(element.waitForExistence(timeout: 5))
        let ready = expectation(for: NSPredicate(format: "value == %@", value), evaluatedWith: element)
        wait(for: [ready], timeout: 5)
    }

    private func waitForLabel(_ label: String, in element: XCUIElement) {
        XCTAssertTrue(element.waitForExistence(timeout: 5))
        let ready = expectation(for: NSPredicate(format: "label == %@", label), evaluatedWith: element)
        wait(for: [ready], timeout: 5)
    }

    func testExplicitConfigurationAndMalformedConfigurationLaunch() throws {
        let configuration = fixtureDirectory.appendingPathComponent("explicit.toml")
        try """
        [[databases]]
        name = "disposable-platform-profile"
        host = "localhost"
        port = 1
        user = "fixture"
        database = "fixture"
        """.write(to: configuration, atomically: true, encoding: .utf8)
        app.launchArguments = ["--empty-config", "YES"]
        app.launchEnvironment["DZBAN_CONFIG"] = configuration.path
        launchApp()
        XCTAssertTrue(app.buttons["connection.disposable-platform-profile"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["connection.fixture"].exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
        try "[[databases]\nname = 42".write(to: configuration, atomically: true, encoding: .utf8)
        app.launch()
        XCTAssertTrue(app.windows.buttons["OK"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "value CONTAINS %@ OR label CONTAINS %@", "Failed to parse TOML", "Failed to parse TOML")).firstMatch.exists)
        XCTAssertFalse(app.buttons["connection.disposable-platform-profile"].exists)
        app.windows.buttons["OK"].click()
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testEmptyConfigurationLaunch() throws {
        app.launchArguments = ["--empty-config", "YES"]
        app.launchEnvironment["DZBAN_CONFIG"] = "/dev/null"
        launchApp()
        XCTAssertTrue(app.staticTexts["connections.empty"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["connection.fixture"].exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }
}
