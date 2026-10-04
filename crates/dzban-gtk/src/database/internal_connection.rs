use std::sync::Arc;

use dzban_core::execution::{QueryEngine, QueryPhase};
use dzban_core::{DatabaseBackend, QueryPlan, ResultSet};

#[derive(Clone)]
pub struct InternalConnection {
    engine: Arc<QueryEngine>,
    backend: DatabaseBackend,
}

impl InternalConnection {
    pub fn new(engine: Arc<QueryEngine>, backend: DatabaseBackend) -> Self {
        Self { engine, backend }
    }

    pub fn backend(&self) -> DatabaseBackend {
        self.backend
    }

    pub fn engine(&self) -> Arc<QueryEngine> {
        self.engine.clone()
    }

    pub async fn ping(&self) -> Result<u64, String> {
        self.engine.ping().await
    }

    pub async fn execute_query_rows(&self, query: &str) -> Result<ResultSet, String> {
        self.engine
            .execute(query)
            .await
            .map(|result| result.result_set)
    }

    pub async fn cancel_query(&self) -> Result<(), String> {
        let state = self.engine.subscribe();
        let id = match state.borrow().phase {
            QueryPhase::Queued(id)
            | QueryPhase::Explaining(id)
            | QueryPhase::Executing(id)
            | QueryPhase::Canceling(id) => Some(id),
            _ => None,
        };
        if let Some(id) = id {
            self.engine.cancel(id).await
        } else {
            Ok(())
        }
    }

    pub async fn get_version(&self) -> Result<String, String> {
        self.engine.version().await
    }

    pub async fn explain(&self, query: &str) -> Result<QueryPlan, String> {
        self.engine.preview(query).await
    }
}
