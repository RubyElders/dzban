use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use dzban_core::documents::QueryDocuments;
use dzban_core::error::ExecutionError;
use dzban_core::execution::QueryEngine;
use dzban_core::preview::{
    PREVIEW_HOLD, PeekState, PlanPreviewService, PreviewPhase, PreviewRelease, preview_release,
};
use dzban_core::{DatabaseBackend, QueryPlan, ResultSet};
use tokio::sync::Notify;

#[derive(Default)]
struct Session {
    entered: Notify,
    release: Notify,
    calls: AtomicUsize,
}

#[async_trait]
impl DatabaseSession for Session {
    async fn cancel(&self) -> Result<(), String> {
        Ok(())
    }
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), ExecutionError> {
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(1)
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        Err("not used".into())
    }
    async fn explain(&self, sql: &str) -> Result<QueryPlan, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if sql == "wait" {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if sql == "fail" {
            return Err("Fixture plan rejected".into());
        }
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!({"sql": sql}),
            estimate: Default::default(),
        })
    }
}

async fn until(service: &PlanPreviewService, predicate: impl Fn(&PreviewPhase) -> bool) {
    let mut changes = service.subscribe();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if predicate(&changes.borrow().phase) {
                return;
            }
            changes.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn canceled_debounce_never_reaches_the_adapter() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let document = QueryDocuments::default().open();
    let service = PlanPreviewService::default();
    service
        .schedule(engine, document, "query".into(), Duration::from_millis(10))
        .unwrap();
    service.cancel();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(session.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.snapshot().phase, PreviewPhase::Empty);
}

#[tokio::test]
async fn rejected_requests_preserve_the_current_preview() {
    let engine = Arc::new(QueryEngine::new(Arc::new(Session::default())));
    let service = PlanPreviewService::default();
    let mut documents = QueryDocuments::default();
    let document = documents.open();
    let target = service
        .schedule(
            engine.clone(),
            document.clone(),
            "query".into(),
            Duration::ZERO,
        )
        .unwrap();
    until(&service, |phase| matches!(phase, PreviewPhase::Ready(_))).await;
    assert!(
        service
            .schedule(engine.clone(), document, " \n".into(), Duration::ZERO)
            .is_err()
    );
    let closed = documents.open();
    documents.close(closed.id());
    assert_eq!(
        service.schedule(engine.clone(), closed, "query".into(), Duration::ZERO),
        Err(ExecutionError::Closed)
    );
    let busy = documents.open();
    busy.start(dzban_core::execution::QueryId(uuid::Uuid::new_v4()));
    assert_eq!(
        service.schedule(engine, busy, "query".into(), Duration::ZERO),
        Err(ExecutionError::Busy)
    );
    assert_eq!(service.snapshot().target, Some(target));
    assert!(matches!(service.snapshot().phase, PreviewPhase::Ready(_)));
}

#[tokio::test]
async fn replaced_request_cannot_publish_over_another_session_and_cleanup_keeps_admission() {
    let old_session = Arc::new(Session::default());
    let old_engine = Arc::new(QueryEngine::new(old_session.clone()));
    let service = PlanPreviewService::default();
    let mut documents = QueryDocuments::default();
    let old = documents.open();
    service
        .schedule(
            old_engine.clone(),
            old.clone(),
            "wait".into(),
            Duration::ZERO,
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), old_session.entered.notified())
        .await
        .unwrap();
    let new = documents.open();
    let new_engine = Arc::new(QueryEngine::new(Arc::new(Session::default())));
    let target = service
        .schedule(new_engine, new.clone(), "new".into(), Duration::ZERO)
        .unwrap();
    until(&service, |phase| matches!(phase, PreviewPhase::Ready(_))).await;
    assert_eq!(
        old_engine.ping_detailed().await.unwrap_err(),
        ExecutionError::Busy
    );
    old_session.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while old_engine.ping_detailed().await.is_err() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let snapshot = service.snapshot();
    assert_eq!(snapshot.target, Some(target));
    assert!(service.accepts(&snapshot, &new));
    assert!(!service.accepts(&snapshot, &old));
    let PreviewPhase::Ready(plan) = snapshot.phase else {
        panic!("new preview missing");
    };
    assert_eq!(plan.document["sql"], "new");
}

#[tokio::test]
async fn ready_preview_is_invalidated_by_draft_cursor_workspace_and_close() {
    let engine = Arc::new(QueryEngine::new(Arc::new(Session::default())));
    let service = PlanPreviewService::default();
    let mut documents = QueryDocuments::default();
    let document = documents.open();
    for change in 0..4 {
        service
            .schedule(
                engine.clone(),
                document.clone(),
                "query".into(),
                Duration::ZERO,
            )
            .unwrap();
        until(&service, |phase| matches!(phase, PreviewPhase::Ready(_))).await;
        let ready = service.snapshot();
        match change {
            0 => {
                document.set_draft("SELECT 2".into());
            }
            1 => document.query_context_changed(),
            2 => service.workspace_changed(&documents),
            _ => {
                documents.close(document.id());
            }
        }
        assert!(!service.accepts(&ready, &document));
        until(&service, |phase| *phase == PreviewPhase::Empty).await;
    }
}

#[tokio::test]
async fn failures_are_retained_without_observers_and_ready_services_can_be_dropped() {
    let engine = Arc::new(QueryEngine::new(Arc::new(Session::default())));
    let document = QueryDocuments::default().open();
    let service = PlanPreviewService::default();
    service
        .schedule(
            engine.clone(),
            document.clone(),
            "fail".into(),
            Duration::ZERO,
        )
        .unwrap();
    until(&service, |phase| matches!(phase, PreviewPhase::Failed(_))).await;
    assert!(service.accepts(&service.snapshot(), &document));
    service
        .schedule(engine, document.clone(), "ready".into(), Duration::ZERO)
        .unwrap();
    until(&service, |phase| matches!(phase, PreviewPhase::Ready(_))).await;
    let mut observer = service.subscribe();
    drop(service);
    tokio::time::timeout(Duration::from_secs(2), async {
        while observer.changed().await.is_ok() {}
    })
    .await
    .unwrap();
}

#[test]
fn one_peek_policy_handles_taps_holds_and_pinned_visibility() {
    assert_eq!(
        preview_release(Duration::from_millis(100)),
        PreviewRelease::Pin
    );
    assert_eq!(preview_release(PREVIEW_HOLD), PreviewRelease::Hide);
    let mut peek = PeekState::default();
    assert!(!peek.pin());
    peek.show_transient();
    assert!(peek.pin());
    peek.hide_transient();
    assert!(peek.is_visible());
    assert!(peek.is_pinned());
    assert!(peek.close());
    assert!(!peek.close());
    peek.show_transient();
    peek.hide_transient();
    assert!(!peek.is_visible());
}
