use dzban_macos_bridge::{
    dzban_buffer_release, dzban_create, dzban_release, dzban_send, dzban_wait,
};
use serde_json::{Value, json};

fn request(app: u64, command: Value) -> Value {
    let bytes = serde_json::to_vec(&command).unwrap();
    let operation = unsafe { dzban_send(app, bytes.as_ptr(), bytes.len()) };
    assert_ne!(operation, 0);
    let buffer = dzban_wait(operation, 0, 10_000);
    assert_eq!(buffer.status, 0);
    let record: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(buffer.data, buffer.length) })
            .unwrap();
    assert!(dzban_buffer_release(buffer.token));
    assert!(!dzban_buffer_release(buffer.token));
    assert!(dzban_release(operation));
    record["payload"].clone()
}

#[test]
fn ownership_and_invalid_input_do_not_require_configuration() {
    let app = dzban_create();
    assert_ne!(app, 0);
    assert!(!dzban_release(app));
    let invalid = unsafe { dzban_send(app, std::ptr::null(), 0) };
    let buffer = dzban_wait(invalid, 0, 1000);
    assert_eq!(buffer.status, 0);
    let record: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(buffer.data, buffer.length) })
            .unwrap();
    assert_eq!(record["payload"]["failure"]["kind"], "invalid_input");
    assert!(dzban_buffer_release(buffer.token));
    assert!(dzban_release(invalid));
    for bytes in [
        b"\xff".as_slice(),
        b"{}".as_slice(),
        b"{\"command\":\"shutdown\",\"extra\":true}".as_slice(),
    ] {
        let operation = unsafe { dzban_send(app, bytes.as_ptr(), bytes.len()) };
        let buffer = dzban_wait(operation, 0, 1000);
        assert_eq!(buffer.status, 0);
        let record: Value = serde_json::from_slice(unsafe {
            std::slice::from_raw_parts(buffer.data, buffer.length)
        })
        .unwrap();
        assert_eq!(record["payload"]["failure"]["kind"], "invalid_input");
        assert!(dzban_buffer_release(buffer.token));
        assert!(dzban_release(operation));
    }
    assert_eq!(
        request(app, json!({"command": "shutdown"}))["shutdown"],
        true
    );
    assert!(dzban_release(app));
    assert!(!dzban_release(app));
    assert_eq!(dzban_wait(invalid, 0, 0).status, 4);
    assert_eq!(unsafe { dzban_send(app, std::ptr::null(), 0) }, 0);
}

#[cfg(feature = "test-fixtures")]
#[test]
fn a_foreign_consumer_can_stream_detach_cancel_and_shutdown() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    assert_eq!(
        request(app, json!({"command":"test_panic"}))["failure"]["kind"],
        "worker_failed"
    );
    let connection = request(app, json!({"command":"connect", "profile":"fixture"}));
    let session = &connection["session"];
    let document =
        request(app, json!({"command":"open_document", "session":session}))["document"].clone();
    let bytes = serde_json::to_vec(
        &json!({"command":"observe_document", "session":session, "document":document}),
    )
    .unwrap();
    let stream = unsafe { dzban_send(app, bytes.as_ptr(), bytes.len()) };
    assert_eq!(
        request(
            app,
            json!({"command":"submit", "session":session, "document":document, "sql":"wait"})
        )["generation"],
        1
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut revision = 0;
    loop {
        assert!(std::time::Instant::now() < deadline);
        let buffer = dzban_wait(stream, revision, 1000);
        if buffer.status == 1 {
            continue;
        }
        assert_eq!(buffer.status, 0);
        let record: Value = serde_json::from_slice(unsafe {
            std::slice::from_raw_parts(buffer.data, buffer.length)
        })
        .unwrap();
        dzban_buffer_release(buffer.token);
        revision = record["revision"].as_u64().unwrap();
        if record["payload"]["rows"].as_u64().unwrap() >= 5 {
            break;
        }
    }
    assert!(dzban_release(stream));
    assert!(
        request(app, json!({"command":"shutdown"}))
            .get("failure")
            .is_some()
    );
    let page = request(
        app,
        json!({"command":"read_page", "session":session, "document":document, "generation":1, "start":0, "count":5}),
    );
    assert_eq!(page["page"]["rows"][1][1], Value::Null);
    assert_eq!(page["page"]["rows"][0][1], "žluť\nNULL😀");
    assert_eq!(
        request(
            app,
            json!({"command":"cancel", "session":session, "document":document})
        )["cancellation_requested"],
        true
    );
    let stream = unsafe { dzban_send(app, bytes.as_ptr(), bytes.len()) };
    let mut revision = 0;
    loop {
        assert!(std::time::Instant::now() < deadline);
        let buffer = dzban_wait(stream, revision, 1000);
        if buffer.status == 1 {
            continue;
        }
        assert_eq!(buffer.status, 0);
        let record: Value = serde_json::from_slice(unsafe {
            std::slice::from_raw_parts(buffer.data, buffer.length)
        })
        .unwrap();
        dzban_buffer_release(buffer.token);
        revision = record["revision"].as_u64().unwrap();
        if record["payload"]["execution"] == "finished" {
            break;
        }
    }
    assert!(dzban_release(stream));
    assert_eq!(
        request(app, json!({"command":"shutdown"}))["shutdown"],
        true
    );
    assert!(dzban_release(app));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn editor_actions_select_unicode_sql_and_reject_invalid_or_stale_targets() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    let connection = request(app, json!({"command":"connect", "profile":"fixture"}));
    let session = &connection["session"];
    let document =
        request(app, json!({"command":"open_document", "session":session}))["document"].clone();
    let draft = "SELECT '😀e\u{301}'; fail";
    let start = draft[..draft.find("fail").unwrap()].chars().count();
    let action = |start, end| json!({"command":"editor_action", "session":session, "document":document, "draft":draft, "start":start, "end":end, "intent":"submit"});
    assert_eq!(
        request(app, action(start, start + 4))["submission"]["generation"],
        1
    );
    let observe = json!({"command":"observe_document", "session":session, "document":document});
    let finished = |generation| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(std::time::Instant::now() < deadline);
            let snapshot = request(app, observe.clone());
            if snapshot["generation"] == generation && snapshot["execution"] == "finished" {
                break snapshot;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    assert_eq!(finished(1)["error"]["Message"], "Fixture query failed");
    assert_eq!(request(app, action(0, 0))["submission"]["generation"], 2);
    assert_eq!(finished(2)["error"], Value::Null);
    let history = request(app, json!({"command":"history", "session":session}));
    assert!(
        history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["sql"] == "SELECT '😀e\u{301}'")
    );
    assert_eq!(
        request(app, action(1000, 1000))["failure"]["kind"],
        "selection"
    );
    assert_eq!(request(app, observe)["generation"], 2);
    request(app, json!({"command":"select_session", "session":null}));
    assert_eq!(
        request(app, action(0, 0))["failure"]["kind"],
        "selection_superseded"
    );
    assert_eq!(
        request(app, json!({"command":"shutdown"}))["shutdown"],
        true
    );
    assert!(dzban_release(app));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn viewport_previews_bound_huge_cells_and_close_drains_admitted_reads() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    let connection = request(app, json!({"command":"connect", "profile":"fixture"}));
    let session = &connection["session"];
    let document =
        request(app, json!({"command":"open_document", "session":session}))["document"].clone();
    request(
        app,
        json!({"command":"submit", "session":session, "document":document, "sql":"huge"}),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(std::time::Instant::now() < deadline);
        let snapshot = request(
            app,
            json!({"command":"observe_document", "session":session, "document":document}),
        );
        if snapshot["execution"] == "finished" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let viewport = json!({"command":"read_viewport", "session":session, "document":document, "generation":1, "start":0, "count":3, "column_start":1, "column_count":1});
    let page = request(app, viewport.clone());
    assert!(serde_json::to_vec(&page).unwrap().len() < 4096);
    assert_eq!(page["rows"][0][0]["length"], 2_500_000);
    assert_eq!(
        page["rows"][0][0]["text"].as_str().unwrap().chars().count(),
        256
    );
    assert_eq!(page["rows"][0][0]["truncated"], true);
    let tail = request(
        app,
        json!({"command":"read_cell", "session":session,"document":document,"generation":1,"row":0,"column":1,"offset":2_499_998,"count":8}),
    );
    assert_eq!(tail["text"], "😀😀");
    let bytes = serde_json::to_vec(&viewport).unwrap();
    let operations: Vec<_> = (0..8)
        .map(|_| unsafe { dzban_send(app, bytes.as_ptr(), bytes.len()) })
        .collect();
    let closed = request(
        app,
        json!({"command":"close_document", "session":session, "document":document}),
    );
    assert_eq!(closed["workspace_closed"], true);
    for operation in operations {
        let buffer = dzban_wait(operation, 0, 10_000);
        assert_eq!(buffer.status, 0);
        assert!(dzban_buffer_release(buffer.token));
        assert!(dzban_release(operation));
    }
    assert_eq!(
        request(app, json!({"command":"shutdown"}))["shutdown"],
        true
    );
    assert!(dzban_release(app));
}
