import Foundation

public enum ConnectionState: String, Codable, Sendable {
    case unknown, disconnected, connecting, connected, busy, error
    public var label: String { rawValue.capitalized }
}

public struct ConnectionProbe: Decodable, Sendable {
    public let reconnected: Bool?
    public let state: ConnectionState
    public let latencyMicros: UInt64?
    public let version: String?
    public let error: String?
}

public enum RecoveryPhase: String, Decodable, Sendable {
    case idle, reconnecting, retryWaiting = "retry_waiting"
}

public struct HealthStatus: Decodable, Sendable {
    public let recovery: RecoveryPhase?
    public let recoveries: UInt64?
    public let state: ConnectionState
    public let probing: Bool
    public let latencyMicros: UInt64?
    public let version: String?
    public let error: String?
    public let checkedAtMillis: UInt64?
}

public struct HealthSnapshot: Decodable, Sendable {
    public struct Session: Decodable, Sendable {
        public let session: UUID
        public let status: HealthStatus
    }
    public struct Profile: Decodable, Sendable {
        public let profile: String
        public let status: HealthStatus
    }
    public let revision: UInt64
    public let sessions: [Session]
    public let profiles: [Profile]
}

public struct NativeInput: Encodable, Sendable {
    public enum Key: String, Encodable, Sendable, Hashable { case primary, preview, inspect, submit, cancel }
    let kind: String
    let key: Key?
    let atMs: UInt64?
    private var elapsedMs: UInt64? = nil
    public static func press(_ key: Key, at: UInt64) -> Self { Self(kind: "press", key: key, atMs: at) }
    public static func release(_ key: Key, at: UInt64) -> Self { Self(kind: "release", key: key, atMs: at) }
    public static let openPlan = Self(kind: "open_plan", key: nil, atMs: nil)
    public static let showPlan = Self(kind: "show_plan", key: nil, atMs: nil)
    public static let estimate = Self(kind: "estimate", key: nil, atMs: nil)
    public static func releasePlan(milliseconds: UInt64) -> Self {
        var event = Self(kind: "release_plan", key: nil, atMs: nil)
        event.elapsedMs = milliseconds
        return event
    }
    public static let pin = Self(kind: "pin", key: nil, atMs: nil)
    public static let reset = Self(kind: "reset", key: nil, atMs: nil)
    public static let contextChanged = Self(kind: "context_changed", key: nil, atMs: nil)
}

public struct InputProjection: Decodable, Sendable {
    public struct Hint: Decodable, Sendable {
        public let kind: InputHintKind
        public let plan: PlanPresentation?
        public let error: String?
    }
    public let session: UUID
    public let document: UUID
    public let commands: [InputEffect]
    public let hint: Hint
    public let pinned: Bool
}

public struct InteractionProjection: Decodable, Sendable {
    public let revision: UInt64
    public let draftRevision: UInt64
    public let retired: Bool
    public let input: InputProjection
    public let visible: Bool
    public let loading: Bool
    public let plan: PlanPresentation?
    public let error: String?
}

public struct InputOutcome: Decodable, Sendable {
    public let interaction: InteractionProjection
    public let outcome: EditorOutcome
}

extension BridgeCommand {
    public static let observeHealth = Self(command: "observe_health")
    public static let checkProfilesHealth = Self(command: "check_profiles_health")
    public static func checkHealth(session: UUID) -> Self { Self(command: "check_health", session: session) }
    public static func checkProfileHealth(_ profile: String) -> Self { Self(command: "check_profile_health", profile: profile) }
    public static func awaitIdle(session: UUID) -> Self { Self(command: "await_idle", session: session) }
    public static func probe(session: UUID, version: Bool) -> Self { Self(command: "probe_session", session: session, includeVersion: version) }
    public static func probe(profile: String) -> Self { Self(command: "probe_profile", profile: profile) }
    public static func awaitPlan(session: UUID, revision: UInt64) -> Self { Self(command: "await_plan", session: session, revision: revision) }
    public static func interact(session: UUID, document: UUID, revision: UInt64, draft: String, selection: Range<Int>, event: NativeInput) -> Self {
        Self(command: "interact", session: session, document: document, draft: draft, start: selection.lowerBound, revision: revision, end: selection.upperBound, input: event)
    }
    public static func observeInteraction(session: UUID) -> Self { Self(command: "observe_interaction", session: session) }
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
