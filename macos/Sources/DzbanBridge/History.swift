import Foundation

public struct HistoryList: Decodable, Sendable {
    public struct Entry: Decodable, Sendable, Identifiable {
        public let index: Int
        public let sqlPreview: String
        public let executedAt: UInt64
        public let status: String
        public let rows: Int?
        public let durationMs: UInt64?
        public var id: Int { index }
    }
    public let revision: UUID
    public let entries: [Entry]
}

public struct SavedPlan: Decodable, Sendable {
    public let plan: JSONValue?
}

extension BridgeCommand {
    public static func historyList(session: UUID) -> Self { Self(command: "history_list", session: session) }
    public static func restoreHistory(session: UUID, document: UUID, revision: UUID, index: Int) -> Self {
        Self(command: "restore_history", session: session, document: document, historyRevision: revision, index: index)
    }
    public static func savedPlan(session: UUID, document: UUID, generation: UInt64) -> Self {
        Self(command: "saved_plan", session: session, document: document, generation: generation)
    }
}
