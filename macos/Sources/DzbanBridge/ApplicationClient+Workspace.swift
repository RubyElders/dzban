import Foundation

extension ApplicationClient {
    public func workspace() async throws -> TabInventory {
        try await bridge.request(.tabs, as: TabInventory.self)
    }
    public func openTab(revision: UInt64? = nil) async throws -> CreatedTab {
        try await bridge.request(.openTab(revision: revision), as: CreatedTab.self)
    }
    public func selectTab(_ tab: UUID, revision: UInt64) async throws -> TabInventory {
        try await bridge.request(.selectTab(tab, revision: revision), as: TabInventory.self)
    }
    public func cycleTabs(_ direction: Int, revision: UInt64) async throws -> TabInventory {
        try await bridge.request(.cycleTabs(direction, revision: revision), as: TabInventory.self)
    }
    public func closeTab(_ tab: UUID) async throws -> TabInventory {
        try await bridge.request(.closeTab(tab), as: TabInventory.self)
    }
    public func connect(profile: String, tab: UUID, revision: UInt64) async throws -> Connection {
        try await bridge.request(.connect(profile: profile, tab: tab, revision: revision), as: Connection.self)
    }
    public func cancelConnection(_ tab: UUID) async throws -> TabInventory {
        try await bridge.request(.cancelTabConnection(tab), as: TabInventory.self)
    }
    public func profiles() async throws -> [Profiles.Profile] {
        try await bridge.request(.profiles, as: Profiles.self).profiles
    }
    public func restoreWorkspace(_ location: WorkspaceLocation) async throws -> AsyncThrowingStream<WorkspaceRestoration, Error> {
        try await bridge.restoreWorkspace(location)
    }
    public func saveWorkspace(_ location: WorkspaceLocation, drafts: [WorkspaceDraft]) async throws {
        let _: [String: Bool] = try await bridge.request(.saveWorkspace(location, drafts: drafts), as: [String: Bool].self)
    }
    @discardableResult public func shutdown() async throws -> AttentionSnapshot { try await bridge.shutdown() }
}
