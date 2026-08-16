use std::sync::Arc;
use tokio_postgres::{Client, NoTls, Row};
use tracing::error;

use crate::config::DatabaseConfig;

#[derive(Clone)]
pub struct InternalConnection {
    client: Option<Arc<Client>>,
    config: DatabaseConfig,
}

impl InternalConnection {
    pub fn new(config: DatabaseConfig) -> Self {
        Self {
            client: None,
            config,
        }
    }

    pub async fn connect(&mut self) -> Result<(), String> {
        let conn_string = self.config.connection_string();

        match tokio_postgres::connect(&conn_string, NoTls).await {
            Ok((client, connection)) => {
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        error!("Connection error: {}", e);
                    }
                });

                self.client = Some(Arc::new(client));
                Ok(())
            }
            Err(e) => {
                error!("Failed to connect: {}", e);
                Err(e.to_string())
            }
        }
    }

    pub async fn ping(&self) -> Result<u64, String> {
        let client = self.client.as_ref().ok_or("Not connected")?;

        let start = std::time::Instant::now();

        match client.simple_query("SELECT 1").await {
            Ok(_) => {
                let duration = start.elapsed();
                Ok(duration.as_micros() as u64)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    pub async fn execute_query(&self, query: &str) -> Result<u64, String> {
        let client = self.client.as_ref().ok_or("Not connected")?;

        match client.simple_query(query).await {
            Ok(results) => {
                let row_count = results
                    .iter()
                    .filter_map(|msg| match msg {
                        tokio_postgres::SimpleQueryMessage::Row(_) => Some(1u64),
                        _ => None,
                    })
                    .sum();
                Ok(row_count)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    pub async fn execute_query_rows(&self, query: &str) -> Result<Vec<Row>, String> {
        let client = self.client.as_ref().ok_or("Not connected")?;

        client.query(query, &[]).await.map_err(|e| e.to_string())
    }

    pub async fn cancel_query(&self) -> Result<(), String> {
        let client = self.client.as_ref().ok_or("Not connected")?;
        client
            .cancel_token()
            .cancel_query(NoTls)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn get_version(&self) -> Result<String, String> {
        let client = self.client.as_ref().ok_or("Not connected")?;

        match client.query_one("SELECT version()", &[]).await {
            Ok(row) => {
                let version: String = row.get(0);
                Ok(version)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    pub async fn test_connection(config: &DatabaseConfig) -> Result<String, String> {
        let conn_string = config.connection_string();

        match tokio_postgres::connect(&conn_string, NoTls).await {
            Ok((client, connection)) => {
                tokio::spawn(async move {
                    let _ = connection.await;
                });

                client
                    .query_one("SELECT version()", &[])
                    .await
                    .map(|row| row.get::<_, String>(0))
                    .map_err(|error| error.to_string())
            }
            Err(e) => Err(e.to_string()),
        }
    }
}
