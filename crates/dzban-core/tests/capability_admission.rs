use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use dzban_core::catalog::{
    Capabilities, CatalogAction, CatalogEntry, CatalogError, CatalogRequest, ObjectKind, Visibility,
};
use dzban_core::error::ExecutionError;
use dzban_core::execution::{QueryEngine, QueryPhase};
use dzban_core::{QueryPlan, ResultSet};
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Default)]
struct Session {
    started: Notify,
    finish: Notify,
    supports_tables: bool,
}

#[async_trait]
impl DatabaseSession for Session {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            plain_explain: false,
            cancellation: false,
            catalog: if self.supports_tables {
                [CatalogAction::List(ObjectKind::Table)]
                    .into_iter()
                    .collect()
            } else {
                Default::default()
            },
        }
    }
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(0)
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        panic!("Unsupported plan reached adapter")
    }
    async fn cancel(&self) -> Result<(), String> {
        panic!("Unsupported cancellation reached adapter")
    }
    async fn catalog(&self, request: CatalogRequest) -> Result<Vec<CatalogEntry>, CatalogError> {
        assert!(self.supports_tables);
        assert_eq!(request.action(), CatalogAction::List(ObjectKind::Table));
        Ok(Vec::new())
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        self.started.notify_one();
        self.finish.notified().await;
        Ok(Default::default())
    }
}

#[tokio::test]
async fn a_supported_empty_catalog_remains_distinct_from_an_unsupported_kind() {
    let engine = QueryEngine::new(Arc::new(Session {
        supports_tables: true,
        ..Default::default()
    }));
    let request = |kind| CatalogRequest::List {
        kind,
        database: None,
        schema: None,
        glob: None,
        visibility: Visibility::UserObjects,
    };
    assert!(
        engine
            .catalog(request(ObjectKind::Table))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        engine
            .catalog(request(ObjectKind::Database))
            .await
            .unwrap_err(),
        CatalogError::Unsupported(CatalogAction::List(ObjectKind::Database))
    );
    assert!(engine.ping_detailed().await.is_ok());
}

#[tokio::test]
async fn unsupported_actions_are_rejected_by_core_not_only_hidden_by_frontends() {
    let session = Arc::new(Session::default());
    let engine = Arc::new(QueryEngine::new(session.clone()));
    assert_eq!(
        engine.preview_detailed("SELECT 1").await.unwrap_err(),
        ExecutionError::Unsupported("query.plan".into())
    );
    let request = CatalogRequest::List {
        kind: ObjectKind::Table,
        database: None,
        schema: None,
        glob: None,
        visibility: Visibility::UserObjects,
    };
    assert_eq!(
        engine.catalog(request).await.unwrap_err(),
        CatalogError::Unsupported(CatalogAction::List(ObjectKind::Table))
    );
    let query = engine.start_detailed("SELECT 1".into()).unwrap();
    session.started.notified().await;
    let id = query.id;
    assert_eq!(
        engine.subscribe().borrow().planning_error,
        Some(ExecutionError::Unsupported("query.plan".into()))
    );
    assert_eq!(
        engine.cancel_detailed(id).await.unwrap_err(),
        ExecutionError::Unsupported("query.cancel".into())
    );
    assert_eq!(engine.subscribe().borrow().phase, QueryPhase::Executing(id));
    session.finish.notify_one();
    assert!(query.wait_detailed().await.is_ok());
    assert!(engine.cancel_detailed(id).await.is_ok());
    assert!(engine.ping_detailed().await.is_ok());
}
