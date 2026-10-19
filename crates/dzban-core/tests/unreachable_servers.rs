use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::{ApplicationContext, ApplicationPhase};
use dzban_core::health::HealthState;
use dzban_core::tabs::TabPhase;
use dzban_core::{
    Config, ConnectionProfileId, DatabaseBackend, DatabaseConfig, QueryPlan, ResultSet,
};

#[derive(Default)]
struct Server {
    unreachable: AtomicBool,
    silent: AtomicBool,
}

struct Session(Arc<Server>);

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        if self.0.unreachable.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        if self.0.silent.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(1)
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
        Err("No plan".into())
    }
}

struct Factory(Arc<Server>);

impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(Arc::new(Session(self.0.clone())))
    }
}

fn fixture(unreachable: bool) -> (Arc<ApplicationContext>, Arc<Server>, Config) {
    let server = Arc::new(Server::default());
    server.unreachable.store(unreachable, Ordering::SeqCst);
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(Factory(server.clone())))
        .unwrap();
    let config: Config = toml::from_str(
        "[[databases]]\nname = \"remote\"\nhost = \"10.255.255.1\"\nuser = \"u\"\ndatabase = \"d\"\n",
    )
    .unwrap();
    (Arc::new(ApplicationContext::new(registry)), server, config)
}

fn profile(config: &Config) -> DatabaseConfig {
    config
        .resolve_profile(&ConnectionProfileId("remote".into()), None)
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn connecting_to_an_unreachable_server_fails_after_the_connect_timeout() {
    let (context, _, config) = fixture(true);
    let tab = context.open_tab(None, "kept".into()).unwrap();
    let started = tokio::time::Instant::now();
    let error = context
        .start_connect_in_tab(tab, profile(&config))
        .wait()
        .await
        .err()
        .unwrap();
    assert_eq!(started.elapsed(), dzban_core::application::CONNECT_TIMEOUT);
    assert!(error.to_string().contains("timed out"), "{error}");
    let tab = context.tabs.find(tab).unwrap();
    assert!(matches!(tab.phase, TabPhase::Failed(_)));
    assert_eq!(tab.document.draft(), "kept");
    assert!(context.subscribe_pending_connections().borrow().is_empty());
}

#[tokio::test]
async fn canceling_or_closing_a_hung_connection_does_not_wait_for_the_server() {
    let (context, _, config) = fixture(true);
    let kept = context.open_tab(None, "draft".into()).unwrap();
    let pending = context.start_connect_in_tab(kept, profile(&config));
    tokio::time::timeout(
        Duration::from_secs(1),
        context.start_cancel_connection(kept).unwrap().wait(),
    )
    .await
    .expect("cancel waited for the unreachable server")
    .unwrap();
    assert!(pending.wait().await.is_err());
    let tab = context.tabs.find(kept).unwrap();
    assert_eq!(tab.phase, TabPhase::Choosing);
    assert_eq!(tab.document.draft(), "draft");

    let closed = context.open_tab(None, String::new()).unwrap();
    let _pending = context.start_connect_in_tab(closed, profile(&config));
    tokio::time::timeout(
        Duration::from_secs(1),
        context.start_close_tab(closed).unwrap().wait(),
    )
    .await
    .expect("close waited for the unreachable server")
    .unwrap();
    assert!(context.tabs.find(closed).is_none());
    assert!(context.session_ids().await.is_empty());
}

#[tokio::test]
async fn shutdown_does_not_wait_for_a_hung_connection() {
    let (context, _, config) = fixture(true);
    let tab = context.open_tab(None, String::new()).unwrap();
    let _pending = context.start_connect_in_tab(tab, profile(&config));
    tokio::time::timeout(
        Duration::from_secs(1),
        context.start_shutdown().unwrap().wait(),
    )
    .await
    .expect("shutdown waited for the unreachable server")
    .unwrap();
    assert_eq!(
        *context.subscribe_phase().borrow(),
        ApplicationPhase::Closed
    );
}

#[tokio::test(start_paused = true)]
async fn probing_an_unreachable_profile_reports_a_timeout_without_leaking_a_session() {
    let (context, server, config) = fixture(true);
    let started = tokio::time::Instant::now();
    context
        .health
        .check_named_profile(
            context.clone(),
            Some(&config),
            ConnectionProfileId("remote".into()),
        )
        .await;
    assert_eq!(started.elapsed(), dzban_core::health::PROBE_TIMEOUT);
    let snapshot = context.health.snapshot();
    let status = &snapshot.profiles[0].status;
    assert_eq!(status.state, HealthState::Error);
    assert!(status.error.as_deref().unwrap().contains("timed out"));
    assert!(context.subscribe_pending_connections().borrow().is_empty());
    server.unreachable.store(false, Ordering::SeqCst);
    tokio::time::sleep(dzban_core::application::CONNECT_TIMEOUT).await;
    assert!(context.session_ids().await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_silent_server_releases_the_session_after_the_probe_timeout() {
    let (context, server, config) = fixture(false);
    let tab = context.open_tab(None, String::new()).unwrap();
    let engine = context
        .start_connect_in_tab(tab, profile(&config))
        .wait()
        .await
        .unwrap();
    server.silent.store(true, Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    context
        .health
        .check_tab(&context, tab)
        .wait()
        .await
        .unwrap();
    assert!(started.elapsed() <= dzban_core::health::PROBE_TIMEOUT);
    let snapshot = context.health.snapshot();
    let status = &snapshot.sessions[0].status;
    assert_eq!(status.state, HealthState::Error);
    assert!(status.error.as_deref().unwrap().contains("timed out"));
    server.silent.store(false, Ordering::SeqCst);
    assert_eq!(engine.ping_detailed().await.unwrap(), 1);
}
