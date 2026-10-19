use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::application::{ApplicationContext, ApplicationError, ApplicationPhase};
use crate::error::ExecutionError;
use crate::execution::{QueryEngine, SessionId, SessionPhase};
use crate::{ConnectionProfileId, DatabaseConfig};

pub const HEALTH_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    #[default]
    Unknown,
    Connected,
    Error,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthStatus {
    pub state: HealthState,
    pub probing: bool,
    pub latency_micros: Option<u64>,
    pub version: Option<String>,
    pub error: Option<String>,
    pub checked_at_millis: Option<u64>,
}

impl HealthStatus {
    fn complete(&mut self, result: &ProbeResult) {
        self.probing = false;
        match result {
            Ok(result) if result.state == ProbeState::Busy => (),
            Ok(result) => {
                self.state = match result.state {
                    ProbeState::Connected => HealthState::Connected,
                    ProbeState::Error => HealthState::Error,
                    ProbeState::Busy => unreachable!(),
                };
                self.latency_micros = result.latency_micros;
                if let Some(version) = &result.version {
                    self.version = Some(version.clone());
                }
                self.error = result.error.clone();
                self.checked_at_millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .and_then(|duration| u64::try_from(duration.as_millis()).ok());
            }
            Err(error) => {
                self.state = HealthState::Error;
                self.latency_micros = None;
                self.error = Some(error.to_string());
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHealth {
    pub session: uuid::Uuid,
    pub status: HealthStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileHealth {
    pub profile: String,
    pub status: HealthStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub revision: u64,
    pub sessions: Vec<SessionHealth>,
    pub profiles: Vec<ProfileHealth>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Connected,
    Busy,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProbe {
    pub state: ProbeState,
    pub latency_micros: Option<u64>,
    pub version: Option<String>,
    pub error: Option<String>,
}

impl ConnectionProbe {
    fn busy() -> Self {
        Self {
            state: ProbeState::Busy,
            latency_micros: None,
            version: None,
            error: None,
        }
    }
    fn failed(error: impl std::fmt::Display) -> Self {
        Self {
            state: ProbeState::Error,
            latency_micros: None,
            version: None,
            error: Some(error.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ProbeTarget {
    Session(SessionId, bool),
    Profile(ConnectionProfileId),
}

type ProbeResult = Result<ConnectionProbe, ApplicationError>;
type ProbeReceiver = watch::Receiver<Option<ProbeResult>>;

#[derive(Default)]
struct HealthData {
    pending: HashMap<ProbeTarget, ProbeReceiver>,
    sessions: BTreeMap<uuid::Uuid, HealthStatus>,
    profiles: BTreeMap<String, HealthStatus>,
    profiles_by_session: HashMap<SessionId, ConnectionProfileId>,
    monitored: HashSet<SessionId>,
    revision: u64,
}

#[derive(Clone)]
pub struct HealthService {
    data: Arc<Mutex<HealthData>>,
    snapshots: watch::Sender<HealthSnapshot>,
}

impl Default for HealthService {
    fn default() -> Self {
        Self {
            data: Default::default(),
            snapshots: watch::channel(HealthSnapshot::default()).0,
        }
    }
}

pub struct ProbeHandle {
    result: ProbeReceiver,
}

impl ProbeHandle {
    fn ready(result: ProbeResult) -> Self {
        Self {
            result: watch::channel(Some(result)).1,
        }
    }
    pub async fn wait(mut self) -> ProbeResult {
        loop {
            if let Some(result) = self.result.borrow_and_update().clone() {
                return result;
            }
            self.result.changed().await.map_err(|_| {
                ApplicationError::Operation(ExecutionError::Message(
                    "Probe tracking was dropped".into(),
                ))
            })?;
        }
    }
}

impl HealthService {
    pub fn snapshot(&self) -> HealthSnapshot {
        self.snapshots.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<HealthSnapshot> {
        self.snapshots.subscribe()
    }

    fn publish(&self, data: &mut HealthData) {
        data.revision = data.revision.checked_add(1).unwrap();
        self.snapshots.send_replace(HealthSnapshot {
            revision: data.revision,
            sessions: data
                .sessions
                .iter()
                .map(|(session, status)| SessionHealth {
                    session: *session,
                    status: status.clone(),
                })
                .collect(),
            profiles: data
                .profiles
                .iter()
                .map(|(profile, status)| ProfileHealth {
                    profile: profile.clone(),
                    status: status.clone(),
                })
                .collect(),
        });
    }

    pub fn track_tab(
        &self,
        context: &Arc<ApplicationContext>,
        tab: crate::tabs::TabId,
    ) -> Result<(), ApplicationError> {
        if *context.subscribe_phase().borrow() != ApplicationPhase::Running {
            return Err(ApplicationError::Operation(ExecutionError::Closed));
        }
        let target = context
            .tabs
            .find(tab)
            .ok_or(ApplicationError::UnknownTab(tab))?;
        let engine = target
            .engine
            .ok_or(ApplicationError::Operation(ExecutionError::Closed))?;
        if *engine.subscribe_session().borrow() != SessionPhase::Open {
            return Err(ApplicationError::Operation(ExecutionError::Closed));
        }
        let profile = target
            .profile
            .ok_or(ApplicationError::Operation(ExecutionError::Closed))?;
        let session = engine.id();
        let mut data = self.data.lock().unwrap();
        if !data.monitored.insert(session) {
            return Ok(());
        }
        data.profiles_by_session.insert(session, profile);
        data.sessions
            .entry(session.0)
            .or_insert_with(|| HealthStatus {
                state: HealthState::Connected,
                ..Default::default()
            });
        self.publish(&mut data);
        let context = Arc::downgrade(context);
        let service = self.clone();
        tokio::spawn(async move {
            let mut lifecycle = engine.subscribe_session();
            loop {
                if *lifecycle.borrow_and_update() == SessionPhase::Closed {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(HEALTH_INTERVAL) => {
                        let Some(context) = context.upgrade() else { break; };
                        let _ = service.check_tab(&context, tab).wait().await;
                    }
                    changed = lifecycle.changed() => if changed.is_err() { break; },
                }
            }
            service.forget_session(session);
        });
        Ok(())
    }

    pub(crate) fn forget_session(&self, session: SessionId) {
        let mut data = self.data.lock().unwrap();
        let profile = data.profiles_by_session.remove(&session);
        let monitored = data.monitored.remove(&session);
        if data.sessions.remove(&session.0).is_some() || profile.is_some() || monitored {
            self.publish(&mut data);
        }
    }

    pub fn check_tab(
        &self,
        context: &Arc<ApplicationContext>,
        tab: crate::tabs::TabId,
    ) -> ProbeHandle {
        if *context.subscribe_phase().borrow() != ApplicationPhase::Running
            || !context.tabs.health_available(tab)
        {
            return ProbeHandle::ready(Ok(ConnectionProbe::busy()));
        }
        let Some(target) = context.tabs.find(tab) else {
            return ProbeHandle::ready(Err(ApplicationError::UnknownTab(tab)));
        };
        let Some(engine) = target.engine else {
            return ProbeHandle::ready(Err(ApplicationError::Operation(ExecutionError::Closed)));
        };
        let include_version = {
            let mut data = self.data.lock().unwrap();
            if let Some(profile) = target.profile {
                data.profiles_by_session.insert(engine.id(), profile);
            }
            data.sessions
                .get(&engine.id().0)
                .is_none_or(|status| status.version.is_none())
        };
        self.start_session(engine, include_version)
    }

    pub fn check_profile(
        &self,
        context: Arc<ApplicationContext>,
        profile: DatabaseConfig,
    ) -> ProbeHandle {
        if *context.subscribe_phase().borrow() != ApplicationPhase::Running {
            return ProbeHandle::ready(Ok(ConnectionProbe::busy()));
        }
        if let Some(tab) = context.tabs.snapshot().tabs.iter().find(|tab| {
            tab.profile.as_ref().is_some_and(|id| id.0 == profile.name) && tab.engine.is_some()
        }) {
            return self.check_tab(&context, tab.id);
        }
        self.start_profile(context, profile)
    }

    pub fn fail_profile(&self, profile: String, message: String) {
        let mut data = self.data.lock().unwrap();
        let mut status = HealthStatus::default();
        status.complete(&Err(ApplicationError::Operation(ExecutionError::Message(
            message,
        ))));
        data.profiles.insert(profile, status);
        self.publish(&mut data);
    }

    pub async fn drain(&self) {
        loop {
            let pending: Vec<_> = self
                .data
                .lock()
                .unwrap()
                .pending
                .values()
                .cloned()
                .collect();
            if pending.is_empty() {
                return;
            }
            for result in pending {
                let _ = ProbeHandle { result }.wait().await;
            }
        }
    }

    pub async fn check_named_profile(
        &self,
        context: Arc<ApplicationContext>,
        config: Option<&crate::Config>,
        profile: ConnectionProfileId,
    ) {
        let resolved = config
            .ok_or_else(|| "Load profiles before probing".to_owned())
            .and_then(|config| {
                config
                    .resolve_profile(&profile, None)
                    .map_err(|error| error.to_string())
            });
        match resolved {
            Ok(profile) => {
                let _ = self.check_profile(context, profile).wait().await;
            }
            Err(error) => self.fail_profile(profile.0, error),
        }
    }

    pub async fn check_all(&self, context: Arc<ApplicationContext>, config: crate::Config) {
        for profile in &config.databases {
            self.check_named_profile(
                context.clone(),
                Some(&config),
                ConnectionProfileId(profile.name.clone()),
            )
            .await;
        }
    }

    pub fn start_session(&self, engine: Arc<QueryEngine>, include_version: bool) -> ProbeHandle {
        self.start(
            ProbeTarget::Session(engine.id(), include_version),
            async move { Ok(probe(&engine, include_version).await) },
        )
    }

    pub fn start_profile(
        &self,
        context: Arc<ApplicationContext>,
        profile: DatabaseConfig,
    ) -> ProbeHandle {
        self.start(
            ProbeTarget::Profile(ConnectionProfileId(profile.name.clone())),
            async move {
                let engine = match context.connect_profile(profile).await {
                    Ok(engine) => engine,
                    Err(error) => return Ok(ConnectionProbe::failed(error)),
                };
                let result = probe(&engine, true).await;
                context.close(engine.id()).await?;
                Ok(result)
            },
        )
    }

    fn start(
        &self,
        target: ProbeTarget,
        operation: impl std::future::Future<Output = ProbeResult> + Send + 'static,
    ) -> ProbeHandle {
        let mut data = self.data.lock().unwrap();
        if let Some(result) = data.pending.get(&target) {
            return ProbeHandle {
                result: result.clone(),
            };
        }
        let (publish, result) = watch::channel(None);
        data.pending.insert(target.clone(), result.clone());
        match &target {
            ProbeTarget::Session(session, _) => {
                data.sessions.entry(session.0).or_default().probing = true;
            }
            ProbeTarget::Profile(profile) => {
                data.profiles.entry(profile.0.clone()).or_default().probing = true;
            }
        }
        self.publish(&mut data);
        let worker = tokio::spawn(operation);
        let service = self.clone();
        tokio::spawn(async move {
            let result = worker.await.unwrap_or_else(|error| {
                Err(ApplicationError::Operation(ExecutionError::Message(
                    format!("Probe worker failed: {error}"),
                )))
            });
            let mut data = service.data.lock().unwrap();
            data.pending.remove(&target);
            match &target {
                ProbeTarget::Session(session, _) => {
                    let probing = data.pending.keys().any(|target| matches!(target, ProbeTarget::Session(candidate, _) if candidate == session));
                    if let Some(status) = data.sessions.get_mut(&session.0) {
                        status.complete(&result);
                        status.probing = probing;
                        let status = status.clone();
                        if let Some(profile) = data.profiles_by_session.get(session).cloned() {
                            data.profiles.insert(profile.0, status);
                        }
                    }
                }
                ProbeTarget::Profile(profile) => {
                    let mut status = HealthStatus::default();
                    status.complete(&result);
                    data.profiles.insert(profile.0.clone(), status);
                }
            }
            service.publish(&mut data);
            publish.send_replace(Some(result));
        });
        ProbeHandle { result }
    }
}

async fn probe(engine: &QueryEngine, include_version: bool) -> ConnectionProbe {
    let latency = match engine.ping_detailed().await {
        Ok(latency) => latency,
        Err(ExecutionError::Busy) => {
            return ConnectionProbe {
                state: ProbeState::Busy,
                latency_micros: None,
                version: None,
                error: None,
            };
        }
        Err(error) => return ConnectionProbe::failed(error),
    };
    let version = if include_version {
        match engine.version_detailed().await {
            Ok(version) => Some(version),
            Err(ExecutionError::Busy) => None,
            Err(error) => return ConnectionProbe::failed(error),
        }
    } else {
        None
    };
    ConnectionProbe {
        state: ProbeState::Connected,
        latency_micros: Some(latency),
        version,
        error: None,
    }
}
