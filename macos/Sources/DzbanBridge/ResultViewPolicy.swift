import CDzban
import Foundation

public struct ResultPosition: Codable, Sendable, Equatable {
    public let row: Int
    public let column: Int
    public init(row: Int, column: Int) { self.row = row; self.column = column }
}

public enum ResultRestorationState: String, Decodable, Sendable {
    case waitingSchema = "waiting_schema", schemaChanged = "schema_changed", waitingRows = "waiting_rows", apply
}

public struct ResultViewDecision: Decodable, Sendable {
    public let selection: ResultPosition?
    public let restoration: ResultRestorationState?
    public let selectionPending: Bool
}

public struct ResultViewPolicy: Encodable, Sendable {
    private enum Action: String, Encodable, Sendable { case select, clamp, moveRow = "move_row", restore }
    private let action: Action
    var row: Int?
    var column: Int?
    var rows: Int
    var columns: Int?
    var position: ResultPosition?
    var direction: Int?
    var previous: [Column]?
    var current: [Column]?
    var terminal: Bool?

    public static func select(row: Int, column: Int, rows: Int, columns: Int) throws -> ResultPosition? {
        try Self(action: .select, row: row, column: column, rows: rows, columns: columns).evaluate().selection
    }
    public static func clamp(_ position: ResultPosition, rows: Int, columns: Int) throws -> ResultPosition? {
        try Self(action: .clamp, rows: rows, columns: columns, position: position).evaluate().selection
    }
    public static func moveRow(_ position: ResultPosition, direction: Int, rows: Int, columns: Int) throws -> ResultPosition? {
        try Self(action: .moveRow, rows: rows, columns: columns, position: position, direction: direction).evaluate().selection
    }
    public static func restore(previous: [Column], current: [Column], row: Int?, column: Int, rows: Int, terminal: Bool) throws -> ResultViewDecision {
        try Self(action: .restore, row: row, column: column, rows: rows, previous: previous, current: current, terminal: terminal).evaluate()
    }

    private func evaluate() throws -> ResultViewDecision {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let data = try encoder.encode(self)
        let buffer = data.withUnsafeBytes { dzban_result_view($0.bindMemory(to: UInt8.self).baseAddress, data.count) }
        guard buffer.status == 0, let pointer = buffer.data else { throw BridgeFailure(kind: "result_view", message: "Cannot evaluate result selection") }
        defer { _ = dzban_buffer_release(buffer.token) }
        return try decodeRecord(Data(bytes: pointer, count: buffer.length), as: ResultViewDecision.self).payload
    }
}
