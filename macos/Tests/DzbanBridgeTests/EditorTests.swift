import AppKit
import DzbanBridge
@testable import DzbanApplication
import XCTest

@MainActor
final class EditorTests: XCTestCase {
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
        await store.newDocument()
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
