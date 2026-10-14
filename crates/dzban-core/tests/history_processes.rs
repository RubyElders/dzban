use std::process::Command;

use dzban_core::history::{QueryHistoryEntry, QueryHistoryStore};

#[test]
fn unpublished_writer_process() {
    use std::io::Write;
    let Some(directory) = std::env::var_os("DZBAN_TEST_UNPUBLISHED_HISTORY") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    let mut file = std::fs::File::create(directory.join("interrupted.tmp")).unwrap();
    file.write_all(br#"{"version":1,"database":"fixture","entry":{"sql":"unpublished""#)
        .unwrap();
    file.sync_all().unwrap();
    let record = serde_json::json!({
        "version": 1,
        "database": "fixture",
        "published_at": 1,
        "entry": QueryHistoryEntry::failure("unpublished-complete".into(), "fixture".into(), None, None),
    });
    let mut complete = std::fs::File::create(directory.join("complete.tmp")).unwrap();
    complete
        .write_all(&serde_json::to_vec(&record).unwrap())
        .unwrap();
    complete.sync_all().unwrap();
    std::fs::write(directory.join("writer-ready"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn killed_writer_does_not_publish_partial_history_or_block_later_publication() {
    let directory =
        std::env::temp_dir().join(format!("dzban-history-crash-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let mut store = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    store
        .add(QueryHistoryEntry::failure(
            "committed".into(),
            "fixture".into(),
            None,
            None,
        ))
        .unwrap();
    let records = path.with_extension("entries");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "unpublished_writer_process"])
        .env("DZBAN_TEST_UNPUBLISHED_HISTORY", &records)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !records.join("writer-ready").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let ready = records.join("writer-ready").exists();
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(
        ready,
        "Writer did not reach the unpublished-record boundary"
    );
    let partial = std::fs::read(records.join("interrupted.tmp")).unwrap();
    let mut reader = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
    assert_eq!(reader.entries().len(), 1);
    assert_eq!(reader.entries()[0].sql, "committed");
    reader
        .add(QueryHistoryEntry::failure(
            "after-crash".into(),
            "fixture".into(),
            None,
            None,
        ))
        .unwrap();
    store.refresh().unwrap();
    assert_eq!(store.entries().len(), 2);
    assert!(store.entries().iter().any(|entry| entry.sql == "committed"));
    assert!(
        store
            .entries()
            .iter()
            .any(|entry| entry.sql == "after-crash")
    );
    assert_eq!(
        std::fs::read(records.join("interrupted.tmp")).unwrap(),
        partial
    );
    assert!(records.join("complete.tmp").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn writer_process() {
    let Some(path) = std::env::var_os("DZBAN_TEST_HISTORY") else {
        return;
    };
    let label = std::env::var("DZBAN_TEST_WRITER").unwrap();
    let mut store = QueryHistoryStore::load_from_path(path.into(), "fixture").unwrap();
    for index in 0..20 {
        store
            .add(QueryHistoryEntry::failure(
                format!("{label}-{index}"),
                "fixture".into(),
                None,
                None,
            ))
            .unwrap();
    }
}

#[test]
fn independent_processes_do_not_lose_history_updates() {
    let directory =
        std::env::temp_dir().join(format!("dzban-history-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("history.json");
    let executable = std::env::current_exe().unwrap();
    let mut children = Vec::new();
    for writer in ["gtk", "cli"] {
        children.push(
            Command::new(&executable)
                .args(["--exact", "writer_process"])
                .env("DZBAN_TEST_HISTORY", &path)
                .env("DZBAN_TEST_WRITER", writer)
                .spawn()
                .unwrap(),
        );
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let history = QueryHistoryStore::load_from_path(path, "fixture").unwrap();
    assert_eq!(history.entries().len(), 40);
    for writer in ["gtk", "cli"] {
        for index in 0..20 {
            assert!(
                history
                    .entries()
                    .iter()
                    .any(|entry| entry.sql == format!("{writer}-{index}"))
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
