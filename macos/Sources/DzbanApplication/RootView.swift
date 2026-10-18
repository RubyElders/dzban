import AppKit
import DzbanBridge
import SwiftUI

enum WorkspaceLayout {
    static let outerInset: CGFloat = 19
    static let gutterInset: CGFloat = 8
    static let textInset: CGFloat = 32
    static let columnInset: CGFloat = 6
}

public struct RootView: View {
    @ObservedObject var store: ApplicationStore
    @ObservedObject private var plan: PlanPreviewModel
    @State private var actionsVisible = false
    @State private var actionSearch = ""
    @State private var selectedAction: String?
    @FocusState private var focusedProfile: String?
    @State private var selectedProfile: String?

    public init(store: ApplicationStore) { self.store = store; plan = store.plan }

    public var body: some View {
        VStack(spacing: 0) {
            connectionTabs
            Divider()
            HSplitView {
                if store.historyVisible {
                    VStack(spacing: 0) {
                        if let session = store.selectedSession {
                            HistoryBrowser(store: store, session: session) { store.historyVisible = false }
                                .id(session)
                        } else {
                            ContentUnavailableView("Query History", systemImage: "clock", description: Text("Connect to a database to browse saved queries."))
                        }
                    }
                    .frame(minWidth: 240, idealWidth: 280, maxWidth: 320, maxHeight: .infinity)
                }
                Group {
                    if let workspace = store.workspace { workspaceView(workspace) }
                    else { chooser }
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
            Divider()
            statusbar
        }
        .frame(minWidth: 900, maxWidth: .infinity, minHeight: 500, maxHeight: .infinity)
        .background(WindowPolicy(store: store, toolbar: toolbar).frame(width: 0, height: 0))
        .overlay {
            if actionsVisible {
                GeometryReader { geometry in
                    ZStack(alignment: .top) {
                        Color.clear.allowsHitTesting(false)
                        actionMenu
                            .frame(width: min(600, max(380, geometry.size.width - 320)), height: min(actionMenuHeight, geometry.size.height - 16))
                            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
                            .overlay(RoundedRectangle(cornerRadius: 10).stroke(.quaternary))
                            .shadow(radius: 12, y: 5)
                            .padding(.top, 8)
                    }.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                }
            }
        }
        .sheet(isPresented: $store.shortcutsVisible, onDismiss: restoreFocus) { NativeShortcutReference() }
        .alert("Dzban", isPresented: Binding(get: { store.failure != nil }, set: { if !$0 { store.failure = nil } })) {
            Button("OK") { store.failure = nil }
        } message: { Text(store.failure ?? "") }
        .task { await store.start(); await Task.yield(); restoreFocus() }
        .onChange(of: store.selectedPage) { _, _ in
            dismissActions(restore: false)
            if store.connectionsVisible { focusConnection() }
        }
        .onChange(of: store.selectedSession) { _, session in
            if session == nil { focusConnection() }
        }
        .onChange(of: store.isClosed) { _, closed in if closed { NSApp.terminate(nil) } }
        .onChange(of: store.isShuttingDown) { _, closing in
            if closing { store.connectionsVisible = false; store.shortcutsVisible = false; actionsVisible = false }
        }
        .onChange(of: store.historyVisible) { _, visible in if !visible { restoreFocus() } }
        .onChange(of: plan.visible) { _, visible in if !visible { restoreFocus() } }
        .onChange(of: store.document?.document) { _, _ in focusEditor() }
        .onChange(of: store.failure) { _, value in if value == nil { restoreFocus() } }
    }

    private func focusConnection() {
        guard store.connectionsVisible || store.selectedSession == nil else { return }
        selectedProfile = store.workspace?.profile ?? store.profiles.first?.name
        focusedProfile = selectedProfile
    }

    private func focusEditor() {
        guard let document = store.document else { return }
        store.editor(for: document).requestFocus()
    }

    private func restoreFocus() {
        if store.selectedSession == nil { focusConnection() }
        else { focusEditor() }
    }

    private func moveConnection(_ direction: Int) {
        guard !store.profiles.isEmpty else { return }
        let index = store.profiles.firstIndex { $0.name == (focusedProfile ?? selectedProfile) } ?? 0
        focusedProfile = store.profiles[min(max(index + direction, 0), store.profiles.count - 1)].name
    }

    private var actionPlaceholder: String {
        if store.isShuttingDown { return "Closing connections..." }
        if let hint = store.sharedInput?.hint {
            switch hint.kind {
            case "primary_guide": return "↩: run - ⇧: estimate - ⇧↩: estimate and run"
            case "estimating": return "↩: run - estimating rows and cost..."
            case "estimate_ready": return hint.plan.map { "↩: run - " + PlanFormatting.compact($0) } ?? "↩: run"
            case "estimate_error": return hint.error ?? "Estimate failed"
            case "planning", "running", "canceling": return "Search actions..."
            default: break
            }
        }
        if plan.loading { return "Estimating query..." }
        if plan.visible, let value = plan.plan { return PlanFormatting.compact(value) }
        return store.selectedSession == nil ? "Search actions..." : "Hold ⌘⇧: estimate | ⌘↩: run"
    }

    private var newTabButton: some View {
        Button { Task { await store.newDocument() } } label: {
            Label("New Tab", systemImage: "plus")
        }.labelStyle(.iconOnly).frame(width: 28, height: 28).help("New tab: choose a connection (⌘T)")
            .disabled(!store.navigationAvailable).accessibilityIdentifier("query.new")
    }

    private var primaryButton: some View {
        Button { store.sendEditorIntent(.primary) } label: {
            Label(store.controls?.primaryAction == "stop" ? "Stop" : store.controls?.primaryAction == "stopping" ? "Stopping" : "Run", systemImage: store.controls?.primaryAction == "stop" ? "stop.fill" : store.controls?.primaryAction == "stopping" ? "hourglass" : "play.fill")
        }.labelStyle(.iconOnly).frame(width: 28, height: 28).help("Run or stop: ⌘↩. Estimate and run: ⌘⇧↩.")
            .disabled(!store.currentTabAvailable || store.controls?.primaryAction == "unavailable" || store.controls?.primaryAction == "stopping" || store.selectedSession == nil).accessibilityIdentifier("query.primary").accessibilityValue(store.document?.execution.capitalized ?? "Idle")
    }

    private var toolbar: NativeToolbarConfiguration {
        NativeToolbarConfiguration(
            history: AnyView(Button { store.historyVisible.toggle() } label: {
                Label("Query History", systemImage: "sidebar.left")
            }.labelStyle(.iconOnly).help("Toggle query history").accessibilityIdentifier("history.open")),
            plan: AnyView(PlanPreviewButton(store: store)),
            primary: AnyView(primaryButton),
            placeholder: actionPlaceholder,
            searchText: actionSearch,
            searchVisible: actionsVisible,
            menuHeight: actionMenuHeight,
            focusRequest: store.actionSearchRequest,
            search: { text in
                actionSearch = text
                selectedAction = nil
                actionsVisible = true
            },
            move: { direction in
                let enabled = actions.filter(\.enabled)
                guard !enabled.isEmpty else { actionsVisible = true; return }
                if !actionsVisible {
                    selectedAction = enabled.first?.id
                } else {
                    let index = enabled.firstIndex { $0.id == highlightedAction } ?? 0
                    selectedAction = enabled[min(max(0, index + direction), enabled.count - 1)].id
                }
                actionsVisible = true
            },
            submit: {
                guard let action = actions.first(where: { $0.enabled && $0.id == highlightedAction }) else { return }
                dismissActions(restore: true)
                action.perform()
            },
            dismiss: { dismissActions(restore: $0) }
        )
    }

    private struct ActionItem: Identifiable {
        let id: String
        let title: String
        let detail: String
        let enabled: Bool
        let perform: () -> Void
    }

    private var actions: [ActionItem] {
        var items = [
            ActionItem(id: "focus-query", title: "Focus Query Editor", detail: "Return to SQL at the current cursor position", enabled: store.document != nil && store.navigationAvailable) { focusEditor() },
            ActionItem(id: "focus-results", title: "Focus Results", detail: "Navigate result rows and cells", enabled: store.document != nil && store.navigationAvailable && store.results.isFocusable) { store.results.requestFocus() },
            ActionItem(id: "connections", title: "Show Connections", detail: "Choose a database connection", enabled: store.navigationAvailable) { Task { await store.chooseConnections() } },
            ActionItem(id: "history", title: "Query History", detail: "Show saved queries on the left", enabled: true) { store.historyVisible = true },
            ActionItem(id: "new", title: "New Query Tab", detail: "Choose a connection for a new query", enabled: store.navigationAvailable) { Task { await store.newDocument() } },
            ActionItem(id: "run", title: store.controls?.primaryAction == "stop" ? "Stop Query" : "Run Query", detail: "Run or cancel the current query", enabled: store.controls?.primaryAction == "run" || store.controls?.primaryAction == "stop") { store.sendEditorIntent(.primary) },
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
        return items.filter { actionSearch.isEmpty || "\($0.title) \($0.detail)".localizedCaseInsensitiveContains(actionSearch) }
    }

    private var actionMenuHeight: CGFloat { min(360, CGFloat(max(1, actions.count)) * 48 + 16) }

    private var highlightedAction: String? {
        let enabled = actions.filter(\.enabled)
        return enabled.first(where: { $0.id == selectedAction })?.id ?? enabled.first?.id
    }

    private func dismissActions(restore: Bool) {
        actionsVisible = false
        actionSearch = ""
        selectedAction = nil
        if restore { restoreFocus() }
    }

    private var actionMenu: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    ForEach(actions) { action in
                        Button { dismissActions(restore: true); action.perform() } label: {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(action.title)
                                Text(action.detail).font(.caption).foregroundStyle(.secondary)
                            }.frame(maxWidth: .infinity, alignment: .leading).padding(7)
                            .contentShape(Rectangle())
                            .background(highlightedAction == action.id ? Color.accentColor.opacity(0.15) : .clear, in: RoundedRectangle(cornerRadius: 5))
                        }
                        .buttonStyle(.plain)
                        .disabled(!action.enabled)
                        .accessibilityIdentifier("action.\(action.id)")
                        .accessibilityAddTraits(highlightedAction == action.id ? .isSelected : [])
                        .id(action.id)
                    }
                    if actions.isEmpty { Text("No matching actions").foregroundStyle(.secondary).padding(7).accessibilityIdentifier("actions.empty") }
                }.padding(8)
            }
            .onChange(of: highlightedAction) { _, id in
                if let id { proxy.scrollTo(id) }
            }
        }
    }

    private func backend(_ profile: String) -> String {
        store.profiles.first { $0.name == profile }?.backend ?? "postgresql"
    }

    private var connectionTabs: some View {
        HStack(spacing: 6) {
            ScrollView(.horizontal) {
                HStack(spacing: 6) {
                    ForEach(store.pages) { page in
                        let workspace = store.workspaces.first { $0.id == page.session }
                        let title = workspace.map { "\($0.profile) - \($0.documents.first?.title ?? "Connecting")" } ?? page.savedProfile.map { "\($0) - Reconnect" } ?? "New Query"
                        HStack(spacing: 6) {
                            Button { Task { await store.selectPage(page.id) } } label: {
                                HStack(spacing: 2) {
                                    if let workspace { DatabaseArtwork(backend: backend(workspace.profile)) }
                                    else { Image(systemName: "network").frame(width: 18, height: 18) }
                                    Text(title).lineLimit(1)
                                }
                            }.accessibilityLabel(title)
                            .accessibilityIdentifier(workspace.map { "workspace.\($0.id)" } ?? "tab.page.\(page.id)")
                            ZStack {
                                if let workspace, workspace.documents.contains(where: { ["planning", "running", "canceling"].contains($0.execution) }) {
                                    ProgressView().controlSize(.mini).accessibilityLabel("Query running")
                                        .accessibilityIdentifier("tab.activity.\(workspace.id)")
                                } else if let workspace, workspace.hasUnseenResult {
                                    QueryCompletionIndicator(failed: workspace.unseenResultFailed, revision: workspace.completionRevision)
                                        .accessibilityIdentifier("tab.unread.\(workspace.id)")
                                }
                            }.frame(width: 12, height: 12)
                            Button { Task { await store.closePage(page.id) } } label: {
                                Image(systemName: "xmark").font(.caption).frame(width: 18, height: 18)
                            }.accessibilityLabel("Close \(workspace?.documents.first?.title ?? "New Query")")
                                .accessibilityIdentifier(workspace.map { "connection.close.\($0.id)" } ?? "tab.close.\(page.id)")
                        }
                        .padding(.horizontal, 4).padding(.vertical, 5)
                        .background(store.selectedPage == page.id ? Color.accentColor.opacity(0.15) : .clear, in: RoundedRectangle(cornerRadius: 6))
                        .disabled(page.session.map { store.closingSessions.contains($0) } ?? false)
                    }
                }
            }

            newTabButton

        }
        .padding(.leading, WorkspaceLayout.outerInset).padding(.trailing, WorkspaceLayout.textInset - 8).padding(.vertical, 6)
        .frame(height: 40)
        .buttonStyle(.borderless)
        .disabled(!store.navigationAvailable)
        .accessibilityElement(children: .contain).accessibilityIdentifier("query.tabs")
    }

    private var statusbar: some View {
        let profile = store.workspace?.profile ?? store.pendingProfile
        let status = store.selectedSession.flatMap { store.health[$0] } ?? profile.flatMap { store.profileHealth[$0] }
        let state: ConnectionState = store.pendingProfile != nil && store.selectedSession == nil ? .connecting : status?.state ?? .disconnected
        let name = profile ?? "No database"
        let connectionText = state == .error ? "\(name): \(status?.failure ?? "connection failed")" : name
        return HStack(spacing: 12) {
            HStack(spacing: 4) {
                Circle().fill(state == .connected ? Color.green : state == .error ? Color.red : state == .connecting ? Color.orange : Color.secondary)
                    .frame(width: 7, height: 7)
                    .accessibilityLabel(state.label)
                Text(connectionText).lineLimit(1).truncationMode(.middle).frame(maxWidth: 200, alignment: .leading).fixedSize(horizontal: true, vertical: false)
            }.help(state.label + ": " + connectionText).accessibilityElement(children: .combine).accessibilityIdentifier("connection.status")
            Divider().frame(height: 14)
            HStack(spacing: 5) {
                FooterArtwork(name: "dzban-query-time-symbolic")
                Text(store.document?.executionSeconds.map { String(format: "%.2fs", $0) } ?? "--").accessibilityIdentifier("status.duration")
            }.fixedSize().help("Query execution time")
            Divider().frame(height: 14)
            ResultFooter(model: store.results, cell: store.cellInspector, document: store.document, bridge: store.bridge)
            Spacer(minLength: 16)
            Button { if let session = store.selectedSession { Task { await store.probe(session: session) } } } label: {
                HStack(spacing: 5) {
                    if status?.probing == true { ProgressView().controlSize(.small) }
                    else { FooterArtwork(name: "dzban-ping-symbolic") }
                    Text(ConnectionHealth.latency(status?.latencyMicros)).accessibilityIdentifier("status.latency")
                }
            }.buttonStyle(.plain).disabled(store.selectedSession == nil || !store.navigationAvailable || status?.probing == true || store.controls?.primaryAction == "stop" || plan.loading)
                .help("Database ping latency. Click to probe now.").accessibilityIdentifier("connection.probe").accessibilityValue(ConnectionHealth.latency(status?.latencyMicros)).frame(width: 86, alignment: .trailing)
            Divider().frame(height: 14)
            HStack(spacing: 5) {
                if let profile { DatabaseArtwork(backend: backend(profile)) }
                Text(status?.version.map { $0.split(whereSeparator: \.isWhitespace).prefix(2).joined(separator: " ") } ?? "--").lineLimit(1).accessibilityIdentifier("status.version")
            }.fixedSize().help(status?.version ?? "Database server version")
            Button { store.shortcutsVisible = true } label: { Image(systemName: "keyboard") }
                .help("Keyboard Shortcuts (⌘?)").accessibilityLabel("Keyboard Shortcuts").accessibilityIdentifier("shortcuts.open")
        }.font(.caption).monospacedDigit().buttonStyle(.borderless).frame(height: 30).padding(.leading, WorkspaceLayout.outerInset).padding(.trailing, WorkspaceLayout.textInset)
        .accessibilityElement(children: .contain).accessibilityIdentifier("workspace.statusbar")
    }

    private var chooser: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(spacing: 6) {
                    HStack {
                        Text("Connections").font(.headline)
                        Spacer()
                        Button("Test All") { Task { await store.probeAllProfiles() } }
                            .disabled(!store.navigationAvailable).accessibilityIdentifier("connections.probe-all")
                    }.padding(.bottom, 4)
                    if store.profiles.isEmpty {
                        Text("No connection profiles found").foregroundStyle(.secondary).accessibilityIdentifier("connections.empty")
                        Button("Reload Profiles") { Task { await store.loadProfiles() } }
                    } else {
                        ForEach(store.profiles) { profile in
                            HStack(spacing: 12) {
                                Button { selectedProfile = profile.name; Task { await store.connect(profile: profile.name) } } label: {
                                    HStack {
                                        DatabaseArtwork(backend: backend(profile.name))
                                        Text(profile.name).lineLimit(1)
                                        Spacer(minLength: 0)
                                    }
                                    .frame(maxWidth: .infinity, alignment: .leading).frame(height: 34)
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .disabled(store.pendingProfile != nil || !store.navigationAvailable)
                                .focusable(false)
                                .focusEffectDisabled()
                                .accessibilityIdentifier("connection.\(profile.name)")
                                .accessibilityLabel("Connect to \(profile.name)")
                                HStack(spacing: 6) {
                                    ZStack {
                                        if store.profileHealth[profile.name]?.probing == true || store.pendingProfile == profile.name {
                                            ProgressView().controlSize(.mini)
                                        }
                                    }.frame(width: 16, height: 16)
                                    if let health = store.profileHealth[profile.name], !health.probing {
                                        Text(health.failure ?? "\(health.state.label) - \(ConnectionHealth.latency(health.latencyMicros))")
                                            .foregroundStyle(health.state == .error ? Color.red : Color.primary)
                                            .lineLimit(1).help(health.failure ?? health.version ?? health.state.label)
                                            .accessibilityIdentifier("connection.health.\(profile.name)")
                                    }
                                }.font(.callout).frame(width: 180, alignment: .trailing)
                                Button("Open") { selectedProfile = profile.name; Task { await store.connect(profile: profile.name) } }
                                    .disabled(store.pendingProfile != nil || !store.navigationAvailable)
                                    .accessibilityIdentifier("connection.open.\(profile.name)")
                                Button("Test") { selectedProfile = profile.name; Task { await store.probeProfile(profile.name) } }
                                    .disabled(store.profileHealth[profile.name]?.probing == true || !store.navigationAvailable)
                                    .accessibilityIdentifier("connection.probe.\(profile.name)")
                            }.padding(.horizontal, 8).frame(height: 38)
                                .frame(maxWidth: .infinity)
                                .focusable()
                                .focusEffectDisabled()
                                .focused($focusedProfile, equals: profile.name)
                                .onKeyPress(.return) {
                                    guard store.pendingProfile == nil, store.navigationAvailable else { return .ignored }
                                    Task { await store.connect(profile: profile.name) }
                                    return .handled
                                }
                                .onKeyPress(.space) {
                                    guard store.pendingProfile == nil, store.navigationAvailable else { return .handled }
                                    Task { await store.probeProfile(profile.name) }
                                    return .handled
                                }
                                .onKeyPress(.downArrow) { moveConnection(1); return .handled }
                                .onKeyPress(.upArrow) { moveConnection(-1); return .handled }
                                .background(selectedProfile == profile.name ? Color.accentColor.opacity(0.12) : Color.clear, in: RoundedRectangle(cornerRadius: 4))
                                .overlay(RoundedRectangle(cornerRadius: 4).stroke(selectedProfile == profile.name ? Color.accentColor : Color.clear, lineWidth: 1.5))
                                .accessibilityElement(children: .contain)
                                .accessibilityIdentifier("connection.row.\(profile.name)")
                                .accessibilityLabel(profile.name)
                                .accessibilityAddTraits(selectedProfile == profile.name ? .isSelected : [])
                                .id(profile.name)
                            Divider()
                        }
                    }
                }
                .padding(12)
            }
            .onChange(of: focusedProfile) { _, profile in
                if let profile { selectedProfile = profile; proxy.scrollTo(profile, anchor: .center) }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("connections.chooser")
        .onAppear { focusConnection() }
        .onKeyPress(.escape) { store.cancelConnectionPicker(); return .handled }
    }

    private func workspaceView(_ workspace: Workspace) -> some View {
        VStack(spacing: 0) {
            if let document = store.document {
                VStack(alignment: .leading, spacing: 0) {
                    VSplitView {
                        SQLEditor(buffer: store.editor(for: document), editable: store.currentTabAvailable, changed: { [weak store] in
                            store?.editorChanged(session: workspace.id, document: document.document)
                        }, contextChanged: { [weak store] in
                            store?.editorContextChanged(session: workspace.id, document: document.document)
                        }, action: { [weak store] intent in
                            guard let store else { return false }
                            if intent == .cancel && store.controls?.primaryAction != "stop" { return false }
                            store.sendEditorIntent(intent, session: workspace.id, document: document.document)
                            return true
                        })
                        .id(document.document)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .frame(minHeight: 120, idealHeight: 220)
                        ResultPanel(model: store.results, cell: store.cellInspector, row: store.rowInspector, bridge: store.bridge, session: workspace.id, document: document, cancel: { [weak store] in
                            guard let store, store.controls?.primaryAction == "stop" else { return false }
                            store.sendEditorIntent(.cancel)
                            return true
                        })
                        .frame(minHeight: 160)
                    }

                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("query.document")
            } else {
                ContentUnavailableView("No query selected", systemImage: "doc.text")
            }
        }
    }
}

private struct QueryCompletionIndicator: View {
    let failed: Bool
    let revision: UInt64
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pulsing = false

    var body: some View {
        Image(systemName: failed ? "exclamationmark.circle.fill" : "circle.fill")
            .font(.system(size: failed ? 10 : 7))
            .foregroundStyle(failed ? Color.red : Color.accentColor)
            .scaleEffect(pulsing ? 1.35 : 1)
            .accessibilityLabel(failed ? "Query failed" : "New result")
            .help(failed ? "Query failed - select tab to view" : "Query completed - select tab to view")
            .task(id: revision) {
                guard !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 0.22).repeatCount(4, autoreverses: true)) { pulsing = true }
                try? await Task.sleep(for: .milliseconds(900))
                guard !Task.isCancelled else { return }
                withTransaction(Transaction(animation: nil)) { pulsing = false }
            }
    }
}
