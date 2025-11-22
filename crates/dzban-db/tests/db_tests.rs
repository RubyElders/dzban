// Integration tests for database connection
use dzban_db::{DbConnection, ConnectionState, Config, DatabaseConfig};
use std::time::Duration;

#[tokio::test]
async fn test_connection_lifecycle() {
    println!("🧪 Testing connection lifecycle...");
    
    let config = Config::load().expect("Config should load");
    if config.databases.is_empty() {
        println!("  ⚠️  Skipping: no databases configured");
        return;
    }
    
    let db_config = &config.databases[0];
    let conn_string = db_config.connection_string();
    let db_conn = DbConnection::new(db_config.name.clone(), conn_string);
    
    // Initial state should be disconnected
    assert_eq!(db_conn.get_state().await, ConnectionState::Disconnected);
    println!("  ✓ Initial state: Disconnected");
    
    // Connect
    match db_conn.connect().await {
        Ok(_) => {
            println!("  ✓ Connected successfully");
            assert_eq!(db_conn.get_state().await, ConnectionState::Connected);
        }
        Err(e) => {
            eprintln!("  ✗ Connection failed: {}", e);
            panic!("Connection should succeed");
        }
    }
    
    // Check version
    if let Some(version) = db_conn.get_version().await {
        println!("  ✓ PostgreSQL version: {}", version);
        assert!(version.contains("PostgreSQL"));
    } else {
        panic!("Should have version after connect");
    }
    
    // Test ping
    match db_conn.ping().await {
        Ok(latency) => {
            println!("  ✓ Ping successful: {:?}", latency);
            assert!(latency < Duration::from_secs(1));
        }
        Err(e) => panic!("Ping failed: {}", e),
    }
    
    // Disconnect
    db_conn.disconnect().await;
    assert_eq!(db_conn.get_state().await, ConnectionState::Disconnected);
    println!("  ✓ Disconnected successfully");
    
    println!("✅ Connection lifecycle test passed\n");
}

#[tokio::test]
async fn test_multiple_pings() {
    println!("🧪 Testing multiple pings...");
    
    let config = Config::load().expect("Config should load");
    if config.databases.is_empty() {
        println!("  ⚠️  Skipping: no databases configured");
        return;
    }
    
    let db_config = &config.databases[0];
    let conn_string = db_config.connection_string();
    let db_conn = DbConnection::new(db_config.name.clone(), conn_string);
    
    db_conn.connect().await.expect("Connection should succeed");
    
    for i in 1..=5 {
        match db_conn.ping().await {
            Ok(latency) => {
                println!("  ✓ Ping #{}: {:?}", i, latency);
                assert!(latency < Duration::from_secs(1));
            }
            Err(e) => panic!("Ping #{} failed: {}", i, e),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    
    // Check latency was recorded
    if let Some(latency) = db_conn.get_latency().await {
        println!("  ✓ Final latency: {:?}", latency);
    } else {
        panic!("Should have latency after pings");
    }
    
    db_conn.disconnect().await;
    println!("✅ Multiple pings test passed\n");
}

#[tokio::test]
async fn test_connection_to_nonexistent_db() {
    println!("🧪 Testing connection to nonexistent database...");
    
    let config = DatabaseConfig {
        name: "nonexistent".to_string(),
        host: Some("/tmp".to_string()),
        port: Some(5432),
        user: "retro".to_string(),
        database: "nonexistent_db_12345".to_string(),
        password: None,
    };
    
    let conn_string = config.connection_string();
    let db_conn = DbConnection::new("nonexistent".to_string(), conn_string);
    
    match db_conn.connect().await {
        Ok(_) => panic!("Should fail to connect to nonexistent database"),
        Err(e) => {
            println!("  ✓ Expected error: {}", e);
            assert_eq!(db_conn.get_state().await, ConnectionState::Error);
        }
    }
    
    println!("✅ Nonexistent database test passed\n");
}

#[tokio::test]
async fn test_config_loading() {
    println!("🧪 Testing config loading...");
    
    match Config::load() {
        Ok(config) => {
            println!("  ✓ Config loaded successfully");
            println!("  ✓ Found {} databases", config.databases.len());
            
            for db in &config.databases {
                println!("    - {}", db.name);
                assert!(!db.name.is_empty());
                assert!(!db.user.is_empty());
                assert!(!db.database.is_empty());
            }
        }
        Err(e) => panic!("Failed to load config: {}", e),
    }
    
    println!("✅ Config loading test passed\n");
}

#[tokio::test]
async fn test_all_configured_databases() {
    println!("🧪 Testing all configured databases...");
    
    let config = Config::load().expect("Config should load");
    
    for db_config in &config.databases {
        println!("  Testing database: {}", db_config.name);
        
        let conn_string = db_config.connection_string();
        let db_conn = DbConnection::new(db_config.name.clone(), conn_string);
        
        match db_conn.connect().await {
            Ok(_) => {
                println!("    ✓ Connected to {}", db_config.name);
                
                if let Some(version) = db_conn.get_version().await {
                    println!("    ✓ Version: {}", version.split('\n').next().unwrap_or(&version));
                }
                
                match db_conn.ping().await {
                    Ok(latency) => println!("    ✓ Ping: {:?}", latency),
                    Err(e) => eprintln!("    ✗ Ping failed: {}", e),
                }
                
                db_conn.disconnect().await;
                println!("    ✓ Disconnected from {}", db_config.name);
            }
            Err(e) => {
                eprintln!("    ✗ Failed to connect to {}: {}", db_config.name, e);
                // Don't panic, just log - some databases might not be running
            }
        }
    }
    
    println!("✅ All configured databases test passed\n");
}

#[tokio::test]
async fn test_concurrent_connections() {
    println!("🧪 Testing concurrent connections...");
    
    let config = Config::load().expect("Config should load");
    
    if config.databases.len() < 2 {
        println!("  ⚠️  Skipping: need at least 2 databases configured");
        return;
    }
    
    let db1 = &config.databases[0];
    let db2 = &config.databases[1];
    
    let conn1 = DbConnection::new(db1.name.clone(), db1.connection_string());
    let conn2 = DbConnection::new(db2.name.clone(), db2.connection_string());
    
    // Connect both concurrently
    let (result1, result2) = tokio::join!(
        conn1.connect(),
        conn2.connect()
    );
    
    assert!(result1.is_ok(), "First connection should succeed");
    assert!(result2.is_ok(), "Second connection should succeed");
    println!("  ✓ Both connections established concurrently");
    
    // Ping both concurrently
    let (ping1, ping2) = tokio::join!(
        conn1.ping(),
        conn2.ping()
    );
    
    assert!(ping1.is_ok(), "First ping should succeed");
    assert!(ping2.is_ok(), "Second ping should succeed");
    println!("  ✓ Both pings successful");
    println!("    - {}: {:?}", db1.name, ping1.unwrap());
    println!("    - {}: {:?}", db2.name, ping2.unwrap());
    
    // Disconnect both
    tokio::join!(
        conn1.disconnect(),
        conn2.disconnect()
    );
    println!("  ✓ Both connections closed");
    
    println!("✅ Concurrent connections test passed\n");
}

#[test]
fn test_connection_string_generation() {
    println!("🧪 Testing connection string generation...");
    
    let config = DatabaseConfig {
        name: "test".to_string(),
        host: Some("/tmp".to_string()),
        port: Some(5432),
        user: "testuser".to_string(),
        database: "testdb".to_string(),
        password: Some("secret".to_string()),
    };
    
    let conn_string = config.connection_string();
    println!("  Generated: {}", conn_string);
    
    assert!(conn_string.contains("user=testuser"));
    assert!(conn_string.contains("dbname=testdb"));
    assert!(conn_string.contains("host=/tmp"));
    assert!(conn_string.contains("port=5432"));
    assert!(conn_string.contains("password=secret"));
    
    println!("✅ Connection string test passed\n");
}

#[test]
fn test_connection_string_without_password() {
    println!("🧪 Testing connection string without password...");
    
    let config = DatabaseConfig {
        name: "test".to_string(),
        host: Some("localhost".to_string()),
        port: None,
        user: "testuser".to_string(),
        database: "testdb".to_string(),
        password: None,
    };
    
    let conn_string = config.connection_string();
    println!("  Generated: {}", conn_string);
    
    assert!(conn_string.contains("user=testuser"));
    assert!(conn_string.contains("dbname=testdb"));
    assert!(conn_string.contains("host=localhost"));
    assert!(!conn_string.contains("password"));
    assert!(!conn_string.contains("port"));
    
    println!("✅ Connection string without password test passed\n");
}
