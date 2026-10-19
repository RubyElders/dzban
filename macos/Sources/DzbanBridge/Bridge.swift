import CDzban
import Foundation

public enum JSONValue: Decodable, Sendable, Equatable {
    case null, bool(Bool), number(Double), string(String), array([JSONValue]), object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer()
        if value.decodeNil() { self = .null }
        else if let item = try? value.decode(Bool.self) { self = .bool(item) }
        else if let item = try? value.decode(Double.self) { self = .number(item) }
        else if let item = try? value.decode(String.self) { self = .string(item) }
        else if let item = try? value.decode([JSONValue].self) { self = .array(item) }
        else { self = .object(try value.decode([String: JSONValue].self)) }
    }
}

public struct BridgeFailure: Error, Decodable, Sendable {
    public let kind: String
    public let message: String
    public let diagnostic: JSONValue?

    init(kind: String, message: String, diagnostic: JSONValue? = nil) {
        self.kind = kind
        self.message = message
        self.diagnostic = diagnostic
    }
}

public struct DocumentSnapshot: Decodable, Sendable {
    public let document: UUID
    public let session: UUID?
    public let query: UUID?
    public let generation: UInt64
    public let draftRevision: UInt64
    public let title: String
    public let draft: String
    public let execution: ExecutionPhase
    public let resultPhase: CapturePhase?
    public let resultAvailable: Bool
    public let rows: Int
    public let error: JSONValue?
    public let cutoff: JSONValue?
    public let receivedRows: Int?
    public let captureBytes: Int?
    public let termination: CaptureTermination?
    public let captureError: JSONValue?
    public let commandResult: CommandResult?
    public let startedAtSeconds: Double?
    public let elapsedSeconds: Double?
    public let executionPlan: PlanPresentation?
    public let executionSeconds: Double?
    public let feedback: ResultFeedback
    public let history: Bool?
    public let historyTotalRows: Int?
    public let historyPlanAvailable: Bool?
}

public struct CommandResult: Decodable, Sendable {
    public let affectedRows: UInt64?
}

public struct Connection: Decodable, Sendable {
    public let tab: UUID
    public let request: UUID
    public let session: UUID
    public let selected: Bool
}

public struct Controls: Decodable, Sendable {
    public let revision: UInt64
    public let session: UUID?
    public let query: UUID?
    public let primaryAction: PrimaryAction
}

public struct Connections: Decodable, Sendable {
    public struct Pending: Decodable, Sendable {
        public let request: UUID
        public let profile: String
        public let connecting: Bool
    }
    public let connections: [Pending]
}

public struct Profiles: Decodable, Sendable {
    public struct Profile: Decodable, Sendable, Identifiable {
        public let name: String
        public let backend: String
        public var id: String { name }
    }
    public let profiles: [Profile]
}

public struct DocumentClose: Decodable, Sendable {
    public let workspaceClosed: Bool
    public let document: UUID
    public let selected: DocumentSnapshot?
}

public enum EditorIntent: String, Encodable, Sendable {
    case submit, primary, cancel, reset
}

public struct EditorOutcome: Decodable, Sendable {
    public let submission: Submission?
    public let cancellationRequested: Bool
}

public struct Submission: Decodable, Sendable {
    public let query: UUID
    public let document: UUID
    public let session: UUID
    public let generation: UInt64
}

public struct Column: Codable, Sendable, Equatable {
    public let name: String
    public let typeName: String
}

#if DZBAN_TEST_FIXTURES
public struct BridgeByteMetrics: Decodable, Sendable {
    public let exportReaders: Int
    public let observers: Int
    public let recordBytes: Int
    public let peakRecordBytes: Int
    public let bufferBytes: Int
    public let peakBufferBytes: Int
}
#endif

public struct ResultSchema: Decodable, Sendable {
    public let document: UUID
    public let generation: UInt64
    public let columns: [Column]
}

public struct ViewportCell: Decodable, Sendable, Equatable {
    public let text: String?
    public let length: Int
    public let truncated: Bool

    public init(text: String?, length: Int, truncated: Bool) {
        self.text = text
        self.length = length
        self.truncated = truncated
    }
}

public struct ViewportPage: Decodable, Sendable {
    public let document: UUID
    public let generation: UInt64
    public let start: Int
    public let columnStart: Int
    public let rows: [[ViewportCell]]
}

public struct ResultPage: Decodable, Sendable {
    public struct Page: Decodable, Sendable {
        public let columns: [Column]
        public let rows: [[String?]]
    }
    public let document: UUID
    public let generation: UInt64
    public let start: Int
    public let page: Page
}

public struct CellChunk: Decodable, Sendable {
    public let document: UUID
    public let generation: UInt64
    public let row: Int
    public let column: Int
    public let offset: Int
    public let total: Int
    public let text: String?
}

public struct Preview: Decodable, Sendable {
    public let request: UUID
    public let session: UUID
    public let document: UUID
    public let draftRevision: UInt64
    public let plan: PlanPresentation
}

public struct History: Decodable, Sendable {
    public struct Entry: Decodable, Sendable { public let sql: String }
    public let entries: [Entry]
}

public struct ExportRowRange: Encodable, Sendable {
    public let start: Int
    public let end: Int
    public init(start: Int, end: Int) { self.start = start; self.end = end }
}

public struct ExportChunk: Decodable, Sendable {
    public let text: String
    public let done: Bool
    public let isNull: Bool
}

public struct BridgeCommand: Encodable, Sendable {
    let command: String
    var profile: String?
    var tab: UUID?
    var session: UUID?
    var document: UUID?
    var draft: String?
    var sql: String?
    var generation: UInt64?
    var start: Int?
    var count: Int?
    var row: Int?
    var column: Int?
    var offset: Int?
    var revision: UInt64?
    var end: Int?
    var intent: EditorIntent?
    var columnStart: Int?
    var columnCount: Int?
    var request: CatalogQuery?
    var surface: PeekSurface?
    var event: PeekEvent?
    var elapsedMs: UInt64?
    var requestRevision: UInt64?
    var historyRevision: UUID?
    var index: Int?
    var direction: Int?
    var includeVersion: Bool?
    var input: NativeInput?
    var export: UUID?
    var ranges: [ExportRowRange]?
    var location: WorkspaceLocation?
    var drafts: [WorkspaceDraft]?

    public static func restoreWorkspace(_ location: WorkspaceLocation) -> Self { Self(command: "restore_workspace", location: location) }
    public static func saveWorkspace(_ location: WorkspaceLocation, drafts: [WorkspaceDraft]) -> Self { Self(command: "save_workspace", location: location, drafts: drafts) }
    public static func connect(profile: String, tab: UUID? = nil, revision: UInt64? = nil) -> Self { Self(command: "connect", profile: profile, tab: tab, revision: revision) }
    public static func selectSession(_ session: UUID?, revision: UInt64? = nil) -> Self { Self(command: "select_session", session: session, revision: revision) }
    public static func closeDocument(session: UUID, document: UUID) -> Self { Self(command: "close_document", session: session, document: document) }
    public static func openDocument(session: UUID) -> Self { Self(command: "open_document", session: session) }
    public static func setDraft(session: UUID, document: UUID, draft: String) -> Self {
        Self(command: "set_draft", session: session, document: document, draft: draft)
    }
    public static func observeDocument(session: UUID, document: UUID) -> Self {
        Self(command: "observe_document", session: session, document: document)
    }
    public static func submit(session: UUID, document: UUID, sql: String) -> Self {
        Self(command: "submit", session: session, document: document, sql: sql)
    }
    public static func cancel(session: UUID, document: UUID) -> Self {
        Self(command: "cancel", session: session, document: document)
    }
    public static func resultSchema(session: UUID, document: UUID, generation: UInt64) -> Self {
        Self(command: "result_schema", session: session, document: document, generation: generation)
    }
    public static func readViewport(session: UUID, document: UUID, generation: UInt64, start: Int, count: Int, columnStart: Int, columnCount: Int) -> Self {
        Self(command: "read_viewport", session: session, document: document, generation: generation, start: start, count: count, columnStart: columnStart, columnCount: columnCount)
    }
    public static func readPage(session: UUID, document: UUID, generation: UInt64, start: Int, count: Int) -> Self {
        Self(command: "read_page", session: session, document: document, generation: generation, start: start, count: count)
    }
    public static func closeSession(session: UUID) -> Self { Self(command: "close_session", session: session) }
    public static func preview(session: UUID, document: UUID, sql: String, revision: UInt64, start: Int? = nil, end: Int? = nil, requestRevision: UInt64? = nil) -> Self {
        Self(command: "preview", session: session, document: document, sql: sql, start: start, revision: revision, end: end, requestRevision: requestRevision)
    }
    public static func cancelPreview(session: UUID, requestRevision: UInt64? = nil) -> Self { Self(command: "cancel_preview", session: session, requestRevision: requestRevision) }
    public static func history(session: UUID) -> Self { Self(command: "history", session: session) }
    public static func startExport(_ export: UUID, session: UUID, document: UUID, generation: UInt64, row: Int? = nil, column: Int? = nil, ranges: [ExportRowRange]? = nil) -> Self {
        Self(command: "start_export", session: session, document: document, generation: generation, row: row, column: column, export: export, ranges: ranges)
    }
    public static func readExport(_ export: UUID) -> Self { Self(command: "read_export", export: export) }
    public static func closeExport(_ export: UUID) -> Self { Self(command: "close_export", export: export) }
    public static func readCell(session: UUID, document: UUID, generation: UInt64, row: Int, column: Int, offset: Int, count: Int) -> Self {
        Self(command: "read_cell", session: session, document: document, generation: generation, count: count, row: row, column: column, offset: offset)
    }
    public static func editorAction(session: UUID, document: UUID, draft: String, start: Int, end: Int, intent: EditorIntent) -> Self {
        Self(command: "editor_action", session: session, document: document, draft: draft, start: start, end: end, intent: intent)
    }
    #if DZBAN_TEST_FIXTURES
    public static let testMetrics = Self(command: "test_metrics")
    #endif
    public static let profiles = Self(command: "profiles")
    public static let controls = Self(command: "controls")
    public static let observeAttention = Self(command: "observe_attention")
    public static let connections = Self(command: "connections")
    public static let shutdown = Self(command: "shutdown")
}

struct Envelope<T: Decodable>: Decodable {
    let schema: Int
    let revision: UInt64
    let payload: T
}

private struct Header: Decodable { let schema: Int }
private struct FailurePayload: Decodable { let failure: BridgeFailure? }

func decodeRecord<T: Decodable>(_ data: Data, as type: T.Type) throws -> Envelope<T> {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    let header = try decoder.decode(Header.self, from: data)
    guard header.schema == 1 else { throw BridgeFailure(kind: "schema", message: "Unsupported bridge schema") }
    let probe = try decoder.decode(Envelope<FailurePayload>.self, from: data)
    if let error = probe.payload.failure { throw error }
    return try decoder.decode(Envelope<T>.self, from: data)
}

private final class Observation: @unchecked Sendable {
    let handle: UInt64
    private let lock = NSLock()
    private var released = false

    init(_ handle: UInt64) { self.handle = handle }
    func release() {
        lock.lock()
        defer { lock.unlock() }
        if !released {
            released = true
            _ = dzban_release(handle)
        }
    }
    var isReleased: Bool {
        lock.lock()
        defer { lock.unlock() }
        return released
    }
    deinit { release() }
}

private func wait(_ handle: UInt64, after revision: UInt64) throws -> Data? {
    let buffer = dzban_wait(handle, revision, 1000)
    switch buffer.status {
    case 0:
        defer { _ = dzban_buffer_release(buffer.token) }
        guard let pointer = buffer.data else { throw BridgeFailure(kind: "buffer", message: "Missing bridge buffer") }
        return Data(bytes: pointer, count: buffer.length)
    case 1: return nil
    case 2: throw CancellationError()
    default: throw BridgeFailure(kind: "transport", message: "Bridge wait failed with status \(buffer.status)")
    }
}

public final class Bridge: Sendable {
    private let handle: UInt64

    public convenience init() throws { try self.init(handle: dzban_create()) }

    #if DZBAN_TEST_FIXTURES
    public static func fixture() throws -> Bridge { try Bridge(handle: dzban_create_fixture()) }
    #endif

    init(handle: UInt64) throws {
        guard dzban_abi_version() == 1, handle != 0 else {
            throw BridgeFailure(kind: "initialization", message: "Cannot initialize native bridge")
        }
        self.handle = handle
    }

    private func send(_ command: BridgeCommand) throws -> UInt64 {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let data = try encoder.encode(command)
        let operation = data.withUnsafeBytes { bytes in
            dzban_send(handle, bytes.bindMemory(to: UInt8.self).baseAddress, data.count)
        }
        guard operation != 0 else { throw BridgeFailure(kind: "admission", message: "Cannot admit bridge command") }
        return operation
    }

    public func request<T: Decodable & Sendable>(_ command: BridgeCommand, as type: T.Type) async throws -> T {
        let operation = try send(command)
        let observation = Observation(operation)
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                DispatchQueue.global(qos: .userInitiated).async {
                    defer { observation.release() }
                    do {
                        while !observation.isReleased {
                            if let data = try wait(operation, after: 0) {
                                guard !observation.isReleased else { throw CancellationError() }
                                continuation.resume(returning: try decodeRecord(data, as: type).payload)
                                return
                            }
                        }
                        throw CancellationError()
                    } catch { continuation.resume(throwing: error) }
                }
            }
        } onCancel: { observation.release() }
    }

    public func restoreWorkspace(_ location: WorkspaceLocation) async throws -> AsyncThrowingStream<WorkspaceRestoration, Error> {
        try observe(.restoreWorkspace(location), as: WorkspaceRestoration.self)
    }
    public func observeDocument(session: UUID, document: UUID) async throws -> AsyncThrowingStream<DocumentSnapshot, Error> {
        try observe(.observeDocument(session: session, document: document), as: DocumentSnapshot.self)
    }
    public func observeControls(session: UUID) async throws -> AsyncThrowingStream<Controls, Error> {
        try observe(.observeControls(session: session), as: Controls.self)
    }
    public func attention(_ input: AttentionInput) throws -> AttentionSnapshot {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let data = try encoder.encode(input)
        let buffer = data.withUnsafeBytes { dzban_attention(handle, $0.bindMemory(to: UInt8.self).baseAddress, data.count) }
        guard buffer.status == 0, let pointer = buffer.data else { throw BridgeFailure(kind: "attention", message: "Cannot report application visibility") }
        defer { _ = dzban_buffer_release(buffer.token) }
        return try decodeRecord(Data(bytes: pointer, count: buffer.length), as: AttentionSnapshot.self).payload
    }
    public func observeAttention() async throws -> AsyncThrowingStream<AttentionSnapshot, Error> {
        try observe(.observeAttention, as: AttentionSnapshot.self)
    }
    public func observeHealth() async throws -> AsyncThrowingStream<HealthSnapshot, Error> {
        try observe(.observeHealth, as: HealthSnapshot.self)
    }
    public func observeInteraction(session: UUID) async throws -> AsyncThrowingStream<InteractionProjection, Error> {
        try observe(.observeInteraction(session: session), as: InteractionProjection.self)
    }
    private func observe<T: Decodable & Sendable>(_ command: BridgeCommand, as type: T.Type) throws -> AsyncThrowingStream<T, Error> {
        let stream = try send(command)
        let observation = Observation(stream)
        return AsyncThrowingStream(bufferingPolicy: .bufferingNewest(1)) { continuation in
            continuation.onTermination = { _ in observation.release() }
            DispatchQueue(label: "dzban.observer.\(stream)").async {
                defer { observation.release() }
                var revision: UInt64 = 0
                do {
                    while !observation.isReleased {
                        if let data = try wait(stream, after: revision) {
                            guard !observation.isReleased else { break }
                            let envelope = try decodeRecord(data, as: type)
                            revision = envelope.revision
                            continuation.yield(envelope.payload)
                        }
                    }
                    continuation.finish()
                } catch is CancellationError { continuation.finish() }
                catch { continuation.finish(throwing: error) }
            }
        }
    }

    @discardableResult public func shutdown() async throws -> AttentionSnapshot {
        struct Shutdown: Decodable, Sendable { let shutdown: Bool; let attention: AttentionSnapshot }
        let result = try await request(.shutdown, as: Shutdown.self)
        guard result.shutdown, dzban_release(handle) else {
            throw BridgeFailure(kind: "shutdown", message: "Application cleanup did not finish")
        }
        return result.attention
    }
}
