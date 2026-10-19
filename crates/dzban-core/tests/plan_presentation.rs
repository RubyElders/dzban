use dzban_core::presentation::PlanPresentation;
use dzban_core::{DatabaseBackend, QueryEstimate, QueryPlan};
use serde_json::json;

#[test]
fn saved_and_live_plans_have_identical_presentation() {
    for backend in [DatabaseBackend::Postgresql, DatabaseBackend::Clickhouse] {
        let plan = QueryPlan {
            backend,
            document: json!([{"Plan":{"Node Type":"Result","Plan Rows":1,"Startup Cost":0.0,"Total Cost":0.01}}]),
            estimate: QueryEstimate {
                rows: Some(1),
                total_cost: Some(0.01),
                ..Default::default()
            },
        };
        let live = PlanPresentation::from(&plan);
        let saved = PlanPresentation::from_saved(&serde_json::to_value(&plan).unwrap());
        assert_eq!(saved, live);
        assert_eq!(saved.text, "Result  (cost=0.00..0.01 rows=1)");
        let encoded = serde_json::to_value(&live).unwrap();
        assert_eq!(encoded["estimate"]["rows"], 1);
        assert_eq!(
            serde_json::from_value::<PlanPresentation>(encoded).unwrap(),
            live
        );
    }
}

#[test]
fn legacy_saved_documents_remain_readable_without_inventing_metadata() {
    for document in [
        json!([{"Plan":{"Node Type":"Result","Plan Rows":1}}]),
        json!("Result  (cost=0.00..0.01 rows=1)"),
        json!({"Extension":{"Detail":"legacy"}}),
    ] {
        let plan = PlanPresentation::from_saved(&document);
        assert_eq!(plan.document, document);
        assert!(!plan.text.is_empty());
        assert!(!plan.text.contains('{'));
        assert!(plan.backend.is_none());
        assert!(plan.estimate.is_none());
        let encoded = serde_json::to_value(plan).unwrap();
        assert!(encoded.get("backend").is_none());
        assert!(encoded.get("estimate").is_none());
    }
}
