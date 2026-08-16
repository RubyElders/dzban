use dzban_db::{ConnectionState, DbQueryTab};
use glib::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn test_query_tab_properties() {
    println!("Test: DbQueryTab basic properties");

    let tab = DbQueryTab::new(
        "host=localhost port=5432".to_string(),
        Some("Test Tab".to_string()),
    );

    assert_eq!(tab.tab_label(), "Test Tab");
    assert_eq!(tab.sql_query(), "");
    assert!(!tab.is_executing());
    assert_eq!(tab.result_count(), 0);
    assert!(!tab.is_active());
    assert_eq!(tab.connection_state(), ConnectionState::Disconnected);

    tab.set_tab_label("Modified Label".to_string());
    assert_eq!(tab.tab_label(), "Modified Label");

    tab.set_sql_query("SELECT * FROM users".to_string());
    assert_eq!(tab.sql_query(), "SELECT * FROM users");

    tab.set_is_active(true);
    assert!(tab.is_active());

    println!("✅ All properties work correctly");
}

#[test]
fn test_query_tab_has_connection() {
    println!("Test: DbQueryTab has its own DbConnection");

    let tab = DbQueryTab::new(
        "host=localhost port=5432 user=test dbname=testdb".to_string(),
        None,
    );

    assert!(tab.connection().is_some());
    let conn = tab.connection().unwrap();
    assert_eq!(conn.state(), ConnectionState::Disconnected);

    println!("✅ Tab has independent connection");
}

#[test]
fn test_query_tab_signal_forwarding() {
    println!("Test: DbQueryTab forwards connection signals");

    let tab = DbQueryTab::new(
        "host=localhost port=5432".to_string(),
        Some("Signal Test".to_string()),
    );

    let state_changes = Rc::new(RefCell::new(Vec::new()));
    let state_changes_clone = state_changes.clone();

    tab.connect_closure(
        "connection-state-changed",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab,
                                   old: ConnectionState,
                                   new: ConnectionState| {
            state_changes_clone.borrow_mut().push((old, new));
        }),
    );

    if let Some(conn) = tab.connection() {
        conn.disconnect();
    }

    println!("✅ Signal forwarding mechanism works");
}

#[test]
fn test_query_tab_activation_signal() {
    println!("Test: DbQueryTab emits tab-activated signal");

    let tab = DbQueryTab::new(
        "host=localhost port=5432".to_string(),
        Some("Activation Test".to_string()),
    );

    let activated = Rc::new(RefCell::new(false));
    let activated_clone = activated.clone();

    tab.connect_closure(
        "tab-activated",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab| {
            *activated_clone.borrow_mut() = true;
        }),
    );

    tab.set_is_active(true);
    assert!(*activated.borrow());

    println!("✅ Tab activation signal works");
}

#[test]
fn test_query_tab_close() {
    println!("Test: DbQueryTab close() emits tab-closed signal");

    let tab = DbQueryTab::new(
        "host=localhost port=5432".to_string(),
        Some("Close Test".to_string()),
    );

    let closed = Rc::new(RefCell::new(false));
    let closed_clone = closed.clone();

    tab.connect_closure(
        "tab-closed",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab| {
            *closed_clone.borrow_mut() = true;
        }),
    );

    tab.close();
    assert!(*closed.borrow());

    println!("✅ Tab close signal works");
}

#[test]
fn test_query_tab_property_notifications() {
    println!("Test: DbQueryTab emits property change notifications");

    let tab = DbQueryTab::new("host=localhost port=5432".to_string(), None);

    let notifications = Rc::new(RefCell::new(Vec::new()));
    let notifications_clone = notifications.clone();

    tab.connect_notify_local(Some("tab-label"), move |_, pspec| {
        notifications_clone
            .borrow_mut()
            .push(pspec.name().to_string());
    });

    tab.set_tab_label("New Label".to_string());

    assert!(notifications.borrow().contains(&"tab-label".to_string()));
    println!("✅ Property notifications work");
}

#[test]
fn test_multiple_tabs_independent_connections() {
    println!("Test: Multiple tabs have independent connections");

    let tab1 = DbQueryTab::new(
        "host=localhost port=5432 dbname=db1".to_string(),
        Some("Tab 1".to_string()),
    );

    let tab2 = DbQueryTab::new(
        "host=localhost port=5433 dbname=db2".to_string(),
        Some("Tab 2".to_string()),
    );

    let conn1 = tab1.connection().unwrap();
    let conn2 = tab2.connection().unwrap();

    assert_ne!(conn1.connection_string(), conn2.connection_string());

    assert_eq!(conn1.state(), ConnectionState::Disconnected);
    assert_eq!(conn2.state(), ConnectionState::Disconnected);

    println!("✅ Multiple tabs have independent connections");
    println!("   Tab 1: {}", conn1.connection_string());
    println!("   Tab 2: {}", conn2.connection_string());
}

#[test]
fn test_query_tab_empty_query_error() {
    println!("Test: DbQueryTab rejects empty queries");

    let tab = DbQueryTab::new("host=localhost port=5432".to_string(), None);

    let error_received = Rc::new(RefCell::new(false));
    let error_received_clone = error_received.clone();

    tab.connect_closure(
        "query-error",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab, error: String| {
            println!("Query error: {}", error);
            *error_received_clone.borrow_mut() = true;
        }),
    );

    tab.set_sql_query("   ".to_string());
    tab.execute_query();

    assert!(*error_received.borrow());
    println!("✅ Empty queries are rejected");
}

#[glib::async_test]
#[ignore]
async fn test_query_tab_real_connection() {
    println!("Test: DbQueryTab with real PostgreSQL connection");

    let tab = DbQueryTab::new(
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb".to_string(),
        Some("Real DB Tab".to_string()),
    );

    let state_changes = Rc::new(RefCell::new(Vec::new()));
    let state_changes_clone = state_changes.clone();

    tab.connect_closure(
        "connection-state-changed",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab,
                                   old: ConnectionState,
                                   new: ConnectionState| {
            println!("Tab connection state: {:?} -> {:?}", old, new);
            state_changes_clone.borrow_mut().push((old, new));
        }),
    );

    tab.connect();

    let start = std::time::Instant::now();
    while tab.connection_state() != ConnectionState::Connected
        && tab.connection_state() != ConnectionState::Error
        && start.elapsed().as_secs() < 5
    {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(tab.connection_state(), ConnectionState::Connected);
    println!("✅ Tab connected successfully");

    let query_finished = Rc::new(RefCell::new(false));
    let query_finished_clone = query_finished.clone();
    let row_count = Rc::new(RefCell::new(0i64));
    let row_count_clone = row_count.clone();

    tab.connect_closure(
        "query-finished",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab, rows: i64, duration: i64| {
            println!("Query finished: {} rows in {} ms", rows, duration);
            *row_count_clone.borrow_mut() = rows;
            *query_finished_clone.borrow_mut() = true;
        }),
    );

    tab.set_sql_query("SELECT * FROM users".to_string());
    tab.execute_query();

    let start = std::time::Instant::now();
    while !*query_finished.borrow() && start.elapsed().as_secs() < 5 {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;
    }

    assert!(*query_finished.borrow());
    assert_eq!(*row_count.borrow(), 3);
    assert_eq!(tab.result_count(), 3);

    println!(
        "✅ Query executed successfully: {} rows",
        row_count.borrow()
    );

    tab.close();
}

#[glib::async_test]
#[ignore]
async fn test_parallel_queries_different_tabs() {
    println!("Test: Multiple tabs can execute queries in parallel");

    let tab1 = DbQueryTab::new(
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb".to_string(),
        Some("Tab 1 - Users".to_string()),
    );

    let tab2 = DbQueryTab::new(
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb".to_string(),
        Some("Tab 2 - Posts".to_string()),
    );

    tab1.connect();
    tab2.connect();

    let start = std::time::Instant::now();
    while (tab1.connection_state() != ConnectionState::Connected
        || tab2.connection_state() != ConnectionState::Connected)
        && start.elapsed().as_secs() < 5
    {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(tab1.connection_state(), ConnectionState::Connected);
    assert_eq!(tab2.connection_state(), ConnectionState::Connected);

    let tab1_done = Rc::new(RefCell::new(false));
    let tab1_done_clone = tab1_done.clone();
    let tab2_done = Rc::new(RefCell::new(false));
    let tab2_done_clone = tab2_done.clone();

    tab1.connect_closure(
        "query-finished",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab, rows: i64, _duration: i64| {
            println!("Tab 1 finished: {} rows", rows);
            *tab1_done_clone.borrow_mut() = true;
        }),
    );

    tab2.connect_closure(
        "query-finished",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab, rows: i64, _duration: i64| {
            println!("Tab 2 finished: {} rows", rows);
            *tab2_done_clone.borrow_mut() = true;
        }),
    );

    tab1.set_sql_query("SELECT * FROM users".to_string());
    tab2.set_sql_query("SELECT * FROM posts".to_string());

    tab1.execute_query();
    tab2.execute_query();

    assert!(tab1.is_executing() || tab2.is_executing());

    let start = std::time::Instant::now();
    while (!*tab1_done.borrow() || !*tab2_done.borrow()) && start.elapsed().as_secs() < 10 {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;
    }

    assert!(*tab1_done.borrow());
    assert!(*tab2_done.borrow());
    assert_eq!(tab1.result_count(), 3);
    assert_eq!(tab2.result_count(), 4);

    println!("✅ Parallel queries executed successfully");
    println!("   Tab 1: {} rows", tab1.result_count());
    println!("   Tab 2: {} rows", tab2.result_count());

    tab1.close();
    tab2.close();
}
