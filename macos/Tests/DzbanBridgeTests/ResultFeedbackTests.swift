import Foundation
import DzbanBridge
@testable import DzbanApplication
import XCTest

final class ResultFeedbackTests: XCTestCase {
    private struct Fixture: Decodable {
        struct Case: Decodable { let name: String; let document: DocumentSnapshot; let text: String }
        struct Diagnostic: Decodable { let presentation: DiagnosticPresentation }
        let schema: Int
        let cases: [Case]
        let diagnostics: [Diagnostic]
    }

    func testSharedFeedbackPreservesNativeWordingAndDiagnosticFields() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let fixture = try decoder.decode(Fixture.self, from: Data(contentsOf: root.appendingPathComponent("tests/fixtures/result-feedback-v1.json")))
        XCTAssertEqual(fixture.schema, 1)
        for sample in fixture.cases {
            XCTAssertEqual(ResultStatus.describe(sample.document), sample.text, sample.name)
        }
        let postgres = fixture.diagnostics[6].presentation
        XCTAssertEqual(postgres.backend, .postgresql)
        XCTAssertEqual(postgres.databaseKind, .server)
        XCTAssertEqual(postgres.code, .sqlState("42601"))
        XCTAssertEqual(postgres.text, "Syntax error\n42601\nAt line 2\nCheck the expression")
        let clickhouse = fixture.diagnostics[7].presentation
        XCTAssertEqual(clickhouse.backend, .clickhouse)
        XCTAssertEqual(clickhouse.code, .clickhouse(62))
        XCTAssertEqual(clickhouse.text, "Syntax error\n62.0\nAt line 2\nCheck the expression")
    }

    func testInvalidDiagnosticCodesAndUnknownStatusAreRejected() {
        for value in [#"{"kind":"sql_state","value":42}"#, #"{"kind":"clickhouse","value":-1}"#, #"{"kind":"clickhouse","value":4294967296}"#, #"{"kind":"future","value":1}"#] {
            XCTAssertThrowsError(try JSONDecoder().decode(DiagnosticCode.self, from: Data(value.utf8)))
        }
        XCTAssertThrowsError(try JSONDecoder().decode(ResultStatusReason.self, from: Data(#""future""#.utf8)))
    }
}
