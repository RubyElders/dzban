//! Bridge between GLib/GTK async runtime and Tokio runtime
//!
//! This module solves the problem that tokio-postgres requires a Tokio runtime,
//! but GTK applications run on GLib's MainContext event loop.
//!
//! Solution: Run a Tokio runtime in a background thread and communicate via channels.

use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tracing::{debug, error, info, trace};

use crate::config::DatabaseConfig;
use crate::internal_connection::InternalConnection;
use crate::{QueryPlan, ResultSet};

/// A bridge that runs Tokio tasks from non-Tokio contexts (like GTK/GLib)
#[derive(Clone, Debug)]
pub struct RuntimeBridge {
    runtime: Arc<Runtime>,
}

impl RuntimeBridge {
    /// Create a new runtime bridge with a dedicated Tokio runtime
    pub fn new() -> Result<Self, String> {
        info!("Creating Tokio runtime bridge");

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2) // 2 threads for DB operations
            .thread_name("dzban-tokio")
            .enable_all()
            .build()
            .map_err(|e| format!("Failed to create Tokio runtime: {}", e))?;

        debug!("Tokio runtime bridge created successfully");

        Ok(Self {
            runtime: Arc::new(runtime),
        })
    }

    /// Connect to a database (runs on Tokio runtime)
    /// Returns a oneshot channel that will receive the result
    pub fn connect(
        &self,
        db_config: DatabaseConfig,
    ) -> oneshot::Receiver<Result<InternalConnection, String>> {
        let (tx, rx) = oneshot::channel();

        self.runtime.spawn(async move {
            debug!("Bridge: Running connect on Tokio runtime");

            let conn = match InternalConnection::new(db_config) {
                Ok(conn) => conn,
                Err(error) => {
                    let _ = tx.send(Err(error));
                    return;
                }
            };

            let result = conn.connect().await;

            match result {
                Ok(()) => {
                    debug!("Bridge: Connection successful");
                    let _ = tx.send(Ok(conn));
                }
                Err(e) => {
                    error!("Bridge: Connection failed: {}", e);
                    let _ = tx.send(Err(e));
                }
            }
        });

        rx
    }

    /// Test a database connection (runs on Tokio runtime)
    pub fn test_connection(
        &self,
        db_config: DatabaseConfig,
    ) -> oneshot::Receiver<Result<String, String>> {
        let (tx, rx) = oneshot::channel();

        self.runtime.spawn(async move {
            debug!("Bridge: Running connection test on Tokio runtime");

            let result = InternalConnection::test_connection(&db_config).await;

            match result {
                Ok(version) => {
                    debug!("Bridge: Connection test successful, version: {}", version);
                    let _ = tx.send(Ok(version));
                }
                Err(e) => {
                    error!("Bridge: Connection test failed: {}", e);
                    let _ = tx.send(Err(e));
                }
            }
        });

        rx
    }

    /// Disconnect from a database (runs on Tokio runtime)
    pub fn disconnect(&self, _conn: InternalConnection) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();

        self.runtime.spawn(async move {
            debug!("Bridge: Disconnecting (connection will be dropped)");
            let _ = tx.send(());
        });

        rx
    }

    /// Execute a query (runs on Tokio runtime)
    pub fn execute_query(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> oneshot::Receiver<Result<(u64, InternalConnection), String>> {
        let (tx, rx) = oneshot::channel();

        self.runtime.spawn(async move {
            debug!("Bridge: Running query on Tokio runtime");
            let result = conn.execute_query_rows(&query).await;
            match result {
                Ok(result_set) => {
                    let _ = tx.send(Ok((result_set.rows.len() as u64, conn)));
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        });

        rx
    }

    pub fn execute_query_rows(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> oneshot::Receiver<Result<ResultSet, String>> {
        let (tx, rx) = oneshot::channel();

        self.runtime.spawn(async move {
            debug!("Bridge: Running row query on Tokio runtime");
            let result = conn.execute_query_rows(&query).await;
            let _ = tx.send(result);
        });

        rx
    }

    pub fn explain(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> oneshot::Receiver<Result<QueryPlan, String>> {
        let (tx, rx) = oneshot::channel();

        self.runtime.spawn(async move {
            let _ = tx.send(conn.explain(&query).await);
        });

        rx
    }

    pub fn cancel_query(&self, conn: InternalConnection) -> oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = tx.send(conn.cancel_query().await);
        });
        rx
    }

    pub fn version(&self, conn: InternalConnection) -> oneshot::Receiver<Result<String, String>> {
        let (tx, rx) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = tx.send(conn.get_version().await);
        });
        rx
    }

    /// Ping a connection (runs on Tokio runtime)
    pub fn ping(
        &self,
        conn: InternalConnection,
    ) -> oneshot::Receiver<Result<(u64, InternalConnection), String>> {
        let (tx, rx) = oneshot::channel();

        self.runtime.spawn(async move {
            trace!("Bridge: Running ping on Tokio runtime");
            let result = conn.ping().await;
            match result {
                Ok(latency) => {
                    let _ = tx.send(Ok((latency, conn)));
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        });

        rx
    }
}

impl Default for RuntimeBridge {
    fn default() -> Self {
        Self::new().expect("Failed to create RuntimeBridge")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_creation() {
        let bridge = RuntimeBridge::new();
        assert!(bridge.is_ok());
    }
}
