import AppKit
import DzbanBridge
@testable import DzbanApplication
import XCTest

@MainActor
final class EditorTests: XCTestCase {
    func testPlanTextUsesSharedPresentationInsteadOfSerializingJSON() {
        let text = "Seq Scan on widgets  (cost=0.00..12.50 rows=500 width=16)\n  Filter: (id > 10)"
        let plan: JSONValue = .object(["text": .string(text), "document": .object(["Plan": .string("raw JSON")])])
        XCTAssertEqual(PlanFormatting.text(plan), text)
        XCTAssertEqual(PlanFormatting.text(.object(["document": .object([:])])), "No text plan available")
    }

    func testCompletionAttentionGroupsBackgroundRequestsAndClearsOnlyViewedTabs() {
        var state = QueryAttentionState()
        let first = UUID(), second = UUID()
        XCTAssertFalse(state.complete(session: first, outcome: .success, selected: true))
        XCTAssertTrue(state.unseen.isEmpty)
        XCTAssertFalse(state.complete(session: first, outcome: .success, selected: false))
        XCTAssertEqual(state.unseen.count, 1)
        state.setVisible(false, selected: first)
        XCTAssertTrue(state.complete(session: second, outcome: .success, selected: true))
        XCTAssertFalse(state.complete(session: second, outcome: .success, selected: true))
        XCTAssertFalse(state.complete(session: first, outcome: .failure, selected: false))
        XCTAssertEqual(state.unseen.count, 2)
        XCTAssertEqual(state.unseen[first], .failure)
        state.viewed(second)
        XCTAssertEqual(state.unseen.count, 2)
        state.setVisible(true, selected: second)
        XCTAssertEqual(state.unseen.count, 1)
        XCTAssertNotNil(state.unseen[first])
        state.viewed(first)
        XCTAssertTrue(state.unseen.isEmpty)
        state.setVisible(false, selected: nil)
        XCTAssertTrue(state.complete(session: first, outcome: .success, selected: false))
        state.remove(first)
        XCTAssertTrue(state.unseen.isEmpty)
        state.clear()
        XCTAssertTrue(state.complete(session: second, outcome: .failure, selected: true))
    }

    func testFocusCycleUsesParticipatingVisibleViewsAndLeavesSingleParticipantAlone() {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 600, height: 240), styleMask: [.titled], backing: .buffered, defer: false)
        let cycle = WorkspaceFocusCycle()
        let views = (0..<3).map { index in NSTextView(frame: NSRect(x: index * 180, y: 0, width: 160, height: 200)) }
        for view in views {
            window.contentView?.addSubview(view)
            cycle.update(view, included: false) { window.makeFirstResponder(view) }
        }
        window.makeFirstResponder(views[0])
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === views[0])
        cycle.update(views[0], included: true) { window.makeFirstResponder(views[0]) }
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === views[0])
        window.makeFirstResponder(nil)
        let unfocused = window.firstResponder
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === unfocused)
        for view in views { cycle.update(view, included: true) { window.makeFirstResponder(view) } }
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === views[0])
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === views[1])
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === views[2])
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === views[0])
        views[1].isHidden = true
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === views[2])
        views[2].removeFromSuperview()
        window.makeFirstResponder(views[0])
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === views[0])
        cycle.remove(views[0])
        cycle.advance(in: window)
        XCTAssertTrue(window.firstResponder === views[0])
    }

    func testVimNavigationUsesArrowEventsAndPreservesTextAndModifiers() throws {
        func event(_ text: String, modifiers: NSEvent.ModifierFlags = [], type: NSEvent.EventType = .keyDown, repeatKey: Bool = false) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(with: type, location: .zero, modifierFlags: modifiers,
                timestamp: 42, windowNumber: 0, context: nil, characters: text,
                charactersIgnoringModifiers: text, isARepeat: repeatKey, keyCode: 0))
        }
        for (letter, code, character) in [("h", 123, "\u{f702}"), ("j", 125, "\u{f701}"), ("k", 126, "\u{f700}"), ("l", 124, "\u{f703}")] {
            for type in [NSEvent.EventType.keyDown, .keyUp] {
                let original = try event(letter, type: type, repeatKey: true)
                let arrow = VimNavigation.remap(original, responder: NSView())
                XCTAssertEqual(arrow.keyCode, UInt16(code))
                XCTAssertEqual(arrow.characters, character)
                XCTAssertEqual(arrow.type, type)
                XCTAssertEqual(arrow.timestamp, 42)
                XCTAssertTrue(arrow.isARepeat)
                for responder in [NSTextView(), NSTextField(), NSSearchField()] as [NSResponder] {
                    XCTAssertTrue(VimNavigation.remap(original, responder: responder) === original)
                }
            }
            for modifiers: NSEvent.ModifierFlags in [.command, .control, .option, .shift] {
                let original = try event(letter, modifiers: modifiers)
                XCTAssertTrue(VimNavigation.remap(original, responder: NSView()) === original)
            }
        }
        let other = try event("a")
        XCTAssertTrue(VimNavigation.remap(other, responder: NSView()) === other)
    }

    func testRowEstimateDistinguishesOutputAndScanRows() {
        let estimate: JSONValue = .object(["rows": .number(500)])
        XCTAssertEqual(PlanFormatting.rowEstimate(.object(["backend": .string("postgresql"), "estimate": estimate])), "Est. ~500 rows")
        XCTAssertEqual(PlanFormatting.rowEstimate(.object(["backend": .string("clickhouse"), "estimate": estimate])), "Scan est. ~500 rows")
        XCTAssertEqual(PlanFormatting.rowEstimate(.object(["estimate": .object(["rows": .number(0)])])), "Est. ~0 rows")
        XCTAssertNil(PlanFormatting.rowEstimate(.object(["estimate": .object(["rows": .null])])))
        XCTAssertNil(PlanFormatting.rowEstimate(.object(["estimate": .object(["rows": .number(-1)])])))
    }

    func testRowNumberGutterWidthFollowsDigits() {
        XCTAssertEqual(ResultRowNumberLayout.width(rows: 0), 32)
        XCTAssertEqual(ResultRowNumberLayout.width(rows: 99), 32)
        XCTAssertEqual(ResultRowNumberLayout.width(rows: 100), 40)
        XCTAssertEqual(ResultRowNumberLayout.width(rows: 999), 40)
        XCTAssertEqual(ResultRowNumberLayout.width(rows: 1000), 48)
        XCTAssertEqual(ResultRowNumberLayout.width(rows: 1_000_000), 72)
    }

    func testPlannerCostFormattingRequiresFiniteNonnegativeCost() {
        func plan(_ fields: [String: JSONValue]) -> JSONValue { .object(["estimate": .object(fields)]) }
        XCTAssertEqual(PlanFormatting.costEstimate(plan(["startup_cost": .number(0), "total_cost": .number(12.5)])), "Cost 0.00..12.50")
        XCTAssertEqual(PlanFormatting.costEstimate(plan(["total_cost": .number(0)])), "Cost 0.00")
        XCTAssertNil(PlanFormatting.costEstimate(plan(["rows": .number(500), "parts": .number(2)])))
        XCTAssertNil(PlanFormatting.costEstimate(plan(["total_cost": .null])))
        XCTAssertNil(PlanFormatting.costEstimate(plan(["total_cost": .number(-1)])))
        XCTAssertNil(PlanFormatting.costEstimate(plan(["total_cost": .number(.infinity)])))
    }

    func testSQLSyntaxKeepsQuotedTextAndCommentsSeparateFromKeywords() {
        let sql = "SELECT 'it''s FROM 😀', \"select\", `order`, 12.5e-2, .25 FROM žluť WHERE n = 3 -- SELECT 9\n/* outer /* nested */ FROM */"
        let tokens = SQLSyntax.tokens(in: sql)
        func text(_ kind: SQLSyntax.Kind) -> [String] {
            tokens.filter { $0.kind == kind }.map { (sql as NSString).substring(with: $0.range) }
        }
        XCTAssertEqual(text(.keyword), ["SELECT", "FROM", "WHERE"])
        XCTAssertEqual(text(.string), ["'it''s FROM 😀'"])
        XCTAssertEqual(text(.identifier), ["\"select\"", "`order`"])
        XCTAssertEqual(text(.number), ["12.5e-2", ".25", "3"])
        XCTAssertEqual(text(.comment), ["-- SELECT 9", "/* outer /* nested */ FROM */"])
        let dollar = "$tag$SELECT 'text' 😀$tag$ SELECT $$FROM$$"
        let parts = SQLSyntax.tokens(in: dollar)
        XCTAssertEqual(parts.map(\.kind), [.string, .keyword, .string])
        XCTAssertEqual(parts.map { (dollar as NSString).substring(with: $0.range) }, ["$tag$SELECT 'text' 😀$tag$", "SELECT", "$$FROM$$"])
        XCTAssertEqual(SQLSyntax.tokens(in: "SELECT 'unfinished").map(\.kind), [.keyword, .string])
    }

    func testSyntaxColorsDoNotChangeTextSelectionOrUndoHistory() throws {
        let buffer = EditorBuffer(text: "SELECT '😀'")
        let view = buffer.textView
        let undo = try XCTUnwrap(view.undoManager)
        XCTAssertFalse(undo.canUndo)
        let layout = try XCTUnwrap(view.layoutManager)
        let initialColor = try XCTUnwrap(layout.temporaryAttribute(.foregroundColor, atCharacterIndex: 0, effectiveRange: nil) as? NSColor)
        XCTAssertNotEqual(initialColor, view.textStorage?.attribute(.foregroundColor, at: 0, effectiveRange: nil) as? NSColor)
        view.setSelectedRange(NSRange(location: view.string.utf16.count, length: 0))
        undo.groupsByEvent = false
        undo.beginUndoGrouping()
        view.insertText(" WHERE n = 42", replacementRange: view.selectedRange())
        undo.endUndoGrouping()
        XCTAssertEqual(view.string, "SELECT '😀' WHERE n = 42")
        XCTAssertEqual(view.selectedRange(), NSRange(location: view.string.utf16.count, length: 0))
        let number = (view.string as NSString).range(of: "42")
        XCTAssertNotNil(layout.temporaryAttribute(.foregroundColor, atCharacterIndex: number.location, effectiveRange: nil))
        undo.undo()
        XCTAssertEqual(view.string, "SELECT '😀'")
        XCTAssertFalse(undo.canUndo)
        undo.redo()
        XCTAssertEqual(view.string, "SELECT '😀' WHERE n = 42")
    }

    func testUTF16AndScalarConversionsIncludeSurrogatesAndCombiningMarks() {
        let text = "A😀e\u{301}ž"
        XCTAssertEqual(EditorOffsets.scalarRange(in: text, utf16: NSRange(location: 1, length: 2)), 1..<2)
        XCTAssertEqual(EditorOffsets.scalarRange(in: text, utf16: NSRange(location: 3, length: 2)), 2..<4)
        XCTAssertEqual(EditorOffsets.scalarRange(in: text, utf16: NSRange(location: 4, length: 1)), 3..<4)
        XCTAssertEqual(EditorOffsets.scalarRange(in: text, utf16: NSRange(location: 2, length: 0)), 1..<1)
        XCTAssertEqual(EditorOffsets.scalarRange(in: text, utf16: NSRange(location: 2, length: 1)), 1..<2)
        XCTAssertEqual(EditorOffsets.scalarRange(in: text, utf16: NSRange(location: NSNotFound, length: Int.max)), 5..<5)
        XCTAssertEqual(EditorOffsets.utf16Range(in: text, scalars: 1..<4), NSRange(location: 1, length: 4))
        for start in 0...5 {
            for end in start...5 {
                let range = EditorOffsets.utf16Range(in: text, scalars: start..<end)
                XCTAssertEqual(EditorOffsets.scalarRange(in: text, utf16: range), start..<end)
            }
        }
    }

    func testShortcutAliasesRepeatsCompositionAndNativeEditing() {
        for keys: NSEvent.ModifierFlags in [.command, [.command, .shift], [.command, .shift, .numericPad]] {
            XCTAssertTrue(EditorShortcuts.queryPrimary(keys))
        }
        for keys: NSEvent.ModifierFlags in [[], .control, [.control, .shift], [.command, .control], [.command, .option]] {
            XCTAssertFalse(EditorShortcuts.queryPrimary(keys))
        }
        XCTAssertEqual(EditorShortcuts.intent(keyCode: 36, modifiers: .command, isRepeat: false, composing: false), .submit)
        XCTAssertEqual(EditorShortcuts.intent(keyCode: 76, modifiers: [.command, .shift, .numericPad], isRepeat: false, composing: false), .submit)
        XCTAssertEqual(EditorShortcuts.intent(keyCode: 53, modifiers: [], isRepeat: false, composing: false), .cancel)
        XCTAssertNil(EditorShortcuts.intent(keyCode: 36, modifiers: .command, isRepeat: true, composing: false))
        XCTAssertNil(EditorShortcuts.intent(keyCode: 36, modifiers: .command, isRepeat: false, composing: true))
        XCTAssertNil(EditorShortcuts.intent(keyCode: 36, modifiers: [], isRepeat: false, composing: false))
        XCTAssertNil(EditorShortcuts.intent(keyCode: 6, modifiers: .command, isRepeat: false, composing: false))
        XCTAssertNil(EditorShortcuts.intent(keyCode: 36, modifiers: [.command, .control], isRepeat: false, composing: false))
    }

    func testNativeCompositionAndUndoRemainInTheDocumentEditor() throws {
        let buffer = EditorBuffer(text: "")
        let view = buffer.textView
        let coordinator = SQLEditor.Coordinator(buffer: buffer, changed: {})
        view.delegate = coordinator
        let undo = try XCTUnwrap(view.undoManager)
        undo.groupsByEvent = false
        undo.beginUndoGrouping()
        view.setMarkedText("に", selectedRange: NSRange(location: 1, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertTrue(view.hasMarkedText())
        view.insertText("日本", replacementRange: NSRange(location: NSNotFound, length: 0))
        view.unmarkText()
        XCTAssertFalse(view.hasMarkedText())
        XCTAssertEqual(view.string, "日本")
        undo.endUndoGrouping()
        view.breakUndoCoalescing()
        undo.beginUndoGrouping()
        view.insertText("語", replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertEqual(view.string, "日本語")
        undo.endUndoGrouping()
        view.undoManager?.undo()
        XCTAssertEqual(view.string, "日本")
        view.undoManager?.redo()
        XCTAssertEqual(view.string, "日本語")
        XCTAssertEqual(buffer.text, "日本語")
    }

    func testHistoryReplacementPreservesNativeUndoAndUnicode() throws {
        let buffer = EditorBuffer(text: "SELECT 'draft 😀'")
        let coordinator = SQLEditor.Coordinator(buffer: buffer, changed: {})
        buffer.textView.delegate = coordinator
        let undo = try XCTUnwrap(buffer.textView.undoManager)
        undo.groupsByEvent = false
        undo.beginUndoGrouping()
        buffer.restore(text: "SELECT 'saved 日本語'")
        undo.endUndoGrouping()
        XCTAssertEqual(buffer.textView.string, "SELECT 'saved 日本語'")
        XCTAssertEqual(buffer.text, buffer.textView.string)
        XCTAssertEqual(buffer.savedVersion, buffer.version)
        undo.undo()
        XCTAssertEqual(buffer.textView.string, "SELECT 'draft 😀'")
        XCTAssertEqual(buffer.text, buffer.textView.string)
        XCTAssertGreaterThan(buffer.version, buffer.savedVersion)
        undo.redo()
        XCTAssertEqual(buffer.text, "SELECT 'saved 日本語'")
    }

    func testNativeEditorConsumesHeldSubmitWithoutRepeatingAndResetsFocus() throws {
        let buffer = EditorBuffer(text: "SELECT 1")
        var intents: [EditorIntent] = []
        buffer.textView.intent = { intents.append($0); return true }
        let initial = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0, windowNumber: 0, context: nil, characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36))
        let repeated = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 1, windowNumber: 0, context: nil, characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: true, keyCode: 36))
        XCTAssertTrue(buffer.textView.performKeyEquivalent(with: initial))
        XCTAssertTrue(buffer.textView.performKeyEquivalent(with: repeated))
        XCTAssertEqual(intents, [.submit])
        XCTAssertTrue(buffer.textView.resignFirstResponder())
        XCTAssertEqual(intents, [.submit, .reset])
    }

    func testMenuSubmissionWaitsForNativeCompositionToCommit() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.loadProfiles()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let document = try XCTUnwrap(store.document)
        let editor = store.editor(for: document)
        editor.textView.setMarkedText("に", selectedRange: NSRange(location: 1, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertTrue(editor.textView.hasMarkedText())
        await store.performEditorIntent(.submit)
        XCTAssertEqual(store.document?.generation, 0)
        editor.textView.unmarkText()
        try await store.shutdown()
    }

    func testDocumentCloseAndShutdownReleaseNativeEditors() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.loadProfiles()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let original = try XCTUnwrap(store.document)
        weak var firstEditor: EditorBuffer?
        weak var firstView: NSTextView?
        autoreleasepool {
            let editor = store.editor(for: original)
            firstEditor = editor
            firstView = editor.textView
            editor.textView.insertText("SELECT 1", replacementRange: NSRange(location: 0, length: 0))
        }
        _ = await store.connect(profile: "fixture")
        await store.closeDocument(original.document)
        XCTAssertNil(firstEditor)
        for _ in 0..<100 {
            if firstView == nil { break }
            try await Task.sleep(for: .milliseconds(20))
        }
        XCTAssertNil(firstView)
        weak var finalEditor: EditorBuffer?
        do { finalEditor = store.editor(for: try XCTUnwrap(store.document)) }
        try await store.shutdown()
        XCTAssertNil(finalEditor)
    }

    func testStructuredDiagnosticsPreserveBackendDetails() {
        let error = JSONValue.object(["Database": .object([
            "message": .string("Syntax error"), "code": .object(["SqlState": .string("42601")]),
            "detail": .string("At line 2"), "hint": .string("Check the expression"),
        ])])
        XCTAssertEqual(QueryDiagnostic.describe(error), "Syntax error\n42601\nAt line 2\nCheck the expression")
    }
}
