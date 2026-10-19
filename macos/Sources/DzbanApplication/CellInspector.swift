import AppKit
import Combine
import DzbanBridge
import Foundation

public struct OriginalCell: Sendable {
    public let text: String?
    public var plainText: String { text ?? "NULL" }
    public var encoded: Data {
        get throws { try JSONSerialization.data(withJSONObject: ["isNull": text == nil, "text": text.map { $0 as Any } ?? NSNull()], options: [.sortedKeys]) }
    }
    @MainActor public func write(to pasteboard: NSPasteboard) throws {
        try write(to: pasteboard, encoded: encoded)
    }
    @MainActor func write(to pasteboard: NSPasteboard, encoded: Data) {
        pasteboard.declareTypes([.string, NSPasteboard.PasteboardType("com.rubyelders.dzban.cell")], owner: nil)
        pasteboard.setString(plainText, forType: .string)
        pasteboard.setData(encoded, forType: NSPasteboard.PasteboardType("com.rubyelders.dzban.cell"))
    }
}

public enum FullCellReader {
    public static func read(source: any ResultSource, address: ResultAddress) async throws -> OriginalCell {
        try await source.originalCell(address)
    }

    static func validate(_ chunk: CellChunk, address: ResultAddress, offset: Int, count: Int) throws {
        guard chunk.document == address.scope.document, chunk.generation == address.scope.generation,
              chunk.row == address.row, chunk.column == address.column, chunk.offset == offset,
              chunk.total >= offset else { throw ViewportFailure("Cell belongs to retired output") }
        if let text = chunk.text {
            guard text.unicodeScalars.count == min(count, chunk.total - offset) else { throw ViewportFailure("Cell chunk is incomplete") }
        } else if chunk.total != 0 || offset != 0 { throw ViewportFailure("Invalid SQL null chunk") }
    }
}

@MainActor
public final class CellInspectorModel: ObservableObject {
    public static let chunkSize = 2_048
    @Published public private(set) var text: String?
    @Published public private(set) var total = 0
    @Published public private(set) var offset = 0
    @Published public private(set) var loaded = false
    @Published public private(set) var loading = false
    @Published public private(set) var copying = false
    @Published public private(set) var failure: String?
    public private(set) var address: ResultAddress?
    private let source: any ResultSource
    private var epoch: UInt64 = 0
    private var copyEpoch: UInt64 = 0
    private var loadTask: Task<Void, Never>?
    private var copyTask: Task<Void, Never>?

    public init(source: any ResultSource) { self.source = source }
    public func open(_ address: ResultAddress) {
        cancel()
        self.address = address
        load(offset: 0)
    }
    public func openEntire(_ address: ResultAddress) {
        cancel()
        self.address = address
        loading = true
        let epoch = epoch
        loadTask = Task { [weak self, source] in
            do {
                let value = try await FullCellReader.read(source: source, address: address)
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.text = value.text
                self.total = value.text?.unicodeScalars.count ?? 0
                self.loaded = true
                self.loading = false
            } catch {
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
                self.loading = false
            }
        }
    }
    public func load(offset: Int) {
        guard let address else { return }
        epoch &+= 1
        let epoch = epoch
        loadTask?.cancel()
        loading = true
        loaded = false
        failure = nil
        text = nil
        loadTask = Task { [weak self, source] in
            do {
                let chunk = try await source.cell(address, offset: offset, count: Self.chunkSize)
                try FullCellReader.validate(chunk, address: address, offset: offset, count: Self.chunkSize)
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.text = chunk.text
                self.total = chunk.total
                self.offset = offset
                self.loaded = true
                self.loading = false
            } catch {
                guard !Task.isCancelled, let self, self.epoch == epoch else { return }
                self.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
                self.loading = false
            }
        }
    }
    public func copy(_ address: ResultAddress, to pasteboard: NSPasteboard? = nil) {
        let pasteboard = pasteboard ?? copyPasteboard
        cancelCopy()
        copying = true
        failure = nil
        let copyEpoch = copyEpoch
        let source = source
        let reader = Task.detached {
            let value = try await FullCellReader.read(source: source, address: address)
            return (value, try value.encoded)
        }
        copyTask = Task { [weak self] in
            do {
                let (value, encoded) = try await withTaskCancellationHandler { try await reader.value } onCancel: { reader.cancel() }
                guard !Task.isCancelled, let self, self.copyEpoch == copyEpoch else { return }
                value.write(to: pasteboard, encoded: encoded)
                self.copying = false
            } catch {
                guard !Task.isCancelled, let self, self.copyEpoch == copyEpoch else { return }
                self.failure = (error as? BridgeFailure)?.message ?? error.localizedDescription
                self.copying = false
            }
        }
    }
    private var copyPasteboard: NSPasteboard {
        #if DZBAN_TEST_FIXTURES
        let arguments = ProcessInfo.processInfo.arguments
        if let index = arguments.firstIndex(of: "--test-pasteboard"), arguments.indices.contains(index + 1) {
            return NSPasteboard(name: NSPasteboard.Name(arguments[index + 1]))
        }
        #endif
        return .general
    }
    public func cancelCopy() { copyEpoch &+= 1; copyTask?.cancel(); copyTask = nil; copying = false }
    @discardableResult
    func presentationEnded() -> Task<Void, Never> {
        epoch &+= 1
        copyEpoch &+= 1
        loadTask?.cancel()
        copyTask?.cancel()
        let epoch = epoch
        return Task { @MainActor [weak self] in
            guard let self, self.epoch == epoch else { return }
            self.cancel()
        }
    }
    public func cancel() {
        epoch &+= 1
        loadTask?.cancel()
        loadTask = nil
        cancelCopy()
        address = nil
        loaded = false
        loading = false
        text = nil
        total = 0
        offset = 0
        failure = nil
    }
}
