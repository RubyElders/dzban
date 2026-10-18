import AppKit
import SwiftUI

struct NativeToolbarConfiguration {
    let history: AnyView
    let plan: AnyView
    let primary: AnyView
    let placeholder: String
    let searchText: String
    let searchVisible: Bool
    let menuHeight: CGFloat
    let focusRequest: Int
    let search: (String) -> Void
    let move: (Int) -> Void
    let submit: () -> Void
    let dismiss: (Bool) -> Void
}

@MainActor
private final class ActionSearchField: NSSearchField {
    var openSuggestions: (() -> Void)?

    override func mouseDown(with event: NSEvent) {
        super.mouseDown(with: event)
        openSuggestions?()
    }
}

@MainActor
final class NativeToolbarController: NSObject, NSToolbarDelegate, NSSearchFieldDelegate {
    private static let history = NSToolbarItem.Identifier("dzban.history")
    private static let plan = NSToolbarItem.Identifier("dzban.plan")
    private static let primary = NSToolbarItem.Identifier("dzban.primary")
    private static let actions = NSToolbarItem.Identifier("dzban.actions")
    private var configuration: NativeToolbarConfiguration
    private weak var window: NSWindow?
    private var toolbar: NSToolbar?
    private var hosts: [NSToolbarItem.Identifier: NSHostingView<AnyView>] = [:]
    private let field = ActionSearchField(frame: NSRect(x: 0, y: 0, width: 300, height: 28))
    private var fieldWidth: NSLayoutConstraint?
    private var monitor: Any?
    private var focusRequest: Int
    private struct EventResult: @unchecked Sendable { let event: NSEvent? }

    init(configuration: NativeToolbarConfiguration) {
        self.configuration = configuration
        focusRequest = configuration.focusRequest
        super.init()
        field.delegate = self
        field.openSuggestions = { [weak self] in
            guard let self else { return }
            self.configuration.search(self.field.stringValue)
        }
        field.alignment = .left
        field.font = .systemFont(ofSize: 12)
        field.identifier = NSUserInterfaceItemIdentifier("actions.search")
        field.setAccessibilityIdentifier("actions.search")
        field.setAccessibilityLabel("Search actions, connections, and queries")
        field.sendsSearchStringImmediately = true
    }

    func attach(_ window: NSWindow?) {
        detach()
        guard let window else { return }
        self.window = window
        NotificationCenter.default.addObserver(self, selector: #selector(resize), name: NSWindow.didResizeNotification, object: window)
        let toolbar = NSToolbar(identifier: "DzbanNativeToolbar")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        toolbar.allowsUserCustomization = false
        toolbar.centeredItemIdentifiers = [Self.actions]
        self.toolbar = toolbar
        window.toolbar = toolbar
        window.toolbarStyle = .unified
        window.titleVisibility = .hidden
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] event in
            MainActor.assumeIsolated {
                guard let self, event.window == self.window, self.configuration.searchVisible,
                      let window = self.window else { return EventResult(event: event) }
                let bounds = self.field.convert(self.field.bounds, to: nil)
                let height = min(self.configuration.menuHeight, window.contentLayoutRect.height - 16)
                let menu = NSRect(x: bounds.minX, y: window.contentLayoutRect.maxY - 8 - height, width: bounds.width, height: height)
                if !bounds.contains(event.locationInWindow) && !menu.contains(event.locationInWindow) {
                    self.configuration.dismiss(false)
                }
                return EventResult(event: event)
            }.event
        }
        update(configuration)
    }

    func update(_ configuration: NativeToolbarConfiguration) {
        self.configuration = configuration
        if field.stringValue != configuration.searchText { field.stringValue = configuration.searchText }
        if focusRequest != configuration.focusRequest {
            focusRequest = configuration.focusRequest
            DispatchQueue.main.async { [weak self] in self?.focusSearch() }
        }
        for (identifier, view) in [(Self.history, configuration.history), (Self.plan, configuration.plan), (Self.primary, configuration.primary)] {
            hosts[identifier]?.rootView = AnyView(view.frame(width: 28, height: 28))
        }
        window?.titleVisibility = .hidden
        resize()
        field.placeholderString = configuration.placeholder
        field.toolTip = configuration.placeholder + "\n⌘L: search actions, connections, and query tabs"
    }

    func detach() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        NotificationCenter.default.removeObserver(self, name: NSWindow.didResizeNotification, object: window)
        fieldWidth?.isActive = false
        fieldWidth = nil
        if window?.toolbar === toolbar { window?.toolbar = nil }
        toolbar?.delegate = nil
        toolbar = nil
        hosts = [:]
        window = nil
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [Self.history, .flexibleSpace, Self.actions, .flexibleSpace, Self.plan, Self.primary]
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar)
    }

    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier identifier: NSToolbarItem.Identifier, willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: identifier)
        if identifier == Self.actions {
            let control = NSView()
            field.translatesAutoresizingMaskIntoConstraints = false
            control.addSubview(field)
            fieldWidth = control.widthAnchor.constraint(equalToConstant: 600)
            NSLayoutConstraint.activate([
                fieldWidth!, control.heightAnchor.constraint(equalToConstant: 30),
                field.leadingAnchor.constraint(equalTo: control.leadingAnchor),
                field.trailingAnchor.constraint(equalTo: control.trailingAnchor),
                field.centerYAnchor.constraint(equalTo: control.centerYAnchor)
            ])
            item.visibilityPriority = .user
            item.view = control
            resize()
            item.label = "Actions"
        } else {
            let content: AnyView
            switch identifier {
            case Self.history: content = configuration.history; item.label = "Query History"
            case Self.plan: content = configuration.plan; item.label = "Preview Plan"
            case Self.primary: content = configuration.primary; item.label = "Run or Stop Query"
            default: return nil
            }
            let host = NSHostingView(rootView: AnyView(content.frame(width: 28, height: 28)))
            hosts[identifier] = host
            item.view = host
        }
        return item
    }

    @objc private func resize() {
        fieldWidth?.constant = min(600, max(380, (window?.frame.width ?? 1024) - 320))
    }

    private func focusSearch() {
        guard window?.attachedSheet == nil else { return }
        window?.makeFirstResponder(field)
        field.selectText(nil)
        configuration.search(field.stringValue)
    }

    func controlTextDidBeginEditing(_ notification: Notification) { configuration.search(field.stringValue) }
    func controlTextDidChange(_ notification: Notification) { configuration.search(field.stringValue) }
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            if NSApp.currentEvent?.isARepeat != true { configuration.submit() }
        case #selector(NSResponder.moveDown(_:)): configuration.move(1)
        case #selector(NSResponder.moveUp(_:)): configuration.move(-1)
        case #selector(NSResponder.cancelOperation(_:)):
            configuration.dismiss(true)
        default: return false
        }
        return true
    }
}
