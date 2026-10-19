import Foundation
@testable import DzbanBridge
@testable import DzbanApplication
import XCTest

final class ProtocolContractTests: XCTestCase {
    private struct Fixture: Decodable {
        let schema: Int
        let tabPhases: [TabPhase]
        let executionPhases: [ExecutionPhase]
        let capturePhases: [CapturePhase]
        let captureTerminations: [CaptureTermination]
        let primaryActions: [PrimaryAction]
        let inputEffects: [InputEffect]
        let inputHints: [InputProjection.Hint]
        let documentRecord: Envelope<DocumentSnapshot>
        let plan: PlanPresentation
        let legacyPlan: PlanPresentation
        let input: InputProjection
        let interaction: InteractionProjection
        let health: HealthSnapshot
        let controls: Controls
        let attention: AttentionSnapshot
        let history: HistoryList
    }

    private func fixture() throws -> Fixture {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let data = try Data(contentsOf: root.appendingPathComponent("tests/fixtures/application-contract-v1.json"))
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(Fixture.self, from: data)
    }

    func testSharedFixturePreservesAllPhasesEffectsAndIntegerPrecision() throws {
        let value = try fixture()
        XCTAssertEqual(value.interaction.revision, UInt64.max - 1)
        XCTAssertEqual(value.interaction.draftRevision, UInt64.max)
        XCTAssertTrue(value.interaction.visible)
        XCTAssertFalse(value.interaction.loading)
        XCTAssertTrue(value.interaction.input.commands.isEmpty)
        XCTAssertEqual(value.interaction.plan?.estimate?.rows, UInt64.max - 1)
        XCTAssertNil(value.interaction.error)
        XCTAssertEqual(value.health.revision, UInt64.max - 2)
        XCTAssertEqual(value.health.sessions[0].status.latencyMicros, UInt64.max - 1)
        XCTAssertEqual(value.health.sessions[0].status.checkedAtMillis, 1_800_000_000_123)
        XCTAssertEqual(value.health.profiles.map { $0.status.state }, [.connected, .error, .unknown])
        XCTAssertTrue(value.health.profiles[2].status.probing)
        XCTAssertEqual(value.controls.revision, UInt64.max - 3)
        XCTAssertEqual(value.controls.primaryAction, .run)
        XCTAssertEqual(value.attention.revision, UInt64.max - 4)
        XCTAssertEqual(value.attention.requestRevision, UInt64.max - 5)
        XCTAssertEqual(value.attention.tabs[0].completionRevision, UInt64.max - 1)
        XCTAssertEqual(value.attention.tabs[0].unseen?.identity.output.generation, UInt64.max)
        XCTAssertEqual(value.attention.tabs[0].unseen?.outcome, .failure)
        XCTAssertEqual(value.history.entries.map(\.status), [.success, .failure])
        XCTAssertEqual(value.history.entries[0].executedAt, UInt64.max - 1)
        XCTAssertEqual(value.history.entries[0].durationMs, UInt64.max)
        XCTAssertNil(value.history.entries[1].rows)
        XCTAssertNil(value.history.entries[1].durationMs)
        XCTAssertEqual(value.schema, 1)
        XCTAssertEqual(value.tabPhases, [.choosing, .connecting, .ready, .closing, .error])
        XCTAssertEqual(value.documentRecord.schema, 1)
        XCTAssertEqual(value.documentRecord.revision, UInt64.max)
        XCTAssertEqual(value.documentRecord.payload.generation, UInt64.max - 1)
        XCTAssertEqual(value.documentRecord.payload.draftRevision, UInt64.max)
        XCTAssertEqual(value.documentRecord.payload.commandResult?.affectedRows, UInt64.max - 1)
        XCTAssertEqual(value.documentRecord.payload.draft, "SELECT 'žluť 😀';")
        XCTAssertEqual(value.plan.estimate?.rows, UInt64.max - 1)
        XCTAssertEqual(PlanFormatting.rowEstimate(value.plan), "Est. ~18446744073709551614 rows")
        XCTAssertNil(value.legacyPlan.backend)
        XCTAssertNil(value.legacyPlan.estimate)
        XCTAssertEqual(value.input.commands, value.inputEffects)
        XCTAssertEqual(value.executionPhases, [.idle, .planning, .running, .canceling, .finished])
        XCTAssertEqual(value.capturePhases, [.waitingForSchema, .receiving, .draining, .failedAwaitingCleanup, .finished])
        XCTAssertEqual(value.captureTerminations, [.complete, .limited, .canceled, .failed])
        XCTAssertEqual(value.primaryActions, [.unavailable, .run, .stop, .stopping])
        XCTAssertEqual(value.inputHints.count, 8)
        XCTAssertEqual(value.inputEffects, [.beginPreview, .cancelPreview, .showPlan, .hidePlan, .pinPlan, .runQuery, .cancelQuery])
    }

    func testUnknownRecordSchemaFailsBeforeDecodingThePayload() {
        let data = Data(#"{"schema":2,"revision":0,"payload":null}"#.utf8)
        XCTAssertThrowsError(try decodeRecord(data, as: DocumentSnapshot.self)) { error in
            XCTAssertEqual((error as? BridgeFailure)?.kind, "schema")
        }
    }

    func testUnknownEffectsAndInvalidIntegerEstimatesAreRejected() {
        XCTAssertThrowsError(try JSONDecoder().decode(HistoryOutcomeKind.self, from: Data(#""future""#.utf8)))
        XCTAssertThrowsError(try JSONDecoder().decode(InputEffect.self, from: Data(#""future_effect""#.utf8)))
        XCTAssertThrowsError(try JSONDecoder().decode(ExecutionPhase.self, from: Data(#""future_phase""#.utf8)))
        for json in [#"{"rows":-1}"#, #"{"rows":1.5}"#, #"{"rows":18446744073709551616}"#] {
            XCTAssertThrowsError(try JSONDecoder().decode(QueryEstimate.self, from: Data(json.utf8)))
        }
    }
}
