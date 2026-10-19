import Combine
import DzbanBridge
import SwiftUI

@MainActor
public final class PlanPreviewModel: ObservableObject {
    @Published public private(set) var visible = false { didSet { presentationRevision &+= 1 } }
    @Published public private(set) var pinned = false
    @Published public private(set) var loading = false
    @Published public private(set) var plan: PlanPresentation?
    @Published public private(set) var failure: String?
    var userPin: (() -> Void)?
    var userClose: (() -> Void)?
    var userRelease: ((UInt64) -> Void)?
    var userReset: (() -> Void)?
    private var presentationRevision: UInt64 = 0

    public init() {}
    func apply(_ projection: InteractionProjection) {
        visible = projection.visible
        pinned = projection.input.pinned
        loading = projection.loading
        plan = projection.plan
        failure = projection.error
    }
    func present(visible: Bool, pinned: Bool = false) { self.visible = visible; self.pinned = pinned }
    public func close() { dismiss(); userClose?() }
    @discardableResult
    func presentationDismissed() -> Task<Void, Never>? {
        guard visible else { return nil }
        let revision = presentationRevision
        return Task { @MainActor [weak self] in
            guard let self, self.visible, self.presentationRevision == revision else { return }
            self.close()
        }
    }
    public func release(milliseconds: UInt64) { userRelease?(milliseconds) }
    public func pin() { userPin?() }
    public func reset() { userReset?() }
    public func dismiss() {
        visible = false
        pinned = false
        loading = false
        plan = nil
        failure = nil
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
            .popover(isPresented: Binding(get: { model.visible }, set: { if !$0 { model.presentationDismissed() } }), arrowEdge: .bottom) {
                PlanPreviewPanel(model: model)
            }
    }
}

public enum PlanFormatting {
    public static func rowEstimate(_ plan: PlanPresentation) -> String? {
        guard let rows = plan.estimate?.rows else { return nil }
        let prefix = plan.backend == .clickhouse ? "Scan est." : "Est."
        return "\(prefix) ~\(rows) rows"
    }

    public static func costEstimate(_ plan: PlanPresentation) -> String? {
        guard let total = plan.estimate?.totalCost, total.isFinite, total >= 0 else { return nil }
        if let startup = plan.estimate?.startupCost, startup.isFinite, startup >= 0 {
            return "Cost " + String(format: "%.2f..%.2f", startup, total)
        }
        return "Cost " + String(format: "%.2f", total)
    }

    public static func compact(_ plan: PlanPresentation) -> String {
        guard let estimate = plan.estimate, let rows = estimate.rows else { return "Estimate ready" }
        if let startup = estimate.startupCost, let total = estimate.totalCost {
            return "~\(rows) rows - cost " + String(format: "%.2f..%.2f", startup, total)
        }
        if let parts = estimate.parts, let marks = estimate.marks {
            return "~\(rows) rows - \(parts) parts - \(marks) marks"
        }
        return "~\(rows) rows"
    }

    public static func summary(_ plan: PlanPresentation) -> String {
        guard let estimate = plan.estimate else { return "Estimated query plan" }
        var parts: [String] = []
        if let rows = estimate.rows { parts.append("Estimated rows: \(summaryNumber(rows))") }
        if let cost = estimate.startupCost { parts.append("Startup cost: \(cost)") }
        if let cost = estimate.totalCost { parts.append("Total cost: \(cost)") }
        if let value = estimate.parts { parts.append("Parts: \(summaryNumber(value))") }
        if let value = estimate.marks { parts.append("Marks: \(summaryNumber(value))") }
        if let diagnostic = estimate.diagnostic { parts.append("Estimate unavailable: \(diagnostic.text)") }
        return parts.isEmpty ? "Estimated query plan" : parts.joined(separator: " | ")
    }

    public static func text(_ plan: PlanPresentation) -> String { plan.text ?? "No text plan available" }

    private static func summaryNumber(_ value: UInt64) -> String {
        value <= 9_007_199_254_740_992 ? String(Double(value)) : String(value)
    }
}
