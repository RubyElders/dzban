import Foundation

public enum DiagnosticKind: String, Codable, Sendable {
    case database, busy, canceled, closed, unsupported, message
}

public struct DiagnosticPresentation: Codable, Sendable, Equatable {
    public let kind: DiagnosticKind
    public let backend: DatabaseBackend?
    public let databaseKind: DatabaseErrorKind?
    public let message: String
    public let code: DiagnosticCode?
    public let detail: String?
    public let hint: String?
    public let text: String
}

public enum DatabaseErrorKind: String, Codable, Sendable {
    case server = "Server", transport = "Transport", canceled = "Canceled", busy = "Busy", unsupported = "Unsupported"
}

public enum DiagnosticCode: Codable, Sendable, Equatable {
    case sqlState(String), clickhouse(UInt32)
    private enum CodingKeys: String, CodingKey { case kind, value }
    private enum Kind: String, Codable { case sqlState = "sql_state", clickhouse }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(Kind.self, forKey: .kind) {
        case .sqlState: self = .sqlState(try container.decode(String.self, forKey: .value))
        case .clickhouse: self = .clickhouse(try container.decode(UInt32.self, forKey: .value))
        }
    }
    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .sqlState(let value):
            try container.encode(Kind.sqlState, forKey: .kind)
            try container.encode(value, forKey: .value)
        case .clickhouse(let value):
            try container.encode(Kind.clickhouse, forKey: .kind)
            try container.encode(value, forKey: .value)
        }
    }
}

public enum ResultStatusReason: String, Decodable, Sendable {
    case ready, planning, waiting, receiving, draining, finishing, canceling, canceled, failed, command, complete
    case waitingForSchema = "waiting_for_schema"
    case captureFailedAwaitingCleanup = "capture_failed_awaiting_cleanup"
    case limitedByRows = "limited_by_rows"
    case limitedByBytes = "limited_by_bytes"
    case savedFailed = "saved_failed"
    case savedCommand = "saved_command"
    case savedRows = "saved_rows"
}

public enum EmptyResultReason: String, Decodable, Sendable {
    case ready, command, canceled
    case noRows = "no_rows"
}

public enum ResultProgress: String, Decodable, Sendable { case planning, running, canceling }

public struct ResultFeedback: Decodable, Sendable {
    public let status: ResultStatusReason
    public let empty: EmptyResultReason
    public let progress: ResultProgress?
    public let rows: Int
    public let totalRows: Int?
    public let affectedRows: UInt64?
    public let queryDiagnostic: DiagnosticPresentation?
    public let captureDiagnostic: DiagnosticPresentation?
}
