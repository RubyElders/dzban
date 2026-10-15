import AppKit
import SwiftUI

struct WindowPolicy: NSViewRepresentable {
    let store: ApplicationStore
    let toolbar: NativeToolbarConfiguration

    func makeCoordinator() -> Coordinator { Coordinator(store: store, toolbar: toolbar) }
    func makeNSView(context: Context) -> AttachmentView {
        let view = AttachmentView()
        view.coordinator = context.coordinator
        return view
    }
    func updateNSView(_ view: AttachmentView, context: Context) {
        view.window?.delegate = context.coordinator
        context.coordinator.toolbar.update(toolbar)
    }

    static func dismantleNSView(_ view: AttachmentView, coordinator: Coordinator) { coordinator.detach() }

    final class AttachmentView: NSView {
        weak var coordinator: Coordinator?
        override func viewDidMoveToWindow() { super.viewDidMoveToWindow(); window?.delegate = coordinator; coordinator?.attach(window) }
    }

    @MainActor
    final class Coordinator: NSObject, NSWindowDelegate {
        weak var store: ApplicationStore?
        let toolbar: NativeToolbarController
        private var closing = false
        private struct EventResult: @unchecked Sendable { let event: NSEvent? }
        private weak var window: NSWindow?
        private var monitor: Any?
        private var previewPressed: TimeInterval?
        private var previewTask: Task<Void, Never>?
        private var previewEpoch: UInt64 = 0
        private var primaryHeld = false
        private var previewHeld = false
        private var inspectHeld = false
        private var submitHeld = false

        func attach(_ window: NSWindow?) {
            if let monitor { NSEvent.removeMonitor(monitor) }
            self.window = window
            toolbar.attach(window)
            NotificationCenter.default.removeObserver(self)
            guard window != nil else { return }
            NotificationCenter.default.addObserver(self, selector: #selector(applicationResigned), name: NSApplication.didResignActiveNotification, object: nil)
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp, .flagsChanged]) { [weak self] event in
                MainActor.assumeIsolated {
                    guard let self else { return EventResult(event: event) }
                    return EventResult(event: self.handle(event))
                }.event
            }
        }
        private func handle(_ event: NSEvent) -> NSEvent? {
            guard let store else { return event }
            let at = UInt64(max(0, event.timestamp * 1000))
            if event.type == .keyUp && event.keyCode == 49 && inspectHeld {
                inspectHeld = false
                store.sendSharedKey(.inspect, pressed: false, at: at)
                return nil
            }
            if event.type == .keyUp && [36, 76].contains(event.keyCode) && submitHeld {
                submitHeld = false
                store.sendSharedKey(.submit, pressed: false, at: at)
                return nil
            }
            let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
            let editor = window?.firstResponder as? NSTextView
            let allowed = store.document != nil && window?.attachedSheet == nil && editor?.isFieldEditor != true && editor?.hasMarkedText() != true && !modifiers.contains(.option)
            if event.window == window || (event.type == .flagsChanged && NSApp.keyWindow == window) {
                syncModifiers(modifiers, allowed: allowed, at: at)
            }
            if event.type == .flagsChanged { return event }
            if event.type == .keyDown && event.window == window && allowed {
                if [36, 76].contains(event.keyCode) && primaryHeld {
                    if !event.isARepeat { submitHeld = true; store.sendSharedKey(.submit, pressed: true, at: at) }
                    return nil
                }
                if event.keyCode == 49 && primaryHeld && previewHeld {
                    if !event.isARepeat { inspectHeld = true; store.sendSharedKey(.inspect, pressed: true, at: at) }
                    return nil
                }
                if event.keyCode == 53 && modifiers.isEmpty {
                    if store.controls?.primaryAction == "stop" {
                        store.sendSharedKey(.cancel, pressed: true, at: at)
                        store.sendSharedKey(.cancel, pressed: false, at: at)
                        return nil
                    }
                    if store.plan.loading || store.plan.visible || store.sharedInput?.hint.kind == "estimating" {
                        store.plan.close()
                        return nil
                    }
                }
            }
            if event.type == .keyDown, event.window == window, window?.attachedSheet == nil,
               event.keyCode == 13, event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command,
               let document = store.document {
                if !event.isARepeat { Task { await store.closeDocument(document.document) } }
                return nil
            }
            guard event.keyCode == 35 else { return event }
            if event.type == .keyUp, let pressed = previewPressed {
                previewPressed = nil
                let elapsed = UInt64(max(0, event.timestamp - pressed) * 1000)
                let pending = previewTask
                let epoch = previewEpoch
                Task { [weak self] in
                    await pending?.value
                    if self?.previewEpoch == epoch { store.plan.release(milliseconds: elapsed) }
                }
                return nil
            }
            if event.type == .keyDown, previewPressed != nil { return nil }
            guard event.type == .keyDown, event.window == window,
                  event.modifierFlags.intersection([.command, .control, .option, .shift]) == [.command, .shift],
                  store.canPreview, store.document != nil,
                  (window?.firstResponder as? NSTextView)?.hasMarkedText() != true else { return event }
            if !event.isARepeat {
                previewPressed = event.timestamp
                previewEpoch &+= 1
                previewTask = Task { await store.prepareLegacyPreview(); await store.previewCurrent() }
            }
            return nil
        }
        private func syncModifiers(_ modifiers: NSEvent.ModifierFlags, allowed: Bool, at: UInt64) {
            let primary = allowed && (!modifiers.intersection([.command, .control]).isEmpty)
            let preview = allowed && modifiers.contains(.shift)
            if primary != primaryHeld { primaryHeld = primary; store?.sendSharedKey(.primary, pressed: primary, at: at) }
            if preview != previewHeld { previewHeld = preview; store?.sendSharedKey(.preview, pressed: preview, at: at) }
        }
        @objc private func applicationResigned() {
            previewEpoch &+= 1; previewTask?.cancel(); previewPressed = nil
            primaryHeld = false; previewHeld = false; inspectHeld = false; submitHeld = false
            store?.resetSharedKeyboard()
        }
        func detach() {
            toolbar.detach()
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
            NotificationCenter.default.removeObserver(self)
            window = nil
            previewPressed = nil
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
