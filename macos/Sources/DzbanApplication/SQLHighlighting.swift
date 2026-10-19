import DzbanBridge
import Foundation

@MainActor
final class SQLHighlighting {
    typealias Tokenize = @Sendable (String, UInt64) async throws -> SQLSyntaxSnapshot
    private struct Request { let source: String; let revision: UInt64 }
    private let delay: Duration
    private let tokenize: Tokenize
    private var pending: Request?
    private var worker: Task<Void, Never>?
    private var retired = false
    private(set) var revision: UInt64 = 0
    var isIdle: Bool { worker == nil }
    var completed: ((SQLSyntaxSnapshot) -> Void)?
    var failed: (() -> Void)?

    init(delay: Duration = .milliseconds(25), tokenize: @escaping Tokenize = SQLHighlighting.sharedTokens) {
        self.delay = delay
        self.tokenize = tokenize
    }

    @discardableResult func request(_ source: String) -> UInt64 {
        guard !retired else { return revision }
        revision &+= 1
        pending = Request(source: source, revision: revision)
        if worker == nil { worker = Task { [weak self] in await self?.run() } }
        return revision
    }

    func invalidate() { revision &+= 1; pending = nil }
    func retire() { retired = true; invalidate(); worker?.cancel() }

    private func run() async {
        defer { worker = nil }
        while !Task.isCancelled, !retired, let request = pending {
            do { try await Task.sleep(for: delay) } catch { return }
            guard pending?.revision == request.revision else { continue }
            pending = nil
            do {
                let snapshot = try await tokenize(request.source, request.revision)
                guard !Task.isCancelled, !retired, revision == request.revision else { continue }
                guard snapshot.revision == request.revision else { failed?(); continue }
                completed?(snapshot)
            } catch {
                if !Task.isCancelled, !retired, revision == request.revision { failed?() }
            }
        }
    }

    nonisolated private static func sharedTokens(_ source: String, _ revision: UInt64) async throws -> SQLSyntaxSnapshot {
        let task = Task.detached(priority: .userInitiated) { try SQLTokenizer.tokens(in: source, revision: revision) }
        return try await withTaskCancellationHandler { try await task.value } onCancel: { task.cancel() }
    }
}
