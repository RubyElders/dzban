import AppKit
import Combine
import DzbanBridge
@testable import DzbanApplication
import XCTest

@MainActor
final class EditorAttachmentTests: XCTestCase {
    private func connected() async throws -> (ApplicationClient, UUID, DocumentSnapshot) {
        let client = ApplicationClient(bridge: try Bridge.fixture())
        let tab = try await client.openTab()
        let connection = try await client.connect(profile: "fixture", tab: tab.tab, revision: 1)
        let snapshot = try await client.workspace().tabs.first?.snapshot
        return (client, connection.session, try XCTUnwrap(snapshot))
    }

    func testCoalescedNativeDraftDeliveryKeepsTheLatestTextAndAcknowledgement() async throws {
        let (client, session, document) = try await connected()
        let attachments = EditorAttachments(application: client, focusCycle: WorkspaceFocusCycle())
        let buffer = attachments.editor(for: document)
        var delivered: DocumentSnapshot?
        var failure: Error?
        attachments.updated = { delivered = $0; XCTAssertEqual($1, session) }
        attachments.failed = { failure = $0 }
        for number in 0..<100 {
            buffer.edit(text: "SELECT 'žluť 😀 \(number)'", selection: NSRange(location: 0, length: 0))
            attachments.draftChanged(session: session, document: document.document)
        }
        await attachments.wait(for: document.document)
        XCTAssertNil(failure)
        XCTAssertEqual(buffer.savedVersion, buffer.version)
        XCTAssertEqual(delivered?.draft, buffer.text)
        let snapshot = try await client.workspace().tabs.first?.snapshot
        XCTAssertEqual(snapshot?.draft, buffer.text)
        attachments.retireAll()
        try await client.shutdown()
    }

    func testRetiringEditorPreventsQueuedDeliveryAndReleasesNativeEditingState() async throws {
        let (client, session, document) = try await connected()
        let attachments = EditorAttachments(application: client, focusCycle: WorkspaceFocusCycle())
        let buffer = attachments.editor(for: document)
        var deliveries = 0
        var failure: Error?
        attachments.updated = { _, _ in deliveries += 1 }
        attachments.failed = { failure = $0 }
        buffer.textView.insertText("SELECT 42", replacementRange: NSRange(location: 0, length: 0))
        buffer.edit(text: buffer.textView.string, selection: buffer.textView.selectedRange())
        attachments.draftChanged(session: session, document: document.document)
        attachments.contextChanged(session: session, document: document.document)
        attachments.retire(document.document)
        await attachments.wait(for: document.document)
        XCTAssertEqual(deliveries, 0)
        XCTAssertNil(failure)
        XCTAssertNil(attachments.existing(document.document))
        XCTAssertFalse(buffer.textView.isEditable)
        XCTAssertFalse(buffer.textView.undoManager?.canUndo ?? true)
        let snapshot = try await client.workspace().tabs.first?.snapshot
        XCTAssertEqual(snapshot?.draft, document.draft)
        try await client.shutdown()
    }

    func testRetiredInteractionCannotRecreateEditorsOrPublishThroughAnAttachedHost() async throws {
        final class Host: QueryInteractionHost {
            let interactionTarget: NativeEditorTarget?
            var interactionAvailable = true
            var canPreview = true
            var refreshes = 0
            var failures = 0
            init(_ target: NativeEditorTarget) { interactionTarget = target }
            func refreshControls() async { refreshes += 1 }
            func interactionFailed(_ error: Error) { failures += 1 }
        }
        let (client, session, document) = try await connected()
        let attachments = EditorAttachments(application: client, focusCycle: WorkspaceFocusCycle())
        let plan = PlanPreviewModel()
        let interaction = QueryInteractionModel(application: client, editors: attachments, plan: plan)
        let host = Host(NativeEditorTarget(session: session, document: document))
        interaction.host = host
        var changes = 0
        let subscription = interaction.objectWillChange.sink { changes += 1 }
        interaction.retire()
        interaction.send(.preview, pressed: true, at: 0)
        interaction.clear()
        interaction.reset()
        await interaction.preview(pinned: true, display: true)
        XCTAssertNil(attachments.existing(document.document))
        XCTAssertNil(interaction.pending)
        XCTAssertNil(interaction.input)
        XCTAssertFalse(plan.visible)
        XCTAssertEqual(changes, 0)
        XCTAssertEqual(host.refreshes, 0)
        XCTAssertEqual(host.failures, 0)
        withExtendedLifetime(subscription) {}
        try await client.shutdown()
    }
}
