use std::sync::Arc;
use tokio_postgres::{Client, Row};
use tokio_postgres_rustls::MakeRustlsConnect;
use tracing::{error, warn};

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

        let tls = tls_connector()?;

        match tokio_postgres::connect(&conn_string, tls).await {
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
                Err(format_postgres_error(&e))
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
            Err(e) => Err(format_postgres_error(&e)),
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
            Err(e) => Err(format_postgres_error(&e)),
        }
    }

    pub async fn execute_query_rows(&self, query: &str) -> Result<Vec<Row>, String> {
        let client = self.client.as_ref().ok_or("Not connected")?;

        client
            .query(query, &[])
            .await
            .map_err(|error| format_postgres_error(&error))
    }

    pub async fn cancel_query(&self) -> Result<(), String> {
        let client = self.client.as_ref().ok_or("Not connected")?;
        client
            .cancel_token()
            .cancel_query(tls_connector()?)
            .await
            .map_err(|error| format_postgres_error(&error))
    }

    pub async fn get_version(&self) -> Result<String, String> {
        let client = self.client.as_ref().ok_or("Not connected")?;

        match client.query_one("SELECT version()", &[]).await {
            Ok(row) => {
                let version: String = row.get(0);
                Ok(version)
            }
            Err(e) => Err(format_postgres_error(&e)),
        }
    }

    pub async fn test_connection(config: &DatabaseConfig) -> Result<String, String> {
        let conn_string = config.connection_string();

        let tls = tls_connector()?;

        match tokio_postgres::connect(&conn_string, tls).await {
            Ok((client, connection)) => {
                tokio::spawn(async move {
                    let _ = connection.await;
                });

                client
                    .query_one("SELECT version()", &[])
                    .await
                    .map(|row| row.get::<_, String>(0))
                    .map_err(|error| format_postgres_error(&error))
            }
            Err(e) => Err(format_postgres_error(&e)),
        }
    }
}

fn tls_connector() -> Result<MakeRustlsConnect, String> {
    match MakeRustlsConnect::with_native_certs() {
        Ok((connector, errors)) => {
            if !errors.is_empty() {
                warn!(
                    "Some system TLS certificates could not be loaded: {:?}",
                    errors
                );
            }
            Ok(connector)
        }
        Err(errors) => Err(format!(
            "Could not load the system TLS certificate store: {:?}",
            errors
        )),
    }
}

fn format_postgres_error(error: &tokio_postgres::Error) -> String {
    if let Some(database_error) = error.as_db_error() {
        let mut message = format!(
            "{} (SQLSTATE {})",
            database_error.message(),
            database_error.code().code()
        );
        if let Some(detail) = database_error.detail() {
            message.push_str(&format!(". Detail: {}", detail));
        }
        if let Some(hint) = database_error.hint() {
            message.push_str(&format!(". Hint: {}", hint));
        }
        return message;
    }

    format_error_chain(error)
}

fn format_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut messages = Vec::new();
    let mut current = Some(error);
    while let Some(error) = current {
        let message = error.to_string();
        if messages.last() != Some(&message) {
            messages.push(message);
        }
        current = error.source();
    }
    messages.join(": ")
}

#[cfg(test)]
mod tests {
    use super::format_error_chain;
    use std::error::Error;
    use std::fmt;

    #[derive(Debug)]
    struct OuterError(std::io::Error);

    impl fmt::Display for OuterError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("could not connect")
        }
    }

    impl Error for OuterError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn error_chains_include_the_underlying_cause() {
        let error = OuterError(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert_eq!(
            format_error_chain(&error),
            "could not connect: connection refused"
        );
    }
}
