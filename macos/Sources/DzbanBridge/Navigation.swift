import Foundation

public struct BackendActions: Decodable, Sendable {
    public struct Action: Decodable, Sendable, Identifiable {
        public struct Argument: Decodable, Sendable {
            public let name: String
            public let kind: String
            public let required: Bool
        }
        public let id: String
        public let name: String
        public let description: String
        public let effect: String
        public let arguments: [Argument]
    }
    public let session: UUID
    public let actions: [Action]
}

public struct CatalogIdentity: Codable, Sendable, Hashable {
    public let database: String
    public let schema: String?
    public let kind: String
    public let name: String
}

public struct CatalogResponse: Decodable, Sendable {
    public struct Entry: Decodable, Sendable {
        public let identity: CatalogIdentity
        public let attributes: [String: JSONValue]
    }
    public let session: UUID
    public let entries: [Entry]
}

public struct CatalogQuery: Encodable, Sendable {
    let operation: String
    let kind: String?
    let database: String?
    let schema: String?
    let glob: String?
    let includeSystem: Bool?
    let object: CatalogIdentity?
    let full: Bool?

    public static func list(kind: String, database: String? = nil, schema: String? = nil, glob: String? = nil, includeSystem: Bool = false) -> Self {
        Self(operation: "list", kind: kind, database: database, schema: schema, glob: glob, includeSystem: includeSystem, object: nil, full: nil)
    }
    public static func describe(_ object: CatalogIdentity, full: Bool = true) -> Self {
        Self(operation: "describe", kind: nil, database: nil, schema: nil, glob: nil, includeSystem: nil, object: object, full: full)
    }
}

extension BridgeCommand {
    public static func actions(session: UUID) -> Self { Self(command: "actions", session: session) }
    public static func catalog(session: UUID, request: CatalogQuery) -> Self { Self(command: "catalog", session: session, request: request) }
}

public enum PeekSurface: String, Encodable, Sendable { case plan, row }
public enum PeekEvent: String, Encodable, Sendable { case show, open, pin, close, release, reset }
public struct PeekVisibility: Decodable, Sendable {
    public let visible: Bool
    public let pinned: Bool
}
extension BridgeCommand {
    public static func peek(session: UUID, surface: PeekSurface, event: PeekEvent, elapsedMilliseconds: UInt64? = nil) -> Self {
        Self(command: "peek", session: session, surface: surface, event: event, elapsedMs: elapsedMilliseconds)
    }
}

extension BridgeCommand {
    public static func contextChanged(session: UUID, document: UUID) -> Self { Self(command: "context_changed", session: session, document: document) }
}

extension BridgeCommand {
    public static func observeControls(session: UUID) -> Self { Self(command: "observe_controls", session: session) }
}

public enum TabPhase: String, Codable, Sendable {
    case choosing, connecting, ready, closing, error
}

public struct TabInventory: Decodable, Sendable {
    public struct Tab: Decodable, Sendable, Identifiable {
        public let tab: UUID
        public var id: UUID { tab }
        public let session: UUID?
        public let profile: String?
        public let backend: DatabaseBackend?
        public let document: UUID
        public let execution: ExecutionPhase
        public let phase: TabPhase
        public let failure: String?
        public var snapshot: DocumentSnapshot
    }
    public let revision: UInt64
    public var tabs: [Tab]
    public let selected: UUID?
}

public struct CreatedTab: Decodable, Sendable {
    public let tab: UUID
    public let workspace: TabInventory
}

extension BridgeCommand {
    public static var tabs: Self { Self(command: "tabs") }
    public static func openTab(profile: String? = nil, draft: String = "", revision: UInt64? = nil) -> Self { Self(command: "open_tab", profile: profile, draft: draft, revision: revision) }
    public static func cycleTabs(_ direction: Int, revision: UInt64? = nil) -> Self { Self(command: "cycle_tabs", revision: revision, direction: direction) }
    public static func selectTab(_ tab: UUID, revision: UInt64? = nil) -> Self { Self(command: "select_tab", tab: tab, revision: revision) }
    public static func cancelTabConnection(_ tab: UUID) -> Self { Self(command: "cancel_tab_connection", tab: tab) }
    public static func closeTab(_ tab: UUID) -> Self { Self(command: "close_tab", tab: tab) }
    public static func setTabDraft(_ tab: UUID, draft: String) -> Self { Self(command: "set_tab_draft", tab: tab, draft: draft) }
}
