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

        func attach(_ window: NSWindow?) {
            if let monitor { NSEvent.removeMonitor(monitor) }
            self.window = window
            toolbar.attach(window)
            NotificationCenter.default.removeObserver(self)
            guard window != nil else { return }
            NotificationCenter.default.addObserver(self, selector: #selector(applicationResigned), name: NSApplication.didResignActiveNotification, object: nil)
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp]) { [weak self] event in
                MainActor.assumeIsolated {
                    guard let self else { return EventResult(event: event) }
                    return EventResult(event: self.handle(event))
                }.event
            }
        }
        private func handle(_ event: NSEvent) -> NSEvent? {
            guard let store else { return event }
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
                previewTask = Task { await store.previewCurrent() }
            }
            return nil
        }
        @objc private func applicationResigned() { previewEpoch &+= 1; previewTask?.cancel(); previewPressed = nil; store?.plan.reset() }
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
