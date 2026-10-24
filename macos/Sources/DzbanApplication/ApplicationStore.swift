import Combine
import DzbanBridge
import Foundation

public struct Workspace: Identifiable {
    public let tab: UUID
    public let id: UUID
    public let profile: String
    public let snapshot: DocumentSnapshot
    public var documents: [DocumentSnapshot] { [snapshot] }
    public var selectedDocument: UUID? { snapshot.document }
    public let hasUnseenResult: Bool
    public let unseenResultFailed: Bool
    public let completionRevision: UInt64
}

extension TabInventory.Tab {
    public var savedProfile: String? { backend == nil ? profile : nil }
    public var savedDraft: String { snapshot.draft }
}

@MainActor
public final class ApplicationStore: ObservableObject, QueryInteractionHost {
    @Published public private(set) var backendActions: [BackendActions.Action] = []
    @Published public private(set) var profiles: [Profiles.Profile] = []
    @Published public private(set) var inventory: TabInventory?
    private struct ConnectionAttachment {
        let profile: String
        let worker: Task<Connection, Error>
    }
    @Published private var connectionAttachments: [UUID: ConnectionAttachment] = [:]
    private var cancelledConnections: Set<UUID> = []
    private var cancelingRestoredTabs: Set<UUID> = []
    private var connectionCancellations: [UUID: Task<Void, Never>] = [:]
    private var closingTabs: Set<UUID> = []
    private var workspaceBootstrap: Task<TabInventory, Error>?
    private var attention: AttentionSnapshot? { queryAttention.snapshot }

    public var pages: [TabInventory.Tab] { inventory?.tabs ?? [] }
    public var selectedPage: UUID? { inventory?.selected }
    public var selectedSession: UUID? {
        guard let page = pages.first(where: { $0.id == selectedPage }), connectionAttachments[page.id] == nil, !cancelingRestoredTabs.contains(page.id) else { return nil }
        return page.session
    }
    public var pendingProfile: String? {
        guard let page = pages.first(where: { $0.id == selectedPage }) else { return nil }
        return connectionAttachments[page.id]?.profile ?? (page.phase == .connecting ? page.profile : nil)
    }
    public var workspaces: [Workspace] {
        pages.compactMap { page in
            guard let session = page.session, let profile = page.profile, connectionAttachments[page.id] == nil else { return nil }
            return Workspace(tab: page.id, id: session, profile: profile, snapshot: page.snapshot,
                hasUnseenResult: attention?.tabs.first(where: { $0.tab == page.id })?.unseen != nil,
                unseenResultFailed: attention?.tabs.first(where: { $0.tab == page.id })?.unseen?.outcome == .failure,
                completionRevision: attention?.tabs.first(where: { $0.tab == page.id })?.completionRevision ?? 0)
        }
    }
    @Published public private(set) var closingSessions: Set<UUID> = []
    @Published public private(set) var controls: Controls?
    @Published public private(set) var isShuttingDown = false
    @Published public private(set) var isClosed = false
    @Published public var failure: String?
    @Published public var recoveryNotice: String?
    public var historyFocused = false
    @Published public var historyVisible = false {
        didSet { if historyVisible != oldValue { historyFocused = historyVisible } }
    }
    @Published var actionSearchRequest = 0
    @Published public var shortcutsVisible = false
    public var connectionsVisible: Bool { !restoring && !isShuttingDown && selectedSession == nil }
    public var health: [UUID: ConnectionHealth] { connectionHealth.sessions }
    public var profileHealth: [String: ConnectionHealth] { connectionHealth.profiles }
    public var sharedInput: InputProjection? { queryInteraction.input }

    var attentionChanged: ((QueryAttentionUpdate) -> Void)? {
        get { queryAttention.changed }
        set { queryAttention.changed = newValue }
    }
    var unseenCompletionCount: Int { attention?.unseenCount ?? 0 }

    let connectionHealth: ConnectionHealthModel
    private let queryAttention: QueryAttentionModel
    private var featureUpdates: Set<AnyCancellable> = []
    public let bridge: Bridge
    let focusCycle = WorkspaceFocusCycle()
    public let results: ResultViewport
    public let cellInspector: CellInspectorModel
    let rowInspector: RowInspectorModel
    public let plan: PlanPreviewModel
    #if DZBAN_TEST_FIXTURES
    let responsiveness = MainThreadProbe()
    #endif
    private let workspaceLocation: WorkspaceLocation?
    private var startup: Task<Void, Never>?
    @Published public private(set) var restoring = false
    private var controlRevision: UInt64 = 0
    private var navigationRevision: UInt64 = 0
    private var observations: [UUID: Task<Void, Never>] = [:]
    private var controlObservation: Task<Void, Never>?
    private var shutdownTask: Task<Void, Error>?
    private let queryInteraction: QueryInteractionModel
    private let editorAttachments: EditorAttachments
    let application: ApplicationClient

    public init(bridge: Bridge, resultSource: (any ResultSource)? = nil, restorationURL: URL? = nil, restoreWorkspace: Bool = false) {
        workspaceLocation = restorationURL.map { WorkspaceLocation(host: .macos, path: $0.path) } ?? (restoreWorkspace ? WorkspaceLocation(host: .macos) : nil)
        restoring = restorationURL != nil || restoreWorkspace
        self.bridge = bridge
        let application = ApplicationClient(bridge: bridge)
        self.application = application
        let attachments = EditorAttachments(application: application, focusCycle: focusCycle)
        editorAttachments = attachments
        connectionHealth = ConnectionHealthModel(application: application)
        queryAttention = QueryAttentionModel(application: application)
        let preview = PlanPreviewModel()
        plan = preview
        queryInteraction = QueryInteractionModel(application: application, editors: attachments, plan: preview)
        let source = resultSource ?? BridgeResultSource(application: application)
        results = ResultViewport(source: source)
        results.focusCycle = focusCycle
        cellInspector = CellInspectorModel(source: source)
        rowInspector = RowInspectorModel(application: application, source: source)
        queryInteraction.host = self
        editorAttachments.updated = { [weak self] snapshot, session in self?.apply(snapshot, session: session) }
        editorAttachments.failed = { [weak self] error in self?.show(error) }
        connectionHealth.failed = { [weak self] error in
            guard let self, !self.isClosed, !self.isShuttingDown else { return }
            self.show(error)
        }
        queryAttention.failed = { [weak self] error in
            guard let self, !self.isClosed, !self.isShuttingDown else { return }
            self.show(error)
        }
        queryInteraction.objectWillChange.sink { [weak self] in self?.objectWillChange.send() }.store(in: &featureUpdates)
        connectionHealth.objectWillChange.sink { [weak self] in self?.objectWillChange.send() }.store(in: &featureUpdates)
        queryAttention.objectWillChange.sink { [weak self] in self?.objectWillChange.send() }.store(in: &featureUpdates)
        connectionHealth.recovered = { [weak self] _ in
            guard let self, self.navigationAvailable else { return }
            self.recoveryNotice = "Connection restored. " + ConnectionHealth.resetMessage
        }
        connectionHealth.start()
        queryAttention.start()
    }

    public func start() async {
        if let startup { await startup.value; return }
        let startup = Task { await self.restoreWorkspace() }
        self.startup = startup
        await startup.value
    }

    private func restoreWorkspace() async {
        restoring = true
        reportAttentionLifecycle()
        defer { restoring = false; reportAttentionLifecycle() }
        await loadProfileData()
        do {
            if let workspaceLocation {
                let stream = try await application.restoreWorkspace(workspaceLocation)
                var handledIssues = 0
                for try await restored in stream {
                    guard navigationAvailable else { return }
                    let selected = selectedPage
                    let session = selectedSession
                    adopt(restored.workspace)
                    for page in pages {
                        if let session = page.session, observations[page.document] == nil, !cancelingRestoredTabs.contains(page.id) {
                            _ = editor(for: page.snapshot)
                            observe(session: session, document: page.document)
                            await probe(session: session)
                        }
                    }
                    if selected != selectedPage || session != selectedSession { await activateSelection() }
                    if restored.progress.issues.count > handledIssues, let issue = restored.progress.issues.last {
                        handledIssues = restored.progress.issues.count
                        failure = issue.message
                        if let profile = issue.profile {
                            connectionHealth.recordFailure(profile: profile, message: issue.message)
                        }
                    }
                    if let error = restored.progress.error { failure = error }
                    if restored.progress.finished {
                        if let document { editor(for: document).requestFocus() }
                        return
                    }
                }
            }
            try await ensureWorkspace()
        } catch {
            show(error)
            try? await ensureWorkspace()
        }
    }

    private func ensureWorkspace() async throws {
        guard navigationAvailable else { return }
        if !pages.isEmpty { return }
        if let workspaceBootstrap { adopt(try await workspaceBootstrap.value); return }
        let worker = Task { [application] in
            let current = try await application.workspace()
            if !current.tabs.isEmpty { return current }
            return try await application.openTab().workspace
        }
        workspaceBootstrap = worker
        defer { workspaceBootstrap = nil }
        adopt(try await worker.value)
    }

    private func saveWorkspace() async throws {
        guard let workspaceLocation, !restoring else { return }
        let drafts = pages.map { page in
            WorkspaceDraft(tab: page.id, draft: editorAttachments.existing(page.document)?.text ?? page.snapshot.draft)
        }
        try await application.saveWorkspace(workspaceLocation, drafts: drafts)
    }

    private func adopt(_ next: TabInventory) {
        guard !isClosed, next.revision >= (inventory?.revision ?? 0) else { return }
        var next = next
        let old = inventory?.tabs ?? []
        for index in next.tabs.indices {
            if let retained = old.first(where: { $0.id == next.tabs[index].id }), observations[retained.document] != nil {
                next.tabs[index].snapshot = retained.snapshot
            }
        }
        for removed in old where !next.tabs.contains(where: { $0.id == removed.id }) { retire(removed) }
        inventory = next
    }

    private func refreshInventory() async throws { adopt(try await application.workspace()) }

    public var workspace: Workspace? { workspaces.first { $0.id == selectedSession } }
    public var document: DocumentSnapshot? {
        guard let workspace else { return nil }
        return workspace.documents.first { $0.document == workspace.selectedDocument }
    }
    public var canPreview: Bool { currentTabAvailable && !connectionsVisible && !shortcutsVisible && controls?.primaryAction == .run && backendActions.contains { $0.id == "query.plan" } }
    public var navigationAvailable: Bool { !isShuttingDown && !isClosed }
    public var currentTabAvailable: Bool { navigationAvailable && !(selectedPage.map { closingTabs.contains($0) } ?? false) }

    public func editor(for document: DocumentSnapshot) -> EditorBuffer { editorAttachments.editor(for: document) }

    public func editorContextChanged(session: UUID, document: UUID) {
        guard navigationAvailable, editorAttachments.existing(document) != nil else { return }
        clearSharedInput()
        plan.dismiss()
        editorAttachments.contextChanged(session: session, document: document)
    }

    public func editorChanged(session: UUID, document: UUID) {
        clearSharedInput()
        plan.dismiss()
        editorAttachments.draftChanged(session: session, document: document)
    }

    public func sendEditorIntent(_ intent: EditorIntent, session: UUID? = nil, document: UUID? = nil) {
        guard let command = editorRequest(intent, session: session, document: document) else { return }
        if intent != .reset { clearSharedInput(); plan.dismiss() }
        Task { await performEditorRequest(command) }
    }

    public func performEditorIntent(_ intent: EditorIntent, session: UUID? = nil, document: UUID? = nil) async {
        guard let command = editorRequest(intent, session: session, document: document) else { return }
        if intent != .reset { clearSharedInput(); plan.dismiss() }
        await performEditorRequest(command)
    }

    private func editorRequest(_ intent: EditorIntent, session: UUID?, document: UUID?) -> EditorActionRequest? {
        guard currentTabAvailable, !connectionsVisible, !shortcutsVisible, let session = session ?? selectedSession,
              let workspace = workspaces.first(where: { $0.id == session }),
              let snapshot = workspace.documents.first(where: { $0.document == (document ?? workspace.selectedDocument) }) else { return nil }
        let editor = editor(for: snapshot)
        if editor.textView.hasMarkedText(), intent == .submit || intent == .primary { return nil }
        let selection = editor.scalarSelection
        return EditorActionRequest(session: session, document: snapshot.document, draft: editor.text, selection: selection, intent: intent)
    }

    private func performEditorRequest(_ command: EditorActionRequest) async {
        do {
            guard navigationAvailable else { return }
            try await application.performEditorAction(command)
            await refreshControls()
        } catch { show(error) }
    }

    public func openCurrentPlan() async { await previewCurrent(pinned: true) }

    public func previewCurrent(pinned: Bool = false, display: Bool = true) async {
        await queryInteraction.preview(pinned: pinned, display: display)
    }

    @discardableResult
    public func restoreHistory(session: UUID, revision: UUID, index: Int) async -> Bool {
        guard navigationAvailable, session == selectedSession, let document else { return false }
        let id = document.document
        let editor = editor(for: document)
        guard !editor.textView.hasMarkedText() else { return false }
        await editorAttachments.wait(for: id)
        guard !Task.isCancelled, navigationAvailable, selectedSession == session,
              self.document?.document == id, !editor.textView.hasMarkedText() else { return false }
        do {
            let snapshot = try await application.restoreHistory(session: session, document: id, revision: revision, index: index)
            guard workspaces.contains(where: { $0.id == session && $0.documents.contains(where: { $0.document == id }) }) else { return false }
            apply(snapshot, session: session)
            editor.restore(text: snapshot.draft)
            clearSharedInput()
            plan.dismiss()
            return true
        } catch { show(error); return false }
    }

    private func loadProfileData() async {
        do { profiles = try await application.profiles() }
        catch { show(error) }
    }

    public func loadProfiles() async {
        await loadProfileData()
        do { try await ensureWorkspace() } catch { show(error) }
    }

    @discardableResult
    public func connect(profile: String, tab requestedTab: UUID? = nil) async -> Bool {
        guard navigationAvailable else { return false }
        do { try await ensureWorkspace() } catch { show(error); return false }
        var target = requestedTab ?? selectedPage
        if requestedTab == nil, selectedSession != nil {
            do { target = try await openSharedTab(); await activateSelection() }
            catch { show(error); return false }
        }
        guard let tab = target, pages.contains(where: { $0.id == tab }), connectionAttachments[tab] == nil, !closingTabs.contains(tab) else { return false }
        navigationRevision &+= 1
        let revision = navigationRevision
        let attachment = ConnectionAttachment(profile: profile, worker: Task { [application] in
            try await application.connect(profile: profile, tab: tab, revision: revision)
        })
        connectionAttachments[tab] = attachment
        if selectedPage == tab {
            queryInteraction.detachObservation()
            controlObservation?.cancel()
            backendActions = []
            controls = nil
        }
        defer {
            if connectionAttachments[tab] != nil { connectionAttachments.removeValue(forKey: tab) }
            cancelledConnections.remove(tab)
            connectionCancellations.removeValue(forKey: tab)
        }
        do {
            let connection = try await attachment.worker.value
            if cancelledConnections.contains(tab) {
                await connectionCancellations[tab]?.value
                if let current = try? await application.workspace(),
                   let page = current.tabs.first(where: { $0.id == tab }), page.phase != .closing,
                   page.session != nil || page.phase == .connecting {
                    do { adopt(try await application.cancelConnection(tab)) }
                    catch { if navigationAvailable { show(error) } }
                }
                try? await refreshInventory()
                return false
            }
            guard !isShuttingDown, !isClosed else { return false }
            connectionAttachments.removeValue(forKey: tab)
            try await refreshInventory()
            guard let page = pages.first(where: { $0.id == tab && $0.session == connection.session }) else { return false }
            _ = editor(for: page.snapshot)
            observe(session: connection.session, document: page.document)
            if selectedPage == tab { await activateSelection() }
            await probe(session: connection.session)
            return true
        } catch {
            if cancelledConnections.contains(tab) {
                await connectionCancellations[tab]?.value
                try? await refreshInventory()
                return false
            }
            connectionAttachments.removeValue(forKey: tab)
            try? await refreshInventory()
            if (error as? BridgeFailure)?.kind == "selection_superseded" { return false }
            if pages.contains(where: { $0.id == tab }), !isShuttingDown, !isClosed {
                connectionHealth.recordFailure(profile: profile, message: (error as? BridgeFailure)?.message ?? error.localizedDescription)
                show(error)
            }
            return false
        }
    }

    public func cancelConnectionPicker() {
        guard let tab = selectedPage else { return }
        if connectionAttachments[tab] == nil, pages.first(where: { $0.id == tab })?.phase == .connecting {
            guard cancelingRestoredTabs.insert(tab).inserted else { return }
            Task { [weak self] in
                guard let self else { return }
                defer { cancelingRestoredTabs.remove(tab) }
                if pages.count > 1 { await closePage(tab) }
                else {
                    do { adopt(try await application.cancelConnection(tab)) }
                    catch { if navigationAvailable { show(error) } }
                }
            }
            return
        }
        if connectionAttachments[tab] != nil {
            guard cancelledConnections.insert(tab).inserted else { return }
            connectionCancellations[tab] = Task { [weak self, application] in
                guard let self else { return }
                if self.pages.count > 1 { await self.closePage(tab) }
                else {
                    do { self.adopt(try await application.cancelConnection(tab)) }
                    catch { try? await self.refreshInventory(); if self.navigationAvailable { self.show(error) } }
                }
            }
        } else if pages.count > 1 { Task { await closePage(tab) } }
    }

    public func chooseConnections() async {
        guard navigationAvailable else { return }
        if selectedSession != nil { await newDocument() }
    }

    public func movePage(_ page: UUID, to index: Int) async {
        guard navigationAvailable, !restoring, index >= 0, pages.contains(where: { $0.id == page }), !closingTabs.contains(page) else { return }
        do {
            adopt(try await application.moveTab(page, index: index))
            try await saveWorkspace()
        } catch { show(error) }
    }

    public func selectPage(_ page: UUID) async {
        guard navigationAvailable, pages.contains(where: { $0.id == page }), !closingTabs.contains(page) else { return }
        clearSharedInput()
        plan.dismiss()
        navigationRevision &+= 1
        do {
            adopt(try await application.selectTab(page, revision: navigationRevision))
            await activateSelection()
        } catch { show(error) }
    }

    private func activateSelection() async {
        controlObservation?.cancel()
        controlObservation = nil
        backendActions = []
        controls = nil
        reportViewedOutput()
        if let session = selectedSession {
            observeControls(session: session)
            await refreshActions(session: session)
        }
        await refreshControls()
    }

    public func closePage(_ page: UUID) async {
        guard navigationAvailable, let target = pages.first(where: { $0.id == page }), closingTabs.insert(page).inserted else { return }
        if target.session == nil, pages.count == 1 {
            defer { closingTabs.remove(page) }
            do { try await shutdown() } catch {}
            return
        }
        if connectionAttachments[page] != nil { cancelledConnections.insert(page) }
        if let session = target.session { closingSessions.insert(session) }
        defer {
            closingTabs.remove(page)
            if let session = target.session { closingSessions.remove(session) }
        }
        if selectedPage == page { clearSharedInput(); plan.dismiss() }
        do {
            await editorAttachments.wait(for: target.document)
            adopt(try await application.closeTab(page))
            if pages.isEmpty, navigationAvailable { adopt(try await application.openTab().workspace) }
            await activateSelection()
        } catch { try? await refreshInventory(); show(error) }
    }

    public func closeCurrentTab() async {
        do { try await ensureWorkspace() } catch { show(error); return }
        if let selectedPage { await closePage(selectedPage) }
    }

    public func selectSession(_ session: UUID) async {
        guard let page = pages.first(where: { $0.session == session }) else { return }
        await selectPage(page.id)
    }

    public func selectDocument(_ document: UUID) async {
        guard let page = pages.first(where: { $0.document == document }) else { return }
        await selectPage(page.id)
    }

    public func cycleDocument(_ direction: Int) async { await cycleWorkspace(direction) }

    public func cycleWorkspace(_ direction: Int) async {
        guard navigationAvailable, !pages.isEmpty else { return }
        clearSharedInput()
        plan.dismiss()
        navigationRevision &+= 1
        do {
            adopt(try await application.cycleTabs(direction, revision: navigationRevision))
            await activateSelection()
        } catch { show(error) }
    }

    private func openSharedTab() async throws -> UUID? {
        clearSharedInput()
        plan.dismiss()
        navigationRevision &+= 1
        let created = try await application.openTab(revision: navigationRevision)
        adopt(created.workspace)
        return created.tab
    }

    public func newDocument() async {
        guard navigationAvailable else { return }
        do {
            _ = try await openSharedTab()
            await activateSelection()
        } catch { show(error) }
    }

    public func closeDocument(_ document: UUID) async {
        guard let page = pages.first(where: { $0.document == document }) else { return }
        await closePage(page.id)
    }

    public func closeSession(_ session: UUID) async {
        guard let page = pages.first(where: { $0.session == session }) else { return }
        await closePage(page.id)
    }

    private func observeControls(session: UUID) {
        controlObservation?.cancel()
        controlObservation = Task { [weak self, application] in
            do {
                let stream = try await application.controlUpdates(session: session)
                for try await projection in stream {
                    guard !Task.isCancelled, let self else { return }
                    if self.selectedSession == session, projection.session == session { self.applyControls(projection) }
                }
            } catch { if let self, !self.isClosed, !(error is CancellationError) { self.show(error) } }
        }
    }

    private func refreshActions(session: UUID) async {
        do {
            let response = try await application.actions(session: session)
            if selectedSession == session, response.session == session { backendActions = response.actions }
        } catch { show(error) }
    }

    func applyControls(_ projection: Controls) {
        guard projection.revision >= controlRevision else { return }
        controlRevision = projection.revision
        controls = projection
    }

    public func refreshControls() async {
        let session = selectedSession
        do {
            let projection = try await application.controls()
            if selectedSession == session, projection.session == session { applyControls(projection) }
        } catch { if !isClosed && !isShuttingDown { show(error) } }
    }

    public func shutdown() async throws {
        if isClosed { return }
        if let shutdownTask { try await shutdownTask.value; return }
        isShuttingDown = true
        reportAttentionLifecycle()
        let task = Task { [self] in
            try await saveWorkspace()
            cellInspector.cancel()
            rowInspector.close()
            clearSharedInput()
            let pendingInput = queryInteraction.pending
            let pendingDrafts = editorAttachments.pending
            plan.dismiss()
            await pendingInput?.value
            for worker in pendingDrafts { await worker.value }
            applyAttention(try await application.shutdown())
        }
        shutdownTask = task
        defer { shutdownTask = nil; isShuttingDown = false; reportAttentionLifecycle() }
        do {
            try await task.value
            clearSharedInput()
            plan.dismiss()
            results.retire()
            #if DZBAN_TEST_FIXTURES
            responsiveness.stop()
            #endif
            connectionHealth.retire()
            queryInteraction.retire()
            controlObservation?.cancel()
            controlObservation = nil
            isClosed = true
            queryAttention.retire()
            for task in observations.values { task.cancel() }
            observations.removeAll()
            inventory = nil
            editorAttachments.retireAll()
            controls = nil
        } catch {
            if let current = try? await application.workspace() {
                adopt(current)
                isShuttingDown = false
                await activateSelection()
            }
            show(error); throw error
        }
    }

    private func observe(session: UUID, document: UUID) {
        observations[document]?.cancel()
        observations[document] = Task { [weak self, application] in
            do {
                let stream = try await application.documentUpdates(session: session, document: document)
                for try await snapshot in stream {
                    guard !Task.isCancelled, let self else { return }
                    self.apply(snapshot, session: session)
                    await self.refreshControls()
                }
            } catch is CancellationError {}
            catch { if let self, !self.isClosed { self.show(error) } }
        }
    }

    private var viewedOutput: ViewedOutput? {
        guard !connectionsVisible, let page = pages.first(where: { $0.id == selectedPage }), page.session != nil else { return nil }
        return ViewedOutput(tab: page.id, document: page.document, generation: page.snapshot.generation)
    }

    func updateApplicationVisibility(_ visible: Bool) {
        guard navigationAvailable else { return }
        queryAttention.visibility(visible, viewed: viewedOutput)
    }

    private func reportAttentionLifecycle() {
        guard !isClosed else { return }
        queryAttention.report(.lifecycle(restoring: restoring, quitting: isShuttingDown))
    }

    private func reportViewedOutput() {
        guard navigationAvailable else { return }
        queryAttention.report(.viewed(viewedOutput))
    }

    func applyAttention(_ snapshot: AttentionSnapshot) { queryAttention.apply(snapshot) }

    private func apply(_ snapshot: DocumentSnapshot, session: UUID) {
        guard let index = inventory?.tabs.firstIndex(where: { $0.session == session && $0.document == snapshot.document }),
              let retained = inventory?.tabs[index].snapshot else { return }
        guard snapshot.generation >= retained.generation, snapshot.draftRevision >= retained.draftRevision else { return }
        inventory?.tabs[index].snapshot = snapshot
        if selectedSession == session { reportViewedOutput() }
    }

    private func retire(_ page: TabInventory.Tab) {
        if let session = page.session {
            if selectedSession == session { clearSharedInput(); plan.dismiss(); controlObservation?.cancel(); controlObservation = nil; backendActions = [] }
            if results.scope?.session == session { results.retire() }
        }
        results.tablePositions.removeValue(forKey: page.document)
        observations.removeValue(forKey: page.document)?.cancel()
        editorAttachments.retire(page.document)
    }

    private func show(_ error: Error) {
        guard !Task.isCancelled, !(error is CancellationError) else { return }
        if (error as? BridgeFailure)?.kind == "selection_superseded" { return }
        failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
    }
}

extension ApplicationStore {
    public func probe(session: UUID) async {
        guard navigationAvailable else { return }
        await connectionHealth.probe(session: session)
    }

    public func probeProfile(_ profile: String) async {
        guard navigationAvailable else { return }
        await connectionHealth.probe(profile: profile)
    }

    public func probeAllProfiles() async {
        guard navigationAvailable else { return }
        await connectionHealth.probeAll()
    }

    public func sendSharedKey(_ key: NativeInput.Key, pressed: Bool, at: UInt64) {
        queryInteraction.send(key, pressed: pressed, at: at)
    }

    public func resetSharedKeyboard() { queryInteraction.reset() }
    public func clearSharedInput() { queryInteraction.clear() }

    var interactionTarget: NativeEditorTarget? {
        guard !isClosed, let session = selectedSession, let document else { return nil }
        return NativeEditorTarget(session: session, document: document)
    }
    var interactionAvailable: Bool { currentTabAvailable && selectedSession != nil }
    func interactionFailed(_ error: Error) { show(error) }
}
