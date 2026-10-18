use dzban_core::{DatabaseBackend, QueryEstimate, QueryPlan};
use serde_json::json;

#[test]
fn postgresql_plan_renders_nodes_costs_and_details_as_text() {
    let plan = QueryPlan {
        backend: DatabaseBackend::Postgresql,
        estimate: QueryEstimate::default(),
        document: json!([{"Plan": {
            "Node Type": "Limit", "Startup Cost": 0.0, "Total Cost": 2.5,
            "Plan Rows": 5, "Plan Width": 16,
            "Plans": [{"Node Type": "Index Scan", "Index Name": "widgets_pkey",
                "Schema": "public", "Relation Name": "widgets", "Alias": "w",
                "Startup Cost": 0.0, "Total Cost": 25.0, "Plan Rows": 50,
                "Plan Width": 16, "Index Cond": "(id > 10)", "Output": ["id", "name"]}]
        }, "Planning Time": 0.125}]),
    };
    assert_eq!(
        plan.text(),
        "Limit  (cost=0.00..2.50 rows=5 width=16)\n  ->  Index Scan using widgets_pkey on public.widgets w  (cost=0.00..25.00 rows=50 width=16)\n    Index Cond: (id > 10)\n    Output: id, name\nPlanning Time: 0.125"
    );
}

#[test]
fn clickhouse_plan_keeps_descriptions_and_index_details() {
    let plan = QueryPlan {
        backend: DatabaseBackend::Clickhouse,
        estimate: QueryEstimate::default(),
        document: json!([{"Plan": {"Node Type": "Expression", "Description": "Projection",
            "Plans": [{"Node Type": "ReadFromMergeTree", "Description": "reporting.events",
                "Indexes": [{"Type": "PrimaryKey", "Keys": ["event_id"], "Condition": "event_id > 10", "Granules": 4}]}]}}]),
    };
    let text = plan.text();
    assert!(text.starts_with("Expression\n  Description: Projection\n  ->  ReadFromMergeTree\n"));
    assert!(text.contains("Description: reporting.events"));
    assert!(text.contains("Indexes:\n      Condition: event_id > 10\n      Granules: 4\n      Keys: event_id\n      Type: PrimaryKey"));
    assert!(!text.contains('{'));
    assert!(!text.contains('"'));
}

#[test]
fn text_plans_and_unknown_properties_remain_readable() {
    let mut plan = QueryPlan {
        backend: DatabaseBackend::Postgresql,
        estimate: QueryEstimate::default(),
        document: json!("Result  (cost=0.00..0.01 rows=1 width=4)"),
    };
    assert_eq!(plan.text(), "Result  (cost=0.00..0.01 rows=1 width=4)");
    plan.document = json!({"Extension": {"Custom Detail": "still visible"}});
    assert_eq!(plan.text(), "Extension:\n  Custom Detail: still visible");
    plan.document = json!([]);
    assert_eq!(plan.text(), "No plan details available");
}
