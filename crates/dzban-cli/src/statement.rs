#[derive(Clone, Copy, Debug, Default)]
pub enum Dialect {
    #[default]
    Postgresql,
    Clickhouse,
}

pub fn complete(sql: &str, dialect: Dialect) -> bool {
    let bytes = sql.as_bytes();
    let mut index = 0;
    let mut parentheses = 0usize;
    let mut terminated = false;
    while index < bytes.len() {
        match bytes[index] {
            byte if byte.is_ascii_whitespace() => index += 1,
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                let mut depth = 1;
                while index < bytes.len() && depth > 0 {
                    if bytes[index..].starts_with(b"/*") {
                        depth += 1;
                        index += 2;
                    } else if bytes[index..].starts_with(b"*/") {
                        depth -= 1;
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
                if depth > 0 {
                    return false;
                }
            }
            quote @ (b'\'' | b'"' | b'`') => {
                terminated = false;
                let escapes = matches!(dialect, Dialect::Clickhouse)
                    || (quote == b'\''
                        && index > 0
                        && matches!(bytes[index - 1], b'e' | b'E')
                        && (index < 2 || !bytes[index - 2].is_ascii_alphanumeric()));
                index += 1;
                loop {
                    if index >= bytes.len() {
                        return false;
                    }
                    if escapes && bytes[index] == b'\\' {
                        index += 2;
                    } else if bytes[index] == quote {
                        index += 1;
                        if bytes.get(index) == Some(&quote) {
                            index += 1;
                        } else {
                            break;
                        }
                    } else {
                        index += 1;
                    }
                }
            }
            b'$' => {
                terminated = false;
                let start = index;
                index += 1;
                if bytes
                    .get(index)
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
                {
                    index += 1;
                    while bytes
                        .get(index)
                        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                    {
                        index += 1;
                    }
                }
                if bytes.get(index) == Some(&b'$') {
                    index += 1;
                    let delimiter = &bytes[start..index];
                    let Some(end) = bytes[index..]
                        .windows(delimiter.len())
                        .position(|part| part == delimiter)
                    else {
                        return false;
                    };
                    index += end + delimiter.len();
                }
            }
            b'(' => {
                parentheses += 1;
                terminated = false;
                index += 1;
            }
            b')' => {
                parentheses = parentheses.saturating_sub(1);
                terminated = false;
                index += 1;
            }
            b';' => {
                terminated = parentheses == 0;
                index += 1;
            }
            _ => {
                terminated = false;
                index += 1;
            }
        }
    }
    terminated
}

#[cfg(test)]
mod tests {
    use super::{Dialect, complete};

    #[test]
    fn backslash_quoting_matches_the_backend() {
        assert!(complete("SELECT '\\';", Dialect::Postgresql));
        assert!(!complete("SELECT '\\';", Dialect::Clickhouse));
        assert!(complete("SELECT 'a\\';b';", Dialect::Clickhouse));
        assert!(complete("SELECT \"\\\";", Dialect::Postgresql));
    }

    #[test]
    fn only_complete_buffers_submit() {
        for sql in [
            "SELECT 1;",
            "SELECT ';'; -- comment",
            "SELECT 1; /* nested /* ; */ */",
            "SELECT $$;$$;",
            "SELECT $body$; ' $body$;",
            "SELECT E'a\\';b';",
            "SELECT 1; SELECT 2;",
            "SELECT `;`;",
            "SELECT (';');",
        ] {
            assert!(complete(sql, Dialect::Postgresql), "{sql}");
        }
        for sql in [
            "",
            "-- ;",
            "SELECT 1",
            "SELECT ';'",
            "SELECT 'unfinished;",
            "SELECT $$unfinished;",
            "SELECT 1; SELECT 2",
            "SELECT 1; /* unfinished",
            "SELECT (1;",
            "SELECT \";\"",
            "SELECT $1",
        ] {
            assert!(!complete(sql, Dialect::Postgresql), "{sql}");
        }
    }
}
