import Combine
import DzbanBridge
import Foundation

public struct Workspace: Identifiable {
    public let id: UUID
    public let profile: String
    public var documents: [DocumentSnapshot]
    public var selectedDocument: UUID?
}

@MainActor
public final class ApplicationStore: ObservableObject {
    @Published public private(set) var profiles: [Profiles.Profile] = []
    @Published public private(set) var workspaces: [Workspace] = []
    @Published public private(set) var selectedSession: UUID?
    @Published public private(set) var pendingProfile: String?
    @Published public private(set) var closingSession: UUID?
    @Published public private(set) var controls: Controls?
    @Published public private(set) var isShuttingDown = false
    @Published public private(set) var isClosed = false
    @Published public var failure: String?

    public let bridge: Bridge
    private var connectionIntent: UUID?
    private var controlRevision: UInt64 = 0
    private var navigationRevision: UInt64 = 0
    private var documentRevisions: [UUID: UInt64] = [:]
    private var observations: [UUID: Task<Void, Never>] = [:]
    private var shutdownTask: Task<Void, Error>?
    private var editors: [UUID: EditorBuffer] = [:]
    private var draftWorkers: [UUID: Task<Void, Never>] = [:]

    public init(bridge: Bridge) { self.bridge = bridge }

    public var workspace: Workspace? { workspaces.first { $0.id == selectedSession } }
    public var document: DocumentSnapshot? {
        guard let workspace else { return nil }
        return workspace.documents.first { $0.document == workspace.selectedDocument }
    }
    public var navigationAvailable: Bool { closingSession == nil && !isShuttingDown && !isClosed }

    public func editor(for document: DocumentSnapshot) -> EditorBuffer {
        if let editor = editors[document.document] { return editor }
        let editor = EditorBuffer(text: document.draft)
        editors[document.document] = editor
        return editor
    }

    public func editorChanged(session: UUID, document: UUID) {
        guard let editor = editors[document], draftWorkers[document] == nil else { return }
        draftWorkers[document] = Task { [weak self, editor, bridge] in
            defer { self?.draftWorkers[document] = nil }
            while let self, self.editors[document] === editor, editor.savedVersion < editor.version {
                let version = editor.version
                let draft = editor.text
                do {
                    let snapshot = try await bridge.request(.setDraft(session: session, document: document, draft: draft), as: DocumentSnapshot.self)
                    editor.acknowledge(version)
                    self.apply(snapshot, session: session)
                } catch { self.show(error); return }
            }
        }
    }

    public func sendEditorIntent(_ intent: EditorIntent, session: UUID? = nil, document: UUID? = nil) {
        guard let command = editorCommand(intent, session: session, document: document) else { return }
        Task { await performEditorCommand(command) }
    }

    public func performEditorIntent(_ intent: EditorIntent, session: UUID? = nil, document: UUID? = nil) async {
        guard let command = editorCommand(intent, session: session, document: document) else { return }
        await performEditorCommand(command)
    }

    private func editorCommand(_ intent: EditorIntent, session: UUID?, document: UUID?) -> BridgeCommand? {
        guard navigationAvailable, let session = session ?? selectedSession,
              let workspace = workspaces.first(where: { $0.id == session }),
              let snapshot = workspace.documents.first(where: { $0.document == (document ?? workspace.selectedDocument) }) else { return nil }
        let editor = editor(for: snapshot)
        if editor.textView.hasMarkedText(), intent == .submit || intent == .primary { return nil }
        let selection = editor.scalarSelection
        return .editorAction(session: session, document: snapshot.document, draft: editor.text, start: selection.lowerBound, end: selection.upperBound, intent: intent)
    }

    private func performEditorCommand(_ command: BridgeCommand) async {
        do {
            let _: EditorOutcome = try await bridge.request(command, as: EditorOutcome.self)
            await refreshControls()
        } catch { show(error) }
    }

    public func loadProfiles() async {
        do { profiles = try await bridge.request(.profiles, as: Profiles.self).profiles }
        catch { show(error) }
    }

    @discardableResult
    public func connect(profile: String) async -> Bool {
        guard navigationAvailable, pendingProfile == nil else { return false }
        let intent = UUID()
        navigationRevision += 1
        let revision = navigationRevision
        connectionIntent = intent
        pendingProfile = profile
        selectedSession = nil
        controls = nil
        defer { pendingProfile = nil }
        do {
            let _: Controls = try await bridge.request(.selectSession(nil, revision: revision), as: Controls.self)
            let connection = try await bridge.request(.connect(profile: profile, revision: revision), as: Connection.self)
            guard connectionIntent == intent, connection.selected, !isShuttingDown else {
                struct Closed: Decodable, Sendable { let closed: UUID }
                do { let _: Closed = try await bridge.request(.closeSession(session: connection.session), as: Closed.self) }
                catch {
                    workspaces.append(Workspace(id: connection.session, profile: profile, documents: [], selectedDocument: nil))
                    show(error)
                }
                return false
            }
            workspaces.append(Workspace(id: connection.session, profile: profile, documents: [], selectedDocument: nil))
            selectedSession = connection.session
            await newDocument()
            await refreshControls()
            return true
        } catch {
            if connectionIntent == intent { show(error) }
            return false
        }
    }

    public func chooseConnections() async {
        guard navigationAvailable else { return }
        connectionIntent = nil
        navigationRevision += 1
        let revision = navigationRevision
        selectedSession = nil
        controls = nil
        controlRevision &+= 1
        do { let _: Controls = try await bridge.request(.selectSession(nil, revision: revision), as: Controls.self) }
        catch { show(error) }
    }

    public func selectSession(_ session: UUID) async {
        guard navigationAvailable, workspaces.contains(where: { $0.id == session }) else { return }
        connectionIntent = nil
        navigationRevision += 1
        let revision = navigationRevision
        selectedSession = session
        controls = nil
        do {
            let projection = try await bridge.request(.selectSession(session, revision: revision), as: Controls.self)
            if navigationRevision == revision, selectedSession == session { controls = projection }
        } catch { show(error) }
    }

    public func selectDocument(_ document: UUID) async {
        guard navigationAvailable, let session = selectedSession else { return }
        documentRevisions[session, default: 0] += 1
        let revision = documentRevisions[session]!
        do {
            let snapshot = try await bridge.request(.selectDocument(session: session, document: document, revision: revision), as: DocumentSnapshot.self)
            guard documentRevisions[session] == revision, selectedSession == session, let index = workspaces.firstIndex(where: { $0.id == session }) else { return }
            workspaces[index].selectedDocument = snapshot.document
            apply(snapshot, session: session)
            await refreshControls()
        } catch { show(error) }
    }

    public func newDocument() async {
        guard navigationAvailable, let session = selectedSession else { return }
        documentRevisions[session, default: 0] += 1
        let revision = documentRevisions[session]!
        do {
            let document = try await bridge.request(.openDocument(session: session, revision: revision), as: DocumentSnapshot.self)
            guard let index = workspaces.firstIndex(where: { $0.id == session }) else { return }
            workspaces[index].documents.append(document)
            if documentRevisions[session] == revision { workspaces[index].selectedDocument = document.document }
            observe(session: session, document: document.document)
        } catch { show(error) }
    }

    public func closeDocument(_ document: UUID) async {
        guard navigationAvailable, let session = selectedSession else { return }
        closingSession = session
        defer { closingSession = nil }
        do {
            await draftWorkers[document]?.value
            let result = try await bridge.request(.closeDocument(session: session, document: document), as: DocumentClose.self)
            if result.workspaceClosed { removeWorkspace(session) }
            else if let index = workspaces.firstIndex(where: { $0.id == session }) {
                observations.removeValue(forKey: document)?.cancel()
                editors.removeValue(forKey: document)?.retire()
                workspaces[index].documents.removeAll { $0.document == document }
                workspaces[index].selectedDocument = result.selected?.document
                if let selected = result.selected { apply(selected, session: session) }
            }
            await refreshControls()
        } catch { show(error) }
    }

    public func closeSession(_ session: UUID) async {
        guard navigationAvailable else { return }
        closingSession = session
        defer { closingSession = nil }
        do {
            struct Closed: Decodable, Sendable { let closed: UUID }
            for document in workspaces.first(where: { $0.id == session })?.documents ?? [] {
                await draftWorkers[document.document]?.value
            }
            let _: Closed = try await bridge.request(.closeSession(session: session), as: Closed.self)
            removeWorkspace(session)
            await refreshControls()
        } catch { show(error) }
    }

    public func refreshControls() async {
        controlRevision &+= 1
        let revision = controlRevision
        let session = selectedSession
        do {
            let projection = try await bridge.request(.controls, as: Controls.self)
            if revision == controlRevision, selectedSession == session, projection.session == session {
                controls = projection
            }
        } catch { if !isClosed && !isShuttingDown { show(error) } }
    }

    public func shutdown() async throws {
        if isClosed { return }
        if let shutdownTask { try await shutdownTask.value; return }
        isShuttingDown = true
        let pendingDrafts = Array(draftWorkers.values)
        let task = Task { [bridge] in
            for worker in pendingDrafts { await worker.value }
            try await bridge.shutdown()
        }
        shutdownTask = task
        defer { shutdownTask = nil; isShuttingDown = false }
        do {
            try await task.value
            isClosed = true
            for task in observations.values { task.cancel() }
            observations.removeAll()
            workspaces.removeAll()
            for editor in editors.values { editor.retire() }
            editors.removeAll()
            selectedSession = nil
            controls = nil
        } catch { show(error); throw error }
    }

    private func observe(session: UUID, document: UUID) {
        observations[document]?.cancel()
        observations[document] = Task { [weak self, bridge] in
            do {
                let stream = try await bridge.observeDocument(session: session, document: document)
                for try await snapshot in stream {
                    guard !Task.isCancelled, let self else { return }
                    self.apply(snapshot, session: session)
                    await self.refreshControls()
                }
            } catch is CancellationError {}
            catch { if let self, !self.isClosed { self.show(error) } }
        }
    }

    private func apply(_ snapshot: DocumentSnapshot, session: UUID) {
        guard let workspace = workspaces.firstIndex(where: { $0.id == session }),
              let document = workspaces[workspace].documents.firstIndex(where: { $0.document == snapshot.document }) else { return }
        let retained = workspaces[workspace].documents[document]
        guard snapshot.generation >= retained.generation, snapshot.draftRevision >= retained.draftRevision else { return }
        workspaces[workspace].documents[document] = snapshot
    }

    private func removeWorkspace(_ session: UUID) {
        if let workspace = workspaces.first(where: { $0.id == session }) {
            for document in workspace.documents {
                observations.removeValue(forKey: document.document)?.cancel()
                editors.removeValue(forKey: document.document)?.retire()
            }
        }
        workspaces.removeAll { $0.id == session }
        documentRevisions.removeValue(forKey: session)
        if selectedSession == session { selectedSession = nil; controls = nil }
    }

    private func show(_ error: Error) {
        guard !Task.isCancelled, !(error is CancellationError) else { return }
        if (error as? BridgeFailure)?.kind == "selection_superseded" { return }
        failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
    }
}
