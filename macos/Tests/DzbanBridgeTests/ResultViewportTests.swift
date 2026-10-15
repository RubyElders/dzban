import AppKit
import DzbanBridge
@testable import DzbanApplication
import XCTest

private func resultDTO<T: Decodable>(_ value: Any, as type: T.Type) throws -> T {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return try decoder.decode(type, from: JSONSerialization.data(withJSONObject: value))
}

private func resultSnapshot(document: UUID = UUID(), generation: UInt64 = 1, rows: Int = 500, phase: String = "finished", execution: String = "finished", termination: String = "complete") throws -> DocumentSnapshot {
    try resultDTO(["document": document.uuidString, "generation": generation, "draft_revision": 0, "title": "Query 1", "draft": "SELECT 1", "execution": execution, "result_phase": phase, "result_available": phase != "waiting_for_schema", "termination": termination, "rows": rows], as: DocumentSnapshot.self)
}

private func resultPage(_ scope: ResultScope, start: Int, count: Int, columnStart: Int, columnCount: Int, text: String? = nil) throws -> ViewportPage {
    let rows = (start..<(start + count)).map { row in (columnStart..<(columnStart + columnCount)).map { column -> [String: Any] in
        let text = text ?? "\(scope.document.uuidString):\(row):\(column)"
        return ["text": text, "length": text.unicodeScalars.count, "truncated": false]
    } }
    return try resultDTO(["document": scope.document.uuidString, "generation": scope.generation, "start": start, "column_start": columnStart, "rows": rows], as: ViewportPage.self)
}

private actor ViewportSourceFixture: ResultSource {
    struct Pending {
        let response: ViewportPage
        let continuation: CheckedContinuation<ViewportPage, Error>
    }
    var paused = false
    var failNext = false
    var partialRows = 200
    var original: String? = "NULL\n😀e\u{301}"
    var pending: [Pending] = []
    var pendingCount: Int { pending.count }
    var pageCalls = 0
    var returnedRows = 0
    var inFlight = 0
    var peakInFlight = 0

    func configure(paused: Bool = false, partialRows: Int = 200, failNext: Bool = false, original: String? = "NULL\n😀e\u{301}") {
        self.paused = paused
        self.partialRows = partialRows
        self.failNext = failNext
        self.original = original
    }
    func schema(_ scope: ResultScope) async throws -> ResultSchema {
        try resultDTO(["document": scope.document.uuidString, "generation": scope.generation, "columns": (0..<32).map { ["name": "column_\($0)", "type_name": "text"] }], as: ResultSchema.self)
    }
    func page(_ scope: ResultScope, start: Int, count: Int, columnStart: Int, columnCount: Int) async throws -> ViewportPage {
        pageCalls += 1
        inFlight += 1
        peakInFlight = max(peakInFlight, inFlight)
        defer { inFlight -= 1 }
        if failNext { failNext = false; throw ViewportFailure("Fixture page failed") }
        let response = try resultPage(scope, start: start, count: min(count, partialRows), columnStart: columnStart, columnCount: columnCount)
        returnedRows += response.rows.count
        if paused {
            return try await withCheckedThrowingContinuation { pending.append(Pending(response: response, continuation: $0)) }
        }
        return response
    }
    func cell(_ address: ResultAddress, offset: Int, count: Int) async throws -> CellChunk {
        let part = original.map { String($0.unicodeScalars.dropFirst(offset).prefix(count)) }
        return try resultDTO(["document": address.scope.document.uuidString, "generation": address.scope.generation, "row": address.row, "column": address.column, "offset": offset, "total": original?.unicodeScalars.count ?? 0, "text": part.map { $0 as Any } ?? NSNull()], as: CellChunk.self)
    }
    func release(error: Bool = false) {
        paused = false
        let admitted = pending
        pending = []
        for request in admitted {
            if error { request.continuation.resume(throwing: ViewportFailure("Retired page failed")) }
            else { request.continuation.resume(returning: request.response) }
        }
    }
}

@MainActor
final class ResultViewportTests: XCTestCase {
    private func eventually(_ condition: @MainActor () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(5)
        while !condition() {
            if ContinuousClock.now >= deadline { XCTFail("Viewport did not reach the expected state"); throw ViewportFailure("Viewport test timed out") }
            try await Task.sleep(for: .milliseconds(5))
        }
    }

    func testLateFailuresCannotReplaceNewOutputAndReadsStayBounded() async throws {
        let source = ViewportSourceFixture()
        await source.configure(paused: true)
        let model = ResultViewport(source: source)
        defer { model.retire(); Task { await source.release() } }
        let session = UUID()
        let old = try resultSnapshot(rows: 256)
        model.update(session: session, document: old)
        model.show(rows: 0..<128, columns: 0..<16)
        try await eventually { model.inFlightCount == 2 && model.columns.count == 32 }
        let deadline = ContinuousClock.now + .seconds(5)
        while await source.pendingCount < 2 {
            if ContinuousClock.now >= deadline { throw ViewportFailure("Fixture did not admit both page reads") }
            try await Task.sleep(for: .milliseconds(5))
        }
        for _ in 0..<20 { model.show(rows: 0..<128, columns: 0..<16) }
        XCTAssertEqual(model.requestCount, 3)
        let next = try resultSnapshot(generation: 2, rows: 3)
        model.update(session: session, document: next)
        model.show(rows: 0..<3, columns: 0..<2)
        XCTAssertEqual(model.inFlightCount, 2)
        await source.release(error: true)
        try await eventually { model.value(row: 2, column: 1) != nil }
        XCTAssertTrue(model.value(row: 0, column: 0)?.text?.hasPrefix(next.document.uuidString) == true)
        XCTAssertNil(model.loadError)
        XCTAssertLessThanOrEqual(model.peakInFlight, 2)
        let peak = await source.peakInFlight
        XCTAssertLessThanOrEqual(peak, 2)
    }

    func testPartialPagesScrollEvictionAndShrinkKeepTheVisibleScope() async throws {
        let source = ViewportSourceFixture()
        await source.configure(partialRows: 3)
        let model = ResultViewport(source: source)
        defer { model.retire() }
        let session = UUID()
        let snapshot = try resultSnapshot(rows: 5000)
        model.update(session: session, document: snapshot)
        model.show(rows: 0..<32, columns: 0..<4)
        try await eventually { model.value(row: 63, column: 0) != nil && model.inFlightCount == 0 }
        let initialRows = await source.returnedRows
        XCTAssertEqual(initialRows, 64)
        for page in 1...10 {
            let start = page * 128
            model.show(rows: start..<(start + 20), columns: 16..<20)
            try await eventually { model.value(row: start + 19, column: 19) != nil && model.inFlightCount == 0 }
            XCTAssertLessThanOrEqual(model.pageCount, 8)
            XCTAssertLessThanOrEqual(model.cacheBytes, 8 * 1024 * 1024)
        }
        XCTAssertNil(model.value(row: 0, column: 0))
        model.select(row: 1280, column: 19)
        model.update(session: session, document: try resultSnapshot(document: snapshot.document, rows: 2))
        XCTAssertEqual(model.selected?.row, 1)
        XCTAssertNil(model.value(row: 1280, column: 19))
        model.show(rows: 1280..<1300, columns: 16..<20)
        try await eventually { model.value(row: 1, column: 19) != nil }
    }

    func testReadFailureRequiresRetryAndPreservesAvailableRows() async throws {
        let source = ViewportSourceFixture()
        await source.configure(failNext: true)
        let model = ResultViewport(source: source)
        defer { model.retire() }
        model.update(session: UUID(), document: try resultSnapshot(rows: 20))
        model.show(rows: 0..<20, columns: 0..<2)
        try await eventually { model.loadError != nil && model.inFlightCount == 0 }
        XCTAssertEqual(model.loadError, "Fixture page failed")
        XCTAssertEqual(model.availableRows, 20)
        let calls = await source.pageCalls
        model.show(rows: 0..<20, columns: 0..<2)
        XCTAssertEqual(model.requestCount, calls + 1)
        model.retry()
        try await eventually { model.value(row: 19, column: 1) != nil }
        XCTAssertNil(model.loadError)
    }

    func testCacheUsesBothPageAndByteLimitsWithoutEvictingOnAnOversizedInsert() throws {
        let scope = ResultScope(session: UUID(), document: UUID(), generation: 1)
        var cache = ResultTileCache(maximumPages: 2, maximumBytes: 1000)
        for row in 0..<2 {
            XCTAssertTrue(cache.insert(try resultPage(scope, start: row, count: 1, columnStart: 0, columnCount: 1, text: String(repeating: "x", count: 100)), key: ResultTileKey(row: row, column: 0)))
        }
        _ = cache.cell(row: 0, column: 0, key: ResultTileKey(row: 0, column: 0))
        XCTAssertTrue(cache.insert(try resultPage(scope, start: 2, count: 1, columnStart: 0, columnCount: 1, text: "x"), key: ResultTileKey(row: 2, column: 0)))
        XCTAssertFalse(cache.hasRow(1, in: ResultTileKey(row: 1, column: 0)))
        XCTAssertTrue(cache.hasRow(0, in: ResultTileKey(row: 0, column: 0)))
        cache.maximumBytes = 400
        XCTAssertTrue(cache.insert(try resultPage(scope, start: 3, count: 1, columnStart: 0, columnCount: 1, text: "x"), key: ResultTileKey(row: 3, column: 0)))
        XCTAssertLessThanOrEqual(cache.bytes, 400)
        XCTAssertFalse(cache.insert(try resultPage(scope, start: 4, count: 1, columnStart: 0, columnCount: 1, text: String(repeating: "x", count: 2000)), key: ResultTileKey(row: 4, column: 0)))
        XCTAssertTrue(cache.hasRow(3, in: ResultTileKey(row: 3, column: 0)))
    }

    func testOriginalCellCopyPreservesNullUnicodeAndMultilineValues() async throws {
        let source = ViewportSourceFixture()
        let original = String(repeating: "NULL\n😀e\u{301}", count: 20_000)
        await source.configure(original: original)
        let address = ResultAddress(scope: ResultScope(session: UUID(), document: UUID(), generation: 1), row: 0, column: 1)
        let result = try await FullCellReader.read(source: source, address: address)
        XCTAssertEqual(result.text, original)
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("dzban-test-\(UUID())"))
        defer { pasteboard.releaseGlobally() }
        try result.write(to: pasteboard)
        XCTAssertEqual(pasteboard.string(forType: .string), original)
        let typed = try XCTUnwrap(pasteboard.data(forType: NSPasteboard.PasteboardType("com.rubyelders.dzban.cell")))
        XCTAssertEqual((try JSONSerialization.jsonObject(with: typed) as? [String: Any])?["isNull"] as? Bool, false)
        await source.configure(original: nil)
        let null = try await FullCellReader.read(source: source, address: address)
        XCTAssertNil(null.text)
        try null.write(to: pasteboard)
        let encoded = try XCTUnwrap(pasteboard.data(forType: NSPasteboard.PasteboardType("com.rubyelders.dzban.cell")))
        XCTAssertEqual((try JSONSerialization.jsonObject(with: encoded) as? [String: Any])?["isNull"] as? Bool, true)
        XCTAssertEqual(ResultCellDisplay.preview(ViewportCell(text: "NULL\n😀", length: 6, truncated: true)), "NULL\\n😀...")
    }

    func testResultStatusKeepsTransientCleanupAndTerminalOutcomesDistinct() throws {
        XCTAssertEqual(ResultStatus.describe(try resultSnapshot(rows: 5, phase: "receiving", execution: "running")), "Receiving - 5 rows")
        XCTAssertEqual(ResultStatus.describe(try resultSnapshot(rows: 5, phase: "draining", execution: "running")), "Truncated - 5 rows; waiting for cleanup")
        XCTAssertEqual(ResultStatus.describe(try resultSnapshot(rows: 5, phase: "failed_awaiting_cleanup", execution: "running")), "Result capture failed; waiting for cleanup")
        XCTAssertEqual(ResultStatus.describe(try resultSnapshot(rows: 5, termination: "canceled")), "Canceled - 5 rows")
        XCTAssertEqual(ResultStatus.describe(try resultSnapshot(rows: 0, phase: "waiting_for_schema", execution: "running")), "Waiting for schema")
    }
}
