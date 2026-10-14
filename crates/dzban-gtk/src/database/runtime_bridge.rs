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

use crate::database::InternalConnection;
use crate::database::config::DatabaseConfig;
use crate::database::connection_gobject::QueryReceiver;
use crate::database::{QueryPlan, ResultSet};

pub struct ConnectionDelivery {
    pub state: tokio::sync::watch::Receiver<dzban_core::application::ConnectionSnapshot>,
    pub result:
        oneshot::Receiver<Result<InternalConnection, dzban_core::application::ApplicationError>>,
}

pub struct StreamedQueryDelivery {
    pub id: dzban_core::execution::QueryId,
    pub generation: Option<u64>,
    pub state: tokio::sync::watch::Receiver<dzban_core::execution::QuerySnapshot>,
    pub result: QueryReceiver<
        dzban_core::execution::StreamedQueryCompletion,
        dzban_core::error::ExecutionError,
    >,
}

/// A bridge that runs Tokio tasks from non-Tokio contexts (like GTK/GLib)
#[derive(Clone)]
pub struct RuntimeBridge {
    runtime: Arc<Runtime>,
    context: Arc<dzban_core::application::ApplicationContext>,
}

impl std::fmt::Debug for RuntimeBridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeBridge")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl RuntimeBridge {
    pub fn schedule_preview(
        &self,
        connection: InternalConnection,
        service: dzban_core::preview::PlanPreviewService,
        document: dzban_core::documents::QueryDocument,
        sql: String,
    ) -> Result<dzban_core::preview::PreviewTarget, dzban_core::error::ExecutionError> {
        let _runtime = self.runtime.enter();
        service.schedule(
            connection.engine(),
            document,
            sql,
            dzban_core::preview::PREVIEW_DELAY,
        )
    }

    pub fn start_streamed(
        &self,
        connection: InternalConnection,
        sql: String,
        history: Option<dzban_core::history::QueryHistoryStore>,
    ) -> Result<StreamedQueryDelivery, dzban_core::error::ExecutionError> {
        let engine = connection.engine();
        let query = {
            let _runtime = self.runtime.enter();
            engine.start_streamed(sql, Default::default(), history)?
        };
        let id = query.id;
        let state = engine.subscribe();
        let (sender, result) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = sender.send(query.wait_completion().await);
        });
        Ok(StreamedQueryDelivery {
            id,
            generation: None,
            state,
            result,
        })
    }

    pub fn start_document(
        &self,
        connection: InternalConnection,
        document: dzban_core::documents::QueryDocument,
        sql: String,
        history: Option<dzban_core::history::QueryHistoryStore>,
    ) -> Result<StreamedQueryDelivery, dzban_core::error::ExecutionError> {
        let engine = connection.engine();
        let query = {
            let _runtime = self.runtime.enter();
            document.submit(engine.clone(), sql, Default::default(), history)?
        };
        let id = query.query;
        let generation = Some(query.generation);
        let state = engine.subscribe();
        let (sender, result) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = sender.send(query.wait().await);
        });
        Ok(StreamedQueryDelivery {
            id,
            generation,
            state,
            result,
        })
    }

    pub fn cancel_document(
        &self,
        connection: InternalConnection,
        document: dzban_core::documents::QueryDocument,
    ) -> Result<oneshot::Receiver<Result<(), String>>, dzban_core::error::ExecutionError> {
        let cancellation = {
            let _runtime = self.runtime.enter();
            document.request_cancel(connection.engine())?
        };
        let (sender, result) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = sender.send(cancellation.wait().await.map_err(|error| error.to_string()));
        });
        Ok(result)
    }
    pub fn read_result_page(
        &self,
        result: dzban_core::result_stream::ResultHandle,
        start: usize,
        count: usize,
    ) -> QueryReceiver<ResultSet, dzban_core::error::ExecutionError> {
        let (sender, receiver) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = sender.send(result.read_page(start, count).await);
        });
        receiver
    }

    pub fn read_result_rows(
        &self,
        result: dzban_core::result_stream::ResultHandle,
        start: usize,
    ) -> QueryReceiver<Vec<String>, dzban_core::error::ExecutionError> {
        let (sender, receiver) = oneshot::channel();
        self.runtime.spawn(async move {
            let page = result
                .read_page(start, dzban_core::result_store::MAX_PAGE_ROWS)
                .await;
            let rows = page.map(|page| {
                let mut bytes = 0usize;
                page.rows
                    .into_iter()
                    .map(|row| {
                        let chars = (1_048_576usize / row.len().max(1)).saturating_sub(3) / 6;
                        serde_json::to_string(
                            &row.iter()
                                .map(|cell| {
                                    cell.display()
                                        .chars()
                                        .take(chars.min(512))
                                        .collect::<String>()
                                })
                                .collect::<Vec<_>>(),
                        )
                        .expect("result preview serialization")
                    })
                    .take_while(|row| {
                        if bytes + row.len() > 2 * 1024 * 1024 {
                            return false;
                        }
                        bytes += row.len();
                        true
                    })
                    .collect()
            });
            let _ = sender.send(rows);
        });
        receiver
    }
    pub fn application_context(&self) -> Arc<dzban_core::application::ApplicationContext> {
        self.context.clone()
    }

    pub fn start_shutdown(
        &self,
    ) -> Result<
        oneshot::Receiver<Result<(), dzban_core::application::ApplicationError>>,
        dzban_core::application::ApplicationError,
    > {
        let shutdown = {
            let _runtime = self.runtime.enter();
            self.context.start_shutdown()?
        };
        let (sender, receiver) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = sender.send(shutdown.wait().await);
        });
        Ok(receiver)
    }

    /// Create a new runtime bridge with a dedicated Tokio runtime
    pub fn new() -> Result<Self, String> {
        Self::with_context(Arc::new(dzban_core::application::ApplicationContext::new(
            dzban_db::adapters::registry(),
        )))
    }

    pub fn with_context(
        context: Arc<dzban_core::application::ApplicationContext>,
    ) -> Result<Self, String> {
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
            context,
        })
    }

    /// Connect to a database (runs on Tokio runtime)
    /// Returns a oneshot channel that will receive the result
    pub fn connect(
        &self,
        db_config: DatabaseConfig,
    ) -> oneshot::Receiver<Result<InternalConnection, String>> {
        let (tx, rx) = oneshot::channel();
        let delivery = self.start_connect(db_config);
        let context = self.context.clone();
        self.runtime.spawn(async move {
            let result = delivery
                .result
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            if let Err(Ok(conn)) = tx.send(result) {
                let _ = context.close(conn.engine().id()).await;
            }
        });
        rx
    }

    pub fn start_connect(&self, db_config: DatabaseConfig) -> ConnectionDelivery {
        let (tx, rx) = oneshot::channel();
        let context = self.context.clone();
        let backend = db_config.backend;
        let request = {
            let _runtime = self.runtime.enter();
            context.start_connect(db_config)
        };
        let state = request.subscribe();

        self.runtime.spawn(async move {
            debug!("Bridge: Running connect on Tokio runtime");

            match request.wait().await {
                Ok(engine) => {
                    debug!("Bridge: Connection successful");
                    let conn = InternalConnection::new(engine.clone(), backend);
                    if tx.send(Ok(conn)).is_err() {
                        let _ = context.close(engine.id()).await;
                    }
                }
                Err(e) => {
                    error!("Bridge: Connection failed: {}", e);
                    let _ = tx.send(Err(e));
                }
            }
        });

        ConnectionDelivery { state, result: rx }
    }

    /// Test a database connection (runs on Tokio runtime)
    pub fn test_connection(
        &self,
        db_config: DatabaseConfig,
    ) -> oneshot::Receiver<Result<String, String>> {
        let (tx, rx) = oneshot::channel();
        let context = self.context.clone();

        self.runtime.spawn(async move {
            debug!("Bridge: Running connection test on Tokio runtime");

            let result = async {
                let engine = context
                    .connect_profile(db_config)
                    .await
                    .map_err(|error| error.to_string())?;
                let version = engine.version().await;
                let closed = context
                    .close(engine.id())
                    .await
                    .map_err(|error| error.to_string());
                match (version, closed) {
                    (Ok(version), Ok(())) => Ok(version),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                }
            }
            .await;

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
    pub fn disconnect(
        &self,
        conn: InternalConnection,
    ) -> Result<
        QueryReceiver<(), dzban_core::error::ExecutionError>,
        dzban_core::error::ExecutionError,
    > {
        let (tx, rx) = oneshot::channel();
        let closing = {
            let _runtime = self.runtime.enter();
            self.context.track_close(conn.engine().start_close()?)
        };
        self.runtime.spawn(async move {
            let result = closing.wait().await.map_err(|error| match error {
                dzban_core::application::ApplicationError::Operation(error) => error,
                error => dzban_core::error::ExecutionError::Message(error.to_string()),
            });
            let _ = tx.send(result);
        });
        Ok(rx)
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

    pub fn execute_result(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> QueryReceiver<dzban_core::QueryResult> {
        let detailed = self.execute_result_detailed(conn, query);
        let (tx, rx) = oneshot::channel();
        self.runtime.spawn(async move {
            let result = detailed.await.unwrap_or_else(|error| {
                Err(dzban_core::error::ExecutionError::Message(
                    error.to_string(),
                ))
            });
            let _ = tx.send(result.map_err(|error| error.to_string()));
        });
        rx
    }

    pub fn execute_result_detailed(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> QueryReceiver<dzban_core::QueryResult, dzban_core::error::ExecutionError> {
        match self.start_result(conn, query) {
            Ok(receiver) => receiver,
            Err(error) => {
                let (sender, receiver) = oneshot::channel();
                let _ = sender.send(Err(error));
                receiver
            }
        }
    }

    pub fn start_result(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> Result<
        QueryReceiver<dzban_core::QueryResult, dzban_core::error::ExecutionError>,
        dzban_core::error::ExecutionError,
    > {
        let (tx, rx) = oneshot::channel();
        let engine = conn.engine();
        let execution = {
            let _runtime = self.runtime.enter();
            engine.start_detailed(query)?
        };
        self.runtime.spawn(async move {
            let result = execution.wait_detailed().await;
            let _ = tx.send(result);
        });
        Ok(rx)
    }

    pub fn execute_planned(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> (
        QueryReceiver<dzban_core::QueryResult, dzban_core::error::ExecutionError>,
        QueryReceiver<QueryPlan, dzban_core::error::ExecutionError>,
    ) {
        match self.start_planned(conn, query) {
            Ok(receivers) => receivers,
            Err(error) => {
                let (result_tx, result_rx) = oneshot::channel();
                let (plan_tx, plan_rx) = oneshot::channel();
                let _ = result_tx.send(Err(error.clone()));
                let _ = plan_tx.send(Err(error));
                (result_rx, plan_rx)
            }
        }
    }

    pub fn start_planned(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> Result<
        crate::database::connection_gobject::PlannedQueryReceivers,
        dzban_core::error::ExecutionError,
    > {
        self.start_planned_with_history(conn, query, None)
    }

    pub fn start_planned_with_history(
        &self,
        conn: InternalConnection,
        query: String,
        history: Option<dzban_core::history::QueryHistoryStore>,
    ) -> Result<
        crate::database::connection_gobject::PlannedQueryReceivers,
        dzban_core::error::ExecutionError,
    > {
        self.start_planned_identified(conn, query, history)
            .map(|(_, receivers)| receivers)
    }

    pub fn start_planned_identified(
        &self,
        conn: InternalConnection,
        query: String,
        history: Option<dzban_core::history::QueryHistoryStore>,
    ) -> Result<
        (
            dzban_core::execution::QueryId,
            crate::database::connection_gobject::PlannedQueryReceivers,
        ),
        dzban_core::error::ExecutionError,
    > {
        let (result_tx, result_rx) = oneshot::channel();
        let (plan_tx, plan_rx) = oneshot::channel();
        let engine = conn.engine();
        let execution = {
            let _runtime = self.runtime.enter();
            match history {
                Some(history) => engine.start_recorded_detailed(query, history)?,
                None => engine.start_detailed(query)?,
            }
        };
        let id = execution.id;
        self.runtime.spawn(async move {
            let mut state = engine.subscribe();
            let execution = execution.wait_completion();
            tokio::pin!(execution);
            let mut plan_tx = Some(plan_tx);
            let result = loop {
                tokio::select! {
                    result = &mut execution => break result,
                    _ = state.changed() => {
                        let snapshot = state.borrow();
                        if snapshot.phase.id() == Some(id) && !matches!(snapshot.phase, dzban_core::execution::QueryPhase::Idle | dzban_core::execution::QueryPhase::Queued(_) | dzban_core::execution::QueryPhase::Explaining(_) | dzban_core::execution::QueryPhase::Canceling(_))
                            && let Some(tx) = plan_tx.take() {
                                let plan = snapshot.plan.clone().ok_or_else(|| snapshot.planning_error.clone().unwrap_or_else(|| "Plan unavailable".to_string().into()));
                                let _ = tx.send(plan);
                        }
                    }
                }
            };
            let result = match result {
                Ok(completion) => {
                    if let Some(error) = completion.history.as_ref().and_then(|history| history.error_for(id)) {
                        tracing::warn!("Failed to save query history: {}", error);
                    }
                    if let Some(tx) = plan_tx {
                        let snapshot = completion.snapshot;
                        let _ = tx.send(snapshot.plan.ok_or_else(|| snapshot.planning_error.unwrap_or_else(|| "Plan unavailable".to_string().into())));
                    }
                    completion.result
                }
                Err(error) => {
                    if let Some(tx) = plan_tx {
                        let _ = tx.send(Err(error.clone()));
                    }
                    Err(error)
                }
            };
            let _ = result_tx.send(result);
        });
        Ok((id, (result_rx, plan_rx)))
    }

    pub fn explain(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> oneshot::Receiver<Result<QueryPlan, String>> {
        let (tx, rx) = oneshot::channel();
        let request = self.start_explain(conn, query);
        self.runtime.spawn(async move {
            let result = match request {
                Ok(request) => request.await.unwrap_or_else(|error| {
                    Err(dzban_core::error::ExecutionError::Message(
                        error.to_string(),
                    ))
                }),
                Err(error) => Err(error),
            };
            let _ = tx.send(result.map_err(|error| error.to_string()));
        });

        rx
    }

    pub fn start_explain(
        &self,
        conn: InternalConnection,
        query: String,
    ) -> Result<
        QueryReceiver<QueryPlan, dzban_core::error::ExecutionError>,
        dzban_core::error::ExecutionError,
    > {
        let request = {
            let _runtime = self.runtime.enter();
            conn.engine().start_preview(query)?
        };
        let (tx, rx) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = tx.send(request.wait().await);
        });
        Ok(rx)
    }

    pub fn cancel_query(&self, conn: InternalConnection) -> oneshot::Receiver<Result<(), String>> {
        let engine = conn.engine();
        let state = engine.subscribe();
        let id = match state.borrow().phase {
            dzban_core::execution::QueryPhase::Queued(id)
            | dzban_core::execution::QueryPhase::Explaining(id)
            | dzban_core::execution::QueryPhase::Executing(id) => Some(id),
            _ => None,
        };
        if let Some(id) = id {
            return self.cancel_query_identified(conn, id);
        }
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Ok(()));
        rx
    }

    pub fn cancel_query_identified(
        &self,
        conn: InternalConnection,
        id: dzban_core::execution::QueryId,
    ) -> oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = oneshot::channel();
        let engine = conn.engine();
        self.runtime.spawn(async move {
            let _ = tx.send(engine.cancel(id).await);
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

    pub fn start_ping(
        &self,
        conn: InternalConnection,
    ) -> Result<
        QueryReceiver<u64, dzban_core::error::ExecutionError>,
        dzban_core::error::ExecutionError,
    > {
        let request = {
            let _runtime = self.runtime.enter();
            conn.engine().start_ping()?
        };
        let (tx, rx) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = tx.send(request.wait().await);
        });
        Ok(rx)
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

    struct Session;

    #[async_trait::async_trait]
    impl dzban_core::adapter::DatabaseSession for Session {
        async fn connect(&self) -> Result<(), String> {
            Ok(())
        }
        async fn ping(&self) -> Result<u64, String> {
            Ok(0)
        }
        async fn execute(&self, _: &str) -> Result<ResultSet, String> {
            Ok(ResultSet::default())
        }
        async fn cancel(&self) -> Result<(), String> {
            Ok(())
        }
        async fn version(&self) -> Result<String, String> {
            Ok("fixture".into())
        }
        async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
            Err("No fixture plan".into())
        }
    }

    #[test]
    fn idle_cancel_delivery_cannot_cancel_a_later_submission() {
        let bridge = RuntimeBridge {
            runtime: Arc::new(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
            ),
            context: Arc::new(dzban_core::application::ApplicationContext::new(
                Default::default(),
            )),
        };
        let engine = Arc::new(dzban_core::execution::QueryEngine::new(Arc::new(Session)));
        let connection = InternalConnection::new(engine, dzban_core::DatabaseBackend::Postgresql);
        let cancellation = bridge.cancel_query(connection.clone());
        let query = bridge.start_result(connection, "SELECT 1".into()).unwrap();
        let result = bridge.runtime.block_on(async {
            cancellation.await.unwrap().unwrap();
            query.await.unwrap()
        });
        assert!(
            result.is_ok(),
            "A stale idle cancellation changed the new query: {result:?}"
        );
    }

    #[test]
    fn test_bridge_creation() {
        let bridge = RuntimeBridge::new();
        assert!(bridge.is_ok());
    }
}
