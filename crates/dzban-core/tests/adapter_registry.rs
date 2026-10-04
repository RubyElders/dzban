use async_trait::async_trait;
use std::sync::Arc;

use dzban_core::adapter::{AdapterError, AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::{DatabaseBackend, DatabaseConfig, QueryPlan, ResultSet};

struct OfflineSession(bool);

#[async_trait]
impl DatabaseSession for OfflineSession {
    async fn connect(&self) -> Result<(), String> {
        Err("connection fixture".into())
    }
    async fn connect_detailed(&self) -> Result<(), dzban_core::error::ExecutionError> {
        if self.0 {
            Err(dzban_core::error::ExecutionError::Database(
                dzban_core::error::DatabaseError {
                    backend: DatabaseBackend::Postgresql,
                    kind: dzban_core::error::ErrorKind::Server,
                    message: "Authentication refused".into(),
                    code: Some(dzban_core::error::NativeErrorCode::SqlState("28P01".into())),
                    detail: Some("Fixture detail".into()),
                    hint: None,
                },
            ))
        } else {
            self.connect().await.map_err(Into::into)
        }
    }
    async fn ping(&self) -> Result<u64, String> {
        unreachable!()
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        unreachable!()
    }
    async fn cancel(&self) -> Result<(), String> {
        unreachable!()
    }
    async fn version(&self) -> Result<String, String> {
        unreachable!()
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        unreachable!()
    }
}

struct OfflineFactory(bool);

impl AdapterFactory for OfflineFactory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(Arc::new(OfflineSession(self.0)))
    }
}

struct Factory(DatabaseBackend, &'static str);

impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        self.0
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Err(self.1.into())
    }
}

fn profile(backend: DatabaseBackend) -> DatabaseConfig {
    DatabaseConfig {
        name: "fixture".into(),
        backend,
        host: None,
        port: None,
        user: "fixture".into(),
        database: "fixture".into(),
        password: None,
        url: None,
    }
}

#[test]
fn registry_routes_by_backend_and_preserves_creation_error_category() {
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(Factory(DatabaseBackend::Postgresql, "pg fixture")))
        .unwrap();
    registry
        .register(Arc::new(Factory(DatabaseBackend::Clickhouse, "ch fixture")))
        .unwrap();
    for (backend, message) in [
        (DatabaseBackend::Postgresql, "pg fixture"),
        (DatabaseBackend::Clickhouse, "ch fixture"),
    ] {
        assert_eq!(
            registry.create(profile(backend)).err().unwrap(),
            AdapterError::Create {
                backend,
                message: message.into()
            }
        );
    }
}

#[test]
fn duplicate_registration_cannot_replace_the_existing_adapter() {
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(Factory(DatabaseBackend::Postgresql, "original")))
        .unwrap();
    assert_eq!(
        registry.register(Arc::new(Factory(
            DatabaseBackend::Postgresql,
            "replacement"
        ))),
        Err(AdapterError::Duplicate(DatabaseBackend::Postgresql))
    );
    assert_eq!(
        registry
            .create(profile(DatabaseBackend::Postgresql))
            .err()
            .unwrap(),
        AdapterError::Create {
            backend: DatabaseBackend::Postgresql,
            message: "original".into()
        }
    );
}

#[tokio::test]
async fn unavailable_backend_is_distinct_from_a_failed_adapter() {
    let registry = AdapterRegistry::default();
    assert_eq!(
        registry
            .connect(profile(DatabaseBackend::Clickhouse))
            .await
            .err()
            .unwrap(),
        AdapterError::Unsupported(DatabaseBackend::Clickhouse)
    );
}

#[tokio::test]
async fn connection_failure_retains_its_backend_and_stage() {
    let mut registry = AdapterRegistry::default();
    registry.register(Arc::new(OfflineFactory(false))).unwrap();
    assert_eq!(
        registry
            .connect(profile(DatabaseBackend::Postgresql))
            .await
            .err()
            .unwrap(),
        AdapterError::Connect {
            backend: DatabaseBackend::Postgresql,
            error: dzban_core::error::ExecutionError::Message("connection fixture".into())
        }
    );
}

#[tokio::test]
async fn connection_snapshot_preserves_native_diagnostics_without_registering_a_session() {
    use dzban_core::application::{ApplicationContext, ApplicationError, ConnectionPhase};
    let mut registry = AdapterRegistry::default();
    registry.register(Arc::new(OfflineFactory(true))).unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let request = context.start_connect(profile(DatabaseBackend::Postgresql));
    let state = request.subscribe();
    let error = request.wait().await.err().unwrap();
    assert_eq!(state.borrow().phase, ConnectionPhase::Failed(error.clone()));
    assert!(context.session_ids().await.is_empty());
    let ApplicationError::Adapter(AdapterError::Connect {
        error: dzban_core::error::ExecutionError::Database(diagnostic),
        ..
    }) = error
    else {
        panic!("Expected a typed connection failure");
    };
    assert_eq!(
        diagnostic.code,
        Some(dzban_core::error::NativeErrorCode::SqlState("28P01".into()))
    );
    assert_eq!(diagnostic.detail.as_deref(), Some("Fixture detail"));
}
