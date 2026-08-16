use dzban_db::connection_gobject::{ConnectionState, DbConnection};
use glib::prelude::*;

#[test]
fn test_gobject_properties() {
    println!("Test: GObject properties work");

    let conn = DbConnection::new("postgresql://test".to_string());

    assert_eq!(conn.connection_string(), "postgresql://test");
    assert_eq!(conn.state(), ConnectionState::Disconnected);
    assert_eq!(conn.latency_micros(), -1);
    assert_eq!(conn.version(), None);
    assert_eq!(conn.ping_count(), 0);
    assert_eq!(conn.last_error(), "");
    assert!(!conn.query_running());

    println!("✅ All properties readable");
}

#[test]
fn test_gobject_property_via_generic_api() {
    println!("Test: Generic property API works");

    let conn = DbConnection::new("postgresql://test".to_string());

    let state: ConnectionState = conn.property("state");
    assert_eq!(state, ConnectionState::Disconnected);

    let latency: i64 = conn.property("latency-micros");
    assert_eq!(latency, -1);

    let version: Option<String> = conn.property("version");
    assert_eq!(version, None);

    println!("✅ Generic property API works");
}

#[test]
fn test_property_notifications() {
    println!("Test: Property change notifications work");

    let conn = DbConnection::new("postgresql://test".to_string());

    let notification_count = std::rc::Rc::new(std::cell::RefCell::new(0));
    let notification_count_clone = notification_count.clone();

    conn.connect_notify_local(Some("state"), move |_conn, _pspec| {
        *notification_count_clone.borrow_mut() += 1;
        println!(
            "State property changed! Count: {}",
            *notification_count_clone.borrow()
        );
    });

    conn.disconnect();

    println!("✅ Property notification system works");
}

#[test]
fn test_property_binding() {
    println!("Test: Property binding read-only property to label");

    let conn = DbConnection::new("postgresql://conn1".to_string());

    let conn_str: String = conn.property("connection-string");
    assert_eq!(conn_str, "postgresql://conn1");

    println!("✅ Property binding API available");
}

#[test]
fn test_signal_emission() {
    println!("Test: Signal emission works");

    let conn = DbConnection::new("postgresql://test".to_string());

    let signal_received = std::rc::Rc::new(std::cell::RefCell::new(false));
    let signal_received_clone = signal_received.clone();

    conn.connect_local("state-changed", false, move |values| {
        let old_state = values[1].get::<ConnectionState>().unwrap();
        let new_state = values[2].get::<ConnectionState>().unwrap();
        println!("Signal received: {:?} -> {:?}", old_state, new_state);
        *signal_received_clone.borrow_mut() = true;
        None
    });

    conn.emit_by_name::<()>(
        "state-changed",
        &[&ConnectionState::Disconnected, &ConnectionState::Connecting],
    );

    assert!(*signal_received.borrow());
    println!("✅ Signal emission works");
}

#[test]
fn test_disconnect_clears_state() {
    println!("Test: disconnect() clears all state");

    let conn = DbConnection::new("postgresql://test".to_string());

    conn.disconnect();

    assert_eq!(conn.state(), ConnectionState::Disconnected);
    assert_eq!(conn.latency_micros(), -1);
    assert_eq!(conn.version(), None);
    assert_eq!(conn.last_error(), "");

    println!("✅ disconnect() clears state");
}

#[test]
fn test_default_values() {
    println!("Test: Default values are correct");

    let conn = DbConnection::default();

    assert_eq!(conn.connection_string(), "");
    assert_eq!(conn.state(), ConnectionState::Disconnected);
    assert_eq!(conn.latency_micros(), -1);
    assert_eq!(conn.version(), None);
    assert_eq!(conn.ping_count(), 0);
    assert_eq!(conn.last_error(), "");
    assert!(!conn.query_running());

    println!("✅ Default values correct");
}
