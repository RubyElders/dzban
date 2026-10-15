// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "DzbanNative",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "DzbanBridge", targets: ["DzbanBridge"]),
        .executable(name: "DzbanBridgeCheck", targets: ["DzbanBridgeCheck"]),
    ],
    targets: [
        .systemLibrary(name: "CDzban"),
        .target(name: "DzbanBridge", dependencies: ["CDzban"]),
        .target(name: "DzbanBridgeScenarios", dependencies: ["DzbanBridge"], path: "Checks/Scenarios"),
        .executableTarget(name: "DzbanBridgeCheck", dependencies: ["DzbanBridgeScenarios"], path: "Checks/Runner"),
        .testTarget(name: "DzbanBridgeTests", dependencies: ["DzbanBridgeScenarios"]),
    ]
)
