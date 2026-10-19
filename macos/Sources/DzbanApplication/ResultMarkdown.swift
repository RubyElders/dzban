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

    static func read(source: any ResultSource, scope: ResultScope, rows: IndexSet) async throws -> String {
        try await source.markdown(scope, rows: rows)
    }
}
