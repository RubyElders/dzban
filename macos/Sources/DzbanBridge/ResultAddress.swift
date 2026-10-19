import Foundation

public struct ResultScope: Hashable, Sendable {
    public let session: UUID
    public let document: UUID
    public let generation: UInt64

    public init(session: UUID, document: UUID, generation: UInt64) {
        self.session = session
        self.document = document
        self.generation = generation
    }
}

public struct ResultAddress: Hashable, Identifiable, Sendable {
    public let scope: ResultScope
    public let row: Int
    public let column: Int
    public var id: Self { self }
    public init(scope: ResultScope, row: Int, column: Int) {
        self.scope = scope
        self.row = row
        self.column = column
    }
}
