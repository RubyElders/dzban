use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use super::connection::DbConnection;
use crate::config::DatabaseConfig;

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
    pub async fn connect(&self, db_config: &DatabaseConfig) -> Result<DbConnection, String> {
        let mut conns = self.connections.write().await;
        
        if let Some(conn) = conns.get(&db_config.name) {
            // Already connected
            return Ok(conn.clone());
        }

        // Create new connection
        let conn = DbConnection::new(
            db_config.name.clone(),
            db_config.connection_string(),
        );

        conn.connect().await?;
        
        conns.insert(db_config.name.clone(), conn.clone());
        
        Ok(conn)
    }

    /// Get an existing connection
    #[allow(dead_code)]
    pub async fn get(&self, db_name: &str) -> Option<DbConnection> {
        let conns = self.connections.read().await;
        conns.get(db_name).cloned()
    }

    /// Disconnect from a database
    pub async fn disconnect(&self, db_name: &str) {
        let mut conns = self.connections.write().await;
        if let Some(conn) = conns.remove(db_name) {
            conn.disconnect().await;
        }
    }

    /// Get all active connections
    #[allow(dead_code)]
    pub async fn get_all(&self) -> Vec<DbConnection> {
        let conns = self.connections.read().await;
        conns.values().cloned().collect()
    }

    /// Start keep-alive ping loop for a connection
    pub fn start_keepalive(&self, db_name: String, interval_secs: u64) {
        let connections = Arc::clone(&self.connections);
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
            
            loop {
                interval.tick().await;
                
                let conns = connections.read().await;
                if let Some(conn) = conns.get(&db_name) {
                    let conn = conn.clone();
                    drop(conns); // Release lock before ping
                    
                    if let Err(e) = conn.ping().await {
                        eprintln!("Ping failed for {}: {}", db_name, e);
                    }
                } else {
                    // Connection removed, stop keep-alive
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
