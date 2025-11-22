use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_postgres::{Client, NoTls};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

pub struct DbConnection {
    pub db_name: String,
    pub connection_string: String,
    client: Arc<RwLock<Option<Client>>>,
    state: Arc<RwLock<ConnectionState>>,
    version: Arc<RwLock<Option<String>>>,
    last_ping: Arc<RwLock<Option<Instant>>>,
    latency: Arc<RwLock<Option<Duration>>>,
}

impl DbConnection {
    pub fn new(db_name: String, connection_string: String) -> Self {
        Self {
            db_name,
            connection_string,
            client: Arc::new(RwLock::new(None)),
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            version: Arc::new(RwLock::new(None)),
            last_ping: Arc::new(RwLock::new(None)),
            latency: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn connect(&self) -> Result<(), String> {
        *self.state.write().await = ConnectionState::Connecting;

        match tokio_postgres::connect(&self.connection_string, NoTls).await {
            Ok((client, connection)) => {
                // Spawn connection handler
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        eprintln!("Connection error: {}", e);
                    }
                });

                // Get PostgreSQL version
                match client.query_one("SELECT version()", &[]).await {
                    Ok(row) => {
                        let version: String = row.get(0);
                        // Extract just the version number (e.g., "PostgreSQL 15.3")
                        let version_short = version
                            .split_whitespace()
                            .take(2)
                            .collect::<Vec<_>>()
                            .join(" ");
                        *self.version.write().await = Some(version_short);
                    }
                    Err(e) => {
                        eprintln!("Failed to get version: {}", e);
                    }
                }

                *self.client.write().await = Some(client);
                *self.state.write().await = ConnectionState::Connected;
                *self.last_ping.write().await = Some(Instant::now());

                Ok(())
            }
            Err(e) => {
                *self.state.write().await = ConnectionState::Error;
                Err(format!("Connection failed: {}", e))
            }
        }
    }

    pub async fn disconnect(&self) {
        *self.client.write().await = None;
        *self.state.write().await = ConnectionState::Disconnected;
        *self.version.write().await = None;
        *self.last_ping.write().await = None;
        *self.latency.write().await = None;
    }

    pub async fn ping(&self) -> Result<Duration, String> {
        let start = Instant::now();
        
        let client = self.client.read().await;
        if let Some(client) = client.as_ref() {
            match client.query_one("SELECT 1", &[]).await {
                Ok(_) => {
                    let latency = start.elapsed();
                    // Read lock released when client is dropped
                    
                    *self.last_ping.write().await = Some(Instant::now());
                    *self.latency.write().await = Some(latency);
                    
                    Ok(latency)
                }
                Err(e) => {
                    // Read lock released when client is dropped
                    *self.state.write().await = ConnectionState::Error;
                    Err(format!("Ping failed: {}", e))
                }
            }
        } else {
            Err("Not connected".to_string())
        }
    }

    pub async fn get_state(&self) -> ConnectionState {
        *self.state.read().await
    }

    pub async fn get_version(&self) -> Option<String> {
        self.version.read().await.clone()
    }

    pub async fn get_latency(&self) -> Option<Duration> {
        *self.latency.read().await
    }

    #[allow(dead_code)]
    pub async fn execute_query(&self, query: &str) -> Result<Vec<tokio_postgres::Row>, String> {
        let client = self.client.read().await;
        if let Some(client) = client.as_ref() {
            match client.query(query, &[]).await {
                Ok(rows) => Ok(rows),
                Err(e) => Err(format!("Query failed: {}", e))
            }
        } else {
            Err("Not connected".to_string())
        }
    }
}

impl Clone for DbConnection {
    fn clone(&self) -> Self {
        Self {
            db_name: self.db_name.clone(),
            connection_string: self.connection_string.clone(),
            client: Arc::clone(&self.client),
            state: Arc::clone(&self.state),
            version: Arc::clone(&self.version),
            last_ping: Arc::clone(&self.last_ping),
            latency: Arc::clone(&self.latency),
        }
    }
}
