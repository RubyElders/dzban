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
    @Published public private(set) var backendActions: [BackendActions.Action] = []
    @Published public private(set) var profiles: [Profiles.Profile] = []
    @Published public private(set) var workspaces: [Workspace] = []
    @Published public private(set) var selectedSession: UUID?
    @Published public private(set) var pendingProfile: String?
    @Published public private(set) var closingSession: UUID?
    @Published public private(set) var controls: Controls?
    @Published public private(set) var isShuttingDown = false
    @Published public private(set) var isClosed = false
    @Published public var failure: String?
    @Published public var historyVisible = true
    @Published public var shortcutsVisible = false
    @Published public private(set) var health: [UUID: ConnectionHealth] = [:]
    @Published public private(set) var profileHealth: [String: ConnectionHealth] = [:]
    @Published public private(set) var sharedInput: InputProjection?
    private var healthTimers: [UUID: Task<Void, Never>] = [:]
    private var probeWorkers: [UUID: Task<Void, Never>] = [:]
    private var profileProbeWorkers: [String: Task<Void, Never>] = [:]
    private var inputWorker: Task<Void, Never>?
    private var inputPreview: Task<Void, Never>?
    private var inputContext: UInt64 = 0
    private var inputPreviewRevision: UInt64 = 0
    private var sharedInputMode = false
    private var inputHeld: Set<NativeInput.Key> = []

    public let bridge: Bridge
    public let results: ResultViewport
    public let plan: PlanPreviewModel
    #if DZBAN_TEST_FIXTURES
    let responsiveness = MainThreadProbe()
    #endif
    private var connectionIntent: UUID?
    private var controlRevision: UInt64 = 0
    private var navigationRevision: UInt64 = 0
    private var documentRevisions: [UUID: UInt64] = [:]
    private var observations: [UUID: Task<Void, Never>] = [:]
    private var controlObservation: Task<Void, Never>?
    private var shutdownTask: Task<Void, Error>?
    private var editors: [UUID: EditorBuffer] = [:]
    private var contextWorkers: [UUID: Task<Void, Never>] = [:]
    private var contextVersions: [UUID: UInt64] = [:]
    private var draftWorkers: [UUID: Task<Void, Never>] = [:]

    public init(bridge: Bridge, resultSource: (any ResultSource)? = nil) {
        self.bridge = bridge
        plan = PlanPreviewModel(bridge: bridge)
        results = ResultViewport(source: resultSource ?? BridgeResultSource(bridge: bridge))
        plan.userPin = { [weak self] in
            guard let self, self.sharedInputMode else { return false }
            self.enqueueInput(.pin)
            return true
        }
        plan.userClose = { [weak self] in self?.clearSharedInput() }
    }

    public var workspace: Workspace? { workspaces.first { $0.id == selectedSession } }
    public var document: DocumentSnapshot? {
        guard let workspace else { return nil }
        return workspace.documents.first { $0.document == workspace.selectedDocument }
    }
    public var canPreview: Bool { navigationAvailable && controls?.primaryAction == "run" && backendActions.contains { $0.id == "query.plan" } }
    public var navigationAvailable: Bool { closingSession == nil && !isShuttingDown && !isClosed }

    public func editor(for document: DocumentSnapshot) -> EditorBuffer {
        if let editor = editors[document.document] { return editor }
        let editor = EditorBuffer(text: document.draft)
        editors[document.document] = editor
        return editor
    }

    public func editorContextChanged(session: UUID, document: UUID) {
        guard navigationAvailable, editors[document] != nil else { return }
        clearSharedInput()
        plan.dismiss()
        contextVersions[document, default: 0] &+= 1
        guard contextWorkers[document] == nil else { return }
        contextWorkers[document] = Task { [weak self, bridge] in
            defer { self?.contextWorkers[document] = nil }
            while let self, self.editors[document] != nil {
                let version = self.contextVersions[document]
                do {
                    let snapshot = try await bridge.request(.contextChanged(session: session, document: document), as: DocumentSnapshot.self)
                    self.apply(snapshot, session: session)
                } catch { self.show(error); return }
                if self.contextVersions[document] == version { return }
            }
        }
    }

    public func editorChanged(session: UUID, document: UUID) {
        clearSharedInput()
        plan.dismiss()
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
        if intent != .reset { clearSharedInput(); plan.dismiss() }
        Task { await performEditorCommand(command) }
    }

    public func performEditorIntent(_ intent: EditorIntent, session: UUID? = nil, document: UUID? = nil) async {
        guard let command = editorCommand(intent, session: session, document: document) else { return }
        if intent != .reset { clearSharedInput(); plan.dismiss() }
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

    private func performEditorCommand(_ command: BridgeCommand, context: UInt64? = nil) async {
        do {
            await plan.waitForCleanup()
            if let session = selectedSession {
                await probeWorkers[session]?.value
                guard selectedSession == session, let workspace = workspaces.first(where: { $0.id == session }) else { return }
                if !workspace.documents.contains(where: { ["planning", "running", "canceling"].contains($0.execution) }) {
                    let _: JSONValue = try await bridge.request(.awaitIdle(session: session), as: JSONValue.self)
                }
            }
            if let context, context != inputContext { return }
            guard navigationAvailable else { return }
            let _: EditorOutcome = try await bridge.request(command, as: EditorOutcome.self)
            await refreshControls()
        } catch { show(error) }
    }

    public func previewCurrent(pinned: Bool = false, display: Bool = true) async {
        guard canPreview, let session = selectedSession, let document else { return }
        let editor = editor(for: document)
        guard !editor.textView.hasMarkedText() else { return }
        let text = editor.text
        let selection = editor.scalarSelection
        await draftWorkers[document.document]?.value
        await contextWorkers[document.document]?.value
        guard !Task.isCancelled, selectedSession == session, self.document?.document == document.document, editor.text == text,
              let current = self.document else { return }
        await plan.show(session: session, document: current, text: text, selection: selection, pinned: pinned, display: display)
    }

    @discardableResult
    public func restoreHistory(session: UUID, revision: UUID, index: Int) async -> Bool {
        guard navigationAvailable, session == selectedSession, let document else { return false }
        let id = document.document
        let editor = editor(for: document)
        guard !editor.textView.hasMarkedText() else { return false }
        await draftWorkers[id]?.value
        await contextWorkers[id]?.value
        guard !Task.isCancelled, navigationAvailable, selectedSession == session,
              self.document?.document == id, !editor.textView.hasMarkedText() else { return false }
        do {
            let snapshot = try await bridge.request(.restoreHistory(session: session, document: id, revision: revision, index: index), as: DocumentSnapshot.self)
            guard workspaces.contains(where: { $0.id == session && $0.documents.contains(where: { $0.document == id }) }) else { return false }
            apply(snapshot, session: session)
            editor.restore(text: snapshot.draft)
            clearSharedInput()
            plan.dismiss()
            return true
        } catch { show(error); return false }
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
        controlObservation?.cancel()
        selectedSession = nil
        backendActions = []
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
            observeControls(session: connection.session)
            await refreshActions(session: connection.session)
            await refreshControls()
            var status = ConnectionHealth()
            status.state = "connected"
            health[connection.session] = status
            await probe(session: connection.session)
            startHealth(connection.session)
            return true
        } catch {
            if connectionIntent == intent {
                var status = ConnectionHealth()
                status.state = "error"
                status.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
                profileHealth[profile] = status
                show(error)
            }
            return false
        }
    }

    public func chooseConnections() async {
        clearSharedInput()
        plan.dismiss()
        guard navigationAvailable else { return }
        connectionIntent = nil
        navigationRevision += 1
        let revision = navigationRevision
        controlObservation?.cancel()
        selectedSession = nil
        backendActions = []
        controls = nil
        controlRevision &+= 1
        do { let _: Controls = try await bridge.request(.selectSession(nil, revision: revision), as: Controls.self) }
        catch { show(error) }
    }

    public func selectSession(_ session: UUID) async {
        clearSharedInput()
        plan.dismiss()
        guard navigationAvailable, workspaces.contains(where: { $0.id == session }) else { return }
        connectionIntent = nil
        navigationRevision += 1
        let revision = navigationRevision
        selectedSession = session
        backendActions = []
        controls = nil
        do {
            let projection = try await bridge.request(.selectSession(session, revision: revision), as: Controls.self)
            if navigationRevision == revision, selectedSession == session { controls = projection; observeControls(session: session); await refreshActions(session: session) }
        } catch { show(error) }
    }

    public func selectDocument(_ document: UUID) async {
        clearSharedInput()
        plan.dismiss()
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

    public func cycleDocument(_ direction: Int) async {
        guard let workspace, !workspace.documents.isEmpty else { return }
        let index = workspace.documents.firstIndex { $0.document == workspace.selectedDocument } ?? 0
        await selectDocument(workspace.documents[(index + direction + workspace.documents.count) % workspace.documents.count].document)
    }

    public func cycleWorkspace(_ direction: Int) async {
        guard !workspaces.isEmpty else { return }
        let index = workspaces.firstIndex { $0.id == selectedSession } ?? 0
        await selectSession(workspaces[(index + direction + workspaces.count) % workspaces.count].id)
    }

    public func newDocument() async {
        clearSharedInput()
        plan.dismiss()
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
        clearSharedInput()
        plan.dismiss()
        guard navigationAvailable, let session = selectedSession else { return }
        closingSession = session
        defer { closingSession = nil }
        do {
            await draftWorkers[document]?.value
            await contextWorkers[document]?.value
            await probeWorkers[session]?.value
            await plan.waitForCleanup()
            let _: JSONValue = try await bridge.request(.awaitIdle(session: session), as: JSONValue.self)
            let result = try await bridge.request(.closeDocument(session: session, document: document), as: DocumentClose.self)
            if result.workspaceClosed { removeWorkspace(session) }
            else if let index = workspaces.firstIndex(where: { $0.id == session }) {
                observations.removeValue(forKey: document)?.cancel()
                editors.removeValue(forKey: document)?.retire()
                contextVersions.removeValue(forKey: document)
                if results.scope?.document == document { results.retire() }
                workspaces[index].documents.removeAll { $0.document == document }
                workspaces[index].selectedDocument = result.selected?.document
                if let selected = result.selected { apply(selected, session: session) }
            }
            await refreshControls()
        } catch { show(error) }
    }

    public func closeSession(_ session: UUID) async {
        guard navigationAvailable else { return }
        if selectedSession == session { clearSharedInput(); plan.dismiss() }
        closingSession = session
        defer { closingSession = nil }
        do {
            struct Closed: Decodable, Sendable { let closed: UUID }
            for document in workspaces.first(where: { $0.id == session })?.documents ?? [] {
                await draftWorkers[document.document]?.value
                await contextWorkers[document.document]?.value
            }
            await probeWorkers[session]?.value
            await plan.waitForCleanup()
            let _: JSONValue = try await bridge.request(.awaitIdle(session: session), as: JSONValue.self)
            let _: Closed = try await bridge.request(.closeSession(session: session), as: Closed.self)
            removeWorkspace(session)
            await refreshControls()
        } catch { show(error) }
    }

    private func observeControls(session: UUID) {
        controlObservation?.cancel()
        controlObservation = Task { [weak self, bridge] in
            do {
                let stream = try await bridge.observeControls(session: session)
                for try await projection in stream {
                    guard !Task.isCancelled, let self else { return }
                    if self.selectedSession == session, projection.session == session { self.controlRevision &+= 1; self.controls = projection }
                }
            } catch { if let self, !self.isClosed, !(error is CancellationError) { self.show(error) } }
        }
    }

    private func refreshActions(session: UUID) async {
        do {
            let response = try await bridge.request(.actions(session: session), as: BackendActions.self)
            if selectedSession == session, response.session == session { backendActions = response.actions }
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
        clearSharedInput()
        inputPreview?.cancel()
        for task in healthTimers.values { task.cancel() }
        healthTimers.removeAll()
        let pendingHealth = Array(probeWorkers.values) + Array(profileProbeWorkers.values)
        let pendingInput = inputWorker
        let pendingDrafts = Array(draftWorkers.values) + Array(contextWorkers.values)
        plan.dismiss()
        let sessions = workspaces.map(\.id)
        let task = Task { [bridge, plan] in
            for worker in pendingHealth { await worker.value }
            await pendingInput?.value
            await plan.waitForCleanup()
            for worker in pendingDrafts { await worker.value }
            for session in sessions { let _: JSONValue = try await bridge.request(.awaitIdle(session: session), as: JSONValue.self) }
            try await bridge.shutdown()
        }
        shutdownTask = task
        defer { shutdownTask = nil; isShuttingDown = false }
        do {
            try await task.value
            clearSharedInput()
            plan.dismiss()
            results.retire()
            #if DZBAN_TEST_FIXTURES
            responsiveness.stop()
            #endif
            controlObservation?.cancel()
            controlObservation = nil
            isClosed = true
            for task in observations.values { task.cancel() }
            observations.removeAll()
            workspaces.removeAll()
            for editor in editors.values { editor.retire() }
            editors.removeAll()
            selectedSession = nil
            controls = nil
        } catch {
            for workspace in workspaces { startHealth(workspace.id) }
            show(error); throw error
        }
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
        healthTimers.removeValue(forKey: session)?.cancel()
        probeWorkers.removeValue(forKey: session)
        health.removeValue(forKey: session)
        if selectedSession == session { clearSharedInput(); plan.dismiss(); controlObservation?.cancel(); controlObservation = nil; backendActions = [] }
        if results.scope?.session == session { results.retire() }
        if let workspace = workspaces.first(where: { $0.id == session }) {
            for document in workspace.documents {
                observations.removeValue(forKey: document.document)?.cancel()
                editors.removeValue(forKey: document.document)?.retire()
                contextVersions.removeValue(forKey: document.document)
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

extension ApplicationStore {
    private func startHealth(_ session: UUID) {
        healthTimers[session]?.cancel()
        healthTimers[session] = Task { [weak self] in
            while !Task.isCancelled {
                do { try await Task.sleep(for: .seconds(5)) } catch { return }
                guard self?.workspaces.contains(where: { $0.id == session }) == true else { return }
                await self?.probe(session: session)
            }
        }
    }

    public func probe(session: UUID) async {
        guard navigationAvailable, let workspace = workspaces.first(where: { $0.id == session }),
              !workspace.documents.contains(where: { ["planning", "running", "canceling"].contains($0.execution) }),
              selectedSession != session || (!plan.loading && inputHeld.isEmpty), probeWorkers[session] == nil else { return }
        let version = health[session]?.version == nil
        health[session, default: ConnectionHealth()].probing = true
        let worker = Task { [weak self, bridge] in
            do {
                let result = try await bridge.request(.probe(session: session, version: version), as: ConnectionProbe.self)
                guard let self, self.workspaces.contains(where: { $0.id == session }) else { return }
                var status = self.health[session] ?? ConnectionHealth()
                status.probing = false
                if result.state != "busy" {
                    status.state = result.state
                    status.latencyMicros = result.latencyMicros
                    status.failure = result.error
                    status.checkedAt = Date()
                    if let version = result.version { status.version = version }
                }
                self.health[session] = status
                self.profileHealth[workspace.profile] = status
            } catch {
                guard let self, self.workspaces.contains(where: { $0.id == session }) else { return }
                var status = self.health[session] ?? ConnectionHealth()
                status.probing = false
                status.state = "error"
                status.latencyMicros = nil
                status.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
                self.health[session] = status
            }
        }
        probeWorkers[session] = worker
        await worker.value
        probeWorkers[session] = nil
    }

    public func probeProfile(_ profile: String) async {
        guard navigationAvailable, profileProbeWorkers[profile] == nil else { return }
        if let workspace = workspaces.first(where: { $0.profile == profile }) { await probe(session: workspace.id); return }
        profileHealth[profile, default: ConnectionHealth()].probing = true
        let worker = Task { [weak self, bridge] in
            do {
                let result = try await bridge.request(.probe(profile: profile), as: ConnectionProbe.self)
                guard let self else { return }
                var status = ConnectionHealth()
                status.state = result.state
                status.latencyMicros = result.latencyMicros
                status.version = result.version
                status.failure = result.error
                status.checkedAt = Date()
                self.profileHealth[profile] = status
            } catch {
                guard let self else { return }
                var status = ConnectionHealth()
                status.state = "error"
                status.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
                self.profileHealth[profile] = status
            }
        }
        profileProbeWorkers[profile] = worker
        await worker.value
        profileProbeWorkers[profile] = nil
    }

    public func probeAllProfiles() async { for profile in profiles { await probeProfile(profile.name) } }

    public func sendSharedKey(_ key: NativeInput.Key, pressed: Bool, at: UInt64) {
        if pressed { inputHeld.insert(key) } else { inputHeld.remove(key) }
        if !pressed && !sharedInputMode { return }
        sharedInputMode = true
        if pressed && key == .cancel { inputContext &+= 1 }
        enqueueInput(pressed ? .press(key, at: at) : .release(key, at: at))
    }

    public func resetSharedKeyboard() {
        inputHeld.removeAll()
        if sharedInputMode { enqueueInput(.reset) } else { plan.reset() }
    }

    public func clearSharedInput() {
        inputContext &+= 1
        inputPreviewRevision &+= 1
        inputPreview?.cancel()
        inputPreview = nil
        inputHeld.removeAll()
        sharedInputMode = false
        sharedInput = nil
        enqueueInput(.contextChanged, publish: false)
    }

    public func prepareLegacyPreview() async {
        clearSharedInput()
        plan.dismiss()
        await inputWorker?.value
        await plan.waitForCleanup()
        if let session = selectedSession { await probeWorkers[session]?.value }
    }

    private func enqueueInput(_ event: NativeInput, publish: Bool = true) {
        guard let session = selectedSession, let id = document?.document, !isClosed else { return }
        let previous = inputWorker
        let context = inputContext
        inputWorker = Task { [weak self, bridge] in
            await previous?.value
            guard let self, self.inputContext == context, self.selectedSession == session, self.document?.document == id else { return }
            await self.draftWorkers[id]?.value
            await self.contextWorkers[id]?.value
            await self.probeWorkers[session]?.value
            guard self.inputContext == context, self.selectedSession == session, let document = self.document, document.document == id else { return }
            do {
                let projection = try await bridge.request(.input(session: session, document: id, revision: document.draftRevision, event: event), as: InputProjection.self)
                guard self.inputContext == context, self.selectedSession == session, self.document?.document == id else { return }
                if publish {
                    self.sharedInput = projection
                    await self.performInputCommands(projection.commands, context: context)
                }
            } catch {
                if (error as? BridgeFailure)?.kind != "selection_superseded" { self.show(error) }
            }
        }
    }

    private func performInputCommands(_ commands: [String], context: UInt64) async {
        for command in commands {
            switch command {
            case "begin_preview":
                inputPreviewRevision &+= 1
                let revision = inputPreviewRevision
                inputPreview?.cancel()
                inputPreview = Task { [weak self] in
                    guard let self else { return }
                    await self.refreshControls()
                    await self.previewCurrent(display: false)
                    guard !Task.isCancelled, self.inputContext == context, self.inputPreviewRevision == revision else { return }
                    if let plan = self.plan.plan { self.enqueueInput(.ready(plan)) }
                    else if let failure = self.plan.failure { self.enqueueInput(.failed(failure)) }
                }
            case "cancel_preview":
                inputPreviewRevision &+= 1
                inputPreview?.cancel()
                plan.dismiss()
                enqueueInput(.previewCanceled)
            case "show_plan": plan.present(visible: true)
            case "pin_plan": plan.present(visible: true, pinned: true)
            case "hide_plan": plan.present(visible: false)
            case "run_query":
                inputPreviewRevision &+= 1
                inputPreview?.cancel()
                plan.dismiss()
                if let command = editorCommand(.primary, session: nil, document: nil) { await performEditorCommand(command, context: context) }
            case "cancel_query":
                if let command = editorCommand(.cancel, session: nil, document: nil) { await performEditorCommand(command, context: context) }
            default: break
            }
        }
    }
}
