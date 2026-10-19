import CDzban
import Foundation

public enum SQLTokenKind: UInt64, Sendable { case keyword, string, number, comment, identifier, function }
public struct SQLTextOffset: Decodable, Sendable, Equatable {
    public let bytes: Int
    public let scalars: Int
    public let utf16: Int
    public init(bytes: Int, scalars: Int, utf16: Int) { self.bytes = bytes; self.scalars = scalars; self.utf16 = utf16 }
}
public struct SQLToken: Sendable, Equatable {
    public let start: SQLTextOffset
    public let end: SQLTextOffset
    public let kind: SQLTokenKind
}
public struct SQLSyntaxSnapshot: Sendable {
    public let revision: UInt64
    public let length: SQLTextOffset
    public let tokens: [SQLToken]
    public init(revision: UInt64, length: SQLTextOffset, tokens: [SQLToken]) { self.revision = revision; self.length = length; self.tokens = tokens }
}

public enum SQLTokenizer {
    public static func tokens(in source: String, revision: UInt64) throws -> SQLSyntaxSnapshot {
        try Task.checkCancellation()
        let bytes = Array(source.utf8)
        let handle = bytes.withUnsafeBufferPointer { dzban_syntax_create($0.baseAddress, bytes.count, revision) }
        guard handle != 0 else { throw BridgeFailure(kind: "syntax", message: "Cannot prepare SQL highlighting") }
        defer { _ = dzban_release(handle) }
        var tokens: [SQLToken] = []
        var length: SQLTextOffset?
        while true {
            try Task.checkCancellation()
            let buffer = dzban_syntax_read(handle)
            guard buffer.status == 0, let pointer = buffer.data else { throw BridgeFailure(kind: "syntax", message: "Cannot read SQL highlighting") }
            let batch: (revision: UInt64, length: SQLTextOffset, tokens: [SQLToken], done: Bool)
            do {
                defer { _ = dzban_buffer_release(buffer.token) }
                batch = try decode(pointer, count: buffer.length)
            }
            guard batch.revision == revision, length == nil || length == batch.length else { throw BridgeFailure(kind: "syntax", message: "SQL highlighting revision changed") }
            length = batch.length
            tokens.append(contentsOf: batch.tokens)
            try Task.checkCancellation()
            if batch.done { return SQLSyntaxSnapshot(revision: revision, length: batch.length, tokens: tokens) }
        }
    }
    private static func decode(_ pointer: UnsafePointer<UInt8>, count: Int) throws -> (revision: UInt64, length: SQLTextOffset, tokens: [SQLToken], done: Bool) {
        guard count >= 56 else { throw BridgeFailure(kind: "syntax", message: "Incomplete SQL highlighting header") }
        let raw = UnsafeRawPointer(pointer)
        func word(_ index: Int) -> UInt64 { raw.loadUnaligned(fromByteOffset: index * 8, as: UInt64.self).littleEndian }
        func offset(_ index: Int) throws -> SQLTextOffset {
            guard let bytes = Int(exactly: word(index)), let scalars = Int(exactly: word(index + 1)), let utf16 = Int(exactly: word(index + 2)) else { throw BridgeFailure(kind: "syntax", message: "Invalid SQL highlighting offset") }
            return SQLTextOffset(bytes: bytes, scalars: scalars, utf16: utf16)
        }
        guard word(0) == 1, let size = Int(exactly: word(5)), size <= 512, count == 56 + size * 56, word(6) <= 1 else { throw BridgeFailure(kind: "syntax", message: "Invalid SQL highlighting batch") }
        var tokens: [SQLToken] = []
        tokens.reserveCapacity(size)
        let length = try offset(2)
        var previous = 0
        for index in 0..<size {
            let base = 7 + index * 7
            let start = try offset(base), end = try offset(base + 3)
            guard let kind = SQLTokenKind(rawValue: word(base + 6)), start.utf16 >= previous, end.utf16 >= start.utf16, end.utf16 <= length.utf16 else { throw BridgeFailure(kind: "syntax", message: "Invalid SQL highlighting span") }
            tokens.append(SQLToken(start: start, end: end, kind: kind))
            previous = end.utf16
        }
        return (word(1), length, tokens, word(6) == 1)
    }

}
