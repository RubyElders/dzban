import XCTest
import DzbanBridgeScenarios

@MainActor
final class LifecycleTests: XCTestCase {
    func testConnectStreamCancelRestartAndShutdownThroughSwift() async throws {
        try await LifecycleScenarios.connectStreamCancelRestartAndShutdownThroughSwift()
    }
    func testCleanupFailureCanRetryAndRepeatedAppsReleaseTheirRuntime() async throws {
        try await LifecycleScenarios.cleanupFailureCanRetryAndRepeatedAppsReleaseTheirRuntime()
    }
    func testNativeStoreNavigatesClosesAndRejectsStaleConnections() async throws {
        try await LifecycleScenarios.nativeStoreNavigatesClosesAndRejectsStaleConnections()
    }
    func testNativeStoreKeepsBusyAndFailedCleanupAvailableForRetry() async throws {
        try await LifecycleScenarios.nativeStoreKeepsBusyAndFailedCleanupAvailableForRetry()
    }
}
