import AppKit
import Combine
import DzbanBridge
import SwiftUI

@MainActor
final class RowInspectorModel: ObservableObject {
    @Published private(set) var address: ResultAddress?
    @Published var inspectedCell: ResultAddress?
    @Published private(set) var values: [ViewportCell] = []
    @Published private(set) var loading = false
    @Published private(set) var failure: String?
    @Published private(set) var pinned = false
    private(set) weak var anchorView: NSView?
    private(set) var anchorRect: NSRect = .zero
    private let application: ApplicationClient
    private let source: any ResultSource
    private var epoch: UInt64 = 0
    private var visibilityWork: Task<Void, Never>?
    private var readWork: Task<Void, Never>?
    private var session: UUID?

    init(application: ApplicationClient, source: any ResultSource) { self.application = application; self.source = source }
    func show(_ address: ResultAddress, pinned: Bool, columns: Int, anchorView: NSView? = nil, anchorRect: NSRect = .zero) {
        self.anchorView = anchorView
        self.anchorRect = anchorRect
        epoch &+= 1
        let epoch = epoch
        self.session = address.scope.session
        readWork?.cancel()
        values = []
        loading = false
        failure = nil
        enqueue(pinned ? .open : .show, epoch: epoch, session: address.scope.session) { [weak self] visible in
            guard visible else { return }
            self?.inspectedCell = nil
            self?.address = address
            self?.loadMore(columns: columns)
        }
    }
    @discardableResult
    func moveRow(_ direction: Int, rows: Int, columns: Int, column: Int) -> ResultAddress? {
        guard let address else { return nil }
        let position: ResultPosition
        do {
            guard let moved = try ResultViewPolicy.moveRow(ResultPosition(row: address.row, column: column), direction: direction, rows: rows, columns: columns) else { return nil }
            position = moved
        } catch { failure = error.localizedDescription; return nil }
        epoch &+= 1
        readWork?.cancel()
        values = []
        loading = false
        failure = nil
        inspectedCell = nil
        let next = ResultAddress(scope: address.scope, row: position.row, column: position.column)
        self.address = next
        pin()
        loadMore(columns: columns)
        return next
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
        visibilityWork = Task { [weak self, application] in
            await previous?.value
            guard let self, self.epoch == epoch else { return }
            do {
                let response = try await application.rowVisibility(session: session, event: event, elapsedMilliseconds: milliseconds)
                guard self.epoch == epoch else { return }
                self.pinned = response.pinned
                applied(response.visible)
            } catch { if self.epoch == epoch { self.failure = error.localizedDescription } }
        }
    }
    func loadMore(columns: Int) {
        let start = values.count
        guard let address, !loading, start < columns else { return }
        readWork?.cancel()
        loading = true
        failure = nil
        let epoch = epoch
        readWork = Task { [weak self, source] in
            do {
                let page = try await source.page(address.scope, start: address.row, count: 1, columnStart: start, columnCount: min(16, columns - start))
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                guard page.document == address.scope.document, page.generation == address.scope.generation,
                      page.start == address.row, page.columnStart == start, page.rows.count == 1 else { throw ViewportFailure("Row belongs to retired output") }
                self.values.append(contentsOf: page.rows[0])
                self.loading = false
            } catch {
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
                self.loading = false
            }
        }
    }
    func backOrClose() {
        if inspectedCell != nil { inspectedCell = nil } else { close() }
    }
    private func clear() { anchorView = nil; anchorRect = .zero; inspectedCell = nil; address = nil; values = []; readWork?.cancel(); loading = false }
    func close() {
        let session = session
        epoch &+= 1
        self.session = nil
        clear()
        pinned = false
        let previous = visibilityWork
        visibilityWork = Task { [application] in
            await previous?.value
            if let session { _ = try? await application.rowVisibility(session: session, event: .close) }
        }
    }
}

struct RowInspectorView: View {
    @ObservedObject var model: RowInspectorModel
    let columns: [Column]
    let cell: CellInspectorModel
    let rows: Int
    let selectRow: (ResultAddress) -> Void
    @State private var positionedColumn: Int?
    @State private var selectedColumn: Int?
    @FocusState private var focusedColumn: Int?

    var body: some View {
        Group {
            if let address = model.inspectedCell {
                CellInspectorView(model: cell, address: address, back: { model.inspectedCell = nil }, close: { model.close() })
            } else {
                rowContents
            }
        }
        .frame(width: 640, height: 460)
        .onChange(of: model.address) { old, new in
            positionedColumn = nil
            if old?.scope != new?.scope { selectedColumn = nil }
        }
        .onChange(of: model.inspectedCell) { _, address in if address == nil { positionedColumn = nil } }
    }

    private var rowContents: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Row \((model.address?.row ?? 0) + 1)").font(.headline).accessibilityIdentifier("row.title")
                Spacer()
                Button("Close") { model.close() }.accessibilityIdentifier("row.close")
            }
            if let error = model.failure {
                Text(error).foregroundStyle(.red).accessibilityIdentifier("row.error")
                Button("Retry") { model.loadMore(columns: columns.count) }
            }
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 12) {
                        ForEach(Array(model.values.enumerated()), id: \.offset) { offset, value in
                            let index = offset
                            HStack(alignment: .top) {
                                VStack(alignment: .leading) {
                                    Text(columns.indices.contains(index) ? columns[index].name : "Column \(index + 1)").font(.headline)
                                    Text(value.text ?? "SQL NULL").lineLimit(4)
                                        .accessibilityIdentifier("row.value.\(index)")
                                    if value.truncated { Text("Preview truncated").font(.caption).foregroundStyle(.secondary) }
                                }
                                Spacer()
                                Button("Inspect Cell") {
                                    select(index)
                                    openSelectedCell()
                                }.focusable(false).accessibilityIdentifier("row.inspect.\(index)")
                            }
                            .padding(8).frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                            .onTapGesture { select(index) }
                            .focusable().focusEffectDisabled()
                            .focused($focusedColumn, equals: index)
                            .onKeyPress(.upArrow) { move(-1); return .handled }
                            .onKeyPress(.downArrow) { move(1); return .handled }
                            .onKeyPress(.leftArrow) { Task { @MainActor in moveRow(-1) }; return .handled }
                            .onKeyPress(.rightArrow) { Task { @MainActor in moveRow(1) }; return .handled }
                            .onKeyPress(.return) { Task { @MainActor in openSelectedCell() }; return .handled }
                            .onKeyPress(.space, phases: .down) { _ in Task { @MainActor in openSelectedCell() }; return .handled }
                            .background(selectedColumn == index ? Color.accentColor.opacity(0.12) : Color.clear, in: RoundedRectangle(cornerRadius: 4))
                            .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(selectedColumn == index ? Color.accentColor : Color.clear, lineWidth: 1.5))
                            .accessibilityElement(children: .contain)
                            .accessibilityIdentifier("row.column.\(index)")
                            .accessibilityAddTraits(selectedColumn == index ? .isSelected : [])
                            .id(index)
                            .onAppear { if index == model.values.count - 1 { model.loadMore(columns: columns.count) } }
                        }
                    }.padding(.trailing, 16).frame(maxWidth: .infinity, alignment: .leading)
                }
                .opacity(positionedColumn == nil ? 0 : 1)
                .overlay {
                    if positionedColumn == nil && model.failure == nil {
                        ProgressView("Loading row").accessibilityIdentifier("row.loading")
                    }
                }
                .accessibilityIdentifier("row.columns")
                .task(id: model.values.count) {
                    guard let address = model.address, !columns.isEmpty else { return }
                    let target = selectedColumn ?? min(address.column, columns.count - 1)
                    if selectedColumn == nil { selectedColumn = target }
                    guard positionedColumn != target else { return }
                    guard model.values.count >= min(columns.count, target + 8) else {
                        model.loadMore(columns: columns.count)
                        return
                    }
                    await Task.yield()
                    guard !Task.isCancelled else { return }
                    proxy.scrollTo(target, anchor: .center)
                    positionedColumn = target
                    focusedColumn = target
                }
                .onChange(of: focusedColumn) { _, column in
                    if let column {
                        positionedColumn = column
                        selectedColumn = column
                    }
                }
                .onChange(of: selectedColumn) { _, column in
                    guard positionedColumn != nil else { return }
                    if let column, column < model.values.count {
                        if positionedColumn != column {
                            focusedColumn = column
                            proxy.scrollTo(column, anchor: .center)
                            positionedColumn = column
                        }
                    } else { model.loadMore(columns: columns.count) }
                }
            }
        }.padding(20).frame(width: 640, height: 460)
    }

    private func select(_ column: Int) {
        selectedColumn = column
        focusedColumn = column
    }

    private func move(_ direction: Int) {
        guard !columns.isEmpty else { return }
        selectedColumn = min(max(0, (selectedColumn ?? 0) + direction), columns.count - 1)
    }

    private func moveRow(_ direction: Int) {
        guard let column = selectedColumn ?? model.address?.column else { return }
        if let address = model.moveRow(direction, rows: rows, columns: columns.count, column: column) {
            selectRow(address)
        }
    }

    private func openSelectedCell() {
        guard let address = model.address else { return }
        let column = selectedColumn ?? address.column
        guard columns.indices.contains(column) else { return }
        model.pin()
        model.inspectedCell = ResultAddress(scope: address.scope, row: address.row, column: column)
    }

}

struct RowPopover: NSViewRepresentable {
    @ObservedObject var model: RowInspectorModel
    let columns: [Column]
    let cell: CellInspectorModel
    let rows: Int
    let selectRow: (ResultAddress) -> Void
    func makeCoordinator() -> Coordinator { Coordinator(model: model) }
    func makeNSView(context: Context) -> NSView { NSView() }
    func updateNSView(_ view: NSView, context: Context) {
        #if DZBAN_TEST_FIXTURES
        ViewUpdateDiagnostics.frames.append("RowInspector.swift")
        defer { ViewUpdateDiagnostics.frames.removeLast() }
        #endif
        let coordinator = context.coordinator
        coordinator.updateRevision &+= 1
        let revision = coordinator.updateRevision
        DispatchQueue.main.async { [weak view, weak coordinator] in
            guard let view, let coordinator, coordinator.updateRevision == revision else { return }
            guard model.address != nil else { coordinator.popover.close(); return }
            if coordinator.popover.contentViewController == nil || coordinator.columns != columns || coordinator.rows != rows {
                let content = RowInspectorView(model: model, columns: columns, cell: cell, rows: rows, selectRow: selectRow)
                if let host = coordinator.popover.contentViewController as? NSHostingController<RowInspectorView> { host.rootView = content }
                else { coordinator.popover.contentViewController = NSHostingController(rootView: content) }
                coordinator.columns = columns
                coordinator.rows = rows
                coordinator.popover.contentSize = NSSize(width: 640, height: 460)
            }
            let anchor = view
            let rect = model.anchorView.map { view.convert(model.anchorRect, from: $0) } ?? view.bounds
            if !coordinator.popover.isShown, anchor.window != nil {
                coordinator.returnWindow = anchor.window
                coordinator.returnResponder = anchor.window?.firstResponder
                coordinator.popover.show(relativeTo: rect, of: anchor, preferredEdge: .minY)
            } else if coordinator.popover.positioningRect != rect { coordinator.popover.positioningRect = rect }
        }
    }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) { coordinator.detach(); coordinator.popover.close() }
    @MainActor
    final class Coordinator: NSObject, NSPopoverDelegate {
        let popover = NSPopover()
        let model: RowInspectorModel
        weak var returnWindow: NSWindow?
        weak var returnResponder: NSResponder?
        var columns: [Column] = []
        var rows = 0
        var updateRevision: UInt64 = 0
        private var monitor: Any?
        private struct EventResult: @unchecked Sendable { let event: NSEvent? }
        init(model: RowInspectorModel) {
            self.model = model
            super.init()
            popover.behavior = .applicationDefined
            popover.animates = false
            popover.delegate = self
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                MainActor.assumeIsolated {
                    guard let self, self.popover.isShown, self.model.address != nil, event.keyCode == 53 else { return EventResult(event: event) }
                    if !event.isARepeat { self.model.backOrClose() }
                    return EventResult(event: nil)
                }.event
            }
        }
        func detach() {
            updateRevision &+= 1
            returnResponder = nil
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
        }
        func popoverDidClose(_ notification: Notification) {
            if model.address != nil { model.close() }
            if let window = returnWindow, let responder = returnResponder, NSApp.isActive, window.isVisible,
               (responder as? NSView)?.window == window {
                window.makeKey()
                window.makeFirstResponder(responder)
            }
            returnResponder = nil
        }
    }
}
