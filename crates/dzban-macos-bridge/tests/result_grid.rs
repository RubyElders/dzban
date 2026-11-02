use dzban_macos_bridge::{dzban_buffer_release, dzban_release, dzban_result_grid};
use serde_json::{Value, json};

fn apply(handle: u64, request: Value) -> Value {
    let bytes = serde_json::to_vec(&request).unwrap();
    let buffer = unsafe { dzban_result_grid(handle, bytes.as_ptr(), bytes.len()) };
    assert_eq!(buffer.status, 0);
    let record: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(buffer.data, buffer.length) })
            .unwrap();
    assert!(dzban_buffer_release(buffer.token));
    record["payload"].clone()
}

#[test]
fn native_grid_uses_shared_ranges_column_order_and_frozen_layout() {
    assert_eq!(
        unsafe { dzban_result_grid(0, std::ptr::null(), 0) }.status,
        7
    );
    let grid = apply(
        0,
        json!({"action":"create","columns":(0..5).map(|index| json!({"name":format!("c{index}"),"type_name":"text"})).collect::<Vec<_>>(),"rows":100}),
    );
    let handle = grid["handle"].as_u64().unwrap();
    assert_eq!(grid["frozen"], json!([0]));
    apply(
        handle,
        json!({"action":"cell","row":3,"column":2,"extend":false}),
    );
    let range = apply(
        handle,
        json!({"action":"move","horizontal":2,"vertical":2,"extend":true}),
    );
    assert_eq!(range["ranges"], json!([{"start":3,"end":6}]));
    assert_eq!(range["columns"], json!([2, 3, 4]));
    let rows = apply(handle, json!({"action":"toggle_rows","visible":[0,3,4]}));
    assert_eq!(rows["mode"], "rows");
    assert_eq!(rows["columns"], json!([0, 1, 2, 3, 4, 5]));
    apply(handle, json!({"action":"freeze","column":4}));
    let right = apply(
        handle,
        json!({"action":"layout","widths":[50,50,50,50,50,50],"scroll":0,"viewport":150,"column":4}),
    );
    assert_eq!(right["right"], json!([4]));
    assert_eq!(right["reveal"], 0.0);
    let left = apply(
        handle,
        json!({"action":"layout","widths":[50,50,50,50,50,50],"scroll":240,"viewport":150}),
    );
    assert_eq!(left["left"], json!([0, 4]));
    apply(
        handle,
        json!({"action":"column","column":2,"gesture":"replace"}),
    );
    let grown = apply(handle, json!({"action":"resize","rows":110}));
    assert_eq!(grown["ranges"], json!([{"start":0,"end":110}]));
    assert_eq!(grown["columns"], json!([2]));
    let clone = apply(handle, json!({"action":"clone"}));
    let cloned_handle = clone["handle"].as_u64().unwrap();
    assert_ne!(cloned_handle, handle);
    apply(
        cloned_handle,
        json!({"action":"cell","row":0,"column":1,"extend":false}),
    );
    assert_eq!(
        apply(handle, json!({"action":"resize","rows":110}))["mode"],
        "columns"
    );
    assert!(dzban_release(cloned_handle));
    assert!(dzban_release(handle));
    assert!(!dzban_release(handle));
    let bytes = br#"{"action":"all"}"#;
    assert_ne!(
        unsafe { dzban_result_grid(handle, bytes.as_ptr(), bytes.len()) }.status,
        0
    );
}
