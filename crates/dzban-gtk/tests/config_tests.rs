use dzban_gtk::database::{ConnectionState, DbConfig, DbQueryTab};
use glib::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn test_dbconfig_properties() {
    println!("Test: DbConfig basic properties");

    let config = DbConfig::new(
        "Production".to_string(),
        "host=localhost port=5432 user=admin dbname=mydb".to_string(),
    );

    assert_eq!(config.config_name(), "Production");
    assert_eq!(
        config.connection_string(),
        "host=localhost port=5432 user=admin dbname=mydb"
    );
    assert_eq!(config.tab_count(), 0);
    assert_eq!(config.active_tab_index(), -1);

    println!("✅ All properties work correctly");
}

#[test]
fn test_dbconfig_add_tab() {
    println!("Test: DbConfig can add tabs");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    let added_tabs = Rc::new(RefCell::new(Vec::new()));
    let added_tabs_clone = added_tabs.clone();

    config.connect_closure(
        "tab-added",
        false,
        glib::closure_local!(move |_config: &DbConfig, index: i32, _tab: DbQueryTab| {
            added_tabs_clone.borrow_mut().push(index);
        }),
    );

    let index1 = config.add_tab(Some("Query 1".to_string()));
    assert_eq!(index1, 0);
    assert_eq!(config.tab_count(), 1);
    assert_eq!(config.active_tab_index(), 0);

    let index2 = config.add_tab(None);
    assert_eq!(index2, 1);
    assert_eq!(config.tab_count(), 2);

    assert_eq!(*added_tabs.borrow(), vec![0, 1]);

    assert!(config.get_tab(0).is_some());
    assert!(config.get_tab(1).is_some());
    assert!(config.get_tab(2).is_none());

    println!("✅ Adding tabs works correctly");
}

#[test]
fn test_dbconfig_remove_tab() {
    println!("Test: DbConfig can remove tabs");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));
    config.add_tab(Some("Tab 3".to_string()));
    assert_eq!(config.tab_count(), 3);

    let removed_tabs = Rc::new(RefCell::new(Vec::new()));
    let removed_tabs_clone = removed_tabs.clone();

    config.connect_closure(
        "tab-removed",
        false,
        glib::closure_local!(move |_config: &DbConfig, index: i32| {
            removed_tabs_clone.borrow_mut().push(index);
        }),
    );

    assert!(config.remove_tab(1));
    assert_eq!(config.tab_count(), 2);
    assert_eq!(*removed_tabs.borrow(), vec![1]);
    assert_eq!(config.get_tab(0).unwrap().tab_label(), "Tab 1");
    assert_eq!(config.get_tab(1).unwrap().tab_label(), "Tab 3");

    let new_index = config.add_tab(Some("Tab 4".to_string()));
    assert_eq!(new_index, 2);
    assert_eq!(config.get_all_tabs().len(), 3);

    assert!(!config.remove_tab(5));
    assert_eq!(config.tab_count(), 3);

    println!("✅ Removing tabs works correctly");
}

#[test]
fn test_dbconfig_switch_tabs() {
    println!("Test: DbConfig can switch between tabs");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));
    config.add_tab(Some("Tab 3".to_string()));

    assert_eq!(config.active_tab_index(), 0);

    let switched_to = Rc::new(RefCell::new(Vec::new()));
    let switched_to_clone = switched_to.clone();

    config.connect_closure(
        "tab-switched",
        false,
        glib::closure_local!(move |_config: &DbConfig, index: i32| {
            switched_to_clone.borrow_mut().push(index);
        }),
    );

    assert!(config.switch_to_tab(2));
    assert_eq!(config.active_tab_index(), 2);

    let tab0 = config.get_tab(0).unwrap();
    let tab1 = config.get_tab(1).unwrap();
    let tab2 = config.get_tab(2).unwrap();

    assert!(!tab0.is_active());
    assert!(!tab1.is_active());
    assert!(tab2.is_active());

    assert!(!config.switch_to_tab(5));
    assert_eq!(config.active_tab_index(), 2);

    assert_eq!(*switched_to.borrow(), vec![2]);

    println!("✅ Tab switching works correctly");
}

#[test]
fn test_dbconfig_active_tab() {
    println!("Test: DbConfig active_tab() returns correct tab");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    assert!(config.active_tab().is_none());

    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));

    let active = config.active_tab().unwrap();
    assert_eq!(active.tab_label(), "Tab 1");

    config.switch_to_tab(1);
    let active = config.active_tab().unwrap();
    assert_eq!(active.tab_label(), "Tab 2");

    println!("✅ active_tab() works correctly");
}

#[test]
fn test_dbconfig_get_all_tabs() {
    println!("Test: DbConfig get_all_tabs() returns all tabs");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    assert_eq!(config.get_all_tabs().len(), 0);

    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));
    config.add_tab(Some("Tab 3".to_string()));

    let all_tabs = config.get_all_tabs();
    assert_eq!(all_tabs.len(), 3);
    assert_eq!(all_tabs[0].tab_label(), "Tab 1");
    assert_eq!(all_tabs[1].tab_label(), "Tab 2");
    assert_eq!(all_tabs[2].tab_label(), "Tab 3");

    println!("✅ get_all_tabs() works correctly");
}

#[glib::async_test]
async fn test_dbconfig_signal_forwarding() {
    println!("Test: DbConfig forwards query signals from tabs");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    let query_started = Rc::new(RefCell::new(Vec::new()));
    let query_started_clone = query_started.clone();
    let query_error = Rc::new(RefCell::new(Vec::new()));
    let query_error_clone = query_error.clone();

    config.connect_closure(
        "query-started",
        false,
        glib::closure_local!(move |_config: &DbConfig, query: String| {
            println!("Config received query-started: {}", query);
            query_started_clone.borrow_mut().push(query);
        }),
    );

    config.connect_closure(
        "query-error",
        false,
        glib::closure_local!(move |_config: &DbConfig, error: String| {
            println!("Config received query-error: {}", error);
            query_error_clone.borrow_mut().push(error);
        }),
    );

    config.add_tab(Some("Query Tab".to_string()));

    let tab = config.get_tab(0).unwrap();
    tab.set_sql_query("SELECT * FROM users".to_string());
    tab.execute_query();

    if !query_started.borrow().is_empty() {
        assert_eq!(query_started.borrow()[0], "SELECT * FROM users");
        println!("✅ Signal forwarding works correctly (query-started)");
    } else if !query_error.borrow().is_empty() {
        println!("✅ Signal forwarding works correctly (query-error received as expected)");
    } else {
        panic!("Neither query-started nor query-error was forwarded!");
    }
}

#[test]
fn test_dbconfig_remove_active_tab() {
    println!("Test: DbConfig handles removing active tab");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));
    config.add_tab(Some("Tab 3".to_string()));

    config.switch_to_tab(1);
    assert_eq!(config.active_tab_index(), 1);

    config.remove_tab(1);

    assert_eq!(config.active_tab_index(), 0);
    assert_eq!(config.tab_count(), 2);

    println!("✅ Removing active tab handled correctly");
}

#[test]
fn test_dbconfig_remove_first_active_tab() {
    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));
    assert!(config.remove_tab(0));

    assert_eq!(config.active_tab_index(), 0);
    assert_eq!(config.active_tab().unwrap().tab_label(), "Tab 2");
}

#[test]
fn test_dbconfig_property_notifications() {
    println!("Test: DbConfig emits property change notifications");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    let notifications = Rc::new(RefCell::new(Vec::new()));
    let notifications_clone = notifications.clone();

    config.connect_notify_local(Some("tab-count"), move |_, pspec| {
        notifications_clone
            .borrow_mut()
            .push(pspec.name().to_string());
    });

    config.add_tab(None);
    config.add_tab(None);

    assert_eq!(notifications.borrow().len(), 2);
    assert!(notifications.borrow().iter().all(|n| n == "tab-count"));

    println!("✅ Property notifications work");
}

#[test]
fn test_dbconfig_close() {
    println!("Test: DbConfig close() cleans up all tabs");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5432".to_string(),
    );

    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));
    config.add_tab(Some("Tab 3".to_string()));

    assert_eq!(config.tab_count(), 3);

    let closed = Rc::new(RefCell::new(false));
    let closed_clone = closed.clone();

    config.connect_closure(
        "config-closed",
        false,
        glib::closure_local!(move |_config: &DbConfig| {
            *closed_clone.borrow_mut() = true;
        }),
    );

    config.close();

    assert_eq!(config.tab_count(), 0);
    assert_eq!(config.active_tab_index(), -1);
    assert!(*closed.borrow());
    assert_eq!(config.get_all_tabs().len(), 0);

    println!("✅ close() works correctly");
}

#[test]
fn test_dbconfig_tabs_have_correct_connection_string() {
    println!("Test: DbConfig tabs inherit connection string");

    let connection_string = "host=localhost port=5432 user=testuser dbname=testdb".to_string();
    let config = DbConfig::new("Test Config".to_string(), connection_string.clone());

    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));

    let tab1 = config.get_tab(0).unwrap();
    let tab2 = config.get_tab(1).unwrap();

    assert!(tab1.connection().is_some());
    assert!(tab2.connection().is_some());

    let conn1 = tab1.connection().unwrap();
    let conn2 = tab2.connection().unwrap();

    assert_eq!(conn1.connection_string(), connection_string);
    assert_eq!(conn2.connection_string(), connection_string);
    assert_eq!(conn1, config.connection());
    assert_eq!(conn2, config.connection());

    println!("✅ Tabs have correct connection string");
}

#[glib::async_test]
async fn test_dbconfig_query_signals_stay_with_the_executing_tab() {
    let config = DbConfig::new("Test Config".to_string(), "host=localhost".to_string());
    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));

    let tab1 = config.get_tab(0).unwrap();
    let tab2 = config.get_tab(1).unwrap();
    let tab1_errors = Rc::new(RefCell::new(0));
    let tab2_errors = Rc::new(RefCell::new(0));

    let errors = tab1_errors.clone();
    tab1.connect_closure(
        "query-error",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab, _error: String| {
            *errors.borrow_mut() += 1;
        }),
    );
    let errors = tab2_errors.clone();
    tab2.connect_closure(
        "query-error",
        false,
        glib::closure_local!(move |_tab: &DbQueryTab, _error: String| {
            *errors.borrow_mut() += 1;
        }),
    );

    tab1.set_sql_query("SELECT 1".to_string());
    tab1.execute_query();

    assert_eq!(*tab1_errors.borrow(), 1);
    assert_eq!(*tab2_errors.borrow(), 0);
}

#[test]
fn test_dbconfig_close_releases_the_object_graph() {
    let config = DbConfig::new("Test Config".to_string(), "host=localhost".to_string());
    config.add_tab(None);

    let tab = config.get_tab(0).unwrap();
    let connection = config.connection();
    let weak_config = config.downgrade();
    let weak_tab = tab.downgrade();
    let weak_connection = connection.downgrade();

    config.close();
    drop(tab);
    drop(connection);
    drop(config);

    assert!(weak_config.upgrade().is_none());
    assert!(weak_tab.upgrade().is_none());
    assert!(weak_connection.upgrade().is_none());
}

#[glib::async_test]
#[ignore = "requires the PostgreSQL service from docker-compose.test.yml"]
async fn test_dbconfig_connect_all_tabs() {
    println!("Test: DbConfig can connect all tabs to database");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb".to_string(),
    );

    config.add_tab(Some("Tab 1".to_string()));
    config.add_tab(Some("Tab 2".to_string()));
    config.add_tab(Some("Tab 3".to_string()));

    let state_changes = Rc::new(RefCell::new(Vec::new()));
    let state_changes_clone = state_changes.clone();

    config.connect_closure(
        "connection-state-changed",
        false,
        glib::closure_local!(move |_config: &DbConfig,
                                   _old: ConnectionState,
                                   new: ConnectionState| {
            state_changes_clone.borrow_mut().push(new);
        }),
    );

    config.connect_all();

    let start = std::time::Instant::now();
    while !config.all_connected() && start.elapsed().as_secs() < 10 {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        config.all_connected(),
        "connection errors: {:?}",
        (0..config.tab_count())
            .filter_map(|index| config.get_tab(index)?.connection())
            .map(|connection| connection.last_error())
            .collect::<Vec<_>>()
    );

    for i in 0..config.tab_count() {
        let tab = config.get_tab(i).unwrap();
        assert_eq!(tab.connection_state(), ConnectionState::Connected);
    }

    println!("✅ All tabs connected successfully");

    config.disconnect_all();
    config.close();
}

#[glib::async_test]
#[ignore = "requires the PostgreSQL service from docker-compose.test.yml"]
async fn test_dbconfig_rejects_overlapping_tab_queries() {
    println!("Test: DbConfig rejects concurrent queries on its shared connection");

    let config = DbConfig::new(
        "Test Config".to_string(),
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb".to_string(),
    );

    config.add_tab(Some("Users Query".to_string()));
    config.add_tab(Some("Posts Query".to_string()));

    config.connect_all();

    let start = std::time::Instant::now();
    while !config.all_connected() && start.elapsed().as_secs() < 5 {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        config.all_connected(),
        "connection errors: {:?}",
        (0..config.tab_count())
            .filter_map(|index| config.get_tab(index)?.connection())
            .map(|connection| connection.last_error())
            .collect::<Vec<_>>()
    );

    let query_count = Rc::new(RefCell::new(0));
    let query_count_clone = query_count.clone();
    let error_count = Rc::new(RefCell::new(0));
    let error_count_clone = error_count.clone();

    config.connect_closure(
        "query-finished",
        false,
        glib::closure_local!(move |_config: &DbConfig, _rows: i64, _duration: i64| {
            *query_count_clone.borrow_mut() += 1;
        }),
    );
    config.connect_closure(
        "query-error",
        false,
        glib::closure_local!(move |_config: &DbConfig, error: String| {
            assert_eq!(error, "Connection is busy");
            *error_count_clone.borrow_mut() += 1;
        }),
    );

    let tab1 = config.get_tab(0).unwrap();
    let tab2 = config.get_tab(1).unwrap();

    tab1.set_sql_query("SELECT * FROM users".to_string());
    tab2.set_sql_query("SELECT * FROM posts".to_string());

    tab1.execute_query();
    tab2.execute_query();

    let start = std::time::Instant::now();
    while (*query_count.borrow() < 1 || *error_count.borrow() < 1) && start.elapsed().as_secs() < 10
    {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(*query_count.borrow(), 1);
    assert_eq!(*error_count.borrow(), 1);
    assert_eq!(tab1.result_count(), 3);
    assert_eq!(tab2.result_count(), 0);

    config.disconnect_all();
    config.close();
}
