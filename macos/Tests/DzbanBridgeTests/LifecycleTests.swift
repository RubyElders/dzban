import XCTest
import DzbanBridgeScenarios

final class LifecycleTests: XCTestCase {
    func testConnectStreamCancelRestartAndShutdownThroughSwift() async throws {
        try await LifecycleScenarios.connectStreamCancelRestartAndShutdownThroughSwift()
    }
    func testCleanupFailureCanRetryAndRepeatedAppsReleaseTheirRuntime() async throws {
        try await LifecycleScenarios.cleanupFailureCanRetryAndRepeatedAppsReleaseTheirRuntime()
    }
}
