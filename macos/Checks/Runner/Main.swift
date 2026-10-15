import DzbanBridgeScenarios
import Foundation

@main
enum BridgeCheck {
    static func main() async {
        do {
            #if DZBAN_TEST_FIXTURES
            let watchdog = Task {
                try await Task.sleep(for: .seconds(60))
                FileHandle.standardError.write(Data("Bridge scenarios exceeded 60 seconds\n".utf8))
                exit(1)
            }
            defer { watchdog.cancel() }
            try await LifecycleScenarios.connectStreamCancelRestartAndShutdownThroughSwift()
            try await LifecycleScenarios.cleanupFailureCanRetryAndRepeatedAppsReleaseTheirRuntime()
            try await LifecycleScenarios.nativeStoreNavigatesClosesAndRejectsStaleConnections()
            try await LifecycleScenarios.nativeStoreKeepsBusyAndFailedCleanupAvailableForRetry()
            try await LifecycleScenarios.nativeEditorSelectsSQLPreservesDraftsAndScopesCancellation()
            print("Swift bridge lifecycle scenarios passed")
            #else
            throw NSError(domain: "DzbanBridgeCheck", code: 1, userInfo: [NSLocalizedDescriptionKey: "Build the check runner with test fixtures enabled"])
            #endif
        } catch {
            FileHandle.standardError.write(Data("Bridge scenarios failed: \(error)\n".utf8))
            exit(1)
        }
    }
}
