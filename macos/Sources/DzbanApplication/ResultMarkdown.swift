import AppKit
import DzbanBridge
import Foundation

enum ResultMarkdown {
    @MainActor static var pasteboard: NSPasteboard {
        #if DZBAN_TEST_FIXTURES
        let arguments = ProcessInfo.processInfo.arguments
        if let index = arguments.firstIndex(of: "--test-pasteboard"), arguments.indices.contains(index + 1) {
            return NSPasteboard(name: NSPasteboard.Name(arguments[index + 1]))
        }
        #endif
        return .general
    }

    static func escape(_ text: String) -> String {
        var value = text.replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
        for character in ["\\", "|", "`", "*", "_", "[", "]"] {
            value = value.replacingOccurrences(of: character, with: "\\" + character)
        }
        return value.replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
            .replacingOccurrences(of: "\n", with: "<br>")
    }

    static func line(_ cells: [String]) -> String { "| " + cells.joined(separator: " | ") + " |\n" }

    static func read(source: any ResultSource, scope: ResultScope, columns: [String], rows: IndexSet) async throws -> String {
        var output = line(columns.map(escape)) + line(columns.map { _ in "---" })
        for range in rows.rangeView {
            var start = range.lowerBound
            while start < range.upperBound {
                try Task.checkCancellation()
                let count = min(64, range.upperBound - start)
                var batchBytes = 0
                var batch = Array(repeating: [String](), count: count)
                for column in stride(from: 0, to: columns.count, by: 8) {
                    let width = min(8, columns.count - column)
                    let page = try await source.page(scope, start: start, count: count, columnStart: column, columnCount: width)
                    guard page.document == scope.document, page.generation == scope.generation,
                          page.start == start, page.columnStart == column, page.rows.count == count,
                          page.rows.allSatisfy({ $0.count == width }) else { throw ViewportFailure("Results changed during copy") }
                    for (offset, cells) in page.rows.enumerated() {
                        for (index, cell) in cells.enumerated() {
                            try Task.checkCancellation()
                            let text: String
                            if cell.truncated {
                                let address = ResultAddress(scope: scope, row: start + offset, column: column + index)
                                text = try await FullCellReader.read(source: source, address: address).plainText
                            } else { text = cell.text ?? "NULL" }
                            let escaped = escape(text)
                            batchBytes += escaped.utf8.count + 3
                            guard output.utf8.count + batchBytes <= 32 * 1024 * 1024 else { throw ViewportFailure("Results exceed the 32 MiB clipboard limit") }
                            batch[offset].append(escaped)
                        }
                    }
                }
                for row in batch {
                    let text = line(row)
                    guard output.utf8.count + text.utf8.count <= 32 * 1024 * 1024 else { throw ViewportFailure("Results exceed the 32 MiB clipboard limit") }
                    output += text
                }
                start += count
            }
        }
        return output
    }
}
