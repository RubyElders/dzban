#if DZBAN_TEST_FIXTURES
import Darwin
import DzbanBridge
import Foundation

@MainActor
final class MainThreadProbe {
    private var timer: Timer?
    private var previous: TimeInterval = 0
    private(set) var samples = 0
    private(set) var maximumGapMilliseconds: Double = 0
    private(set) var gapsOver50Milliseconds = 0

    func start() {
        stop()
        samples = 0
        maximumGapMilliseconds = 0
        gapsOver50Milliseconds = 0
        previous = ProcessInfo.processInfo.systemUptime
        let timer = Timer(timeInterval: 0.016, repeats: true) { [weak self] _ in
            let now = ProcessInfo.processInfo.systemUptime
            Task { @MainActor [weak self] in self?.record(now) }
        }
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }
    private func record(_ now: TimeInterval) {
        guard timer != nil, now >= previous else { return }
        let gap = (now - previous) * 1000
        previous = now
        samples += 1
        maximumGapMilliseconds = max(maximumGapMilliseconds, gap)
        if gap > 50 { gapsOver50Milliseconds += 1 }
    }
    func stop() { timer?.invalidate(); timer = nil }
}

private func residentMemory() -> (current: UInt64, peak: Int) {
    var info = mach_task_basic_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info_data_t>.size / MemoryLayout<integer_t>.size)
    let result = withUnsafeMutablePointer(to: &info) { pointer in
        pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
        }
    }
    var usage = rusage()
    getrusage(RUSAGE_SELF, &usage)
    return (result == KERN_SUCCESS ? info.resident_size : 0, usage.ru_maxrss)
}

func writeViewportLayout(_ report: [String: Any]) {
    let arguments = ProcessInfo.processInfo.arguments
    guard let index = arguments.firstIndex(of: "--metrics-output"), arguments.indices.contains(index + 1),
          let data = try? JSONSerialization.data(withJSONObject: report, options: [.sortedKeys]) else { return }
    try? data.write(to: URL(fileURLWithPath: arguments[index + 1] + ".layout"), options: .atomic)
}

extension ApplicationStore {
    public func resetResultMetrics() { responsiveness.start(); results.resetMetrics() }
    public func recordResultMetrics() async {
        let arguments = ProcessInfo.processInfo.arguments
        guard let index = arguments.firstIndex(of: "--metrics-output"), arguments.indices.contains(index + 1) else { return }
        do {
            let bridgeBytes = try await bridge.request(.testMetrics, as: BridgeByteMetrics.self)
            let memory = residentMemory()
            responsiveness.stop()
            let report: [String: Any] = [
                "cacheBytes": results.cacheBytes, "peakCacheBytes": results.peakCacheBytes,
                "cachePages": results.pageCount, "peakPageSchemaReads": results.peakInFlight,
                "pageSchemaRequests": results.requestCount,
                "bridgeRecordBytes": bridgeBytes.recordBytes, "bridgePeakRecordBytes": bridgeBytes.peakRecordBytes,
                "bridgeBufferBytes": bridgeBytes.bufferBytes, "bridgePeakBufferBytes": bridgeBytes.peakBufferBytes,
                "residentBytes": memory.current, "peakResidentBytes": memory.peak,
                "mainThreadSamples": responsiveness.samples, "maximumMainThreadGapMs": responsiveness.maximumGapMilliseconds,
                "mainThreadGapsOver50Ms": responsiveness.gapsOver50Milliseconds,
            ]
            let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: URL(fileURLWithPath: arguments[index + 1]), options: .atomic)
        } catch { failure = error.localizedDescription }
    }
}

public func fixtureResultSource(_ bridge: Bridge) -> any ResultSource {
    let source = BridgeResultSource(bridge: bridge)
    return ProcessInfo.processInfo.arguments.contains("--fail-first-page") ? FailingPageSource(source: source) : source
}

private actor FailingPageSource: ResultSource {
    let source: BridgeResultSource
    private var failNext = true
    init(source: BridgeResultSource) { self.source = source }
    func schema(_ scope: ResultScope) async throws -> ResultSchema { try await source.schema(scope) }
    func page(_ scope: ResultScope, start: Int, count: Int, columnStart: Int, columnCount: Int) async throws -> ViewportPage {
        if failNext { failNext = false; throw ViewportFailure("Fixture page read failed") }
        return try await source.page(scope, start: start, count: count, columnStart: columnStart, columnCount: columnCount)
    }
    func cell(_ address: ResultAddress, offset: Int, count: Int) async throws -> CellChunk {
        try await source.cell(address, offset: offset, count: count)
    }
}
#endif
