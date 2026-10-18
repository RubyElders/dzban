import AppKit
import Combine
import DzbanBridge
import SwiftUI

public enum EditorOffsets {
    public static func scalarRange(in text: String, utf16 range: NSRange) -> Range<Int> {
        let units = Array(text.utf16)
        func scalarOffset(_ offset: Int) -> Int {
            var boundary = min(max(offset, 0), units.count)
            if boundary > 0, boundary < units.count,
               (0xDC00...0xDFFF).contains(units[boundary]), (0xD800...0xDBFF).contains(units[boundary - 1]) {
                boundary -= 1
            }
            return String(decoding: units.prefix(boundary), as: UTF16.self).unicodeScalars.count
        }
        let start = scalarOffset(range.location)
        let location = min(max(range.location, 0), units.count)
        let remaining = units.count - location
        let end = scalarOffset(location + min(max(range.length, 0), remaining))
        return start..<max(start, end)
    }

    public static func utf16Range(in text: String, scalars range: Range<Int>) -> NSRange {
        let scalars = Array(text.unicodeScalars)
        let start = min(max(range.lowerBound, 0), scalars.count)
        let end = min(max(range.upperBound, start), scalars.count)
        let location = scalars.prefix(start).reduce(0) { $0 + $1.utf16.count }
        let length = scalars[start..<end].reduce(0) { $0 + $1.utf16.count }
        return NSRange(location: location, length: length)
    }
}

public enum EditorShortcuts {
    static func queryPrimary(_ modifiers: NSEvent.ModifierFlags) -> Bool {
        let keys = modifiers.intersection([.command, .control, .option, .shift])
        return keys == .command || keys == [.command, .shift]
    }

    public static func intent(keyCode: UInt16, modifiers: NSEvent.ModifierFlags, isRepeat: Bool, composing: Bool) -> EditorIntent? {
        guard !isRepeat, !composing else { return nil }
        let modifiers = modifiers.intersection([.command, .control, .option, .shift])
        if (keyCode == 36 || keyCode == 76), modifiers == .command || modifiers == [.command, .shift] { return .submit }
        if keyCode == 53, modifiers.isEmpty { return .cancel }
        return nil
    }
}

@MainActor
public final class EditorBuffer: ObservableObject {
    @Published public private(set) var text: String
    public private(set) var selection = NSRange(location: 0, length: 0)
    public private(set) var version: UInt64 = 0
    public private(set) var savedVersion: UInt64 = 0
    let textView = SQLTextView()
    lazy var scrollView: NSScrollView = {
        let scroll = EditorScrollView()
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.documentView = textView
        return scroll
    }()

    public init(text: String) {
        self.text = text
        textView.string = text
        textView.highlightSyntax()
    }

    public func edit(text: String, selection: NSRange) {
        if self.text != text { self.text = text; version &+= 1 }
        self.selection = selection
    }

    public func restore(text: String) {
        let range = NSRange(location: 0, length: textView.string.utf16.count)
        textView.breakUndoCoalescing()
        textView.insertText(text, replacementRange: range)
        edit(text: textView.string, selection: textView.selectedRange())
        acknowledge(version)
    }

    weak var focusCycle: WorkspaceFocusCycle?

    public func requestFocus() {
        textView.focusWhenAttached = true
        textView.focusIfNeeded()
    }

    func retire() {
        textView.intent = nil
        textView.delegate = nil
        textView.breakUndoCoalescing()
        textView.undoManager?.removeAllActions()
        textView.isEditable = false
    }

    public func acknowledge(_ version: UInt64) { savedVersion = max(savedVersion, version) }
    public var scalarSelection: Range<Int> { EditorOffsets.scalarRange(in: text, utf16: selection) }
}

public struct SQLEditor: NSViewRepresentable {
    @ObservedObject var buffer: EditorBuffer
    let editable: Bool
    let changed: () -> Void
    let contextChanged: () -> Void
    let action: (EditorIntent) -> Bool

    public init(buffer: EditorBuffer, editable: Bool, changed: @escaping () -> Void, contextChanged: @escaping () -> Void = {}, action: @escaping (EditorIntent) -> Bool) {
        self.buffer = buffer
        self.editable = editable
        self.changed = changed
        self.contextChanged = contextChanged
        self.action = action
    }

    public func makeCoordinator() -> Coordinator { Coordinator(buffer: buffer, changed: changed, contextChanged: contextChanged) }
    public func makeNSView(context: Context) -> NSScrollView {
        let view = buffer.textView
        view.delegate = context.coordinator
        view.intent = action
        buffer.requestFocus()
        return buffer.scrollView
    }
    public func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.changed = changed
        context.coordinator.contextChanged = contextChanged
        buffer.textView.intent = action
        if buffer.textView.isEditable != editable { buffer.textView.isEditable = editable }
        buffer.focusCycle?.update(buffer.textView, included: editable) { [weak buffer] in buffer?.requestFocus() }
    }

    public static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.buffer.focusCycle?.remove(coordinator.buffer.textView)
    }

    @MainActor
    public final class Coordinator: NSObject, NSTextViewDelegate {
        let buffer: EditorBuffer
        var changed: () -> Void
        var contextChanged: () -> Void
        init(buffer: EditorBuffer, changed: @escaping () -> Void, contextChanged: @escaping () -> Void = {}) { self.buffer = buffer; self.changed = changed; self.contextChanged = contextChanged }
        public func textDidChange(_ notification: Notification) {
            buffer.edit(text: buffer.textView.string, selection: buffer.textView.selectedRange())
            buffer.textView.needsDisplay = true
            changed()
        }
        public func textViewDidChangeSelection(_ notification: Notification) {
            let previous = buffer.selection
            buffer.edit(text: buffer.textView.string, selection: buffer.textView.selectedRange())
            if previous != buffer.selection { contextChanged() }
        }
    }
}

@MainActor
final class SQLTextView: NSTextView {
    var intent: ((EditorIntent) -> Bool)?
    var focusWhenAttached = false
    override var textContainerOrigin: NSPoint {
        NSPoint(x: WorkspaceLayout.outerInset + 2, y: super.textContainerOrigin.y)
    }

    func focusIfNeeded() {
        guard focusWhenAttached, let window, window.attachedSheet == nil else { return }
        if window.makeFirstResponder(self) { focusWhenAttached = false }
    }
    private let documentUndoManager = UndoManager()
    private let documentStorage: NSTextStorage
    override var undoManager: UndoManager? { documentUndoManager }

    init() {
        let storage = NSTextStorage()
        let layout = NSLayoutManager()
        let container = NSTextContainer(containerSize: NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude))
        storage.addLayoutManager(layout)
        layout.addTextContainer(container)
        documentStorage = storage
        super.init(frame: .zero, textContainer: container)
        isEditable = true
        isSelectable = true
        isRichText = false
        allowsUndo = true
        isAutomaticQuoteSubstitutionEnabled = false
        isAutomaticDashSubstitutionEnabled = false
        isAutomaticTextReplacementEnabled = false
        isAutomaticSpellingCorrectionEnabled = false
        isContinuousSpellCheckingEnabled = false
        font = .monospacedSystemFont(ofSize: 14, weight: .regular)
        textContainerInset = NSSize(width: (WorkspaceLayout.outerInset + 2 + WorkspaceLayout.textInset) / 2, height: 12)
        textContainer?.lineFragmentPadding = 0
        isVerticallyResizable = true
        isHorizontallyResizable = false
        autoresizingMask = [.width]
        textContainer?.widthTracksTextView = true
        setAccessibilityIdentifier("query.editor")
        setAccessibilityLabel("SQL editor")
        setAccessibilityHelp("Command-Return runs the selection or current statement. Escape cancels active work.")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func highlightSyntax() {
        guard !hasMarkedText(), let layoutManager else { return }
        let range = NSRange(location: 0, length: string.utf16.count)
        layoutManager.removeTemporaryAttribute(.foregroundColor, forCharacterRange: range)
        let dark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        for token in SQLSyntax.tokens(in: string) {
            let color: NSColor
            switch token.kind {
            case .keyword: color = dark ? .systemPurple : NSColor(calibratedRed: 0.5, green: 0.12, blue: 0.58, alpha: 1)
            case .string: color = dark ? .systemGreen : NSColor(calibratedRed: 0.08, green: 0.38, blue: 0.18, alpha: 1)
            case .number: color = dark ? .systemBlue : NSColor(calibratedRed: 0.1, green: 0.28, blue: 0.62, alpha: 1)
            case .comment: color = .secondaryLabelColor
            case .identifier: color = dark ? .systemTeal : NSColor(calibratedRed: 0.05, green: 0.37, blue: 0.43, alpha: 1)
            case .function: color = dark ? .systemOrange : NSColor(calibratedRed: 0.53, green: 0.28, blue: 0.02, alpha: 1)
            }
            layoutManager.addTemporaryAttribute(.foregroundColor, value: color, forCharacterRange: token.range)
        }
        needsDisplay = true
    }

    override func didChangeText() { super.didChangeText(); highlightSyntax() }
    override func unmarkText() { super.unmarkText(); highlightSyntax() }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); highlightSyntax() }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard string.isEmpty else { return }
        let origin = textContainerOrigin
        ("Write a SQL query..." as NSString).draw(at: origin, withAttributes: [
            .font: font ?? NSFont.monospacedSystemFont(ofSize: 14, weight: .regular),
            .foregroundColor: NSColor.placeholderTextColor,
        ])
    }

    @objc func undo(_ sender: Any?) { if isEditable && documentUndoManager.canUndo { documentUndoManager.undo() } }
    @objc func redo(_ sender: Any?) { if isEditable && documentUndoManager.canRedo { documentUndoManager.redo() } }
    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(undo(_:)) { return isEditable && documentUndoManager.canUndo }
        if item.action == #selector(redo(_:)) { return isEditable && documentUndoManager.canRedo }
        return super.validateUserInterfaceItem(item)
    }
    override func validateMenuItem(_ item: NSMenuItem) -> Bool {
        if item.action == #selector(undo(_:)) { return isEditable && documentUndoManager.canUndo }
        if item.action == #selector(redo(_:)) { return isEditable && documentUndoManager.canRedo }
        return super.validateMenuItem(item)
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if handle(event) { return true }
        return super.performKeyEquivalent(with: event)
    }
    override func keyDown(with event: NSEvent) {
        if !handle(event) { super.keyDown(with: event) }
    }
    private func handle(_ event: NSEvent) -> Bool {
        let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
        let submit = (event.keyCode == 36 || event.keyCode == 76) && (modifiers == .command || modifiers == [.command, .shift])
        if submit && event.isARepeat { return true }
        guard let action = EditorShortcuts.intent(keyCode: event.keyCode, modifiers: modifiers, isRepeat: event.isARepeat, composing: hasMarkedText()) else { return false }
        return intent?(action) ?? false
    }
    override func resignFirstResponder() -> Bool {
        let result = super.resignFirstResponder()
        if result { _ = intent?(.reset) }
        return result
    }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        focusIfNeeded()
        NotificationCenter.default.removeObserver(self, name: NSWindow.didResignKeyNotification, object: nil)
        if let window { NotificationCenter.default.addObserver(self, selector: #selector(windowResigned), name: NSWindow.didResignKeyNotification, object: window) }
    }
    @objc private func windowResigned() { _ = intent?(.reset) }
}

@MainActor
final class EditorScrollView: NSScrollView {
    override func tile() {
        super.tile()
        guard let editor = documentView as? NSTextView else { return }
        editor.minSize = contentSize
        editor.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        editor.setFrameSize(NSSize(width: contentSize.width, height: max(contentSize.height, editor.frame.height)))
    }
}
