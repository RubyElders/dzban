use dzban_core::result_store::{ResultCutoff, ResultLimits, ResultStore};
use dzban_core::{ResultCell, ResultColumn};

#[test]
fn disk_pages_respect_byte_working_set_and_make_progress_on_a_large_cell() {
    let mut store = ResultStore::new(
        vec![ResultColumn {
            name: "v".into(),
            type_name: "text".into(),
        }],
        ResultLimits {
            memory_bytes: 16,
            memory_rows: 0,
            ..Default::default()
        },
    )
    .unwrap();
    for text in [
        "123456",
        "abcdef",
        "a cell larger than the working set",
        "last",
    ] {
        store.push(vec![ResultCell::Text(text.into())]).unwrap();
    }
    assert_eq!(store.read_page(0, 4).unwrap().len(), 1);
    assert_eq!(store.read_page(1, 4).unwrap().len(), 1);
    let large = store.read_page(2, 4).unwrap();
    assert_eq!(large.len(), 1);
    assert_eq!(large[0][0].display(), "a cell larger than the working set");
    assert_eq!(store.read_page(3, 4).unwrap()[0][0].display(), "last");
}

#[test]
fn readers_can_page_existing_results_while_a_worker_appends_more() {
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        ResultStore::new(
            vec![ResultColumn {
                name: "id".into(),
                type_name: "text".into(),
            }],
            ResultLimits {
                rows: 3,
                memory_rows: 1,
                ..Default::default()
            },
        )
        .unwrap(),
    ));
    let worker = store.clone();
    std::thread::spawn(move || {
        worker
            .lock()
            .unwrap()
            .push(vec![ResultCell::Text("first".into())])
            .unwrap();
    })
    .join()
    .unwrap();
    let first = store.lock().unwrap().read_page(0, 1).unwrap();
    assert_eq!(first, vec![vec![ResultCell::Text("first".into())]]);
    let worker = store.clone();
    std::thread::spawn(move || {
        let mut store = worker.lock().unwrap();
        assert_eq!(store.push(vec![ResultCell::Null]).unwrap(), None);
        assert_eq!(
            store.push(vec![ResultCell::Text("last".into())]).unwrap(),
            Some(ResultCutoff::Rows)
        );
    })
    .join()
    .unwrap();
    let mut store = store.lock().unwrap();
    assert_eq!(store.len(), 3);
    assert_eq!(store.read_page(0, 1).unwrap(), first);
    assert_eq!(
        store.read_page(1, 2).unwrap(),
        vec![
            vec![ResultCell::Null],
            vec![ResultCell::Text("last".into())]
        ]
    );
}
