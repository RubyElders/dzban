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
