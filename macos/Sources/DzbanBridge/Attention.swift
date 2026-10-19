import Foundation

public struct ViewedOutput: Codable, Sendable, Equatable {
    public let tab: UUID
    public let document: UUID
    public let generation: UInt64
    public init(tab: UUID, document: UUID, generation: UInt64) { self.tab = tab; self.document = document; self.generation = generation }
}

public enum QueryCompletionOutcome: String, Codable, Sendable { case success, failure, canceled }

public struct AttentionSnapshot: Decodable, Sendable {
    public struct Tab: Decodable, Sendable {
        public struct Unseen: Decodable, Sendable {
            public struct Identity: Decodable, Sendable {
                public let output: ViewedOutput
                public let session: UUID
                public let query: UUID
            }
            public let identity: Identity
            public let outcome: QueryCompletionOutcome
        }
        public let tab: UUID
        public let document: UUID
        public let completionRevision: UInt64
        public let unseen: Unseen?
    }
    public let revision: UInt64
    public let applicationVisible: Bool
    public let requestRevision: UInt64
    public let cancelAttention: Bool
    public let tabs: [Tab]
    public var unseenCount: Int { tabs.filter { $0.unseen != nil }.count }
}

public struct AttentionInput: Encodable, Sendable {
    private enum Event: String, Encodable { case visibility, viewed, lifecycle }
    private let event: Event
    private var visible: Bool?
    private var viewed: ViewedOutput?
    private var output: ViewedOutput?
    private var restoring: Bool?
    private var quitting: Bool?
    public static func visibility(_ visible: Bool, viewed: ViewedOutput?) -> Self { Self(event: .visibility, visible: visible, viewed: viewed) }
    public static func viewed(_ output: ViewedOutput?) -> Self { Self(event: .viewed, output: output) }
    public static func lifecycle(restoring: Bool, quitting: Bool) -> Self { Self(event: .lifecycle, restoring: restoring, quitting: quitting) }
}
