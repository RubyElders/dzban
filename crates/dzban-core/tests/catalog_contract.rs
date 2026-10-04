use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use dzban_core::catalog::{
    CatalogAction, CatalogError, CatalogRequest, Detail, ObjectIdentity, ObjectKind, Visibility,
};
use dzban_core::{QueryPlan, ResultSet};

struct Session;

struct GatedCatalog {
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[async_trait]
impl DatabaseSession for GatedCatalog {
    fn capabilities(&self) -> dzban_core::catalog::Capabilities {
        dzban_core::catalog::Capabilities {
            catalog: [
                CatalogAction::List(ObjectKind::Table),
                CatalogAction::Describe(ObjectKind::Table),
            ]
            .into(),
            ..Default::default()
        }
    }
    async fn connect(&self) -> Result<(), String> {
        Session.connect().await
    }
    async fn ping(&self) -> Result<u64, String> {
        Session.ping().await
    }
    async fn execute(&self, sql: &str) -> Result<ResultSet, String> {
        Session.execute(sql).await
    }
    async fn cancel(&self) -> Result<(), String> {
        Session.cancel().await
    }
    async fn version(&self) -> Result<String, String> {
        Session.version().await
    }
    async fn explain(&self, sql: &str) -> Result<QueryPlan, String> {
        Session.explain(sql).await
    }
    async fn catalog(
        &self,
        request: CatalogRequest,
    ) -> Result<Vec<dzban_core::catalog::CatalogEntry>, CatalogError> {
        match request {
            CatalogRequest::List { .. } | CatalogRequest::Find { .. } => {
                self.entered.notify_one();
                self.resume.notified().await;
                Ok(vec![dzban_core::catalog::CatalogEntry {
                    identity: ObjectIdentity {
                        database: "fixture".into(),
                        schema: None,
                        kind: ObjectKind::Table,
                        name: "table".into(),
                    },
                    attributes: Default::default(),
                }])
            }
            CatalogRequest::Describe { object, detail } => {
                assert_eq!(object.name, "table");
                assert_eq!(detail, Detail::Full);
                Ok(Vec::new())
            }
        }
    }
}

#[tokio::test]
async fn named_description_keeps_admission_owned_after_its_waiter_is_dropped() {
    use dzban_core::execution::{OperationState, QueryEngine};
    use std::sync::Arc;
    let session = Arc::new(GatedCatalog {
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let engine = Arc::new(QueryEngine::new(session.clone()));
    let mut operation = engine.subscribe_operation();
    let running = engine.clone();
    let waiter =
        tokio::spawn(async move { running.describe_named("table".into(), Detail::Full).await });
    session.entered.notified().await;
    waiter.abort();
    let _ = waiter.await;
    assert_eq!(*operation.borrow(), OperationState::Catalog);
    assert!(matches!(
        engine.start_detailed("SELECT 1".into()),
        Err(dzban_core::error::ExecutionError::Busy)
    ));
    session.resume.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while *operation.borrow_and_update() != OperationState::Idle {
            operation.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(engine.ping_detailed().await.is_ok());
}

#[async_trait]
impl DatabaseSession for Session {
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(0)
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        Ok(ResultSet::default())
    }
    async fn cancel(&self) -> Result<(), String> {
        Ok(())
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn explain(&self, _: &str) -> Result<QueryPlan, String> {
        Err("No plan".into())
    }
}

#[tokio::test]
async fn unimplemented_catalog_actions_are_unsupported_not_empty_or_denied() {
    let session: &dyn DatabaseSession = &Session;
    assert!(session.capabilities().catalog.is_empty());
    let requests = [
        CatalogRequest::Find {
            kind: ObjectKind::Table,
            name: "literal ?star*".into(),
        },
        CatalogRequest::List {
            kind: ObjectKind::Table,
            database: Some("exact database".into()),
            schema: None,
            glob: Some("prefix*".into()),
            visibility: Visibility::UserObjects,
        },
        CatalogRequest::Describe {
            object: ObjectIdentity {
                database: "db".into(),
                schema: Some("schema".into()),
                kind: ObjectKind::Index,
                name: "Mixed Case \" index".into(),
            },
            detail: Detail::Full,
        },
    ];
    for request in requests {
        let action = request.action();
        assert_eq!(
            session.catalog(request).await,
            Err(CatalogError::Unsupported(action))
        );
    }
    assert_ne!(
        CatalogError::Unsupported(CatalogAction::List(ObjectKind::Table)),
        CatalogError::PermissionDenied {
            message: "denied".into(),
            code: None
        }
    );
}
#[test]
fn named_objects_require_unique_identity_and_preserve_literal_names() {
    use dzban_core::catalog::{CatalogEntry, ObjectIdentity, ObjectKind, resolve_object};
    let entries = ["public", "other"]
        .into_iter()
        .map(|schema| CatalogEntry {
            identity: ObjectIdentity {
                database: "fixture".into(),
                schema: Some(schema.into()),
                kind: ObjectKind::Table,
                name: "odd's table".into(),
            },
            attributes: Default::default(),
        })
        .collect::<Vec<_>>();
    assert!(
        resolve_object(&entries, "odd's table")
            .unwrap_err()
            .to_string()
            .contains("Ambiguous")
    );
    assert_eq!(
        resolve_object(&entries, "public.odd's table").unwrap(),
        entries[0].identity
    );
    assert!(
        resolve_object(&entries, "missing")
            .unwrap_err()
            .to_string()
            .contains("No matching")
    );
}
