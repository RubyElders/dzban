import AppKit
import SwiftUI

struct WindowPolicy: NSViewRepresentable {
    let store: ApplicationStore

    func makeCoordinator() -> Coordinator { Coordinator(store: store) }
    func makeNSView(context: Context) -> AttachmentView {
        let view = AttachmentView()
        view.coordinator = context.coordinator
        return view
    }
    func updateNSView(_ view: AttachmentView, context: Context) { view.window?.delegate = context.coordinator }

    final class AttachmentView: NSView {
        weak var coordinator: Coordinator?
        override func viewDidMoveToWindow() { super.viewDidMoveToWindow(); window?.delegate = coordinator }
    }

    @MainActor
    final class Coordinator: NSObject, NSWindowDelegate {
        weak var store: ApplicationStore?
        private var closing = false

        init(store: ApplicationStore) { self.store = store }
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
