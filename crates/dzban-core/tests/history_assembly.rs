use dzban_core::error::ExecutionError;
use dzban_core::history::{
    CompletedQuery, PendingQueryHistory, QueryHistoryOutcome, QueryResultSnapshot, SuccessfulQuery,
};
use std::time::Duration;

#[test]
fn command_acknowledgment_and_plan_survive_assembly() {
    let mut pending = PendingQueryHistory::new("UPDATE items SET active = true".into());
    let plan = dzban_core::QueryPlan {
        backend: dzban_core::DatabaseBackend::Postgresql,
        document: serde_json::json!([{ "Plan": { "Node Type": "ModifyTable" } }]),
        estimate: dzban_core::QueryEstimate::default(),
    };
    let expected_plan = serde_json::to_value(&plan).unwrap();
    pending.planned(Ok(plan));
    pending.complete(CompletedQuery::Success(SuccessfulQuery {
        command: Some(dzban_core::CommandAcknowledgment {
            affected_rows: Some(2),
        }),
        duration: Duration::from_millis(3),
        rows: 0,
        result: QueryResultSnapshot::default(),
    }));
    let entry = pending.take_entry().unwrap();
    assert!(entry.planning_error.is_none());
    match entry.outcome {
        QueryHistoryOutcome::Success {
            command,
            explain,
            explain_error,
            ..
        } => {
            assert_eq!(command.unwrap().affected_rows, Some(2));
            assert_eq!(explain, Some(expected_plan));
            assert!(explain_error.is_none());
        }
        other => panic!("Unexpected history outcome: {other:?}"),
    }
}

#[test]
fn completion_and_planning_can_arrive_in_either_order() {
    for planning_first in [false, true] {
        let mut pending = PendingQueryHistory::new("SELECT 1".into());
        if planning_first {
            pending.planned(Err(ExecutionError::Message("No plan".into())));
            assert!(pending.take_entry().is_none());
        }
        pending.complete(CompletedQuery::Success(SuccessfulQuery {
            command: None,
            duration: Duration::from_millis(5),
            rows: 1,
            result: QueryResultSnapshot::default(),
        }));
        if !planning_first {
            assert!(pending.take_entry().is_none());
            pending.planned(Err(ExecutionError::Message("No plan".into())));
        }
        pending.complete(CompletedQuery::Failure(ExecutionError::Canceled));
        pending.planned(Err(ExecutionError::Canceled));
        let entry = pending.take_entry().unwrap();
        assert_eq!(entry.sql, "SELECT 1");
        assert_eq!(
            entry.planning_error,
            Some(ExecutionError::Message("No plan".into()))
        );
        assert!(matches!(
            entry.outcome,
            QueryHistoryOutcome::Success { duration_ms: 5, .. }
        ));
        pending.complete(CompletedQuery::Failure(ExecutionError::Canceled));
        pending.planned(Err(ExecutionError::Canceled));
        assert!(pending.take_entry().is_none());
    }
}

#[test]
fn failure_keeps_execution_and_planning_errors_separate() {
    let mut pending = PendingQueryHistory::new("invalid".into());
    pending.complete(CompletedQuery::Failure(ExecutionError::Canceled));
    pending.planned(Err(ExecutionError::Message("Planning failed".into())));
    let entry = pending.take_entry().unwrap();
    assert_eq!(
        entry.planning_error,
        Some(ExecutionError::Message("Planning failed".into()))
    );
    assert!(matches!(
        entry.outcome,
        QueryHistoryOutcome::Failure {
            diagnostic: Some(ExecutionError::Canceled),
            ..
        }
    ));
}
