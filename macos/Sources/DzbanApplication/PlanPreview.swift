import Combine
import DzbanBridge
import SwiftUI

@MainActor
public final class PlanPreviewModel: ObservableObject {
    @Published public private(set) var visible = false
    @Published public private(set) var pinned = false
    @Published public private(set) var loading = false
    @Published public private(set) var plan: JSONValue?
    @Published public private(set) var failure: String?
    var userPin: (() -> Bool)?
    var userClose: (() -> Void)?
    private let bridge: Bridge
    private var epoch: UInt64 = 0
    private var session: UUID?
    private var task: Task<Void, Never>?
    private var cleanup: Task<Void, Never>?
    private var visibilityWork: Task<Void, Never>?

    public init(bridge: Bridge) { self.bridge = bridge }
    public func show(session: UUID, document: DocumentSnapshot, text: String, selection: Range<Int>, pinned: Bool = false, display: Bool = true) async {
        dismiss()
        self.session = session
        let epoch = epoch
        await cleanup?.value
        guard self.epoch == epoch else { return }
        loading = true
        visible = display
        task = Task { [weak self, bridge] in
            guard let self else { return }
            do {
                if display {
                    self.visibility(pinned ? .open : .show)
                    await self.visibilityWork?.value
                }
                guard !Task.isCancelled, self.epoch == epoch else { return }
                let response = try await bridge.request(.preview(session: session, document: document.document, sql: text, revision: document.draftRevision, start: selection.lowerBound, end: selection.upperBound, requestRevision: epoch), as: Preview.self)
                guard !Task.isCancelled, self.epoch == epoch, response.session == session,
                      response.document == document.document, response.draftRevision == document.draftRevision else { return }
                self.plan = response.plan
                self.loading = false
            } catch {
                guard !Task.isCancelled, self.epoch == epoch else { return }
                self.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
                self.loading = false
            }
        }
        await task?.value
    }
    public func present(visible: Bool, pinned: Bool = false) { self.visible = visible; self.pinned = pinned }
    public func waitForCleanup() async { await cleanup?.value }
    public func close() { dismiss(); userClose?() }
    public func release(milliseconds: UInt64) { visibility(.release, milliseconds: milliseconds) }
    public func pin() { if userPin?() != true { visibility(.pin) } }
    public func reset() { visibility(.reset) }
    private func visibility(_ event: PeekEvent, milliseconds: UInt64? = nil) {
        guard let session else { return }
        let epoch = epoch
        let previous = visibilityWork
        visibilityWork = Task { [weak self] in
            await previous?.value
            guard let self, self.epoch == epoch else { return }
            await self.changeVisibility(event, epoch: epoch, session: session, milliseconds: milliseconds)
        }
    }
    private func changeVisibility(_ event: PeekEvent, epoch: UInt64, session: UUID, milliseconds: UInt64? = nil) async {
        do {
            let result = try await bridge.request(.peek(session: session, surface: .plan, event: event, elapsedMilliseconds: milliseconds), as: PeekVisibility.self)
            guard self.epoch == epoch else { return }
            visible = result.visible
            pinned = result.pinned
            if !result.visible { dismiss() }
        } catch { if self.epoch == epoch { failure = error.localizedDescription } }
    }
    public func dismiss() {
        epoch &+= 1
        task?.cancel()
        task = nil
        visible = false
        pinned = false
        loading = false
        plan = nil
        failure = nil
        if let session {
            self.session = nil
            let revision = epoch
            let previous = cleanup
            let pendingVisibility = visibilityWork
            cleanup = Task { [bridge] in
                await previous?.value
                await pendingVisibility?.value
                let _: JSONValue? = try? await bridge.request(.cancelPreview(session: session, requestRevision: revision), as: JSONValue.self)
                let _: PeekVisibility? = try? await bridge.request(.peek(session: session, surface: .plan, event: .close), as: PeekVisibility.self)
            }
        }
    }
}

struct PlanPreviewPanel: View {
    @ObservedObject var model: PlanPreviewModel
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Query Plan").font(.headline)
                Spacer()
                Button(model.pinned ? "Pinned" : "Pin") { model.pin() }.disabled(model.pinned).accessibilityIdentifier("plan.pin")
                Button("Close") { model.close() }.keyboardShortcut(.cancelAction).accessibilityIdentifier("plan.close")
            }
            if model.loading { ProgressView("Loading plan").accessibilityIdentifier("plan.loading") }
            if let error = model.failure {
                Text(error).foregroundStyle(.red).textSelection(.enabled).accessibilityIdentifier("plan.error")
            }
            if let plan = model.plan {
                ReadOnlyText(text: PlanFormatting.text(plan), identifier: "plan.value")
            }
        }.padding(16).frame(width: 620, height: 400)
    }
}

struct PlanPreviewButton: View {
    @ObservedObject var store: ApplicationStore
    @ObservedObject private var model: PlanPreviewModel
    init(store: ApplicationStore) { self.store = store; model = store.plan }
    var body: some View {
        Button { Task { await store.openCurrentPlan() } } label: {
            Label("Preview Plan", systemImage: "waveform.path.ecg")
        }
            .labelStyle(.iconOnly)
            .help("Preview the current query plan")
            .disabled(!store.canPreview)
            .accessibilityIdentifier("plan.open")
            .popover(isPresented: Binding(get: { model.visible }, set: { if !$0 && model.visible { model.close() } }), arrowEdge: .bottom) {
                PlanPreviewPanel(model: model)
            }
    }
}

public enum PlanFormatting {
    public static func rowEstimate(_ plan: JSONValue) -> String? {
        guard case .object(let root) = plan, case .object(let estimate) = root["estimate"],
              case .number(let rows) = estimate["rows"], rows.isFinite, rows >= 0 else { return nil }
        let prefix = root["backend"] == .string("clickhouse") ? "Scan est." : "Est."
        return "\(prefix) ~" + String(format: "%.0f", rows) + " rows"
    }

    public static func costEstimate(_ plan: JSONValue) -> String? {
        guard case .object(let root) = plan, case .object(let estimate) = root["estimate"],
              case .number(let total) = estimate["total_cost"], total.isFinite, total >= 0 else { return nil }
        if case .number(let startup) = estimate["startup_cost"], startup.isFinite, startup >= 0 {
            return "Cost " + String(format: "%.2f..%.2f", startup, total)
        }
        return "Cost " + String(format: "%.2f", total)
    }

    public static func compact(_ plan: JSONValue) -> String {
        guard case .object(let root) = plan, case .object(let estimate) = root["estimate"], case .number(let rows) = estimate["rows"] else { return "Estimate ready" }
        let count = String(format: "%.0f", rows)
        if case .number(let startup) = estimate["startup_cost"], case .number(let total) = estimate["total_cost"] {
            return "~\(count) rows - cost " + String(format: "%.2f..%.2f", startup, total)
        }
        if case .number(let parts) = estimate["parts"], case .number(let marks) = estimate["marks"] {
            return "~\(count) rows - " + String(format: "%.0f parts - %.0f marks", parts, marks)
        }
        return "~\(count) rows"
    }

    public static func summary(_ plan: JSONValue) -> String {
        guard case .object(let root) = plan, case .object(let estimate) = root["estimate"] else { return "Estimated query plan" }
        var parts: [String] = []
        for (key, label) in [("rows", "Estimated rows"), ("startup_cost", "Startup cost"), ("total_cost", "Total cost"), ("parts", "Parts"), ("marks", "Marks")] {
            if let value = estimate[key], value != .null { parts.append("\(label): \(describe(value))") }
        }
        if let diagnostic = estimate["diagnostic"], diagnostic != .null { parts.append("Estimate unavailable: \(QueryDiagnostic.describe(diagnostic))") }
        return parts.isEmpty ? "Estimated query plan" : parts.joined(separator: " | ")
    }
    public static func text(_ plan: JSONValue) -> String {
        if case .object(let root) = plan, case .string(let text) = root["text"] { return text }
        return "No text plan available"
    }

    private static func describe(_ value: JSONValue) -> String {
        switch value {
        case .null: return "SQL NULL"
        case .string(let text): return text
        case .bool(let value): return String(value)
        case .number(let value): return String(value)
        case .array(let values): return values.map(describe).joined(separator: ", ")
        case .object(let values): return values.keys.sorted().map { "\($0): \(describe(values[$0]!))" }.joined(separator: ", ")
        }
    }

}
