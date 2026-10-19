import AppKit
import SwiftUI

@MainActor
enum VimNavigation {
    static func remap(_ event: NSEvent, responder: NSResponder?) -> NSEvent {
        guard event.type == .keyDown || event.type == .keyUp,
              event.modifierFlags.intersection([.command, .control, .option, .shift]).isEmpty,
              !(responder is NSText), !(responder is NSTextField) else { return event }
        let key: (UInt16, String)
        switch event.charactersIgnoringModifiers {
        case "h": key = (123, "\u{f702}")
        case "j": key = (125, "\u{f701}")
        case "k": key = (126, "\u{f700}")
        case "l": key = (124, "\u{f703}")
        default: return event
        }
        return NSEvent.keyEvent(with: event.type, location: event.locationInWindow,
            modifierFlags: event.modifierFlags.union([.numericPad, .function]),
            timestamp: event.timestamp, windowNumber: event.windowNumber, context: nil,
            characters: key.1, charactersIgnoringModifiers: key.1,
            isARepeat: event.isARepeat, keyCode: key.0) ?? event
    }
}

@MainActor
final class WorkspaceFocusCycle {
    private struct Participant {
        weak var view: NSView?
        let included: Bool
        let focus: () -> Void
    }
    private var participants: [Participant] = []

    func update(_ view: NSView, included: Bool, focus: @escaping () -> Void) {
        participants.removeAll { $0.view == nil }
        let participant = Participant(view: view, included: included, focus: focus)
        if let index = participants.firstIndex(where: { $0.view === view }) {
            participants[index] = participant
        } else {
            participants.append(participant)
        }
    }

    func remove(_ view: NSView) { participants.removeAll { $0.view == nil || $0.view === view } }

    func advance(in window: NSWindow) {
        let available = participants.filter {
            guard let view = $0.view else { return false }
            return $0.included && view.window === window && !view.isHiddenOrHasHiddenAncestor && !view.visibleRect.isEmpty
        }
        guard available.count > 1 else { return }
        let current = available.firstIndex {
            guard let view = $0.view, let responder = window.firstResponder as? NSView else { return false }
            return responder === view || responder.isDescendant(of: view)
        }
        available[current.map { ($0 + 1) % available.count } ?? 0].focus()
    }
}

extension ApplicationStore {
    var canToggleQueryResultsFocus: Bool {
        document != nil && navigationAvailable && !connectionsVisible && !shortcutsVisible
            && rowInspector.address == nil && NSApp.keyWindow?.attachedSheet == nil
    }

    func toggleQueryResultsFocus() {
        guard canToggleQueryResultsFocus, let window = NSApp.keyWindow else { return }
        focusCycle.advance(in: window)
    }
}

struct WindowPolicy: NSViewRepresentable {
    let store: ApplicationStore
    let toolbar: NativeToolbarConfiguration

    func makeCoordinator() -> Coordinator { Coordinator(store: store, toolbar: toolbar) }
    func makeNSView(context: Context) -> AttachmentView {
        #if DZBAN_TEST_FIXTURES
        ViewUpdateDiagnostics.frames.append("WindowPolicy.swift")
        defer { ViewUpdateDiagnostics.frames.removeLast() }
        #endif
        let view = AttachmentView()
        view.coordinator = context.coordinator
        return view
    }
    func updateNSView(_ view: AttachmentView, context: Context) {
        #if DZBAN_TEST_FIXTURES
        ViewUpdateDiagnostics.frames.append("WindowPolicy.swift")
        defer { ViewUpdateDiagnostics.frames.removeLast() }
        #endif
        view.window?.delegate = context.coordinator
        context.coordinator.toolbar.update(toolbar)
    }

    static func dismantleNSView(_ view: AttachmentView, coordinator: Coordinator) { coordinator.detach() }

    final class AttachmentView: NSView {
        weak var coordinator: Coordinator?
        override func viewDidMoveToWindow() {
            #if DZBAN_TEST_FIXTURES
            ViewUpdateDiagnostics.frames.append("window attachment")
            defer { ViewUpdateDiagnostics.frames.removeLast() }
            #endif
            super.viewDidMoveToWindow()
            window?.delegate = coordinator
            coordinator?.attach(window)
        }
    }

    @MainActor
    final class Coordinator: NSObject, NSWindowDelegate {
        weak var store: ApplicationStore?
        let toolbar: NativeToolbarController
        private var closing = false
        private struct EventResult: @unchecked Sendable { let event: NSEvent? }
        private weak var window: NSWindow?
        private var monitor: Any?
        private var primaryHeld = false
        private var previewHeld = false
        private var attentionRequest: Int?
        #if DZBAN_TEST_FIXTURES
        private var attentionRequests = 0
        #endif

        func attach(_ window: NSWindow?) {
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
            self.window = window
            toolbar.attach(window)
            NotificationCenter.default.removeObserver(self)
            guard window != nil else { return }
            NotificationCenter.default.addObserver(self, selector: #selector(applicationResigned), name: NSApplication.didResignActiveNotification, object: nil)
            for name in [NSApplication.didBecomeActiveNotification, NSApplication.didResignActiveNotification,
                         NSApplication.didHideNotification, NSApplication.didUnhideNotification,
                         NSWindow.didBecomeKeyNotification, NSWindow.didMiniaturizeNotification, NSWindow.didDeminiaturizeNotification] {
                NotificationCenter.default.addObserver(self, selector: #selector(attentionVisibilityChanged), name: name, object: nil)
            }
            store?.attentionChanged = { [weak self] update in self?.updateAttention(update) }
            attentionVisibilityChanged()
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp, .flagsChanged]) { [weak self] event in
                MainActor.assumeIsolated {
                    guard let self else { return EventResult(event: event) }
                    let navigation = VimNavigation.remap(event, responder: event.window?.firstResponder ?? NSApp.keyWindow?.firstResponder)
                    return EventResult(event: self.handle(navigation))
                }.event
            }
        }
        func handle(_ event: NSEvent) -> NSEvent? {
            guard let store else { return event }
            let at = UInt64(max(0, event.timestamp * 1000))
            let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
            if event.type == .keyDown, event.window == window, window?.attachedSheet == nil,
               modifiers == [.command, .shift],
               event.charactersIgnoringModifiers == "?" || event.charactersIgnoringModifiers == "/" {
                if !event.isARepeat { store.shortcutsVisible = true }
                return nil
            }
            if event.type == .keyDown, event.window == window, window?.attachedSheet == nil,
               event.keyCode == 47, modifiers == .command { return nil }
            if event.type == .keyDown, event.window == window, window?.attachedSheet == nil,
               event.keyCode == 48, modifiers == .control || modifiers == [.control, .shift],
               !store.shortcutsVisible {
                if !event.isARepeat { Task { await store.cycleWorkspace(modifiers.contains(.shift) ? -1 : 1) } }
                return nil
            }
            let editor = window?.firstResponder as? NSTextView
            let allowed = store.document != nil && window?.attachedSheet == nil && editor?.isFieldEditor != true && editor?.hasMarkedText() != true && !modifiers.contains(.option)
            if event.window == window || (event.type == .flagsChanged && NSApp.keyWindow == window) {
                syncModifiers(modifiers, allowed: allowed, at: at)
            }
            if event.type == .flagsChanged { return event }
            if event.type == .keyDown, event.window == window, event.keyCode == 49,
               modifiers == [.command, .shift], window?.attachedSheet == nil {
                if !event.isARepeat, store.canPreview { Task { await store.openCurrentPlan() } }
                return nil
            }
            if event.type == .keyDown, event.window == window, event.keyCode == 40,
               modifiers == .command, store.canToggleQueryResultsFocus {
                if !event.isARepeat { store.toggleQueryResultsFocus() }
                return nil
            }
            if event.type == .keyDown && event.window == window && allowed {
                if [36, 76].contains(event.keyCode) && primaryHeld {
                    if !event.isARepeat {
                        store.sendSharedKey(.submit, pressed: true, at: at)
                        store.sendSharedKey(.submit, pressed: false, at: at)
                    }
                    return nil
                }
                if event.keyCode == 53 && modifiers.isEmpty {
                    if store.rowInspector.address != nil { store.rowInspector.backOrClose(); return nil }
                    if store.historyVisible && store.historyFocused { return event }
                    if store.controls?.primaryAction == .stop {
                        store.sendSharedKey(.cancel, pressed: true, at: at)
                        store.sendSharedKey(.cancel, pressed: false, at: at)
                        return nil
                    }
                    if store.plan.loading || store.plan.visible || store.sharedInput?.hint.kind == .estimating {
                        store.plan.close()
                        return nil
                    }
                }
            }
            if event.type == .keyDown, event.window == window, window?.attachedSheet == nil,
               event.keyCode == 13, event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command,
               store.navigationAvailable, !store.shortcutsVisible {
                if !event.isARepeat { Task { await store.closeCurrentTab() } }
                return nil
            }
            return event
        }
        private func syncModifiers(_ modifiers: NSEvent.ModifierFlags, allowed: Bool, at: UInt64) {
            let primary = allowed && EditorShortcuts.queryPrimary(modifiers)
            let preview = allowed && modifiers.contains(.shift)
            if primary != primaryHeld { primaryHeld = primary; store?.sendSharedKey(.primary, pressed: primary, at: at) }
            if preview != previewHeld { previewHeld = preview; store?.sendSharedKey(.preview, pressed: preview, at: at) }
        }
        @objc private func applicationResigned() {
            primaryHeld = false; previewHeld = false
            store?.resetSharedKeyboard()
        }
        @objc private func attentionVisibilityChanged() {
            store?.updateApplicationVisibility(NSApp.isActive && !NSApp.isHidden && window?.isVisible == true && window?.isMiniaturized == false)
        }

        private func updateAttention(_ update: QueryAttentionUpdate) {
            NSApp.dockTile.badgeLabel = update.unreadCount == 0 ? nil : String(update.unreadCount)
            if update.cancelAttention, let request = attentionRequest {
                NSApp.cancelUserAttentionRequest(request)
                attentionRequest = nil
            }
            if update.requestAttention {
                attentionRequest = NSApp.requestUserAttention(.informationalRequest)
                #if DZBAN_TEST_FIXTURES
                attentionRequests += 1
                #endif
            }
            #if DZBAN_TEST_FIXTURES
            let arguments = ProcessInfo.processInfo.arguments
            if let index = arguments.firstIndex(of: "--metrics-output"), arguments.indices.contains(index + 1) {
                let metrics: [String: Any] = ["badge": NSApp.dockTile.badgeLabel ?? "", "requests": attentionRequests]
                if let data = try? JSONSerialization.data(withJSONObject: metrics) {
                    try? data.write(to: URL(fileURLWithPath: arguments[index + 1] + ".attention"), options: .atomic)
                }
            }
            #endif
        }

        func detach() {
            store?.attentionChanged = nil
            if let request = attentionRequest { NSApp.cancelUserAttentionRequest(request) }
            attentionRequest = nil
            toolbar.detach()
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
            NotificationCenter.default.removeObserver(self)
            window = nil
        }

        init(store: ApplicationStore, toolbar: NativeToolbarConfiguration) {
            self.store = store
            self.toolbar = NativeToolbarController(configuration: toolbar)
        }
        func windowShouldClose(_ window: NSWindow) -> Bool {
            guard let store, !store.isClosed else { return true }
            guard !closing else { return false }
            closing = true
            Task {
                defer { closing = false }
                do { try await store.shutdown(); window.performClose(nil) }
                catch {}
            }
            return false
        }
    }
}
