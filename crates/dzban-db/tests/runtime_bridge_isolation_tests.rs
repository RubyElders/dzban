use dzban_db::{DatabaseConfig, RuntimeBridge};
use std::time::Duration;

#[test]
#[ignore]
fn test_runtime_bridge_connect() {
    println!("Test: RuntimeBridge can connect to PostgreSQL");

    let bridge = RuntimeBridge::new().expect("Failed to create bridge");

    let config = DatabaseConfig {
        name: "test_pg15".to_string(),
        host: Some("localhost".to_string()),
        port: Some(5415),
        user: "testuser".to_string(),
        password: Some("testpass".to_string()),
        database: "testdb".to_string(),
    };

    let rx = bridge.connect(config);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(rx);

    match result {
        Ok(Ok(db_conn)) => {
            println!("✅ Connected successfully");

            let disconnect_rx = bridge.disconnect(db_conn);
            let _ = rt.block_on(disconnect_rx);
        }
        Ok(Err(e)) => {
            panic!("Connection failed: {}", e);
        }
        Err(_) => {
            panic!("Connection cancelled");
        }
    }
}

#[test]
#[ignore]
fn test_runtime_bridge_ping() {
    println!("Test: RuntimeBridge can ping database");

    let bridge = RuntimeBridge::new().expect("Failed to create bridge");

    let config = DatabaseConfig {
        name: "test_pg15".to_string(),
        host: Some("localhost".to_string()),
        port: Some(5415),
        user: "testuser".to_string(),
        password: Some("testpass".to_string()),
        database: "testdb".to_string(),
    };

    let rx = bridge.connect(config);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(rx).unwrap();
    let db_conn = result.expect("Connect failed");

    let ping_rx = bridge.ping(db_conn);
    let ping_result = rt.block_on(ping_rx).expect("Ping channel closed");

    let db_conn = match ping_result {
        Ok((latency_micros, conn)) => {
            println!("✅ Ping successful: {} µs", latency_micros);
            assert!(latency_micros > 0);
            conn
        }
        Err(e) => {
            panic!("Ping failed: {}", e);
        }
    };

    let disconnect_rx = bridge.disconnect(db_conn);
    let _ = rt.block_on(disconnect_rx);
}

#[test]
#[ignore]
fn test_runtime_bridge_query() {
    println!("Test: RuntimeBridge can execute query");

    let bridge = RuntimeBridge::new().expect("Failed to create bridge");

    let config = DatabaseConfig {
        name: "test_pg15".to_string(),
        host: Some("localhost".to_string()),
        port: Some(5415),
        user: "testuser".to_string(),
        password: Some("testpass".to_string()),
        database: "testdb".to_string(),
    };

    let rx = bridge.connect(config);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(rx).unwrap();
    let db_conn = result.expect("Connect failed");

    let query_rx = bridge.execute_query(db_conn, "SELECT * FROM users".to_string());
    let query_result = rt.block_on(query_rx).expect("Query channel closed");

    let db_conn = match query_result {
        Ok((row_count, conn)) => {
            println!("✅ Query successful: {} rows", row_count);
            assert_eq!(row_count, 3);
            conn
        }
        Err(e) => {
            panic!("Query failed: {}", e);
        }
    };

    let disconnect_rx = bridge.disconnect(db_conn);
    let _ = rt.block_on(disconnect_rx);
}

#[test]
#[ignore]
fn test_runtime_bridge_invalid_connection() {
    println!("Test: RuntimeBridge handles invalid connection");

    let bridge = RuntimeBridge::new().expect("Failed to create bridge");

    let config = DatabaseConfig {
        name: "invalid".to_string(),
        host: Some("invalid-host-does-not-exist".to_string()),
        port: Some(5432),
        user: "user".to_string(),
        password: Some("pass".to_string()),
        database: "db".to_string(),
    };

    let rx = bridge.connect(config);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { tokio::time::timeout(Duration::from_secs(5), rx).await });

    match result {
        Ok(Ok(Err(e))) => {
            println!("✅ Correctly failed with error: {}", e);
            assert!(!e.is_empty());
        }
        Ok(Ok(Ok(_))) => {
            panic!("Should not have connected to invalid host");
        }
        Ok(Err(_)) => {
            panic!("Connection should not be cancelled");
        }
        Err(_) => {
            println!("✅ Connection timed out as expected for invalid host");
        }
    }
}

#[test]
#[ignore]
fn test_runtime_bridge_test_connection() {
    println!("Test: RuntimeBridge test_connection helper");

    let bridge = RuntimeBridge::new().expect("Failed to create bridge");

    let config = DatabaseConfig {
        name: "test_pg16".to_string(),
        host: Some("localhost".to_string()),
        port: Some(5416),
        user: "user16".to_string(),
        password: Some("pass16".to_string()),
        database: "proddb".to_string(),
    };

    let rx = bridge.test_connection(config);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(rx).expect("Channel closed");

    match result {
        Ok(version) => {
            println!("✅ Test connection successful: {}", version);
            assert!(version.contains("PostgreSQL"));
        }
        Err(e) => {
            panic!("Test connection failed: {}", e);
        }
    }
}
