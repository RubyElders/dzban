use dzban_core::documents::{CloseDocument, DocumentExecution, QueryDocuments, QueryExecution};
use dzban_core::execution::QueryId;

fn query() -> QueryId {
    QueryId(uuid::Uuid::new_v4())
}

#[test]
fn documents_keep_drafts_titles_identity_and_order_across_navigation() {
    let mut workspace = QueryDocuments::default();
    let first = workspace.open();
    first.set_draft("SELECT 1;".to_string());
    assert!(first.rename("  My query  "));
    assert!(!first.rename("  "));
    let second = workspace.open();
    assert_ne!(first.id(), second.id());
    assert_eq!(second.snapshot().draft, "");
    assert_eq!(workspace.select_relative(true).unwrap().id(), first.id());
    assert_eq!(workspace.selected().unwrap().snapshot().title, "My query");
    assert_eq!(workspace.selected().unwrap().snapshot().draft, "SELECT 1;");
    assert!(workspace.reorder(second.id(), 0));
    assert_eq!(workspace.documents()[0].id(), second.id());
    assert_eq!(workspace.select_relative(false).unwrap().id(), second.id());
}

#[test]
fn close_policy_preserves_busy_documents_and_selects_a_surviving_neighbor() {
    let mut workspace = QueryDocuments::default();
    let first = workspace.open();
    let second = workspace.open();
    let third = workspace.open();
    let operation = QueryExecution::start(second.clone(), query()).unwrap();
    assert!(workspace.select(second.id()));
    assert_eq!(workspace.close(second.id()), CloseDocument::Busy);
    assert!(operation.finish());
    assert_eq!(
        workspace.close(second.id()),
        CloseDocument::Closed { last: false }
    );
    assert_eq!(workspace.selected().unwrap().id(), third.id());
    assert_eq!(
        workspace.close(third.id()),
        CloseDocument::Closed { last: false }
    );
    assert_eq!(workspace.selected().unwrap().id(), first.id());
    assert_eq!(
        workspace.close(first.id()),
        CloseDocument::Closed { last: true }
    );
    assert!(workspace.selected().is_none());
    assert_eq!(workspace.close(first.id()), CloseDocument::Missing);
    assert_eq!(workspace.open().number(), 4);
}

#[test]
fn background_execution_and_late_cancel_replies_do_not_modify_another_operation() {
    let mut workspace = QueryDocuments::default();
    let first = workspace.open();
    let first_operation = QueryExecution::start(first.clone(), query()).unwrap();
    workspace.open();
    assert!(first.is_active());
    assert!(!workspace.selected().unwrap().is_active());
    assert!(QueryExecution::start(first.clone(), query()).is_none());
    let cancel = first.cancel().unwrap();
    assert!(first.cancel().is_none());
    assert!(first.cancel_failed(cancel));
    assert!(first.cancel().is_some());
    assert!(first_operation.finish());
    let restarted = QueryExecution::start(first.clone(), query()).unwrap();
    assert!(!first_operation.finish());
    assert!(!first.cancel_failed(cancel));
    assert_eq!(
        first.snapshot().execution,
        DocumentExecution::Running(restarted.query)
    );
    assert!(restarted.finish());
}

#[test]
fn replacing_output_rejects_late_results_without_canceling_background_execution() {
    let mut workspace = QueryDocuments::default();
    let document = workspace.open();
    let execution = QueryExecution::start(document.clone(), query()).unwrap();
    let running_output = document.replace_output();
    let history_output = document.replace_output();
    assert!(!document.accepts_output(running_output));
    assert!(!document.set_execution_plan(query(), None));
    let plan = dzban_core::QueryPlan {
        backend: dzban_core::DatabaseBackend::Postgresql,
        document: serde_json::json!([]),
        estimate: Default::default(),
    };
    assert!(document.set_execution_plan(execution.query, Some(plan.clone())));
    assert_eq!(document.plan(), Some(plan));
    assert!(document.accepts_output(history_output));
    assert!(document.is_active());
    assert!(execution.finish());
    assert!(document.set_execution_plan(execution.query, None));
    assert!(!document.set_execution_plan(query(), None));
}

#[test]
fn closed_document_handles_cannot_restart_or_accept_late_output() {
    let mut workspace = QueryDocuments::default();
    let document = workspace.open();
    let output = document.replace_output();
    assert_eq!(
        workspace.close(document.id()),
        CloseDocument::Closed { last: true }
    );
    assert!(!document.start(query()));
    assert!(!document.rename("Lost tab"));
    assert!(!document.set_draft("SELECT 2;".to_string()));
    assert!(!document.accepts_output(output));
    assert!(!document.set_execution_plan(query(), None));
}

#[test]
fn last_document_requests_workspace_close_without_discarding_its_draft() {
    let mut workspace = QueryDocuments::default();
    let document = workspace.open();
    document.set_draft("SELECT 42;".into());
    assert_eq!(
        workspace.close_in_workspace(document.id()),
        CloseDocument::CloseWorkspace
    );
    assert_eq!(workspace.selected().unwrap().id(), document.id());
    assert_eq!(document.draft(), "SELECT 42;");
    let execution = QueryExecution::start(document.clone(), query()).unwrap();
    assert_eq!(
        workspace.close_in_workspace(document.id()),
        CloseDocument::Busy
    );
    execution.finish();
    let neighbor = workspace.open();
    assert_eq!(
        workspace.close_in_workspace(document.id()),
        CloseDocument::Closed { last: false }
    );
    assert_eq!(workspace.selected().unwrap().id(), neighbor.id());
}
