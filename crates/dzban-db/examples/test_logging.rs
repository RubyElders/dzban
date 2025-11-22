/// Example demonstrating the logging capabilities of dzban-db
/// 
/// Run with: cargo run --example test_logging
/// 
/// To see more detailed logs:
/// RUST_LOG=dzban_db=trace cargo run --example test_logging

use dzban_db::config::{Config, DatabaseConfig};
use dzban_db::reactive_manager::ReactiveConnectionManager;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() {
    // Initialize logging with detailed output
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("dzban_db=debug"))
        )
        .with_target(true)
        .with_thread_ids(false)
        .with_line_number(true)
        .init();

    tracing::info!("=== Dzban Database Logging Demo ===\n");

    // Create a test database configuration
    let config = DatabaseConfig {
        name: "test_db".to_string(),
        host: Some("localhost".to_string()),
        port: Some(5432),
        user: "postgres".to_string(),
        database: "postgres".to_string(),
        password: None,
    };

    tracing::info!("Creating ReactiveConnectionManager...");
    let manager = ReactiveConnectionManager::new();
    
    tracing::info!("\n=== Testing Connection ===");
    manager.test_connection("test_db".to_string(), config.clone()).await;
    
    // Wait a bit for the test to complete
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    tracing::info!("\n=== Attempting to Connect ===");
    manager.connect_async("test_db".to_string(), config.clone()).await;
    
    // Wait for connection
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    
    // Let monitoring run for a bit to see ping logs
    tracing::info!("\n=== Monitoring Connection (watch for pings) ===");
    tokio::time::sleep(tokio::time::Duration::from_secs(12)).await;
    
    tracing::info!("\n=== Disconnecting ===");
    manager.disconnect_async("test_db".to_string()).await;
    
    // Wait for disconnect to complete
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    
    tracing::info!("\n=== Demo Complete ===");
}
