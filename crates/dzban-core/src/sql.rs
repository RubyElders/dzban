use serde::{Deserialize, Serialize};

pub use crate::query_selection::{query_at_cursor, selected_sql};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextOffset {
    pub bytes: usize,
    pub scalars: usize,
    pub utf16: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum SqlTokenKind {
    Keyword = 0,
    String = 1,
    Number = 2,
    Comment = 3,
    Identifier = 4,
    Function = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlToken {
    pub start: TextOffset,
    pub end: TextOffset,
    pub kind: SqlTokenKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxBatch {
    pub revision: u64,
    pub length: TextOffset,
    pub tokens: Vec<SqlToken>,
    pub done: bool,
}

pub struct SqlLexer {
    units: Vec<u16>,
    index: usize,
    offset: TextOffset,
    length: TextOffset,
    revision: u64,
}

impl SqlLexer {
    pub fn new(source: &str, revision: u64) -> Self {
        let units: Vec<u16> = source.encode_utf16().collect();
        let length = TextOffset {
            bytes: source.len(),
            scalars: source.chars().count(),
            utf16: units.len(),
        };
        Self {
            units,
            index: 0,
            offset: TextOffset::default(),
            length,
            revision,
        }
    }

    pub fn read(&mut self, count: usize) -> SyntaxBatch {
        let tokens = self.by_ref().take(count).collect();
        SyntaxBatch {
            revision: self.revision,
            length: self.length,
            tokens,
            done: self.index == self.units.len(),
        }
    }

    fn advance_offset(&mut self, start: usize) {
        for scalar in char::decode_utf16(self.units[start..self.index].iter().copied()) {
            let scalar = scalar.unwrap_or(char::REPLACEMENT_CHARACTER);
            self.offset.bytes += scalar.len_utf8();
            self.offset.scalars += 1;
        }
        self.offset.utf16 = self.index;
    }
}

impl Iterator for SqlLexer {
    type Item = SqlToken;
    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.units.len() {
            let start = self.index;
            let offset = self.offset;
            let unit = self.units[self.index];
            let mut kind = None;
            if unit == 45 && self.units.get(self.index + 1) == Some(&45) {
                self.index += 2;
                while self.index < self.units.len() && ![10, 13].contains(&self.units[self.index]) {
                    self.index += 1;
                }
                kind = Some(SqlTokenKind::Comment);
            } else if unit == 47 && self.units.get(self.index + 1) == Some(&42) {
                self.index += 2;
                let mut depth = 1;
                while self.index < self.units.len() && depth > 0 {
                    if self.units[self.index] == 47 && self.units.get(self.index + 1) == Some(&42) {
                        depth += 1;
                        self.index += 2;
                    } else if self.units[self.index] == 42
                        && self.units.get(self.index + 1) == Some(&47)
                    {
                        depth -= 1;
                        self.index += 2;
                    } else {
                        self.index += 1;
                    }
                }
                kind = Some(SqlTokenKind::Comment);
            } else if [39, 34, 96].contains(&unit) {
                self.index += 1;
                while self.index < self.units.len() {
                    if self.units[self.index] == 92 {
                        self.index = (self.index + 2).min(self.units.len());
                    } else if self.units[self.index] == unit {
                        self.index += 1;
                        if self.units.get(self.index) == Some(&unit) {
                            self.index += 1;
                        } else {
                            break;
                        }
                    } else {
                        self.index += 1;
                    }
                }
                kind = Some(if unit == 39 {
                    SqlTokenKind::String
                } else {
                    SqlTokenKind::Identifier
                });
            } else if unit == 36 {
                let mut end = self.index + 1;
                while end < self.units.len()
                    && (letter(self.units[end]) || (end > self.index + 1 && digit(self.units[end])))
                {
                    end += 1;
                }
                if self.units.get(end) == Some(&36) {
                    let delimiter = self.units[self.index..=end].to_vec();
                    self.index = end + 1;
                    while self.index < self.units.len() {
                        if self.units[self.index..].starts_with(&delimiter) {
                            self.index += delimiter.len();
                            break;
                        }
                        self.index += 1;
                    }
                    kind = Some(SqlTokenKind::String);
                } else {
                    self.index += 1;
                }
            } else if digit(unit)
                || (unit == 46
                    && self
                        .units
                        .get(self.index + 1)
                        .is_some_and(|unit| digit(*unit)))
            {
                self.index += 1;
                while self.index < self.units.len() && digit(self.units[self.index]) {
                    self.index += 1;
                }
                if unit != 46 && self.units.get(self.index) == Some(&46) {
                    self.index += 1;
                    while self.index < self.units.len() && digit(self.units[self.index]) {
                        self.index += 1;
                    }
                }
                if self
                    .units
                    .get(self.index)
                    .is_some_and(|unit| [69, 101].contains(unit))
                {
                    let mut end = self.index + 1;
                    if self
                        .units
                        .get(end)
                        .is_some_and(|unit| [43, 45].contains(unit))
                    {
                        end += 1;
                    }
                    let exponent = end;
                    while end < self.units.len() && digit(self.units[end]) {
                        end += 1;
                    }
                    if end > exponent {
                        self.index = end;
                    }
                }
                kind = Some(SqlTokenKind::Number);
            } else if letter(unit) {
                self.index += 1;
                while self.index < self.units.len() && word(self.units[self.index]) {
                    self.index += 1;
                }
                let name = String::from_utf16_lossy(&self.units[start..self.index]).to_uppercase();
                if keyword(&name) {
                    kind = Some(SqlTokenKind::Keyword);
                } else {
                    let mut next = self.index;
                    while self
                        .units
                        .get(next)
                        .is_some_and(|unit| [9, 10, 13, 32].contains(unit))
                    {
                        next += 1;
                    }
                    if self.units.get(next) == Some(&40) {
                        kind = Some(SqlTokenKind::Function);
                    }
                }
            } else {
                self.index += 1;
            }
            self.advance_offset(start);
            if let Some(kind) = kind {
                return Some(SqlToken {
                    start: offset,
                    end: self.offset,
                    kind,
                });
            }
        }
        None
    }
}

fn digit(unit: u16) -> bool {
    (48..=57).contains(&unit)
}
fn letter(unit: u16) -> bool {
    (65..=90).contains(&unit) || (97..=122).contains(&unit) || unit == 95 || unit >= 128
}
fn word(unit: u16) -> bool {
    letter(unit) || digit(unit) || unit == 36
}

fn keyword(name: &str) -> bool {
    matches!(
        name,
        "SELECT"
            | "FROM"
            | "WHERE"
            | "AS"
            | "AND"
            | "OR"
            | "NOT"
            | "NULL"
            | "TRUE"
            | "FALSE"
            | "IS"
            | "IN"
            | "BETWEEN"
            | "LIKE"
            | "ILIKE"
            | "JOIN"
            | "INNER"
            | "LEFT"
            | "RIGHT"
            | "FULL"
            | "OUTER"
            | "CROSS"
            | "ON"
            | "USING"
            | "UNION"
            | "ALL"
            | "DISTINCT"
            | "EXCEPT"
            | "INTERSECT"
            | "WITH"
            | "RECURSIVE"
            | "GROUP"
            | "BY"
            | "HAVING"
            | "ORDER"
            | "ASC"
            | "DESC"
            | "NULLS"
            | "FIRST"
            | "LAST"
            | "LIMIT"
            | "OFFSET"
            | "FETCH"
            | "NEXT"
            | "ROW"
            | "ROWS"
            | "ONLY"
            | "INSERT"
            | "INTO"
            | "VALUES"
            | "UPDATE"
            | "SET"
            | "DELETE"
            | "RETURNING"
            | "CREATE"
            | "ALTER"
            | "DROP"
            | "TABLE"
            | "VIEW"
            | "MATERIALIZED"
            | "INDEX"
            | "DATABASE"
            | "SCHEMA"
            | "IF"
            | "EXISTS"
            | "PRIMARY"
            | "KEY"
            | "FOREIGN"
            | "REFERENCES"
            | "UNIQUE"
            | "CHECK"
            | "DEFAULT"
            | "CONSTRAINT"
            | "CASE"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "END"
            | "CAST"
            | "OVER"
            | "PARTITION"
            | "WINDOW"
            | "RANGE"
            | "UNBOUNDED"
            | "PRECEDING"
            | "FOLLOWING"
            | "CURRENT"
            | "FILTER"
            | "WITHIN"
            | "LATERAL"
            | "EXPLAIN"
            | "ANALYZE"
            | "VERBOSE"
            | "FORMAT"
            | "BEGIN"
            | "COMMIT"
            | "ROLLBACK"
            | "TRANSACTION"
            | "SAVEPOINT"
            | "RELEASE"
            | "TRUNCATE"
            | "GRANT"
            | "REVOKE"
            | "SHOW"
            | "DESCRIBE"
            | "USE"
            | "SETTINGS"
            | "PREWHERE"
            | "ARRAY"
            | "FINAL"
            | "SAMPLE"
            | "ENGINE"
            | "INT"
            | "INTEGER"
            | "BIGINT"
            | "SMALLINT"
            | "FLOAT"
            | "DOUBLE"
            | "DECIMAL"
            | "NUMERIC"
            | "BOOLEAN"
            | "BOOL"
            | "CHAR"
            | "VARCHAR"
            | "TEXT"
            | "DATE"
            | "TIME"
            | "TIMESTAMP"
            | "INTERVAL"
            | "UUID"
            | "JSON"
            | "JSONB"
    )
}
