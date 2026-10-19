import AppKit
import Combine
import DzbanBridge
import SwiftUI

@MainActor
final class WindowActionsModel: ObservableObject {
    @Published var visible = false
    @Published private(set) var searchText = ""
    @Published private var selected: String?
    private let store: ApplicationStore
    private var updates: AnyCancellable?

    init(store: ApplicationStore) {
        self.store = store
        updates = store.objectWillChange.sink { [weak self] in self?.objectWillChange.send() }
    }

    private var actionPlaceholder: String {
        if store.isShuttingDown { return "Closing connections..." }
        if let hint = store.sharedInput?.hint {
            switch hint.kind {
            case .primaryGuide: return "↩: run - ⇧: estimate - ⇧↩: estimate and run"
            case .estimating: return "↩: run - estimating rows and cost..."
            case .estimateReady: return hint.plan.map { "↩: run - " + PlanFormatting.compact($0) } ?? "↩: run"
            case .estimateError: return hint.error ?? "Estimate failed"
            case .planning, .running, .canceling: return "Search actions..."
            default: break
            }
        }
        if store.plan.loading { return "Estimating query..." }
        if store.plan.visible, let value = store.plan.plan { return PlanFormatting.compact(value) }
        return store.selectedSession == nil ? "Search actions..." : "Hold ⌘⇧: estimate | ⌘↩: run"
    }

    private var primaryButton: some View {
        let store = store
        return Button { store.sendEditorIntent(.primary) } label: {
            Label(store.controls?.primaryAction == .stop ? "Stop" : store.controls?.primaryAction == .stopping ? "Stopping" : "Run", systemImage: store.controls?.primaryAction == .stop ? "stop.fill" : store.controls?.primaryAction == .stopping ? "hourglass" : "play.fill")
        }.labelStyle(.iconOnly).frame(width: 28, height: 28).help("Run or stop: ⌘↩. Estimate and run: ⌘⇧↩.")
            .disabled(!store.currentTabAvailable || store.controls?.primaryAction == .unavailable || store.controls?.primaryAction == .stopping || store.selectedSession == nil).accessibilityIdentifier("query.primary").accessibilityValue(store.document?.execution.rawValue.capitalized ?? "Idle")
    }

    func configuration(restoreFocus: @escaping () -> Void) -> NativeToolbarConfiguration {
        let store = store
        return NativeToolbarConfiguration(
            history: AnyView(Button { store.historyVisible.toggle() } label: {
                Label("Query History", systemImage: "sidebar.left")
            }.labelStyle(.iconOnly).help("Toggle query history").accessibilityIdentifier("history.open")),
            plan: AnyView(PlanPreviewButton(store: store)),
            primary: AnyView(primaryButton),
            placeholder: actionPlaceholder,
            searchText: searchText,
            searchVisible: visible,
            menuHeight: height,
            focusRequest: store.actionSearchRequest,
            search: { [weak self] in self?.search($0) },
            move: { [weak self] in self?.move($0) },
            submit: { [weak self] in self?.submit(restoreFocus: restoreFocus) },
            dismiss: { [weak self] restore in self?.dismiss(); if restore { restoreFocus() } }
        )
    }

    struct ActionItem: Identifiable {
        let id: String
        let title: String
        let detail: String
        let enabled: Bool
        let perform: () -> Void
    }

    var items: [ActionItem] {
        let store = store
        var items = [
            ActionItem(id: "focus-query", title: "Focus Query Editor", detail: "Return to SQL at the current cursor position", enabled: store.document != nil && store.navigationAvailable) { if let document = store.document { store.editor(for: document).requestFocus() } },
            ActionItem(id: "focus-results", title: "Focus Results", detail: "Navigate result rows and cells", enabled: store.document != nil && store.navigationAvailable && store.results.isFocusable) { store.results.requestFocus() },
            ActionItem(id: "connections", title: "Show Connections", detail: "Choose a database connection", enabled: store.navigationAvailable) { Task { await store.chooseConnections() } },
            ActionItem(id: "history", title: "Query History", detail: "Show saved queries on the left", enabled: true) { store.historyVisible = true },
            ActionItem(id: "new", title: "New Query Tab", detail: "Choose a connection for a new query", enabled: store.navigationAvailable) { Task { await store.newDocument() } },
            ActionItem(id: "run", title: store.controls?.primaryAction == .stop ? "Stop Query" : "Run Query", detail: "Run or cancel the current query", enabled: store.controls?.primaryAction == .run || store.controls?.primaryAction == .stop) { store.sendEditorIntent(.primary) },
            ActionItem(id: "plan", title: "Preview Query Plan", detail: "Estimate the current query", enabled: store.canPreview) { Task { await store.openCurrentPlan() } },
            ActionItem(id: "disconnect", title: "Disconnect", detail: "Close the current workspace", enabled: store.selectedSession != nil && store.navigationAvailable) { if let session = store.selectedSession { Task { await store.closeSession(session) } } }
        ]
        items += [
            ActionItem(id: "probe", title: "Test All Connections", detail: "Probe configured servers and measure delay", enabled: store.navigationAvailable) { Task { await store.probeAllProfiles() } },
            ActionItem(id: "shortcuts", title: "Keyboard Shortcuts", detail: "Show native keyboard controls", enabled: true) { store.shortcutsVisible = true },
            ActionItem(id: "close-query", title: "Close Query Tab", detail: "Close the selected tab", enabled: store.navigationAvailable) { Task { await store.closeCurrentTab() } },
            ActionItem(id: "next-query", title: "Next Query Tab", detail: "Select the next query", enabled: store.pages.count > 1 && store.navigationAvailable) { Task { await store.cycleDocument(1) } },
            ActionItem(id: "previous-query", title: "Previous Query Tab", detail: "Select the previous query", enabled: store.pages.count > 1 && store.navigationAvailable) { Task { await store.cycleDocument(-1) } },
            ActionItem(id: "next-connection", title: "Next Connection", detail: "Select the next open database", enabled: store.pages.count > 1 && store.navigationAvailable) { Task { await store.cycleWorkspace(1) } },
            ActionItem(id: "previous-connection", title: "Previous Connection", detail: "Select the previous open database", enabled: store.pages.count > 1 && store.navigationAvailable) { Task { await store.cycleWorkspace(-1) } }
        ]
        items += store.profiles.map { profile in
            ActionItem(id: "connect.\(profile.name)", title: profile.name, detail: "Open database connection", enabled: store.pendingProfile == nil && store.navigationAvailable) { Task { await store.connect(profile: profile.name) } }
        }
        items += store.pages.map { page in
            let workspace = store.workspaces.first { $0.id == page.session }
            let title = workspace.map { "\($0.profile) - \($0.documents.first?.title ?? "Query")" } ?? "New Query - Choose Connection"
            return ActionItem(id: "workspace.\(page.id)", title: title, detail: "Select query tab", enabled: store.navigationAvailable) { Task { await store.selectPage(page.id) } }
        }
        return items.filter { searchText.isEmpty || "\($0.title) \($0.detail)".localizedCaseInsensitiveContains(searchText) }
    }

    var height: CGFloat { min(360, CGFloat(max(1, items.count)) * 48 + 16) }
    var highlighted: String? {
        let enabled = items.filter(\.enabled)
        return enabled.first(where: { $0.id == selected })?.id ?? enabled.first?.id
    }

    func search(_ text: String) { searchText = text; selected = nil; visible = true }
    func move(_ direction: Int) {
        let enabled = items.filter(\.enabled)
        guard !enabled.isEmpty else { visible = true; return }
        if !visible { selected = enabled.first?.id }
        else {
            let index = enabled.firstIndex { $0.id == highlighted } ?? 0
            selected = enabled[min(max(0, index + direction), enabled.count - 1)].id
        }
        visible = true
    }
    func dismiss() { visible = false; searchText = ""; selected = nil }
    func submit(restoreFocus: () -> Void) {
        guard let action = items.first(where: { $0.enabled && $0.id == highlighted }) else { return }
        dismiss()
        restoreFocus()
        action.perform()
    }
}
