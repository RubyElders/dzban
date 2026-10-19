import AppKit
import DzbanBridge
@testable import DzbanApplication
import XCTest

@MainActor
final class SQLSyntaxParityTests: XCTestCase {
    private func milliseconds(_ duration: Duration) -> Double {
        Double(duration.components.seconds) * 1000 + Double(duration.components.attoseconds) / 1e15
    }

    private struct Fixture: Decodable { let schema: Int; let cases: [Case] }
    private struct Case: Decodable { let name: String; let source: String; let tokens: [Token] }
    private struct Token: Decodable { let start: Int; let length: Int; let kind: String }

    func testSharedLexerMatchesFrozenNativeFixtures() async throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let fixture = try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: root.appendingPathComponent("tests/fixtures/sql-syntax-v1.json")))
        XCTAssertEqual(fixture.schema, 1)
        for sample in fixture.cases {
            let actual = try await Task.detached { try SQLTokenizer.tokens(in: sample.source, revision: .max) }.value
            XCTAssertEqual(actual.revision, .max)
            XCTAssertEqual(actual.tokens.count, sample.tokens.count, sample.name)
            XCTAssertEqual(actual.length.bytes, sample.source.utf8.count)
            XCTAssertEqual(actual.length.scalars, sample.source.unicodeScalars.count)
            XCTAssertEqual(actual.length.utf16, sample.source.utf16.count)
            for (token, expected) in zip(actual.tokens, sample.tokens) {
                XCTAssertEqual(token.start.utf16, expected.start, sample.name)
                XCTAssertEqual(token.end.utf16 - token.start.utf16, expected.length, sample.name)
                XCTAssertEqual(String(describing: token.kind), expected.kind, sample.name)
            }
        }
    }

    func testBackgroundHighlightingBenchmark() async throws {
        guard ProcessInfo.processInfo.environment["DZBAN_SYNTAX_BENCHMARK"] == "1" else { throw XCTSkip("Set DZBAN_SYNTAX_BENCHMARK=1 for synthetic highlighting measurements") }
        let sources = [
            "SELECT '😀', count(x) FROM widgets WHERE id = 42; -- sample\n",
            String(repeating: "SELECT 'žluť 😀', sum(value) FROM widgets WHERE value > 12.5e-2;\n", count: 300),
            String(repeating: "SELECT 'žluť 😀', sum(value) FROM widgets WHERE value > 12.5e-2;\n", count: 3000),
            String(repeating: "1,", count: 50_000),
            "SELECT \"some_id\", `order`, $$body$$, $tag$word$tag$",
        ]
        let clock = ContinuousClock()
        var rows: [[String: Any]] = []
        for (index, source) in sources.enumerated() {
            let sharedStart = clock.now
            let shared = try await Task.detached { try SQLTokenizer.tokens(in: source, revision: .max) }.value
            let sharedMilliseconds = milliseconds(sharedStart.duration(to: clock.now))
            XCTAssertEqual(shared.revision, .max)
            XCTAssertEqual(shared.length.bytes, source.utf8.count)
            XCTAssertEqual(shared.length.scalars, source.unicodeScalars.count)
            XCTAssertEqual(shared.length.utf16, source.utf16.count)
            let view = SQLTextView()
            view.string = source
            let colorStart = clock.now
            view.highlightSyntax()
            let highlightMilliseconds = milliseconds(colorStart.duration(to: clock.now))
            let pipeline = SQLHighlighting()
            var painted: SQLSyntaxSnapshot?
            pipeline.completed = { painted = $0 }
            let queued = clock.now
            for _ in 0..<20 { pipeline.request(source) }
            let schedulingMilliseconds = milliseconds(queued.duration(to: clock.now)) / 20
            let deadline = clock.now + .seconds(5)
            var previous = clock.now
            var gap = 0.0
            while painted == nil || view.appliedSyntaxRevision != view.syntaxRevision {
                if clock.now >= deadline { throw ViewportFailure("Highlighting benchmark timed out") }
                try await Task.sleep(for: .milliseconds(2))
                gap = max(gap, milliseconds(previous.duration(to: clock.now)))
                previous = clock.now
            }
            pipeline.retire()
            XCTAssertEqual(painted?.tokens, shared.tokens)
            let undo = try XCTUnwrap(view.undoManager)
            undo.groupsByEvent = false
            undo.beginUndoGrouping()
            var editMilliseconds = 0.0
            for _ in 0..<20 {
                let before = clock.now
                view.insertText(" ", replacementRange: NSRange(location: view.string.utf16.count, length: 0))
                editMilliseconds = max(editMilliseconds, milliseconds(before.duration(to: clock.now)))
            }
            undo.endUndoGrouping()
            let undoStart = clock.now
            undo.undo()
            let undoMilliseconds = milliseconds(undoStart.duration(to: clock.now))
            XCTAssertEqual(view.string, source)
            view.retireHighlighting()
            rows.append(["case": index, "bytes": source.utf8.count, "tokens": shared.tokens.count,
                         "sharedLexerTransportMs": sharedMilliseconds,
                         "highlightScheduleMainMs": highlightMilliseconds, "nativePaintMainMs": view.syntaxPaintMilliseconds, "scheduleMainMsPerEdit": schedulingMilliseconds,
                         "maxMainTickMsWhileTokenizing": gap, "maxInsertMainMs": editMilliseconds, "undoMainMs": undoMilliseconds])
        }
        let data = try JSONSerialization.data(withJSONObject: rows, options: [.sortedKeys])
        print("SQLSyntaxCandidate " + String(decoding: data, as: UTF8.self))
    }
}
