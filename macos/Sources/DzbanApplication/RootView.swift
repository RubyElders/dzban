import DzbanBridge
import SwiftUI

public struct RootView: View {
    @ObservedObject var store: ApplicationStore
    private enum WorkspacePanel: Identifiable {
        case catalog(UUID)
        var id: String { switch self { case .catalog(let id): return "catalog-\(id)" } }
    }
    @State private var panel: WorkspacePanel?
    @State private var columns: NavigationSplitViewVisibility = .all
    @State private var actionsVisible = false
    @State private var actionSearch = ""
    @State private var selectedAction = 0

    public init(store: ApplicationStore) { self.store = store }

    public var body: some View {
        VStack(spacing: 0) {
            NavigationSplitView(columnVisibility: $columns) {
                VStack(spacing: 0) {
                    List {
                        Button { Task { await store.chooseConnections() } } label: {
                            Label("Connections", systemImage: "externaldrive.connected.to.line.below")
                        }
                        .accessibilityIdentifier("connections.show")
                        Section("Workspaces") {
                            ForEach(store.workspaces) { workspace in
                                Button { Task { await store.selectSession(workspace.id) } } label: {
                                    HStack {
                                        Label(workspace.profile, systemImage: "externaldrive")
                                        Spacer()
                                        if store.selectedSession == workspace.id { Image(systemName: "checkmark") }
                                    }
                                }
                                .accessibilityIdentifier("workspace.\(workspace.id)")
                                .contextMenu {
                                    Button("Disconnect") { Task { await store.closeSession(workspace.id) } }
                                }
                            }
                        }
                    }
                    .listStyle(.sidebar)
                    .frame(maxHeight: store.selectedSession == nil ? .infinity : 145)
                    .disabled(!store.navigationAvailable)
                    if let session = store.selectedSession {
                        Divider()
                        HistoryBrowser(store: store, session: session) { columns = .detailOnly }
                            .id(session)
                    } else {
                        Divider()
                        ContentUnavailableView("Query History", systemImage: "clock", description: Text("Connect to a database to browse saved queries."))
                    }
                }
                .navigationSplitViewColumnWidth(min: 260, ideal: 300, max: 420)
                .toolbar(removing: .sidebarToggle)
            } detail: {
                if let workspace = store.workspace {
                    workspaceView(workspace)
                } else {
                    chooser
                }
            }
        }
        .frame(minWidth: 900, minHeight: 500)
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

    private var toolbar: NativeToolbarConfiguration {
        NativeToolbarConfiguration(
            history: AnyView(Button { columns = columns == .detailOnly ? .all : .detailOnly } label: {
                Label("Query History", systemImage: "sidebar.left")
            }.labelStyle(.iconOnly).help("Toggle query history").accessibilityIdentifier("history.open")),
            plan: AnyView(PlanPreviewButton(store: store)),
            newQuery: AnyView(Button { Task { await store.newDocument() } } label: {
                Label("New Query", systemImage: "plus")
            }.labelStyle(.iconOnly).disabled(store.selectedSession == nil || !store.navigationAvailable).accessibilityIdentifier("query.new")),
            primary: AnyView(Button(store.controls?.primaryAction == "stop" ? "Stop" : store.controls?.primaryAction == "stopping" ? "Stopping" : "Run") {
                store.sendEditorIntent(.primary)
            }.disabled(!store.navigationAvailable || store.controls?.primaryAction == "unavailable" || store.controls?.primaryAction == "stopping" || store.selectedSession == nil).accessibilityIdentifier("query.primary")),
            placeholder: store.isShuttingDown ? "Closing connections..." : store.controls?.primaryAction == "stopping" ? "Canceling query..." : store.controls?.primaryAction == "stop" ? "Query running..." : "Search actions...",
            busy: store.isShuttingDown || store.controls?.primaryAction == "stop" || store.controls?.primaryAction == "stopping",
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
            ActionItem(id: "history", title: "Query History", detail: "Show saved queries on the left", enabled: true) { columns = .all },
            ActionItem(id: "new", title: "New Query", detail: "Open a query tab", enabled: store.selectedSession != nil && store.navigationAvailable) { Task { await store.newDocument() } },
            ActionItem(id: "run", title: store.controls?.primaryAction == "stop" ? "Stop Query" : "Run Query", detail: "Run or cancel the current query", enabled: store.controls?.primaryAction == "run" || store.controls?.primaryAction == "stop") { store.sendEditorIntent(.primary) },
            ActionItem(id: "plan", title: "Preview Query Plan", detail: "Estimate the current query", enabled: store.canPreview) { Task { await store.previewCurrent(pinned: true) } },
            ActionItem(id: "catalog", title: "Database Catalog", detail: "Browse tables, columns, and database objects", enabled: store.selectedSession != nil && store.navigationAvailable) { if let session = store.selectedSession { panel = .catalog(session) } },
            ActionItem(id: "disconnect", title: "Disconnect", detail: "Close the current workspace", enabled: store.selectedSession != nil && store.navigationAvailable) { if let session = store.selectedSession { Task { await store.closeSession(session) } } }
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

    private var chooser: some View {
        VStack(spacing: 20) {
            Image(systemName: "externaldrive.connected.to.line.below").font(.system(size: 44)).foregroundStyle(.secondary)
            Text("Connections").font(.largeTitle)
            if store.profiles.isEmpty {
                Text("No connection profiles found").foregroundStyle(.secondary).accessibilityIdentifier("connections.empty")
                Button("Reload Profiles") { Task { await store.loadProfiles() } }
            } else {
                ForEach(store.profiles) { profile in
                    Button { Task { await store.connect(profile: profile.name) } } label: {
                        HStack {
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
                }
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
                        .padding(7)
                        .background(workspace.selectedDocument == document.document ? Color.accentColor.opacity(0.15) : Color.clear, in: RoundedRectangle(cornerRadius: 6))
                    }
                }.padding(8)
            }
            .disabled(!store.navigationAvailable)
            Divider()
            if let document = store.document {
                VStack(alignment: .leading, spacing: 14) {
                    Text(document.title).font(.title2)
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
                .padding(24)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("query.document")
            } else {
                ContentUnavailableView("No query selected", systemImage: "doc.text")
            }
            Divider()
            HStack {
                Text(workspace.profile)
                if let document = store.document {
                    Text(document.execution.capitalized).accessibilityIdentifier("query.execution")
                    if document.generation > 0 {
                        Text("Run \(document.generation)").accessibilityIdentifier("query.generation")
                    }
                }
                Spacer()
                Text(store.controls?.primaryAction == "run" ? "Ready" : "Connection active")
                    .accessibilityIdentifier("connection.status")
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .padding(8)
        }
    }
}
