import AppKit
import DzbanBridge
import SwiftUI
import QuartzCore

public enum ResultStatus {
    public static func describe(_ document: DocumentSnapshot) -> String {
        let feedback = document.feedback
        switch feedback.status {
        case .ready: return "Run a query to see results"
        case .planning: return "Planning query"
        case .waiting: return "Waiting for results"
        case .waitingForSchema: return "Waiting for schema"
        case .receiving: return "Receiving - \(feedback.rows) rows"
        case .draining: return "Truncated - \(feedback.rows) rows; waiting for cleanup"
        case .captureFailedAwaitingCleanup: return "Result capture failed; waiting for cleanup"
        case .finishing: return "Finishing - \(feedback.rows) rows"
        case .canceling: return "Canceling - \(feedback.rows) rows captured"
        case .limitedByRows: return "Truncated - \(feedback.rows) rows (row limit)"
        case .limitedByBytes: return "Truncated - \(feedback.rows) rows (byte limit)"
        case .canceled: return "Canceled - \(feedback.rows) rows"
        case .failed: return "Failed - \(feedback.rows) rows"
        case .command: return feedback.affectedRows.map { "\($0) rows affected" } ?? "Command completed"
        case .complete: return "Complete - \(feedback.rows) rows"
        case .savedFailed: return "Saved query failed"
        case .savedCommand: return feedback.affectedRows.map { "Saved command - \($0) rows affected" } ?? "Saved command"
        case .savedRows: return "Saved - \(feedback.rows) of \(feedback.totalRows ?? feedback.rows) rows"
        }
    }
}

struct ResultPanel: View {
    @ObservedObject var model: ResultViewport
    let session: UUID
    let document: DocumentSnapshot
    let cancel: () -> Bool
    @ObservedObject private var row: RowInspectorModel
    @ObservedObject private var cell: CellInspectorModel
    @State private var inspecting: ResultAddress?

    init(model: ResultViewport, cell: CellInspectorModel, row: RowInspectorModel, session: UUID, document: DocumentSnapshot, cancel: @escaping () -> Bool) {
        self.model = model
        self.session = session
        self.document = document
        self.cancel = cancel
        _row = ObservedObject(wrappedValue: row)
        _cell = ObservedObject(wrappedValue: cell)
    }

    private struct Revision: Equatable {
        let session: UUID
        let document: UUID
        let generation: UInt64
        let rows: Int
        let phase: CapturePhase?
        let execution: ExecutionPhase
    }
    private var revision: Revision { Revision(session: session, document: document.document, generation: document.generation, rows: document.rows, phase: document.resultPhase, execution: document.execution) }

    private var issues: [ResultIssue] {
        var issues: [ResultIssue] = []
        if let diagnostic = document.feedback.queryDiagnostic {
            issues.append(ResultIssue(id: "query.error", title: "Query failed", detail: diagnostic.text))
        }
        if let failure = model.loadError { issues.append(ResultIssue(id: "results.read-error", title: "Unable to load results", detail: failure)) }
        if let failure = model.copyError { issues.append(ResultIssue(id: "results.copy-error", title: "Unable to copy results", detail: failure)) }
        if let failure = cell.failure { issues.append(ResultIssue(id: "results.cell-error", title: "Unable to copy cell", detail: failure)) }
        if let diagnostic = document.feedback.captureDiagnostic {
            issues.append(ResultIssue(id: "results.capture-error", title: "Result capture stopped", detail: diagnostic.text))
        }
        return issues
    }

    private var emptyTitle: String {
        switch document.feedback.empty {
        case .ready: return "Ready to query"
        case .command: return "Command complete"
        case .canceled: return "Query stopped"
        case .noRows: return "No rows returned"
        }
    }

    private var emptyDetail: String {
        switch document.feedback.empty {
        case .ready: return "Run your SQL to explore the results."
        case .command: return document.feedback.affectedRows.map { "\($0) rows affected." } ?? "The command completed successfully."
        case .canceled: return "No rows were returned before the query stopped."
        case .noRows: return "The query completed without returning any rows."
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            QueryProgress(busy: document.feedback.progress != nil)
                .frame(height: 2)
            VStack(spacing: 0) {
                if !issues.isEmpty && model.availableRows > 0 {
                    ResultDiagnostic(issues: issues, retry: model.loadError == nil ? nil : { model.retry() })
                        .padding(.horizontal, 12).padding(.vertical, 8)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color.red.opacity(0.04))
                    Divider()
                }
                ResultTable(model: model, inspect: { inspecting = model.selected }, cancel: cancel, peek: { anchor, rect in
                    if let selected = model.selected { row.show(selected, pinned: false, columns: model.columns.count, anchorView: anchor, anchorRect: rect) }
                }, releasePeek: { row.release(milliseconds: $0) })
                .overlay {
                    if model.availableRows == 0 && !issues.isEmpty {
                        ResultState(symbol: "exclamationmark.circle", title: issues.first?.title ?? "Unable to show results", tint: .red) {
                            ResultDiagnostic(issues: issues, retry: model.loadError == nil ? nil : { model.retry() }, showTitle: false)
                        }
                    } else if document.feedback.progress != nil, let started = document.startedAtSeconds, document.history != true {
                        QueryRunSummary(document: document, started: Date(timeIntervalSince1970: started))
                            .allowsHitTesting(false)
                    } else if model.availableRows == 0 && !model.isLoading {
                        ResultState(symbol: document.feedback.empty == .ready ? "tablecells" : document.feedback.empty == .command ? "checkmark" : "line.3.horizontal.decrease", title: emptyTitle, tint: .secondary) {
                            Text(emptyDetail).font(.callout).foregroundStyle(.secondary)
                        }.allowsHitTesting(false)
                    }
                }
            }

        }
        .overlay {
            RowPopover(model: row, columns: model.columns, cell: cell, rows: model.availableRows) { address in
                model.select(row: address.row, column: address.column)
            }
                .frame(maxWidth: .infinity, maxHeight: .infinity).allowsHitTesting(false)
        }
        .onChange(of: revision, initial: true) { _, _ in
            let scope = ResultScope(session: session, document: document.document, generation: document.generation)
            if model.scope != scope { cell.cancel(); row.close(); inspecting = nil }
            model.update(session: session, document: document)
        }
        .onDisappear { model.retire(); cell.cancel(); row.close() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didResignActiveNotification)) { _ in row.reset() }
        .sheet(item: $inspecting) { address in
            CellInspectorView(model: cell, address: address)
                .frame(minWidth: 600, idealWidth: 760, minHeight: 400, idealHeight: 520)
        }
    }
}

struct ResultFooter: View {
    @ObservedObject var model: ResultViewport
    @ObservedObject var cell: CellInspectorModel
    let document: DocumentSnapshot?
    let application: ApplicationClient

    var body: some View {
        HStack(spacing: 6) {
            FooterArtwork(name: "dzban-result-rows-symbolic")
            if let document {
                Text(document.generation == 0 ? "Ready" : ResultStatus.describe(document).components(separatedBy: ";")[0])
                    .accessibilityValue(ResultStatus.describe(document)).accessibilityIdentifier("results.status")
                    .help(ResultStatus.describe(document))
                if document.history != true, document.commandResult == nil, let plan = document.executionPlan, let estimate = PlanFormatting.rowEstimate(plan) {
                    Text(estimate).foregroundStyle(.secondary).lineLimit(1)
                        .help(PlanFormatting.summary(plan)).accessibilityIdentifier("results.row-estimate")
                }
                if document.execution == .finished, document.history != true, document.commandResult == nil,
                   let plan = document.executionPlan, let cost = PlanFormatting.costEstimate(plan) {
                    Text(cost).foregroundStyle(.secondary)
                        .help("Planner cost estimate").accessibilityIdentifier("results.cost-estimate")
                }
                if document.history == true {
                    Label("Text sample", systemImage: "info.circle").foregroundStyle(.secondary).lineLimit(1)
                        .help("Saved results contain text previews. Original column types and SQL NULL identity are not preserved.")
                        .accessibilityIdentifier("history.sample-note")
                }
                if document.historyPlanAvailable == true, let session = document.session {
                    SavedPlanButton(application: application, scope: ResultScope(session: session, document: document.document, generation: document.generation))
                }
            } else { Text("-- rows") }
            if model.isLoading || cell.copying {
                ProgressView().controlSize(.small).accessibilityLabel(cell.copying ? "Copying cell" : "Loading result pages")
            }
            if cell.copying { Button("Cancel Copy") { cell.cancelCopy() } }
        }.lineLimit(1)
    }
}

private struct ResultIssue: Identifiable {
    let id: String
    let title: String
    let detail: String
}

private struct ResultState<Content: View>: View {
    let symbol: String
    let title: String
    let tint: Color
    @ViewBuilder let content: () -> Content

    var body: some View {
        GeometryReader { geometry in
            ScrollView {
                VStack(spacing: 10) {
                    Image(systemName: symbol).font(.system(size: 22, weight: .regular))
                        .foregroundStyle(tint).frame(width: 44, height: 44)
                        .background(tint.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
                    Text(title).font(.system(size: 15, weight: .semibold)).accessibilityIdentifier("results.state-title")
                    content().frame(maxWidth: 520)
                }
                .multilineTextAlignment(.center).padding(20)
                .frame(maxWidth: .infinity, minHeight: geometry.size.height)
            }
            .background(Color(nsColor: .textBackgroundColor))
        }
        .accessibilityElement(children: .contain).accessibilityIdentifier("results.feedback")
    }
}

private struct ResultDiagnostic: View {
    let issues: [ResultIssue]
    let retry: (() -> Void)?
    var showTitle = true

    var body: some View {
        VStack(alignment: showTitle ? .leading : .center, spacing: 6) {
            if showTitle {
                Label(issues.first?.title ?? "Unable to show results", systemImage: "exclamationmark.circle")
                    .font(.caption.weight(.semibold)).foregroundStyle(.red)
            }
            ForEach(issues) { issue in
                Text(issue.detail).font(.system(size: 12, design: .monospaced)).foregroundStyle(.primary)
                    .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier(issue.id)
            }
            if let retry { Button("Retry", action: retry).controlSize(.small).accessibilityIdentifier("results.retry") }
        }
    }
}

private struct QueryRunSummary: View {
    let document: DocumentSnapshot
    let started: Date
    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.1)) { context in summary(at: context.date) }
    }

    private func summary(at now: Date) -> some View {
        VStack(spacing: 7) {
            HStack(spacing: 7) {
                ProgressView().controlSize(.small)
                Text(document.feedback.progress == .canceling ? "Stopping query" : document.feedback.progress == .planning ? "Planning query" : "Running query")
                    .font(.caption.weight(.medium)).foregroundStyle(.secondary)
            }
            Text(String(format: "Elapsed %.1fs", max(0, now.timeIntervalSince(started))))
                .font(.system(size: 24, weight: .medium, design: .rounded)).monospacedDigit().foregroundStyle(.primary)
                .accessibilityIdentifier("results.elapsed")
            HStack(spacing: 12) {
                Text("Started \(started.formatted(date: .omitted, time: .standard))")
                    .help(started.formatted(date: .complete, time: .standard)).accessibilityIdentifier("results.started")
                Text("\(document.rows) rows received").accessibilityIdentifier("results.received")
            }.font(.caption).foregroundStyle(.secondary).monospacedDigit()
            if let plan = document.executionPlan {
                Text(PlanFormatting.compact(plan)).help(PlanFormatting.summary(plan))
                    .accessibilityIdentifier("results.estimate")
            } else {
                Text(document.feedback.progress == .planning ? "Estimating..." : "Estimate unavailable")
                    .accessibilityIdentifier("results.estimate")
            }
        }
        .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
        .padding(.horizontal, 16).padding(.vertical, 24).frame(maxWidth: .infinity)
        .background {
            LinearGradient(stops: [.init(color: .clear, location: 0), .init(color: Color(nsColor: .textBackgroundColor).opacity(0.98), location: 0.18), .init(color: Color(nsColor: .textBackgroundColor).opacity(0.98), location: 0.82), .init(color: .clear, location: 1)], startPoint: .top, endPoint: .bottom)
        }
        .accessibilityElement(children: .contain).accessibilityIdentifier("results.metrics-box")
    }
}

struct CellInspectorView: View {
    @ObservedObject var model: CellInspectorModel
    let address: ResultAddress
    var back: (() -> Void)? = nil
    var close: (() -> Void)? = nil
    @Environment(\.dismiss) private var dismiss

    private var closeButton: some View {
        Button("Close") { if let close { close() } else { dismiss() } }.accessibilityIdentifier("cell.close")
    }

    private var rangeDescription: String {
        if model.total == 0 { return "Empty text" }
        return "Characters \(model.offset + 1)-\(model.offset + (model.text?.unicodeScalars.count ?? 0)) of \(model.total)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                if let back {
                    Button(action: back) { Label("Back to Row", systemImage: "chevron.left") }
                        .accessibilityIdentifier("cell.back")
                }
                Text("Row \(address.row + 1), column \(address.column + 1)").font(.headline)
                Spacer()
                if back == nil { closeButton.keyboardShortcut(.cancelAction) }
                else { closeButton }
            }
            if model.loading { ProgressView("Loading cell") }
            if let error = model.failure {
                Text(error).foregroundStyle(.red).textSelection(.enabled)
                Button("Retry") { model.openEntire(address) }
            }
            if model.loaded {
                if let text = model.text {
                    ReadOnlyText(text: text, focusOnAppear: true)
                    Text(verbatim: rangeDescription)
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityValue(rangeDescription).accessibilityIdentifier("cell.range")
                } else { Text("SQL NULL").foregroundStyle(.secondary).accessibilityIdentifier("cell.null") }
            }
            Spacer(minLength: 0)
            HStack {
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
        .task(id: address) { model.openEntire(address) }
        .onDisappear { model.presentationEnded() }
    }
}

struct ReadOnlyText: NSViewRepresentable {
    let text: String
    var identifier = "cell.value"
    var focusOnAppear = false

    func makeNSView(context: Context) -> NSScrollView {
        #if DZBAN_TEST_FIXTURES
        ViewUpdateDiagnostics.frames.append("ResultPanel.swift")
        defer { ViewUpdateDiagnostics.frames.removeLast() }
        #endif
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 600, height: 400))
        scroll.hasVerticalScroller = true
        let view = ReadOnlyTextView(frame: scroll.contentView.bounds)
        view.focusOnAppear = focusOnAppear
        view.minSize = .zero
        view.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        view.layoutManager?.allowsNonContiguousLayout = true
        view.layoutManager?.backgroundLayoutEnabled = false
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
        #if DZBAN_TEST_FIXTURES
        ViewUpdateDiagnostics.frames.append("ResultPanel.swift")
        defer { ViewUpdateDiagnostics.frames.removeLast() }
        #endif
        guard let view = scroll.documentView as? NSTextView, view.string != text else { return }
        view.string = text
        view.scrollRangeToVisible(NSRange(location: 0, length: 0))
    }
}

private final class ReadOnlyTextView: NSTextView {
    var focusOnAppear = false

    #if DZBAN_TEST_FIXTURES
    override func copy(_ sender: Any?) {
        if ProcessInfo.processInfo.arguments.contains("--test-pasteboard") {
            let pasteboard = ResultMarkdown.pasteboard
            pasteboard.declareTypes([.string], owner: nil)
            let text = string as NSString
            pasteboard.setString(selectedRanges.map { text.substring(with: $0.rangeValue) }.joined(separator: "\n"), forType: .string)
        } else { super.copy(sender) }
    }
    #endif

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard focusOnAppear, let window else { return }
        DispatchQueue.main.async { [weak self, weak window] in
            guard let self, let window, self.window == window else { return }
            window.makeKey()
            window.makeFirstResponder(self)
        }
    }
}

private struct QueryProgress: NSViewRepresentable {
    let busy: Bool
    func makeNSView(context: Context) -> QueryProgressView { QueryProgressView() }
    func updateNSView(_ view: QueryProgressView, context: Context) { view.setBusy(busy) }
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
