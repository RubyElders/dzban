import AppKit
import Combine
import DzbanBridge
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

public protocol ResultSource: Sendable {
    func schema(_ scope: ResultScope) async throws -> ResultSchema
    func page(_ scope: ResultScope, start: Int, count: Int, columnStart: Int, columnCount: Int) async throws -> ViewportPage
    func cell(_ address: ResultAddress, offset: Int, count: Int) async throws -> CellChunk
}

public struct BridgeResultSource: ResultSource {
    let bridge: Bridge
    public init(bridge: Bridge) { self.bridge = bridge }
    public func schema(_ scope: ResultScope) async throws -> ResultSchema {
        try await bridge.request(.resultSchema(session: scope.session, document: scope.document, generation: scope.generation), as: ResultSchema.self)
    }
    public func page(_ scope: ResultScope, start: Int, count: Int, columnStart: Int, columnCount: Int) async throws -> ViewportPage {
        try await bridge.request(.readViewport(session: scope.session, document: scope.document, generation: scope.generation, start: start, count: count, columnStart: columnStart, columnCount: columnCount), as: ViewportPage.self)
    }
    public func cell(_ address: ResultAddress, offset: Int, count: Int) async throws -> CellChunk {
        let scope = address.scope
        return try await bridge.request(.readCell(session: scope.session, document: scope.document, generation: scope.generation, row: address.row, column: address.column, offset: offset, count: count), as: CellChunk.self)
    }
}

struct ResultTileKey: Hashable {
    let row: Int
    let column: Int
}

struct ResultTileCache {
    struct Tile {
        var rows: [Int: [ViewportCell]] = [:]
        var bytes: Int {
            128 + rows.values.reduce(0) { $0 + 64 + $1.reduce(0) { $0 + 64 + ($1.text?.utf8.count ?? 0) } }
        }
    }
    let maximumPages: Int
    var maximumBytes: Int
    private(set) var tiles: [ResultTileKey: Tile] = [:]
    private var order: [ResultTileKey] = []
    var bytes: Int { tiles.values.reduce(0) { $0 + $1.bytes } }

    mutating func clear() { tiles.removeAll(); order.removeAll() }
    func hasRow(_ row: Int, in key: ResultTileKey) -> Bool { tiles[key]?.rows[row] != nil }
    mutating func cell(row: Int, column: Int, key: ResultTileKey) -> ViewportCell? {
        guard let cells = tiles[key]?.rows[row], cells.indices.contains(column - key.column) else { return nil }
        touch(key)
        return cells[column - key.column]
    }
    mutating func insert(_ page: ViewportPage, key: ResultTileKey) -> Bool {
        var tile = tiles[key] ?? Tile()
        for (offset, row) in page.rows.enumerated() { tile.rows[page.start + offset] = row }
        guard tile.bytes <= maximumBytes else { return false }
        tiles[key] = tile
        touch(key)
        while tiles.count > maximumPages || bytes > maximumBytes {
            guard let oldest = order.first else { break }
            order.removeFirst()
            tiles.removeValue(forKey: oldest)
        }
        return true
    }
    mutating func trim(rows: Int) {
        for key in Array(tiles.keys) {
            let retained = tiles[key]!.rows.filter { $0.key < rows }
            tiles[key]?.rows = retained
            if tiles[key]?.rows.isEmpty == true { tiles.removeValue(forKey: key); order.removeAll { $0 == key } }
        }
    }
    private mutating func touch(_ key: ResultTileKey) { order.removeAll { $0 == key }; order.append(key) }
}

@MainActor
public final class ResultViewport: ObservableObject {
    public static let pageRows = 64
    public static let pageColumns = 8
    var tablePositions: [UUID: ResultTablePosition] = [:]
    var isFinished: Bool { terminal }
    weak var focusCycle: WorkspaceFocusCycle?
    var isFocusable: Bool { availableRows > 0 && !columns.isEmpty }
    @Published private(set) var focusRequest: UInt64 = 0
    @Published public private(set) var revision: UInt64 = 0
    @Published public private(set) var columns: [Column] = []
    @Published public private(set) var availableRows = 0
    @Published public private(set) var loadError: String?
    @Published private(set) var copyError: String?
    private var copyTask: Task<Void, Never>?
    @Published public var selected: ResultAddress?
    public private(set) var scope: ResultScope?
    public private(set) var peakCacheBytes = 0
    public private(set) var peakInFlight = 0
    public private(set) var requestCount = 0
    public var cacheBytes: Int { schemaBytes + cache.bytes }
    public var pageCount: Int { cache.tiles.count }
    public var inFlightCount: Int { active.count }
    public var isLoading: Bool { active.values.contains { $0.epoch == epoch } }
    let source: any ResultSource
    private let maximumBytes: Int
    private var cache: ResultTileCache
    private var schemaBytes = 0
    private var epoch: UInt64 = 0
    private var canRead = false
    private var terminal = false
    private var schemaReady = false
    private var schemaFailed = false
    private var visibleRows = 0..<1
    private var visibleColumns = 0..<ResultViewport.pageColumns
    private var failed: Set<ResultTileKey> = []
    private var emptyAt: [ResultTileKey: Int] = [:]
    private enum Work: Hashable {
        case schema
        case page(ResultTileKey, start: Int, count: Int, columns: Int)
    }
    private struct Active {
        let epoch: UInt64
        let work: Work
        let task: Task<Void, Never>
    }
    private var active: [UUID: Active] = [:]

    public init(source: any ResultSource, maximumPages: Int = 8, maximumBytes: Int = 8 * 1024 * 1024) {
        self.source = source
        self.maximumBytes = max(1, maximumBytes)
        cache = ResultTileCache(maximumPages: max(1, maximumPages), maximumBytes: max(1, maximumBytes))
    }

    public func update(session: UUID, document: DocumentSnapshot) {
        let next = ResultScope(session: session, document: document.document, generation: document.generation)
        if scope != next {
            copyTask?.cancel()
            copyError = nil
            epoch &+= 1
            for item in active.values { item.task.cancel() }
            scope = next
            columns = []
            schemaReady = false
            schemaFailed = false
            schemaBytes = 0
            cache.clear()
            cache.maximumBytes = maximumBytes
            failed.removeAll()
            emptyAt.removeAll()
            selected = nil
            loadError = nil
            visibleRows = 0..<Self.pageRows
            visibleColumns = 0..<Self.pageColumns
        }
        if availableRows != document.rows {
            availableRows = max(0, document.rows)
            cache.trim(rows: availableRows)
            if let selected, selected.row >= availableRows {
                self.selected = availableRows == 0 ? nil : ResultAddress(scope: next, row: availableRows - 1, column: selected.column)
            }
        }
        terminal = document.execution == "finished"
        canRead = document.generation > 0 && document.resultAvailable
        restoreTableSelection()
        revision &+= 1
        pump()
    }

    private func restoreTableSelection() {
        guard selected == nil, let scope, let position = tablePositions[scope.document],
              !columns.isEmpty, position.headers == columns, availableRows > 0,
              let row = position.row, row < availableRows || terminal else { return }
        selected = ResultAddress(scope: scope, row: min(row, availableRows - 1), column: min(position.column, columns.count - 1))
    }

    public func retire() {
        copyTask?.cancel()
        copyError = nil
        epoch &+= 1
        scope = nil
        for item in active.values { item.task.cancel() }
        cache.clear()
        columns = []
        schemaBytes = 0
        availableRows = 0
        selected = nil
        canRead = false
        revision &+= 1
    }

    func copyRows(_ rows: IndexSet, to pasteboard: NSPasteboard? = nil) {
        let pasteboard = pasteboard ?? ResultMarkdown.pasteboard
        guard let scope, !columns.isEmpty, !rows.isEmpty else { return }
        copyTask?.cancel()
        copyError = nil
        let columns = columns
        let rows = rows.intersection(IndexSet(integersIn: 0..<availableRows))
        copyTask = Task { [weak self, source] in
            do {
                let markdown = try await ResultMarkdown.read(source: source, scope: scope, columns: columns.map(\.name), rows: rows)
                try Task.checkCancellation()
                guard self?.scope == scope else { return }
                pasteboard.declareTypes([.string, NSPasteboard.PasteboardType("net.daringfireball.markdown")], owner: nil)
                pasteboard.setString(markdown, forType: .string)
                pasteboard.setString(markdown, forType: NSPasteboard.PasteboardType("net.daringfireball.markdown"))
            } catch is CancellationError {
            } catch {
                guard !Task.isCancelled, self?.scope == scope else { return }
                self?.copyError = error.localizedDescription
            }
        }
    }

    public func show(rows: Range<Int>, columns: Range<Int>) {
        guard !rows.isEmpty, !columns.isEmpty else { return }
        visibleRows = max(0, rows.lowerBound)..<max(0, rows.upperBound)
        visibleColumns = max(0, columns.lowerBound)..<max(0, columns.upperBound)
        pump()
    }
    public func value(row: Int, column: Int) -> ViewportCell? {
        guard row >= 0, row < availableRows, columns.indices.contains(column) else { return nil }
        let key = ResultTileKey(row: row / Self.pageRows * Self.pageRows, column: column / Self.pageColumns * Self.pageColumns)
        return cache.cell(row: row, column: column, key: key)
    }
    func requestFocus() { focusRequest &+= 1 }

    public func select(row: Int, column: Int) {
        guard let scope, row >= 0, row < availableRows, columns.indices.contains(column) else {
            if selected != nil { selected = nil }
            return
        }
        let next = ResultAddress(scope: scope, row: row, column: column)
        if selected != next { selected = next }
    }
    #if DZBAN_TEST_FIXTURES
    public func resetMetrics() {
        peakCacheBytes = cacheBytes
        peakInFlight = active.count
        requestCount = 0
    }
    #endif

    public func retry() {
        loadError = nil
        schemaFailed = false
        failed.removeAll()
        emptyAt.removeAll()
        pump()
    }

    private func wanted() -> [ResultTileKey] {
        guard availableRows > 0, !columns.isEmpty else { return [] }
        let firstRow = min(visibleRows.lowerBound, availableRows - 1) / Self.pageRows * Self.pageRows
        let lastRow = min(max(visibleRows.upperBound - 1, firstRow), availableRows - 1)
        let firstColumn = min(visibleColumns.lowerBound, columns.count - 1) / Self.pageColumns * Self.pageColumns
        let lastColumn = min(max(visibleColumns.upperBound - 1, firstColumn), columns.count - 1)
        let worstTileBytes = Self.pageRows * Self.pageColumns * (256 * 4 + 64) + Self.pageRows * 64 + 128
        let count = min(cache.maximumPages, max(1, cache.maximumBytes / worstTileBytes))
        var keys: [ResultTileKey] = []
        for row in stride(from: firstRow, through: lastRow, by: Self.pageRows) {
            for column in stride(from: firstColumn, through: lastColumn, by: Self.pageColumns) {
                keys.append(ResultTileKey(row: row, column: column))
                if keys.count == count { return keys }
            }
        }
        return keys
    }

    private func pump() {
        guard let scope, canRead else { return }
        while active.count < 2 {
            if !schemaReady {
                guard !schemaFailed, !active.values.contains(where: { $0.epoch == epoch && $0.work == .schema }) else { return }
                launch(.schema, scope: scope)
                return
            }
            var next: Work?
            for key in wanted() {
                if failed.contains(key) || emptyAt[key] == availableRows { continue }
                if active.values.contains(where: { item in
                    guard item.epoch == epoch, case .page(let pending, _, _, _) = item.work else { return false }
                    return pending == key
                }) { continue }
                let end = min(key.row + Self.pageRows, availableRows)
                if let start = (key.row..<end).first(where: { !cache.hasRow($0, in: key) }) {
                    let missingEnd = (start..<end).first(where: { cache.hasRow($0, in: key) }) ?? end
                    next = .page(key, start: start, count: missingEnd - start, columns: min(Self.pageColumns, columns.count - key.column))
                    break
                }
            }
            guard let next else { return }
            launch(next, scope: scope)
        }
    }

    private func launch(_ work: Work, scope: ResultScope) {
        let id = UUID()
        let epoch = epoch
        let source = source
        requestCount += 1
        let task = Task { [weak self] in
            defer { self?.finished(id) }
            do {
                switch work {
                case .schema:
                    let schema = try await source.schema(scope)
                    guard !Task.isCancelled, let self, self.epoch == epoch, self.scope == scope else { return }
                    guard schema.document == scope.document, schema.generation == scope.generation else { throw ViewportFailure("Schema belongs to retired output") }
                    let bytes = 256 + schema.columns.reduce(0) { $0 + 96 + $1.name.utf8.count + $1.typeName.utf8.count }
                    guard bytes < self.maximumBytes else { throw ViewportFailure("The result schema is too large to display") }
                    self.columns = schema.columns
                    self.schemaBytes = bytes
                    self.cache.maximumBytes = self.maximumBytes - bytes
                    self.schemaReady = true
                    self.restoreTableSelection()
                case .page(let key, let start, let count, let width):
                    let page = try await source.page(scope, start: start, count: count, columnStart: key.column, columnCount: width)
                    guard !Task.isCancelled, let self, self.epoch == epoch, self.scope == scope else { return }
                    guard page.document == scope.document, page.generation == scope.generation, page.start == start, page.columnStart == key.column,
                          page.rows.count <= count, page.rows.allSatisfy({ $0.count == width }) else { throw ViewportFailure("Result page does not match the requested viewport") }
                    if page.rows.isEmpty { self.emptyAt[key] = self.availableRows }
                    else if !self.cache.insert(page, key: key) { throw ViewportFailure("The result preview is too large to display") }
                }
                self?.peakCacheBytes = max(self?.peakCacheBytes ?? 0, self?.cacheBytes ?? 0)
            } catch {
                guard !Task.isCancelled, let self, self.epoch == epoch, self.scope == scope else { return }
                switch work {
                case .schema:
                    if (error as? BridgeFailure)?.kind == "result_unavailable", self.terminal, self.availableRows == 0 { self.schemaReady = true }
                    else { self.schemaFailed = true; self.loadError = (error as? BridgeFailure)?.message ?? error.localizedDescription }
                case .page(let key, _, _, _): self.failed.insert(key); self.loadError = (error as? BridgeFailure)?.message ?? error.localizedDescription
                }
            }
        }
        active[id] = Active(epoch: epoch, work: work, task: task)
        peakInFlight = max(peakInFlight, active.count)
        revision &+= 1
    }

    private func finished(_ id: UUID) {
        active.removeValue(forKey: id)
        revision &+= 1
        pump()
    }
}

struct ViewportFailure: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
