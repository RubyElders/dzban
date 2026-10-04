use std::sync::Arc;

use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::{DatabaseBackend, DatabaseConfig};

use crate::clickhouse::ClickHouseSession;
use crate::postgresql::PostgresSession;

pub struct PostgresqlAdapter;
pub struct ClickhouseAdapter;

pub fn registry() -> AdapterRegistry {
    let mut registry = AdapterRegistry::default();
    registry
        .register(Arc::new(PostgresqlAdapter))
        .expect("PostgreSQL adapter is unique");
    registry
        .register(Arc::new(ClickhouseAdapter))
        .expect("ClickHouse adapter is unique");
    registry
}

fn create(
    config: DatabaseConfig,
    backend: DatabaseBackend,
) -> Result<Arc<dyn DatabaseSession>, String> {
    if config.backend != backend {
        return Err("Connection profile does not match adapter backend".to_string());
    }
    config.validate()?;
    match backend {
        DatabaseBackend::Postgresql => Ok(Arc::new(PostgresSession::new(config))),
        DatabaseBackend::Clickhouse => Ok(Arc::new(ClickHouseSession::new(config)?)),
    }
}

impl AdapterFactory for PostgresqlAdapter {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }

    fn create(&self, config: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        create(config, self.backend())
    }
}

impl AdapterFactory for ClickhouseAdapter {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Clickhouse
    }

    fn create(&self, config: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        create(config, self.backend())
    }
}
