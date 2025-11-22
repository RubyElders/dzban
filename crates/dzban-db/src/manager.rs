use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use super::connection::DbConnection;
use crate::config::DatabaseConfig;
use tracing::{info, debug, warn, instrument};

/// Global connection manager that tracks all open database connections
pub struct ConnectionManager {
    connections: Arc<RwLock<HashMap<String, DbConnection>>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Connect to a database (or return existing connection)
    #[instrument(skip(self, db_config), fields(db_name = %db_config.name))]
    pub async fn connect(&self, db_config: &DatabaseConfig) -> Result<DbConnection, String> {
        let mut conns = self.connections.write().await;
        
        if let Some(conn) = conns.get(&db_config.name) {
            // Already connected
            info!("Reusing existing connection to '{}'", db_config.name);
            return Ok(conn.clone());
        }

        info!("Creating new connection to '{}'", db_config.name);
        
        // Create new connection
        let conn = DbConnection::new(
            db_config.name.clone(),
            db_config.connection_string(),
        );

        conn.connect().await?;
        
        conns.insert(db_config.name.clone(), conn.clone());
        
        info!("✅ Connection to '{}' established and cached", db_config.name);
        
        Ok(conn)
    }

    /// Get an existing connection
    #[allow(dead_code)]
    pub async fn get(&self, db_name: &str) -> Option<DbConnection> {
        let conns = self.connections.read().await;
        conns.get(db_name).cloned()
    }

    /// Disconnect from a database
    #[instrument(skip(self))]
    pub async fn disconnect(&self, db_name: &str) {
        info!("Disconnecting from '{}'", db_name);
        
        let mut conns = self.connections.write().await;
        if let Some(conn) = conns.remove(db_name) {
            conn.disconnect().await;
            debug!("✅ Disconnected and removed '{}' from cache", db_name);
        } else {
            warn!("Attempted to disconnect from unknown connection '{}'", db_name);
        }
    }

    /// Get all active connections
    #[allow(dead_code)]
    pub async fn get_all(&self) -> Vec<DbConnection> {
        let conns = self.connections.read().await;
        conns.values().cloned().collect()
    }

    /// Start keep-alive ping loop for a connection
    #[instrument(skip(self))]
    pub fn start_keepalive(&self, db_name: String, interval_secs: u64) {
        info!("Starting keepalive for '{}' (interval: {}s)", db_name, interval_secs);
        
        let connections = Arc::clone(&self.connections);
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
            let mut ping_count = 0;
            
            loop {
                interval.tick().await;
                ping_count += 1;
                
                let conns = connections.read().await;
                if let Some(conn) = conns.get(&db_name) {
                    let conn = conn.clone();
                    drop(conns); // Release lock before ping
                    
                    debug!("Keepalive ping #{} for '{}'", ping_count, db_name);
                    
                    if let Err(e) = conn.ping().await {
                        warn!("Keepalive ping failed for '{}': {}", db_name, e);
                    }
                } else {
                    // Connection removed, stop keep-alive
                    info!("Stopping keepalive for '{}' (connection removed, {} pings total)", db_name, ping_count);
                    break;
                }
            }
        });
    }
}

impl Clone for ConnectionManager {
    fn clone(&self) -> Self {
        Self {
            connections: Arc::clone(&self.connections),
        }
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}
