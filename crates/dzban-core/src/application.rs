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
    UnknownTab(crate::tabs::TabId),
    SelectionSuperseded,
    Operation(ExecutionError),
}

impl std::fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Adapter(error) => error.fmt(formatter),
            Self::UnknownSession(id) => write!(formatter, "Unknown session: {}", id.0),
            Self::UnknownTab(id) => write!(formatter, "Unknown tab: {}", id.0),
            Self::Operation(error) => error.fmt(formatter),
            Self::SelectionSuperseded => {
                formatter.write_str("A newer navigation request is active")
            }
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

type CloseReceiver = watch::Receiver<Option<Result<(), ApplicationError>>>;

pub struct CloseHandle {
    completion: CloseReceiver,
}

impl CloseHandle {
    pub async fn wait(mut self) -> Result<(), ApplicationError> {
        loop {
            if let Some(result) = self.completion.borrow_and_update().clone() {
                return result;
            }
            self.completion.changed().await.map_err(|_| {
                ApplicationError::Operation(ExecutionError::Message(
                    "Session cleanup tracking was dropped".into(),
                ))
            })?;
        }
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
    pub health: crate::health::HealthService,
    pub outputs: crate::outputs::OutputService,
    pub workspace: crate::workspace::WorkspaceService,
    adapters: AdapterRegistry,
    sessions: RwLock<HashMap<SessionId, Arc<QueryEngine>>>,
    pending_connections: watch::Sender<Vec<ConnectionSnapshot>>,
    closing_sessions: std::sync::Mutex<HashMap<SessionId, CloseReceiver>>,
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
            health: Default::default(),
            outputs: Default::default(),
            workspace: Default::default(),
            adapters,
            sessions: RwLock::new(HashMap::new()),
            pending_connections: watch::channel(Vec::new()).0,
            closing_sessions: Default::default(),
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
        self.start_connection(profile, None, false)
    }

    pub fn start_connect_tab(self: &Arc<Self>, profile: DatabaseConfig) -> ConnectionHandle {
        self.start_connection(profile, None, true)
    }

    pub fn open_tab(
        &self,
        profile: Option<crate::ConnectionProfileId>,
        draft: String,
    ) -> Result<crate::tabs::TabId, ApplicationError> {
        self.open_tab_at(profile, draft, None)
    }

    pub fn open_tab_at(
        &self,
        profile: Option<crate::ConnectionProfileId>,
        draft: String,
        revision: Option<u64>,
    ) -> Result<crate::tabs::TabId, ApplicationError> {
        let phase = self.phase.borrow();
        if *phase != ApplicationPhase::Running {
            return Err(ApplicationError::Operation(
                if *phase == ApplicationPhase::Closed {
                    ExecutionError::Closed
                } else {
                    ExecutionError::Busy
                },
            ));
        }
        self.tabs.open(profile, draft, revision)
    }

    pub fn start_connect_in_tab(
        self: &Arc<Self>,
        tab: crate::tabs::TabId,
        profile: DatabaseConfig,
    ) -> ConnectionHandle {
        self.start_connection(profile, Some(tab), false)
    }

    fn start_connection(
        self: &Arc<Self>,
        profile: DatabaseConfig,
        tab: Option<crate::tabs::TabId>,
        create_tab: bool,
    ) -> ConnectionHandle {
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
                tab,
                snapshot: receiver,
                task: tokio::spawn(async move { Err(error) }),
            };
        }
        let tab = if create_tab {
            Some(self.tabs.open(None, String::new(), None).unwrap())
        } else {
            tab
        };
        if let Some(tab_id) = tab
            && let Err(error) = self.tabs.begin(tab_id, id, &profile)
        {
            let (_, receiver) = watch::channel(ConnectionSnapshot {
                phase: ConnectionPhase::Failed(error.clone()),
                ..initial
            });
            return ConnectionHandle {
                id,
                tab,
                snapshot: receiver,
                task: tokio::spawn(async move { Err(error) }),
            };
        }
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
            let mut result = worker.await.map_err(worker_error).and_then(|result| result);
            if let Some(tab) = tab {
                match supervisor_context.tabs.complete(tab, id, &result) {
                    crate::tabs::ConnectionCompletion::Attached => {}
                    crate::tabs::ConnectionCompletion::Closing => {
                        result = Err(ApplicationError::Operation(ExecutionError::Closed));
                    }
                    crate::tabs::ConnectionCompletion::Superseded => {
                        result = match result {
                            Ok(engine) => supervisor_context
                                .close(engine.id())
                                .await
                                .and(Err(ApplicationError::Operation(ExecutionError::Closed))),
                            Err(error) => Err(error),
                        };
                    }
                }
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

    pub fn start_close_tab(
        self: &Arc<Self>,
        tab: crate::tabs::TabId,
    ) -> Result<CloseHandle, ApplicationError> {
        self.start_tab_cleanup(tab, false)
    }

    pub fn start_cancel_connection(
        self: &Arc<Self>,
        tab: crate::tabs::TabId,
    ) -> Result<CloseHandle, ApplicationError> {
        self.start_tab_cleanup(tab, true)
    }

    fn start_tab_cleanup(
        self: &Arc<Self>,
        tab: crate::tabs::TabId,
        retain: bool,
    ) -> Result<CloseHandle, ApplicationError> {
        let mut updates = self.tabs.subscribe();
        self.tabs.begin_close(tab, retain)?;
        let context = self.clone();
        let (completion, receiver) = watch::channel(None);
        tokio::spawn(async move {
            let result = async {
                loop {
                    let snapshot = updates.borrow_and_update().clone();
                    let Some(current) = snapshot.tabs.iter().find(|current| current.id == tab)
                    else {
                        return Ok(());
                    };
                    if let Some(engine) = &current.engine {
                        return match context.begin_close(engine.clone()) {
                            Ok(close) => close.wait().await,
                            Err(error) => {
                                context.tabs.close_failed(tab);
                                Err(ApplicationError::Operation(error))
                            }
                        };
                    }
                    if current.connection_request.is_none() {
                        let result = context.tabs.finish_unconnected_close(tab);
                        if result.is_err() {
                            context.tabs.close_failed(tab);
                        }
                        return result;
                    }
                    updates
                        .changed()
                        .await
                        .map_err(|_| ApplicationError::Operation(ExecutionError::Closed))?;
                }
            }
            .await;
            completion.send_replace(Some(result));
        });
        Ok(CloseHandle {
            completion: receiver,
        })
    }

    pub fn subscribe_pending_connections(&self) -> watch::Receiver<Vec<ConnectionSnapshot>> {
        self.pending_connections.subscribe()
    }

    pub fn attention(&self) -> &crate::attention::AttentionService {
        self.tabs.attention()
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
        self.attention().suspend(true);
        let context = self.clone();
        let worker = tokio::spawn(async move {
            context.workspace.drain().await;
            context.health.drain().await;
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
                let Ok(engine) = context.session(id).await else {
                    continue;
                };
                let outcome = match context.begin_close(engine) {
                    Ok(close) => close.wait().await,
                    Err(error) => Err(ApplicationError::Operation(error)),
                };
                if let Err(error) = outcome {
                    failure.get_or_insert(error);
                }
            }
            if failure.is_none() {
                context.tabs.finish_shutdown();
                context.outputs.clear();
            }
            failure.map_or(Ok(()), Err)
        });
        let context = self.clone();
        let task = tokio::spawn(async move {
            let result = worker.await.map_err(worker_error).and_then(|result| result);
            if result.is_err() {
                context.attention().suspend(false);
            }
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
        let mut tracked = self.closing_sessions.lock().unwrap();
        if let Some(completion) = tracked.get(&engine.id()) {
            return Ok(CloseHandle {
                completion: completion.clone(),
            });
        }
        let closing = if self.tabs.session(engine.id()).is_some() {
            engine.start_close_draining()?
        } else {
            engine.start_close()?
        };
        Ok(self.track_close_locked(closing, &mut tracked))
    }

    pub fn track_close(
        self: &Arc<Self>,
        closing: crate::execution::SessionCloseHandle,
    ) -> CloseHandle {
        self.track_close_locked(closing, &mut self.closing_sessions.lock().unwrap())
    }

    fn track_close_locked(
        self: &Arc<Self>,
        closing: crate::execution::SessionCloseHandle,
        tracked: &mut HashMap<SessionId, CloseReceiver>,
    ) -> CloseHandle {
        let id = closing.session;
        self.tabs.mark_closing(id);
        let tab = self.tabs.session(id).map(|tab| tab.id);
        let (completion, receiver) = watch::channel(None);
        tracked.insert(id, receiver.clone());
        let context = self.clone();
        let worker = tokio::spawn(async move {
            closing.wait().await.map_err(ApplicationError::Operation)?;
            let document = context.tabs.session(id).map(|tab| tab.document.id());
            context
                .tabs
                .retire(id)
                .map_err(ApplicationError::Operation)?;
            context.sessions.write().await.remove(&id);
            context.outputs.retire_session(id, document);
            context.health.forget_session(id);
            Ok(())
        });
        let context = self.clone();
        tokio::spawn(async move {
            let result = worker.await.map_err(worker_error).and_then(|result| result);
            if result.is_err()
                && let Some(tab) = tab
            {
                context.tabs.close_failed(tab);
            }
            let mut tracked = context.closing_sessions.lock().unwrap();
            completion.send_replace(Some(result));
            tracked.remove(&id);
        });
        CloseHandle {
            completion: receiver,
        }
    }
}
