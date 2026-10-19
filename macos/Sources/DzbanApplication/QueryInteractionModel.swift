import Combine
import DzbanBridge
import Foundation

struct NativeEditorTarget {
    let session: UUID
    let document: DocumentSnapshot
}

@MainActor
protocol QueryInteractionHost: AnyObject {
    var interactionTarget: NativeEditorTarget? { get }
    var interactionAvailable: Bool { get }
    var canPreview: Bool { get }
    func refreshControls() async
    func interactionFailed(_ error: Error)
}

@MainActor
final class QueryInteractionModel: ObservableObject {
    @Published private(set) var input: InputProjection?
    weak var host: (any QueryInteractionHost)?
    private let application: ApplicationClient
    private let editors: EditorAttachments
    private let plan: PlanPreviewModel
    private(set) var pending: Task<Void, Never>?
    private var observation: Task<Void, Never>?
    private var observationContext: UInt64?
    private var revision: UInt64 = 0
    private var context: UInt64 = 0
    private var retired = false

    init(application: ApplicationClient, editors: EditorAttachments, plan: PlanPreviewModel) {
        self.application = application
        self.editors = editors
        self.plan = plan
        plan.userPin = { [weak self] in
            guard let self, self.host?.interactionAvailable == true else { return }
            self.enqueue(.pin)
        }
        plan.userClose = { [weak self] in self?.clear() }
        plan.userRelease = { [weak self] milliseconds in
            guard let self, self.host?.interactionAvailable == true else { return }
            self.enqueue(.releasePlan(milliseconds: milliseconds))
        }
        plan.userReset = { [weak self] in
            guard let self, self.host?.interactionAvailable == true else { return }
            self.enqueue(.reset)
        }
    }

    deinit { observation?.cancel() }

    func detachObservation() { observation?.cancel(); observation = nil }
    func retire() { retired = true; context &+= 1; detachObservation() }

    func send(_ key: NativeInput.Key, pressed: Bool, at: UInt64) {
        guard !retired else { return }
        if pressed, [.preview, .inspect, .submit].contains(key), let target = host?.interactionTarget,
           editors.editor(for: target.document).textView.hasMarkedText() { return }
        if pressed && key == .cancel { context &+= 1 }
        enqueue(pressed ? .press(key, at: at) : .release(key, at: at))
    }

    func reset() { enqueue(.reset) }

    func clear() {
        guard !retired else { return }
        context &+= 1
        input = nil
        detachObservation()
        observationContext = nil
        enqueue(.contextChanged, publish: false)
    }

    func preview(pinned: Bool, display: Bool) async {
        guard !retired, host?.interactionAvailable == true, let target = host?.interactionTarget,
              !editors.editor(for: target.document).textView.hasMarkedText() else { return }
        let context = context
        await host?.refreshControls()
        guard host?.canPreview == true, matches(target, context: context) else { return }
        enqueue(display ? (pinned ? .openPlan : .showPlan) : .estimate)
        await pending?.value
        guard matches(target, context: context) else { return }
        do { apply(try await application.awaitPlan(session: target.session, revision: revision), context: context) }
        catch { if self.context == context { host?.interactionFailed(error) } }
    }

    private func matches(_ target: NativeEditorTarget, context: UInt64) -> Bool {
        guard !retired, self.context == context, let selected = host?.interactionTarget else { return false }
        return selected.session == target.session && selected.document.document == target.document.document
    }

    private func apply(_ projection: InteractionProjection, context: UInt64) {
        guard !retired, self.context == context, let target = host?.interactionTarget,
              projection.input.session == target.session, projection.input.document == target.document.document,
              projection.revision > revision else { return }
        revision = projection.revision
        input = projection.input
        plan.apply(projection)
    }

    private func observe(session: UUID, context: UInt64) {
        guard observationContext != context else { return }
        detachObservation()
        observationContext = context
        observation = Task { [weak self, application] in
            do {
                let stream = try await application.interactionUpdates(session: session)
                for try await projection in stream {
                    guard !Task.isCancelled, let self, !self.retired, self.context == context else { return }
                    self.apply(projection, context: context)
                    if projection.retired { return }
                }
            } catch { if let self, !self.retired, self.context == context { self.host?.interactionFailed(error) } }
        }
    }

    private func enqueue(_ event: NativeInput, publish: Bool = true) {
        guard !retired, let target = host?.interactionTarget else { return }
        let previous = pending
        let context = context
        pending = Task { [weak self, application] in
            await previous?.value
            guard let self, self.matches(target, context: context) else { return }
            await self.editors.wait(for: target.document.document)
            guard self.matches(target, context: context), let current = self.host?.interactionTarget else { return }
            let editor = self.editors.editor(for: current.document)
            do {
                let outcome = try await application.interact(session: current.session, document: current.document.document, revision: current.document.draftRevision, draft: editor.text, selection: editor.scalarSelection, event: event)
                guard self.matches(target, context: context) else { return }
                if publish {
                    if self.observationContext != context { self.revision = 0 }
                    self.apply(outcome.interaction, context: context)
                    self.observe(session: current.session, context: context)
                    await self.host?.refreshControls()
                }
            } catch {
                if self.matches(target, context: context), (error as? BridgeFailure)?.kind != "selection_superseded" {
                    self.host?.interactionFailed(error)
                }
            }
        }
    }
}
