import Foundation

public struct EditorActionRequest: Sendable {
    public let session: UUID
    public let document: UUID
    public let draft: String
    public let selection: Range<Int>
    public let intent: EditorIntent

    public init(session: UUID, document: UUID, draft: String, selection: Range<Int>, intent: EditorIntent) {
        self.session = session
        self.document = document
        self.draft = draft
        self.selection = selection
        self.intent = intent
    }
}

extension ApplicationClient {
    public func documentUpdates(session: UUID, document: UUID) async throws -> AsyncThrowingStream<DocumentSnapshot, Error> {
        try await bridge.observeDocument(session: session, document: document)
    }
    public func setDraft(session: UUID, document: UUID, draft: String) async throws -> DocumentSnapshot {
        try await bridge.request(.setDraft(session: session, document: document, draft: draft), as: DocumentSnapshot.self)
    }
    public func contextChanged(session: UUID, document: UUID) async throws -> DocumentSnapshot {
        try await bridge.request(.contextChanged(session: session, document: document), as: DocumentSnapshot.self)
    }
    @discardableResult public func performEditorAction(_ request: EditorActionRequest) async throws -> EditorOutcome {
        do {
            let result = try await bridge.request(.editorAction(session: request.session, document: request.document, draft: request.draft, start: request.selection.lowerBound, end: request.selection.upperBound, intent: request.intent), as: EditorOutcome.self)
            #if DZBAN_TEST_FIXTURES
            if ProcessInfo.processInfo.environment["DZBAN_TRACE_EDITOR_SCENARIO"] == "1" {
                FileHandle.standardError.write(Data("Editor command \(request.intent.rawValue): generation=\(result.submission?.generation.description ?? "none") canceled=\(result.cancellationRequested)\n".utf8))
            }
            #endif
            return result
        } catch {
            #if DZBAN_TEST_FIXTURES
            if ProcessInfo.processInfo.environment["DZBAN_TRACE_EDITOR_SCENARIO"] == "1" {
                FileHandle.standardError.write(Data("Editor command \(request.intent.rawValue) failed: \((error as? BridgeFailure)?.kind ?? "transport")\n".utf8))
            }
            #endif
            throw error
        }
    }
    public func restoreHistory(session: UUID, document: UUID, revision: UUID, index: Int) async throws -> DocumentSnapshot {
        try await bridge.request(.restoreHistory(session: session, document: document, revision: revision, index: index), as: DocumentSnapshot.self)
    }
    public func controls() async throws -> Controls {
        try await bridge.request(.controls, as: Controls.self)
    }
    public func controlUpdates(session: UUID) async throws -> AsyncThrowingStream<Controls, Error> {
        try await bridge.observeControls(session: session)
    }
    public func actions(session: UUID) async throws -> BackendActions {
        try await bridge.request(.actions(session: session), as: BackendActions.self)
    }
    public func interactionUpdates(session: UUID) async throws -> AsyncThrowingStream<InteractionProjection, Error> {
        try await bridge.observeInteraction(session: session)
    }
    public func interact(session: UUID, document: UUID, revision: UInt64, draft: String, selection: Range<Int>, event: NativeInput) async throws -> InputOutcome {
        try await bridge.request(.interact(session: session, document: document, revision: revision, draft: draft, selection: selection, event: event), as: InputOutcome.self)
    }
    public func awaitPlan(session: UUID, revision: UInt64) async throws -> InteractionProjection {
        try await bridge.request(.awaitPlan(session: session, revision: revision), as: InteractionProjection.self)
    }
}
