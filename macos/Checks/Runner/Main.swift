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
            FileHandle.standardError.write(Data("Checking connectStreamCancelRestartAndShutdownThroughSwift\n".utf8))
            try await LifecycleScenarios.connectStreamCancelRestartAndShutdownThroughSwift()
            FileHandle.standardError.write(Data("Checking cleanupFailureCanRetryAndRepeatedAppsReleaseTheirRuntime\n".utf8))
            try await LifecycleScenarios.cleanupFailureCanRetryAndRepeatedAppsReleaseTheirRuntime()
            FileHandle.standardError.write(Data("Checking nativeStoreNavigatesClosesAndRejectsStaleConnections\n".utf8))
            try await LifecycleScenarios.nativeStoreNavigatesClosesAndRejectsStaleConnections()
            FileHandle.standardError.write(Data("Checking nativeStoreKeepsBusyAndFailedCleanupAvailableForRetry\n".utf8))
            try await LifecycleScenarios.nativeStoreKeepsBusyAndFailedCleanupAvailableForRetry()
            FileHandle.standardError.write(Data("Checking nativeEditorSelectsSQLPreservesDraftsAndScopesCancellation\n".utf8))
            let editorRepetitions = min(20, max(1, Int(ProcessInfo.processInfo.environment["DZBAN_EDITOR_REPETITIONS"] ?? "1") ?? 1))
            for iteration in 1...editorRepetitions {
                if editorRepetitions > 1 { FileHandle.standardError.write(Data("Editor repetition \(iteration)/\(editorRepetitions)\n".utf8)) }
                try await LifecycleScenarios.nativeEditorSelectsSQLPreservesDraftsAndScopesCancellation()
            }
            FileHandle.standardError.write(Data("Checking nativeResultsStreamPageInspectAndRetireAcrossOutputs\n".utf8))
            try await LifecycleScenarios.nativeResultsStreamPageInspectAndRetireAcrossOutputs()
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
