use dzban_core::history::{QueryHistoryEntry, QueryHistoryStore};

#[test]
fn refresh_merges_legacy_records_without_duplicates_and_keeps_stable_order() {
    let directory =
        std::env::temp_dir().join(format!("dzban-history-refresh-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let entries: Vec<_> = ["newest", "same-time", "oldest"]
        .into_iter()
        .map(|sql| {
            let mut entry = QueryHistoryEntry::failure(sql.into(), "fixture".into(), None, None);
            entry.executed_at = 1;
            entry
        })
        .collect();
    let legacy = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "database": "fixture",
        "entries": entries,
    }))
    .unwrap();
    std::fs::write(&path, &legacy).unwrap();
    let mut reader = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    let mut writer = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    writer
        .add(QueryHistoryEntry::failure(
            "published".into(),
            "fixture".into(),
            None,
            None,
        ))
        .unwrap();
    assert_eq!(reader.entries().len(), 3);
    for _ in 0..3 {
        reader.refresh().unwrap();
        let sql: Vec<_> = reader
            .entries()
            .iter()
            .map(|entry| entry.sql.as_str())
            .collect();
        assert_eq!(sql, ["published", "newest", "same-time", "oldest"]);
        assert_eq!(std::fs::read(&path).unwrap(), legacy);
    }
    for index in 0..101 {
        writer
            .add(QueryHistoryEntry::failure(
                format!("query-{index}"),
                "fixture".into(),
                None,
                None,
            ))
            .unwrap();
    }
    reader.refresh().unwrap();
    assert_eq!(reader.entries().len(), 100);
    assert_eq!(reader.entries()[0].sql, "query-100");
    assert_eq!(reader.entries()[99].sql, "query-1");
    assert_eq!(
        std::fs::read_dir(path.with_extension("entries"))
            .unwrap()
            .count(),
        102
    );
    assert_eq!(std::fs::read(&path).unwrap(), legacy);
    std::fs::remove_dir_all(directory).unwrap();
}
