import AppKit
import DzbanBridge
import SwiftUI

public enum ResultStatus {
    public static func describe(_ document: DocumentSnapshot) -> String {
        if document.history == true {
            if document.termination == "failed" { return "Saved query failed" }
            if let command = document.commandResult { return command.affectedRows.map { "Saved command - \($0) rows affected" } ?? "Saved command" }
            return "Saved - \(document.rows) of \(document.historyTotalRows ?? document.rows) rows"
        }
        if document.generation == 0 { return "Run a query to see results" }
        if document.execution == "canceling" { return "Canceling - \(document.rows) rows captured" }
        switch document.resultPhase {
        case "waiting_for_schema": return "Waiting for schema"
        case "receiving": return "Receiving - \(document.rows) rows"
        case "draining": return "Truncated - \(document.rows) rows; waiting for cleanup"
        case "failed_awaiting_cleanup": return "Result capture failed; waiting for cleanup"
        case "finished":
            if document.execution != "finished" { return "Finishing - \(document.rows) rows" }
            if document.termination == "limited" || document.cutoff != nil { return "Truncated - \(document.rows) rows (\(document.cutoff == .string("Bytes") ? "byte" : "row") limit)" }
            if document.termination == "canceled" { return "Canceled - \(document.rows) rows" }
            if document.termination == "failed" { return "Failed - \(document.rows) rows" }
            if let command = document.commandResult { return command.affectedRows.map { "\($0) rows affected" } ?? "Command completed" }
            return "Complete - \(document.rows) rows"
        default: return document.execution == "planning" ? "Planning query" : "Waiting for results"
        }
    }
}

struct ResultPanel: View {
    @ObservedObject var model: ResultViewport
    let bridge: Bridge
    let session: UUID
    let document: DocumentSnapshot
    let cancel: () -> Bool
    @StateObject private var row: RowInspectorModel
    @StateObject private var cell: CellInspectorModel
    @State private var inspecting: ResultAddress?

    init(model: ResultViewport, bridge: Bridge, session: UUID, document: DocumentSnapshot, cancel: @escaping () -> Bool) {
        self.model = model
        self.bridge = bridge
        self.session = session
        self.document = document
        self.cancel = cancel
        _row = StateObject(wrappedValue: RowInspectorModel(bridge: bridge, source: model.source))
        _cell = StateObject(wrappedValue: CellInspectorModel(source: model.source))
    }

    private struct Revision: Equatable {
        let session: UUID
        let document: UUID
        let generation: UInt64
        let rows: Int
        let phase: String?
        let execution: String
    }
    private var revision: Revision { Revision(session: session, document: document.document, generation: document.generation, rows: document.rows, phase: document.resultPhase, execution: document.execution) }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(ResultStatus.describe(document)).accessibilityIdentifier("results.status")
                Text(document.execution.capitalized).foregroundStyle(.secondary).accessibilityIdentifier("query.execution")
                if document.generation > 0 { Text("Run \(document.generation)").foregroundStyle(.secondary).accessibilityIdentifier("query.generation") }
                if model.isLoading || cell.copying { ProgressView().controlSize(.small).accessibilityLabel(cell.copying ? "Copying cell" : "Loading result pages") }
                Spacer()
                if document.historyPlanAvailable == true {
                    SavedPlanButton(bridge: bridge, scope: ResultScope(session: session, document: document.document, generation: document.generation))
                }
                Button("Inspect Row") { if let selected = model.selected { row.show(selected, pinned: true, columns: model.columns.count) } }
                    .disabled(model.selected == nil).accessibilityIdentifier("results.inspect-row")
                    .overlay(alignment: .bottom) {
                        RowPopover(model: row, columns: model.columns, inspect: { address in row.close(); inspecting = address }).frame(width: 1, height: 1)
                    }
                Button("Inspect Cell") { inspecting = model.selected }
                    .disabled(model.selected == nil)
                    .accessibilityIdentifier("results.inspect")
                Button("Copy Cell") { if let selected = model.selected { cell.copy(selected) } }
                    .disabled(model.selected == nil || cell.copying)
                    .accessibilityIdentifier("results.copy")
                if cell.copying { Button("Cancel Copy") { cell.cancelCopy() } }
            }
            .font(.caption)
            if document.history == true {
                Text("Saved samples contain legacy text values; SQL null identity and original column types are unavailable.").font(.caption).foregroundStyle(.secondary).accessibilityIdentifier("history.sample-note")
            }
            if let failure = model.loadError {
                HStack {
                    Text(failure).foregroundStyle(.red).textSelection(.enabled).accessibilityIdentifier("results.read-error")
                    Button("Retry") { model.retry() }.accessibilityIdentifier("results.retry")
                }
            }
            if let failure = cell.failure { Text(failure).foregroundStyle(.red).textSelection(.enabled).accessibilityIdentifier("results.cell-error") }
            if let error = document.captureError, document.error == nil {
                Text(QueryDiagnostic.describe(error)).foregroundStyle(.red).textSelection(.enabled).accessibilityIdentifier("results.capture-error")
            }
            ResultTable(model: model, inspect: { inspecting = model.selected }, copy: {
                if let selected = model.selected { cell.copy(selected) }
            }, cancel: cancel, peek: {
                if let selected = model.selected { row.show(selected, pinned: false, columns: model.columns.count) }
            }, releasePeek: { row.release(milliseconds: $0) })
            .id(model.scope)
            .overlay {
                if model.availableRows == 0 && !model.isLoading {
                    Text(document.generation == 0 ? "Results appear here" : "No rows").foregroundStyle(.secondary).allowsHitTesting(false)
                }
            }
        }
        .padding(.top, 8)
        .onChange(of: revision, initial: true) { _, _ in
            let scope = ResultScope(session: session, document: document.document, generation: document.generation)
            if model.scope != scope { cell.cancel(); row.close(); inspecting = nil }
            model.update(session: session, document: document)
        }
        .onDisappear { model.retire(); cell.cancel(); row.close() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didResignActiveNotification)) { _ in row.reset() }
        .sheet(item: $inspecting) { address in CellInspectorView(model: cell, address: address) }
    }
}

private struct CellInspectorView: View {
    @ObservedObject var model: CellInspectorModel
    let address: ResultAddress
    @Environment(\.dismiss) private var dismiss

    private var rangeDescription: String {
        if model.total == 0 { return "Empty text" }
        return "Characters \(model.offset + 1)-\(model.offset + (model.text?.unicodeScalars.count ?? 0)) of \(model.total)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Row \(address.row + 1), column \(address.column + 1)").font(.headline)
                Spacer()
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction).accessibilityIdentifier("cell.close")
            }
            if model.loading { ProgressView("Loading cell") }
            if let error = model.failure {
                Text(error).foregroundStyle(.red).textSelection(.enabled)
                Button("Retry") { model.load(offset: model.offset) }
            }
            if model.loaded {
                if let text = model.text {
                    ReadOnlyText(text: text)
                    Text(verbatim: rangeDescription)
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityValue(rangeDescription).accessibilityIdentifier("cell.range")
                } else { Text("SQL NULL").foregroundStyle(.secondary).accessibilityIdentifier("cell.null") }
            }
            Spacer(minLength: 0)
            HStack {
                Button("Previous") { model.load(offset: max(0, model.offset - CellInspectorModel.chunkSize)) }
                    .disabled(model.loading || model.offset == 0).accessibilityIdentifier("cell.previous")
                Button("Next") { model.load(offset: model.offset + CellInspectorModel.chunkSize) }
                    .disabled(model.loading || model.offset + CellInspectorModel.chunkSize >= model.total).accessibilityIdentifier("cell.next")
                Button("Last") { model.load(offset: max(0, (model.total - 1) / CellInspectorModel.chunkSize * CellInspectorModel.chunkSize)) }
                    .disabled(model.loading || model.offset + CellInspectorModel.chunkSize >= model.total).accessibilityIdentifier("cell.last")
                Spacer()
                if model.copying {
                    ProgressView().controlSize(.small)
                    Button("Cancel Copy") { model.cancelCopy() }
                } else {
                    Button("Copy Entire Cell") { model.copy(address) }.accessibilityIdentifier("cell.copy")
                }
            }
        }
        .padding(20)
        .frame(minWidth: 600, idealWidth: 760, minHeight: 400, idealHeight: 520)
        .onAppear { model.open(address) }
        .onDisappear { model.cancel() }
    }
}

struct ReadOnlyText: NSViewRepresentable {
    let text: String
    var identifier = "cell.value"

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        let view = NSTextView()
        view.isEditable = false
        view.isSelectable = true
        view.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        view.isHorizontallyResizable = false
        view.isVerticallyResizable = true
        view.autoresizingMask = [.width]
        view.textContainer?.widthTracksTextView = true
        view.setAccessibilityIdentifier(identifier)
        scroll.documentView = view
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let view = scroll.documentView as? NSTextView, view.string != text else { return }
        view.string = text
        view.scrollRangeToVisible(NSRange(location: 0, length: 0))
    }
}
