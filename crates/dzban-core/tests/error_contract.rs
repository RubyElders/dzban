use dzban_core::DatabaseBackend;
use dzban_core::error::{DatabaseError, ErrorKind, NativeErrorCode};

#[test]
fn native_codes_and_diagnostics_survive_serialization_without_parsing_messages() {
    for code in [
        NativeErrorCode::SqlState("42703".into()),
        NativeErrorCode::ClickHouse(60),
    ] {
        let error = DatabaseError {
            backend: match code {
                NativeErrorCode::SqlState(_) => DatabaseBackend::Postgresql,
                NativeErrorCode::ClickHouse(_) => DatabaseBackend::Clickhouse,
            },
            kind: ErrorKind::Server,
            message: "missing object".into(),
            code: Some(code),
            detail: Some("object lookup failed".into()),
            hint: Some("check the name".into()),
        };
        let encoded = serde_json::to_string(&error).unwrap();
        let decoded: DatabaseError = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, error);
        assert!(
            decoded
                .to_string()
                .contains(". Detail: object lookup failed. Hint: check the name")
        );
    }
}

#[test]
fn transport_failures_do_not_invent_a_native_code() {
    let error = DatabaseError {
        backend: DatabaseBackend::Postgresql,
        kind: ErrorKind::Transport,
        message: "connection closed".into(),
        code: None,
        detail: None,
        hint: None,
    };
    assert_eq!(error.to_string(), "connection closed");
}

#[test]
fn optional_estimate_diagnostics_preserve_the_plan_and_legacy_reads() {
    let diagnostic = dzban_core::error::ExecutionError::Database(DatabaseError {
        backend: DatabaseBackend::Clickhouse,
        kind: ErrorKind::Server,
        message: "estimate unavailable".into(),
        code: Some(NativeErrorCode::ClickHouse(497)),
        detail: None,
        hint: None,
    });
    let plan = dzban_core::QueryPlan {
        backend: DatabaseBackend::Clickhouse,
        document: serde_json::json!({"Plan": "fixture"}),
        estimate: dzban_core::QueryEstimate {
            diagnostic: Some(diagnostic.clone()),
            ..Default::default()
        },
    };
    let encoded = serde_json::to_value(&plan).unwrap();
    let decoded: dzban_core::QueryPlan = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded, plan);
    assert_eq!(decoded.estimate.rows, None);
    assert_eq!(decoded.estimate.total_cost, None);
    let entry = dzban_core::history::QueryHistoryEntry::failure(
        "SELECT fixture".into(),
        "fixture".into(),
        Some(encoded),
        None,
    );
    let entry: dzban_core::history::QueryHistoryEntry =
        serde_json::from_value(serde_json::to_value(entry).unwrap()).unwrap();
    let dzban_core::history::QueryHistoryOutcome::Failure {
        explain: Some(explain),
        ..
    } = entry.outcome
    else {
        panic!("Expected retained plan")
    };
    let stored: dzban_core::QueryPlan = serde_json::from_value(explain).unwrap();
    assert_eq!(stored.estimate.diagnostic, Some(diagnostic));
    let legacy: dzban_core::QueryEstimate =
        serde_json::from_value(serde_json::json!({"rows": 42})).unwrap();
    assert_eq!(legacy.rows, Some(42));
    assert_eq!(legacy.diagnostic, None);
}
