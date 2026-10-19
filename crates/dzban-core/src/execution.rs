use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use crate::adapter::DatabaseSession;
use crate::result_store::ResultLimits;
use crate::result_stream::{ResultSnapshot, start_results};
use crate::{QueryPlan, QueryResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryId(pub Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryScope {
    pub session: SessionId,
    pub query: Option<QueryId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    Open,
    Closing,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationState {
    Idle,
    Query(QueryId),
    Preview,
    Ping,
    Version,
    Catalog,
    Closing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryPublication {
    Idle,
    Pending(QueryId),
    Published(QueryId),
    Failed(QueryId, String),
}

impl HistoryPublication {
    pub fn error_for(&self, query: QueryId) -> Option<&str> {
        match self {
            Self::Failed(id, error) if *id == query => Some(error),
            _ => None,
        }
    }
}

struct TrackedOperation {
    _lock: tokio::sync::OwnedMutexGuard<()>,
    state: watch::Sender<OperationState>,
}

impl Drop for TrackedOperation {
    fn drop(&mut self) {
        self.state.send_replace(OperationState::Idle);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum QueryPhase {
    Idle,
    Queued(QueryId),
    Explaining(QueryId),
    Executing(QueryId),
    Canceling(QueryId),
    Completed(QueryId),
    Canceled(QueryId),
    Failed(QueryId, String),
}

impl QueryPhase {
    pub fn id(&self) -> Option<QueryId> {
        match self {
            Self::Idle => None,
            Self::Queued(id)
            | Self::Explaining(id)
            | Self::Executing(id)
            | Self::Canceling(id)
            | Self::Completed(id)
            | Self::Canceled(id)
            | Self::Failed(id, _) => Some(*id),
        }
    }
}

#[derive(Clone, Debug)]
pub struct QuerySnapshot {
    pub capture: Option<ResultSnapshot>,
    pub session: SessionId,
    pub phase: QueryPhase,
    pub plan: Option<QueryPlan>,
    pub explain_error: Option<String>,
    pub planning_error: Option<crate::error::ExecutionError>,
    pub execution_error: Option<crate::error::ExecutionError>,
    pub planning_duration: Duration,
    pub execution_duration: Duration,
    pub started_at: Option<Instant>,
    pub started_wall_time: Option<SystemTime>,
    pub finished_at: Option<Instant>,
}

impl QuerySnapshot {
    pub fn scope(&self) -> QueryScope {
        QueryScope {
            session: self.session,
            query: self.phase.id(),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.map_or(Duration::ZERO, |start| {
            self.finished_at
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(start)
        })
    }
}

pub struct QueryEngine {
    id: SessionId,
    session: Arc<dyn DatabaseSession>,
    operation: Arc<Mutex<()>>,
    operation_state: watch::Sender<OperationState>,
    cancellation: Arc<Mutex<()>>,
    snapshot: watch::Sender<QuerySnapshot>,
    session_phase: watch::Sender<SessionPhase>,
    history: watch::Sender<HistoryPublication>,
}

pub struct QueryHandle {
    pub session: SessionId,
    pub id: QueryId,
    task: tokio::task::JoinHandle<QueryCompletion>,
}

pub struct QueryCompletion {
    pub result: Result<QueryResult, crate::error::ExecutionError>,
    pub snapshot: QuerySnapshot,
    pub history: Option<HistoryPublication>,
    pub transaction_status: Option<crate::adapter::TransactionStatus>,
}

pub struct StreamedQueryHandle {
    pub session: SessionId,
    pub id: QueryId,
    pub results: watch::Receiver<ResultSnapshot>,
    task: tokio::task::JoinHandle<StreamedQueryCompletion>,
}

#[derive(Clone, Debug)]
pub struct StreamedQueryResult {
    pub capture: ResultSnapshot,
    pub command: Option<crate::CommandAcknowledgment>,
    pub duration: Duration,
}

#[derive(Clone, Debug)]
pub struct StreamedQueryCompletion {
    pub result: Result<StreamedQueryResult, crate::error::ExecutionError>,
    pub snapshot: QuerySnapshot,
    pub history: Option<HistoryPublication>,
    pub transaction_status: Option<crate::adapter::TransactionStatus>,
}

impl StreamedQueryHandle {
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub async fn wait_completion(
        self,
    ) -> Result<StreamedQueryCompletion, crate::error::ExecutionError> {
        self.task.await.map_err(|error| {
            crate::error::ExecutionError::Message(format!("Query worker failed: {error}"))
        })
    }
}

pub struct SessionCloseHandle {
    pub session: SessionId,
    task: tokio::task::JoinHandle<Result<(), crate::error::ExecutionError>>,
}

pub struct AuxiliaryHandle<T> {
    task: tokio::task::JoinHandle<Result<T, crate::error::ExecutionError>>,
}

impl<T> AuxiliaryHandle<T> {
    pub async fn wait(self) -> Result<T, crate::error::ExecutionError> {
        self.task.await.map_err(|error| {
            crate::error::ExecutionError::Message(format!("Database worker failed: {error}"))
        })?
    }
}

impl SessionCloseHandle {
    pub async fn wait(self) -> Result<(), crate::error::ExecutionError> {
        self.task.await.map_err(|error| {
            crate::error::ExecutionError::Message(format!("Disconnect worker failed: {error}"))
        })?
    }
}

impl QueryHandle {
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
    pub async fn wait(self) -> Result<QueryResult, String> {
        self.wait_detailed()
            .await
            .map_err(|error| error.to_string())
    }
    pub async fn wait_detailed(self) -> Result<QueryResult, crate::error::ExecutionError> {
        self.wait_completion().await?.result
    }

    pub async fn wait_completion(self) -> Result<QueryCompletion, crate::error::ExecutionError> {
        self.task.await.map_err(|error| {
            crate::error::ExecutionError::Message(format!("Query worker failed: {error}"))
        })
    }
}

impl QueryEngine {
    pub fn new(session: Arc<dyn DatabaseSession>) -> Self {
        let id = SessionId(Uuid::new_v4());
        let (snapshot, _) = watch::channel(QuerySnapshot {
            capture: None,
            session: id,
            phase: QueryPhase::Idle,
            plan: None,
            explain_error: None,
            planning_error: None,
            execution_error: None,
            planning_duration: Duration::ZERO,
            execution_duration: Duration::ZERO,
            started_at: None,
            started_wall_time: None,
            finished_at: None,
        });
        Self {
            id,
            session,
            operation: Arc::new(Mutex::new(())),
            cancellation: Arc::new(Mutex::new(())),
            snapshot,
            session_phase: watch::channel(SessionPhase::Open).0,
            operation_state: watch::channel(OperationState::Idle).0,
            history: watch::channel(HistoryPublication::Idle).0,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<QuerySnapshot> {
        self.snapshot.subscribe()
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn subscribe_session(&self) -> watch::Receiver<SessionPhase> {
        self.session_phase.subscribe()
    }

    pub fn subscribe_operation(&self) -> watch::Receiver<OperationState> {
        self.operation_state.subscribe()
    }

    fn track_operation(
        &self,
        lock: tokio::sync::OwnedMutexGuard<()>,
        state: OperationState,
    ) -> TrackedOperation {
        self.operation_state.send_replace(state);
        TrackedOperation {
            _lock: lock,
            state: self.operation_state.clone(),
        }
    }

    fn ensure_open(&self) -> Result<(), crate::error::ExecutionError> {
        match *self.session_phase.borrow() {
            SessionPhase::Open => Ok(()),
            SessionPhase::Closing => Err(crate::error::ExecutionError::Busy),
            SessionPhase::Closed => Err(crate::error::ExecutionError::Closed),
        }
    }

    pub async fn close(self: &Arc<Self>) -> Result<(), crate::error::ExecutionError> {
        self.start_close()?.wait().await
    }

    pub fn start_close(
        self: &Arc<Self>,
    ) -> Result<SessionCloseHandle, crate::error::ExecutionError> {
        let operation = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| crate::error::ExecutionError::Busy)?;
        if *self.session_phase.borrow() == SessionPhase::Closed {
            return Ok(SessionCloseHandle {
                session: self.id,
                task: tokio::spawn(async { Ok(()) }),
            });
        }
        self.ensure_open()?;
        self.session_phase.send_replace(SessionPhase::Closing);
        let operation = self.track_operation(operation, OperationState::Closing);
        let session = self.session.clone();
        let worker = tokio::spawn(async move {
            let _operation = operation;
            session.disconnect().await
        });
        let engine = self.clone();
        let task = tokio::spawn(async move {
            let result = worker.await.unwrap_or_else(|error| {
                Err(crate::error::ExecutionError::Message(format!(
                    "Disconnect worker failed: {error}"
                )))
            });
            engine.session_phase.send_replace(if result.is_ok() {
                SessionPhase::Closed
            } else {
                SessionPhase::Open
            });
            if result.is_ok() {
                engine
                    .snapshot
                    .send_modify(|snapshot| snapshot.capture = None);
            }
            result
        });
        Ok(SessionCloseHandle {
            session: self.id,
            task,
        })
    }

    pub fn start_close_draining(
        self: &Arc<Self>,
    ) -> Result<SessionCloseHandle, crate::error::ExecutionError> {
        let mut admitted = false;
        self.session_phase.send_modify(|phase| {
            if *phase == SessionPhase::Open {
                *phase = SessionPhase::Closing;
                admitted = true;
            }
        });
        if !admitted {
            return self.start_close();
        }
        let engine = self.clone();
        let task = tokio::spawn(async move {
            let worker = engine.clone();
            let result = tokio::spawn(async move {
                let mut snapshots = worker.subscribe();
                let operation = loop {
                    let phase = snapshots.borrow_and_update().phase.clone();
                    if let QueryPhase::Queued(id) | QueryPhase::Explaining(id) | QueryPhase::Executing(id) = phase {
                        worker.cancel_detailed(id).await?;
                    }
                    tokio::select! {
                        operation = worker.operation.clone().lock_owned() => break operation,
                        changed = snapshots.changed() => {
                            if changed.is_err() { return Err(crate::error::ExecutionError::Closed); }
                        }
                    }
                };
                let _operation = worker.track_operation(operation, OperationState::Closing);
                worker.session.disconnect().await
            }).await.unwrap_or_else(|error| Err(crate::error::ExecutionError::Message(format!("Disconnect worker failed: {error}"))));
            engine.session_phase.send_replace(if result.is_ok() {
                SessionPhase::Closed
            } else {
                SessionPhase::Open
            });
            if result.is_ok() {
                engine
                    .snapshot
                    .send_modify(|snapshot| snapshot.capture = None);
            }
            result
        });
        Ok(SessionCloseHandle {
            session: self.id,
            task,
        })
    }

    pub fn capabilities(&self) -> crate::catalog::Capabilities {
        self.session.capabilities()
    }

    pub fn action_registry(
        &self,
    ) -> Result<crate::action::ActionRegistry, crate::action::RegistryError> {
        let mut registry = crate::action::ActionRegistry::portable();
        for action in self.session.action_descriptors() {
            registry.register(action)?;
        }
        Ok(registry)
    }

    pub async fn preview(&self, sql: &str) -> Result<QueryPlan, String> {
        self.preview_detailed(sql)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn preview_detailed(
        &self,
        sql: &str,
    ) -> Result<QueryPlan, crate::error::ExecutionError> {
        self.start_preview(sql.to_owned())?.wait().await
    }

    pub fn start_preview(
        &self,
        sql: String,
    ) -> Result<AuxiliaryHandle<QueryPlan>, crate::error::ExecutionError> {
        let session = self.session.clone();
        self.start_auxiliary(OperationState::Preview, async move {
            if !session.capabilities().plain_explain {
                return Err(crate::error::ExecutionError::Unsupported(
                    "query.plan".into(),
                ));
            }
            session.explain_detailed(&sql).await
        })
    }

    pub async fn ping(&self) -> Result<u64, String> {
        self.ping_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn ping_detailed(&self) -> Result<u64, crate::error::ExecutionError> {
        self.start_ping()?.wait().await
    }

    pub fn start_ping(&self) -> Result<AuxiliaryHandle<u64>, crate::error::ExecutionError> {
        let session = self.session.clone();
        self.start_auxiliary(OperationState::Ping, async move {
            bounded("Ping", session.ping_detailed()).await
        })
    }

    pub async fn version(&self) -> Result<String, String> {
        self.version_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn version_detailed(&self) -> Result<String, crate::error::ExecutionError> {
        self.start_version()?.wait().await
    }

    pub fn start_version(&self) -> Result<AuxiliaryHandle<String>, crate::error::ExecutionError> {
        let session = self.session.clone();
        self.start_auxiliary(OperationState::Version, async move {
            bounded("Version check", session.version_detailed()).await
        })
    }

    fn start_auxiliary<T, F>(
        &self,
        state: OperationState,
        future: F,
    ) -> Result<AuxiliaryHandle<T>, crate::error::ExecutionError>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T, crate::error::ExecutionError>> + Send + 'static,
    {
        let operation = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| crate::error::ExecutionError::Busy)?;
        self.ensure_open()?;
        let operation = self.track_operation(operation, state);
        let task = tokio::spawn(async move {
            let _operation = operation;
            future.await
        });
        Ok(AuxiliaryHandle { task })
    }

    pub async fn describe_named(
        &self,
        name: String,
        detail: crate::catalog::Detail,
    ) -> Result<Vec<crate::catalog::CatalogEntry>, crate::catalog::CatalogError> {
        self.describe_matching(name, detail, None).await
    }

    pub async fn describe_kind(
        &self,
        name: String,
        kind: crate::catalog::ObjectKind,
        detail: crate::catalog::Detail,
    ) -> Result<Vec<crate::catalog::CatalogEntry>, crate::catalog::CatalogError> {
        self.describe_matching(name, detail, Some(kind)).await
    }

    async fn describe_matching(
        &self,
        name: String,
        detail: crate::catalog::Detail,
        selection: Option<crate::catalog::ObjectKind>,
    ) -> Result<Vec<crate::catalog::CatalogEntry>, crate::catalog::CatalogError> {
        use crate::catalog::{CatalogAction, CatalogError, CatalogRequest, ObjectKind};
        let operation = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| CatalogError::Busy)?;
        self.ensure_open().map_err(|error| CatalogError::Backend {
            message: error.to_string(),
            code: None,
        })?;
        let capabilities = self.capabilities();
        let session = self.session.clone();
        let operation = self.track_operation(operation, OperationState::Catalog);
        tokio::spawn(async move {
            let _operation = operation;
            let mut entries = Vec::new();
            let mut supported = false;
            for kind in [ObjectKind::Table, ObjectKind::View] {
                if selection.is_some_and(|selected| kind != selected) {
                    continue;
                }
                if capabilities.catalog.contains(&CatalogAction::List(kind))
                    && capabilities
                        .catalog
                        .contains(&CatalogAction::Describe(kind))
                {
                    supported = true;
                    entries.extend(
                        session
                            .catalog(CatalogRequest::Find {
                                kind,
                                name: name.clone(),
                            })
                            .await?,
                    );
                }
            }
            if !supported {
                return Err(CatalogError::Unsupported(CatalogAction::Describe(
                    selection.unwrap_or(ObjectKind::Table),
                )));
            }
            let object = crate::catalog::resolve_object(&entries, &name)?;
            session
                .catalog(CatalogRequest::Describe { object, detail })
                .await
        })
        .await
        .map_err(|error| CatalogError::Backend {
            message: format!("Catalog worker failed: {error}"),
            code: None,
        })?
    }

    pub async fn catalog(
        &self,
        request: crate::catalog::CatalogRequest,
    ) -> Result<Vec<crate::catalog::CatalogEntry>, crate::catalog::CatalogError> {
        let operation = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| crate::catalog::CatalogError::Busy)?;
        self.ensure_open()
            .map_err(|error| crate::catalog::CatalogError::Backend {
                message: error.to_string(),
                code: None,
            })?;
        if !self.capabilities().catalog.contains(&request.action()) {
            return Err(crate::catalog::CatalogError::Unsupported(request.action()));
        }
        let session = self.session.clone();
        let operation = self.track_operation(operation, OperationState::Catalog);
        tokio::spawn(async move {
            let _operation = operation;
            session.catalog(request).await
        })
        .await
        .map_err(|error| crate::catalog::CatalogError::Backend {
            message: format!("Catalog worker failed: {error}"),
            code: None,
        })?
    }

    pub fn start(self: &Arc<Self>, sql: String) -> Result<QueryHandle, String> {
        self.start_detailed(sql).map_err(|error| error.to_string())
    }

    pub fn start_detailed(
        self: &Arc<Self>,
        sql: String,
    ) -> Result<QueryHandle, crate::error::ExecutionError> {
        self.start_with_history(sql, None)
    }

    pub fn subscribe_history(&self) -> watch::Receiver<HistoryPublication> {
        self.history.subscribe()
    }

    pub fn start_streamed(
        self: &Arc<Self>,
        sql: String,
        limits: ResultLimits,
        history: Option<crate::history::QueryHistoryStore>,
    ) -> Result<StreamedQueryHandle, crate::error::ExecutionError> {
        let (id, operation) = self.begin_query()?;
        let (mut producer, results) = start_results(limits);
        self.snapshot
            .send_modify(|snapshot| snapshot.capture = Some(results.borrow().clone()));
        let mut updates = results.clone();
        let engine = self.clone();
        let relay = tokio::spawn(async move {
            loop {
                let capture = updates.borrow_and_update().clone();
                let finished = matches!(
                    capture.phase,
                    crate::result_stream::ResultPhase::Finished(_)
                );
                engine.snapshot.send_modify(|snapshot| {
                    if snapshot.phase.id() == Some(id)
                        && (finished
                            || !matches!(
                                snapshot.phase,
                                QueryPhase::Completed(_)
                                    | QueryPhase::Canceled(_)
                                    | QueryPhase::Failed(_, _)
                            ))
                    {
                        snapshot.capture = Some(capture);
                    }
                });
                if finished || updates.changed().await.is_err() {
                    break;
                }
            }
        });
        let engine = self.clone();
        let query_sql = sql.clone();
        let completed_results = results.clone();
        let worker = tokio::spawn(async move {
            let (outcome, duration) = match engine.plan(id, &query_sql).await {
                Ok(()) => {
                    let start = Instant::now();
                    let outcome = engine
                        .session
                        .execute_streamed(&query_sql, &mut producer)
                        .await;
                    (outcome, start.elapsed())
                }
                Err(error) => (Err(error), Duration::ZERO),
            };
            let outcome = engine
                .finish_execution(
                    id,
                    outcome,
                    duration,
                    Some((&mut producer, &completed_results)),
                )
                .await;
            outcome.map(|(command, duration)| StreamedQueryResult {
                capture: completed_results.borrow().clone(),
                command,
                duration,
            })
        });
        if history.is_some() {
            self.history.send_replace(HistoryPublication::Pending(id));
        }
        let engine = self.clone();
        let task = tokio::spawn(async move {
            let _operation = operation;
            let result = match worker.await {
                Ok(result) => result,
                Err(error) => Err(engine.worker_failed(id, error)),
            };
            let _ = relay.await;
            let snapshot = engine.snapshot.borrow().clone();
            let transaction_status = engine.transaction_status();
            let publication = if let Some(history) = history {
                match crate::history::QueryHistoryEntry::from_streamed(sql, &result, &snapshot)
                    .await
                {
                    Ok(entry) => Some(engine.publish_history(id, history, entry).await),
                    Err(error) => {
                        let publication = HistoryPublication::Failed(id, error.to_string());
                        engine.history.send_replace(publication.clone());
                        Some(publication)
                    }
                }
            } else {
                None
            };
            StreamedQueryCompletion {
                result,
                snapshot,
                history: publication,
                transaction_status,
            }
        });
        Ok(StreamedQueryHandle {
            session: self.id,
            id,
            results,
            task,
        })
    }

    pub fn start_recorded_detailed(
        self: &Arc<Self>,
        sql: String,
        history: crate::history::QueryHistoryStore,
    ) -> Result<QueryHandle, crate::error::ExecutionError> {
        self.start_with_history(sql, Some(history))
    }

    fn start_with_history(
        self: &Arc<Self>,
        sql: String,
        history: Option<crate::history::QueryHistoryStore>,
    ) -> Result<QueryHandle, crate::error::ExecutionError> {
        let (id, operation) = self.begin_query()?;
        let engine = self.clone();
        let query_sql = sql.clone();
        let worker = tokio::spawn(async move {
            engine
                .run(id, &query_sql, engine.session.execute_detailed(&query_sql))
                .await
                .map(|(outcome, duration)| {
                    let (result_set, command) = outcome.into_parts();
                    QueryResult {
                        result_set,
                        command,
                        duration,
                    }
                })
        });
        if history.is_some() {
            self.history.send_replace(HistoryPublication::Pending(id));
        }
        let engine = self.clone();
        let task = tokio::spawn(async move {
            let _operation = operation;
            let result = match worker.await {
                Ok(result) => result,
                Err(error) => Err(engine.worker_failed(id, error)),
            };
            let snapshot = engine.snapshot.borrow().clone();
            let transaction_status = engine.transaction_status();
            let history = if let Some(history) = history {
                let entry =
                    crate::history::QueryHistoryEntry::from_execution(sql, &result, &snapshot);
                Some(engine.publish_history(id, history, entry).await)
            } else {
                None
            };
            QueryCompletion {
                result,
                snapshot,
                history,
                transaction_status,
            }
        });
        Ok(QueryHandle {
            session: self.id,
            id,
            task,
        })
    }

    fn begin_query(&self) -> Result<(QueryId, TrackedOperation), crate::error::ExecutionError> {
        let operation = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| crate::error::ExecutionError::Busy)?;
        self.ensure_open()?;
        let id = QueryId(Uuid::new_v4());
        let operation = self.track_operation(operation, OperationState::Query(id));
        self.snapshot.send_replace(QuerySnapshot {
            capture: None,
            session: self.id,
            phase: QueryPhase::Queued(id),
            plan: None,
            explain_error: None,
            planning_error: None,
            execution_error: None,
            planning_duration: Duration::ZERO,
            execution_duration: Duration::ZERO,
            started_at: Some(Instant::now()),
            started_wall_time: Some(SystemTime::now()),
            finished_at: None,
        });
        Ok((id, operation))
    }

    fn worker_failed(
        &self,
        id: QueryId,
        error: tokio::task::JoinError,
    ) -> crate::error::ExecutionError {
        let error = format!("Query worker failed: {error}");
        self.snapshot.send_modify(|snapshot| {
            if snapshot.phase.id() == Some(id) {
                snapshot.phase = QueryPhase::Failed(id, error.clone());
                snapshot.execution_error = Some(error.clone().into());
                snapshot.finished_at = Some(Instant::now());
            }
        });
        error.into()
    }

    fn transaction_status(&self) -> Option<crate::adapter::TransactionStatus> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.session.transaction_status()
        }))
        .unwrap_or_else(|_| {
            tracing::warn!("Adapter transaction status failed");
            None
        })
    }

    async fn publish_history(
        &self,
        id: QueryId,
        mut history: crate::history::QueryHistoryStore,
        entry: crate::history::QueryHistoryEntry,
    ) -> HistoryPublication {
        let published = tokio::task::spawn_blocking(move || history.add(entry)).await;
        let publication = match published {
            Ok(Ok(())) => HistoryPublication::Published(id),
            Ok(Err(error)) => HistoryPublication::Failed(id, error),
            Err(error) => HistoryPublication::Failed(id, format!("History worker failed: {error}")),
        };
        self.history.send_replace(publication.clone());
        publication
    }

    pub async fn execute(self: &Arc<Self>, sql: &str) -> Result<QueryResult, String> {
        self.start(sql.to_owned())?.wait().await
    }

    pub async fn execute_detailed(
        self: &Arc<Self>,
        sql: &str,
    ) -> Result<QueryResult, crate::error::ExecutionError> {
        self.start_detailed(sql.to_owned())?.wait_detailed().await
    }

    async fn run<T>(
        &self,
        id: QueryId,
        sql: &str,
        execution: impl std::future::Future<Output = Result<T, crate::error::ExecutionError>>,
    ) -> Result<(T, Duration), crate::error::ExecutionError> {
        self.plan(id, sql).await?;
        let start = Instant::now();
        let result = execution.await;
        self.finish_execution(id, result, start.elapsed(), None)
            .await
    }

    async fn plan(&self, id: QueryId, sql: &str) -> Result<(), crate::error::ExecutionError> {
        {
            let _cancel = self.cancellation.lock().await;
            if self.snapshot.borrow().phase == QueryPhase::Canceled(id) {
                return Err(crate::error::ExecutionError::Canceled);
            }
            self.snapshot
                .send_modify(|snapshot| snapshot.phase = QueryPhase::Explaining(id));
        }
        let planning_start = Instant::now();
        let plan = if self.capabilities().plain_explain {
            self.session.explain_detailed(sql).await
        } else {
            Err(crate::error::ExecutionError::Unsupported(
                "query.plan".into(),
            ))
        };
        let planning_duration = planning_start.elapsed();
        {
            let _cancel = self.cancellation.lock().await;
            self.snapshot.send_modify(|snapshot| {
                snapshot.planning_duration = planning_duration;
                match plan {
                    Ok(plan) => snapshot.plan = Some(plan),
                    Err(error) => {
                        snapshot.explain_error = Some(error.to_string());
                        snapshot.planning_error = Some(error);
                    }
                }
            });
            if self.snapshot.borrow().phase == QueryPhase::Canceling(id) {
                self.snapshot.send_modify(|snapshot| {
                    snapshot.phase = QueryPhase::Canceled(id);
                    snapshot.execution_error = Some(crate::error::ExecutionError::Canceled);
                    snapshot.finished_at = Some(Instant::now());
                });
                return Err(crate::error::ExecutionError::Canceled);
            }
            self.snapshot.send_modify(|snapshot| {
                snapshot.phase = QueryPhase::Executing(id);
            });
        }
        Ok(())
    }

    async fn finish_execution<T>(
        &self,
        id: QueryId,
        mut result: Result<T, crate::error::ExecutionError>,
        execution_duration: Duration,
        capture: Option<(
            &mut crate::result_stream::ResultSink,
            &watch::Receiver<ResultSnapshot>,
        )>,
    ) -> Result<(T, Duration), crate::error::ExecutionError> {
        let _cancel = self.cancellation.lock().await;
        let canceled = matches!(self.snapshot.borrow().phase, QueryPhase::Canceling(active) | QueryPhase::Canceled(active) if active == id)
            || matches!(result, Err(crate::error::ExecutionError::Canceled));
        if canceled {
            result = Err(crate::error::ExecutionError::Canceled);
        }
        let capture = if let Some((producer, reader)) = capture {
            if let Err(error) = producer
                .finish(result.as_ref().map(|_| ()).map_err(Clone::clone))
                .await
            {
                result = Err(error);
            }
            let capture = reader.borrow().clone();
            if result.is_ok()
                && let crate::result_stream::ResultPhase::Finished(
                    crate::result_stream::ResultTermination::Failed(error),
                ) = &capture.phase
            {
                result = Err(error.clone());
            }
            Some(capture)
        } else {
            None
        };
        self.snapshot.send_modify(|snapshot| {
            if capture.is_some() {
                snapshot.capture = capture;
            }
            snapshot.finished_at = Some(Instant::now());
            snapshot.execution_error = result
                .as_ref()
                .err()
                .cloned()
                .or_else(|| canceled.then_some(crate::error::ExecutionError::Canceled));
            snapshot.execution_duration = execution_duration;
            snapshot.phase = if canceled {
                QueryPhase::Canceled(id)
            } else {
                match &result {
                    Ok(_) => QueryPhase::Completed(id),
                    Err(error) => QueryPhase::Failed(id, error.to_string()),
                }
            };
        });
        if canceled {
            return Err(crate::error::ExecutionError::Canceled);
        }
        result.map(|outcome| (outcome, execution_duration))
    }

    pub async fn cancel(&self, id: QueryId) -> Result<(), String> {
        self.cancel_detailed(id)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn cancel_detailed(&self, id: QueryId) -> Result<(), crate::error::ExecutionError> {
        let cancellation = self.cancellation.clone().lock_owned().await;
        let phase = self.snapshot.borrow().phase.clone();
        if phase == QueryPhase::Queued(id) {
            self.snapshot.send_modify(|snapshot| {
                snapshot.phase = QueryPhase::Canceled(id);
                snapshot.execution_error = Some(crate::error::ExecutionError::Canceled);
                snapshot.finished_at = Some(Instant::now());
            });
            return Ok(());
        }
        if !matches!(phase, QueryPhase::Explaining(active) | QueryPhase::Executing(active) if active == id)
        {
            return Ok(());
        }
        if !self.capabilities().cancellation {
            return Err(crate::error::ExecutionError::Unsupported(
                "query.cancel".into(),
            ));
        }
        self.snapshot
            .send_modify(|snapshot| snapshot.phase = QueryPhase::Canceling(id));
        let session = self.session.clone();
        let worker = tokio::spawn(async move { session.cancel_detailed().await });
        let snapshot = self.snapshot.clone();
        tokio::spawn(async move {
            let _cancellation = cancellation;
            let result = worker.await.unwrap_or_else(|error| {
                Err(crate::error::ExecutionError::Message(format!(
                    "Cancellation worker failed: {error}"
                )))
            });
            if result.is_err() {
                snapshot.send_modify(|snapshot| {
                    if snapshot.phase == QueryPhase::Canceling(id) {
                        snapshot.phase = phase;
                    }
                });
            }
            result
        })
        .await
        .map_err(|error| {
            crate::error::ExecutionError::Message(format!(
                "Cancellation supervisor failed: {error}"
            ))
        })?
    }
}

pub const AUXILIARY_TIMEOUT: Duration = Duration::from_secs(5);

async fn bounded<T>(
    name: &str,
    operation: impl std::future::Future<Output = Result<T, crate::error::ExecutionError>>,
) -> Result<T, crate::error::ExecutionError> {
    tokio::time::timeout(AUXILIARY_TIMEOUT, operation)
        .await
        .unwrap_or_else(|_| {
            Err(crate::error::ExecutionError::Message(format!(
                "{name} timed out after {} seconds",
                AUXILIARY_TIMEOUT.as_secs()
            )))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatabaseBackend, QueryEstimate, ResultSet};
    use async_trait::async_trait;
    use tokio::sync::Notify;

    #[test]
    fn elapsed_time_is_zero_before_start_and_frozen_after_finish() {
        let engine = QueryEngine::new(Arc::new(Session::default()));
        let mut snapshot = engine.subscribe().borrow().clone();
        assert_eq!(snapshot.elapsed(), Duration::ZERO);
        let start = Instant::now() - Duration::from_secs(3);
        snapshot.started_at = Some(start);
        assert!(snapshot.elapsed() >= Duration::from_secs(3));
        snapshot.finished_at = Some(start + Duration::from_secs(2));
        assert_eq!(snapshot.elapsed(), Duration::from_secs(2));
        assert_eq!(snapshot.elapsed(), Duration::from_secs(2));
    }

    #[derive(Default)]
    enum DisconnectBehavior {
        #[default]
        Complete,
        Wait,
        Fail,
        Panic,
    }

    #[derive(Default)]
    struct Session {
        started: Notify,
        canceled: Notify,
        disconnect_behavior: DisconnectBehavior,
        disconnect_started: Notify,
        disconnect_finished: Notify,
        executions: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl DatabaseSession for Session {
        async fn disconnect(&self) -> Result<(), crate::error::ExecutionError> {
            match self.disconnect_behavior {
                DisconnectBehavior::Complete => {}
                DisconnectBehavior::Wait => {
                    self.disconnect_started.notify_one();
                    self.disconnect_finished.notified().await;
                }
                DisconnectBehavior::Fail => return Err("Disconnect failed".to_string().into()),
                DisconnectBehavior::Panic => panic!("Disconnect worker panic"),
            }
            Ok(())
        }
        async fn connect(&self) -> Result<(), String> {
            Ok(())
        }
        async fn ping(&self) -> Result<u64, String> {
            Ok(1)
        }
        async fn version(&self) -> Result<String, String> {
            Ok("test".into())
        }
        async fn explain(&self, sql: &str) -> Result<QueryPlan, String> {
            if sql == "plan_wait" {
                self.started.notify_one();
                self.canceled.notified().await;
                return Err("planning canceled by server".into());
            }
            Ok(QueryPlan {
                backend: DatabaseBackend::Postgresql,
                document: serde_json::json!([]),
                estimate: QueryEstimate::default(),
            })
        }
        async fn execute(&self, sql: &str) -> Result<ResultSet, String> {
            assert_ne!(sql, "panic", "test worker panic");
            self.executions
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if sql == "wait" {
                self.started.notify_one();
                self.canceled.notified().await;
                return Err("server cancellation".into());
            }
            Ok(ResultSet::default())
        }
        async fn cancel(&self) -> Result<(), String> {
            self.canceled.notify_one();
            Ok(())
        }
    }

    #[tokio::test]
    async fn dropping_a_preview_observer_keeps_admission_until_backend_completion() {
        let session = Arc::new(Session::default());
        let engine = Arc::new(QueryEngine::new(session.clone()));
        let observer_engine = engine.clone();
        let observer = tokio::spawn(async move { observer_engine.preview("plan_wait").await });
        session.started.notified().await;
        observer.abort();
        assert!(observer.await.unwrap_err().is_cancelled());
        assert!(engine.start("overlap".into()).is_err());
        assert_eq!(engine.ping().await.unwrap_err(), "Connection is busy");
        session.canceled.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if engine.ping().await.is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        engine.execute("restart").await.unwrap();
    }

    #[tokio::test]
    async fn snapshots_and_handles_identify_their_session() {
        let first = Arc::new(QueryEngine::new(Arc::new(Session::default())));
        let second = Arc::new(QueryEngine::new(Arc::new(Session::default())));
        assert_ne!(first.id(), second.id());
        let query = first.start("select".into()).unwrap();
        assert_eq!(query.session, first.id());
        assert_eq!(first.subscribe().borrow().session, first.id());
        second.cancel(query.id).await.unwrap();
        assert_eq!(second.subscribe().borrow().phase, QueryPhase::Idle);
        query.wait().await.unwrap();
    }

    #[tokio::test]
    async fn failed_worker_is_terminal_and_can_restart() {
        let engine = Arc::new(QueryEngine::new(Arc::new(Session::default())));
        let query = engine.start("panic".into()).unwrap();
        let id = query.id;
        assert!(
            query
                .wait()
                .await
                .unwrap_err()
                .contains("Query worker failed")
        );
        assert!(
            matches!(engine.subscribe().borrow().phase, QueryPhase::Failed(active, _) if active == id)
        );
        engine.execute("restart").await.unwrap();
    }

    #[tokio::test]
    async fn immediate_cancel_has_an_id_and_never_reaches_the_database() {
        let session = Arc::new(Session::default());
        let engine = Arc::new(QueryEngine::new(session.clone()));
        let query = engine.start("select".into()).unwrap();
        let id = query.id;
        assert_eq!(engine.subscribe().borrow().phase, QueryPhase::Queued(id));
        assert!(matches!(
            engine.start_detailed("overlap".into()),
            Err(crate::error::ExecutionError::Busy)
        ));
        engine.cancel(id).await.unwrap();
        assert_eq!(
            query.wait_detailed().await.unwrap_err(),
            crate::error::ExecutionError::Canceled
        );
        assert!(engine.subscribe().borrow().finished_at.is_some());
        assert_eq!(
            session
                .executions
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        engine.execute("restart").await.unwrap();
    }

    #[tokio::test]
    async fn closing_a_session_is_idle_only_and_blocks_subsequent_operations() {
        let session = Arc::new(Session::default());
        let engine = Arc::new(QueryEngine::new(session.clone()));
        let phases = engine.subscribe_session();
        let query = engine.start_detailed("wait".into()).unwrap();
        session.started.notified().await;
        assert_eq!(
            engine.close().await.unwrap_err(),
            crate::error::ExecutionError::Busy
        );
        assert_eq!(*phases.borrow(), SessionPhase::Open);
        engine.cancel(query.id).await.unwrap();
        assert_eq!(
            query.wait_detailed().await.unwrap_err(),
            crate::error::ExecutionError::Canceled
        );
        engine.close().await.unwrap();
        assert_eq!(*phases.borrow(), SessionPhase::Closed);
        engine.close().await.unwrap();
        assert!(matches!(
            engine.start_detailed("SELECT 1".into()),
            Err(crate::error::ExecutionError::Closed)
        ));
        assert_eq!(engine.ping().await.unwrap_err(), "Session is closed");
        assert_eq!(
            engine.preview("SELECT 1").await.unwrap_err(),
            "Session is closed"
        );
        assert_eq!(engine.version().await.unwrap_err(), "Session is closed");
    }

    #[tokio::test]
    async fn dropping_the_close_waiter_does_not_reopen_a_closing_session() {
        let session = Arc::new(Session {
            disconnect_behavior: DisconnectBehavior::Wait,
            ..Session::default()
        });
        let engine = Arc::new(QueryEngine::new(session.clone()));
        let mut phase = engine.subscribe_session();
        let observer_engine = engine.clone();
        let observer = tokio::spawn(async move { observer_engine.close().await });
        session.disconnect_started.notified().await;
        assert_eq!(*phase.borrow(), SessionPhase::Closing);
        observer.abort();
        assert!(observer.await.unwrap_err().is_cancelled());
        assert!(matches!(
            engine.start_detailed("overlap".into()),
            Err(crate::error::ExecutionError::Busy)
        ));
        session.disconnect_finished.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if *phase.borrow_and_update() == SessionPhase::Closed {
                    break;
                }
                phase.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            engine.start_detailed("after close".into()),
            Err(crate::error::ExecutionError::Closed)
        ));
    }

    #[tokio::test]
    async fn disconnect_failure_and_worker_panic_restore_operation_admission() {
        for disconnect_behavior in [DisconnectBehavior::Fail, DisconnectBehavior::Panic] {
            let engine = Arc::new(QueryEngine::new(Arc::new(Session {
                disconnect_behavior,
                ..Session::default()
            })));
            assert!(engine.close().await.is_err());
            assert_eq!(*engine.subscribe_session().borrow(), SessionPhase::Open);
            engine.execute_detailed("restart").await.unwrap();
        }
    }

    #[tokio::test]
    async fn dropping_the_observer_does_not_release_a_running_connection() {
        let session = Arc::new(Session::default());
        let engine = Arc::new(QueryEngine::new(session.clone()));
        let query = engine.start("wait".into()).unwrap();
        let id = query.id;
        drop(query);
        session.started.notified().await;
        assert!(engine.start("overlap".into()).is_err());
        engine.cancel(id).await.unwrap();
        let mut state = engine.subscribe();
        while state.borrow().phase != QueryPhase::Canceled(id) {
            state.changed().await.unwrap();
        }
        engine.execute("restart").await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_during_planning_never_executes_sql_and_allows_restart() {
        let session = Arc::new(Session::default());
        let engine = Arc::new(QueryEngine::new(session.clone()));
        let worker = engine.clone();
        let result = tokio::spawn(async move { worker.execute("plan_wait").await });
        session.started.notified().await;
        let state = engine.subscribe();
        let QueryPhase::Explaining(id) = state.borrow().phase else {
            panic!("not planning")
        };
        engine.cancel(id).await.unwrap();
        assert_eq!(result.await.unwrap().unwrap_err(), "Query cancelled");
        assert_eq!(state.borrow().phase, QueryPhase::Canceled(id));
        assert!(state.borrow().explain_error.is_some());
        assert_eq!(
            session
                .executions
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        engine.execute("restart").await.unwrap();
        assert_eq!(
            session
                .executions
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[tokio::test]
    async fn plan_and_terminal_outcome_are_available_to_new_subscribers() {
        let engine = Arc::new(QueryEngine::new(Arc::new(Session::default())));
        engine.execute("select").await.unwrap();
        let state = engine.subscribe();
        assert!(matches!(state.borrow().phase, QueryPhase::Completed(_)));
        assert!(state.borrow().plan.is_some());
        assert!(state.borrow().started_at.is_some());
        assert!(state.borrow().started_wall_time.is_some());
        assert!(state.borrow().finished_at.is_some());
        let elapsed = state.borrow().elapsed();
        tokio::task::yield_now().await;
        assert_eq!(state.borrow().elapsed(), elapsed);
    }

    #[tokio::test]
    async fn cancellation_is_query_scoped_and_busy_operations_are_rejected() {
        let session = Arc::new(Session::default());
        let engine = Arc::new(QueryEngine::new(session.clone()));
        let worker = engine.clone();
        let result = tokio::spawn(async move { worker.execute("wait").await });
        session.started.notified().await;
        let snapshot = engine.subscribe();
        let QueryPhase::Executing(id) = snapshot.borrow().phase else {
            panic!("query not executing")
        };
        assert_eq!(
            engine.execute("other").await.unwrap_err(),
            "Connection is busy"
        );
        assert_eq!(
            engine.preview("select").await.unwrap_err(),
            "Connection is busy"
        );
        assert_eq!(engine.ping().await.unwrap_err(), "Connection is busy");
        assert_eq!(engine.version().await.unwrap_err(), "Connection is busy");
        let catalog = engine
            .catalog(crate::catalog::CatalogRequest::List {
                kind: crate::catalog::ObjectKind::Table,
                database: None,
                schema: None,
                glob: None,
                visibility: crate::catalog::Visibility::UserObjects,
            })
            .await;
        assert_eq!(catalog, Err(crate::catalog::CatalogError::Busy));
        engine.cancel(QueryId(Uuid::new_v4())).await.unwrap();
        assert_eq!(snapshot.borrow().phase, QueryPhase::Executing(id));
        engine.cancel(id).await.unwrap();
        assert!(result.await.unwrap().is_err());
        assert_eq!(snapshot.borrow().phase, QueryPhase::Canceled(id));
        engine.execute("restart").await.unwrap();
        engine.cancel(id).await.unwrap();
        assert!(matches!(snapshot.borrow().phase, QueryPhase::Completed(_)));
    }
}
