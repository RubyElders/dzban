#if DZBAN_TEST_FIXTURES
import Combine
import Foundation

@MainActor
enum ViewUpdateDiagnostics {
    static var frames: [String] = []
    static var enabled = false
    static var publications: [String] = []
    private static var subscriptions: [AnyCancellable] = []
    static func reset() {
        enabled = false
        subscriptions.removeAll()
        publications.removeAll()
        frames.removeAll()
    }
    static func watch<Object: ObservableObject>(_ object: Object, name: String) {
        guard enabled else { return }
        subscriptions.append(object.objectWillChange.sink { _ in
            MainActor.assumeIsolated {
                guard enabled, !frames.isEmpty else { return }
                publications.append("\(name) during \(frames.joined(separator: "/"))\n" + Thread.callStackSymbols.joined(separator: "\n"))
            }
        })
    }
}
#endif
