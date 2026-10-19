import Foundation

public enum ExecutionPhase: String, Codable, Sendable {
    case idle, planning, running, canceling, finished
}

public enum CapturePhase: String, Codable, Sendable {
    case waitingForSchema = "waiting_for_schema"
    case receiving, draining
    case failedAwaitingCleanup = "failed_awaiting_cleanup"
    case finished
}

public enum CaptureTermination: String, Codable, Sendable {
    case complete, limited, canceled, failed
}

public enum PrimaryAction: String, Codable, Sendable {
    case unavailable, run, stop, stopping
}

public enum InputEffect: String, Codable, Sendable {
    case beginPreview = "begin_preview"
    case cancelPreview = "cancel_preview"
    case showPlan = "show_plan"
    case hidePlan = "hide_plan"
    case pinPlan = "pin_plan"
    case runQuery = "run_query"
    case cancelQuery = "cancel_query"
}

public enum InputHintKind: String, Codable, Sendable {
    case `default`
    case primaryGuide = "primary_guide"
    case estimating
    case estimateReady = "estimate_ready"
    case estimateError = "estimate_error"
    case planning, running, canceling
}

public enum DatabaseBackend: String, Codable, Sendable {
    case postgresql, clickhouse
}

public struct QueryEstimate: Codable, Sendable, Equatable {
    public var diagnostic: DiagnosticPresentation?
    public var rows: UInt64?
    public var startupCost: Double?
    public var totalCost: Double?
    public var parts: UInt64?
    public var marks: UInt64?

    public init(diagnostic: DiagnosticPresentation? = nil, rows: UInt64? = nil, startupCost: Double? = nil, totalCost: Double? = nil, parts: UInt64? = nil, marks: UInt64? = nil) {
        self.diagnostic = diagnostic
        self.rows = rows
        self.startupCost = startupCost
        self.totalCost = totalCost
        self.parts = parts
        self.marks = marks
    }
}

public struct PlanPresentation: Codable, Sendable, Equatable {
    public var backend: DatabaseBackend?
    public var document: JSONValue?
    public var estimate: QueryEstimate?
    public var text: String?

    public init(backend: DatabaseBackend? = nil, document: JSONValue? = nil, estimate: QueryEstimate? = nil, text: String? = nil) {
        self.backend = backend
        self.document = document
        self.estimate = estimate
        self.text = text
    }
}
