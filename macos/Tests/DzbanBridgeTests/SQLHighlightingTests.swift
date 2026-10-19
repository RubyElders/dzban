import DzbanBridge
@testable import DzbanApplication
import XCTest

private actor GatedSQLTokens {
    private var pending: [UInt64: CheckedContinuation<SQLSyntaxSnapshot, Error>] = [:]
    private(set) var sources: [(String, UInt64)] = []
    var count: Int { sources.count }
    func tokenize(_ source: String, _ revision: UInt64) async throws -> SQLSyntaxSnapshot {
        sources.append((source, revision))
        return try await withCheckedThrowingContinuation { pending[revision] = $0 }
    }
    func cancelAll() {
        let waiting = pending.values
        pending = [:]
        for continuation in waiting { continuation.resume(throwing: CancellationError()) }
    }
    func finish(_ revision: UInt64, as returnedRevision: UInt64? = nil) {
        pending.removeValue(forKey: revision)?.resume(returning: SQLSyntaxSnapshot(revision: returnedRevision ?? revision, length: SQLTextOffset(bytes: 0, scalars: 0, utf16: 0), tokens: []))
    }
}

@MainActor
final class SQLHighlightingTests: XCTestCase {
    private func until(_ condition: () async -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(3)
        while !(await condition()) {
            if ContinuousClock.now >= deadline { throw ViewportFailure("SQL highlighting test timed out") }
            try await Task.sleep(for: .milliseconds(1))
        }
    }

    func testEditsCoalesceAndOnlyTheLatestRevisionCanPublish() async throws {
        let gate = GatedSQLTokens()
        defer { Task { await gate.cancelAll() } }
        let pipeline = SQLHighlighting(delay: .zero, tokenize: { try await gate.tokenize($0, $1) })
        defer { pipeline.retire() }
        var received: [UInt64] = []
        pipeline.completed = { received.append($0.revision) }
        for index in 0..<100 { pipeline.request("first \(index)") }
        try await until { await gate.count == 1 }
        let firstCount = await gate.count
        XCTAssertEqual(firstCount, 1)
        for index in 0..<10 { pipeline.request("next \(index)") }
        await Task.yield()
        let countWhileReading = await gate.count
        XCTAssertEqual(countWhileReading, 1)
        await gate.finish(100)
        try await until { await gate.count == 2 }
        XCTAssertTrue(received.isEmpty)
        let sources = await gate.sources
        XCTAssertEqual(sources.last?.0, "next 9")
        await gate.finish(110)
        try await until { pipeline.isIdle }
        XCTAssertEqual(received, [110])
    }

    func testCompositionInvalidationAndRetirementRejectLateWork() async throws {
        let gate = GatedSQLTokens()
        defer { Task { await gate.cancelAll() } }
        let pipeline = SQLHighlighting(delay: .zero, tokenize: { try await gate.tokenize($0, $1) })
        var received: [UInt64] = []
        pipeline.completed = { received.append($0.revision) }
        pipeline.request("old")
        try await until { await gate.count == 1 }
        pipeline.invalidate()
        await gate.finish(1)
        try await until { pipeline.isIdle }
        XCTAssertTrue(received.isEmpty)
        pipeline.request("committed")
        try await until { await gate.count == 2 }
        pipeline.retire()
        await gate.finish(3)
        try await until { pipeline.isIdle }
        XCTAssertTrue(received.isEmpty)
        pipeline.request("retired")
        XCTAssertEqual(pipeline.revision, 4)
    }

    func testMismatchedRevisionFailsWithoutPoisoningTheNextEdit() async throws {
        let gate = GatedSQLTokens()
        defer { Task { await gate.cancelAll() } }
        let pipeline = SQLHighlighting(delay: .zero, tokenize: { try await gate.tokenize($0, $1) })
        defer { pipeline.retire() }
        var received: [UInt64] = []
        var failures = 0
        pipeline.completed = { received.append($0.revision) }
        pipeline.failed = { failures += 1 }
        pipeline.request("first")
        try await until { await gate.count == 1 }
        await gate.finish(1, as: 0)
        try await until { pipeline.isIdle }
        XCTAssertTrue(received.isEmpty)
        XCTAssertEqual(failures, 1)
        pipeline.request("second")
        try await until { await gate.count == 2 }
        await gate.finish(2)
        try await until { pipeline.isIdle }
        XCTAssertEqual(received, [2])
    }
}
