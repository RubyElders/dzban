import DzbanBridge
import Foundation

@MainActor
final class EditorAttachments {
    var updated: ((DocumentSnapshot, UUID) -> Void)?
    var failed: ((Error) -> Void)?
    private let application: ApplicationClient
    private let focusCycle: WorkspaceFocusCycle
    private var buffers: [UUID: EditorBuffer] = [:]
    private var contextWorkers: [UUID: Task<Void, Never>] = [:]
    private var contextVersions: [UUID: UInt64] = [:]
    private var draftWorkers: [UUID: Task<Void, Never>] = [:]
    var pending: [Task<Void, Never>] { Array(draftWorkers.values) + Array(contextWorkers.values) }

    init(application: ApplicationClient, focusCycle: WorkspaceFocusCycle) {
        self.application = application
        self.focusCycle = focusCycle
    }

    func existing(_ document: UUID) -> EditorBuffer? { buffers[document] }
    func editor(for document: DocumentSnapshot) -> EditorBuffer {
        if let buffer = buffers[document.document] { return buffer }
        let buffer = EditorBuffer(text: document.draft)
        buffer.focusCycle = focusCycle
        buffers[document.document] = buffer
        return buffer
    }

    func contextChanged(session: UUID, document: UUID) {
        guard let buffer = buffers[document] else { return }
        contextVersions[document, default: 0] &+= 1
        guard contextWorkers[document] == nil else { return }
        contextWorkers[document] = Task { [weak self, application, weak buffer] in
            defer { self?.contextWorkers[document] = nil }
            while let self, let buffer, self.buffers[document] === buffer {
                let version = self.contextVersions[document]
                do {
                    let snapshot = try await application.contextChanged(session: session, document: document)
                    guard self.buffers[document] === buffer else { return }
                    self.updated?(snapshot, session)
                } catch {
                    if self.buffers[document] === buffer { self.failed?(error) }
                    return
                }
                if self.contextVersions[document] == version { return }
            }
        }
    }

    func draftChanged(session: UUID, document: UUID) {
        guard let buffer = buffers[document], draftWorkers[document] == nil else { return }
        draftWorkers[document] = Task { [weak self, application, weak buffer] in
            defer { self?.draftWorkers[document] = nil }
            while let self, let buffer, self.buffers[document] === buffer, buffer.savedVersion < buffer.version {
                let version = buffer.version
                let draft = buffer.text
                do {
                    let snapshot = try await application.setDraft(session: session, document: document, draft: draft)
                    guard self.buffers[document] === buffer else { return }
                    buffer.acknowledge(version)
                    self.updated?(snapshot, session)
                } catch {
                    if self.buffers[document] === buffer { self.failed?(error) }
                    return
                }
            }
        }
    }

    func wait(for document: UUID) async {
        await draftWorkers[document]?.value
        await contextWorkers[document]?.value
    }

    func retire(_ document: UUID) {
        buffers.removeValue(forKey: document)?.retire()
        contextVersions.removeValue(forKey: document)
    }

    func retireAll() {
        for document in Array(buffers.keys) { retire(document) }
    }
}
