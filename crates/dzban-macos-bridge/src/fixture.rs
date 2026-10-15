use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::error::ExecutionError;
use dzban_core::result_stream::ResultSink;
use dzban_core::{
    Config, DatabaseBackend, DatabaseConfig, QueryPlan, ResultCell, ResultColumn, ResultSet,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Fixture {
    canceled: Notify,
    reject_close: AtomicBool,
}

#[async_trait]
impl DatabaseSession for Fixture {
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        if self.reject_close.swap(false, Ordering::SeqCst) {
            Err(ExecutionError::Message(
                "Fixture cleanup rejected once".into(),
            ))
        } else {
            Ok(())
        }
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(1)
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        Err("Use streaming".into())
    }
    async fn cancel(&self) -> Result<(), String> {
        self.canceled.notify_one();
        Ok(())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!({"fixture": true}),
            estimate: Default::default(),
        })
    }
    async fn execute_streamed(
        &self,
        sql: &str,
        sink: &mut ResultSink,
    ) -> Result<Option<dzban_core::CommandAcknowledgment>, ExecutionError> {
        if sql == "fail" {
            return Err(ExecutionError::Message("Fixture query failed".into()));
        }
        if sql == "reject_close" {
            self.reject_close.store(true, Ordering::SeqCst);
        }
        sink.schema(vec![
            ResultColumn {
                name: "id".into(),
                type_name: "integer".into(),
            },
            ResultColumn {
                name: "value".into(),
                type_name: "text".into(),
            },
        ])
        .await?;
        for row in 0..500 {
            sink.row(vec![
                ResultCell::Text(row.to_string()),
                if row == 1 {
                    ResultCell::Null
                } else {
                    ResultCell::Text("žluť\nNULL😀".into())
                },
            ])
            .await?;
            if row == 4 && sql == "wait" {
                self.canceled.notified().await;
                return Err(ExecutionError::Canceled);
            }
        }
        Ok(None)
    }
}

struct Factory;

impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(Arc::new(Fixture::default()))
    }
}

pub fn app() -> Result<crate::service::App, String> {
    let mut registry = AdapterRegistry::default();
    registry.register(Arc::new(Factory)).unwrap();
    let mut profile =
        DatabaseConfig::from_connection_string("host=localhost user=fixture dbname=fixture");
    profile.name = "fixture".into();
    crate::service::App::new(
        registry,
        Some(Config {
            databases: vec![profile],
        }),
    )?
    .with_fixture_history()
}
