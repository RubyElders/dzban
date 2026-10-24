import AppKit
import DzbanBridge
import SwiftUI

struct QueryTabStrip: View {
    @ObservedObject var store: ApplicationStore

    private var newTabButton: some View {
        Button { Task { await store.newDocument() } } label: {
            Label("New Tab", systemImage: "plus")
        }.labelStyle(.iconOnly).frame(width: 28, height: 28).help("New tab: choose a connection (⌘T)")
            .disabled(!store.navigationAvailable).accessibilityIdentifier("query.new")
    }

    private func backend(_ profile: String) -> String {
        store.profiles.first { $0.name == profile }?.backend ?? "postgresql"
    }

    var body: some View {
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
                            }.buttonStyle(.plain).accessibilityLabel(title)
                            .accessibilityIdentifier(workspace.map { "workspace.\($0.id)" } ?? "tab.page.\(page.id)")
                            .contextMenu {
                                Button("Move Tab Left") { if let index = store.pages.firstIndex(where: { $0.id == page.id }) { Task { await store.movePage(page.id, to: index - 1) } } }
                                    .disabled(store.pages.first?.id == page.id)
                                Button("Move Tab Right") { if let index = store.pages.firstIndex(where: { $0.id == page.id }) { Task { await store.movePage(page.id, to: index + 1) } } }
                                    .disabled(store.pages.last?.id == page.id)
                            }
                            ZStack {
                                if let workspace, workspace.documents.contains(where: { [ExecutionPhase.planning, .running, .canceling].contains($0.execution) }) {
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
                        .draggable(page.id.uuidString)
                        .dropDestination(for: String.self) { items, _ in
                            guard let value = items.first, let source = UUID(uuidString: value),
                                  store.pages.contains(where: { $0.id == source }), let index = store.pages.firstIndex(where: { $0.id == page.id }) else { return false }
                            Task { await store.movePage(source, to: index) }
                            return true
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
