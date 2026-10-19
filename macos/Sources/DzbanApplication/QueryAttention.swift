import Combine
import DzbanBridge
import Foundation

struct QueryAttentionUpdate {
    let unreadCount: Int
    let requestAttention: Bool
    let cancelAttention: Bool
}

@MainActor
final class QueryAttentionModel: ObservableObject {
    @Published private(set) var snapshot: AttentionSnapshot?
    var changed: ((QueryAttentionUpdate) -> Void)? { didSet { publish(request: false) } }
    var failed: ((Error) -> Void)?
    private let application: ApplicationClient
    private var observation: Task<Void, Never>?
    private var requestRevision: UInt64 = 0
    private var retired = false

    init(application: ApplicationClient) { self.application = application }
    deinit { observation?.cancel() }

    func start() {
        guard observation == nil, !retired else { return }
        observation = Task { [weak self, application] in
            do {
                let stream = try await application.attentionUpdates()
                for try await snapshot in stream {
                    guard !Task.isCancelled, let self, !self.retired else { return }
                    self.apply(snapshot)
                }
            } catch is CancellationError {}
            catch { if let self, !self.retired { self.failed?(error) } }
        }
    }

    func retire() { retired = true; observation?.cancel(); observation = nil }

    func visibility(_ visible: Bool, viewed: ViewedOutput?) {
        guard !retired else { return }
        do { _ = try application.reportAttention(.visibility(visible, viewed: viewed)) }
        catch {
            DispatchQueue.main.async { [weak self] in
                guard let self, !self.retired else { return }
                self.failed?(error)
            }
        }
    }

    func report(_ input: AttentionInput) {
        guard !retired else { return }
        do { apply(try application.reportAttention(input)) }
        catch { failed?(error) }
    }

    func apply(_ next: AttentionSnapshot) {
        guard !retired else { return }
        if let snapshot, next.revision < snapshot.revision { return }
        let request = next.requestRevision > requestRevision && !next.cancelAttention
        requestRevision = max(requestRevision, next.requestRevision)
        snapshot = next
        publish(request: request)
    }

    private func publish(request: Bool) {
        guard let snapshot else { return }
        changed?(QueryAttentionUpdate(unreadCount: snapshot.unseenCount, requestAttention: request, cancelAttention: snapshot.cancelAttention))
    }
}
