import Foundation

enum SQLSyntax {
    enum Kind: Equatable { case keyword, string, number, comment, identifier, function }
    struct Token: Equatable {
        let range: NSRange
        let kind: Kind
    }

    private static let keywords = Set("""
        SELECT FROM WHERE AS AND OR NOT NULL TRUE FALSE IS IN BETWEEN LIKE ILIKE
        JOIN INNER LEFT RIGHT FULL OUTER CROSS ON USING UNION ALL DISTINCT EXCEPT
        INTERSECT WITH RECURSIVE GROUP BY HAVING ORDER ASC DESC NULLS FIRST LAST
        LIMIT OFFSET FETCH NEXT ROW ROWS ONLY INSERT INTO VALUES UPDATE SET DELETE
        RETURNING CREATE ALTER DROP TABLE VIEW MATERIALIZED INDEX DATABASE SCHEMA
        IF EXISTS PRIMARY KEY FOREIGN REFERENCES UNIQUE CHECK DEFAULT CONSTRAINT
        CASE WHEN THEN ELSE END CAST OVER PARTITION WINDOW RANGE UNBOUNDED PRECEDING
        FOLLOWING CURRENT FILTER WITHIN LATERAL EXPLAIN ANALYZE VERBOSE FORMAT
        BEGIN COMMIT ROLLBACK TRANSACTION SAVEPOINT RELEASE TRUNCATE GRANT REVOKE
        SHOW DESCRIBE EXPLAIN USE SETTINGS PREWHERE ARRAY FINAL SAMPLE ENGINE
        INT INTEGER BIGINT SMALLINT FLOAT DOUBLE DECIMAL NUMERIC BOOLEAN BOOL
        CHAR VARCHAR TEXT DATE TIME TIMESTAMP INTERVAL UUID JSON JSONB
        """.split(whereSeparator: { $0.isWhitespace }).map(String.init))

    static func tokens(in text: String) -> [Token] {
        let units = Array(text.utf16)
        var tokens: [Token] = []
        var index = 0
        func digit(_ unit: UInt16) -> Bool { (48...57).contains(unit) }
        func letter(_ unit: UInt16) -> Bool { (65...90).contains(unit) || (97...122).contains(unit) || unit == 95 || unit >= 128 }
        func word(_ unit: UInt16) -> Bool { letter(unit) || digit(unit) || unit == 36 }
        while index < units.count {
            let start = index
            let unit = units[index]
            var kind: Kind?
            if unit == 45, index + 1 < units.count, units[index + 1] == 45 {
                index += 2
                while index < units.count, units[index] != 10, units[index] != 13 { index += 1 }
                kind = .comment
            } else if unit == 47, index + 1 < units.count, units[index + 1] == 42 {
                index += 2
                var depth = 1
                while index < units.count, depth > 0 {
                    if index + 1 < units.count, units[index] == 47, units[index + 1] == 42 { depth += 1; index += 2 }
                    else if index + 1 < units.count, units[index] == 42, units[index + 1] == 47 { depth -= 1; index += 2 }
                    else { index += 1 }
                }
                kind = .comment
            } else if unit == 39 || unit == 34 || unit == 96 {
                index += 1
                while index < units.count {
                    if units[index] == 92 { index = min(index + 2, units.count) }
                    else if units[index] == unit {
                        index += 1
                        if index < units.count, units[index] == unit { index += 1 }
                        else { break }
                    } else { index += 1 }
                }
                kind = unit == 39 ? .string : .identifier
            } else if unit == 36 {
                var end = index + 1
                while end < units.count, letter(units[end]) || (end > index + 1 && digit(units[end])) { end += 1 }
                if end < units.count, units[end] == 36 {
                    let delimiter = Array(units[index...end])
                    index = end + 1
                    while index < units.count {
                        if index + delimiter.count <= units.count, units[index..<(index + delimiter.count)].elementsEqual(delimiter) {
                            index += delimiter.count
                            break
                        }
                        index += 1
                    }
                    kind = .string
                } else { index += 1 }
            } else if digit(unit) || (unit == 46 && index + 1 < units.count && digit(units[index + 1])) {
                index += 1
                while index < units.count, digit(units[index]) { index += 1 }
                if unit != 46, index < units.count, units[index] == 46 {
                    index += 1
                    while index < units.count, digit(units[index]) { index += 1 }
                }
                if index < units.count, units[index] == 69 || units[index] == 101 {
                    var end = index + 1
                    if end < units.count, units[end] == 43 || units[end] == 45 { end += 1 }
                    let exponent = end
                    while end < units.count, digit(units[end]) { end += 1 }
                    if end > exponent { index = end }
                }
                kind = .number
            } else if letter(unit) {
                index += 1
                while index < units.count, word(units[index]) { index += 1 }
                let name = String(decoding: units[start..<index], as: UTF16.self).uppercased()
                if keywords.contains(name) { kind = .keyword }
                else {
                    var next = index
                    while next < units.count, [9, 10, 13, 32].contains(units[next]) { next += 1 }
                    if next < units.count, units[next] == 40 { kind = .function }
                }
            } else { index += 1 }
            if let kind { tokens.append(Token(range: NSRange(location: start, length: index - start), kind: kind)) }
        }
        return tokens
    }
}
