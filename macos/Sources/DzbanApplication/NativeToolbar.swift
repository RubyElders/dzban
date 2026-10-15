import AppKit
import SwiftUI
import QuartzCore

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
    private let progress = QueryProgressView()
    private var fieldWidth: NSLayoutConstraint?
    private var monitor: Any?
    private struct EventResult: @unchecked Sendable { let event: NSEvent? }

    init(configuration: NativeToolbarConfiguration) {
        self.configuration = configuration
        super.init()
        field.delegate = self
        field.alignment = .center
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
        resize()
        field.placeholderString = configuration.placeholder
        field.toolTip = configuration.placeholder + "\nCommand-Shift-K: search actions, connections, and query tabs"
        progress.setBusy(configuration.busy)
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
        [Self.history, .flexibleSpace, Self.actions, .flexibleSpace, Self.plan, Self.newQuery, Self.primary]
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar)
    }

    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier identifier: NSToolbarItem.Identifier, willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: identifier)
        if identifier == Self.actions {
            let control = NSView()
            field.translatesAutoresizingMaskIntoConstraints = false
            progress.translatesAutoresizingMaskIntoConstraints = false
            control.addSubview(field)
            control.addSubview(progress)
            fieldWidth = control.widthAnchor.constraint(equalToConstant: 600)
            NSLayoutConstraint.activate([
                fieldWidth!, control.heightAnchor.constraint(equalToConstant: 30),
                field.leadingAnchor.constraint(equalTo: control.leadingAnchor),
                field.trailingAnchor.constraint(equalTo: control.trailingAnchor),
                field.centerYAnchor.constraint(equalTo: control.centerYAnchor),
                progress.leadingAnchor.constraint(equalTo: control.leadingAnchor, constant: 12),
                progress.trailingAnchor.constraint(equalTo: control.trailingAnchor, constant: -12),
                progress.bottomAnchor.constraint(equalTo: field.bottomAnchor, constant: -2),
                progress.heightAnchor.constraint(equalToConstant: 2)
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

    @objc private func resize() {
        fieldWidth?.constant = min(600, max(380, (window?.frame.width ?? 1024) - 320))
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

@MainActor
private final class QueryProgressView: NSView {
    private let segment = CALayer()
    private var busy = false
    private var animationWidth: CGFloat = -1

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.masksToBounds = true
        layer?.addSublayer(segment)
        isHidden = true
        setAccessibilityElement(true)
        setAccessibilityRole(.progressIndicator)
        setAccessibilityLabel("Query in progress")
        setAccessibilityIdentifier("query.progress")
    }
    required init?(coder: NSCoder) { fatalError() }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func setBusy(_ busy: Bool) {
        guard busy != self.busy else { return }
        self.busy = busy
        isHidden = !busy
        if busy { animate() } else { segment.removeAllAnimations() }
    }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.12).cgColor
        segment.backgroundColor = NSColor.controlAccentColor.cgColor
        segment.frame = CGRect(x: 0, y: 0, width: bounds.width * 0.2, height: bounds.height)
        CATransaction.commit()
        if busy && animationWidth != bounds.width { animate() }
    }

    private func animate() {
        animationWidth = bounds.width
        segment.removeAllAnimations()
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else { return }
        let animation = CABasicAnimation(keyPath: "transform.translation.x")
        animation.fromValue = 0
        animation.toValue = bounds.width * 0.8
        animation.duration = 1.1
        animation.autoreverses = true
        animation.repeatCount = .infinity
        segment.add(animation, forKey: "pulse")
    }
}
