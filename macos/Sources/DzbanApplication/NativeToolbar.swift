import AppKit
import SwiftUI

struct NativeToolbarConfiguration {
    let history: AnyView
    let plan: AnyView
    let newQuery: AnyView
    let primary: AnyView
    let placeholder: String
    let busy: Bool
    let shortcuts: () -> Void
    let search: (String) -> Void
    let move: (Int) -> Void
    let submit: () -> Void
    let dismiss: () -> Void
}

@MainActor
final class NativeToolbarController: NSObject, NSToolbarDelegate, NSSearchFieldDelegate {
    private static let history = NSToolbarItem.Identifier("dzban.history")
    private static let actions = NSToolbarItem.Identifier("dzban.actions")
    private static let plan = NSToolbarItem.Identifier("dzban.plan")
    private static let newQuery = NSToolbarItem.Identifier("dzban.newQuery")
    private static let primary = NSToolbarItem.Identifier("dzban.primary")
    private var configuration: NativeToolbarConfiguration
    private weak var window: NSWindow?
    private var toolbar: NSToolbar?
    private var hosts: [NSToolbarItem.Identifier: NSHostingView<AnyView>] = [:]
    private let field = NSSearchField(frame: NSRect(x: 0, y: 0, width: 300, height: 28))
    private let progress = NSProgressIndicator()
    private var monitor: Any?
    private struct EventResult: @unchecked Sendable { let event: NSEvent? }

    init(configuration: NativeToolbarConfiguration) {
        self.configuration = configuration
        super.init()
        field.delegate = self
        field.identifier = NSUserInterfaceItemIdentifier("actions.search")
        field.setAccessibilityIdentifier("actions.search")
        field.setAccessibilityLabel("Search actions, connections, and queries")
        field.sendsSearchStringImmediately = true
        progress.style = .spinning
        progress.controlSize = .small
        progress.setAccessibilityLabel("Query in progress")
    }

    func attach(_ window: NSWindow?) {
        detach()
        guard let window else { return }
        self.window = window
        let toolbar = NSToolbar(identifier: "DzbanNativeToolbar")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        toolbar.allowsUserCustomization = false
        toolbar.centeredItemIdentifiers = [Self.actions]
        self.toolbar = toolbar
        window.toolbar = toolbar
        window.toolbarStyle = .unified
        window.titleVisibility = .hidden
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            MainActor.assumeIsolated {
                guard let self, event.window == self.window else { return EventResult(event: event) }
                if event.keyCode == 122 { self.configuration.shortcuts(); return EventResult(event: nil) }
                guard event.charactersIgnoringModifiers?.lowercased() == "k",
                      event.modifierFlags.intersection([.command, .shift, .control, .option]) == [.command, .shift] else { return EventResult(event: event) }
                self.window?.makeFirstResponder(self.field)
                self.configuration.search(self.field.stringValue)
                return EventResult(event: nil)
            }.event
        }
        update(configuration)
    }

    func update(_ configuration: NativeToolbarConfiguration) {
        self.configuration = configuration
        for (identifier, view) in [(Self.history, configuration.history), (Self.plan, configuration.plan), (Self.newQuery, configuration.newQuery), (Self.primary, configuration.primary)] {
            hosts[identifier]?.rootView = view
        }
        window?.titleVisibility = .hidden
        field.placeholderString = configuration.placeholder
        field.toolTip = configuration.placeholder + "\nCommand-Shift-K: search actions, connections, and query tabs"
        progress.isHidden = !configuration.busy
        if configuration.busy { progress.startAnimation(nil) } else { progress.stopAnimation(nil) }
    }

    func detach() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        if window?.toolbar === toolbar { window?.toolbar = nil }
        toolbar?.delegate = nil
        toolbar = nil
        hosts = [:]
        window = nil
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [Self.history, .flexibleSpace, Self.actions, .flexibleSpace, Self.plan, Self.newQuery, Self.primary]
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar)
    }

    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier identifier: NSToolbarItem.Identifier, willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: identifier)
        if identifier == Self.actions {
            let stack = NSStackView(views: [field, progress])
            stack.orientation = .horizontal
            stack.spacing = 6
            field.widthAnchor.constraint(equalToConstant: 300).isActive = true
            item.view = stack
            item.label = "Actions"
        } else {
            let content: AnyView
            switch identifier {
            case Self.history: content = configuration.history; item.label = "Query History"; item.isNavigational = true
            case Self.plan: content = configuration.plan; item.label = "Preview Plan"
            case Self.newQuery: content = configuration.newQuery; item.label = "New Query"
            case Self.primary: content = configuration.primary; item.label = "Run or Stop"
            default: return nil
            }
            let host = NSHostingView(rootView: content)
            hosts[identifier] = host
            item.view = host
        }
        return item
    }

    func controlTextDidBeginEditing(_ notification: Notification) { configuration.search(field.stringValue) }
    func controlTextDidChange(_ notification: Notification) { configuration.search(field.stringValue) }
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            field.stringValue = ""
            configuration.submit()
        case #selector(NSResponder.moveDown(_:)): configuration.move(1)
        case #selector(NSResponder.moveUp(_:)): configuration.move(-1)
        case #selector(NSResponder.cancelOperation(_:)):
            configuration.dismiss()
            window?.makeFirstResponder(nil)
        default: return false
        }
        return true
    }
}
