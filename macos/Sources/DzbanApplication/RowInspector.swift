import AppKit
import Combine
import DzbanBridge
import SwiftUI

@MainActor
final class RowInspectorModel: ObservableObject {
    @Published private(set) var address: ResultAddress?
    @Published private(set) var values: [ViewportCell] = []
    @Published private(set) var columnStart = 0
    @Published private(set) var loading = false
    @Published private(set) var failure: String?
    @Published private(set) var pinned = false
    private let bridge: Bridge
    private let source: any ResultSource
    private var epoch: UInt64 = 0
    private var visibilityWork: Task<Void, Never>?
    private var readWork: Task<Void, Never>?
    private var session: UUID?

    init(bridge: Bridge, source: any ResultSource) { self.bridge = bridge; self.source = source }
    func show(_ address: ResultAddress, pinned: Bool, columns: Int) {
        epoch &+= 1
        let epoch = epoch
        self.session = address.scope.session
        readWork?.cancel()
        values = []
        failure = nil
        enqueue(pinned ? .open : .show, epoch: epoch, session: address.scope.session) { [weak self] visible in
            guard visible else { return }
            self?.address = address
            self?.load(start: address.column / 16 * 16, columns: columns)
        }
    }
    func release(milliseconds: UInt64) { event(.release, milliseconds: milliseconds) }
    func pin() { event(.pin) }
    func reset() { event(.reset) }
    private func event(_ event: PeekEvent, milliseconds: UInt64? = nil) {
        guard let session else { return }
        enqueue(event, epoch: epoch, session: session, milliseconds: milliseconds) { [weak self] visible in
            if !visible { self?.clear() }
        }
    }
    private func enqueue(_ event: PeekEvent, epoch: UInt64, session: UUID, milliseconds: UInt64? = nil, applied: @escaping (Bool) -> Void) {
        let previous = visibilityWork
        visibilityWork = Task { [weak self, bridge] in
            await previous?.value
            guard let self, self.epoch == epoch else { return }
            do {
                let response = try await bridge.request(.peek(session: session, surface: .row, event: event, elapsedMilliseconds: milliseconds), as: PeekVisibility.self)
                guard self.epoch == epoch else { return }
                self.pinned = response.pinned
                applied(response.visible)
            } catch { if self.epoch == epoch { self.failure = error.localizedDescription } }
        }
    }
    func load(start: Int, columns: Int) {
        guard let address, start >= 0, start < columns else { return }
        readWork?.cancel()
        columnStart = start
        loading = true
        values = []
        failure = nil
        let epoch = epoch
        readWork = Task { [weak self, source] in
            do {
                let page = try await source.page(address.scope, start: address.row, count: 1, columnStart: start, columnCount: min(16, columns - start))
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                guard page.document == address.scope.document, page.generation == address.scope.generation,
                      page.start == address.row, page.columnStart == start, page.rows.count == 1 else { throw ViewportFailure("Row belongs to retired output") }
                self.values = page.rows[0]
                self.loading = false
            } catch {
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
                self.loading = false
            }
        }
    }
    private func clear() { address = nil; values = []; readWork?.cancel(); loading = false }
    func close() {
        let session = session
        epoch &+= 1
        self.session = nil
        clear()
        pinned = false
        let previous = visibilityWork
        visibilityWork = Task { [bridge] in
            await previous?.value
            if let session { let _: PeekVisibility? = try? await bridge.request(.peek(session: session, surface: .row, event: .close), as: PeekVisibility.self) }
        }
    }
}

struct RowInspectorView: View {
    @ObservedObject var model: RowInspectorModel
    let columns: [Column]
    let inspect: (ResultAddress) -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Row \((model.address?.row ?? 0) + 1)").font(.headline)
                Spacer()
                Button(model.pinned ? "Pinned" : "Pin") { model.pin() }.disabled(model.pinned).accessibilityIdentifier("row.pin")
                Button("Close") { model.close() }.keyboardShortcut(.cancelAction).accessibilityIdentifier("row.close")
            }
            if model.loading { ProgressView("Loading row").accessibilityIdentifier("row.loading") }
            if let error = model.failure {
                Text(error).foregroundStyle(.red).accessibilityIdentifier("row.error")
                Button("Retry") { model.load(start: model.columnStart, columns: columns.count) }
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    ForEach(Array(model.values.enumerated()), id: \.offset) { offset, value in
                        let index = model.columnStart + offset
                        HStack(alignment: .top) {
                            VStack(alignment: .leading) {
                                Text(columns.indices.contains(index) ? columns[index].name : "Column \(index + 1)").font(.headline)
                                Text(value.text ?? "SQL NULL").textSelection(.enabled).lineLimit(4)
                                    .accessibilityIdentifier("row.value.\(index)")
                                if value.truncated { Text("Preview truncated").font(.caption).foregroundStyle(.secondary) }
                            }
                            Spacer()
                            Button("Inspect Cell") {
                                if let address = model.address { inspect(ResultAddress(scope: address.scope, row: address.row, column: index)) }
                            }.accessibilityIdentifier("row.inspect.\(index)")
                        }
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                Button("Previous Columns") { model.load(start: max(0, model.columnStart - 16), columns: columns.count) }.disabled(model.loading || model.columnStart == 0)
                Button("Next Columns") { model.load(start: model.columnStart + 16, columns: columns.count) }.disabled(model.loading || model.columnStart + 16 >= columns.count)
            }
        }.padding(20).frame(width: 640, height: 460)
    }
}

struct RowPopover: NSViewRepresentable {
    @ObservedObject var model: RowInspectorModel
    let columns: [Column]
    let inspect: (ResultAddress) -> Void
    func makeCoordinator() -> Coordinator { Coordinator(model: model) }
    func makeNSView(context: Context) -> NSView { NSView() }
    func updateNSView(_ view: NSView, context: Context) {
        let coordinator = context.coordinator
        guard model.address != nil else { coordinator.popover.close(); return }
        let content = RowInspectorView(model: model, columns: columns, inspect: inspect)
        if let host = coordinator.popover.contentViewController as? NSHostingController<RowInspectorView> { host.rootView = content }
        else { coordinator.popover.contentViewController = NSHostingController(rootView: content) }
        coordinator.popover.contentSize = NSSize(width: 640, height: 460)
        if !coordinator.popover.isShown, view.window != nil { coordinator.popover.show(relativeTo: view.bounds, of: view, preferredEdge: .minY) }
    }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) { coordinator.popover.close() }
    @MainActor
    final class Coordinator: NSObject, NSPopoverDelegate {
        let popover = NSPopover()
        let model: RowInspectorModel
        init(model: RowInspectorModel) {
            self.model = model
            super.init()
            popover.behavior = .applicationDefined
            popover.delegate = self
        }
        func popoverDidClose(_ notification: Notification) { if model.address != nil { model.close() } }
    }
}
