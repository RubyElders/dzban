import Combine
import Foundation
import DzbanBridge
@testable import DzbanApplication
import XCTest

@MainActor
final class FeatureCoordinatorTests: XCTestCase {
    private func decode<T: Decodable>(_ value: Any, as type: T.Type) throws -> T {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(type, from: JSONSerialization.data(withJSONObject: value))
    }

    func testHealthProjectionRejectsOlderSnapshotsAndLateUpdatesAfterRetirement() async throws {
        let bridge = try Bridge.fixture()
        let model = ConnectionHealthModel(application: ApplicationClient(bridge: bridge))
        let session = UUID()
        let snapshot: [String: Any] = ["revision": 5, "sessions": [["session": session.uuidString, "status": ["state": "connected", "probing": false, "version": "fixture", "latency_micros": 1250]]], "profiles": []]
        model.apply(try decode(snapshot, as: HealthSnapshot.self))
        XCTAssertEqual(model.sessions[session]?.version, "fixture")
        XCTAssertEqual(model.sessions[session]?.latencyMicros, 1250)
        model.recordFailure(profile: "missing", message: "Not available")
        XCTAssertEqual(model.profiles["missing"]?.failure, "Not available")
        model.apply(try decode(["revision": 4, "sessions": [], "profiles": []], as: HealthSnapshot.self))
        XCTAssertEqual(model.sessions[session]?.state, .connected)
        XCTAssertEqual(model.profiles["missing"]?.state, .error)
        model.apply(try decode(["revision": 6, "sessions": [], "profiles": []], as: HealthSnapshot.self))
        XCTAssertTrue(model.sessions.isEmpty)
        XCTAssertTrue(model.profiles.isEmpty)
        var changes = 0
        let subscription = model.objectWillChange.sink { changes += 1 }
        model.retire()
        model.apply(try decode(snapshot, as: HealthSnapshot.self))
        model.recordFailure(profile: "late", message: "Late failure")
        await model.probe(profile: "fixture")
        model.start()
        XCTAssertEqual(changes, 0)
        XCTAssertTrue(model.profiles.isEmpty)
        withExtendedLifetime(subscription) {}
        try await bridge.shutdown()
    }

    func testAttentionEffectAttachmentDoesNotReplayAndRetirementRejectsLateState() async throws {
        let bridge = try Bridge.fixture()
        let model = QueryAttentionModel(application: ApplicationClient(bridge: bridge))
        var updates: [QueryAttentionUpdate] = []
        model.changed = { updates.append($0) }
        let snapshot = try decode(["revision": 5, "request_revision": 3, "application_visible": false, "cancel_attention": false, "tabs": []], as: AttentionSnapshot.self)
        model.apply(snapshot)
        XCTAssertTrue(try XCTUnwrap(updates.last).requestAttention)
        model.changed = { updates.append($0) }
        XCTAssertFalse(try XCTUnwrap(updates.last).requestAttention)
        model.apply(snapshot)
        XCTAssertFalse(try XCTUnwrap(updates.last).requestAttention)
        let count = updates.count
        model.retire()
        model.apply(try decode(["revision": 6, "request_revision": 4, "application_visible": false, "cancel_attention": false, "tabs": []], as: AttentionSnapshot.self))
        model.report(.viewed(nil))
        model.visibility(true, viewed: nil)
        model.start()
        XCTAssertEqual(updates.count, count)
        XCTAssertEqual(model.snapshot?.revision, 5)
        try await bridge.shutdown()
    }
}
