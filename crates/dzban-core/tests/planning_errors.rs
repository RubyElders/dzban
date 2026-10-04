use std::sync::Arc;

use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use dzban_core::error::{DatabaseError, ErrorKind, ExecutionError, NativeErrorCode};
use dzban_core::execution::QueryEngine;
use dzban_core::history::{QueryHistoryEntry, QueryHistoryOutcome};
use dzban_core::{DatabaseBackend, QueryPlan, ResultSet};

fn diagnostic() -> ExecutionError {
    ExecutionError::Database(DatabaseError {
        backend: DatabaseBackend::Postgresql,
        kind: ErrorKind::Server,
        code: Some(NativeErrorCode::SqlState("0A000".into())),
        message: "Fixture planning is unsupported".into(),
        detail: Some("Fixture detail".into()),
        hint: Some("Fixture hint".into()),
    })
}

struct Session;

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(0)
    }
    async fn ping_detailed(&self) -> Result<u64, ExecutionError> {
        Err(diagnostic())
    }
    async fn execute(&self, sql: &str) -> Result<ResultSet, String> {
        if sql == "failed SQL" {
            return Err("Fixture execution failure".into());
        }
        Ok(Default::default())
    }
    async fn cancel(&self) -> Result<(), String> {
        Ok(())
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn version_detailed(&self) -> Result<String, ExecutionError> {
        Err(diagnostic())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        Err("Legacy string path must not be used".into())
    }
    async fn explain_detailed(&self, _: &str) -> Result<QueryPlan, ExecutionError> {
        Err(diagnostic())
    }
}

#[tokio::test]
async fn execution_timing_is_retained_for_success_failure_and_reset_on_submission() {
    let engine = Arc::new(QueryEngine::new(Arc::new(Session)));
    let result = engine.execute_detailed("valid SQL").await.unwrap();
    let state = engine.subscribe().borrow().clone();
    assert_eq!(state.execution_duration, result.duration);
    assert!(state.elapsed() >= state.planning_duration + state.execution_duration);
    assert!(engine.execute_detailed("failed SQL").await.is_err());
    let failed = engine.subscribe().borrow().clone();
    assert!(failed.execution_duration > std::time::Duration::ZERO);
    assert!(failed.elapsed() >= failed.planning_duration + failed.execution_duration);
    let restart = engine.start_detailed("valid SQL".into()).unwrap();
    let queued = engine.subscribe().borrow().clone();
    assert_eq!(queued.execution_duration, std::time::Duration::ZERO);
    assert_eq!(queued.planning_duration, std::time::Duration::ZERO);
    assert!(restart.wait_detailed().await.is_ok());
}

#[tokio::test]
async fn auxiliary_operations_preserve_diagnostics_and_release_admission() {
    let engine = Arc::new(QueryEngine::new(Arc::new(Session)));
    assert_eq!(engine.ping_detailed().await.unwrap_err(), diagnostic());
    assert_eq!(engine.version_detailed().await.unwrap_err(), diagnostic());
    assert_eq!(engine.ping().await.unwrap_err(), diagnostic().to_string());
    assert_eq!(
        engine.version().await.unwrap_err(),
        diagnostic().to_string()
    );
    assert!(engine.execute_detailed("valid SQL").await.is_ok());
    engine.close().await.unwrap();
    assert_eq!(
        engine.ping_detailed().await.unwrap_err(),
        ExecutionError::Closed
    );
    assert_eq!(
        engine.version_detailed().await.unwrap_err(),
        ExecutionError::Closed
    );
}

#[tokio::test]
async fn typed_planning_failure_does_not_block_sql_and_survives_history() {
    let engine = Arc::new(QueryEngine::new(Arc::new(Session)));
    assert_eq!(
        engine.preview_detailed("valid SQL").await.unwrap_err(),
        diagnostic()
    );
    let result = engine.execute_detailed("valid SQL").await;
    assert!(result.is_ok());
    let state = engine.subscribe().borrow().clone();
    assert_eq!(state.planning_error, Some(diagnostic()));
    assert_eq!(state.explain_error, Some(diagnostic().to_string()));
    assert_eq!(state.execution_error, None);
    let entry = QueryHistoryEntry::from_execution("valid SQL".into(), &result, &state);
    assert!(matches!(entry.outcome, QueryHistoryOutcome::Success { .. }));
    assert_eq!(entry.planning_error, Some(diagnostic()));
    let mut encoded = serde_json::to_value(&entry).unwrap();
    assert_eq!(
        serde_json::from_value::<QueryHistoryEntry>(encoded.clone()).unwrap(),
        entry
    );
    encoded.as_object_mut().unwrap().remove("planning_error");
    assert_eq!(
        serde_json::from_value::<QueryHistoryEntry>(encoded)
            .unwrap()
            .planning_error,
        None
    );
    engine.close().await.unwrap();
    assert_eq!(
        engine.preview_detailed("valid SQL").await.unwrap_err(),
        ExecutionError::Closed
    );
}
