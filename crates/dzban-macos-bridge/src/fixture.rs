use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::error::ExecutionError;
use dzban_core::result_stream::{Delivery, ResultSink};
use dzban_core::{
    Config, DatabaseBackend, DatabaseConfig, QueryPlan, ResultCell, ResultColumn, ResultSet,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Fixture {
    canceled: Notify,
    reject_close: AtomicBool,
    slow_connect: bool,
    fail_connect: bool,
}

#[async_trait]
impl DatabaseSession for Fixture {
    async fn connect(&self) -> Result<(), String> {
        if self.slow_connect {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
        if self.fail_connect {
            return Err("Fixture connection rejected".into());
        }
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
        if sql == "command" {
            return Ok(Some(dzban_core::CommandAcknowledgment {
                affected_rows: Some(7),
            }));
        }
        if sql == "waiting" {
            self.canceled.notified().await;
            return Err(ExecutionError::Canceled);
        }
        let width = if sql == "wide" { 128 } else { 2 };
        sink.schema(
            (0..width)
                .map(|column| ResultColumn {
                    name: if column == 0 {
                        "id".into()
                    } else if column == 1 {
                        "value".into()
                    } else {
                        format!("column_{column}")
                    },
                    type_name: if column == 0 {
                        "integer".into()
                    } else {
                        "text".into()
                    },
                })
                .collect(),
        )
        .await?;
        let rows = match sql {
            "empty" => 0,
            "huge" => 3,
            "limit" => 10_020,
            _ => 500,
        };
        for row in 0..rows {
            let values = (0..width)
                .map(|column| {
                    if column == 0 {
                        ResultCell::Text(row.to_string())
                    } else if row == 1 && column == 1 {
                        ResultCell::Null
                    } else if sql == "huge" && row == 0 && column == 1 {
                        ResultCell::Text("😀".repeat(2_500_000))
                    } else if sql == "wide" {
                        ResultCell::Text(format!("r{row}c{column}"))
                    } else {
                        ResultCell::Text("žluť\nNULL😀".into())
                    }
                })
                .collect();
            if matches!(sink.row(values).await?, Delivery::Stop(_)) {
                self.canceled.notified().await;
                return Ok(None);
            }
            if row == 4 && sql == "wait" {
                self.canceled.notified().await;
                return Err(ExecutionError::Canceled);
            }
            if row == 4 && sql == "cleanup_failure" {
                let error = sink.row(vec![]).await.unwrap_err();
                self.canceled.notified().await;
                return Err(error);
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
    fn create(&self, config: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(Arc::new(Fixture {
            slow_connect: config.name == "fixture-slow",
            fail_connect: config.name == "fixture-failure",
            ..Default::default()
        }))
    }
}

pub fn app() -> Result<crate::service::App, String> {
    let mut registry = AdapterRegistry::default();
    registry.register(Arc::new(Factory)).unwrap();
    let mut profile =
        DatabaseConfig::from_connection_string("host=localhost user=fixture dbname=fixture");
    profile.name = "fixture".into();
    let mut slow = profile.clone();
    slow.name = "fixture-slow".into();
    let mut failed = profile.clone();
    failed.name = "fixture-failure".into();
    crate::service::App::new(
        registry,
        Some(Config {
            databases: vec![profile, slow, failed],
        }),
    )?
    .with_fixture_history()
}
