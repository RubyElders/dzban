use async_trait::async_trait;
use std::sync::Arc;

use crate::{DatabaseBackend, DatabaseConfig, QueryPlan, ResultSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransactionStatus {
    Idle,
    InTransaction,
    FailedTransaction,
}

#[async_trait]
pub trait DatabaseSession: Send + Sync {
    fn transaction_status(&self) -> Option<TransactionStatus> {
        None
    }
    fn action_descriptors(&self) -> Vec<crate::action::ActionDescriptor> {
        Vec::new()
    }
    fn capabilities(&self) -> crate::catalog::Capabilities {
        crate::catalog::Capabilities::default()
    }
    async fn catalog(
        &self,
        request: crate::catalog::CatalogRequest,
    ) -> Result<Vec<crate::catalog::CatalogEntry>, crate::catalog::CatalogError> {
        Err(crate::catalog::CatalogError::Unsupported(request.action()))
    }
    async fn connect(&self) -> Result<(), String>;
    async fn connect_detailed(&self) -> Result<(), crate::error::ExecutionError> {
        self.connect().await.map_err(Into::into)
    }
    async fn disconnect(&self) -> Result<(), crate::error::ExecutionError> {
        Err(crate::error::ExecutionError::Message(
            "Disconnect is not supported".into(),
        ))
    }
    async fn ping(&self) -> Result<u64, String>;
    async fn ping_detailed(&self) -> Result<u64, crate::error::ExecutionError> {
        self.ping().await.map_err(Into::into)
    }
    async fn execute(&self, query: &str) -> Result<ResultSet, String>;
    async fn execute_outcome(&self, query: &str) -> Result<crate::ExecutionOutcome, String> {
        self.execute(query).await.map(crate::ExecutionOutcome::Rows)
    }
    async fn execute_detailed(
        &self,
        query: &str,
    ) -> Result<crate::ExecutionOutcome, crate::error::ExecutionError> {
        self.execute_outcome(query).await.map_err(Into::into)
    }
    async fn execute_streamed(
        &self,
        _query: &str,
        _results: &mut crate::result_stream::ResultSink,
    ) -> Result<Option<crate::CommandAcknowledgment>, crate::error::ExecutionError> {
        Err(crate::error::ExecutionError::Unsupported(
            "Streaming query execution".into(),
        ))
    }
    async fn cancel(&self) -> Result<(), String>;
    async fn cancel_detailed(&self) -> Result<(), crate::error::ExecutionError> {
        self.cancel().await.map_err(Into::into)
    }
    async fn version(&self) -> Result<String, String>;
    async fn version_detailed(&self) -> Result<String, crate::error::ExecutionError> {
        self.version().await.map_err(Into::into)
    }
    async fn explain(&self, query: &str) -> Result<QueryPlan, String>;
    async fn explain_detailed(
        &self,
        query: &str,
    ) -> Result<QueryPlan, crate::error::ExecutionError> {
        self.explain(query).await.map_err(Into::into)
    }
}

pub trait AdapterFactory: Send + Sync {
    fn backend(&self) -> DatabaseBackend;
    fn create(&self, config: DatabaseConfig)
    -> Result<std::sync::Arc<dyn DatabaseSession>, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterError {
    Duplicate(DatabaseBackend),
    Unsupported(DatabaseBackend),
    Create {
        backend: DatabaseBackend,
        message: String,
    },
    Connect {
        backend: DatabaseBackend,
        error: crate::error::ExecutionError,
    },
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(backend) => {
                write!(formatter, "Adapter already registered for {backend:?}")
            }
            Self::Unsupported(backend) => {
                write!(formatter, "No adapter registered for {backend:?}")
            }
            Self::Create { message, .. } => formatter.write_str(message),
            Self::Connect { error, .. } => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for AdapterError {}

#[derive(Default)]
pub struct AdapterRegistry {
    factories: Vec<Arc<dyn AdapterFactory>>,
}

impl AdapterRegistry {
    pub fn register(&mut self, factory: Arc<dyn AdapterFactory>) -> Result<(), AdapterError> {
        let backend = factory.backend();
        if self
            .factories
            .iter()
            .any(|existing| existing.backend() == backend)
        {
            return Err(AdapterError::Duplicate(backend));
        }
        self.factories.push(factory);
        Ok(())
    }

    pub fn create(&self, config: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, AdapterError> {
        let backend = config.backend;
        let factory = self
            .factories
            .iter()
            .find(|factory| factory.backend() == backend)
            .ok_or(AdapterError::Unsupported(backend))?;
        factory
            .create(config)
            .map_err(|message| AdapterError::Create { backend, message })
    }

    pub async fn connect(
        &self,
        config: DatabaseConfig,
    ) -> Result<Arc<crate::execution::QueryEngine>, AdapterError> {
        let backend = config.backend;
        let session = self.create(config)?;
        session
            .connect_detailed()
            .await
            .map_err(|error| AdapterError::Connect { backend, error })?;
        Ok(Arc::new(crate::execution::QueryEngine::new(session)))
    }
}
