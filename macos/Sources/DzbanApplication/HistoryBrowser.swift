import AppKit
import Combine
import DzbanBridge
import SwiftUI

@MainActor
public final class HistoryBrowserModel: ObservableObject {
    @Published public private(set) var list: HistoryList?
    @Published public private(set) var loading = false
    @Published public private(set) var failure: String?
    private let bridge: Bridge
    private var epoch: UInt64 = 0
    private var work: Task<Void, Never>?
    public init(bridge: Bridge) { self.bridge = bridge }
    public func refresh(session: UUID) {
        epoch &+= 1
        let epoch = epoch
        work?.cancel()
        loading = true
        failure = nil
        work = Task { [weak self, bridge] in
            defer { if self?.epoch == epoch { self?.loading = false } }
            do {
                let response = try await bridge.request(.historyList(session: session), as: HistoryList.self)
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.list = response
            } catch {
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
            }
        }
    }
    public func retire() { epoch &+= 1; work?.cancel(); work = nil; list = nil; loading = false }
}

struct HistoryBrowser: View {
    @ObservedObject var store: ApplicationStore
    @StateObject private var model: HistoryBrowserModel
    let session: UUID
    let close: () -> Void
    @State private var restoring = false
    init(store: ApplicationStore, session: UUID, close: @escaping () -> Void) {
        self.store = store
        self.session = session
        self.close = close
        _model = StateObject(wrappedValue: HistoryBrowserModel(bridge: store.bridge))
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Query History").font(.headline)
                Spacer()
                Button("Refresh") { model.refresh(session: session) }.disabled(model.loading || restoring).accessibilityIdentifier("history.refresh")
                Button("Close", action: close).accessibilityIdentifier("history.close")
            }
            if model.loading || restoring { ProgressView(model.loading ? "Loading history" : "Restoring query") }
            if let failure = model.failure { Text(failure).foregroundStyle(.red).textSelection(.enabled).accessibilityIdentifier("history.error") }
            List(model.list?.entries ?? []) { entry in
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(entry.sqlPreview).font(.system(.body, design: .monospaced)).lineLimit(3)
                        Text(Date(timeIntervalSince1970: Double(entry.executedAt)), style: .date)
                            .font(.caption).foregroundStyle(.secondary)
                        Text(([entry.status == "success" ? "Succeeded" : "Failed"] + [entry.rows.map { "\($0) rows" }, entry.durationMs.map { "\($0) ms" }].compactMap { $0 }).joined(separator: " - "))
                            .font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer()
                    Button("Restore") {
                        guard let revision = model.list?.revision else { return }
                        restoring = true
                        Task {
                            _ = await store.restoreHistory(session: session, revision: revision, index: entry.index)
                            restoring = false
                        }
                    }.disabled(restoring || model.loading).accessibilityIdentifier("history.restore.\(entry.index)")
                }.padding(.vertical, 4)
            }.accessibilityIdentifier("history.entries")
            if model.list?.entries.isEmpty == true { Text("No saved queries").foregroundStyle(.secondary) }
            Text("Restore loads SQL and the saved result sample without running the query.").font(.caption).foregroundStyle(.secondary)
        }.padding(12).frame(minWidth: 240, minHeight: 180)
        .task(id: session) { model.refresh(session: session) }
        .onChange(of: store.document?.execution) { _, execution in if execution == "finished" { model.refresh(session: session) } }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in model.refresh(session: session) }
        .onDisappear { model.retire() }
    }
}

struct SavedPlanButton: View {
    let bridge: Bridge
    let scope: ResultScope
    @State private var showing = false
    @State private var plan: JSONValue?
    @State private var failure: String?
    @State private var loading = false
    var body: some View {
        Button("Saved Plan") { showing = true }.accessibilityIdentifier("history.plan")
            .sheet(isPresented: $showing) {
                VStack(alignment: .leading, spacing: 12) {
                    HStack { Text("Saved Query Plan").font(.headline); Spacer(); Button("Close") { showing = false }.keyboardShortcut(.cancelAction).accessibilityIdentifier("history.plan.close") }
                    if loading { ProgressView("Loading saved plan") }
                    if let failure { Text(failure).foregroundStyle(.red).textSelection(.enabled) }
                    if let plan { Text(PlanFormatting.summary(plan)).font(.caption); ReadOnlyText(text: PlanFormatting.text(plan), identifier: "history.plan.value") }
                }.padding(20).frame(width: 680, height: 460)
                .task {
                    loading = true
                    failure = nil
                    plan = nil
                    defer { loading = false }
                    do { plan = try await bridge.request(.savedPlan(session: scope.session, document: scope.document, generation: scope.generation), as: SavedPlan.self).plan }
                    catch { failure = (error as? BridgeFailure)?.message ?? error.localizedDescription }
                }
            }
    }
}
