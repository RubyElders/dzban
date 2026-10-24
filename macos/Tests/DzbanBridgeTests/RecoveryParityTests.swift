import AppKit
import DzbanBridge
@testable import DzbanApplication
import SwiftUI
import XCTest

@MainActor
final class RecoveryParityTests: XCTestCase {
    private func snapshot(_ revision: Int, session: UUID, phase: String, recoveries: UInt64, state: String = "error", error: String? = nil) throws -> HealthSnapshot {
        let status: [String: Any] = ["state": state, "recovery": phase, "recoveries": recoveries, "probing": phase == "reconnecting", "error": error as Any? ?? NSNull()]
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(HealthSnapshot.self, from: JSONSerialization.data(withJSONObject: ["revision": revision, "sessions": [["session": session.uuidString, "status": status]], "profiles": []]))
    }

    func testRecoveryPhasesAndResetCountsSurviveHealthObservation() async throws {
        let bridge = try Bridge.fixture()
        let model = ConnectionHealthModel(application: ApplicationClient(bridge: bridge))
        let session = UUID()
        var notices: [UUID] = []
        model.recovered = { notices.append($0) }
        model.apply(try snapshot(1, session: session, phase: "reconnecting", recoveries: 0))
        XCTAssertEqual(model.sessions[session]?.displayState, .connecting)
        XCTAssertEqual(model.sessions[session]?.label, "Reconnecting")
        model.apply(try snapshot(2, session: session, phase: "retry_waiting", recoveries: 0, error: "transport closed"))
        XCTAssertEqual(model.sessions[session]?.displayState, .error)
        XCTAssertEqual(model.sessions[session]?.label, "Disconnected - retrying automatically")
        XCTAssertTrue(model.sessions[session]?.detail(profile: "fixture").contains("transport closed") == true)
        model.apply(try snapshot(3, session: session, phase: "idle", recoveries: 1, state: "connected"))
        XCTAssertEqual(notices, [session])
        XCTAssertTrue(model.sessions[session]?.detail(profile: "fixture").contains(ConnectionHealth.resetMessage) == true)
        model.apply(try snapshot(3, session: session, phase: "idle", recoveries: 1, state: "connected"))
        model.apply(try snapshot(4, session: session, phase: "idle", recoveries: 1, state: "connected"))
        XCTAssertEqual(notices, [session])
        model.apply(try snapshot(5, session: session, phase: "idle", recoveries: 2, error: "version lookup failed"))
        XCTAssertEqual(notices, [session, session])
        XCTAssertEqual(model.sessions[session]?.displayState, .error)
        model.retire()
        model.apply(try snapshot(6, session: session, phase: "idle", recoveries: 3))
        XCTAssertEqual(notices.count, 2)
        try await bridge.shutdown()
    }

    func testLegacyHealthDefaultsToNoRecovery() throws {
        let decoder = JSONDecoder()
        let status = try decoder.decode(HealthStatus.self, from: Data(#"{"state":"connected","probing":false}"#.utf8))
        let health = ConnectionHealth(status)
        XCTAssertEqual(health.recoveries, 0)
        XCTAssertEqual(health.recovery, .idle)
        XCTAssertFalse(health.detail(profile: "fixture").contains("reset"))
    }

    func testRecoveryNoticePreservesDraftAndResultGeneration() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let document = try XCTUnwrap(store.document)
        store.editor(for: document).edit(text: "SELECT preserved", selection: .init(location: 0, length: 0))
        let session = try XCTUnwrap(store.selectedSession)
        let next = try snapshot(Int.max, session: session, phase: "idle", recoveries: 1, state: "connected")
        store.connectionHealth.apply(next)
        XCTAssertTrue(store.recoveryNotice?.contains(ConnectionHealth.resetMessage) == true)
        XCTAssertEqual(store.document?.document, document.document)
        XCTAssertEqual(store.document?.generation, document.generation)
        XCTAssertEqual(store.editor(for: document).text, "SELECT preserved")
        try await store.shutdown()
    }

    func testRecoveryRendersWithoutReplacingTheWorkspace() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let document = try XCTUnwrap(store.document)
        let session = try XCTUnwrap(store.selectedSession)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1150, height: 760), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        let host = NSHostingView(rootView: RootView(store: store))
        host.wantsLayer = true
        host.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        window.contentView = host
        window.makeKeyAndOrderFront(nil)
        defer { window.contentView = NSView(); window.orderOut(nil) }
        for (index, phase) in ["reconnecting", "retry_waiting", "idle"].enumerated() {
            store.connectionHealth.apply(try snapshot(Int.max - 3 + index, session: session, phase: phase, recoveries: phase == "idle" ? 1 : 0, state: phase == "idle" ? "connected" : "error"))
            try await Task.sleep(for: .milliseconds(100))
            window.contentView?.layoutSubtreeIfNeeded()
            XCTAssertEqual(store.document?.document, document.document)
            if let path = ProcessInfo.processInfo.environment["DZBAN_PARITY_SCREENSHOTS"], let view = window.contentView,
               let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
                view.cacheDisplay(in: view.bounds, to: bitmap)
                let directory = URL(fileURLWithPath: path)
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                try bitmap.representation(using: .png, properties: [:])?.write(to: directory.appendingPathComponent("recovery-\(phase).png"))
            }
        }
        XCTAssertNotNil(store.recoveryNotice)
        try await store.shutdown()
    }

    func testSpareWidthExpandsOnlyClippedAutomaticColumns() {
        let columns = [ResultColumnSizing(preferred: 56, assigned: 56), ResultColumnSizing(preferred: 720, assigned: 360)]
        XCTAssertEqual(ResultColumnSizing.fit(columns, available: 1000), [56, 720])
        XCTAssertEqual(ResultColumnSizing.fit(columns, available: 600), [56, 544])
        XCTAssertEqual(ResultColumnSizing.fit(columns, available: 300), [56, 360])
        XCTAssertEqual(ResultColumnSizing.fit([ResultColumnSizing(preferred: 56, assigned: 56), ResultColumnSizing(preferred: 120, assigned: 120)], available: 1000), [56, 120])
        XCTAssertEqual(ResultColumnSizing.fit([columns[0], ResultColumnSizing(preferred: 720, assigned: 420, manual: true)], available: 1000), [56, 420])
    }
}
