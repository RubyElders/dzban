pub fn selected_sql(text: &str, start: usize, end: usize) -> Result<String, &'static str> {
    let length = text.chars().count();
    if start > end || end > length {
        return Err("Selection is outside the draft");
    }
    let sql = if start == end {
        query_at_cursor(text, start).map(|query| query.0)
    } else {
        Some(text.chars().skip(start).take(end - start).collect())
    };
    let sql = sql.unwrap_or_default().trim().to_owned();
    if sql.is_empty() {
        Err("Empty query")
    } else {
        Ok(sql)
    }
}

pub fn query_at_cursor(text: &str, cursor: usize) -> Option<(String, usize, usize)> {
    let byte_cursor = if cursor == text.chars().count() {
        text.len()
    } else {
        text.char_indices().nth(cursor)?.0
    };
    let (sql, start, end) = query_at_byte_cursor(text, byte_cursor)?;
    Some((
        sql,
        text[..start].chars().count(),
        text[..end].chars().count(),
    ))
}

/// Extract the SQL query at the cursor position
/// Splits by semicolons and returns the query containing the cursor
/// Returns (query_text, start_offset, end_offset)
fn query_at_byte_cursor(text: &str, cursor_pos: usize) -> Option<(String, usize, usize)> {
    // If cursor is beyond text, return None
    if cursor_pos > text.len() {
        return None;
    }

    // Split by semicolons and track positions
    let mut queries = Vec::new();
    let mut current_pos = 0;

    for part in text.split(';') {
        let part_len = part.len();
        let query_start = current_pos;
        let query_end = current_pos + part_len;

        queries.push((query_start, query_end, part));
        current_pos = query_end + 1; // +1 for the semicolon
    }

    // Strategy: Prefer the query before the cursor when at boundaries
    // 1. First, check if cursor is within a query's content (not just whitespace)
    for (start, _end, query_text) in &queries {
        let trimmed = query_text.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Find actual content boundaries (skip leading/trailing whitespace)
        let trim_start = query_text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        let trim_end = query_text
            .char_indices()
            .rfind(|(_, c)| !c.is_whitespace())
            .map(|(pos, c)| pos + c.len_utf8())
            .unwrap_or(query_text.len());

        let content_start = start + trim_start;
        let content_end = start + trim_end;

        // If cursor is within the actual content, this is our query
        if cursor_pos >= content_start && cursor_pos < content_end {
            return Some((trimmed.to_string(), content_start, content_end));
        }
    }

    // 2. Cursor is not within content - prefer the query before cursor
    for (start, _end, query_text) in queries.iter().rev() {
        let trimmed = query_text.trim();
        if trimmed.is_empty() {
            continue;
        }

        let trim_start = query_text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        let trim_end = query_text
            .char_indices()
            .rfind(|(_, c)| !c.is_whitespace())
            .map(|(pos, c)| pos + c.len_utf8())
            .unwrap_or(query_text.len());

        let content_start = start + trim_start;
        let content_end = start + trim_end;

        // Use the last query whose content ends before or at cursor
        if content_end <= cursor_pos {
            return Some((trimmed.to_string(), content_start, content_end));
        }
    }

    // 3. Still no match? Return the first non-empty query
    for (start, _end, query_text) in &queries {
        let trimmed = query_text.trim();
        if !trimmed.is_empty() {
            let trim_start = query_text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
            let trim_end = query_text
                .char_indices()
                .rfind(|(_, c)| !c.is_whitespace())
                .map(|(pos, c)| pos + c.len_utf8())
                .unwrap_or(query_text.len());

            return Some((trimmed.to_string(), start + trim_start, start + trim_end));
        }
    }

    None
}
