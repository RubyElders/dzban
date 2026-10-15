import Combine
import DzbanBridge
import SwiftUI

@MainActor
public final class CatalogBrowserModel: ObservableObject {
    @Published public private(set) var actions: [BackendActions.Action] = []
    @Published public private(set) var entries: [CatalogResponse.Entry] = []
    @Published public private(set) var loading = false
    @Published public private(set) var failure: String?
    @Published public var selected: CatalogIdentity?
    public private(set) var session: UUID?
    private let bridge: Bridge
    private var epoch: UInt64 = 0
    private var work: Task<Void, Never>?

    public init(bridge: Bridge) { self.bridge = bridge }
    public func open(session: UUID) async {
        retire()
        self.session = session
        let epoch = epoch
        loading = true
        defer { if self.epoch == epoch { loading = false } }
        do {
            let response = try await bridge.request(.actions(session: session), as: BackendActions.self)
            guard self.epoch == epoch, self.session == session, response.session == session else { return }
            actions = response.actions
        } catch { if self.epoch == epoch { failure = (error as? BridgeFailure)?.message ?? error.localizedDescription } }
    }
    public func read(_ request: CatalogQuery) {
        guard let session else { return }
        epoch &+= 1
        let epoch = epoch
        work?.cancel()
        loading = true
        failure = nil
        entries = []
        selected = nil
        work = Task { [weak self, bridge] in
            defer { if let self, self.epoch == epoch { self.loading = false } }
            do {
                let response = try await bridge.request(.catalog(session: session, request: request), as: CatalogResponse.self)
                guard !Task.isCancelled, let self, self.epoch == epoch, self.session == session, response.session == session else { return }
                self.entries = response.entries
            } catch {
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
            }
        }
    }
    public func cancel() { epoch &+= 1; work?.cancel(); work = nil; loading = false }
    public func retire() { cancel(); session = nil; actions = []; entries = []; selected = nil; failure = nil }
}

struct CatalogBrowser: View {
    @StateObject private var model: CatalogBrowserModel
    let session: UUID
    @State private var kind = "table"
    @State private var glob = ""
    @State private var includeSystem = false
    @Environment(\.dismiss) private var dismiss

    init(bridge: Bridge, session: UUID) {
        self.session = session
        _model = StateObject(wrappedValue: CatalogBrowserModel(bridge: bridge))
    }
    private var listActions: [BackendActions.Action] { model.actions.filter { $0.id.hasPrefix("catalog.list.") } }
    private func list() { model.read(.list(kind: kind, glob: glob.isEmpty ? nil : glob, includeSystem: includeSystem)) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Database Catalog").font(.headline)
                Spacer()
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction).accessibilityIdentifier("catalog.close")
            }
            HStack {
                Picker("Objects", selection: $kind) {
                    ForEach(listActions) { action in Text(action.name).tag(String(action.id.dropFirst("catalog.list.".count))) }
                }.frame(width: 220).accessibilityIdentifier("catalog.kind")
                TextField("Name pattern", text: $glob).onSubmit { list() }.accessibilityIdentifier("catalog.pattern")
                Toggle("System objects", isOn: $includeSystem)
                Button("List") { list() }.disabled(model.loading || listActions.isEmpty).accessibilityIdentifier("catalog.list")
            }
            if model.loading { HStack { ProgressView().controlSize(.small); Button("Cancel") { model.cancel() } } }
            if let failure = model.failure { Text(failure).foregroundStyle(.red).textSelection(.enabled).accessibilityIdentifier("catalog.error") }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(Array(model.entries.enumerated()), id: \.offset) { _, entry in
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text([entry.identity.schema ?? entry.identity.database, entry.identity.name].joined(separator: ".")).font(.headline).textSelection(.enabled)
                                Spacer()
                                if model.actions.contains(where: { $0.id == "catalog.describe.\(entry.identity.kind)" }) {
                                    Button("Describe") { model.read(.describe(entry.identity)) }
                                        .accessibilityIdentifier("catalog.describe.\(entry.identity.name)")
                                }
                            }
                            ForEach(entry.attributes.keys.sorted(), id: \.self) { key in
                                Text("\(key): \(CatalogText.describe(entry.attributes[key]!))").textSelection(.enabled)
                            }
                        }.padding(8).frame(maxWidth: .infinity, alignment: .leading).background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
                    }
                }
            }.accessibilityIdentifier("catalog.entries")
            if model.entries.isEmpty && !model.loading && model.failure == nil { Text("No catalog entries").foregroundStyle(.secondary) }
        }
        .padding(20).frame(minWidth: 720, minHeight: 480)
        .task(id: session) { await model.open(session: session); if !listActions.contains(where: { $0.id == "catalog.list.\(kind)" }), let first = listActions.first { kind = String(first.id.dropFirst("catalog.list.".count)) }; if !listActions.isEmpty { list() } }
        .onDisappear { model.retire() }
    }
}

public enum CatalogText {
    public static func describe(_ value: JSONValue) -> String {
        switch value {
        case .null: return "SQL NULL"
        case .string(let text): return text
        case .bool(let value): return String(value)
        case .number(let value): return String(value)
        case .array(let values): return values.map(describe).joined(separator: ", ")
        case .object(let values): return values.keys.sorted().map { "\($0): \(describe(values[$0]!))" }.joined(separator: ", ")
        }
    }
}
