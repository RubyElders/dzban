import DzbanBridge
import SwiftUI

public struct RootView: View {
    @ObservedObject var store: ApplicationStore
    @ObservedObject private var plan: PlanPreviewModel
    private enum WorkspacePanel: Identifiable {
        case catalog(UUID)
        var id: String { switch self { case .catalog(let id): return "catalog-\(id)" } }
    }
    @State private var panel: WorkspacePanel?
    @State private var actionsVisible = false
    @State private var actionSearch = ""
    @State private var selectedAction = 0

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
                ZStack(alignment: .top) {
                    Color.clear.contentShape(Rectangle()).onTapGesture { actionsVisible = false }
                    actionMenu
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10).stroke(.quaternary))
                        .shadow(radius: 12, y: 5)
                        .padding(.top, 8)
                }
            }
        }
        .sheet(isPresented: $store.shortcutsVisible) { NativeShortcutReference() }
        .sheet(item: $panel) { panel in
            switch panel {
            case .catalog(let session): CatalogBrowser(bridge: store.bridge, session: session)
            }
        }
        .alert("Dzban", isPresented: Binding(get: { store.failure != nil }, set: { if !$0 { store.failure = nil } })) {
            Button("OK") { store.failure = nil }
        } message: { Text(store.failure ?? "") }
        .task { await store.loadProfiles() }
    }

    private var actionPlaceholder: String {
        if store.isShuttingDown { return "Closing connections..." }
        if store.controls?.primaryAction == "stopping" { return "Canceling query..." }
        if store.controls?.primaryAction == "stop" { return store.document?.execution == "planning" ? "Planning query..." : "Query running..." }
        if let hint = store.sharedInput?.hint {
            switch hint.kind {
            case "primary_guide": return "Enter: run - Shift: estimate - Shift+Enter: estimate and run"
            case "estimating": return "Enter: run - Space: plan - estimating rows and cost..."
            case "estimate_ready": return hint.plan.map { "Enter: run - Space: plan - " + PlanFormatting.compact($0) } ?? "Enter: run | Space: plan"
            case "estimate_error": return hint.error ?? "Estimate failed"
            case "planning": return "Planning query..."
            case "running": return "Query running..."
            case "canceling": return "Canceling query..."
            default: break
            }
        }
        if plan.loading { return "Estimating query..." }
        if plan.visible, let value = plan.plan { return PlanFormatting.compact(value) }
        return store.selectedSession == nil ? "Search actions..." : "Hold Cmd+Shift: estimate | Enter: run"
    }

    private var toolbar: NativeToolbarConfiguration {
        NativeToolbarConfiguration(
            history: AnyView(Button { store.historyVisible.toggle() } label: {
                Label("Query History", systemImage: "sidebar.left")
            }.labelStyle(.iconOnly).help("Toggle query history").accessibilityIdentifier("history.open")),
            plan: AnyView(PlanPreviewButton(store: store)),
            newQuery: AnyView(Button { Task { await store.newDocument() } } label: {
                Label("New Query", systemImage: "plus")
            }.labelStyle(.iconOnly).disabled(store.selectedSession == nil || !store.navigationAvailable).accessibilityIdentifier("query.new")),
            primary: AnyView(Button { store.sendEditorIntent(.primary) } label: {
                Label(store.controls?.primaryAction == "stop" ? "Stop" : store.controls?.primaryAction == "stopping" ? "Stopping" : "Run", systemImage: store.controls?.primaryAction == "stop" ? "stop.fill" : store.controls?.primaryAction == "stopping" ? "hourglass" : "play.fill")
            }.labelStyle(.iconOnly).help("Run: Command-Return. Cancel: Command-period or Escape.")
                .disabled(!store.navigationAvailable || store.controls?.primaryAction == "unavailable" || store.controls?.primaryAction == "stopping" || store.selectedSession == nil).accessibilityIdentifier("query.primary")),
            placeholder: actionPlaceholder,
            busy: plan.loading || store.isShuttingDown || store.controls?.primaryAction == "stop" || store.controls?.primaryAction == "stopping",
            shortcuts: { store.shortcutsVisible = true },
            search: { text in actionSearch = text; selectedAction = 0; actionsVisible = true },
            move: { direction in
                let enabled = actions.filter(\.enabled)
                guard !enabled.isEmpty else { return }
                selectedAction = min(max(0, selectedAction + direction), enabled.count - 1)
                actionsVisible = true
            },
            submit: {
                let enabled = actions.filter(\.enabled)
                guard enabled.indices.contains(selectedAction) else { return }
                actionsVisible = false
                enabled[selectedAction].perform()
            },
            dismiss: { actionsVisible = false }
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
            ActionItem(id: "connections", title: "Show Connections", detail: "Choose a database connection", enabled: store.navigationAvailable) { Task { await store.chooseConnections() } },
            ActionItem(id: "history", title: "Query History", detail: "Show saved queries on the left", enabled: true) { store.historyVisible = true },
            ActionItem(id: "new", title: "New Query", detail: "Open a query tab", enabled: store.selectedSession != nil && store.navigationAvailable) { Task { await store.newDocument() } },
            ActionItem(id: "run", title: store.controls?.primaryAction == "stop" ? "Stop Query" : "Run Query", detail: "Run or cancel the current query", enabled: store.controls?.primaryAction == "run" || store.controls?.primaryAction == "stop") { store.sendEditorIntent(.primary) },
            ActionItem(id: "plan", title: "Preview Query Plan", detail: "Estimate the current query", enabled: store.canPreview) { Task { await store.previewCurrent(pinned: true) } },
            ActionItem(id: "catalog", title: "Database Catalog", detail: "Browse tables, columns, and database objects", enabled: store.selectedSession != nil && store.navigationAvailable) { if let session = store.selectedSession { panel = .catalog(session) } },
            ActionItem(id: "disconnect", title: "Disconnect", detail: "Close the current workspace", enabled: store.selectedSession != nil && store.navigationAvailable) { if let session = store.selectedSession { Task { await store.closeSession(session) } } }
        ]
        items += [
            ActionItem(id: "probe", title: "Test All Connections", detail: "Probe configured servers and measure delay", enabled: store.navigationAvailable) { Task { await store.probeAllProfiles() } },
            ActionItem(id: "shortcuts", title: "Keyboard Shortcuts", detail: "Show native keyboard controls", enabled: true) { store.shortcutsVisible = true },
            ActionItem(id: "close-query", title: "Close Query Tab", detail: "Close the selected query", enabled: store.document != nil && store.navigationAvailable) { if let document = store.document { Task { await store.closeDocument(document.document) } } },
            ActionItem(id: "next-query", title: "Next Query Tab", detail: "Select the next query", enabled: store.document != nil && store.navigationAvailable) { Task { await store.cycleDocument(1) } },
            ActionItem(id: "previous-query", title: "Previous Query Tab", detail: "Select the previous query", enabled: store.document != nil && store.navigationAvailable) { Task { await store.cycleDocument(-1) } },
            ActionItem(id: "next-connection", title: "Next Connection", detail: "Select the next open database", enabled: !store.workspaces.isEmpty && store.navigationAvailable) { Task { await store.cycleWorkspace(1) } },
            ActionItem(id: "previous-connection", title: "Previous Connection", detail: "Select the previous open database", enabled: !store.workspaces.isEmpty && store.navigationAvailable) { Task { await store.cycleWorkspace(-1) } }
        ]
        items += store.profiles.map { profile in
            ActionItem(id: "connect.\(profile.name)", title: profile.name, detail: "Open database connection", enabled: store.pendingProfile == nil && store.navigationAvailable) { Task { await store.connect(profile: profile.name) } }
        }
        items += store.workspaces.map { workspace in
            ActionItem(id: "workspace.\(workspace.id)", title: workspace.profile, detail: "Select open workspace", enabled: store.navigationAvailable) { Task { await store.selectSession(workspace.id) } }
        }
        if let workspace = store.workspace {
            items += workspace.documents.map { document in
                ActionItem(id: "query.\(document.document)", title: document.title, detail: "Select query tab", enabled: store.navigationAvailable) { Task { await store.selectDocument(document.document) } }
            }
        }
        return items.filter { actionSearch.isEmpty || "\($0.title) \($0.detail)".localizedCaseInsensitiveContains(actionSearch) }
    }

    private var actionMenu: some View {
        VStack(spacing: 10) {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    ForEach(actions) { action in
                        Button { actionsVisible = false; action.perform() } label: {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(action.title)
                                Text(action.detail).font(.caption).foregroundStyle(.secondary)
                            }.frame(maxWidth: .infinity, alignment: .leading).padding(7)
                            .contentShape(Rectangle())
                            .background(actions.filter(\.enabled).indices.contains(selectedAction) && actions.filter(\.enabled)[selectedAction].id == action.id ? Color.accentColor.opacity(0.15) : .clear, in: RoundedRectangle(cornerRadius: 5))
                        }
                        .buttonStyle(.plain)
                        .disabled(!action.enabled)
                        .accessibilityIdentifier("action.\(action.id)")
                    }
                    if actions.isEmpty { Text("No matching actions").foregroundStyle(.secondary).padding(7) }
                }
            }
        }.padding(12).frame(width: 380, height: 360)
    }

    private func backend(_ profile: String) -> String {
        store.profiles.first { $0.name == profile }?.backend ?? "postgresql"
    }

    private var connectionTabs: some View {
        HStack(spacing: 6) {
            Button { Task { await store.chooseConnections() } } label: {
                Label("Connections", systemImage: "externaldrive.connected.to.line.below")
            }.accessibilityIdentifier("connections.show")
            ScrollView(.horizontal) {
                HStack(spacing: 6) {
                    ForEach(store.workspaces) { workspace in
                        HStack(spacing: 6) {
                            Button { Task { await store.selectSession(workspace.id) } } label: {
                                HStack { DatabaseArtwork(backend: backend(workspace.profile)); Text(workspace.profile) }
                            }.accessibilityIdentifier("workspace.\(workspace.id)")
                            Button { Task { await store.closeSession(workspace.id) } } label: {
                                Image(systemName: "xmark").font(.caption)
                            }.accessibilityLabel("Disconnect \(workspace.profile)").accessibilityIdentifier("connection.close.\(workspace.id)")
                        }
                        .padding(5)
                        .background(store.selectedSession == workspace.id ? Color.accentColor.opacity(0.15) : .clear, in: RoundedRectangle(cornerRadius: 6))
                    }
                }
            }
            Menu {
                ForEach(store.profiles) { profile in
                    Button(profile.name) { Task { await store.connect(profile: profile.name) } }
                }
            } label: { Label("New Connection", systemImage: "plus") }
            .menuStyle(.borderlessButton).fixedSize().accessibilityIdentifier("connection.new")
            .disabled(store.pendingProfile != nil)
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
        .frame(height: 40)
        .buttonStyle(.borderless)
        .disabled(!store.navigationAvailable)
        .accessibilityElement(children: .contain).accessibilityIdentifier("connection.tabs")
    }

    private var statusbar: some View {
        let profile = store.workspace?.profile ?? store.pendingProfile
        let status = store.selectedSession.flatMap { store.health[$0] } ?? profile.flatMap { store.profileHealth[$0] }
        let state: ConnectionState = store.pendingProfile != nil ? .connecting : status?.state ?? .disconnected
        let name = profile ?? "No database"
        let connectionText = state == .error ? "\(name): \(status?.failure ?? "connection failed")" : name
        return HStack(spacing: 12) {
            HStack(spacing: 5) {
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
            HStack(spacing: 5) {
                FooterArtwork(name: "dzban-result-rows-symbolic")
                Text(store.document.flatMap { $0.generation > 0 && $0.commandResult == nil ? "\($0.rows) rows" : nil } ?? "-- rows").accessibilityIdentifier("status.rows")
            }.fixedSize().help("Returned rows")
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
                .help("Keyboard Shortcuts (F1)").accessibilityLabel("Keyboard Shortcuts").accessibilityIdentifier("shortcuts.open")
        }.font(.caption).monospacedDigit().buttonStyle(.borderless).frame(height: 30).padding(.horizontal, 12)
        .accessibilityElement(children: .contain).accessibilityIdentifier("workspace.statusbar")
    }

    private var chooser: some View {
        VStack(spacing: 20) {
            Image(systemName: "externaldrive.connected.to.line.below").font(.system(size: 44)).foregroundStyle(.secondary)
            Text("Connections").font(.largeTitle)
            if store.profiles.isEmpty {
                Text("No connection profiles found").foregroundStyle(.secondary).accessibilityIdentifier("connections.empty")
                Button("Reload Profiles") { Task { await store.loadProfiles() } }
            } else {
                ForEach(store.profiles) { profile in
                    HStack(spacing: 12) {
                        Button { Task { await store.connect(profile: profile.name) } } label: {
                            HStack {
                                DatabaseArtwork(backend: backend(profile.name))
                                VStack(alignment: .leading) {
                                    Text(profile.name).font(.headline)
                                    Text(profile.backend == "postgresql" ? "PostgreSQL" : "ClickHouse").font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                if store.pendingProfile == profile.name { ProgressView().controlSize(.small) }
                                else { Image(systemName: "chevron.right") }
                            }
                            .padding(8)
                            .frame(width: 320)
                        }
                        .disabled(store.pendingProfile != nil || !store.navigationAvailable)
                        .accessibilityIdentifier("connection.\(profile.name)")
                        .accessibilityLabel("Connect to \(profile.name)")
                        VStack(alignment: .leading, spacing: 5) {
                            Button("Probe") { Task { await store.probeProfile(profile.name) } }
                                .disabled(store.profileHealth[profile.name]?.probing == true || !store.navigationAvailable)
                                .accessibilityIdentifier("connection.probe.\(profile.name)")
                            if let health = store.profileHealth[profile.name] {
                                if health.probing { ProgressView().controlSize(.small) }
                                else {
                                    Text(health.failure ?? "\(health.state.label) - \(ConnectionHealth.latency(health.latencyMicros))")
                                        .font(.caption).foregroundStyle(health.state == .error ? Color.red : .secondary)
                                        .lineLimit(2).help(health.failure ?? health.version ?? health.state.label)
                                        .accessibilityIdentifier("connection.health.\(profile.name)")
                                }
                            }
                        }.frame(width: 170, alignment: .leading)
                    }
                }
                Button("Test All Connections") { Task { await store.probeAllProfiles() } }
                    .disabled(!store.navigationAvailable).accessibilityIdentifier("connections.probe-all")
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("connections.chooser")
    }

    private func workspaceView(_ workspace: Workspace) -> some View {
        VStack(spacing: 0) {
            ScrollView(.horizontal) {
                HStack(spacing: 4) {
                    ForEach(workspace.documents, id: \.document) { document in
                        HStack(spacing: 8) {
                            Button(document.title) { Task { await store.selectDocument(document.document) } }
                                .accessibilityIdentifier("query.select.\(document.document)")
                            Button { Task { await store.closeDocument(document.document) } } label: {
                                Image(systemName: "xmark").font(.caption)
                            }
                            .accessibilityLabel("Close \(document.title)")
                            .accessibilityIdentifier("query.close.\(document.document)")
                        }
                        .buttonStyle(.borderless)
                        .padding(7)
                        .background(workspace.selectedDocument == document.document ? Color.accentColor.opacity(0.15) : Color.clear, in: RoundedRectangle(cornerRadius: 6))
                    }
                }.padding(8)
            }
            .disabled(!store.navigationAvailable)
            Divider()
            if let document = store.document {
                VStack(alignment: .leading, spacing: 10) {
                    VSplitView {
                        SQLEditor(buffer: store.editor(for: document), editable: store.navigationAvailable, changed: { [weak store] in
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
                        .frame(minHeight: 120, idealHeight: 220)
                        ResultPanel(model: store.results, bridge: store.bridge, session: workspace.id, document: document, cancel: { [weak store] in
                            guard let store, store.controls?.primaryAction == "stop" else { return false }
                            store.sendEditorIntent(.cancel)
                            return true
                        })
                        .frame(minHeight: 160)
                    }
                    if let error = document.error, error != .null {
                        Text(QueryDiagnostic.describe(error))
                            .foregroundStyle(.red)
                            .textSelection(.enabled)
                            .accessibilityIdentifier("query.error")
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("query.document")
            } else {
                ContentUnavailableView("No query selected", systemImage: "doc.text")
            }
        }
    }
}
