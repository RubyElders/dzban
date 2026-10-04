use dzban_core::history::{QueryHistoryEntry, QueryHistoryStore};

struct Fixture(std::path::PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn damaged_records_do_not_hide_valid_history_or_block_new_writes() {
    let directory =
        std::env::temp_dir().join(format!("dzban-history-recovery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let _fixture = Fixture(directory.clone());
    let path = directory.join("history.json");
    let mut history = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    history
        .add(QueryHistoryEntry::failure(
            "before".into(),
            "fixture".into(),
            None,
            None,
        ))
        .unwrap();
    let records = path.with_extension("entries");
    let valid = std::fs::read_dir(&records)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut future: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&valid).unwrap()).unwrap();
    future["version"] = serde_json::json!(99);
    let future_bytes = serde_json::to_vec(&future).unwrap();
    std::fs::write(records.join("future.json"), &future_bytes).unwrap();
    future["version"] = serde_json::json!(1);
    future["database"] = serde_json::json!("other-profile");
    std::fs::write(
        records.join("foreign.json"),
        serde_json::to_vec(&future).unwrap(),
    )
    .unwrap();
    std::fs::write(records.join("broken.json"), b"{truncated").unwrap();
    std::fs::write(records.join("unpublished.tmp"), b"{truncated").unwrap();
    std::fs::create_dir(records.join("unreadable.json")).unwrap();

    let mut recovered = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    assert_eq!(recovered.entries().len(), 1);
    assert_eq!(recovered.entries()[0].sql, "before");
    recovered
        .add(QueryHistoryEntry::failure(
            "after".into(),
            "fixture".into(),
            None,
            None,
        ))
        .unwrap();
    let reloaded = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    assert_eq!(reloaded.entries().len(), 2);
    assert!(reloaded.entries().iter().any(|entry| entry.sql == "after"));
    assert_eq!(
        std::fs::read(records.join("future.json")).unwrap(),
        future_bytes
    );
    assert_eq!(
        std::fs::read(records.join("broken.json")).unwrap(),
        b"{truncated"
    );
}
