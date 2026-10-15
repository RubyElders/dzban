import DzbanBridge

public enum QueryDiagnostic {
    public static func describe(_ value: JSONValue) -> String {
        switch value {
        case .string(let value): return value == "Canceled" ? "Query canceled" : value
        case .object(let fields):
            if let database = fields["Database"] { return describe(database) }
            if let message = fields["Message"] { return describe(message) }
            if let unsupported = fields["Unsupported"] { return "Unsupported: \(describe(unsupported))" }
            return ["message", "code", "detail", "hint"].compactMap { key in
                guard let value = fields[key], value != .null else { return nil }
                if key == "code", case .object(let code) = value { return code.values.map(describe).joined(separator: ", ") }
                return describe(value)
            }.joined(separator: "\n")
        case .number(let value): return String(value)
        default: return "Query failed"
        }
    }
}
