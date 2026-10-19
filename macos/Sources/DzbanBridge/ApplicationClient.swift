import Foundation

public struct ApplicationClient: Sendable {
    let bridge: Bridge

    public init(bridge: Bridge) { self.bridge = bridge }

    public func healthUpdates() async throws -> AsyncThrowingStream<HealthSnapshot, Error> {
        try await bridge.observeHealth()
    }
    public func checkConnection(_ session: UUID) async throws -> HealthSnapshot {
        try await bridge.request(.checkHealth(session: session), as: HealthSnapshot.self)
    }
    public func checkProfile(_ profile: String) async throws -> HealthSnapshot {
        try await bridge.request(.checkProfileHealth(profile), as: HealthSnapshot.self)
    }
    public func checkProfiles() async throws -> HealthSnapshot {
        try await bridge.request(.checkProfilesHealth, as: HealthSnapshot.self)
    }
    public func attentionUpdates() async throws -> AsyncThrowingStream<AttentionSnapshot, Error> {
        try await bridge.observeAttention()
    }
    public func reportAttention(_ input: AttentionInput) throws -> AttentionSnapshot {
        try bridge.attention(input)
    }
}
