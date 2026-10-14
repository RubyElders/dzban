use std::time::Duration;

use dzban_core::execution::{QueryId, QueryPhase, QuerySnapshot, SessionId};
use dzban_core::history::{
    MAX_HISTORY_RESULT_ROWS, QueryHistoryEntry, QueryHistoryOutcome, QueryResultSnapshot,
};
use dzban_core::{DatabaseBackend, QueryEstimate, QueryPlan, QueryResult, ResultSet};
use uuid::Uuid;

fn snapshot() -> QuerySnapshot {
    QuerySnapshot {
        capture: None,
        execution_error: None,
        planning_error: None,
        session: SessionId(Uuid::new_v4()),
        phase: QueryPhase::Completed(QueryId(Uuid::new_v4())),
        plan: Some(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!({"fixture": true}),
            estimate: QueryEstimate::default(),
        }),
        explain_error: Some("planning diagnostic".into()),
        planning_duration: Duration::from_millis(10),
        execution_duration: Duration::ZERO,
        started_at: None,
        finished_at: None,
    }
}

#[test]
fn execution_history_preserves_success_failure_and_plan_metadata() {
    let state = snapshot();
    let result: Result<_, String> = Ok(QueryResult {
        command: None,
        result_set: ResultSet::default(),
        duration: Duration::from_millis(42),
    });
    let entry = QueryHistoryEntry::from_execution("SELECT 1".into(), &result, &state);
    assert_eq!(entry.sql, "SELECT 1");
    let QueryHistoryOutcome::Success {
        duration_ms,
        rows,
        result,
        explain,
        explain_error,
        ..
    } = entry.outcome
    else {
        panic!("success expected")
    };
    assert_eq!(duration_ms, 42);
    assert_eq!(rows, 0);
    assert!(result.is_some());
    assert!(explain.is_some());
    assert_eq!(explain_error.as_deref(), Some("planning diagnostic"));
    let failure = QueryHistoryEntry::from_execution(
        "SELECT invalid".into(),
        &Err("SQL error".to_string()),
        &state,
    );
    let QueryHistoryOutcome::Failure {
        error,
        explain,
        explain_error,
        diagnostic,
    } = failure.outcome
    else {
        panic!("failure expected")
    };
    assert_eq!(error, "SQL error");
    assert!(diagnostic.is_none());
    assert!(explain.is_some());
    assert_eq!(explain_error.as_deref(), Some("planning diagnostic"));
}

#[test]
fn frontend_supplied_snapshots_use_the_same_retention_policy() {
    let rows = MAX_HISTORY_RESULT_ROWS + 50;
    let entry = QueryHistoryEntry::success(
        "SELECT many".into(),
        Duration::ZERO,
        rows,
        QueryResultSnapshot {
            columns: vec!["value".into()],
            rows: vec![vec!["1".into()]; rows],
            total_rows: rows,
        },
        None,
        None,
    );
    let QueryHistoryOutcome::Success {
        rows: total,
        result: Some(result),
        ..
    } = entry.outcome
    else {
        panic!("success expected")
    };
    assert_eq!(total, rows);
    assert_eq!(result.total_rows, rows);
    assert_eq!(result.rows.len(), MAX_HISTORY_RESULT_ROWS);
}

#[test]
fn command_acknowledgments_survive_history_serialization() {
    let result: Result<_, String> = Ok(QueryResult {
        result_set: ResultSet::default(),
        duration: Duration::ZERO,
        command: Some(dzban_core::CommandAcknowledgment {
            affected_rows: Some(7),
        }),
    });
    let entry = QueryHistoryEntry::from_execution("UPDATE fixture".into(), &result, &snapshot());
    let encoded = serde_json::to_vec(&entry).unwrap();
    let decoded: QueryHistoryEntry = serde_json::from_slice(&encoded).unwrap();
    let QueryHistoryOutcome::Success {
        command: Some(command),
        ..
    } = decoded.outcome
    else {
        panic!("command acknowledgment expected")
    };
    assert_eq!(command.affected_rows, Some(7));
}

#[test]
fn structured_failures_round_trip_and_legacy_history_remains_readable() {
    let error = dzban_core::error::ExecutionError::Database(dzban_core::error::DatabaseError {
        backend: DatabaseBackend::Postgresql,
        kind: dzban_core::error::ErrorKind::Server,
        message: "missing column".into(),
        code: Some(dzban_core::error::NativeErrorCode::SqlState("42703".into())),
        detail: None,
        hint: Some("check the name".into()),
    });
    let mut state = snapshot();
    state.execution_error = Some(error.clone());
    let entry =
        QueryHistoryEntry::from_execution("SELECT missing".into(), &Err(error.clone()), &state);
    let mut encoded = serde_json::to_value(&entry).unwrap();
    let decoded: QueryHistoryEntry = serde_json::from_value(encoded.clone()).unwrap();
    let QueryHistoryOutcome::Failure { diagnostic, .. } = decoded.outcome else {
        panic!("failure expected");
    };
    assert_eq!(diagnostic, Some(error));
    encoded["outcome"]
        .as_object_mut()
        .unwrap()
        .remove("diagnostic");
    let legacy: QueryHistoryEntry = serde_json::from_value(encoded).unwrap();
    let QueryHistoryOutcome::Failure { diagnostic, .. } = legacy.outcome else {
        panic!("failure expected");
    };
    assert!(diagnostic.is_none());
}
