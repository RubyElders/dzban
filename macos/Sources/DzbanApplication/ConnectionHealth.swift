import Combine
import Foundation
import DzbanBridge

public struct ConnectionHealth: Sendable {
    public var state: ConnectionState = .unknown
    public var latencyMicros: UInt64?
    public var version: String?
    public var failure: String?
    public var recovery: RecoveryPhase = .idle
    public var recoveries: UInt64 = 0
    public var probing = false
    public var checkedAt: Date?
    public init() {}
    init(_ status: HealthStatus) {
        recovery = status.recovery ?? .idle
        recoveries = status.recoveries ?? 0
        state = status.state
        probing = status.probing
        latencyMicros = status.latencyMicros
        version = status.version
        failure = status.error
        checkedAt = status.checkedAtMillis.map { Date(timeIntervalSince1970: Double($0) / 1000) }
    }
    public static let resetMessage = "Session settings and open transactions were reset. Queries were not replayed."
    public var displayState: ConnectionState {
        switch recovery {
        case .reconnecting: return .connecting
        case .retryWaiting: return .error
        case .idle: return state
        }
    }
    public var label: String {
        switch recovery {
        case .reconnecting: return "Reconnecting"
        case .retryWaiting: return "Disconnected - retrying automatically"
        case .idle: return state.label
        }
    }
    public func detail(profile: String) -> String {
        (["\(label): \(profile)"] + [failure, recoveries > 0 ? Self.resetMessage : nil].compactMap { $0 }).joined(separator: "\n")
    }
    public static func latency(_ micros: UInt64?) -> String {
        guard let micros else { return "--" }
        if micros < 1000 { return "\(micros)µs" }
        if micros < 1_000_000 { return "\(micros / 1000)ms" }
        return String(format: "%.2fs", Double(micros) / 1_000_000)
    }
}

@MainActor
final class ConnectionHealthModel: ObservableObject {
    @Published private(set) var sessions: [UUID: ConnectionHealth] = [:]
    @Published private(set) var profiles: [String: ConnectionHealth] = [:]
    var failed: ((Error) -> Void)?
    var recovered: ((UUID) -> Void)?
    private let application: ApplicationClient
    private var observation: Task<Void, Never>?
    private var revision: UInt64?
    private var retired = false

    init(application: ApplicationClient) { self.application = application }
    deinit { observation?.cancel() }

    func start() {
        guard observation == nil, !retired else { return }
        observation = Task { [weak self, application] in
            do {
                let stream = try await application.healthUpdates()
                for try await snapshot in stream {
                    guard !Task.isCancelled, let self, !self.retired else { return }
                    self.apply(snapshot)
                }
            } catch is CancellationError {}
            catch { if let self, !self.retired { self.failed?(error) } }
        }
    }

    func retire() { retired = true; observation?.cancel(); observation = nil }

    func apply(_ snapshot: HealthSnapshot) {
        guard !retired else { return }
        if let revision, snapshot.revision <= revision { return }
        revision = snapshot.revision
        let previous = sessions
        sessions = Dictionary(uniqueKeysWithValues: snapshot.sessions.map { ($0.session, ConnectionHealth($0.status)) })
        profiles = Dictionary(uniqueKeysWithValues: snapshot.profiles.map { ($0.profile, ConnectionHealth($0.status)) })
        for (session, status) in sessions where status.recoveries > (previous[session]?.recoveries ?? 0) {
            recovered?(session)
        }
    }

    func recordFailure(profile: String, message: String) {
        guard !retired else { return }
        var status = ConnectionHealth()
        status.state = .error
        status.failure = message
        profiles[profile] = status
    }

    func probe(session: UUID) async {
        guard !retired else { return }
        do { apply(try await application.checkConnection(session)) }
        catch { if !retired { failed?(error) } }
    }
    func probe(profile: String) async {
        guard !retired else { return }
        do { apply(try await application.checkProfile(profile)) }
        catch { if !retired { failed?(error) } }
    }
    func probeAll() async {
        guard !retired else { return }
        do { apply(try await application.checkProfiles()) }
        catch { if !retired { failed?(error) } }
    }
}
