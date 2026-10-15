import Foundation

public struct ConnectionHealth: Sendable {
    public var state = "unknown"
    public var latencyMicros: UInt64?
    public var version: String?
    public var failure: String?
    public var probing = false
    public var checkedAt: Date?
    public static func latency(_ micros: UInt64?) -> String {
        guard let micros else { return "--" }
        if micros < 1000 { return "\(micros)µs" }
        if micros < 1_000_000 { return "\(micros / 1000)ms" }
        return String(format: "%.2fs", Double(micros) / 1_000_000)
    }
}
