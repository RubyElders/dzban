import Foundation
import DzbanBridge

#if DZBAN_TEST_FIXTURES

public enum LifecycleScenarios {
    struct CheckFailure: Error, CustomStringConvertible {
        let message: String
        var description: String { message }
    }
    static func expect(_ condition: @autoclosure () -> Bool, _ message: String = "Check failed", file: StaticString = #filePath, line: UInt = #line) throws {
        if !condition() { throw CheckFailure(message: "\(file):\(line): \(message)") }
    }
    struct Ack: Decodable, Sendable { let cancellationRequested: Bool }
    struct Closed: Decodable, Sendable { let closed: UUID }

    public static func connectStreamCancelRestartAndShutdownThroughSwift() async throws {
        let bridge = try Bridge.fixture()
        let connection = try await bridge.request(.connect(profile: "fixture"), as: Connection.self)
        let document = try await bridge.request(.openDocument(session: connection.session), as: DocumentSnapshot.self)
        let draft = try await bridge.request(.setDraft(session: connection.session, document: document.document, draft: "rows"), as: DocumentSnapshot.self)
        let preview = try await bridge.request(.preview(session: connection.session, document: document.document, sql: "rows", revision: draft.draftRevision), as: Preview.self)
        try expect(preview.document == document.document)
        try expect(preview.draftRevision == draft.draftRevision)
        do {
            let _: Preview = try await bridge.request(.preview(session: connection.session, document: document.document, sql: "rows", revision: 0), as: Preview.self)
            throw CheckFailure(message: "Accepted a stale draft preview")
        } catch let error as BridgeFailure { try expect(error.kind == "preview_invalidated") }
        let stream = try await bridge.observeDocument(session: connection.session, document: document.document)
        var iterator = stream.makeAsyncIterator()
        let initial = try await iterator.next()
        try expect(initial?.execution == .idle)
        let submission = try await bridge.request(.submit(session: connection.session, document: document.document, sql: "wait"), as: Submission.self)
        var early: DocumentSnapshot?
        for try await snapshot in stream {
            if snapshot.rows >= 5 { early = snapshot; break }
        }
        try expect(early?.generation == submission.generation)
        try expect(early?.execution != .finished)
        let otherConnection = try await bridge.request(.selectSession(nil), as: Controls.self)
        try expect(otherConnection.session == nil)
        let parallel = try await bridge.request(.connect(profile: "fixture"), as: Connection.self)
        let other = try await bridge.request(.openDocument(session: parallel.session), as: DocumentSnapshot.self)
        try expect(other.document != document.document)
        try expect(parallel.session != connection.session)
        let _: Submission = try await bridge.request(.submit(session: parallel.session, document: other.document, sql: "rows"), as: Submission.self)
        let parallelStream = try await bridge.observeDocument(session: parallel.session, document: other.document)
        for try await snapshot in parallelStream { if snapshot.execution == .finished { try expect(snapshot.rows == 500); break } }
        let detachable = try await bridge.observeDocument(session: connection.session, document: document.document)
        let listener = Task { for try await _ in detachable {} }
        listener.cancel()
        _ = try? await listener.value
        let page = try await bridge.request(.readPage(session: connection.session, document: document.document, generation: submission.generation, start: 0, count: 5), as: ResultPage.self)
        try expect(page.page.rows[1][1] == nil)
        try expect(page.page.rows[0][1] == "žluť\nNULL😀")
        let chunk = try await bridge.request(.readCell(session: connection.session, document: document.document, generation: submission.generation, row: 0, column: 1, offset: 9, count: 1), as: CellChunk.self)
        try expect(chunk.text == "😀")
        try expect(chunk.total == 10)
        let _: Ack = try await bridge.request(.cancel(session: connection.session, document: document.document), as: Ack.self)
        let finishing = try await bridge.observeDocument(session: connection.session, document: document.document)
        for try await snapshot in finishing { if snapshot.execution == .finished { break } }
        let restarted = try await bridge.request(.submit(session: connection.session, document: document.document, sql: "rows"), as: Submission.self)
        try expect(restarted.generation > submission.generation)
        let completed = try await bridge.observeDocument(session: connection.session, document: document.document)
        for try await snapshot in completed {
            if snapshot.execution == .finished { try expect(snapshot.rows == 500); break }
        }
        let lateObserver = try await bridge.observeDocument(session: connection.session, document: document.document)
        var lateIterator = lateObserver.makeAsyncIterator()
        let retained = try await lateIterator.next()
        try expect(retained?.execution == .finished)
        try expect(retained?.rows == 500)
        let finalPage = try await bridge.request(.readPage(session: connection.session, document: document.document, generation: restarted.generation, start: 450, count: 50), as: ResultPage.self)
        try expect(finalPage.page.rows.count == 50)
        try expect(finalPage.page.rows.last?.first == "499")
        let history = try await bridge.request(.history(session: connection.session), as: History.self)
        try expect(history.entries.contains(where: { $0.sql == "rows" }))
        try expect(history.entries.contains(where: { $0.sql == "wait" }))
        do {
            let _: ResultPage = try await bridge.request(.readPage(session: connection.session, document: document.document, generation: submission.generation, start: 0, count: 1), as: ResultPage.self)
            throw CheckFailure(message: "Accepted a stale result generation")
        } catch let error as BridgeFailure { try expect(error.kind == "stale_output") }
        let _: Submission = try await bridge.request(.submit(session: connection.session, document: document.document, sql: "fail"), as: Submission.self)
        let failing = try await bridge.observeDocument(session: connection.session, document: document.document)
        for try await snapshot in failing {
            if snapshot.execution == .finished { try expect(snapshot.error != nil); break }
        }
        let _: Submission = try await bridge.request(.submit(session: connection.session, document: document.document, sql: "rows"), as: Submission.self)
        let recovered = try await bridge.observeDocument(session: connection.session, document: document.document)
        for try await snapshot in recovered {
            if snapshot.execution == .finished { try expect(snapshot.error == nil); break }
        }
        let _: Closed = try await bridge.request(.closeSession(session: connection.session), as: Closed.self)
        try await bridge.shutdown()
    }

    public static func cleanupFailureCanRetryAndRepeatedAppsReleaseTheirRuntime() async throws {
        for _ in 0..<10 {
            let bridge = try Bridge.fixture()
            let connection = try await bridge.request(.connect(profile: "fixture"), as: Connection.self)
            let document = try await bridge.request(.openDocument(session: connection.session), as: DocumentSnapshot.self)
            let _: Submission = try await bridge.request(.submit(session: connection.session, document: document.document, sql: "reject_close"), as: Submission.self)
            let stream = try await bridge.observeDocument(session: connection.session, document: document.document)
            for try await snapshot in stream { if snapshot.execution == .finished { break } }
            do { try await bridge.shutdown(); throw CheckFailure(message: "Cleanup rejection was lost") }
            catch let error as BridgeFailure { try expect(error.kind == "shutdown") }
            try await bridge.shutdown()
        }
    }
}

#endif
