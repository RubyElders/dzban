import DzbanBridge
import SwiftUI

public struct RootView: View {
    @ObservedObject var store: ApplicationStore
    private enum WorkspacePanel: Identifiable {
        case catalog(UUID), history(UUID)
        var id: String { switch self { case .catalog(let id): return "catalog-\(id)"; case .history(let id): return "history-\(id)" } }
    }
    @State private var panel: WorkspacePanel?

    public init(store: ApplicationStore) { self.store = store }

    public var body: some View {
        NavigationSplitView {
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
            .navigationSplitViewColumnWidth(min: 190, ideal: 230)
            .disabled(!store.navigationAvailable)
        } detail: {
            if let workspace = store.workspace {
                workspaceView(workspace)
            } else {
                chooser
            }
        }
        .frame(minWidth: 760, minHeight: 500)
        .background(WindowPolicy(store: store).frame(width: 0, height: 0))
        .toolbar {
            if store.closingSession != nil || store.isShuttingDown {
                ProgressView().controlSize(.small).accessibilityLabel("Closing connections")
            }
            Button(store.controls?.primaryAction == "stop" ? "Stop" : store.controls?.primaryAction == "stopping" ? "Stopping" : "Run") {
                store.sendEditorIntent(.primary)
            }
            .disabled(!store.navigationAvailable || store.controls?.primaryAction == "unavailable" || store.controls?.primaryAction == "stopping" || store.selectedSession == nil)
            .accessibilityIdentifier("query.primary")
            PlanPreviewButton(store: store)
            Button("Catalog") { if let session = store.selectedSession { panel = .catalog(session) } }
                .disabled(store.selectedSession == nil || !store.navigationAvailable)
                .accessibilityIdentifier("catalog.open")
            Button("History") { if let session = store.selectedSession { panel = .history(session) } }
                .disabled(store.selectedSession == nil || !store.navigationAvailable)
                .accessibilityIdentifier("history.open")
            Button { Task { await store.newDocument() } } label: {
                Label("New Query", systemImage: "plus")
            }
            .disabled(store.selectedSession == nil || !store.navigationAvailable)
            .accessibilityIdentifier("query.new")
        }
        .sheet(item: $panel) { panel in
            switch panel {
            case .catalog(let session): CatalogBrowser(bridge: store.bridge, session: session)
            case .history(let session): HistoryBrowser(store: store, session: session)
            }
        }
        .alert("Dzban", isPresented: Binding(get: { store.failure != nil }, set: { if !$0 { store.failure = nil } })) {
            Button("OK") { store.failure = nil }
        } message: { Text(store.failure ?? "") }
        .task { await store.loadProfiles() }
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
        .navigationTitle(workspace.profile)
    }
}
