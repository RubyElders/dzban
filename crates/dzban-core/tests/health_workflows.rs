use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::ApplicationContext;
use dzban_core::error::ExecutionError;
use dzban_core::health::{ProbeState, RecoveryPhase};
use dzban_core::{DatabaseBackend, DatabaseConfig, QueryPlan, ResultSet};
use tokio::sync::Notify;

#[derive(Default)]
struct Session {
    connect_entered: Notify,
    connects: AtomicUsize,
    connect_release: Notify,
    close_entered: Notify,
    close_release: Notify,
    block_connect: AtomicBool,
    block_close: AtomicBool,
    reject_connect: AtomicBool,
    reject_close: AtomicBool,
    closes: AtomicUsize,
    pings: AtomicUsize,
    versions: AtomicUsize,
    reject_version: AtomicBool,
    ping_entered: Notify,
    ping_release: Notify,
    block_ping: AtomicBool,
    reject_ping: AtomicBool,
    disconnected: AtomicBool,
    query_entered: Notify,
    query_release: Notify,
    block_query: AtomicBool,
    executions: AtomicUsize,
}

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.connect_entered.notify_one();
        if self.block_connect.load(Ordering::SeqCst) {
            self.connect_release.notified().await;
        }
        if self.reject_connect.swap(false, Ordering::SeqCst) {
            Err("connection rejected once".into())
        } else {
            self.disconnected.store(false, Ordering::SeqCst);
            Ok(())
        }
    }
    async fn reconnect_if_needed(
        &self,
        started: &(dyn Fn() + Send + Sync),
    ) -> Result<bool, ExecutionError> {
        if !self.disconnected.load(Ordering::SeqCst) {
            return Ok(false);
        }
        started();
        self.connect().await.map_err(ExecutionError::from)?;
        Ok(true)
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.close_entered.notify_one();
        if self.block_close.load(Ordering::SeqCst) {
            self.close_release.notified().await;
        }
        if self.reject_close.swap(false, Ordering::SeqCst) {
            Err(ExecutionError::Message("cleanup rejected once".into()))
        } else {
            Ok(())
        }
    }
    async fn ping(&self) -> Result<u64, String> {
        self.pings.fetch_add(1, Ordering::SeqCst);
        self.ping_entered.notify_one();
        if self.block_ping.load(Ordering::SeqCst) {
            self.ping_release.notified().await;
        }
        if self.disconnected.load(Ordering::SeqCst) {
            Err("transport closed".into())
        } else if self.reject_ping.swap(false, Ordering::SeqCst) {
            Err("probe failed once".into())
        } else {
            Ok(u64::MAX)
        }
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        Err("unused".into())
    }
    async fn cancel(&self) -> Result<(), String> {
        self.query_release.notify_one();
        Ok(())
    }
    async fn version(&self) -> Result<String, String> {
        self.versions.fetch_add(1, Ordering::SeqCst);
        if self.reject_version.swap(false, Ordering::SeqCst) {
            return Err("version failed once".into());
        }
        Ok("fixture".into())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate: Default::default(),
        })
    }
    async fn execute_streamed(
        &self,
        _: &str,
        sink: &mut dzban_core::result_stream::ResultSink,
    ) -> Result<Option<dzban_core::CommandAcknowledgment>, ExecutionError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        sink.schema(vec![dzban_core::ResultColumn {
            name: "value".into(),
            type_name: "text".into(),
        }])
        .await?;
        sink.row(vec![dzban_core::ResultCell::Text("fixture".into())])
            .await?;
        self.query_entered.notify_one();
        if self.block_query.load(Ordering::SeqCst) {
            self.query_release.notified().await;
        }
        Ok(None)
    }
}

struct Factory(Arc<Session>);
impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(self.0.clone())
    }
}

fn fixture() -> (Arc<ApplicationContext>, Arc<Session>, DatabaseConfig) {
    let session = Arc::new(Session::default());
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(Factory(session.clone())))
        .unwrap();
    (
        Arc::new(ApplicationContext::new(registry)),
        session,
        DatabaseConfig::from_connection_string("host=localhost user=fixture dbname=fixture"),
    )
}

async fn reached(signal: &Notify) {
    tokio::time::timeout(Duration::from_secs(3), signal.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn session_probe_waiters_share_work_without_canceling_the_owner() {
    let (context, session, profile) = fixture();
    let engine = context.connect_profile(profile).await.unwrap();
    session.block_ping.store(true, Ordering::SeqCst);
    let first = context.health.start_session(engine.clone(), true);
    reached(&session.ping_entered).await;
    let second = context.health.start_session(engine.clone(), true);
    let overlapping = context
        .health
        .start_session(engine.clone(), false)
        .wait()
        .await
        .unwrap();
    assert_eq!(overlapping.state, ProbeState::Busy);
    assert!(context.health.snapshot().sessions[0].status.probing);
    assert_eq!(
        context.health.snapshot().sessions[0].status.recovery,
        RecoveryPhase::Idle
    );
    drop(first);
    assert_eq!(session.pings.load(Ordering::SeqCst), 1);
    session.block_ping.store(false, Ordering::SeqCst);
    session.ping_release.notify_one();
    let result = second.wait().await.unwrap();
    assert_eq!(result.state, ProbeState::Connected);
    assert_eq!(result.latency_micros, Some(u64::MAX));
    assert_eq!(result.version.as_deref(), Some("fixture"));
    assert_eq!(session.versions.load(Ordering::SeqCst), 1);
    let next = context
        .health
        .start_session(engine, false)
        .wait()
        .await
        .unwrap();
    assert_eq!(next.version, None);
    assert_eq!(session.pings.load(Ordering::SeqCst), 2);
    assert_eq!(session.versions.load(Ordering::SeqCst), 1);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn session_probe_respects_admission_and_can_retry_a_transport_failure() {
    let (context, session, profile) = fixture();
    let engine = context.connect_profile(profile).await.unwrap();
    session.block_ping.store(true, Ordering::SeqCst);
    let occupying = engine.start_ping().unwrap();
    reached(&session.ping_entered).await;
    let busy = context
        .health
        .start_session(engine.clone(), true)
        .wait()
        .await
        .unwrap();
    assert_eq!(busy.state, ProbeState::Busy);
    assert_eq!(busy.latency_micros, None);
    assert_eq!(session.pings.load(Ordering::SeqCst), 1);
    assert_eq!(session.versions.load(Ordering::SeqCst), 0);
    session.block_ping.store(false, Ordering::SeqCst);
    session.ping_release.notify_one();
    occupying.wait().await.unwrap();
    session.reject_ping.store(true, Ordering::SeqCst);
    let failed = context
        .health
        .start_session(engine.clone(), true)
        .wait()
        .await
        .unwrap();
    assert_eq!(failed.state, ProbeState::Error);
    assert_eq!(failed.error.as_deref(), Some("probe failed once"));
    assert_eq!(session.versions.load(Ordering::SeqCst), 0);
    let recovered = context
        .health
        .start_session(engine, true)
        .wait()
        .await
        .unwrap();
    assert_eq!(recovered.state, ProbeState::Connected);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn recovery_keeps_tab_document_and_results_and_owns_connection_admission() {
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    tab.submit("SELECT original".into(), Default::default(), None)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let generation = tab.document.snapshot().output_generation;
    tab.document.set_draft("SELECT preserved".into());
    session.disconnected.store(true, Ordering::SeqCst);
    session.block_connect.store(true, Ordering::SeqCst);
    let recovery = context.health.check_tab(&context, tab.id);
    reached(&session.connect_entered).await;
    wait_for_connects(&session, 2).await;
    assert!(context.health.snapshot().sessions[0].status.probing);
    assert_eq!(
        context.health.snapshot().sessions[0].status.recovery,
        RecoveryPhase::Reconnecting
    );
    assert!(matches!(engine.start_ping(), Err(ExecutionError::Busy)));
    let joined = context.health.check_tab(&context, tab.id);
    drop(recovery);
    session.block_connect.store(false, Ordering::SeqCst);
    session.connect_release.notify_one();
    let result = joined.wait().await.unwrap();
    assert_eq!(result.state, ProbeState::Connected);
    assert!(result.reconnected);
    assert_eq!(context.health.snapshot().sessions[0].status.recoveries, 1);
    assert_eq!(
        context.health.snapshot().sessions[0].status.recovery,
        RecoveryPhase::Idle
    );
    let current = context.tabs.find(tab.id).unwrap();
    assert_eq!(current.engine.unwrap().id(), engine.id());
    assert_eq!(current.document.id(), tab.document.id());
    assert_eq!(current.document.draft(), "SELECT preserved");
    assert_eq!(current.document.snapshot().output_generation, generation);
    assert_eq!(session.executions.load(Ordering::SeqCst), 1);
    assert_eq!(context.tabs.snapshot().selected, Some(tab.id));
    assert_eq!(session.connects.load(Ordering::SeqCst), 2);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn recovery_survives_failed_version_lookup_and_invalidates_cached_metadata() {
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    context
        .health
        .check_tab(&context, tab.id)
        .wait()
        .await
        .unwrap();
    assert_eq!(
        context.health.snapshot().sessions[0]
            .status
            .version
            .as_deref(),
        Some("fixture")
    );
    session.disconnected.store(true, Ordering::SeqCst);
    session.reject_version.store(true, Ordering::SeqCst);
    let result = context
        .health
        .check_tab(&context, tab.id)
        .wait()
        .await
        .unwrap();
    assert_eq!(result.state, ProbeState::Error);
    assert!(result.reconnected);
    let status = &context.health.snapshot().sessions[0].status;
    assert_eq!(status.recoveries, 1);
    assert!(status.version.is_none());
    let result = context
        .health
        .check_tab(&context, tab.id)
        .wait()
        .await
        .unwrap();
    assert_eq!(result.state, ProbeState::Connected);
    assert!(!result.reconnected);
    let status = &context.health.snapshot().sessions[0].status;
    assert_eq!(status.recoveries, 1);
    assert_eq!(status.version.as_deref(), Some("fixture"));
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn recovery_survives_failed_post_connect_ping() {
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    context
        .health
        .check_tab(&context, tab.id)
        .wait()
        .await
        .unwrap();
    session.disconnected.store(true, Ordering::SeqCst);
    session.reject_ping.store(true, Ordering::SeqCst);
    let result = context
        .health
        .check_tab(&context, tab.id)
        .wait()
        .await
        .unwrap();
    assert_eq!(result.state, ProbeState::Error);
    assert!(result.reconnected);
    let status = &context.health.snapshot().sessions[0].status;
    assert_eq!(status.recoveries, 1);
    assert!(status.version.is_none());
    let result = context
        .health
        .check_tab(&context, tab.id)
        .wait()
        .await
        .unwrap();
    assert_eq!(result.state, ProbeState::Connected);
    assert!(!result.reconnected);
    assert_eq!(context.health.snapshot().sessions[0].status.recoveries, 1);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

async fn wait_for_connects(session: &Session, count: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while session.connects.load(Ordering::SeqCst) != count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(start_paused = true)]
async fn periodic_checks_retry_failed_recovery_and_stop_after_shutdown() {
    use dzban_core::health::{HEALTH_INTERVAL, HealthState};
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    context.health.track_tab(&context, tab.id).unwrap();
    session.disconnected.store(true, Ordering::SeqCst);
    session.reject_connect.store(true, Ordering::SeqCst);
    tokio::task::yield_now().await;
    tokio::time::advance(HEALTH_INTERVAL).await;
    let failed = health_until(&context, |snapshot| {
        snapshot.sessions[0].status.state == HealthState::Error
    })
    .await;
    assert_eq!(
        failed.sessions[0].status.recovery,
        RecoveryPhase::RetryWaiting
    );
    assert_eq!(session.connects.load(Ordering::SeqCst), 2);
    tokio::task::yield_now().await;
    tokio::time::advance(HEALTH_INTERVAL).await;
    let recovered = health_until(&context, |snapshot| {
        snapshot.sessions[0].status.recoveries == 1
    })
    .await;
    assert_eq!(recovered.sessions[0].status.state, HealthState::Connected);
    assert_eq!(recovered.sessions[0].status.recovery, RecoveryPhase::Idle);
    assert!(recovered.sessions[0].status.error.is_none());
    assert_eq!(session.connects.load(Ordering::SeqCst), 3);
    context.start_shutdown().unwrap().wait().await.unwrap();
    tokio::time::advance(HEALTH_INTERVAL * 3).await;
    assert_eq!(session.connects.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn recovery_timeout_releases_admission_and_can_be_retried() {
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    session.disconnected.store(true, Ordering::SeqCst);
    session.block_connect.store(true, Ordering::SeqCst);
    let recovery = context.health.check_tab(&context, tab.id);
    wait_for_connects(&session, 2).await;
    let result = recovery.wait().await.unwrap();
    assert_eq!(result.state, ProbeState::Error);
    assert!(result.error.unwrap().contains("timed out"));
    assert_eq!(
        context.health.snapshot().sessions[0].status.recovery,
        RecoveryPhase::RetryWaiting
    );
    session.block_connect.store(false, Ordering::SeqCst);
    let retry = context
        .health
        .check_tab(&context, tab.id)
        .wait()
        .await
        .unwrap();
    assert!(retry.reconnected);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn profile_probe_joins_through_cleanup_and_never_opens_a_tab() {
    let (context, session, profile) = fixture();
    session.block_close.store(true, Ordering::SeqCst);
    let first = context
        .health
        .start_profile(context.clone(), profile.clone());
    reached(&session.close_entered).await;
    let second = context.health.start_profile(context.clone(), profile);
    drop(first);
    assert_eq!(session.pings.load(Ordering::SeqCst), 1);
    assert_eq!(session.closes.load(Ordering::SeqCst), 1);
    assert!(context.tabs.snapshot().tabs.is_empty());
    assert_eq!(context.session_ids().await.len(), 1);
    let shutdown = context.start_shutdown().unwrap();
    session.close_release.notify_one();
    assert_eq!(second.wait().await.unwrap().state, ProbeState::Connected);
    shutdown.wait().await.unwrap();
    assert!(context.session_ids().await.is_empty());
    assert!(context.tabs.snapshot().tabs.is_empty());
    assert_eq!(session.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn profile_probe_failure_still_cleans_up_and_cleanup_failure_remains_owned() {
    let (context, session, profile) = fixture();
    session.reject_ping.store(true, Ordering::SeqCst);
    let failed = context
        .health
        .start_profile(context.clone(), profile.clone())
        .wait()
        .await
        .unwrap();
    assert_eq!(failed.state, ProbeState::Error);
    assert!(context.session_ids().await.is_empty());
    assert_eq!(session.closes.load(Ordering::SeqCst), 1);
    session.reject_close.store(true, Ordering::SeqCst);
    let cleanup = context
        .health
        .start_profile(context.clone(), profile)
        .wait()
        .await;
    assert!(
        cleanup
            .unwrap_err()
            .to_string()
            .contains("cleanup rejected once")
    );
    assert_eq!(context.session_ids().await.len(), 1);
    context.start_shutdown().unwrap().wait().await.unwrap();
    assert!(context.session_ids().await.is_empty());
    assert_eq!(session.closes.load(Ordering::SeqCst), 3);
}

async fn health_until(
    context: &ApplicationContext,
    predicate: impl Fn(&dzban_core::health::HealthSnapshot) -> bool,
) -> dzban_core::health::HealthSnapshot {
    let mut snapshots = context.health.subscribe();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = snapshots.borrow_and_update().clone();
            if predicate(&snapshot) {
                return snapshot;
            }
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn periodic_health_is_shared_deduplicated_and_reuses_the_server_version() {
    use dzban_core::health::{HEALTH_INTERVAL, HealthState};
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    context.health.track_tab(&context, tab.id).unwrap();
    context.health.track_tab(&context, tab.id).unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(HEALTH_INTERVAL - Duration::from_millis(1)).await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 0);
    tokio::time::advance(Duration::from_millis(1)).await;
    reached(&session.ping_entered).await;
    let checked = health_until(&context, |snapshot| {
        snapshot
            .sessions
            .iter()
            .any(|entry| !entry.status.probing && entry.status.version.is_some())
    })
    .await;
    assert_eq!(checked.sessions[0].status.state, HealthState::Connected);
    assert_eq!(checked.sessions[0].status.latency_micros, Some(u64::MAX));
    assert!(checked.sessions[0].status.checked_at_millis.is_some());
    assert_eq!(checked.profiles[0].profile, tab.profile.as_ref().unwrap().0);
    assert_eq!(checked.profiles[0].status, checked.sessions[0].status);
    assert_eq!(session.pings.load(Ordering::SeqCst), 1);
    assert_eq!(session.versions.load(Ordering::SeqCst), 1);
    tokio::task::yield_now().await;
    tokio::time::advance(HEALTH_INTERVAL).await;
    reached(&session.ping_entered).await;
    health_until(&context, |snapshot| {
        snapshot.revision > checked.revision && !snapshot.sessions[0].status.probing
    })
    .await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 2);
    assert_eq!(session.versions.load(Ordering::SeqCst), 1);
    context.start_shutdown().unwrap().wait().await.unwrap();
    tokio::time::advance(HEALTH_INTERVAL * 2).await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 2);
    assert!(context.health.snapshot().sessions.is_empty());
}

#[tokio::test(start_paused = true)]
async fn health_ticks_skip_selected_input_preview_and_active_queries() {
    use dzban_core::health::HEALTH_INTERVAL;
    use dzban_core::interaction::{InputEvent, InputKey};
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    context.health.track_tab(&context, tab.id).unwrap();
    let input = |event| {
        context
            .tabs
            .input(
                tab.id,
                tab.document.id(),
                tab.document.snapshot().draft_revision,
                event,
            )
            .unwrap()
    };
    input(InputEvent::Press {
        key: InputKey::Primary,
        at_ms: 1,
    });
    tokio::task::yield_now().await;
    tokio::time::advance(HEALTH_INTERVAL).await;
    tokio::task::yield_now().await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 0);
    input(InputEvent::Release {
        key: InputKey::Primary,
        at_ms: 2,
    });
    input(InputEvent::OpenPlan);
    tokio::time::advance(HEALTH_INTERVAL).await;
    tokio::task::yield_now().await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 0);
    input(InputEvent::ContextChanged);
    session.block_query.store(true, Ordering::SeqCst);
    let query = tab
        .submit("SELECT 1".into(), Default::default(), None)
        .unwrap();
    reached(&session.query_entered).await;
    tokio::time::advance(HEALTH_INTERVAL).await;
    tokio::task::yield_now().await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 0);
    session.query_release.notify_one();
    query.wait().await.unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(HEALTH_INTERVAL).await;
    reached(&session.ping_entered).await;
    health_until(&context, |snapshot| {
        !snapshot.sessions[0].status.probing
            && snapshot.sessions[0].status.checked_at_millis.is_some()
    })
    .await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 1);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn busy_probes_keep_the_previous_health_and_shutdown_joins_the_owner() {
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    context.health.track_tab(&context, tab.id).unwrap();
    context
        .health
        .check_tab(&context, tab.id)
        .wait()
        .await
        .unwrap();
    let previous = context.health.snapshot().sessions[0].status.clone();
    session.block_ping.store(true, Ordering::SeqCst);
    let occupying = engine.start_ping().unwrap();
    reached(&session.ping_entered).await;
    assert_eq!(
        context
            .health
            .check_tab(&context, tab.id)
            .wait()
            .await
            .unwrap()
            .state,
        ProbeState::Busy
    );
    assert_eq!(context.health.snapshot().sessions[0].status, previous);
    session.ping_release.notify_one();
    occupying.wait().await.unwrap();
    let probe = context.health.check_tab(&context, tab.id);
    reached(&session.ping_entered).await;
    let shutdown = context.start_shutdown().unwrap();
    tokio::task::yield_now().await;
    assert_eq!(session.closes.load(Ordering::SeqCst), 0);
    session.ping_release.notify_one();
    probe.wait().await.unwrap();
    shutdown.wait().await.unwrap();
    assert!(context.health.snapshot().sessions.is_empty());
    assert_eq!(session.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn failed_shutdown_keeps_health_monitoring_retryable() {
    use dzban_core::health::HEALTH_INTERVAL;
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    context.health.track_tab(&context, tab.id).unwrap();
    session.reject_close.store(true, Ordering::SeqCst);
    assert!(context.start_shutdown().unwrap().wait().await.is_err());
    tokio::task::yield_now().await;
    tokio::time::advance(HEALTH_INTERVAL).await;
    reached(&session.ping_entered).await;
    health_until(&context, |snapshot| {
        !snapshot.sessions[0].status.probing && snapshot.sessions[0].status.version.is_some()
    })
    .await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 1);
    assert!(context.tabs.find(tab.id).is_some());
    context.start_shutdown().unwrap().wait().await.unwrap();
    tokio::time::advance(HEALTH_INTERVAL * 2).await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 1);
    assert!(context.health.snapshot().sessions.is_empty());
}

#[tokio::test(start_paused = true)]
async fn profile_checks_reuse_open_tabs_and_bulk_checks_clean_temporary_sessions() {
    let (context, session, mut profile) = fixture();
    profile.name = "existing".into();
    let engine = context
        .start_connect_tab(profile.clone())
        .wait()
        .await
        .unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    let mut independent = profile.clone();
    independent.name = "independent".into();
    context
        .health
        .check_all(
            context.clone(),
            dzban_core::Config {
                databases: vec![profile.clone(), independent],
            },
        )
        .await;
    assert_eq!(session.connects.load(Ordering::SeqCst), 2);
    assert_eq!(session.closes.load(Ordering::SeqCst), 1);
    assert_eq!(session.pings.load(Ordering::SeqCst), 2);
    assert_eq!(context.tabs.snapshot().tabs.len(), 1);
    assert_eq!(context.session_ids().await, [engine.id()]);
    assert_eq!(context.health.snapshot().profiles.len(), 2);
    context
        .health
        .check_all(
            context.clone(),
            dzban_core::Config {
                databases: vec![profile.clone(), profile],
            },
        )
        .await;
    let health = context.health.snapshot();
    let ambiguous = health
        .profiles
        .iter()
        .find(|entry| entry.profile == "existing")
        .unwrap();
    assert_eq!(
        ambiguous.status.state,
        dzban_core::health::HealthState::Error
    );
    assert!(
        ambiguous
            .status
            .error
            .as_ref()
            .unwrap()
            .contains("Multiple connection profiles")
    );
    assert_eq!(session.connects.load(Ordering::SeqCst), 2);
    assert_eq!(session.pings.load(Ordering::SeqCst), 2);
    tokio::time::advance(dzban_core::health::HEALTH_INTERVAL * 2).await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 2);
    context.health.track_tab(&context, tab.id).unwrap();
    let tracked_revision = context.health.snapshot().revision;
    tokio::task::yield_now().await;
    tokio::time::advance(dzban_core::health::HEALTH_INTERVAL).await;
    reached(&session.ping_entered).await;
    health_until(&context, |snapshot| {
        snapshot.revision > tracked_revision && !snapshot.sessions[0].status.probing
    })
    .await;
    assert_eq!(session.pings.load(Ordering::SeqCst), 3);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn probing_before_configuration_is_loaded_reports_inline_health_without_connecting() {
    let (context, session, _) = fixture();
    context
        .health
        .check_named_profile(
            context.clone(),
            None,
            dzban_core::ConnectionProfileId("not-loaded".into()),
        )
        .await;
    let snapshot = context.health.snapshot();
    assert_eq!(
        snapshot.profiles[0].status.state,
        dzban_core::health::HealthState::Error
    );
    assert_eq!(
        snapshot.profiles[0].status.error.as_deref(),
        Some("Load profiles before probing")
    );
    assert!(!snapshot.profiles[0].status.probing);
    assert!(snapshot.sessions.is_empty());
    assert_eq!(session.connects.load(Ordering::SeqCst), 0);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn control_revisions_follow_admission_instead_of_delivery_order() {
    use dzban_core::interaction::PrimaryAction;
    let (context, session, profile) = fixture();
    let engine = context.start_connect_tab(profile).wait().await.unwrap();
    let ready = context.tabs.controls();
    assert_eq!(ready.primary_action, PrimaryAction::Run);
    assert_eq!(context.tabs.controls(), ready);
    session.block_ping.store(true, Ordering::SeqCst);
    let ping = engine.start_ping().unwrap();
    reached(&session.ping_entered).await;
    let busy = context.tabs.controls();
    assert_eq!(busy.primary_action, PrimaryAction::Unavailable);
    assert!(busy.revision > ready.revision);
    assert_eq!(context.tabs.controls(), busy);
    session.ping_release.notify_one();
    ping.wait().await.unwrap();
    let available = context.tabs.controls();
    assert_eq!(available.primary_action, PrimaryAction::Run);
    assert!(available.revision > busy.revision);
    assert_eq!(context.tabs.controls(), available);
    context.start_shutdown().unwrap().wait().await.unwrap();
}
