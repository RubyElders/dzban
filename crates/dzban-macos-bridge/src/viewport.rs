use dzban_core::{ResultCell, ResultSet};
use serde_json::{Value, json};

pub const MAX_VIEW_COLUMNS: usize = 16;
pub const CELL_PREVIEW_SCALARS: usize = 256;

pub fn cells(page: ResultSet, column_start: usize, column_count: usize) -> Value {
    json!(page.rows.iter().map(|row| row.iter().skip(column_start).take(column_count).map(|cell| match cell {
        ResultCell::Null => json!({"text":null, "length":0, "truncated":false}),
        ResultCell::Text(text) => {
            let length = text.chars().count();
            json!({"text":text.chars().take(CELL_PREVIEW_SCALARS).collect::<String>(), "length":length, "truncated":length > CELL_PREVIEW_SCALARS})
        }
    }).collect::<Vec<_>>()).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_preserve_null_unicode_line_breaks_and_column_order() {
        let page = ResultSet {
            columns: vec![],
            rows: vec![vec![
                ResultCell::Text("ignored".into()),
                ResultCell::Null,
                ResultCell::Text("NULL\n😀e\u{301}".into()),
                ResultCell::Text("😀".repeat(1000)),
            ]],
        };
        let cells = cells(page, 1, 3);
        assert_eq!(cells[0][0]["text"], Value::Null);
        assert_eq!(cells[0][1]["text"], "NULL\n😀e\u{301}");
        assert_eq!(cells[0][2]["length"], 1000);
        assert_eq!(
            cells[0][2]["text"].as_str().unwrap().chars().count(),
            CELL_PREVIEW_SCALARS
        );
        assert_eq!(cells[0][2]["truncated"], true);
    }
}
