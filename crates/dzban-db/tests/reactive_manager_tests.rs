// Integration tests for reactive connection manager
// Note: Full reactive tests require GTK main loop, so we test what we can
use dzban_db::{ReactiveConnectionManager, ReactiveConnection, ConnectionState, Config, DatabaseConfig};
use glib::prelude::*;

#[test]
fn test_reactive_manager_creation() {
    println!("🧪 Testing reactive manager creation...");
    
    let manager = ReactiveConnectionManager::new();
    let conns = manager.all_connections();
    assert_eq!(conns.len(), 0, "New manager should have no connections");
    println!("  ✓ Manager created with 0 connections");
    
    println!("✅ Reactive manager creation test passed\n");
}

#[test]
fn test_reactive_connection_creation() {
    println!("🧪 Testing reactive connection creation...");
    
    let config = DatabaseConfig {
        name: "test".to_string(),
        host: Some("/tmp".to_string()),
        port: Some(5432),
        user: "retro".to_string(),
        database: "test_db".to_string(),
        password: None,
    };
    
    let reactive_conn = ReactiveConnection::new("test", config);
    
    // Test initial properties
    let db_name = reactive_conn.db_name();
    assert_eq!(db_name, "test");
    println!("  ✓ db_name property: {}", db_name);
    
    let state = reactive_conn.state();
    assert_eq!(state, ConnectionState::Disconnected);
    println!("  ✓ Initial state: {:?}", state);
    
    let version = reactive_conn.version();
    assert_eq!(version, "");
    println!("  ✓ Initial version: empty");
    
    let latency = reactive_conn.latency();
    assert_eq!(latency, 0);
    println!("  ✓ Initial latency: {}ms", latency);
    
    println!("✅ Reactive connection creation test passed\n");
}

#[test]
fn test_reactive_connection_property_setters() {
    println!("🧪 Testing reactive connection property setters...");
    
    let config = DatabaseConfig {
        name: "test".to_string(),
        host: Some("/tmp".to_string()),
        port: Some(5432),
        user: "retro".to_string(),
        database: "test_db".to_string(),
        password: None,
    };
    
    let reactive_conn = ReactiveConnection::new("test", config);
    
    // Set state
    reactive_conn.set_state(ConnectionState::Connected);
    assert_eq!(reactive_conn.state(), ConnectionState::Connected);
    println!("  ✓ State set to Connected");
    
    // Set version
    reactive_conn.set_version("PostgreSQL 16.0");
    assert_eq!(reactive_conn.version(), "PostgreSQL 16.0");
    println!("  ✓ Version set to 'PostgreSQL 16.0'");
    
    // Set latency
    reactive_conn.set_latency(42);
    assert_eq!(reactive_conn.latency(), 42);
    println!("  ✓ Latency set to 42ms");
    
    println!("✅ Reactive connection property setters test passed\n");
}

#[test]
fn test_reactive_manager_add_remove() {
    println!("🧪 Testing reactive manager add/remove...");
    
    let manager = ReactiveConnectionManager::new();
    
    let config = DatabaseConfig {
        name: "test1".to_string(),
        host: Some("/tmp".to_string()),
        port: Some(5432),
        user: "retro".to_string(),
        database: "test_db".to_string(),
        password: None,
    };
    
    let conn1 = ReactiveConnection::new("test1", config.clone());
    manager.add_connection(conn1);
    
    assert_eq!(manager.all_connections().len(), 1);
    println!("  ✓ Added 1 connection");
    
    let config2 = DatabaseConfig {
        name: "test2".to_string(),
        ..config
    };
    let conn2 = ReactiveConnection::new("test2", config2);
    manager.add_connection(conn2);
    
    assert_eq!(manager.all_connections().len(), 2);
    println!("  ✓ Added 2nd connection, total: 2");
    
    manager.remove_connection("test1");
    assert_eq!(manager.all_connections().len(), 1);
    println!("  ✓ Removed 1 connection, total: 1");
    
    manager.remove_connection("test2");
    assert_eq!(manager.all_connections().len(), 0);
    println!("  ✓ Removed last connection, total: 0");
    
    println!("✅ Reactive manager add/remove test passed\n");
}

#[test]
fn test_reactive_manager_get_connection() {
    println!("🧪 Testing reactive manager get_connection...");
    
    let manager = ReactiveConnectionManager::new();
    
    let config = DatabaseConfig {
        name: "findme".to_string(),
        host: Some("/tmp".to_string()),
        port: Some(5432),
        user: "retro".to_string(),
        database: "test_db".to_string(),
        password: None,
    };
    
    let conn = ReactiveConnection::new("findme", config);
    manager.add_connection(conn);
    
    // Should find it
    let found = manager.get_connection("findme");
    assert!(found.is_some(), "Should find connection 'findme'");
    println!("  ✓ Found connection 'findme'");
    
    if let Some(fc) = found {
        assert_eq!(fc.db_name(), "findme");
        println!("  ✓ Connection has correct name");
    }
    
    // Should not find non-existent
    let not_found = manager.get_connection("doesntexist");
    assert!(not_found.is_none(), "Should not find non-existent connection");
    println!("  ✓ Correctly returns None for non-existent connection");
    
    println!("✅ Reactive manager get_connection test passed\n");
}

#[test]
fn test_config_loading_for_reactive() {
    println!("🧪 Testing config loading for reactive manager...");
    
    let config = Config::load().expect("Config should load");
    println!("  ✓ Config loaded: {} databases", config.databases.len());
    
    let manager = ReactiveConnectionManager::new();
    
    // Create reactive connections for all configured databases
    for db_config in &config.databases {
        let reactive_conn = ReactiveConnection::new(&db_config.name, db_config.clone());
        manager.add_connection(reactive_conn);
        println!("    + Added reactive connection for '{}'", db_config.name);
    }
    
    assert_eq!(manager.all_connections().len(), config.databases.len());
    println!("  ✓ Created {} reactive connections", config.databases.len());
    
    println!("✅ Config loading for reactive test passed\n");
}
