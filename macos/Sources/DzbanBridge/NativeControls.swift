import Foundation

public struct ConnectionProbe: Decodable, Sendable {
    public let state: String
    public let latencyMicros: UInt64?
    public let version: String?
    public let error: String?
}

public struct NativeInput: Encodable, Sendable {
    public enum Key: String, Encodable, Sendable, Hashable { case primary, preview, inspect, submit, cancel }
    let kind: String
    let key: Key?
    let atMs: UInt64?
    let plan: JSONValue?
    let message: String?
    public static func press(_ key: Key, at: UInt64) -> Self { Self(kind: "press", key: key, atMs: at, plan: nil, message: nil) }
    public static func release(_ key: Key, at: UInt64) -> Self { Self(kind: "release", key: key, atMs: at, plan: nil, message: nil) }
    public static let previewCanceled = Self(kind: "preview_canceled", key: nil, atMs: nil, plan: nil, message: nil)
    public static let pin = Self(kind: "pin", key: nil, atMs: nil, plan: nil, message: nil)
    public static let reset = Self(kind: "reset", key: nil, atMs: nil, plan: nil, message: nil)
    public static let contextChanged = Self(kind: "context_changed", key: nil, atMs: nil, plan: nil, message: nil)
    public static func ready(_ plan: JSONValue) -> Self { Self(kind: "preview_ready", key: nil, atMs: nil, plan: plan, message: nil) }
    public static func failed(_ message: String) -> Self { Self(kind: "preview_failed", key: nil, atMs: nil, plan: nil, message: message) }
}

public struct InputProjection: Decodable, Sendable {
    public struct Hint: Decodable, Sendable {
        public let kind: String
        public let plan: JSONValue?
        public let error: String?
    }
    public let session: UUID
    public let document: UUID
    public let commands: [String]
    public let hint: Hint
    public let pinned: Bool
}

extension BridgeCommand {
    public static func awaitIdle(session: UUID) -> Self { Self(command: "await_idle", session: session) }
    public static func probe(session: UUID, version: Bool) -> Self { Self(command: "probe_session", session: session, includeVersion: version) }
    public static func probe(profile: String) -> Self { Self(command: "probe_profile", profile: profile) }
    public static func input(session: UUID, document: UUID, revision: UInt64, event: NativeInput) -> Self {
        Self(command: "input", session: session, document: document, revision: revision, input: event)
    }
}

extension JSONValue: Encodable {
    public func encode(to encoder: Encoder) throws {
        var value = encoder.singleValueContainer()
        switch self {
        case .null: try value.encodeNil()
        case .bool(let item): try value.encode(item)
        case .number(let item): try value.encode(item)
        case .string(let item): try value.encode(item)
        case .array(let items): try value.encode(items)
        case .object(let items): try value.encode(items)
        }
    }
}
