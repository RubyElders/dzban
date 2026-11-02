import CDzban
import Foundation

public struct ResultGridState: Decodable, Sendable {
    public let handle: UInt64
    public let cursor: ResultPosition?
    public let mode: String
    public let ranges: [ExportRowRange]
    public let columns: [Int]
    public let frozen: [Int]
    public let left: [Int]
    public let right: [Int]
    public let visible: [Int]
    public let reveal: Double?

    public func contains(row: Int, column: Int) -> Bool {
        columns.contains(column) && ranges.contains { $0.start <= row && row < $0.end }
    }
}

@MainActor
public final class ResultGrid {
    private var handle: UInt64 = 0
    public private(set) var state: ResultGridState
    public let schema: [Column]

    public init(columns: [Column], rows: Int) throws {
        schema = columns
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let schema = try JSONSerialization.jsonObject(with: encoder.encode(columns))
        state = try Self.evaluate(handle: 0, request: ["action": "create", "columns": schema, "rows": max(0, rows)])
        handle = state.handle
    }
    private init(schema: [Column], state: ResultGridState) { self.schema = schema; self.state = state; handle = state.handle }
    public func clone() throws -> ResultGrid {
        ResultGrid(schema: schema, state: try Self.evaluate(handle: handle, request: ["action": "clone"]))
    }
    deinit { _ = dzban_release(handle) }

    @discardableResult public func apply(_ action: String, _ arguments: [String: Any] = [:]) throws -> ResultGridState {
        var request = arguments
        request["action"] = action
        state = try Self.evaluate(handle: handle, request: request)
        return state
    }

    private static func evaluate(handle: UInt64, request: [String: Any]) throws -> ResultGridState {
        let data = try JSONSerialization.data(withJSONObject: request)
        let buffer = data.withUnsafeBytes { dzban_result_grid(handle, $0.bindMemory(to: UInt8.self).baseAddress, data.count) }
        guard buffer.status == 0, let pointer = buffer.data else { throw BridgeFailure(kind: "result_grid", message: "Cannot update result selection") }
        defer { _ = dzban_buffer_release(buffer.token) }
        return try decodeRecord(Data(bytes: pointer, count: buffer.length), as: ResultGridState.self).payload
    }
}
