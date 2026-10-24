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
    @StateObject private var actions: WindowActionsModel
    @FocusState private var focusedProfile: String?
    @State private var selectedProfile: String?

    public init(store: ApplicationStore) { self.store = store; plan = store.plan; _actions = StateObject(wrappedValue: WindowActionsModel(store: store)) }

    public var body: some View {
        VStack(spacing: 0) {
            if !store.restoring { QueryTabStrip(store: store) }
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
                    if store.restoring {
                        ProgressView("Restoring workspace").accessibilityIdentifier("workspace.restoring")
                    } else if let workspace = store.workspace { workspaceView(workspace) }
                    else { ConnectionChooser(store: store, focusedProfile: $focusedProfile, selectedProfile: $selectedProfile, focusConnection: focusConnection, moveConnection: moveConnection) }
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
            Divider()
            WorkspaceStatusBar(store: store)
        }
        .frame(minWidth: 900, maxWidth: .infinity, minHeight: 500, maxHeight: .infinity)
        .background(WindowPolicy(store: store, toolbar: actions.configuration(restoreFocus: restoreFocus)).frame(width: 0, height: 0))
        .overlay {
            if actions.visible {
                GeometryReader { geometry in
                    ZStack(alignment: .top) {
                        Color.clear.allowsHitTesting(false)
                        WindowActionMenu(model: actions, restoreFocus: restoreFocus)
                            .frame(width: min(600, max(380, geometry.size.width - 320)), height: min(actions.height, geometry.size.height - 16))
                            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
                            .overlay(RoundedRectangle(cornerRadius: 10).stroke(.quaternary))
                            .shadow(radius: 12, y: 5)
                            .padding(.top, 8)
                    }.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                }
            }
        }
        .overlay(alignment: .bottom) {
            if let notice = store.recoveryNotice {
                HStack {
                    Text(notice).accessibilityIdentifier("connection.recovery-notice")
                    Button("Dismiss") { store.recoveryNotice = nil }.accessibilityIdentifier("connection.recovery-dismiss")
                }.padding(12).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8)).padding(.horizontal, 20).padding(.bottom, 42)
            }
        }
        .sheet(isPresented: $store.shortcutsVisible, onDismiss: restoreFocus) { NativeShortcutReference() }
        .alert("Dzban", isPresented: Binding(get: { store.failure != nil }, set: { if !$0 { store.failure = nil } })) {
            Button("OK") { store.failure = nil }
        } message: { Text(store.failure ?? "") }
        .task { await store.start(); await Task.yield(); restoreFocus() }
        .onChange(of: store.selectedPage) { _, _ in
            actions.dismiss()
            if store.connectionsVisible { focusConnection() }
        }
        .onChange(of: store.selectedSession) { _, session in
            if session == nil { focusConnection() }
        }
        .onChange(of: store.isClosed) { _, closed in if closed { NSApp.terminate(nil) } }
        .onChange(of: store.isShuttingDown) { _, closing in
            if closing { store.shortcutsVisible = false; actions.visible = false }
        }
        .onChange(of: store.historyVisible) { _, visible in if !visible { restoreFocus() } }
        .onChange(of: plan.visible) { _, visible in if !visible { restoreFocus() } }
        .onChange(of: store.document?.document) { _, _ in focusEditor() }
        .onChange(of: store.failure) { _, value in if value == nil { restoreFocus() } }
    }

    private func focusConnection() {
        guard !store.restoring, store.connectionsVisible || store.selectedSession == nil else { return }
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
                            if intent == .cancel && store.controls?.primaryAction != .stop { return false }
                            store.sendEditorIntent(intent, session: workspace.id, document: document.document)
                            return true
                        })
                        .id(document.document)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .frame(minHeight: 120, idealHeight: 220)
                        ResultPanel(model: store.results, cell: store.cellInspector, row: store.rowInspector, session: workspace.id, document: document, cancel: { [weak store] in
                            guard let store, store.controls?.primaryAction == .stop else { return false }
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
