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
    private let bridge: Bridge
    private var epoch: UInt64 = 0
    private var session: UUID?
    private var task: Task<Void, Never>?
    private var cleanup: Task<Void, Never>?
    private var visibilityWork: Task<Void, Never>?

    public init(bridge: Bridge) { self.bridge = bridge }
    public func show(session: UUID, document: DocumentSnapshot, text: String, selection: Range<Int>, pinned: Bool = false) async {
        dismiss()
        self.session = session
        let epoch = epoch
        await cleanup?.value
        guard self.epoch == epoch else { return }
        loading = true
        visible = true
        task = Task { [weak self, bridge] in
            guard let self else { return }
            do {
                self.visibility(pinned ? .open : .show)
                await self.visibilityWork?.value
                guard !Task.isCancelled, self.epoch == epoch, self.visible else { return }
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
    public func release(milliseconds: UInt64) { visibility(.release, milliseconds: milliseconds) }
    public func pin() { visibility(.pin) }
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
                Button("Close") { model.dismiss() }.keyboardShortcut(.cancelAction).accessibilityIdentifier("plan.close")
            }
            if model.loading { ProgressView("Loading plan").accessibilityIdentifier("plan.loading") }
            if let error = model.failure {
                Text(error).foregroundStyle(.red).textSelection(.enabled).accessibilityIdentifier("plan.error")
            }
            if let plan = model.plan {
                Text(PlanFormatting.summary(plan)).font(.caption).accessibilityIdentifier("plan.estimate")
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
        Button { Task { await store.previewCurrent(pinned: true) } } label: {
            Label("Preview Plan", systemImage: "waveform.path.ecg")
        }
            .labelStyle(.iconOnly)
            .help("Command-Shift-P: tap to pin the plan, hold to peek")
            .disabled(!store.canPreview)
            .accessibilityIdentifier("plan.open")
            .popover(isPresented: Binding(get: { model.visible }, set: { if !$0 { model.dismiss() } }), arrowEdge: .bottom) {
                PlanPreviewPanel(model: model)
            }
    }
}

public enum PlanFormatting {
    public static func summary(_ plan: JSONValue) -> String {
        guard case .object(let root) = plan, case .object(let estimate) = root["estimate"] else { return "Estimated query plan" }
        var parts: [String] = []
        for (key, label) in [("rows", "Estimated rows"), ("startup_cost", "Startup cost"), ("total_cost", "Total cost"), ("parts", "Parts"), ("marks", "Marks")] {
            if let value = estimate[key], value != .null { parts.append("\(label): \(CatalogText.describe(value))") }
        }
        if let diagnostic = estimate["diagnostic"], diagnostic != .null { parts.append("Estimate unavailable: \(QueryDiagnostic.describe(diagnostic))") }
        return parts.isEmpty ? "Estimated query plan" : parts.joined(separator: " | ")
    }
    public static func text(_ plan: JSONValue) -> String {
        func object(_ value: JSONValue) -> Any {
            switch value {
            case .null: return NSNull()
            case .string(let text): return text
            case .bool(let value): return value
            case .number(let value): return value
            case .array(let values): return values.map(object)
            case .object(let values): return values.mapValues(object)
            }
        }
        let document: JSONValue
        if case .object(let root) = plan { document = root["document"] ?? plan } else { document = plan }
        guard let data = try? JSONSerialization.data(withJSONObject: object(document), options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed]) else { return CatalogText.describe(document) }
        return String(decoding: data, as: UTF8.self)
    }
}
