import Foundation

extension ApplicationClient {
    public func resultSchema(_ scope: ResultScope) async throws -> ResultSchema {
        try await bridge.request(.resultSchema(session: scope.session, document: scope.document, generation: scope.generation), as: ResultSchema.self)
    }
    public func resultPage(_ scope: ResultScope, start: Int, count: Int, columnStart: Int, columnCount: Int) async throws -> ViewportPage {
        try await bridge.request(.readViewport(session: scope.session, document: scope.document, generation: scope.generation, start: start, count: count, columnStart: columnStart, columnCount: columnCount), as: ViewportPage.self)
    }
    public func resultCell(_ address: ResultAddress, offset: Int, count: Int) async throws -> CellChunk {
        let scope = address.scope
        return try await bridge.request(.readCell(session: scope.session, document: scope.document, generation: scope.generation, row: address.row, column: address.column, offset: offset, count: count), as: CellChunk.self)
    }
    public func export(_ scope: ResultScope, row: Int? = nil, column: Int? = nil, ranges: [ExportRowRange]? = nil) async throws -> (text: String, isNull: Bool) {
        let id = UUID()
        let bridge = bridge
        let start = Task.detached {
            try await bridge.request(.startExport(id, session: scope.session, document: scope.document, generation: scope.generation, row: row, column: column, ranges: ranges), as: [String: Bool].self)
        }
        _ = try await start.value
        defer { Task { _ = try? await bridge.request(.closeExport(id), as: [String: Bool].self) } }
        var text = ""
        while true {
            try Task.checkCancellation()
            let chunk = try await bridge.request(.readExport(id), as: ExportChunk.self)
            try Task.checkCancellation()
            text.append(chunk.text)
            if chunk.done { return (text, chunk.isNull) }
        }
    }
    public func history(session: UUID) async throws -> HistoryList {
        try await bridge.request(.historyList(session: session), as: HistoryList.self)
    }
    public func savedPlan(_ scope: ResultScope) async throws -> PlanPresentation? {
        try await bridge.request(.savedPlan(session: scope.session, document: scope.document, generation: scope.generation), as: SavedPlan.self).plan
    }
    public func rowVisibility(session: UUID, event: PeekEvent, elapsedMilliseconds: UInt64? = nil) async throws -> PeekVisibility {
        try await bridge.request(.peek(session: session, surface: .row, event: event, elapsedMilliseconds: elapsedMilliseconds), as: PeekVisibility.self)
    }
}
