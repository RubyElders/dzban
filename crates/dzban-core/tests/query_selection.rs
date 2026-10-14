use dzban_core::query_selection::query_at_cursor;

#[test]
fn selects_statements_and_prefers_the_previous_statement_at_boundaries() {
    let sql = "SELECT 1;\n\nSELECT 2;";
    assert_eq!(query_at_cursor(sql, 3).unwrap().0, "SELECT 1");
    assert_eq!(query_at_cursor(sql, 13).unwrap().0, "SELECT 2");
    assert_eq!(query_at_cursor(sql, 9).unwrap().0, "SELECT 1");
    assert!(query_at_cursor(" \n", 1).is_none());
    assert!(query_at_cursor(sql, 999).is_none());
}

#[test]
fn native_character_offsets_remain_valid_with_unicode_sql_and_trailing_identifiers() {
    let sql = "SELECT žluť;\n SELECT číslo";
    let (query, start, end) = query_at_cursor(sql, sql.chars().count()).unwrap();
    assert_eq!(query, "SELECT číslo");
    assert_eq!(
        sql.chars()
            .skip(start)
            .take(end - start)
            .collect::<String>(),
        query
    );
    assert_eq!(query_at_cursor(sql, 8).unwrap().0, "SELECT žluť");
}
