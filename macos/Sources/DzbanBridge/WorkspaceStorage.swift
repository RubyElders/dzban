import Foundation

public struct WorkspaceLocation: Encodable, Sendable {
    public enum Host: String, Encodable, Sendable { case macos, gtk, cli }
    public let host: Host
    public let path: String?
    public init(host: Host, path: String? = nil) { self.host = host; self.path = path }
}

public struct WorkspaceDraft: Encodable, Sendable {
    public let tab: UUID
    public let draft: String
    public init(tab: UUID, draft: String) { self.tab = tab; self.draft = draft }
}

public struct WorkspaceRestoration: Decodable, Sendable {
    public struct Progress: Decodable, Sendable {
        public struct Issue: Decodable, Sendable {
            public let tab: UUID
            public let profile: String?
            public let message: String
        }
        public let finished: Bool
        public let restored: Bool
        public let issues: [Issue]
        public let error: String?
    }
    public let workspace: TabInventory
    public let progress: Progress
}
