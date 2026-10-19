import AppKit
import Combine
import DzbanBridge
import SwiftUI

@MainActor
public final class HistoryBrowserModel: ObservableObject {
    @Published public private(set) var list: HistoryList?
    @Published public private(set) var loading = false
    @Published public private(set) var failure: String?
    private let application: ApplicationClient
    private var epoch: UInt64 = 0
    private var work: Task<Void, Never>?
    public init(application: ApplicationClient) { self.application = application }
    public func refresh(session: UUID) {
        epoch &+= 1
        let epoch = epoch
        work?.cancel()
        loading = true
        failure = nil
        work = Task { [weak self, application] in
            defer { if self?.epoch == epoch { self?.loading = false } }
            do {
                let response = try await application.history(session: session)
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.list = response
            } catch {
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
            }
        }
    }
    @discardableResult
    func presentationEnded() -> Task<Void, Never> {
        epoch &+= 1
        work?.cancel()
        work = nil
        let epoch = epoch
        return Task { @MainActor [weak self] in
            guard let self, self.epoch == epoch else { return }
            self.retire()
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
    @State private var selectedIndex: Int?
    @State private var selectedIdentity: EntryIdentity?
    @State private var focusPending: Bool
    @FocusState private var focusedEntry: Int?
    @FocusState private var emptyFocused: Bool

    private struct EntryIdentity: Equatable {
        let sql: String
        let executedAt: UInt64
        init(_ entry: HistoryList.Entry) { sql = entry.sqlPreview; executedAt = entry.executedAt }
    }

    init(store: ApplicationStore, session: UUID, close: @escaping () -> Void) {
        self.store = store
        self.session = session
        self.close = close
        _focusPending = State(initialValue: store.historyFocused)
        _model = StateObject(wrappedValue: HistoryBrowserModel(application: store.application))
    }

    private var entries: [HistoryList.Entry] { model.list?.entries ?? [] }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 4) {
                Text("Query History").font(.caption.weight(.semibold)).lineLimit(1)
                Spacer(minLength: 0)
                ZStack {
                    if model.loading || restoring {
                        ProgressView().controlSize(.small).accessibilityLabel(restoring ? "Restoring query" : "Loading history")
                    }
                }.frame(width: 14, height: 14)
                Button { focusPending = true; model.refresh(session: session) } label: { Image(systemName: "arrow.clockwise").frame(width: 20, height: 20) }
                    .help("Refresh history").accessibilityLabel("Refresh").disabled(model.loading || restoring).accessibilityIdentifier("history.refresh")
                Button(action: close) { Image(systemName: "xmark").frame(width: 20, height: 20) }
                    .help("Hide history").accessibilityLabel("Close").accessibilityIdentifier("history.close")
            }.buttonStyle(.borderless).padding(.horizontal, 6).frame(height: 28)
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(entries) { entry in
                            HStack(alignment: .center, spacing: 6) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(entry.sqlPreview).font(.system(size: 12, design: .monospaced)).lineLimit(2)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                    Text(Date(timeIntervalSince1970: Double(entry.executedAt)).formatted(date: .abbreviated, time: .shortened))
                                        .font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                                    Text(summary(entry)).font(.caption2).foregroundStyle(entry.status == .success ? Color.secondary : Color.red).lineLimit(1)
                                }
                                Button { select(entry.index); restore(entry.index) } label: { Image(systemName: "arrow.uturn.backward").frame(width: 20, height: 20) }
                                    .buttonStyle(.borderless).focusable(false)
                                    .help("Restore SQL and saved results without running the query").accessibilityLabel("Restore")
                                    .disabled(restoring || model.loading).accessibilityIdentifier("history.restore.\(entry.index)")
                            }
                            .padding(.horizontal, 8).padding(.trailing, 12).padding(.vertical, 6)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                            .onTapGesture { select(entry.index) }
                            .focusable().focusEffectDisabled().focused($focusedEntry, equals: entry.index)
                            .onKeyPress(.upArrow) { move(-1); return .handled }
                            .onKeyPress(.downArrow) { move(1); return .handled }
                            .onKeyPress(.return) { restore(entry.index); return .handled }
                            .background(selectedIndex == entry.index ? Color.accentColor.opacity(0.12) : Color.clear, in: RoundedRectangle(cornerRadius: 4))
                            .overlay(RoundedRectangle(cornerRadius: 4).stroke(selectedIndex == entry.index ? Color.accentColor : Color.clear, lineWidth: 1.5))
                            .accessibilityElement(children: .contain).accessibilityLabel(entry.sqlPreview)
                            .accessibilityIdentifier("history.row.\(entry.index)")
                            .accessibilityAddTraits(selectedIndex == entry.index ? .isSelected : [])
                            .id(entry.index)
                            Divider()
                        }
                    }.padding(.horizontal, 4).padding(.vertical, 4)
                }
                .accessibilityIdentifier("history.entries")
                .overlay {
                    if entries.isEmpty && !model.loading {
                        Text(model.failure == nil ? "No saved queries" : "History unavailable").font(.callout).foregroundStyle(.secondary)
                    }
                }
                .onChange(of: focusedEntry) { _, index in
                    if let index { select(index, focus: false); proxy.scrollTo(index, anchor: .center) }
                    updateFocus()
                }
                .onChange(of: emptyFocused) { _, _ in updateFocus() }
            }
        }
        .frame(minWidth: 240, minHeight: 180)
        .background(Color(nsColor: .windowBackgroundColor))
        .focusable(entries.isEmpty).focusEffectDisabled().focused($emptyFocused)
        .onKeyPress(.escape) { Task { @MainActor in close() }; return .handled }
        .overlay(alignment: .bottomLeading) {
            if let failure = model.failure {
                Text(failure).font(.caption).foregroundStyle(.red).textSelection(.enabled).lineLimit(3).padding(8)
                    .background(Color(nsColor: .windowBackgroundColor)).accessibilityIdentifier("history.error")
            }
        }
        .task(id: session) { model.refresh(session: session) }
        .onChange(of: model.loading) { _, loading in if !loading { reconcileSelection() } }
        .onChange(of: store.document?.execution) { _, execution in if execution == .finished { model.refresh(session: session) } }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in model.refresh(session: session) }
        .onDisappear { store.historyFocused = false; model.presentationEnded() }
    }

    private func summary(_ entry: HistoryList.Entry) -> String {
        ([entry.status == .success ? "Succeeded" : "Failed"] + [entry.rows.map { "\($0) rows" }, entry.durationMs.map { "\($0) ms" }].compactMap { $0 }).joined(separator: " - ")
    }

    private func select(_ index: Int, focus: Bool = true) {
        guard let entry = entries.first(where: { $0.index == index }) else { return }
        selectedIndex = index
        selectedIdentity = EntryIdentity(entry)
        if focus { focusedEntry = index }
    }

    private func move(_ direction: Int) {
        guard !entries.isEmpty, !restoring else { return }
        let position = entries.firstIndex { $0.index == selectedIndex } ?? 0
        select(entries[min(max(0, position + direction), entries.count - 1)].index)
    }

    private func reconcileSelection() {
        let focus = focusPending || focusedEntry != nil || emptyFocused
        if let entry = entries.first(where: { EntryIdentity($0) == selectedIdentity }) ?? entries.first {
            select(entry.index, focus: focus)
        } else {
            selectedIndex = nil
            selectedIdentity = nil
            if focus { emptyFocused = true }
        }
        focusPending = false
    }

    private func updateFocus() { store.historyFocused = focusedEntry != nil || emptyFocused }

    private func restore(_ index: Int) {
        guard !restoring, !model.loading, let revision = model.list?.revision else { return }
        restoring = true
        Task { [store] in
            let restored = await store.restoreHistory(session: session, revision: revision, index: index)
            restoring = false
            if restored, let document = store.document {
                focusedEntry = nil
                emptyFocused = false
                store.historyFocused = false
                DispatchQueue.main.async { [weak store] in
                    guard let store, store.navigationAvailable, store.selectedSession == session,
                          store.document?.document == document.document, !store.historyFocused else { return }
                    store.editor(for: document).requestFocus()
                }
            }
        }
    }
}

struct SavedPlanButton: View {
    let application: ApplicationClient
    let scope: ResultScope
    @State private var showing = false
    @State private var plan: PlanPresentation?
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
                    do { plan = try await application.savedPlan(scope) }
                    catch { failure = (error as? BridgeFailure)?.message ?? error.localizedDescription }
                }
            }
    }
}
