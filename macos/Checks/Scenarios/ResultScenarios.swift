import DzbanApplication
import DzbanBridge
import Foundation

#if DZBAN_TEST_FIXTURES
extension LifecycleScenarios {
    @MainActor
    public static func nativeResultsStreamPageInspectAndRetireAcrossOutputs() async throws {
        let bridge = try Bridge.fixture()
        let store = ApplicationStore(bridge: bridge)
        await store.loadProfiles()
        let connected = await store.connect(profile: "fixture")
        try expect(connected)
        let session = store.selectedSession!
        let document = store.document!.document
        let model = store.results
        let source = BridgeResultSource(bridge: bridge)
        func submit(_ sql: String) async throws { let _: Submission = try await bridge.request(.submit(session: session, document: document, sql: sql), as: Submission.self) }
        func cancel() async throws { let _: Ack = try await bridge.request(.cancel(session: session, document: document), as: Ack.self) }

        try await submit("wait")
        let early = try await resultState(bridge, session: session, document: document) { $0.rows == 5 && $0.execution != .finished }
        model.update(session: session, document: early)
        model.show(rows: 0..<5, columns: 0..<2)
        try await resultReady { model.value(row: 4, column: 1) != nil }
        try expect(model.value(row: 0, column: 1)?.text == "žluť\nNULL😀")
        try expect(model.value(row: 1, column: 1)?.text == nil)
        let original = try await FullCellReader.read(source: source, address: ResultAddress(scope: model.scope!, row: 0, column: 1))
        try expect(original.text == "žluť\nNULL😀")
        try await cancel()
        _ = try await resultState(bridge, session: session, document: document) { $0.execution == .finished }

        try await submit("huge")
        let huge = try await resultState(bridge, session: session, document: document) { $0.generation == 2 && $0.execution == .finished }
        model.update(session: session, document: huge)
        model.show(rows: 0..<3, columns: 0..<2)
        try await resultReady { model.value(row: 2, column: 1) != nil }
        let preview = model.value(row: 0, column: 1)!
        try expect(preview.length == 2_500_000 && preview.truncated)
        try expect(preview.text?.unicodeScalars.count == 256)
        let tail = try await source.cell(ResultAddress(scope: model.scope!, row: 0, column: 1), offset: 2_499_998, count: 8)
        try expect(tail.total == 2_500_000 && tail.text == "😀😀")
        do {
            let _: ResultPage = try await bridge.request(.readPage(session: session, document: document, generation: 2, start: 0, count: 1), as: ResultPage.self)
            throw CheckFailure(message: "Huge original page did not report its byte limit")
        } catch let error as BridgeFailure { try expect(error.kind == "page_too_large") }

        try await submit("wide")
        let wide = try await resultState(bridge, session: session, document: document) { $0.generation == 3 && $0.execution == .finished }
        model.update(session: session, document: wide)
        model.show(rows: 480..<500, columns: 120..<128)
        try await resultReady { model.value(row: 499, column: 127) != nil }
        try expect(model.columns.count == 128)
        try expect(model.value(row: 499, column: 127)?.text == "r499c127")
        try expect(model.value(row: 0, column: 1) == nil)

        try await submit("limit")
        let limited = try await resultState(bridge, session: session, document: document) { $0.generation == 4 && $0.resultPhase == .draining }
        model.update(session: session, document: limited)
        model.show(rows: 9980..<10000, columns: 0..<2)
        try await resultReady { model.value(row: 9999, column: 0) != nil }
        try expect(limited.rows == 10000 && limited.cutoff != nil)
        try expect(ResultStatus.describe(limited).contains("waiting for cleanup"))
        try await cancel()
        _ = try await resultState(bridge, session: session, document: document) { $0.generation == 4 && $0.execution == .finished }

        try await submit("cleanup_failure")
        let failing = try await resultState(bridge, session: session, document: document) { $0.generation == 5 && $0.resultPhase == .failedAwaitingCleanup }
        model.update(session: session, document: failing)
        model.show(rows: 0..<5, columns: 0..<2)
        try await resultReady { model.value(row: 4, column: 1) != nil }
        try expect(failing.captureError != nil)
        try await cancel()
        _ = try await resultState(bridge, session: session, document: document) { $0.generation == 5 && $0.execution == .finished }

        try await submit("waiting")
        let waiting = try await resultState(bridge, session: session, document: document) { $0.generation == 6 && $0.resultPhase == .waitingForSchema }
        model.update(session: session, document: waiting)
        try expect(model.availableRows == 0 && model.value(row: 0, column: 0) == nil)
        try await cancel()
        _ = try await resultState(bridge, session: session, document: document) { $0.generation == 6 && $0.execution == .finished }

        try await submit("command")
        let command = try await resultState(bridge, session: session, document: document) { $0.generation == 7 && $0.execution == .finished }
        model.update(session: session, document: command)
        try expect(command.commandResult?.affectedRows == 7)
        try expect(ResultStatus.describe(command) == "7 rows affected")
        try expect(model.columns.isEmpty)
        try await submit("empty")
        let empty = try await resultState(bridge, session: session, document: document) { $0.generation == 8 && $0.execution == .finished }
        model.update(session: session, document: empty)
        try await resultReady { model.columns.count == 2 }
        try expect(model.availableRows == 0 && model.loadError == nil)
        try expect(model.peakCacheBytes <= 8 * 1024 * 1024 && model.peakInFlight <= 2)
        print("Native result viewport: peak accounted cache bytes=\(model.peakCacheBytes), peak page/schema reads=\(model.peakInFlight), requests=\(model.requestCount)")
        try await store.shutdown()
        try expect(model.cacheBytes == 0 && model.scope == nil)
    }

    @MainActor
    private static func resultState(_ bridge: Bridge, session: UUID, document: UUID, accepts: (DocumentSnapshot) -> Bool) async throws -> DocumentSnapshot {
        let stream = try await bridge.observeDocument(session: session, document: document)
        for try await snapshot in stream { if accepts(snapshot) { return snapshot } }
        throw CheckFailure(message: "Result state was not published")
    }

    @MainActor
    private static func resultReady(_ predicate: () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(10)
        while !predicate() {
            try expect(ContinuousClock.now < deadline, "Native result viewport did not load")
            try await Task.sleep(for: .milliseconds(5))
        }
    }
}
#endif
