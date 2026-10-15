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
