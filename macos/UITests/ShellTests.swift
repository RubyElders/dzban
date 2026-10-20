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

    func testCommandKSkipsEmptyResultsAndStartupFocusSurvivesRestoration() throws {
        launchApp()
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        let editor = app.textViews["query.editor"]
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        for _ in 0..<3 { app.typeKey("k", modifierFlags: .command) }
        app.typeText("empty")
        waitForValue("empty", in: editor)
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 0 rows", in: app.staticTexts["results.status"])
        for _ in 0..<3 { app.typeKey("k", modifierFlags: .command) }
        app.typeText(" tail")
        waitForValue("empty tail", in: editor)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
        app.launch()
        waitForValue("empty tail", in: editor)
        app.typeKey("k", modifierFlags: .command)
        app.typeKey(.downArrow, modifierFlags: .command)
        app.typeText(" restored")
        waitForValue("empty tail restored", in: editor)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testActionsFocusResultsAndReturnToEditorWithoutChangingDraft() throws {
        app.launchArguments = ["--fixture-query", "wide"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let editor = app.textViews["query.editor"]
        editor.click()
        app.typeText("SELECT hjkl")
        for title in ["Focus Results", "Focus Query Editor", "Focus Results"] {
            app.typeKey("l", modifierFlags: .command)
            app.typeText(title)
            app.typeKey(.return, modifierFlags: [])
            if title == "Focus Query Editor" {
                app.typeText(" tail")
                waitForValue("SELECT hjkl tail", in: editor)
            } else {
                if (editor.value as? String) == "SELECT hjkl" { app.typeKey("j", modifierFlags: []) }
                app.typeText(" ")
                XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
                waitForValue("Row 2", in: app.staticTexts["row.title"])
                app.typeKey(.escape, modifierFlags: [])
            }
        }
        app.typeKey("k", modifierFlags: .command)
        app.typeText(" again")
        waitForValue("SELECT hjkl tail again", in: editor)
        app.typeKey("k", modifierFlags: .command)
        app.typeKey("j", modifierFlags: [])
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        waitForValue("Row 3", in: app.staticTexts["row.title"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey("k", modifierFlags: .command)
        app.typeText(" end")
        waitForValue("SELECT hjkl tail again end", in: editor)
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey("k", modifierFlags: .command)
        XCTAssertFalse(editor.exists)
        app.typeKey("l", modifierFlags: .command)
        app.typeText("Focus Results")
        let action = app.buttons["action.focus-results"]
        XCTAssertTrue(action.waitForExistence(timeout: 5))
        XCTAssertFalse(action.isEnabled)
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testRowPreviewMovesBetweenRowsWithoutMovingPopover() throws {
        app.launchArguments = ["--fixture-query", "few"]
        launchApp()
        waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
        let windowFrame = app.windows.firstMatch.frame
        app.descendants(matching: .any)["result.cell.0.1"].click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        let previewFrame = app.popovers.firstMatch.frame
        waitForValue("žluť\nNULL😀", in: app.staticTexts["row.value.1"])
        XCTAssertEqual(app.popovers.firstMatch.frame, previewFrame)
        XCTAssertEqual(app.windows.firstMatch.frame, windowFrame)
        for _ in 0..<2 {
            for (key, title, value) in [(XCUIKeyboardKey.rightArrow, "Row 2", "1"), (XCUIKeyboardKey(rawValue: "l"), "Row 3", "2"),
                                        (.rightArrow, "Row 3", "2"), (XCUIKeyboardKey(rawValue: "h"), "Row 2", "1"),
                                        (.leftArrow, "Row 1", "0"), (XCUIKeyboardKey(rawValue: "h"), "Row 1", "0"),
                                        (XCUIKeyboardKey(rawValue: "l"), "Row 2", "1"), (.leftArrow, "Row 1", "0")] {
                app.typeKey(key, modifierFlags: [])
                waitForValue(title, in: app.staticTexts["row.title"])
                waitForValue(value, in: app.staticTexts["row.value.0"])
                let positioned = expectation(for: NSPredicate { _, _ in !self.app.progressIndicators["row.loading"].exists }, evaluatedWith: nil)
                wait(for: [positioned], timeout: 5)
                XCTAssertEqual(app.popovers.firstMatch.frame, previewFrame)
            }
        }
        capture("Stable row preview with horizontal row navigation")
        app.typeKey(.return, modifierFlags: [])
        waitForValue("žluť\nNULL😀", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testRowPreviewAnchorsSelectedCellAndSpaceOpensValue() throws {
        app.launchArguments = ["--fixture-query", "wide"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let selected = app.descendants(matching: .any)["result.cell.4.1"]
        selected.click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        capture("Row preview anchored to row five column two")
        app.typeText(" ")
        waitForValue("r4c1", in: app.textViews["cell.value"])
        capture("Second Space opens full value in anchored preview")
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertFalse(app.buttons["row.close"].exists)
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        waitForValue("r4c1", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        app.buttons["result.row-number.6"].click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        capture("Row preview anchored to selected row number")
        app.typeText(" ")
        waitForValue("6", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testFixedRowNumbersSelectRowsWithoutBecomingDataColumns() throws {
        app.launchArguments = ["--fixture-query", "wide"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let table = app.tables["results.table"]
        let number = app.buttons["result.row-number.0"]
        XCTAssertTrue(number.waitForExistence(timeout: 5))
        XCTAssertEqual(number.value as? String, "1")
        let header = app.staticTexts["results.row-number-header"]
        XCTAssertTrue(header.isHittable)
        XCTAssertEqual(header.value as? String, "#")
        let left = number.frame.minX
        XCTAssertLessThan(number.frame.maxX, app.descendants(matching: .any)["result.cell.0.0"].frame.minX)
        number.click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        waitForValue("0", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        scrollInSteps(table, dx: -25000, dy: 0)
        waitForValue("r0c127", in: app.descendants(matching: .any)["result.cell.0.127"])
        XCTAssertEqual(number.frame.minX, left, accuracy: 1)
        XCTAssertTrue(header.isHittable)
        XCTAssertEqual(header.frame.minX, left, accuracy: 1)
        XCTAssertEqual(number.value as? String, "1")
        number.click()
        capture("Fixed row numbers and full row selection during horizontal scrolling")
        scrollInSteps(table, dx: 0, dy: -13000)
        let last = app.buttons["result.row-number.499"]
        XCTAssertTrue(last.waitForExistence(timeout: 5))
        XCTAssertEqual(last.value as? String, "500")
        let firstColumn = table.descendants(matching: .tableColumn).firstMatch.label
        let columnIndex = try XCTUnwrap(Int(firstColumn.replacingOccurrences(of: "column_", with: "")))
        last.click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        waitForValue("r499c\(columnIndex)", in: app.textViews["cell.value"])
        XCTAssertFalse(app.buttons["row.inspect.128"].exists)
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        capture("Final row number stays aligned after vertical scrolling")
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testWorkspaceInsetsShortcutSymbolsAndCompletedCost() throws {
        for appearance in ["light", "dark"] {
            app.launchEnvironment["XDG_DATA_HOME"] = fixtureDirectory.appendingPathComponent(appearance).path
            app.launchArguments = ["--fixture-query", "costed", "--test-appearance", appearance]
            launchApp()
            waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
            XCTAssertTrue(app.staticTexts["results.cost-estimate"].waitForExistence(timeout: 5))
            waitForValue("Cost 0.00..12.50", in: app.staticTexts["results.cost-estimate"])
            let window = app.windows.firstMatch
            resizeWindow(to: 960)
            let status = app.staticTexts["results.status"]
            let close = window.buttons["_XCUI:CloseWindow"]
            XCTAssertEqual(app.descendants(matching: .any)["connection.status"].frame.minX, close.frame.minX, accuracy: 1)
            XCTAssertEqual(app.tables["results.table"].frame.minX, window.frame.minX, accuracy: 2)
            XCTAssertGreaterThanOrEqual(app.staticTexts["results.cost-estimate"].frame.minX, status.frame.maxX)
            XCTAssertLessThan(app.staticTexts["results.cost-estimate"].frame.maxX, app.buttons["connection.probe"].frame.minX)
            capture("Aligned \(appearance) workspace and completed query cost")
            app.typeKey("/", modifierFlags: [.command, .shift])
            XCTAssertTrue(app.buttons["shortcuts.close"].waitForExistence(timeout: 5))
            XCTAssertTrue(app.descendants(matching: .any)["shortcut.Command Question Mark"].exists)
            capture("Native \(appearance) shortcut symbol table")
            XCTAssertTrue(app.descendants(matching: .any)["shortcut.Command Shift Return"].exists)
            app.typeKey(.escape, modifierFlags: [])
            submitResultQuery("wait")
            waitForValue("Receiving - 5 rows", in: status)
            XCTAssertFalse(app.staticTexts["results.cost-estimate"].exists)
            app.typeKey("q", modifierFlags: .command)
            XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
        }
    }

    func testSQLSyntaxHighlightingInLightAndDark() throws {
        let sql = "-- Revenue by customer\nSELECT customer_id, count(*) AS orders,\n       sum(amount) AS total, 'paid' AS status\nFROM orders\nWHERE amount > 12.5 AND status = 'paid'\nGROUP BY customer_id\nORDER BY total DESC;"
        for appearance in ["light", "dark"] {
            app.launchArguments = ["--test-appearance", appearance]
            launchApp()
            if app.buttons["connection.fixture"].waitForExistence(timeout: 2) {
                app.typeKey(.return, modifierFlags: [])
            }
            let editor = app.textViews["query.editor"]
            XCTAssertTrue(editor.waitForExistence(timeout: 5))
            editor.click()
            app.typeKey("a", modifierFlags: .command)
            app.typeText(sql)
            waitForValue(sql, in: editor)
            capture("SQL syntax highlighting \(appearance)")
            app.typeKey("a", modifierFlags: .command)
            app.typeText("SELECT 7")
            waitForValue("SELECT 7", in: editor)
            app.typeKey("z", modifierFlags: .command)
            waitForValue(sql, in: editor)
            app.typeKey("z", modifierFlags: [.command, .shift])
            waitForValue("SELECT 7", in: editor)
            app.typeKey("q", modifierFlags: .command)
            XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
        }
    }

    func testClosingTheFinalConnectionTabQuits() throws {
        launchApp()
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.textViews["query.editor"].waitForExistence(timeout: 5))
        app.typeKey("w", modifierFlags: .command)
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey("w", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
        launchApp()
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'tab.close.'")).firstMatch.click()
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testNewTabReusesTheInitialConnectionPage() throws {
        launchApp()
        let chooser = app.descendants(matching: .any)["connections.chooser"]
        XCTAssertTrue(chooser.waitForExistence(timeout: 5))
        XCTAssertEqual(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'tab.page.'")).count, 1)
        capture("Startup connection tab")
        app.typeKey(.return, modifierFlags: [])
        let editor = app.textViews["query.editor"]
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        app.typeText("wait")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(chooser.waitForExistence(timeout: 5))
        XCTAssertFalse(app.sheets.firstMatch.exists)
        XCTAssertFalse(editor.exists)
        XCTAssertEqual(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'tab.page.'")).count, 1)
        capture("New tab uses the same inline connection page")
        app.typeKey(.tab, modifierFlags: [.control, .shift])
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey(.tab, modifierFlags: .control)
        XCTAssertTrue(chooser.waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(queryTab("Query 2").waitForExistence(timeout: 5))
        XCTAssertEqual(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'tab.page.'")).count, 0)
        XCTAssertEqual(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'workspace.'")).count, 2)
        capture("Connected tab replaces its selector")
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(chooser.waitForExistence(timeout: 5))
        app.typeKey("w", modifierFlags: .command)
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testConnectionSelectionCoversWholeRow() throws {
        launchApp()
        let first = app.descendants(matching: .any)["connection.row.fixture"]
        let second = app.descendants(matching: .any)["connection.row.fixture-slow"]
        XCTAssertTrue(first.waitForExistence(timeout: 5))
        XCTAssertGreaterThanOrEqual(first.frame.maxX, app.buttons["connection.probe.fixture"].frame.maxX)
        capture("Connections first full row selected")
        app.typeKey(.downArrow, modifierFlags: [])
        XCTAssertTrue(second.exists)
        capture("Connections second full row selected")
        app.typeKey(" ", modifierFlags: [])
        XCTAssertTrue(app.staticTexts["connection.health.fixture-slow"].waitForExistence(timeout: 5))
        capture("Connections full row selection after probe")
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.textViews["query.editor"].waitForExistence(timeout: 5))
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(app.buttons["connection.fixture-slow"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.sheets.firstMatch.exists)
        capture("New tab picker full row selection")
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testKeyboardConnectionAndEditorFocus() throws {
        launchApp()
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey("j", modifierFlags: [])
        app.typeKey("k", modifierFlags: [])
        app.typeKey(" ", modifierFlags: [])
        let health = app.staticTexts["connection.health.fixture"]
        XCTAssertTrue(health.waitForExistence(timeout: 5))
        XCTAssertFalse(app.textViews["query.editor"].exists)
        capture("Compact connections with keyboard probe")
        app.typeKey(.return, modifierFlags: [])
        let editor = app.textViews["query.editor"]
        XCTAssertTrue(editor.waitForExistence(timeout: 10))
        app.typeText("hjkl")
        XCTAssertEqual(editor.value as? String, "hjkl")
        app.typeKey("a", modifierFlags: .command)
        app.typeText("SELECT 42")
        XCTAssertEqual(editor.value as? String, "SELECT 42")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        openQueryTab()
        XCTAssertTrue(queryTab("Query 2").waitForExistence(timeout: 5))
        app.typeText("SELECT 7")
        XCTAssertEqual(editor.value as? String, "SELECT 7")
        app.typeKey(.tab, modifierFlags: [.control, .shift])
        waitForValue("SELECT 42", in: editor)
        app.typeKey(.tab, modifierFlags: .control)
        waitForValue("SELECT 7", in: editor)
        app.typeKey(.tab, modifierFlags: [.control, .shift])
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        app.typeText(";")
        XCTAssertEqual(editor.value as? String, "SELECT 42;")
        app.buttons["shortcuts.open"].click()
        XCTAssertTrue(app.sheets.buttons["shortcuts.close"].waitForExistence(timeout: 5))
        app.typeKey(.escape, modifierFlags: [])
        app.typeText(" ")
        XCTAssertEqual(editor.value as? String, "SELECT 42; ")
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(editor.waitForExistence(timeout: 10))
        app.typeText("SELECT 9")
        XCTAssertEqual(editor.value as? String, "SELECT 9")
        app.buttons["query.new"].click()
        app.typeKey(.escape, modifierFlags: [])
        app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'workspace.'")).firstMatch.click()
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        editor.click()
        app.typeText(";")
        XCTAssertEqual(editor.value as? String, "SELECT 42; ;")
        editor.coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: 0.8)).click()
        app.typeText("SELECT 11")
        XCTAssertTrue((editor.value as? String ?? "").contains("SELECT 11"))
        let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
        attachment.name = "Keyboard focused SQL workspace"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testNativeConnectionsStatusAndShortcuts() throws {
        app.launchArguments = ["--fixture-query", "SELECT 42"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let tabs = app.descendants(matching: .any)["query.tabs"]
        let status = app.descendants(matching: .any)["workspace.statusbar"]
        XCTAssertTrue(tabs.exists)
        XCTAssertTrue(status.exists)
        let editor = app.textViews["query.editor"]
        XCTAssertLessThan(tabs.frame.maxY, editor.frame.minY)
        XCTAssertGreaterThan(status.frame.minY, editor.frame.maxY)
        app.buttons["shortcuts.open"].click()
        XCTAssertTrue(app.sheets.buttons["shortcuts.close"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.sheets.descendants(matching: .any)["shortcut.Command Shift Return"].exists)
        app.sheets.buttons["shortcuts.close"].click()
        openQueryTab()
        XCTAssertTrue(queryTab("Query 2").waitForExistence(timeout: 5))
        app.typeKey(.tab, modifierFlags: [.control, .shift])
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        app.typeKey(.tab, modifierFlags: .control)
        waitForValue("Run a query to see results", in: app.staticTexts["results.status"])
        app.typeKey("w", modifierFlags: .command)
        let closed = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: queryTab("Query 2"))
        wait(for: [closed], timeout: 10)
        XCTAssertTrue(queryTab("Query 1").exists)
        app.typeKey("b", modifierFlags: .command)
        XCTAssertTrue(app.buttons["history.close"].waitForExistence(timeout: 5))
        app.typeKey("b", modifierFlags: .command)
        XCTAssertFalse(app.buttons["history.close"].exists)
        app.typeKey("t", modifierFlags: .command)
        app.buttons["connection.fixture-slow"].click()
        XCTAssertTrue(queryTab("Query 1").waitForExistence(timeout: 5))
        app.typeKey(.tab, modifierFlags: [.control, .shift])
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
        attachment.name = "Native connections and status bar"
        attachment.lifetime = .keepAlways
        add(attachment)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testSharedEstimateChordAndMeasuredFooter() throws {
        app.launchArguments = ["--fixture-query", "SELECT 42"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        waitForValue("fixture", in: app.staticTexts["status.version"])
        waitForValue("1µs", in: app.buttons["connection.probe"])
        XCTAssertEqual(app.staticTexts["results.status"].value as? String, "Complete - 500 rows")
        app.buttons["connection.probe"].click()
        waitForValue("1µs", in: app.buttons["connection.probe"])
        app.textViews["query.editor"].click()
        app.textViews["query.editor"].typeText("SELECT 42")
        XCUIElement.perform(withKeyModifiers: [.command, .shift]) {
            app.typeKey(.shift, modifierFlags: [.command, .shift])
            let search = app.toolbars.searchFields["actions.search"]
            let ready = expectation(for: NSPredicate(format: "placeholderValue CONTAINS '↩: run - ' AND NOT placeholderValue CONTAINS 'estimating'"), evaluatedWith: search)
            wait(for: [ready], timeout: 10)
            XCTAssertFalse(app.textViews["plan.value"].exists)
        }
        app.buttons["plan.open"].click()
        XCTAssertTrue(app.buttons["plan.close"].waitForExistence(timeout: 5))
        let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
        attachment.name = "Shared estimate chord and measured footer"
        attachment.lifetime = .keepAlways
        add(attachment)
        app.buttons["plan.close"].click()
        app.typeKey(.return, modifierFlags: [.command, .shift])
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testActionSearchKeyboardAndFocus() throws {
        app.launchArguments = ["--fixture-query", "wait"]
        launchApp()
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        let search = app.toolbars.searchFields["actions.search"]
        app.typeKey("l", modifierFlags: .command)
        XCTAssertTrue(app.buttons["action.new"].waitForExistence(timeout: 5))
        app.typeText("no matching action xyz")
        XCTAssertTrue(app.staticTexts["actions.empty"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.staticTexts["actions.empty"].exists)
        app.typeKey("l", modifierFlags: .command)
        app.typeText("Next Query Tab")
        waitForValue("Next Query Tab", in: search)
        XCTAssertEqual(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'connection.close.'")).count, 1)
        XCTAssertFalse(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'tab.close.'")).firstMatch.exists)
        XCTAssertFalse(app.buttons["action.next-query"].isEnabled)
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.buttons["action.next-query"].exists)
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertFalse(app.buttons["action.next-query"].exists)
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.typeText("SELECT 42")
        waitForValue("SELECT 42", in: app.textViews["query.editor"])
        app.typeKey("l", modifierFlags: .command)
        for _ in 0..<16 { app.typeKey(.downArrow, modifierFlags: []) }
        let last = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'action.workspace.'")).firstMatch
        let visible = expectation(for: NSPredicate(format: "hittable == true"), evaluatedWith: last)
        wait(for: [visible], timeout: 5)
        capture("Action menu keyboard selection scrolled into view")
        app.typeText("New Query Tab")
        waitForValue("New Query Tab", in: search)
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey("l", modifierFlags: .command)
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(queryTab("Query 2").waitForExistence(timeout: 5))
        resizeWindow(to: 900)
        search.click()
        XCTAssertTrue(app.buttons["action.new"].waitForExistence(timeout: 5))
        capture("Action suggestions at minimum window width")
        app.textViews["query.editor"].coordinate(withNormalizedOffset: CGVector(dx: 0.02, dy: 0.8)).click()
        XCTAssertFalse(app.buttons["action.new"].exists)
        app.typeText("SELECT 7")
        waitForValue("SELECT 7", in: app.textViews["query.editor"])
        app.typeKey("l", modifierFlags: .command)
        app.typeText("Keyboard Shortcuts")
        app.typeKey(.downArrow, modifierFlags: [])
        app.typeKey(.upArrow, modifierFlags: [])
        capture("Left aligned action search with filtered suggestion")
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.buttons["shortcuts.close"].waitForExistence(timeout: 5))
        app.typeKey(.escape, modifierFlags: [])
        app.typeText(";")
        waitForValue("SELECT 7;", in: app.textViews["query.editor"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testRemovedShortcutAliasesDoNotInvokeActions() throws {
        app.launchArguments = ["--fixture-query", "wait"]
        launchApp()
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey(".", modifierFlags: .command)
        XCTAssertEqual(app.staticTexts["results.status"].value as? String, "Receiving - 5 rows")
        app.typeKey("k", modifierFlags: [.command, .shift])
        XCTAssertEqual(app.staticTexts["results.status"].value as? String, "Receiving - 5 rows")
        app.typeKey("p", modifierFlags: [.command, .shift])
        XCTAssertEqual(app.staticTexts["results.status"].value as? String, "Receiving - 5 rows")
        app.typeKey(" ", modifierFlags: [.command, .shift])
        XCTAssertEqual(app.staticTexts["results.status"].value as? String, "Receiving - 5 rows")
        app.typeKey(.return, modifierFlags: .control)
        XCTAssertEqual(app.staticTexts["results.status"].value as? String, "Receiving - 5 rows")
        app.typeKey(.return, modifierFlags: [.control, .shift])
        XCTAssertEqual(app.staticTexts["results.status"].value as? String, "Receiving - 5 rows")
        XCTAssertFalse(app.buttons["connection.fixture"].exists)
        XCTAssertFalse(app.buttons["action.new"].exists)
        XCTAssertFalse(app.textViews["plan.value"].exists)
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        openQueryTab()
        let editor = app.textViews["query.editor"]
        app.typeText("second")
        app.typeKey("[", modifierFlags: [.command, .shift])
        app.typeKey("]", modifierFlags: [.command, .shift])
        app.typeKey(.leftArrow, modifierFlags: [.command, .option])
        app.typeKey(.rightArrow, modifierFlags: [.command, .option])
        waitForValue("second", in: editor)
        app.typeKey(.tab, modifierFlags: [.control, .shift])
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.descendants(matching: .any)["result.cell.0.1"].click()
        app.typeKey(.return, modifierFlags: [])
        XCTAssertFalse(app.textViews["cell.value"].exists)
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        waitForValue("žluť\nNULL😀", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testCenteredActionsAndLeftHistory() throws {
        app.launchArguments = ["--fixture-query", "SELECT 42"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        app.typeKey("b", modifierFlags: .command)
        let restore = app.buttons["history.restore.0"]
        XCTAssertTrue(restore.waitForExistence(timeout: 10))
        let editor = app.textViews["query.editor"]
        XCTAssertLessThan(restore.frame.maxX, editor.frame.minX)
        XCTAssertTrue(app.windows.firstMatch.buttons["_XCUI:CloseWindow"].exists)
        XCTAssertTrue(app.toolbars.buttons["history.open"].exists)
        XCTAssertTrue(app.toolbars.buttons["query.primary"].exists)
        XCTAssertTrue(app.buttons["query.primary"].exists)
        let actions = app.toolbars.searchFields["actions.search"]
        let run = app.buttons["query.primary"]
        XCTAssertLessThan(app.buttons["history.open"].frame.maxX, actions.frame.minX)
        XCTAssertGreaterThan(run.frame.minX, actions.frame.maxX)
        app.typeKey("l", modifierFlags: .command)
        let search = app.toolbars.searchFields["actions.search"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["action.new"].waitForExistence(timeout: 5))
        search.click()
        search.typeText("query")
        waitForValue("query", in: search)
        XCTAssertTrue(app.buttons["action.new"].exists)
        XCTAssertFalse(app.buttons["action.disconnect"].exists)
        search.typeKey("a", modifierFlags: .command)
        search.typeText("New Query Tab")
        waitForValue("New Query Tab", in: search)
        search.typeKey(.downArrow, modifierFlags: [])
        search.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.buttons["connection.fixture"].click()
        XCTAssertTrue(queryTab("Query 2").waitForExistence(timeout: 5))
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

    func testToolbarAndFooterStayAlignedDuringProgress() throws {
        app.launchArguments = ["--fixture-query", "waiting"]
        launchApp()
        waitForValue("Running", in: app.buttons["query.primary"])
        let window = app.windows.firstMatch
        resizeWindow(to: 960)
        let field = app.toolbars.searchFields["actions.search"]
        let history = app.toolbars.buttons["history.open"]
        let progress = app.progressIndicators["query.progress"]
        XCTAssertTrue(progress.waitForExistence(timeout: 5))
        XCTAssertEqual(field.frame.midX, window.frame.midX, accuracy: 3)
        XCTAssertLessThan(history.frame.maxX, window.frame.minX + 180)
        XCTAssertGreaterThan(field.frame.minX, history.frame.maxX)
        XCTAssertTrue(app.toolbars.buttons["plan.open"].exists)
        XCTAssertGreaterThan(app.buttons["plan.open"].frame.minX, field.frame.maxX)
        XCTAssertFalse(app.buttons["query.catalog"].exists)
        XCTAssertFalse(app.staticTexts["Cmd+Return to run"].exists)
        XCTAssertLessThanOrEqual(progress.frame.height, 4)
        XCTAssertFalse(app.toolbars.progressIndicators["query.progress"].exists)
        XCTAssertGreaterThanOrEqual(progress.frame.minY, app.textViews["query.editor"].frame.maxY)
        XCTAssertEqual(progress.frame.minX, app.textViews["query.editor"].frame.minX, accuracy: 2)
        XCTAssertEqual(progress.frame.width, app.textViews["query.editor"].frame.width, accuracy: 2)
        for identifier in ["plan.open", "query.new", "query.primary", "connection.probe", "shortcuts.open"] {
            let button = app.buttons[identifier]
            XCTAssertTrue(button.exists)
            XCTAssertGreaterThanOrEqual(button.frame.minX, window.frame.minX)
            XCTAssertLessThanOrEqual(button.frame.maxX, window.frame.maxX)
        }
        for frame in 0..<2 {
            let attachment = XCTAttachment(screenshot: window.screenshot())
            attachment.name = "Native progress frame \(frame)"
            attachment.lifetime = .keepAlways
            add(attachment)
            Thread.sleep(forTimeInterval: 0.2)
        }
        app.typeKey(.return, modifierFlags: .command)
        let complete = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: progress)
        wait(for: [complete], timeout: 10)
        let attachment = XCTAttachment(screenshot: window.screenshot())
        attachment.name = "Native progress reset"
        attachment.lifetime = .keepAlways
        add(attachment)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testSidebarDoesNotMoveToolbarOrRevealWindowTitle() throws {
        app.launchArguments = ["--fixture-query", "SELECT 42"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let window = app.windows.firstMatch
        for width in [1150.0, 960.0] {
            resizeWindow(to: Int(width))
            let toggle = app.toolbars.buttons["history.open"]
            let origin = toggle.frame
            for visible in [true, false, true, false, true] {
                if app.buttons["history.close"].exists != visible { toggle.click() }
                let state = expectation(for: NSPredicate(format: "exists == %@", NSNumber(value: visible)), evaluatedWith: app.buttons["history.close"])
                wait(for: [state], timeout: 5)
                print("History \(visible), window \(window.frame), toggle \(toggle.frame)")
                XCTAssertEqual(toggle.frame.minX, origin.minX, accuracy: 1)
                XCTAssertEqual(toggle.frame.minY, origin.minY, accuracy: 1)
                XCTAssertEqual(toggle.frame.width, origin.width, accuracy: 1)
                XCTAssertEqual(app.toolbars.searchFields["actions.search"].frame.midX, window.frame.midX, accuracy: 3)
                XCTAssertFalse(window.staticTexts["Dzban"].exists)
                let attachment = XCTAttachment(screenshot: window.screenshot())
                attachment.name = "Titlebar \(Int(width)) history \(visible)"
                attachment.lifetime = .keepAlways
                add(attachment)
            }
        }
        let toggle = app.toolbars.buttons["history.open"]
        let origin = toggle.frame
        let splitBefore = app.splitGroups.firstMatch.frame
        let connectionTop = app.descendants(matching: .any)["query.tabs"].frame.minY
        let footerBottom = app.descendants(matching: .any)["workspace.statusbar"].frame.maxY
        app.buttons["query.new"].click()
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        XCTAssertEqual(toggle.frame.minX, origin.minX, accuracy: 1)
        XCTAssertFalse(window.staticTexts["Dzban"].exists)
        XCTAssertEqual(app.descendants(matching: .any)["query.tabs"].frame.minY, connectionTop, accuracy: 1)
        XCTAssertEqual(app.descendants(matching: .any)["workspace.statusbar"].frame.maxY, footerBottom, accuracy: 1)
        let chooser = XCTAttachment(screenshot: window.screenshot())
        chooser.name = "Titlebar connections"
        chooser.lifetime = .keepAlways
        add(chooser)
        let split = app.splitGroups.firstMatch.frame
        print("Workspace split \(splitBefore), chooser split \(split)")
        XCTAssertEqual(split.minY, splitBefore.minY, accuracy: 1)
        XCTAssertEqual(split.maxY, splitBefore.maxY, accuracy: 1)
        app.typeKey(.escape, modifierFlags: [])
        app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'workspace.'")).firstMatch.click()
        XCTAssertTrue(app.buttons["history.close"].waitForExistence(timeout: 5))
        XCTAssertEqual(toggle.frame.minX, origin.minX, accuracy: 1)
        XCTAssertFalse(window.staticTexts["Dzban"].exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testConnectionQueryMenuAndQuit() throws {
        launchApp()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        XCTAssertTrue(queryTab("Query 1").waitForExistence(timeout: 5))
        openQueryTab()
        XCTAssertTrue(queryTab("Query 2").waitForExistence(timeout: 5))
        app.buttons["Close Query 2"].click()
        XCTAssertTrue(queryTab("Query 1").exists)
        let removed = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: queryTab("Query 2"))
        wait(for: [removed], timeout: 10)
        app.typeKey("t", modifierFlags: .command)
        XCTAssertTrue(connect.waitForExistence(timeout: 5))
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testFailedQuitKeepsWindowAvailableForRetry() throws {
        app.launchArguments = ["--fixture-query", "reject_close"]
        launchApp()
        let phase = app.buttons["query.primary"]
        XCTAssertTrue(phase.waitForExistence(timeout: 10))
        let finished = expectation(for: NSPredicate(format: "value == %@", "Finished"), evaluatedWith: phase)
        wait(for: [finished], timeout: 10)
        app.typeKey("q", modifierFlags: .command)
        let acknowledgment = app.windows.buttons["OK"]
        XCTAssertTrue(acknowledgment.waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Fixture cleanup rejected once"].waitForExistence(timeout: 5), app.debugDescription)
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
        XCTAssertFalse(queryTab("Query 1").exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testWindowCloseWaitsForCleanup() throws {
        launchApp()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        XCTAssertTrue(queryTab("Query 1").waitForExistence(timeout: 5))
        app.windows.firstMatch.buttons["_XCUI:CloseWindow"].click()
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testWorkspaceSelectionRestoresQueryTabs() throws {
        launchApp()
        let connect = app.buttons["connection.fixture"]
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        connect.click()
        XCTAssertTrue(queryTab("Query 1").waitForExistence(timeout: 5))
        openQueryTab()
        XCTAssertTrue(queryTab("Query 2").waitForExistence(timeout: 5))
        app.typeKey("t", modifierFlags: .command)
        let second = app.buttons["connection.fixture-slow"]
        XCTAssertTrue(second.waitForExistence(timeout: 5))
        second.click()
        XCTAssertTrue(queryTab("Query 3").waitForExistence(timeout: 5))
        XCTAssertTrue(queryTab("Query 2").exists)
        let original = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@ AND label CONTAINS %@ AND NOT label CONTAINS %@", "workspace.", "fixture", "slow")).firstMatch
        XCTAssertTrue(original.waitForExistence(timeout: 5))
        original.click()
        XCTAssertTrue(queryTab("Query 2").waitForExistence(timeout: 5))
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testNativeEditingExecutionCancellationAndRestart() throws {
        launchApp()
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        let editor = app.textViews["query.editor"]
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        app.typeText("wait")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        openQueryTab()
        guard queryTab("Query 2").waitForExistence(timeout: 5) else { capture("Missing second fixture tab"); XCTFail(app.debugDescription); return }
        XCTAssertTrue(app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH %@", "tab.activity.")).firstMatch.exists)
        capture("Background tab query activity")
        app.typeText("SELECT 2")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        waitForLabel("Run", in: app.buttons["query.primary"])
        queryTab("Query 1").click()
        waitForValue("wait", in: editor)
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        waitForLabel("Stop", in: app.buttons["query.primary"])
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
        editor.click()
        app.typeKey("a", modifierFlags: .command)
        app.typeText("fail")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Fixture query failed", in: app.staticTexts["query.error"])
        app.typeKey("a", modifierFlags: .command)
        app.typeText("SELECT 1")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        queryTab("Query 2").click()
        waitForValue("SELECT 2", in: editor)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        XCTAssertTrue(app.toolbars.buttons["query.primary"].exists)
        XCTAssertEqual(app.descendants(matching: .any).matching(identifier: "query.tabs").count, 1)
        capture("Independent query session tabs")
        editor.click()
        app.typeKey("a", modifierFlags: .command)
        app.typeText("wait")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testBackgroundApplicationCompletionRequestsDockAttentionWithoutActivation() throws {
        launchApp()
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        let editor = app.textViews["query.editor"]
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        app.typeText("delayed_result")
        app.typeKey(.return, modifierFlags: .command)
        XCUIApplication(bundleIdentifier: "com.apple.finder").activate()
        let metrics = fixtureDirectory.appendingPathComponent("viewport.json.attention")
        let notified = expectation(for: NSPredicate { _, _ in
            guard let data = try? Data(contentsOf: metrics),
                  let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return false }
            return value["badge"] as? String == "1" && value["requests"] as? Int == 1
        }, evaluatedWith: nil)
        wait(for: [notified], timeout: 10)
        XCTAssertEqual(NSWorkspace.shared.frontmostApplication?.bundleIdentifier, "com.apple.finder")
        app.activate()
        let viewed = expectation(for: NSPredicate { _, _ in
            guard let data = try? Data(contentsOf: metrics),
                  let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return false }
            return value["badge"] as? String == "" && value["requests"] as? Int == 1
        }, evaluatedWith: nil)
        wait(for: [viewed], timeout: 5)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        XCTAssertFalse(app.images.matching(NSPredicate(format: "identifier BEGINSWITH %@", "tab.unread.")).firstMatch.exists)
        app.typeText(" still here")
        waitForValue("delayed_result still here", in: editor)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testBackgroundTabResultIsMarkedUntilViewed() throws {
        launchApp()
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.textViews["query.editor"].waitForExistence(timeout: 5))
        openQueryTab()
        app.typeText("delayed_result")
        app.typeKey(.return, modifierFlags: .command)
        queryTab("Query 1").click()
        let unread = app.images.matching(NSPredicate(format: "identifier BEGINSWITH %@", "tab.unread.")).firstMatch
        XCTAssertTrue(unread.waitForExistence(timeout: 10))
        waitForLabel("Run", in: app.toolbars.buttons["query.primary"])
        capture("Background tab has an unseen result")
        queryTab("Query 2").click()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        XCTAssertFalse(unread.exists)
        capture("Viewing a tab clears its new result marker")
        submitResultQuery("delayed_failure")
        queryTab("Query 1").click()
        XCTAssertTrue(unread.waitForExistence(timeout: 10))
        waitForLabel("Query failed", in: unread)
        app.typeText("still focused")
        waitForValue("still focused", in: app.textViews["query.editor"])
        capture("Background query failure keeps focus and marks its tab")
        queryTab("Query 2").click()
        waitForValue("Fixture query failed", in: app.staticTexts["query.error"])
        XCTAssertFalse(unread.exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testRunningQuerySummaryAndHiddenHistory() throws {
        launchApp()
        XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
        capture("Compact connection rows with Open and Test buttons")
        app.typeKey(.return, modifierFlags: [])
        let editor = app.textViews["query.editor"]
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["history.close"].exists)
        let editorFrame = editor.frame
        let resultsFrame = app.tables["results.table"].frame
        app.typeText("wait")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        XCTAssertEqual(editor.frame, editorFrame)
        XCTAssertEqual(app.tables["results.table"].frame, resultsFrame)
        waitForValue("Est. ~500 rows", in: app.staticTexts["results.row-estimate"])
        XCTAssertEqual(app.staticTexts["results.row-estimate"].frame.midY, app.staticTexts["results.status"].frame.midY, accuracy: 1)
        let elapsed = app.staticTexts["results.elapsed"]
        XCTAssertTrue(elapsed.waitForExistence(timeout: 5))
        let initial = elapsed.value as? String ?? elapsed.label
        let changed = expectation(for: NSPredicate { _, _ in
            (elapsed.value as? String ?? elapsed.label) != initial
        }, evaluatedWith: elapsed)
        wait(for: [changed], timeout: 5)
        let started = app.staticTexts["results.started"]
        let estimate = app.staticTexts["results.estimate"]
        XCTAssertTrue((started.value as? String ?? started.label).hasPrefix("Started "))
        XCTAssertTrue((estimate.value as? String ?? estimate.label).contains("500 rows - cost 0.00..12.50"))
        let metrics = app.descendants(matching: .any)["results.metrics-box"]
        XCTAssertEqual(metrics.frame.midX, resultsFrame.midX, accuracy: 3)
        XCTAssertEqual(metrics.frame.midY, resultsFrame.midY, accuracy: 16)
        XCTAssertFalse(app.buttons["results.metrics"].exists)
        XCTAssertFalse(app.buttons["results.metrics.close"].exists)
        XCTAssertFalse(app.sheets.firstMatch.exists)
        capture("Centered noninteractive query progress")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
        XCTAssertEqual(editor.frame, editorFrame)
        XCTAssertEqual(app.tables["results.table"].frame, resultsFrame)
        waitForValue("Est. ~500 rows", in: app.staticTexts["results.row-estimate"])
        XCTAssertEqual(app.staticTexts["results.row-estimate"].frame.midY, app.staticTexts["results.status"].frame.midY, accuracy: 1)
        XCTAssertFalse(metrics.exists)
        capture("Completed query clears the progress overlay")
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
        waitForValue("Finished", in: app.buttons["query.primary"])
        app.typeKey("z", modifierFlags: .command)
        waitForValue("", in: editor)
        app.typeKey("z", modifierFlags: [.command, .shift])
        waitForValue("SELECT 1", in: editor)
        openQueryTab()
        XCTAssertTrue(queryTab("Query 2").waitForExistence(timeout: 5))
        editor.click()
        editor.typeText("SELECT 2")
        app.typeKey(.return, modifierFlags: [])
        editor.typeText("SELECT 3")
        waitForValue("SELECT 2\nSELECT 3", in: editor)
        XCTAssertFalse(app.staticTexts["query.generation"].exists)
        queryTab("Query 1").click()
        editor.click()
        app.typeKey("z", modifierFlags: .command)
        waitForValue("", in: editor)
        queryTab("Query 2").click()
        waitForValue("SELECT 2\nSELECT 3", in: editor)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testResultColumnBordersFitSingleAndMultipleColumns() throws {
        app.launchArguments = ["--fixture-query", "single"]
        launchApp()
        waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
        let table = app.tables["results.table"]
        for sql in ["single", "few"] {
            if sql == "few" { submitResultQuery(sql); waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"]) }
            for width in [960.0, 1200.0] {
                resizeWindow(to: Int(width))
                let columns = table.descendants(matching: .tableColumn)
                XCTAssertEqual(columns.count, sql == "single" ? 1 : 2)
                XCTAssertEqual(columns.firstMatch.frame.minX, app.buttons["result.row-number.0"].frame.maxX, accuracy: 2)
                XCTAssertEqual(columns.element(boundBy: columns.count - 1).frame.maxX, table.frame.maxX, accuracy: 2)
                if sql == "few" { try assertMatchingDividerPixels(table: table, column: columns.firstMatch) }
                capture("Full-height result dividers \(sql) \(Int(width))")
            }
        }
        submitResultQuery("wide")
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        scrollInSteps(table, dx: -25000, dy: 0)
        scrollInSteps(table, dx: 0, dy: -13000)
        waitForValue("r499c127", in: app.descendants(matching: .any)["result.cell.499.127"])
        let lastColumn = table.descendants(matching: .tableColumn).matching(NSPredicate(format: "label == %@", "column_127")).firstMatch
        XCTAssertGreaterThanOrEqual(lastColumn.frame.width, 160)
        capture("Full-height dividers with horizontal scrolling")
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testQuitAndRelaunchRestoresTabsAndSQL() throws {
        launchApp()
        app.typeKey(.return, modifierFlags: [])
        let editor = app.textViews["query.editor"]
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        editor.click()
        editor.typeText("SELECT 'first draft';")
        app.typeKey("t", modifierFlags: .command)
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        editor.click()
        editor.typeText("SELECT 'second draft';")
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
        app.launch()
        waitForValue("SELECT 'second draft';", in: editor)
        app.typeKey(.tab, modifierFlags: [.control, .shift])
        waitForValue("SELECT 'first draft';", in: editor)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testExecuteShortcutRunsStopsAndRestarts() throws {
        app.launchArguments = ["--fixture-query", "wait"]
        launchApp()
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        let editor = app.textViews["query.editor"]
        editor.click()
        editor.typeKey("a", modifierFlags: .command)
        editor.typeText("wait")
        for modifiers: XCUIElement.KeyModifierFlags in [.command, [.command, .shift]] {
            app.typeKey(.return, modifierFlags: modifiers)
            waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
            app.typeKey(.return, modifierFlags: modifiers)
            waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        }
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testEarlyRowsOriginalCellInspectionAndCancellation() throws {
        app.launchArguments = ["--fixture-query", "wait"]
        launchApp()
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        waitForValue("4", in: app.descendants(matching: .any)["result.cell.4.0"])
        waitForValue("Running", in: app.buttons["query.primary"])
        capture("Plain edge-to-edge result rows while running")
        let null = app.descendants(matching: .any)["result.cell.1.1"]
        waitForValue("NULL", in: null)
        null.doubleClick()
        waitForValue("SQL NULL", in: app.sheets.staticTexts["cell.null"])
        app.sheets.buttons["cell.close"].click()
        app.descendants(matching: .any)["result.cell.0.1"].doubleClick()
        let value = app.sheets.textViews["cell.value"]
        waitForValue("žluť\nNULL😀", in: value)
        app.sheets.buttons["cell.close"].click()
        app.buttons["query.primary"].click()
        waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
        waitForValue("4", in: app.descendants(matching: .any)["result.cell.4.0"])
        capture("Plain edge-to-edge result rows after cancellation")
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
        large.doubleClick()
        XCTAssertTrue(app.sheets.staticTexts["cell.range"].waitForExistence(timeout: 30))
        waitForValue("Characters 1-2500000 of 2500000", in: app.sheets.staticTexts["cell.range"])
        XCTAssertFalse(app.buttons["cell.previous"].exists)
        XCTAssertFalse(app.buttons["cell.next"].exists)
        XCTAssertFalse(app.buttons["cell.last"].exists)
        app.sheets.buttons["cell.close"].click()
        let editor = app.textViews["query.editor"]
        editor.click()
        editor.typeText("wide")
        waitForValue("wide", in: editor)
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        waitForValue("r0c2", in: app.descendants(matching: .any)["result.cell.0.2"])
        scrollInSteps(app.tables["results.table"], dx: -25000, dy: 0)
        scrollInSteps(app.tables["results.table"], dx: 0, dy: -13000)
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
        scrollInSteps(table, dx: -25000, dy: 0)
        scrollInSteps(table, dx: 0, dy: -13000)
        waitForValue("r499c127", in: app.descendants(matching: .any)["result.cell.499.127"])
        scrollInSteps(table, dx: 25000, dy: 0)
        scrollInSteps(table, dx: 0, dy: 13000)
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
        waitForValue("Finished", in: app.buttons["query.primary"])
        XCTAssertTrue((app.staticTexts["results.status"].value as? String)?.hasPrefix("Truncated - 10000 rows") == true)
        submitResultQuery("cleanup_failure")
        waitForValue("Result capture failed; waiting for cleanup", in: app.staticTexts["results.status"])
        XCTAssertTrue(app.staticTexts["results.capture-error"].exists)
        waitForValue("4", in: app.descendants(matching: .any)["result.cell.4.0"])
        app.buttons["query.primary"].click()
        waitForValue("Finished", in: app.buttons["query.primary"])
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

    func testRerunKeepsResultScrollAndCellWithMatchingHeaders() throws {
        app.launchArguments = ["--fixture-query", "wide"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let editor = app.textViews["query.editor"]
        editor.click()
        app.typeText("wide")
        waitForValue("wide", in: editor)
        let table = app.tables["results.table"]
        scrollInSteps(table, dx: -25000, dy: 0)
        scrollInSteps(table, dx: 0, dy: -13000)
        let last = app.descendants(matching: .any)["result.cell.499.127"]
        waitForValue("r499c127", in: last)
        last.click()
        let before = last.frame
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        waitForValue("r499c127", in: last)
        XCTAssertEqual(last.frame.minX, before.minX, accuracy: 2)
        XCTAssertEqual(last.frame.minY, before.minY, accuracy: 2)
        capture("Matching query rerun preserves result position")
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        waitForValue("r499c127", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        editor.click()
        app.typeKey("a", modifierFlags: .command)
        app.typeText("single")
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
        waitForValue("0", in: app.descendants(matching: .any)["result.cell.0.0"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testCommandShiftSpaceOpensPlanFromEditorAndResults() throws {
        app.launchArguments = ["--fixture-query", "few"]
        launchApp()
        waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
        let editor = app.textViews["query.editor"]
        app.activate()
        editor.click()
        app.typeText("few")
        waitForValue("few", in: editor)
        for results in [false, true] {
            if results { app.typeKey("k", modifierFlags: .command) }
            app.typeKey(" ", modifierFlags: [.command, .shift])
            XCTAssertTrue(app.buttons["plan.close"].waitForExistence(timeout: 5))
            XCTAssertTrue(app.textViews["plan.value"].waitForExistence(timeout: 5))
            XCTAssertEqual(app.textViews["plan.value"].value as? String, "fixture: true\nsql: few")
            capture("Text plan without duplicate estimate summary")
            waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
            app.typeKey(.escape, modifierFlags: [])
            XCTAssertTrue(app.buttons["plan.close"].waitForNonExistence(timeout: 5))
        }
        app.typeKey("/", modifierFlags: [.command, .shift])
        XCTAssertTrue(app.descendants(matching: .any)["shortcut.Command Shift Space"].waitForExistence(timeout: 5))
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testHeldCommandAcrossRunFocusSelectAllCopyAndReturn() throws {
        app.launchArguments = ["--fixture-query", "few"]
        launchApp()
        waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
        let editor = app.textViews["query.editor"]
        editor.click()
        app.typeText("few")
        let pasteboard = NSPasteboard(name: NSPasteboard.Name(fixtureDirectory.lastPathComponent))
        XCUIElement.perform(withKeyModifiers: .command) {
            app.typeKey(.return, modifierFlags: .command)
            waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
            app.typeKey("k", modifierFlags: .command)
            app.typeKey("a", modifierFlags: .command)
            app.typeKey("c", modifierFlags: .command)
            let copied = expectation(for: NSPredicate { _, _ in
                pasteboard.string(forType: .string) == "| id | value |\n| --- | --- |\n| 0 | žluť<br>NULL😀 |\n| 1 | NULL |\n| 2 | žluť<br>NULL😀 |\n"
            }, evaluatedWith: nil)
            wait(for: [copied], timeout: 5)
            app.typeKey("k", modifierFlags: .command)
        }
        app.typeText(" back")
        waitForValue("few back", in: editor)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testResultCopySelectedRowAndSelectAllAsMarkdown() throws {
        app.launchArguments = ["--fixture-query", "wait"]
        launchApp()
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        let pasteboard = NSPasteboard(name: NSPasteboard.Name(fixtureDirectory.lastPathComponent))
        let header = "| id | value |\n| --- | --- |\n"
        app.descendants(matching: .any)["result.cell.0.1"].click()
        app.typeKey("c", modifierFlags: .command)
        let copied = expectation(for: NSPredicate { _, _ in pasteboard.string(forType: .string) == header + "| 0 | žluť<br>NULL😀 |\n" }, evaluatedWith: nil)
        wait(for: [copied], timeout: 5)
        app.descendants(matching: .any)["result.cell.1.1"].click()
        app.typeKey("c", modifierFlags: .command)
        let nullCopied = expectation(for: NSPredicate { _, _ in pasteboard.string(forType: .string) == header + "| 1 | NULL |\n" }, evaluatedWith: nil)
        wait(for: [nullCopied], timeout: 5)
        app.typeKey("a", modifierFlags: .command)
        app.typeKey("c", modifierFlags: .command)
        let allRows = header + (0..<5).map { "| \($0) | \($0 == 1 ? "NULL" : "žluť<br>NULL😀") |\n" }.joined()
        let allCopied = expectation(for: NSPredicate { _, _ in pasteboard.string(forType: .string) == allRows }, evaluatedWith: nil)
        capture("All result rows selected before Markdown copy check")
        let allOutcome = XCTWaiter.wait(for: [allCopied], timeout: 5)
        XCTAssertEqual(pasteboard.string(forType: .string), allRows)
        XCTAssertEqual(allOutcome, .completed)
        app.buttons["query.primary"].click()
        waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testIntegratedResultStatesInLightAndDark() throws {
        for appearance in ["light", "dark"] {
            app.launchEnvironment["XDG_DATA_HOME"] = fixtureDirectory.appendingPathComponent(appearance).path
            app.launchArguments = ["--test-appearance", appearance]
            launchApp()
            XCTAssertTrue(app.buttons["connection.fixture"].waitForExistence(timeout: 5))
            app.typeKey(.return, modifierFlags: [])
            waitForValue("Ready to query", in: app.staticTexts["results.state-title"])
            let window = app.windows.firstMatch
            resizeWindow(to: 1150)
            XCTAssertEqual(window.frame.width, 1150, accuracy: 2)
            XCTAssertEqual(window.frame.height, 760, accuracy: 2)
            let editor = app.textViews["query.editor"]
            let frame = editor.frame
            capture("Integrated results ready - \(appearance)")
            submitResultQuery("empty")
            waitForValue("Complete - 0 rows", in: app.staticTexts["results.status"])
            waitForValue("No rows returned", in: app.staticTexts["results.state-title"])
            XCTAssertEqual(editor.frame, frame)
            capture("Integrated empty results - \(appearance)")
            submitResultQuery("fail")
            waitForValue("Fixture query failed", in: app.staticTexts["query.error"])
            waitForValue("Query failed", in: app.staticTexts["results.state-title"])
            let result = app.tables["results.table"]
            let feedback = app.descendants(matching: .any)["results.feedback"]
            XCTAssertEqual(feedback.frame.midY, result.frame.midY, accuracy: 2)
            XCTAssertLessThan(app.staticTexts["query.error"].frame.maxY, result.frame.maxY - 16)
            XCTAssertEqual(editor.frame, frame)
            capture("Integrated query failure - \(appearance)")
            submitResultQuery("wait")
            waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
            XCTAssertTrue(app.staticTexts["results.elapsed"].waitForExistence(timeout: 5))
            XCTAssertFalse(app.staticTexts["query.error"].exists)
            XCTAssertEqual(editor.frame, frame)
            capture("Integrated running results - \(appearance)")
            app.typeKey(.return, modifierFlags: .command)
            waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
            XCTAssertFalse(app.staticTexts["results.elapsed"].exists)
            app.typeKey("q", modifierFlags: .command)
            XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
        }
    }

    func testResultReadFailureRetriesWithoutReplacingOutput() throws {
        app.launchArguments = ["--fixture-query", "wide", "--fail-first-page", "YES"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        waitForValue("Fixture page read failed", in: app.staticTexts["results.read-error"])
        capture("Integrated result loading error with retry")
        app.buttons["results.retry"].click()
        waitForValue("r0c2", in: app.descendants(matching: .any)["result.cell.0.2"])
        XCTAssertFalse(app.staticTexts["results.read-error"].exists)
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testCatalogIsAbsentAndPreviewFailureRestarts() throws {
        launchApp()
        app.buttons["connection.fixture"].click()
        XCTAssertTrue(app.textViews["query.editor"].waitForExistence(timeout: 10))
        app.toolbars.searchFields["actions.search"].click()
        let actionSearch = app.toolbars.searchFields["actions.search"]
        actionSearch.click()
        actionSearch.typeText("catalog")
        XCTAssertTrue(app.staticTexts["actions.empty"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["action.catalog"].exists)
        app.typeKey(.escape, modifierFlags: [])
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
        XCTAssertFalse(app.buttons["results.inspect-row"].exists)
        XCTAssertFalse(app.buttons["results.inspect"].exists)
        XCTAssertFalse(app.buttons["results.copy"].exists)
        capture("Results without inspection buttons")
        app.descendants(matching: .any)["result.cell.0.1"].click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        waitForValue("žluť\nNULL😀", in: app.staticTexts["row.value.1"])
        capture("Native row inspection")
        app.buttons["row.inspect.1"].click()
        waitForValue("žluť\nNULL😀", in: app.textViews["cell.value"])
        XCTAssertEqual(app.sheets.count, 0)
        capture("Cell inspection inside row popover")
        app.buttons["cell.back"].click()
        waitForValue("žluť\nNULL😀", in: app.staticTexts["row.value.1"])
        app.buttons["row.inspect.0"].click()
        waitForValue("0", in: app.textViews["cell.value"])
        app.buttons["cell.close"].click()
        app.descendants(matching: .any)["result.cell.0.1"].click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.buttons["row.close"].click()
        app.buttons["query.primary"].click()
        waitForValue("Canceled - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testCellDetailSupportsStandardSelectAllAndCopy() throws {
        let pasteboard = NSPasteboard(name: NSPasteboard.Name(fixtureDirectory.lastPathComponent))
        app.launchArguments = ["--fixture-query", "wait"]
        launchApp()
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        let result = app.descendants(matching: .any)["result.cell.0.1"]
        for inline in [false, true] {
            if inline {
                result.click()
                app.typeText(" ")
                XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
                app.typeKey(.return, modifierFlags: [])
            } else {
                result.doubleClick()
            }
            waitForValue("žluť\nNULL😀", in: app.textViews["cell.value"])
            pasteboard.clearContents()
            app.typeKey("a", modifierFlags: .command)
            app.typeKey("c", modifierFlags: .command)
            let copied = expectation(for: NSPredicate { _, _ in pasteboard.string(forType: .string) == "žluť\nNULL😀" }, evaluatedWith: nil)
            wait(for: [copied], timeout: 5)
            app.typeKey(.escape, modifierFlags: [])
            if inline {
                XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
                app.typeKey(.escape, modifierFlags: [])
            }
        }
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testRowPreviewSelectionKeyboardAndScrollbarSpacing() throws {
        app.launchArguments = ["--fixture-query", "wide"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        app.descendants(matching: .any)["result.cell.0.0"].click()
        app.typeKey("l", modifierFlags: [])
        app.typeKey("h", modifierFlags: [])
        app.typeKey("l", modifierFlags: [])
        app.typeKey("j", modifierFlags: [])
        app.typeKey("k", modifierFlags: [])
        app.typeText(" ")
        let scroll = app.scrollViews["row.columns"]
        XCTAssertTrue(scroll.waitForExistence(timeout: 5))
        app.typeKey("j", modifierFlags: [])
        capture("Row preview border and selected-cell anchor")
        app.typeText(" ")
        waitForValue("r0c2", in: app.textViews["cell.value"])
        XCTAssertEqual(app.sheets.count, 0)
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertTrue(app.buttons["row.inspect.2"].waitForExistence(timeout: 5))
        app.typeKey("k", modifierFlags: [])
        app.typeText(" ")
        waitForValue("r0c1", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        let button = app.buttons["row.inspect.1"]
        XCTAssertLessThan(button.frame.maxX, scroll.frame.maxX - 12)
        capture("Selected row preview with reserved scrollbar space")
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertFalse(app.buttons["row.close"].exists)
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        waitForValue("r0c1", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        submitResultQuery("few")
        waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
        app.tables["results.table"].coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.9)).click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        waitForValue("0", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        submitResultQuery("wait")
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.descendants(matching: .any)["result.cell.0.0"].click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        app.typeKey(.return, modifierFlags: [])
        waitForValue("0", in: app.textViews["cell.value"])
        app.typeKey(.escape, modifierFlags: [])
        app.typeKey(.escape, modifierFlags: [])
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testRowInspectorCentersSelectedAndVisibleColumns() throws {
        app.launchArguments = ["--fixture-query", "wide"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        let table = app.tables["results.table"]
        let first = app.descendants(matching: .any)["result.cell.0.0"]
        first.click()
        for _ in 0..<64 { app.typeKey(.rightArrow, modifierFlags: []) }
        app.typeText(" ")
        let selected = app.buttons["row.inspect.64"]
        XCTAssertTrue(selected.waitForExistence(timeout: 5))
        let scroll = app.scrollViews["row.columns"]
        let centered = expectation(for: NSPredicate { _, _ in selected.isHittable && abs(selected.frame.midY - scroll.frame.midY) < 70 }, evaluatedWith: nil)
        wait(for: [centered], timeout: 5)
        capture("Row preview centered on selected column")
        app.buttons["row.close"].click()
        scrollInSteps(table, dx: -25000, dy: 0)
        let lastCell = app.descendants(matching: .any)["result.cell.0.127"]
        XCTAssertTrue(lastCell.waitForExistence(timeout: 5))
        lastCell.click()
        app.typeText(" ")
        let last = app.buttons["row.inspect.127"]
        XCTAssertTrue(last.waitForExistence(timeout: 5))
        XCTAssertTrue(last.isHittable)
        capture("Row preview at visible final columns")
        app.buttons["row.close"].click()
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testRowInspectorScrollsThroughAllColumns() throws {
        app.launchArguments = ["--fixture-query", "wide"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        app.descendants(matching: .any)["result.cell.0.0"].click()
        app.typeText(" ")
        let close = app.buttons["row.close"]
        XCTAssertTrue(close.waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Previous Columns"].exists)
        XCTAssertFalse(app.buttons["Next Columns"].exists)
        let last = app.buttons["row.inspect.127"]
        let scroll = app.scrollViews["row.columns"]
        for _ in 0..<35 {
            if last.exists && last.isHittable && scroll.frame.contains(last.frame) { break }
            scroll.scroll(byDeltaX: 0, deltaY: -450)
        }
        XCTAssertTrue(last.isHittable)
        XCTAssertTrue(scroll.frame.contains(last.frame))
        capture("Row inspector scrolled to final column")
        last.click()
        waitForValue("r0c127", in: app.textViews["cell.value"])
        app.buttons["cell.back"].click()
        app.buttons["row.close"].click()
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
        app.buttons["plan.open"].click()
        XCTAssertTrue(app.textViews["plan.value"].waitForExistence(timeout: 5))
        waitForLabel("Pinned", in: app.buttons["plan.pin"])
        app.buttons["plan.close"].click()
        app.descendants(matching: .any)["result.cell.0.1"].click()
        app.typeText(" ")
        XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["row.pin"].exists)
        app.buttons["row.close"].click()
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    private func assertMatchingDividerPixels(table: XCUIElement, column: XCUIElement) throws {
        let frame = app.windows.firstMatch.frame
        let bitmap = try XCTUnwrap(NSBitmapImageRep(data: app.windows.firstMatch.screenshot().pngRepresentation))
        let scaleX = CGFloat(bitmap.pixelsWide) / frame.width
        let scaleY = CGFloat(bitmap.pixelsHigh) / frame.height
        let edge = Int(((column.frame.maxX - frame.minX) * scaleX).rounded())
        func band(at y: CGFloat) throws -> [Bool] {
            let row = Int(((y - frame.minY) * scaleY).rounded())
            func brightness(_ x: Int) throws -> CGFloat {
                let color = try XCTUnwrap(bitmap.colorAt(x: x, y: row)?.usingColorSpace(.deviceRGB))
                return (color.redComponent + color.greenComponent + color.blueComponent) / 3
            }
            let background = try brightness(edge + 4)
            return try (-4...4).map { abs(try brightness(edge + $0) - background) > 0.03 }
        }
        let header = try band(at: table.frame.minY + 8)
        let body = try band(at: table.frame.minY + 40)
        XCTAssertTrue(header.contains(true))
        XCTAssertEqual(header, body, "Header and result dividers must occupy the same pixels")
    }

    private func capture(_ name: String) {
        let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
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

    func testHistoryKeyboardSelectionSpacingAndFocus() throws {
        app.launchArguments = ["--fixture-query", "SELECT 42"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        submitResultQuery("single")
        waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
        submitResultQuery("few")
        waitForValue("Complete - 3 rows", in: app.staticTexts["results.status"])
        app.typeKey("b", modifierFlags: .command)
        XCTAssertTrue(app.buttons["history.restore.2"].waitForExistence(timeout: 5))
        let list = app.scrollViews["history.entries"]
        let refresh = app.buttons["history.refresh"]
        let top = list.frame.minY
        capture("Compact history with first row selected")
        app.typeKey(.return, modifierFlags: [])
        waitForValue("Saved - 3 of 3 rows", in: app.staticTexts["results.status"])
        waitForValue("few", in: app.textViews["query.editor"])
        app.typeText(" -- draft")
        waitForValue("few -- draft", in: app.textViews["query.editor"])
        app.typeKey("b", modifierFlags: .command)
        app.typeKey("b", modifierFlags: .command)
        XCTAssertTrue(app.buttons["history.restore.2"].waitForExistence(timeout: 5))
        app.typeKey("j", modifierFlags: [])
        refresh.click()
        let ready = expectation(for: NSPredicate(format: "enabled == YES"), evaluatedWith: refresh)
        wait(for: [ready], timeout: 5)
        XCTAssertEqual(list.frame.minY, top, accuracy: 1)
        XCTAssertLessThan(app.buttons["history.restore.1"].frame.maxX, list.frame.maxX - 10)
        capture("Compact history selection preserved after refresh")
        app.typeKey(.return, modifierFlags: [])
        waitForValue("single", in: app.textViews["query.editor"])
        app.typeKey("b", modifierFlags: .command)
        app.typeKey("b", modifierFlags: .command)
        XCTAssertTrue(app.buttons["history.close"].waitForExistence(timeout: 5))
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertFalse(app.buttons["history.close"].exists)
        app.typeKey("a", modifierFlags: .command)
        app.typeText("typed after history")
        waitForValue("typed after history", in: app.textViews["query.editor"])
        submitResultQuery("wait")
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("b", modifierFlags: .command)
        XCTAssertTrue(app.buttons["history.restore.0"].waitForExistence(timeout: 5))
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertFalse(app.buttons["history.close"].exists)
        waitForValue("Receiving - 5 rows", in: app.staticTexts["results.status"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testEmptyHistoryFocusInDarkAppearance() throws {
        app.launchArguments = ["--test-appearance", "dark"]
        launchApp()
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.textViews["query.editor"].waitForExistence(timeout: 5))
        app.typeKey("b", modifierFlags: .command)
        XCTAssertTrue(app.staticTexts["No saved queries"].waitForExistence(timeout: 5))
        capture("Compact empty history in dark appearance")
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertFalse(app.buttons["history.close"].exists)
        app.typeText("SELECT 42")
        waitForValue("SELECT 42", in: app.textViews["query.editor"])
        app.typeKey(.return, modifierFlags: .command)
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        app.typeKey("b", modifierFlags: .command)
        XCTAssertTrue(app.buttons["history.restore.0"].waitForExistence(timeout: 5))
        capture("Compact selected history in dark appearance")
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testHistoryRestoresSampleAndSavedPlanWithoutRunning() throws {
        app.launchArguments = ["--fixture-query", "SELECT 42"]
        launchApp()
        waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
        if !app.buttons["history.close"].exists { app.buttons["history.open"].click() }
        let restore = app.buttons["history.restore.0"]
        XCTAssertTrue(restore.waitForExistence(timeout: 10))
        let entries = app.scrollViews["history.entries"]
        let row = app.descendants(matching: .any)["history.row.0"]
        XCTAssertEqual(row.frame.minX - entries.frame.minX, 4, accuracy: 1)
        XCTAssertEqual(entries.frame.maxX - row.frame.maxX, 4, accuracy: 1)
        let historyAttachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
        historyAttachment.name = "Native history list"
        historyAttachment.lifetime = .keepAlways
        add(historyAttachment)
        restore.click()
        waitForValue("Saved - 200 of 500 rows", in: app.staticTexts["results.status"])
        waitForValue("SELECT 42", in: app.textViews["query.editor"])
        let note = app.staticTexts["history.sample-note"]
        XCTAssertTrue(note.exists)
        XCTAssertEqual(note.frame.midY, app.staticTexts["results.status"].frame.midY, accuracy: 1)
        capture("Saved sample information inside results bar")
        app.buttons["history.plan"].click()
        XCTAssertTrue(app.textViews["history.plan.value"].waitForExistence(timeout: 5))
        let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
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
            throw XCTSkip("Requires disposable services from scripts/macos.sh test-live")
        }
        let configuration = fixtureDirectory.appendingPathComponent("live.toml")
        let profile = postgres ? "native-pg" : "native-ch"
        let pgPort = ProcessInfo.processInfo.environment["DZBAN_TEST_PG_PORT"] ?? "5415"
        let chPort = ProcessInfo.processInfo.environment["DZBAN_TEST_CH_PORT"] ?? "58123"
        let contents = postgres ? """
        [[databases]]
        name = "native-pg"
        host = "localhost"
        port = \(pgPort)
        user = "testuser"
        password = "testpass"
        database = "testdb"
        """ : """
        [[databases]]
        name = "native-ch"
        backend = "clickhouse"
        url = "http://127.0.0.1:\(chPort)"
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
        let version = app.staticTexts["status.version"]
        XCTAssertTrue(version.waitForExistence(timeout: 5))
        XCTAssertTrue((version.value as? String)?.contains(postgres ? "PostgreSQL" : "25.") == true)
        XCTAssertNotEqual(app.buttons["connection.probe"].value as? String, "--")
        app.buttons["connection.probe"].click()
        let sql = postgres
            ? "SELECT 42 AS id, NULL::text AS missing, chr(382)||chr(10)||'value' AS value"
            : "SELECT 42 AS id, CAST(NULL, 'Nullable(String)') AS missing, concat(char(197,190),char(10),'value') AS value"
        submitResultQuery(sql)
        waitForValue("Complete - 1 rows", in: app.staticTexts["results.status"])
        XCTAssertTrue((app.staticTexts["status.duration"].value as? String)?.contains("s") == true)
        waitForValue("42", in: app.descendants(matching: .any)["result.cell.0.0"])
        let null = app.descendants(matching: .any)["result.cell.0.1"]
        waitForValue("NULL", in: null)
        null.doubleClick()
        waitForValue("SQL NULL", in: app.sheets.staticTexts["cell.null"])
        app.sheets.buttons["cell.close"].click()
        app.descendants(matching: .any)["result.cell.0.2"].doubleClick()
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
        if postgres {
            submitResultQuery("CREATE TEMP TABLE dzban_tab_scope (id integer)")
            waitForValue("0 rows affected", in: app.staticTexts["results.status"])
        }
        submitResultQuery("SELECT * FROM dzban_native_missing_table")
        waitForValue("Failed - 0 rows", in: app.staticTexts["results.status"])
        let diagnostic = app.staticTexts["query.error"]
        XCTAssertTrue(diagnostic.waitForExistence(timeout: 5))
        XCTAssertTrue((diagnostic.value as? String)?.contains(postgres ? "42P01" : "60") == true)
        submitResultQuery(postgres ? "SELECT pg_sleep(30)" : "SELECT number, sleepEachRow(0.1) FROM numbers(300) SETTINGS max_block_size=1, max_threads=1")
        waitForValue("Running", in: app.buttons["query.primary"])
        openQueryTab(profile: profile)
        guard queryTab("Query 2").waitForExistence(timeout: 10) else {
            capture("Missing second live query tab")
            XCTFail(app.debugDescription)
            return
        }
        submitResultQuery("SELECT 45")
        waitForValue("Complete - 1 rows", in: app.staticTexts["results.status"])
        waitForValue("45", in: app.descendants(matching: .any)["result.cell.0.0"])
        if postgres {
            submitResultQuery("SELECT to_regclass('pg_temp.dzban_tab_scope') IS NULL AS isolated")
            waitForValue("Complete - 1 rows", in: app.staticTexts["results.status"])
            waitForValue("true", in: app.descendants(matching: .any)["result.cell.0.0"])
        }
        queryTab("Query 1").click()
        waitForValue("Running", in: app.buttons["query.primary"])
        app.buttons["query.primary"].click()
        let canceled = expectation(for: NSPredicate(format: "value BEGINSWITH %@", "Canceled -"), evaluatedWith: app.staticTexts["results.status"])
        wait(for: [canceled], timeout: 10)
        submitResultQuery(postgres ? "SELECT i FROM generate_series(1,10005) i" : "SELECT number FROM numbers(10005)")
        waitForValue("Truncated - 10000 rows (row limit)", in: app.staticTexts["results.status"])
        waitForValue("Finished", in: app.buttons["query.primary"])
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
        submitResultQuery(postgres ? "SELECT pg_sleep(30)" : "SELECT number, sleepEachRow(0.1) FROM numbers(300) SETTINGS max_block_size=1, max_threads=1")
        waitForValue("Running", in: app.buttons["query.primary"])
        queryTab("Query 2").click()
        submitResultQuery(postgres ? "SELECT pg_sleep(30)" : "SELECT number, sleepEachRow(0.1) FROM numbers(300) SETTINGS max_block_size=1, max_threads=1")
        waitForValue("Running", in: app.buttons["query.primary"])
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
    }

    func testNativeAppearanceResizeAndAccessibleInspection() throws {
        for appearance in ["Light", "Dark"] {
            app.launchArguments = ["--fixture-query", "wide", "--test-appearance", appearance.lowercased()]
            launchApp()
            waitForValue("Complete - 500 rows", in: app.staticTexts["results.status"])
            let window = app.windows.firstMatch
            resizeWindow(to: 1150)
            resizeWindow(to: 1030)
            print("Native \(appearance) window points: \(window.frame.size); main display backing scale: \(NSScreen.main?.backingScaleFactor ?? 0)")
            XCTAssertTrue(app.textViews["query.editor"].isHittable)
            let first = app.descendants(matching: .any)["result.cell.0.0"]
            XCTAssertTrue(first.waitForExistence(timeout: 5))
            first.click()
            app.typeText(" ")
            XCTAssertTrue(app.buttons["row.close"].waitForExistence(timeout: 5))
            XCTAssertTrue(app.buttons["row.close"].isHittable)
            let row = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
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

    private func resizeWindow(to width: Int) {
        let window = app.windows.firstMatch
        app.menuBars.menuBarItems["Testing"].click()
        app.menuItems["Resize Window to \(width)"].click()
        let resized = expectation(for: NSPredicate { _, _ in abs(window.frame.width - Double(width)) < 2 }, evaluatedWith: nil)
        wait(for: [resized], timeout: 5)
    }

    private func submitResultQuery(_ sql: String) {
        let editor = app.textViews["query.editor"]
        editor.click()
        app.typeKey("a", modifierFlags: .command)
        editor.typeText(sql)
        waitForValue(sql, in: editor)
        app.typeKey(.return, modifierFlags: .command)
    }

    private func queryTab(_ title: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'workspace.' AND label CONTAINS %@", title)).firstMatch
    }

    private func openQueryTab(profile: String = "fixture") {
        app.typeKey("t", modifierFlags: .command)
        let connect = app.buttons["connection.\(profile)"]
        XCTAssertTrue(connect.waitForExistence(timeout: 5))
        connect.click()
    }

    private func launchApp() {
        app.launchArguments += ["-ApplePersistenceIgnoreState", "YES", "--metrics-output", fixtureDirectory.appendingPathComponent("viewport.json").path]
        app.launchArguments += ["--test-pasteboard", fixtureDirectory.lastPathComponent]
        app.launch()
    }

    private func scrollInSteps(_ element: XCUIElement, dx: CGFloat, dy: CGFloat) {
        let steps = max(1, Int((max(abs(dx), abs(dy)) / 2000).rounded(.up)))
        for _ in 0..<steps {
            element.scroll(byDeltaX: dx / CGFloat(steps), deltaY: dy / CGFloat(steps))
        }
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
