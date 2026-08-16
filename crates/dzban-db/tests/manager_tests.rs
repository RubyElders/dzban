use dzban_db::{ConnectionState, DbConfig, DbManager, DbQueryTab};
use glib::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn test_dbmanager_properties() {
    println!("Test: DbManager basic properties");

    let manager = DbManager::new();

    assert_eq!(manager.config_count(), 0);
    assert_eq!(manager.active_config_name(), "");

    println!("✅ All properties work correctly");
}

#[test]
fn test_dbmanager_add_config() {
    println!("Test: DbManager can add configurations");

    let manager = DbManager::new();

    let added_configs = Rc::new(RefCell::new(Vec::new()));
    let added_configs_clone = added_configs.clone();

    manager.connect_closure(
        "config-added",
        false,
        glib::closure_local!(
            move |_manager: &DbManager, name: String, _config: DbConfig| {
                added_configs_clone.borrow_mut().push(name);
            }
        ),
    );

    assert!(manager.add_config(
        "Production".to_string(),
        "host=prod.example.com port=5432".to_string()
    ));

    assert_eq!(manager.config_count(), 1);
    assert_eq!(manager.active_config_name(), "Production");

    assert!(manager.add_config(
        "Staging".to_string(),
        "host=staging.example.com port=5432".to_string()
    ));

    assert_eq!(manager.config_count(), 2);

    assert!(!manager.add_config(
        "Production".to_string(),
        "host=other.example.com port=5432".to_string()
    ));

    assert_eq!(manager.config_count(), 2);

    assert_eq!(*added_configs.borrow(), vec!["Production", "Staging"]);

    println!("✅ Adding configs works correctly");
}

#[test]
fn test_dbmanager_remove_config() {
    println!("Test: DbManager can remove configurations");

    let manager = DbManager::new();

    manager.add_config(
        "Config1".to_string(),
        "host=localhost port=5432".to_string(),
    );
    manager.add_config(
        "Config2".to_string(),
        "host=localhost port=5433".to_string(),
    );
    manager.add_config(
        "Config3".to_string(),
        "host=localhost port=5434".to_string(),
    );

    assert_eq!(manager.config_count(), 3);

    let removed_configs = Rc::new(RefCell::new(Vec::new()));
    let removed_configs_clone = removed_configs.clone();

    manager.connect_closure(
        "config-removed",
        false,
        glib::closure_local!(move |_manager: &DbManager, name: String| {
            removed_configs_clone.borrow_mut().push(name);
        }),
    );

    assert!(manager.remove_config("Config2"));
    assert_eq!(manager.config_count(), 2);

    assert!(!manager.remove_config("NonExistent"));
    assert_eq!(manager.config_count(), 2);

    assert_eq!(*removed_configs.borrow(), vec!["Config2"]);

    println!("✅ Removing configs works correctly");
}

#[test]
fn test_dbmanager_remove_last_active_config() {
    let manager = DbManager::new();
    assert!(manager.add_config("Only".to_string(), "host=localhost port=5432".to_string()));

    assert!(manager.remove_config("Only"));
    assert_eq!(manager.active_config_name(), "");
    assert!(manager.active_config().is_none());
}

#[test]
fn test_dbmanager_switch_config() {
    println!("Test: DbManager can switch between configurations");

    let manager = DbManager::new();

    manager.add_config(
        "Config1".to_string(),
        "host=localhost port=5432".to_string(),
    );
    manager.add_config(
        "Config2".to_string(),
        "host=localhost port=5433".to_string(),
    );

    assert_eq!(manager.active_config_name(), "Config1");

    let activated_configs = Rc::new(RefCell::new(Vec::new()));
    let activated_configs_clone = activated_configs.clone();

    manager.connect_closure(
        "config-activated",
        false,
        glib::closure_local!(move |_manager: &DbManager, name: String| {
            activated_configs_clone.borrow_mut().push(name);
        }),
    );

    assert!(manager.switch_to_config("Config2"));
    assert_eq!(manager.active_config_name(), "Config2");

    assert!(!manager.switch_to_config("NonExistent"));
    assert_eq!(manager.active_config_name(), "Config2");

    assert_eq!(*activated_configs.borrow(), vec!["Config2"]);

    println!("✅ Config switching works correctly");
}

#[test]
fn test_dbmanager_active_config() {
    println!("Test: DbManager active_config() returns correct config");

    let manager = DbManager::new();

    assert!(manager.active_config().is_none());

    manager.add_config("Production".to_string(), "host=prod port=5432".to_string());
    manager.add_config("Staging".to_string(), "host=staging port=5432".to_string());

    let active = manager.active_config().unwrap();
    assert_eq!(active.config_name(), "Production");

    manager.switch_to_config("Staging");
    let active = manager.active_config().unwrap();
    assert_eq!(active.config_name(), "Staging");

    println!("✅ active_config() works correctly");
}

#[test]
fn test_dbmanager_get_all_configs() {
    println!("Test: DbManager get_all_config_names() and get_all_configs()");

    let manager = DbManager::new();

    assert_eq!(manager.get_all_config_names().len(), 0);
    assert_eq!(manager.get_all_configs().len(), 0);

    manager.add_config(
        "Config1".to_string(),
        "host=localhost port=5432".to_string(),
    );
    manager.add_config(
        "Config2".to_string(),
        "host=localhost port=5433".to_string(),
    );
    manager.add_config(
        "Config3".to_string(),
        "host=localhost port=5434".to_string(),
    );

    let names = manager.get_all_config_names();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"Config1".to_string()));
    assert!(names.contains(&"Config2".to_string()));
    assert!(names.contains(&"Config3".to_string()));

    let configs = manager.get_all_configs();
    assert_eq!(configs.len(), 3);

    println!("✅ get_all methods work correctly");
}

#[test]
fn test_dbmanager_has_config() {
    println!("Test: DbManager has_config() checks existence");

    let manager = DbManager::new();

    assert!(!manager.has_config("Production"));

    manager.add_config("Production".to_string(), "host=prod port=5432".to_string());

    assert!(manager.has_config("Production"));
    assert!(!manager.has_config("Staging"));

    println!("✅ has_config() works correctly");
}

#[test]
fn test_dbmanager_signal_forwarding() {
    println!("Test: DbManager forwards signals from configs/tabs");

    let manager = DbManager::new();

    let tab_added = Rc::new(RefCell::new(Vec::new()));
    let tab_added_clone = tab_added.clone();

    manager.connect_closure(
        "tab-added",
        false,
        glib::closure_local!(move |_manager: &DbManager, index: i32, _tab: DbQueryTab| {
            tab_added_clone.borrow_mut().push(index);
        }),
    );

    manager.add_config("Test".to_string(), "host=localhost port=5432".to_string());

    let config = manager.get_config("Test").unwrap();
    config.add_tab(Some("Tab 1".to_string()));

    assert_eq!(*tab_added.borrow(), vec![0]);

    println!("✅ Signal forwarding works correctly");
}

#[test]
fn test_dbmanager_remove_active_config() {
    println!("Test: DbManager handles removing active config");

    let manager = DbManager::new();

    manager.add_config(
        "Config1".to_string(),
        "host=localhost port=5432".to_string(),
    );
    manager.add_config(
        "Config2".to_string(),
        "host=localhost port=5433".to_string(),
    );
    manager.add_config(
        "Config3".to_string(),
        "host=localhost port=5434".to_string(),
    );

    manager.switch_to_config("Config2");
    assert_eq!(manager.active_config_name(), "Config2");

    manager.remove_config("Config2");

    assert_eq!(manager.config_count(), 2);
    let active = manager.active_config_name();
    assert!(active == "Config1" || active == "Config3");

    println!("✅ Removing active config handled correctly");
}

#[test]
fn test_dbmanager_close_all() {
    println!("Test: DbManager close_all() cleans up everything");

    let manager = DbManager::new();

    manager.add_config(
        "Config1".to_string(),
        "host=localhost port=5432".to_string(),
    );
    manager.add_config(
        "Config2".to_string(),
        "host=localhost port=5433".to_string(),
    );

    let config1 = manager.get_config("Config1").unwrap();
    config1.add_tab(Some("Tab 1".to_string()));
    config1.add_tab(Some("Tab 2".to_string()));

    assert_eq!(manager.config_count(), 2);

    let closed = Rc::new(RefCell::new(false));
    let closed_clone = closed.clone();

    manager.connect_closure(
        "all-closed",
        false,
        glib::closure_local!(move |_manager: &DbManager| {
            *closed_clone.borrow_mut() = true;
        }),
    );

    manager.close_all();

    assert_eq!(manager.config_count(), 0);
    assert_eq!(manager.active_config_name(), "");
    assert_eq!(manager.get_all_configs().len(), 0);
    assert!(*closed.borrow());

    println!("✅ close_all() works correctly");
}

#[test]
fn test_dbmanager_property_notifications() {
    println!("Test: DbManager emits property change notifications");

    let manager = DbManager::new();

    let notifications = Rc::new(RefCell::new(Vec::new()));
    let notifications_clone = notifications.clone();

    manager.connect_notify_local(Some("config-count"), move |_, pspec| {
        notifications_clone
            .borrow_mut()
            .push(pspec.name().to_string());
    });

    manager.add_config(
        "Config1".to_string(),
        "host=localhost port=5432".to_string(),
    );
    manager.add_config(
        "Config2".to_string(),
        "host=localhost port=5433".to_string(),
    );

    assert_eq!(notifications.borrow().len(), 2);
    assert!(notifications.borrow().iter().all(|n| n == "config-count"));

    println!("✅ Property notifications work");
}

#[test]
fn test_dbmanager_hierarchy() {
    println!("Test: DbManager -> DbConfig -> DbQueryTab hierarchy");

    let manager = DbManager::new();

    manager.add_config("MyDB".to_string(), "host=localhost port=5432".to_string());

    let config = manager.get_config("MyDB").unwrap();
    config.add_tab(Some("Query 1".to_string()));
    config.add_tab(Some("Query 2".to_string()));

    assert_eq!(config.tab_count(), 2);

    let tab = config.get_tab(0).unwrap();
    tab.set_sql_query("SELECT * FROM users".to_string());

    assert_eq!(tab.sql_query(), "SELECT * FROM users");
    assert!(tab.connection().is_some());

    println!("✅ Complete hierarchy works correctly");
}

#[test]
fn test_dbmanager_multi_config_isolation() {
    println!("Test: Multiple configs are isolated from each other");

    let manager = DbManager::new();

    manager.add_config(
        "DB1".to_string(),
        "host=localhost port=5432 dbname=db1".to_string(),
    );
    manager.add_config(
        "DB2".to_string(),
        "host=localhost port=5433 dbname=db2".to_string(),
    );

    let config1 = manager.get_config("DB1").unwrap();
    let config2 = manager.get_config("DB2").unwrap();

    config1.add_tab(Some("Tab A".to_string()));
    config1.add_tab(Some("Tab B".to_string()));
    config1.add_tab(Some("Tab C".to_string()));

    config2.add_tab(Some("Tab X".to_string()));

    assert_eq!(config1.tab_count(), 3);
    assert_eq!(config2.tab_count(), 1);

    assert_ne!(config1.connection_string(), config2.connection_string());

    println!("✅ Configs are properly isolated");
}

#[glib::async_test]
#[ignore]
async fn test_dbmanager_full_workflow() {
    println!("Test: DbManager complete workflow with real database");

    let manager = DbManager::new();

    let events = Rc::new(RefCell::new(Vec::new()));
    let events_clone = events.clone();

    manager.connect_closure(
        "config-added",
        false,
        glib::closure_local!(
            move |_manager: &DbManager, name: String, _config: DbConfig| {
                events_clone
                    .borrow_mut()
                    .push(format!("config-added: {}", name));
            }
        ),
    );

    let events_clone = events.clone();
    manager.connect_closure(
        "tab-added",
        false,
        glib::closure_local!(move |_manager: &DbManager, index: i32, _tab: DbQueryTab| {
            events_clone
                .borrow_mut()
                .push(format!("tab-added: {}", index));
        }),
    );

    let events_clone = events.clone();
    manager.connect_closure(
        "connection-state-changed",
        false,
        glib::closure_local!(move |_manager: &DbManager,
                                   _old: ConnectionState,
                                   new: ConnectionState| {
            events_clone
                .borrow_mut()
                .push(format!("connection-state-changed: {:?}", new));
        }),
    );

    manager.add_config(
        "TestDB".to_string(),
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb".to_string(),
    );

    let config = manager.active_config().unwrap();
    config.add_tab(Some("Users Query".to_string()));
    config.add_tab(Some("Posts Query".to_string()));

    config.connect_all();

    let start = std::time::Instant::now();
    while !config.all_connected() && start.elapsed().as_secs() < 10 {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;
    }

    assert!(config.all_connected());

    let tab1 = config.get_tab(0).unwrap();
    let tab2 = config.get_tab(1).unwrap();

    let query_count = Rc::new(RefCell::new(0));
    let query_count_clone = query_count.clone();

    manager.connect_closure(
        "query-finished",
        false,
        glib::closure_local!(move |_manager: &DbManager, rows: i64, _duration: i64| {
            println!("Query finished: {} rows", rows);
            *query_count_clone.borrow_mut() += 1;
        }),
    );

    tab1.set_sql_query("SELECT * FROM users".to_string());
    tab2.set_sql_query("SELECT * FROM posts".to_string());

    tab1.execute_query();
    tab2.execute_query();

    let start = std::time::Instant::now();
    while *query_count.borrow() < 2 && start.elapsed().as_secs() < 10 {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(*query_count.borrow(), 2);
    assert_eq!(tab1.result_count(), 3);
    assert_eq!(tab2.result_count(), 4);

    println!("✅ Full workflow completed successfully");
    println!("Events captured: {}", events.borrow().len());

    manager.close_all();
}

#[glib::async_test]
#[ignore]
async fn test_dbmanager_multiple_databases() {
    println!("Test: DbManager with multiple database connections");

    let manager = DbManager::new();

    manager.add_config(
        "PostgreSQL 15".to_string(),
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb".to_string(),
    );

    manager.add_config(
        "PostgreSQL 16".to_string(),
        "host=localhost port=5416 user=user16 password=pass16 dbname=proddb".to_string(),
    );

    manager.add_config(
        "PostgreSQL 17".to_string(),
        "host=localhost port=5417 user=admin password=secret dbname=devdb".to_string(),
    );

    assert_eq!(manager.config_count(), 3);

    for config_name in manager.get_all_config_names() {
        let config = manager.get_config(&config_name).unwrap();
        config.add_tab(Some(format!("{} Query", config_name)));
        config.connect_all();
    }

    let start = std::time::Instant::now();
    let mut all_connected = false;
    while !all_connected && start.elapsed().as_secs() < 15 {
        glib::timeout_future(std::time::Duration::from_millis(10)).await;

        all_connected = manager
            .get_all_configs()
            .iter()
            .all(|config| config.all_connected());
    }

    assert!(all_connected);

    for config_name in manager.get_all_config_names() {
        let config = manager.get_config(&config_name).unwrap();
        let tab = config.get_tab(0).unwrap();
        assert_eq!(tab.connection_state(), ConnectionState::Connected);
    }

    println!("✅ Multiple database connections successful");

    manager.close_all();
}
