import AppKit
import DzbanBridge
import SwiftUI

struct WorkspaceStatusBar: View {
    @ObservedObject var store: ApplicationStore
    @ObservedObject private var plan: PlanPreviewModel

    init(store: ApplicationStore) { self.store = store; plan = store.plan }

    private func backend(_ profile: String) -> String {
        store.profiles.first { $0.name == profile }?.backend ?? "postgresql"
    }

    var body: some View {
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
            ResultFooter(model: store.results, cell: store.cellInspector, document: store.document, application: store.application)
            Spacer(minLength: 16)
            Button { if let session = store.selectedSession { Task { await store.probe(session: session) } } } label: {
                HStack(spacing: 5) {
                    if status?.probing == true { ProgressView().controlSize(.small) }
                    else { FooterArtwork(name: "dzban-ping-symbolic") }
                    Text(ConnectionHealth.latency(status?.latencyMicros)).accessibilityIdentifier("status.latency")
                }
            }.buttonStyle(.plain).disabled(store.selectedSession == nil || !store.navigationAvailable || status?.probing == true || store.controls?.primaryAction == .stop || plan.loading)
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

}
