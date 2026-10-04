use std::sync::Arc;

use async_trait::async_trait;
use dzban_cli::{CliError, dispatch::finish_session};
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::{ApplicationContext, ApplicationError};
use dzban_core::error::ExecutionError;
use dzban_core::execution::SessionPhase;
use dzban_core::{DatabaseBackend, DatabaseConfig, QueryPlan, ResultSet};

struct Session {
    fail_close: bool,
}

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        if self.fail_close {
            Err(ExecutionError::Message("Fixture disconnect failure".into()))
        } else {
            Ok(())
        }
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(0)
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        Ok(Default::default())
    }
    async fn cancel(&self) -> Result<(), String> {
        Ok(())
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        Err("No fixture plan".into())
    }
}

struct Factory(bool);

impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(Arc::new(Session { fail_close: self.0 }))
    }
}

async fn fixture(
    fail_close: bool,
) -> (
    Arc<ApplicationContext>,
    Arc<dzban_core::execution::QueryEngine>,
) {
    let mut registry = AdapterRegistry::default();
    registry.register(Arc::new(Factory(fail_close))).unwrap();
    let context = Arc::new(ApplicationContext::new(registry));
    let engine = context
        .connect_profile(DatabaseConfig::from_connection_string(
            "host=localhost user=fixture dbname=fixture",
        ))
        .await
        .unwrap();
    (context, engine)
}

#[tokio::test]
async fn successful_and_failed_batch_outcomes_both_close_the_owned_session() {
    for fail_query in [false, true] {
        let (context, engine) = fixture(false).await;
        let result = if fail_query {
            Err(CliError {
                message: "Canceled".into(),
                code: 130,
            })
        } else {
            Ok(42)
        };
        let result = finish_session(&context, engine.id(), result).await;
        if fail_query {
            assert_eq!(result.unwrap_err().code, 130);
        } else {
            assert_eq!(result.unwrap(), 42);
        }
        assert_eq!(*engine.subscribe_session().borrow(), SessionPhase::Closed);
        assert!(matches!(
            context.session(engine.id()).await,
            Err(ApplicationError::UnknownSession(_))
        ));
    }
}

#[tokio::test]
async fn cleanup_failure_is_reported_without_overwriting_the_query_exit_contract() {
    let (context, engine) = fixture(true).await;
    let error = finish_session(&context, engine.id(), Ok(42))
        .await
        .unwrap_err();
    assert_eq!(error.code, 1);
    assert_eq!(error.message, "Fixture disconnect failure");
    assert_eq!(*engine.subscribe_session().borrow(), SessionPhase::Open);
    assert!(context.session(engine.id()).await.is_ok());
    let error = finish_session::<()>(
        &context,
        engine.id(),
        Err(CliError {
            message: "Canceled".into(),
            code: 130,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, 130);
    assert_eq!(error.message, "Canceled");
}
