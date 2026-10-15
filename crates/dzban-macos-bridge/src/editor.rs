use dzban_core::query_selection::query_at_cursor;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorIntent {
    Submit,
    Primary,
    Cancel,
    Reset,
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_use_scalar_offsets_and_preserve_original_sql() {
        let text = "SELECT '😀e\u{301}'; SELECT 'ž';";
        assert_eq!(selected_sql(text, 0, 0).unwrap(), "SELECT '😀e\u{301}'");
        assert_eq!(selected_sql(text, 0, 12).unwrap(), "SELECT '😀e\u{301}'");
        assert_eq!(selected_sql(text, 15, 15).unwrap(), "SELECT 'ž'");
        assert_eq!(
            selected_sql(text, 13, text.chars().count()).unwrap(),
            "SELECT 'ž';"
        );
    }

    #[test]
    fn invalid_and_empty_selections_do_not_execute() {
        assert!(selected_sql("SELECT 1", 3, 2).is_err());
        assert!(selected_sql("😀", 2, 2).is_err());
        assert!(selected_sql("  ;  ", 0, 0).is_err());
        assert!(selected_sql(" SELECT 1", 0, 1).is_err());
    }
}
