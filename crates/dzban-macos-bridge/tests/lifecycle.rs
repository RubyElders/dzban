use dzban_macos_bridge::{
    dzban_buffer_release, dzban_create, dzban_release, dzban_send, dzban_wait,
};
use serde_json::{Value, json};

#[test]
fn result_view_policy_is_pure_bounded_and_uses_owned_buffers() {
    use dzban_macos_bridge::dzban_result_view;
    assert_eq!(unsafe { dzban_result_view(std::ptr::null(), 0) }.status, 7);
    for invalid in [
        b"{}".as_slice(),
        b"\xff".as_slice(),
        br#"{"action":"select","row":0,"column":0,"rows":1,"columns":1,"extra":true}"#.as_slice(),
    ] {
        assert_eq!(
            unsafe { dzban_result_view(invalid.as_ptr(), invalid.len()) }.status,
            7
        );
    }
    let input = serde_json::to_vec(&json!({"action":"clamp", "position":{"row":u64::MAX,"column":u64::MAX}, "rows":3,"columns":2})).unwrap();
    let buffer = unsafe { dzban_result_view(input.as_ptr(), input.len()) };
    assert_eq!(buffer.status, 0);
    let record: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(buffer.data, buffer.length) })
            .unwrap();
    assert_eq!(record["schema"], 1);
    assert_eq!(record["payload"]["selection"], json!({"row":2,"column":1}));
    assert!(dzban_buffer_release(buffer.token));
    assert!(!dzban_buffer_release(buffer.token));
}

#[test]
fn syntax_readers_are_bounded_lossless_revision_scoped_and_independent_of_an_app() {
    use dzban_macos_bridge::{dzban_syntax_create, dzban_syntax_read};
    assert_eq!(unsafe { dzban_syntax_create([0xff].as_ptr(), 1, 1) }, 0);
    let empty = unsafe { dzban_syntax_create(std::ptr::null(), 0, u64::MAX) };
    assert_ne!(empty, 0);
    let buffer = dzban_syntax_read(empty);
    assert_eq!(buffer.status, 0);
    assert_eq!(buffer.length, 56);
    let words: Vec<_> = unsafe { std::slice::from_raw_parts(buffer.data, buffer.length) }
        .as_chunks::<8>()
        .0
        .iter()
        .map(|bytes| u64::from_le_bytes(*bytes))
        .collect();
    assert_eq!(words, [1, u64::MAX, 0, 0, 0, 0, 1]);
    assert!(dzban_buffer_release(buffer.token));
    assert!(dzban_release(empty));
    assert_eq!(dzban_syntax_read(empty).status, 4);
    let source = "SELECT '😀', 1;\n".repeat(1000);
    let expected: Vec<_> = dzban_core::sql::SqlLexer::new(&source, 1).collect();
    let reader = unsafe { dzban_syntax_create(source.as_ptr(), source.len(), u64::MAX) };
    assert_ne!(reader, 0);
    let mut tokens = Vec::new();
    loop {
        let buffer = dzban_syntax_read(reader);
        assert_eq!(buffer.status, 0);
        assert!(buffer.length < 256 * 1024);
        assert_eq!(buffer.length % 8, 0);
        let words: Vec<_> = unsafe { std::slice::from_raw_parts(buffer.data, buffer.length) }
            .as_chunks::<8>()
            .0
            .iter()
            .map(|bytes| u64::from_le_bytes(*bytes))
            .collect();
        assert_eq!(words[0], 1);
        assert_eq!(words[1], u64::MAX);
        assert_eq!(words[2], source.len() as u64);
        assert!(words[5] <= 512);
        assert_eq!(words.len(), 7 + words[5] as usize * 7);
        tokens.extend(
            words[7..]
                .as_chunks::<7>()
                .0
                .iter()
                .map(|token| token.to_vec()),
        );
        let done = words[6] == 1;
        assert!(dzban_buffer_release(buffer.token));
        if done {
            break;
        }
    }
    let expected: Vec<_> = expected
        .iter()
        .map(|token| {
            vec![
                token.start.bytes as u64,
                token.start.scalars as u64,
                token.start.utf16 as u64,
                token.end.bytes as u64,
                token.end.scalars as u64,
                token.end.utf16 as u64,
                token.kind as u64,
            ]
        })
        .collect();
    assert_eq!(tokens, expected);
    assert!(dzban_release(reader));
    assert!(!dzban_release(reader));
    assert_eq!(dzban_syntax_read(reader).status, 4);
}

#[cfg(feature = "test-fixtures")]
#[test]
fn attention_identity_and_visibility_cross_the_ffi_without_document_observers() {
    fn attention(app: u64, input: Value) -> Value {
        let bytes = serde_json::to_vec(&input).unwrap();
        let buffer =
            unsafe { dzban_macos_bridge::dzban_attention(app, bytes.as_ptr(), bytes.len()) };
        assert_eq!(buffer.status, 0);
        let record: Value = serde_json::from_slice(unsafe {
            std::slice::from_raw_parts(buffer.data, buffer.length)
        })
        .unwrap();
        assert!(dzban_buffer_release(buffer.token));
        assert!(!dzban_buffer_release(buffer.token));
        record["payload"].clone()
    }
    assert_eq!(
        unsafe { dzban_macos_bridge::dzban_attention(0, std::ptr::null(), 0) }.status,
        4
    );
    let app = dzban_macos_bridge::dzban_create_fixture();
    assert_eq!(
        unsafe { dzban_macos_bridge::dzban_attention(app, std::ptr::null(), 0) }.status,
        7
    );
    let connection = request(app, json!({"command":"connect", "profile":"fixture"}));
    let tab = &connection["tab"];
    let session = &connection["session"];
    let document =
        request(app, json!({"command":"open_document", "session":session}))["document"].clone();
    attention(
        app,
        json!({"event":"visibility", "visible":false,"viewed":null}),
    );
    let query = request(
        app,
        json!({"command":"submit","session":session,"document":document,"sql":"few"}),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let finished = loop {
        assert!(std::time::Instant::now() < deadline);
        let snapshot = attention(
            app,
            json!({"event":"visibility","visible":false,"viewed":null}),
        );
        if !snapshot["tabs"][0]["unseen"].is_null() {
            break snapshot;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert_eq!(finished["request_revision"], 1);
    assert_eq!(
        finished["tabs"][0]["unseen"]["identity"]["query"],
        query["query"]
    );
    let stale = attention(
        app,
        json!({"event":"visibility","visible":true,"viewed":{"tab":tab,"document":document,"generation":0}}),
    );
    assert!(!stale["tabs"][0]["unseen"].is_null());
    let viewed = attention(
        app,
        json!({"event":"viewed","output":{"tab":tab,"document":document,"generation":query["generation"]}}),
    );
    assert!(viewed["tabs"][0]["unseen"].is_null());
    assert_eq!(viewed["tabs"][0]["completion_revision"], 1);
    let repeated = attention(
        app,
        json!({"event":"visibility","visible":false,"viewed":null}),
    );
    assert!(repeated["tabs"][0]["unseen"].is_null());
    assert_eq!(repeated["request_revision"], 1);
    let shutdown = request(app, json!({"command":"shutdown"}));
    assert!(shutdown["attention"]["tabs"].as_array().unwrap().is_empty());
    assert_eq!(shutdown["attention"]["cancel_attention"], true);
    assert!(dzban_release(app));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn foreign_workspace_restore_survives_observer_loss_and_saves_only_logical_state() {
    let path = std::env::temp_dir().join(format!(
        "dzban-foreign-workspace-{}.json",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&path, serde_json::to_vec(&json!({"version":1,"tabs":[{"profile":"fixture-slow","draft":"SELECT '😀';"},{"draft":"chooser"},{"profile":"missing","draft":"retry"}],"selected":99})).unwrap()).unwrap();
    let app = dzban_macos_bridge::dzban_create_fixture();
    let command = json!({"command":"restore_workspace","location":{"host":"macos","path":path}});
    let bytes = serde_json::to_vec(&command).unwrap();
    let first = unsafe { dzban_send(app, bytes.as_ptr(), bytes.len()) };
    let mut revision = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(std::time::Instant::now() < deadline);
        let buffer = dzban_wait(first, revision, 1000);
        if buffer.status == 1 {
            continue;
        }
        assert_eq!(buffer.status, 0);
        let record: Value = serde_json::from_slice(unsafe {
            std::slice::from_raw_parts(buffer.data, buffer.length)
        })
        .unwrap();
        revision = record["revision"].as_u64().unwrap();
        assert!(dzban_buffer_release(buffer.token));
        if record["payload"]["workspace"]["tabs"][0]["phase"] == "connecting" {
            break;
        }
    }
    assert!(dzban_release(first));
    let second = unsafe { dzban_send(app, bytes.as_ptr(), bytes.len()) };
    revision = 0;
    let restored = loop {
        assert!(std::time::Instant::now() < deadline);
        let buffer = dzban_wait(second, revision, 1000);
        if buffer.status == 1 {
            continue;
        }
        assert_eq!(buffer.status, 0);
        let record: Value = serde_json::from_slice(unsafe {
            std::slice::from_raw_parts(buffer.data, buffer.length)
        })
        .unwrap();
        revision = record["revision"].as_u64().unwrap();
        assert!(dzban_buffer_release(buffer.token));
        if record["payload"]["progress"]["finished"] == true {
            break record["payload"].clone();
        }
    };
    assert!(dzban_release(second));
    let tabs = restored["workspace"]["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 3);
    assert_eq!(restored["progress"]["issues"].as_array().unwrap().len(), 1);
    assert_eq!(restored["workspace"]["selected"], tabs[2]["tab"]);
    assert_eq!(tabs[0]["snapshot"]["draft"], "SELECT '😀';");
    assert_eq!(tabs[0]["snapshot"]["generation"], 0);
    assert_eq!(tabs[0]["phase"], "ready");
    assert_eq!(
        request(
            app,
            json!({"command":"actions","session":tabs[0]["session"]})
        )["session"],
        tabs[0]["session"]
    );
    assert_eq!(
        request(
            app,
            json!({"command":"save_workspace","location":{"host":"macos","path":path},"drafts":[{"tab":tabs[0]["tab"],"draft":"not yet sent"}]})
        )["saved"],
        true
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["tabs"][0]["draft"], "not yet sent");
    assert_eq!(saved["selected"], 2);
    assert_eq!(saved.as_object().unwrap().len(), 3);
    assert_eq!(saved["tabs"][0].as_object().unwrap().len(), 2);
    let large = "😀".repeat(300_000);
    assert_eq!(
        request(
            app,
            json!({"command":"set_tab_draft","tab":tabs[0]["tab"],"draft":large})
        )["failure"]["kind"],
        "invalid_input"
    );
    assert_eq!(
        request(
            app,
            json!({"command":"save_workspace","location":{"host":"macos","path":path},"drafts":[{"tab":tabs[0]["tab"],"draft":large}]})
        )["saved"],
        true
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["tabs"][0]["draft"], large);
    assert_eq!(
        request(app, json!({"command":"shutdown"}))["shutdown"],
        true
    );
    assert!(dzban_release(app));
    std::fs::remove_file(path).unwrap();
}

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
    let export = uuid::Uuid::new_v4();
    assert_eq!(
        request(
            app,
            json!({"command":"start_export", "export":export, "session":session, "document":document, "generation":1, "row":0, "column":1})
        )["opened"],
        true
    );
    let chunk = request(app, json!({"command":"read_export", "export":export}));
    assert_eq!(chunk["text"].as_str().unwrap().len(), 65_536);
    assert_eq!(chunk["done"], false);
    assert_eq!(
        request(app, json!({"command":"close_export", "export":export}))["closed"],
        true
    );
    assert_eq!(
        request(app, json!({"command":"read_export", "export":export}))["failure"]["kind"],
        "stale_output"
    );
    assert_eq!(
        request(
            app,
            json!({"command":"start_export", "export":export, "session":session, "document":document, "generation":1, "row":1, "column":1})
        )["opened"],
        true
    );
    let null = request(app, json!({"command":"read_export", "export":export}));
    assert_eq!(null["text"], "");
    assert_eq!(null["is_null"], true);
    assert_eq!(null["done"], true);
    let chooser = request(app, json!({"command":"open_tab"}));
    let closed_chooser = request(app, json!({"command":"close_tab", "tab":chooser["tab"]}));
    assert!(closed_chooser.get("failure").is_none());
    assert_eq!(
        request(app, json!({"command":"read_export", "export":export}))["done"],
        true
    );
    let bytes = serde_json::to_vec(&viewport).unwrap();
    let operations: Vec<_> = (0..8)
        .map(|_| unsafe { dzban_send(app, bytes.as_ptr(), bytes.len()) })
        .collect();
    let closed = request(
        app,
        json!({"command":"close_document", "session":session, "document":document}),
    );
    assert_eq!(closed["workspace_closed"], true);
    assert_eq!(
        request(app, json!({"command":"read_export", "export":export}))["failure"]["kind"],
        "stale_output"
    );
    assert_eq!(
        request(app, json!({"command":"close_export", "export":export}))["closed"],
        true
    );
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

#[cfg(feature = "test-fixtures")]
#[test]
fn catalog_bridge_preserves_typed_identity_nulls_and_capability_filtering() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    let session = request(app, json!({"command":"connect","profile":"fixture"}))["session"].clone();
    let actions = request(app, json!({"command":"actions","session":session}));
    let ids: Vec<_> = actions["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"catalog.list.table"));
    assert!(ids.contains(&"catalog.describe.table"));
    assert!(!ids.contains(&"catalog.list.schema"));
    let entries = request(
        app,
        json!({"command":"catalog","session":session,"request":{"operation":"list","kind":"table"}}),
    );
    let entry = &entries["entries"][0];
    assert_eq!(entry["identity"]["name"], "žluť table");
    assert_eq!(entry["attributes"]["comment"], Value::Null);
    let described = request(
        app,
        json!({"command":"catalog","session":session,"request":{"operation":"describe","object":entry["identity"],"full":true}}),
    );
    assert_eq!(described["entries"][0]["identity"], entry["identity"]);
    assert_eq!(described["entries"][0]["attributes"]["column_name"], "id");
    let unsupported = request(
        app,
        json!({"command":"catalog","session":session,"request":{"operation":"list","kind":"schema"}}),
    );
    assert_eq!(unsupported["failure"]["kind"], "catalog_unsupported");
    assert!(
        unsupported["failure"]["message"]
            .as_str()
            .unwrap()
            .contains("Unsupported")
    );
    request(app, json!({"command":"shutdown"}));
    assert!(dzban_release(app));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn native_peek_surfaces_share_tap_hold_pin_and_focus_reset_policy() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    let session = request(app, json!({"command":"connect","profile":"fixture"}))["session"].clone();
    for surface in ["plan", "row"] {
        let event = |event: &str, elapsed: u64| {
            request(
                app,
                json!({"command":"peek","session":session,"surface":surface,"event":event,"elapsed_ms":elapsed}),
            )
        };
        assert_eq!(event("show", 0)["visible"], true);
        assert_eq!(event("release", 10)["pinned"], true);
        assert_eq!(event("reset", 0)["visible"], true);
        assert_eq!(event("close", 0)["visible"], false);
        event("show", 0);
        assert_eq!(event("release", 250)["visible"], false);
        event("show", 0);
        assert_eq!(event("reset", 0)["visible"], false);
        assert_eq!(event("open", 0)["pinned"], true);
        event("close", 0);
    }
    request(app, json!({"command":"shutdown"}));
    assert!(dzban_release(app));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn preview_intent_tombstones_reject_late_admission_and_old_cancellation() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    let session = request(app, json!({"command":"connect","profile":"fixture"}))["session"].clone();
    let document =
        request(app, json!({"command":"open_document","session":session}))["document"].clone();
    let snapshot = request(
        app,
        json!({"command":"set_draft","session":session,"document":document,"draft":"SELECT 1; SELECT 2"}),
    );
    let revision = &snapshot["draft_revision"];
    request(
        app,
        json!({"command":"cancel_preview","session":session,"request_revision":10}),
    );
    let preview = |intent| json!({"command":"preview","session":session,"document":document,"sql":"SELECT 1; SELECT 2","revision":revision,"start":1,"end":1,"request_revision":intent});
    assert_eq!(
        request(app, preview(9))["failure"]["kind"],
        "preview_invalidated"
    );
    let captured = request(app, preview(11));
    assert_eq!(captured["plan"]["document"]["sql"], "SELECT 1");
    assert_eq!(captured["plan"]["text"], "fixture: true\nsql: SELECT 1");
    assert_eq!(
        request(
            app,
            json!({"command":"cancel_preview","session":session,"request_revision":10})
        )["failure"]["kind"],
        "preview_invalidated"
    );
    let context = request(
        app,
        json!({"command":"context_changed","session":session,"document":document}),
    );
    assert!(context["draft_revision"].as_u64().unwrap() > revision.as_u64().unwrap());
    assert_eq!(
        request(app, preview(12))["failure"]["kind"],
        "preview_invalidated"
    );
    request(app, json!({"command":"shutdown"}));
    assert!(dzban_release(app));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn saved_history_replacement_keeps_background_execution_and_rejects_stale_pages() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    let session = request(app, json!({"command":"connect","profile":"fixture"}))["session"].clone();
    let document =
        request(app, json!({"command":"open_document","session":session}))["document"].clone();
    let snapshot = || {
        request(
            app,
            json!({"command":"observe_document","session":session,"document":document}),
        )
    };
    let wait = |phase: &str, rows: u64| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let current = snapshot();
            if current["execution"] == phase && current["rows"] == rows {
                break current;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "document did not reach {phase}: {current}"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    let sql = "ž".repeat(600);
    request(
        app,
        json!({"command":"submit","session":session,"document":document,"sql":sql}),
    );
    wait("finished", 500);
    let history = request(app, json!({"command":"history_list","session":session}));
    let revision = &history["revision"];
    assert_eq!(history["entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        history["entries"][0]["sql_preview"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        256
    );
    request(
        app,
        json!({"command":"submit","session":session,"document":document,"sql":"wait"}),
    );
    let running = wait("running", 5);
    let started = running["started_at_seconds"].as_f64().unwrap();
    assert!(started > 1_700_000_000.0);
    assert_eq!(running["execution_plan"]["estimate"]["rows"], 500);
    assert_eq!(running["feedback"]["progress"], "running");
    assert_eq!(running["execution_plan"]["estimate"]["total_cost"], 12.5);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let later = snapshot();
    assert_eq!(later["started_at_seconds"], running["started_at_seconds"]);
    assert!(
        later["elapsed_seconds"].as_f64().unwrap() > running["elapsed_seconds"].as_f64().unwrap()
    );
    let restored = request(
        app,
        json!({"command":"restore_history","session":session,"document":document,"history_revision":revision,"index":0}),
    );
    assert_eq!(restored["history"], true);
    assert_eq!(restored["execution"], "running");
    assert_eq!(restored["rows"], 200);
    assert_eq!(restored["history_total_rows"], 500);
    assert_eq!(restored["feedback"]["status"], "saved_rows");
    assert_eq!(restored["feedback"]["progress"], "running");
    assert_eq!(restored["feedback"]["rows"], 200);
    assert_eq!(restored["feedback"]["total_rows"], 500);
    assert_eq!(restored["draft"], sql);
    let generation = &restored["generation"];
    let page = request(
        app,
        json!({"command":"read_page","session":session,"document":document,"generation":generation,"start":199,"count":1}),
    );
    assert_eq!(page["page"]["rows"][0][0], "199");
    let null_text = request(
        app,
        json!({"command":"read_cell","session":session,"document":document,"generation":generation,"row":1,"column":1,"offset":0,"count":10}),
    );
    assert_eq!(null_text["text"], "NULL");
    let stale = request(
        app,
        json!({"command":"read_page","session":session,"document":document,"generation":2,"start":0,"count":1}),
    );
    assert_eq!(stale["failure"]["kind"], "stale_output");
    request(
        app,
        json!({"command":"cancel","session":session,"document":document}),
    );
    let finished = wait("finished", 200);
    assert_eq!(finished["history"], true);
    let plan = request(
        app,
        json!({"command":"saved_plan","session":session,"document":document,"generation":generation}),
    );
    assert_eq!(plan["plan"]["document"]["sql"], sql);
    assert_eq!(plan["plan"]["text"], format!("fixture: true\nsql: {sql}"));
    request(app, json!({"command":"history_list","session":session}));
    let old = request(
        app,
        json!({"command":"restore_history","session":session,"document":document,"history_revision":revision,"index":0}),
    );
    assert_eq!(old["failure"]["kind"], "stale_history");
    request(
        app,
        json!({"command":"submit","session":session,"document":document,"sql":"restart"}),
    );
    let restarted = wait("finished", 500);
    assert!(restarted.get("history").is_none());
    request(app, json!({"command":"shutdown"}));
    assert!(dzban_release(app));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn probes_and_shared_input_preserve_query_and_selection_boundaries() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    request(app, json!({"command":"profiles"}));
    let probe = request(app, json!({"command":"probe_profile","profile":"fixture"}));
    assert_eq!(probe["state"], "connected");
    assert_eq!(probe["latency_micros"], 1);
    assert_eq!(probe["version"], "fixture");
    assert_eq!(
        request(app, json!({"command":"connections"}))["connections"],
        json!([])
    );
    let session = request(app, json!({"command":"connect","profile":"fixture"}))["session"].clone();
    let document = request(app, json!({"command":"open_document","session":session}));
    let input = |event: Value| {
        request(
            app,
            json!({"command":"input","session":session,"document":document["document"],"revision":document["draft_revision"],"input":event}),
        )
    };
    assert_eq!(
        input(json!({"kind":"press","key":"primary","at_ms":1}))["hint"]["kind"],
        "primary_guide"
    );
    assert_eq!(
        input(json!({"kind":"press","key":"preview","at_ms":2}))["commands"],
        json!(["begin_preview"])
    );
    assert_eq!(
        input(json!({"kind":"press","key":"submit","at_ms":3}))["commands"],
        json!(["run_query"])
    );
    assert_eq!(
        input(json!({"kind":"press","key":"submit","at_ms":4}))["commands"],
        json!([])
    );
    request(
        app,
        json!({"command":"submit","session":session,"document":document["document"],"sql":"wait"}),
    );
    let busy = request(
        app,
        json!({"command":"probe_session","session":session,"include_version":true}),
    );
    assert_eq!(busy["state"], "busy");
    assert_eq!(
        input(json!({"kind":"press","key":"cancel","at_ms":5}))["commands"],
        json!(["cancel_query"])
    );
    request(
        app,
        json!({"command":"cancel","session":session,"document":document["document"]}),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let probe = request(
            app,
            json!({"command":"probe_session","session":session,"include_version":true}),
        );
        if probe["state"] == "connected" {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(input(json!({"kind":"pin"}))["pinned"], true);
    assert_eq!(input(json!({"kind":"reset"}))["pinned"], true);
    assert_eq!(input(json!({"kind":"context_changed"}))["pinned"], false);
    request(
        app,
        json!({"command":"set_draft","session":session,"document":document["document"],"draft":"SELECT 2"}),
    );
    assert_eq!(
        input(json!({"kind":"reset"}))["failure"]["kind"],
        "selection_superseded"
    );
    request(app, json!({"command":"close_session","session":session}));
    assert_eq!(
        input(json!({"kind":"context_changed"}))["failure"]["kind"],
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
fn query_tabs_publish_independent_sessions_and_shutdown_drains_both() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    let first = request(app, json!({"command":"connect","profile":"fixture"}));
    let first_session = &first["session"];
    let first_document = request(
        app,
        json!({"command":"open_document","session":first_session}),
    )["document"]
        .clone();
    assert_eq!(
        request(
            app,
            json!({"command":"submit","session":first_session,"document":first_document,"sql":"wait"})
        )["generation"],
        1
    );
    request(app, json!({"command":"select_session","session":null}));
    let second = request(app, json!({"command":"connect","profile":"fixture"}));
    let second_session = &second["session"];
    let second_document = request(
        app,
        json!({"command":"open_document","session":second_session}),
    )["document"]
        .clone();
    assert_ne!(first_session, second_session);
    assert_ne!(first["tab"], second["tab"]);
    assert_ne!(first_document, second_document);
    assert_eq!(
        request(
            app,
            json!({"command":"submit","session":second_session,"document":second_document,"sql":"rows"})
        )["generation"],
        1
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(std::time::Instant::now() < deadline);
        let snapshot = request(
            app,
            json!({"command":"observe_document","session":second_session,"document":second_document}),
        );
        if snapshot["execution"] == "finished" {
            assert_eq!(snapshot["rows"], 500);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let inventory = request(app, json!({"command":"tabs"}));
    assert_eq!(inventory["tabs"].as_array().unwrap().len(), 2);
    assert_eq!(inventory["selected"], second["tab"]);
    let active = &inventory["tabs"][0]["execution"];
    assert!(active == "running" || active == "planning");
    assert_eq!(
        request(
            app,
            json!({"command":"submit","session":second_session,"document":second_document,"sql":"wait"})
        )["generation"],
        2
    );
    assert_eq!(
        request(app, json!({"command":"shutdown"}))["shutdown"],
        true
    );
    assert!(dzban_release(app));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn shutdown_rejects_a_pending_tab_completion_without_losing_cleanup() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    let bytes = serde_json::to_vec(&json!({"command":"connect","profile":"fixture-slow"})).unwrap();
    let operation = unsafe { dzban_send(app, bytes.as_ptr(), bytes.len()) };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        assert!(std::time::Instant::now() < deadline);
        if !request(app, json!({"command":"connections"}))["connections"]
            .as_array()
            .unwrap()
            .is_empty()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        request(app, json!({"command":"shutdown"}))["shutdown"],
        true
    );
    let buffer = dzban_wait(operation, 0, 5000);
    assert_eq!(buffer.status, 0);
    let record: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(buffer.data, buffer.length) })
            .unwrap();
    assert_eq!(record["payload"]["failure"]["kind"], "closed");
    assert!(dzban_buffer_release(buffer.token));
    assert!(dzban_release(operation));
    assert!(dzban_release(app));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn shared_health_snapshots_track_sessions_and_bulk_profile_checks_without_extra_tabs() {
    let app = dzban_macos_bridge::dzban_create_fixture();
    request(app, json!({"command":"profiles"}));
    let initial = request(app, json!({"command":"observe_health"}));
    assert_eq!(initial["sessions"], json!([]));
    let profile = request(
        app,
        json!({"command":"check_profile_health","profile":"fixture"}),
    );
    assert_eq!(profile["profiles"][0]["status"]["state"], "connected");
    assert_eq!(profile["profiles"][0]["status"]["latency_micros"], 1);
    assert_eq!(profile["sessions"], json!([]));
    let connection = request(app, json!({"command":"connect","profile":"fixture"}));
    let checked = request(
        app,
        json!({"command":"check_health","session":connection["session"]}),
    );
    assert_eq!(checked["sessions"][0]["session"], connection["session"]);
    assert_eq!(checked["sessions"][0]["status"]["version"], "fixture");
    assert_eq!(checked["sessions"][0]["status"]["probing"], false);
    let bulk = request(app, json!({"command":"check_profiles_health"}));
    assert_eq!(bulk["profiles"].as_array().unwrap().len(), 3);
    assert_eq!(bulk["profiles"][1]["profile"], "fixture-failure");
    assert_eq!(bulk["profiles"][1]["status"]["state"], "error");
    assert_eq!(
        request(app, json!({"command":"tabs"}))["tabs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    request(
        app,
        json!({"command":"close_session","session":connection["session"]}),
    );
    let retired = request(app, json!({"command":"observe_health"}));
    assert_eq!(retired["sessions"], json!([]));
    assert!(retired["revision"].as_u64().unwrap() > checked["revision"].as_u64().unwrap());
    assert_eq!(
        request(app, json!({"command":"shutdown"}))["shutdown"],
        true
    );
    assert!(dzban_release(app));
}
