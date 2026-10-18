use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, watch};

use crate::DatabaseConfig;
use crate::adapter::{AdapterError, AdapterRegistry};
use crate::error::ExecutionError;
use crate::execution::{QueryEngine, SessionId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplicationError {
    Adapter(AdapterError),
    UnknownSession(SessionId),
    Operation(ExecutionError),
}

impl std::fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Adapter(error) => error.fmt(formatter),
            Self::UnknownSession(id) => write!(formatter, "Unknown session: {}", id.0),
            Self::Operation(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ApplicationError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConnectionRequestId(pub uuid::Uuid);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionPhase {
    Connecting,
    Connected(SessionId),
    Failed(ApplicationError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionSnapshot {
    pub request: ConnectionRequestId,
    pub profile: crate::ConnectionProfileId,
    pub backend: crate::DatabaseBackend,
    pub phase: ConnectionPhase,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ConnectionSelection {
    #[default]
    Empty,
    Pending(ConnectionRequestId),
    Ready(SessionId),
    Closing(SessionId),
    Failed(ApplicationError),
}

impl ConnectionSelection {
    pub fn accepts_connection_request(&self) -> bool {
        !matches!(self, Self::Ready(_) | Self::Closing(_))
    }

    pub fn begin(&mut self, request: ConnectionRequestId) -> bool {
        if !self.accepts_connection_request() {
            return false;
        }
        *self = Self::Pending(request);
        true
    }

    pub fn apply(&mut self, snapshot: &ConnectionSnapshot) -> bool {
        if *self != Self::Pending(snapshot.request) {
            return false;
        }
        *self = match &snapshot.phase {
            ConnectionPhase::Connecting => Self::Pending(snapshot.request),
            ConnectionPhase::Connected(id) => Self::Ready(*id),
            ConnectionPhase::Failed(error) => Self::Failed(error.clone()),
        };
        true
    }

    pub fn clear(&mut self) {
        *self = Self::Empty;
    }

    pub fn can_forget(&self) -> bool {
        matches!(self, Self::Empty | Self::Failed(_))
    }

    pub fn begin_close(&mut self, session: SessionId) -> bool {
        if *self != Self::Ready(session) {
            return false;
        }
        *self = Self::Closing(session);
        true
    }

    pub fn finish_close(
        &mut self,
        session: SessionId,
        result: &Result<(), ExecutionError>,
    ) -> bool {
        if *self != Self::Closing(session) {
            return false;
        }
        *self = if result.is_ok() {
            Self::Empty
        } else {
            Self::Ready(session)
        };
        true
    }
}

pub struct ConnectionHandle {
    pub id: ConnectionRequestId,
    pub tab: Option<crate::tabs::TabId>,
    snapshot: watch::Receiver<ConnectionSnapshot>,
    task: tokio::task::JoinHandle<Result<Arc<QueryEngine>, ApplicationError>>,
}

pub struct CloseHandle {
    task: tokio::task::JoinHandle<Result<(), ApplicationError>>,
}

impl CloseHandle {
    pub async fn wait(self) -> Result<(), ApplicationError> {
        self.task.await.map_err(worker_error)?
    }
}

impl ConnectionHandle {
    pub fn subscribe(&self) -> watch::Receiver<ConnectionSnapshot> {
        self.snapshot.clone()
    }

    pub async fn wait(self) -> Result<Arc<QueryEngine>, ApplicationError> {
        self.task.await.map_err(worker_error)?
    }
}

fn worker_error(error: tokio::task::JoinError) -> ApplicationError {
    ApplicationError::Operation(ExecutionError::Message(format!(
        "Session worker failed: {error}"
    )))
}

pub struct ApplicationContext {
    pub tabs: crate::tabs::QueryTabs,
    adapters: AdapterRegistry,
    sessions: RwLock<HashMap<SessionId, Arc<QueryEngine>>>,
    pending_connections: watch::Sender<Vec<ConnectionSnapshot>>,
    phase: watch::Sender<ApplicationPhase>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationPhase {
    Running,
    Draining,
    Closed,
}

pub struct ShutdownHandle {
    task: tokio::task::JoinHandle<Result<(), ApplicationError>>,
}

impl ShutdownHandle {
    pub async fn wait(self) -> Result<(), ApplicationError> {
        self.task.await.map_err(worker_error)?
    }
}

impl ApplicationContext {
    pub fn new(adapters: AdapterRegistry) -> Self {
        Self {
            tabs: Default::default(),
            adapters,
            sessions: RwLock::new(HashMap::new()),
            pending_connections: watch::channel(Vec::new()).0,
            phase: watch::channel(ApplicationPhase::Running).0,
        }
    }

    pub async fn connect_profile(
        self: &Arc<Self>,
        profile: DatabaseConfig,
    ) -> Result<Arc<QueryEngine>, ApplicationError> {
        self.start_connect(profile).wait().await
    }

    pub fn start_connect(self: &Arc<Self>, profile: DatabaseConfig) -> ConnectionHandle {
        self.start_connection(profile, false)
    }

    pub fn start_connect_tab(self: &Arc<Self>, profile: DatabaseConfig) -> ConnectionHandle {
        self.start_connection(profile, true)
    }

    fn start_connection(self: &Arc<Self>, profile: DatabaseConfig, tab: bool) -> ConnectionHandle {
        let id = ConnectionRequestId(uuid::Uuid::new_v4());
        let initial = ConnectionSnapshot {
            request: id,
            profile: crate::ConnectionProfileId(profile.name.clone()),
            backend: profile.backend,
            phase: ConnectionPhase::Connecting,
        };
        let admission = self.phase.borrow();
        if *admission != ApplicationPhase::Running {
            let error = ApplicationError::Operation(if *admission == ApplicationPhase::Closed {
                ExecutionError::Closed
            } else {
                ExecutionError::Busy
            });
            let (snapshot, receiver) = watch::channel(ConnectionSnapshot {
                phase: ConnectionPhase::Failed(error.clone()),
                ..initial
            });
            drop(snapshot);
            return ConnectionHandle {
                id,
                tab: None,
                snapshot: receiver,
                task: tokio::spawn(async move { Err(error) }),
            };
        }
        let tab = tab.then(|| self.tabs.begin(&profile));
        self.pending_connections
            .send_modify(|pending| pending.push(initial.clone()));
        drop(admission);
        let (snapshot, receiver) = watch::channel(initial);
        let context = self.clone();
        let worker = tokio::spawn(async move {
            let engine = context
                .adapters
                .connect(profile)
                .await
                .map_err(ApplicationError::Adapter)?;
            context
                .sessions
                .write()
                .await
                .insert(engine.id(), engine.clone());
            Ok::<_, ApplicationError>(engine)
        });
        let supervisor_context = self.clone();
        let task = tokio::spawn(async move {
            let result = worker.await.map_err(worker_error).and_then(|result| result);
            if let Some(tab) = tab {
                supervisor_context.tabs.complete(tab, &result);
            }
            snapshot.send_modify(|state| {
                state.phase = match &result {
                    Ok(engine) => ConnectionPhase::Connected(engine.id()),
                    Err(error) => ConnectionPhase::Failed(error.clone()),
                };
            });
            supervisor_context
                .pending_connections
                .send_modify(|pending| pending.retain(|request| request.request != id));
            result
        });
        ConnectionHandle {
            id,
            tab,
            snapshot: receiver,
            task,
        }
    }

    pub fn subscribe_pending_connections(&self) -> watch::Receiver<Vec<ConnectionSnapshot>> {
        self.pending_connections.subscribe()
    }

    pub fn subscribe_phase(&self) -> watch::Receiver<ApplicationPhase> {
        self.phase.subscribe()
    }

    pub fn start_shutdown(self: &Arc<Self>) -> Result<ShutdownHandle, ApplicationError> {
        let mut previous = ApplicationPhase::Running;
        self.phase.send_modify(|phase| {
            previous = *phase;
            if previous == ApplicationPhase::Running {
                *phase = ApplicationPhase::Draining;
            }
        });
        if previous == ApplicationPhase::Draining {
            return Err(ApplicationError::Operation(ExecutionError::Busy));
        }
        if previous == ApplicationPhase::Closed {
            return Ok(ShutdownHandle {
                task: tokio::spawn(async { Ok(()) }),
            });
        }
        let context = self.clone();
        let worker = tokio::spawn(async move {
            let mut pending = context.subscribe_pending_connections();
            loop {
                if pending.borrow_and_update().is_empty() {
                    break;
                }
                pending.changed().await.map_err(|_| {
                    ApplicationError::Operation(ExecutionError::Message(
                        "Connection tracking was dropped".into(),
                    ))
                })?;
            }
            let mut failure = None;
            for id in context.session_ids().await {
                if let Err(error) = context.close(id).await {
                    failure.get_or_insert(error);
                }
            }
            if failure.is_none() {
                context.tabs.finish_shutdown();
            }
            failure.map_or(Ok(()), Err)
        });
        let context = self.clone();
        let task = tokio::spawn(async move {
            let result = worker.await.map_err(worker_error).and_then(|result| result);
            context.phase.send_replace(if result.is_ok() {
                ApplicationPhase::Closed
            } else {
                ApplicationPhase::Running
            });
            result
        });
        Ok(ShutdownHandle { task })
    }

    pub async fn session_ids(&self) -> Vec<SessionId> {
        let mut ids = self
            .sessions
            .read()
            .await
            .keys()
            .copied()
            .collect::<Vec<_>>();
        ids.sort_by_key(|id| id.0);
        ids
    }

    pub async fn session(&self, id: SessionId) -> Result<Arc<QueryEngine>, ApplicationError> {
        self.sessions
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or(ApplicationError::UnknownSession(id))
    }

    pub async fn close(self: &Arc<Self>, id: SessionId) -> Result<(), ApplicationError> {
        let engine = self.session(id).await?;
        self.begin_close(engine)
            .map_err(ApplicationError::Operation)?
            .wait()
            .await
    }

    pub fn begin_close(
        self: &Arc<Self>,
        engine: Arc<QueryEngine>,
    ) -> Result<CloseHandle, ExecutionError> {
        let closing = if self.tabs.session(engine.id()).is_some() {
            engine.start_close_draining()?
        } else {
            engine.start_close()?
        };
        Ok(self.track_close(closing))
    }

    pub fn track_close(
        self: &Arc<Self>,
        closing: crate::execution::SessionCloseHandle,
    ) -> CloseHandle {
        let id = closing.session;
        let context = self.clone();
        let task = tokio::spawn(async move {
            closing.wait().await.map_err(ApplicationError::Operation)?;
            context
                .tabs
                .retire(id)
                .map_err(ApplicationError::Operation)?;
            context.sessions.write().await.remove(&id);
            Ok(())
        });
        CloseHandle { task }
    }
}
