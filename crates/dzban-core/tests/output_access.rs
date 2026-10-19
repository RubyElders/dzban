use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use dzban_core::adapter::{AdapterFactory, AdapterRegistry, DatabaseSession};
use dzban_core::application::ApplicationContext;
use dzban_core::commands::HistoryLocation;
use dzban_core::error::ExecutionError;
use dzban_core::history::{QueryHistoryEntry, QueryHistoryStore, QueryResultSnapshot};
use dzban_core::outputs::{CELL_PREVIEW_SCALARS, OutputError, ResultReader, ResultScope};
use dzban_core::outputs::{OutputExport, RowRange};
use dzban_core::result_stream::ResultSink;
use dzban_core::{
    CommandAcknowledgment, DatabaseBackend, DatabaseConfig, QueryPlan, ResultCell, ResultColumn,
    ResultSet,
};
use serde_json::json;

#[derive(Default)]
struct Session {
    queries: AtomicUsize,
    plan_entered: tokio::sync::Notify,
    plan_release: tokio::sync::Notify,
}
#[async_trait]
impl DatabaseSession for Session {
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
    async fn cancel(&self) -> Result<(), String> {
        self.plan_release.notify_one();
        Ok(())
    }
    async fn explain(&self, sql: &str) -> Result<QueryPlan, String> {
        if sql == "cancel during plan" {
            self.plan_entered.notify_one();
            self.plan_release.notified().await;
        }
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: json!([]),
            estimate: Default::default(),
        })
    }
    async fn execute(&self, _: &str) -> Result<ResultSet, String> {
        panic!("Result access requires streaming execution")
    }
    async fn execute_streamed(
        &self,
        sql: &str,
        sink: &mut ResultSink,
    ) -> Result<Option<CommandAcknowledgment>, ExecutionError> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        if sql == "native cancellation" {
            return Err(ExecutionError::Database(dzban_core::error::DatabaseError {
                backend: DatabaseBackend::Postgresql,
                kind: dzban_core::error::ErrorKind::Canceled,
                message: "canceled".into(),
                code: None,
                detail: None,
                hint: None,
            }));
        }
        if sql == "failed execution" {
            return Err(ExecutionError::Message("query rejected".into()));
        }

        if sql == "wide export" {
            sink.schema(
                (0..12)
                    .map(|column| ResultColumn {
                        name: format!("column_{column}"),
                        type_name: "text".into(),
                    })
                    .collect(),
            )
            .await?;
            for row in 0..130 {
                sink.row(
                    (0..12)
                        .map(|column| ResultCell::Text(format!("{row}:{column}|😀")))
                        .collect(),
                )
                .await?;
            }
            return Ok(None);
        }
        if sql == "large export" {
            sink.schema(vec![ResultColumn {
                name: "value".into(),
                type_name: "text".into(),
            }])
            .await?;
            sink.row(vec![ResultCell::Text("&".repeat(7 * 1024 * 1024))])
                .await?;
            return Ok(None);
        }
        sink.schema(vec![
            ResultColumn {
                name: "value".into(),
                type_name: "text".into(),
            },
            ResultColumn {
                name: "detail".into(),
                type_name: "text".into(),
            },
        ])
        .await?;
        sink.row(vec![ResultCell::Null, ResultCell::Text("NULL".into())])
            .await?;
        sink.row(vec![
            ResultCell::Text("žluť\r\n😀é".into()),
            ResultCell::Text("😀".repeat(1000)),
        ])
        .await?;
        Ok(None)
    }
}
struct Factory(Arc<Session>);
impl AdapterFactory for Factory {
    fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgresql
    }
    fn create(&self, _: DatabaseConfig) -> Result<Arc<dyn DatabaseSession>, String> {
        Ok(self.0.clone())
    }
}
struct Fixture {
    context: Arc<ApplicationContext>,
    session: Arc<Session>,
    directory: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let session = Arc::new(Session::default());
        let mut adapters = AdapterRegistry::default();
        adapters
            .register(Arc::new(Factory(session.clone())))
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("dzban-output-tests-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        Self {
            context: Arc::new(ApplicationContext::new(adapters)),
            session,
            directory,
        }
    }
    async fn connect(&self) -> dzban_core::tabs::QueryTab {
        let engine = self
            .context
            .start_connect_tab(DatabaseConfig::from_connection_string(
                "host=localhost user=fixture dbname=fixture",
            ))
            .wait()
            .await
            .unwrap();
        self.context.tabs.session(engine.id()).unwrap()
    }
    fn location(&self) -> HistoryLocation {
        HistoryLocation::File(self.directory.join("history.json"))
    }
    async fn history(&self, tab: &dzban_core::tabs::QueryTab) -> QueryHistoryStore {
        self.location()
            .load(tab.profile.as_ref().unwrap().0.clone())
            .await
            .unwrap()
    }
    async fn reader(&self, tab: &dzban_core::tabs::QueryTab) -> ResultReader {
        tab.submit(
            "SELECT 1".into(),
            dzban_core::result_store::ResultLimits {
                memory_rows: 1,
                ..Default::default()
            },
            None,
        )
        .unwrap()
        .wait()
        .await
        .unwrap();
        self.context.result_reader(scope(tab)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}
fn scope(tab: &dzban_core::tabs::QueryTab) -> ResultScope {
    ResultScope {
        session: tab.engine.as_ref().unwrap().id(),
        document: tab.document.id(),
        generation: tab.document.snapshot().output_generation,
    }
}

async fn exported(mut export: OutputExport) -> (String, bool) {
    let mut text = String::new();
    loop {
        let chunk = export.next_chunk().await.unwrap();
        assert!(chunk.text.len() <= 65_536);
        text.push_str(&chunk.text);
        if chunk.done {
            return (text, chunk.is_null);
        }
    }
}

#[tokio::test]
async fn portable_exports_preserve_cells_markdown_selection_and_retirement() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let reader = fixture.reader(&tab).await;
    assert_eq!(
        exported(reader.export_cell(0, 0).await.unwrap()).await,
        (String::new(), true)
    );
    assert_eq!(
        exported(reader.export_cell(0, 1).await.unwrap()).await,
        ("NULL".into(), false)
    );
    let (text, is_null) = exported(
        reader
            .export_markdown(vec![RowRange { start: 1, end: 2 }])
            .await
            .unwrap(),
    )
    .await;
    assert!(!is_null);
    assert_eq!(
        text,
        format!(
            "| value | detail |\n| --- | --- |\n| žluť<br>😀é | {} |\n",
            "😀".repeat(1000)
        )
    );
    assert!(matches!(
        reader
            .export_markdown(vec![RowRange { start: 1, end: 1 }])
            .await,
        Err(OutputError::InvalidSelection)
    ));
    assert!(matches!(
        reader
            .export_markdown(vec![
                RowRange { start: 1, end: 2 },
                RowRange { start: 0, end: 1 }
            ])
            .await,
        Err(OutputError::InvalidSelection)
    ));
    let mut invalid = reader
        .export_markdown(vec![RowRange { start: 2, end: 3 }])
        .await
        .unwrap();
    assert!(!invalid.next_chunk().await.unwrap().done);
    assert_eq!(
        invalid.next_chunk().await,
        Err(OutputError::InvalidSelection)
    );
    assert_eq!(
        invalid.next_chunk().await,
        Err(OutputError::InvalidSelection)
    );
    let mut retired = reader
        .export_markdown(vec![RowRange { start: 0, end: 2 }])
        .await
        .unwrap();
    retired.next_chunk().await.unwrap();
    tab.document.replace_output();
    assert_eq!(retired.next_chunk().await, Err(OutputError::StaleOutput));
}

#[tokio::test]
async fn saved_export_chunks_unicode_and_preserves_markdown_escaping() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let mut history = fixture.history(&tab).await;
    let value = "😀|&\\`*_[]\r\n<z>".repeat(6000);
    history
        .add(QueryHistoryEntry::success(
            "saved".into(),
            Duration::ZERO,
            1,
            QueryResultSnapshot {
                columns: vec!["<&|".into()],
                rows: vec![vec![value.clone()]],
                total_rows: 1,
            },
            None,
            None,
        ))
        .unwrap();
    let listed = fixture
        .context
        .history_list(scope(&tab).session, fixture.location())
        .await
        .unwrap();
    fixture
        .context
        .restore_history(scope(&tab).session, tab.document.id(), listed.revision, 0)
        .unwrap();
    let reader = fixture.context.result_reader(scope(&tab)).unwrap();
    assert_eq!(
        exported(reader.export_cell(0, 0).await.unwrap()).await,
        (value, false)
    );
    let (markdown, _) = exported(
        reader
            .export_markdown(vec![RowRange { start: 0, end: 1 }])
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        markdown,
        format!(
            "| &lt;&amp;\\| |\n| --- |\n| {} |\n",
            "😀\\|&amp;\\\\\\`\\*\\_\\[\\]<br>&lt;z&gt;".repeat(6000)
        )
    );
}

#[tokio::test]
async fn live_output_reader_preserves_null_unicode_offscreen_values_and_bounds() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let reader = fixture.reader(&tab).await;
    let viewport = reader.viewport(0, 2, 0, 2).await.unwrap();
    assert_eq!(viewport.document, tab.document.id().0);
    assert_eq!(viewport.rows[0][0].text, None);
    assert_eq!(viewport.rows[0][1].text.as_deref(), Some("NULL"));
    assert_eq!(viewport.rows[1][0].text.as_deref(), Some("žluť\r\n😀é"));
    assert_eq!(viewport.rows[1][1].length, 1000);
    assert!(viewport.rows[1][1].truncated);
    assert_eq!(
        viewport.rows[1][1].text.as_ref().unwrap().chars().count(),
        CELL_PREVIEW_SCALARS
    );
    assert_eq!(
        reader.original_cell(1, 1).await.unwrap(),
        ResultCell::Text("😀".repeat(1000))
    );
    assert_eq!(
        reader.cell(1, 1, 999, 1).await.unwrap().text.as_deref(),
        Some("😀")
    );
    assert_eq!(reader.cell(0, 0, 0, 10).await.unwrap().total, 0);
    assert!(matches!(
        reader.cell(0, 0, 1, 10).await,
        Err(OutputError::InvalidChunk(_))
    ));
    assert!(matches!(
        reader.cell(0, 2, 0, 10).await,
        Err(OutputError::InvalidCell)
    ));
    assert!(matches!(
        reader.cell(0, 0, 0, 65_537).await,
        Err(OutputError::InvalidChunk(_))
    ));
    assert!(matches!(
        reader.viewport(0, 1, 0, 0).await,
        Err(OutputError::InvalidPage(_))
    ));
    assert!(matches!(
        reader.page(0, 201).await,
        Err(OutputError::InvalidPage(_))
    ));
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(matches!(
        reader.page(0, 1).await,
        Err(OutputError::StaleOutput)
    ));
}

#[tokio::test]
async fn saved_outputs_keep_legacy_null_text_and_survive_history_refresh_without_execution() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let mut history = fixture.history(&tab).await;
    history
        .add(QueryHistoryEntry::success(
            "saved 😀".into(),
            Duration::from_millis(12),
            100,
            QueryResultSnapshot {
                columns: vec!["value".into()],
                rows: vec![vec!["NULL".into()]],
                total_rows: 100,
            },
            Some(json!([{"Plan":{"Node Type":"Result","Plan Rows":1}}])),
            None,
        ))
        .unwrap();
    let listed = fixture
        .context
        .history_list(scope(&tab).session, fixture.location())
        .await
        .unwrap();
    let revision = listed.revision;
    let before = tab.document.snapshot();
    let restored = fixture
        .context
        .restore_history(scope(&tab).session, tab.document.id(), revision, 0)
        .unwrap();
    let workspace = fixture
        .context
        .outputs
        .workspace(&fixture.context.tabs.snapshot());
    let projected = &workspace
        .tabs
        .iter()
        .find(|entry| entry.document == restored.document)
        .unwrap()
        .snapshot;
    assert_eq!(projected, &restored);
    assert_eq!(
        listed.entries[0].status,
        dzban_core::presentation::HistoryOutcomeKind::Success
    );
    assert_eq!(listed.entries[0].rows, Some(100));
    assert_eq!(listed.entries[0].duration_ms, Some(12));
    let saved_scope = scope(&tab);
    assert_eq!(restored.history, Some(true));
    assert_eq!(restored.history_total_rows, Some(100));
    assert_eq!(
        restored.feedback.status,
        dzban_core::presentation::ResultStatusReason::SavedRows
    );
    assert_eq!(restored.feedback.rows, 1);
    assert_eq!(restored.feedback.total_rows, Some(100));
    assert_eq!(restored.rows, 1);
    assert_eq!(
        tab.document.snapshot().draft_revision,
        before.draft_revision + 1
    );
    assert_eq!(saved_scope.generation, before.output_generation + 1);
    assert_eq!(tab.document.draft(), "saved 😀");
    let saved = fixture.context.result_reader(saved_scope).unwrap();
    assert_eq!(
        saved.original_cell(0, 0).await.unwrap(),
        ResultCell::Text("NULL".into())
    );
    let plan = fixture.context.saved_plan(saved_scope).unwrap().unwrap();
    assert!(plan.backend.is_none() && plan.estimate.is_none());
    assert_eq!(plan.text, "Result  (rows=1)");
    history
        .add(QueryHistoryEntry::failure(
            "newest".into(),
            "new failure".into(),
            None,
            None,
        ))
        .unwrap();
    fixture
        .context
        .history_list(saved_scope.session, fixture.location())
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .context
            .restore_history(saved_scope.session, saved_scope.document, revision, 0),
        Err(OutputError::StaleHistory)
    ));
    assert_eq!(
        saved.original_cell(0, 0).await.unwrap(),
        ResultCell::Text("NULL".into())
    );
    assert_eq!(fixture.session.queries.load(Ordering::SeqCst), 0);
    let live = fixture.reader(&tab).await;
    assert!(matches!(
        saved.page(0, 1).await,
        Err(OutputError::StaleOutput)
    ));
    assert_eq!(live.original_cell(0, 0).await.unwrap(), ResultCell::Null);
    assert!(matches!(
        fixture.context.saved_plan(saved_scope),
        Err(OutputError::StaleOutput)
    ));
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn failed_saved_output_and_mixed_tab_targets_do_not_invent_results_or_diagnostics() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let mut history = fixture.history(&tab).await;
    history
        .add(QueryHistoryEntry::failure(
            "failed query".into(),
            "Legacy failure".into(),
            None,
            None,
        ))
        .unwrap();
    let list = fixture
        .context
        .history_list(scope(&tab).session, fixture.location())
        .await
        .unwrap();
    let revision = list.revision;
    let restored = fixture
        .context
        .restore_history(scope(&tab).session, tab.document.id(), revision, 0)
        .unwrap();
    assert_eq!(
        restored.error,
        Some(dzban_core::presentation::DocumentError::Legacy(
            "Legacy failure".into()
        ))
    );
    assert_eq!(
        restored.feedback.status,
        dzban_core::presentation::ResultStatusReason::SavedFailed
    );
    assert_eq!(
        restored.feedback.query_diagnostic.as_ref().unwrap().text,
        "Legacy failure"
    );
    assert!(!restored.result_available);
    assert_eq!(
        restored.termination,
        Some(dzban_core::presentation::CaptureTermination::Failed)
    );
    assert_eq!(restored.execution_seconds, None);
    assert_eq!(restored.received_rows, None);
    assert_eq!(restored.capture_bytes, None);
    let encoded = serde_json::to_value(&restored).unwrap();
    assert_eq!(encoded["error"], "Legacy failure");
    assert_eq!(encoded["received_rows"], serde_json::Value::Null);
    assert_eq!(encoded["capture_bytes"], serde_json::Value::Null);
    let second = fixture.connect().await;
    assert!(matches!(
        fixture.context.result_reader(ResultScope {
            session: scope(&second).session,
            document: tab.document.id(),
            generation: scope(&tab).generation
        }),
        Err(OutputError::UnknownDocument)
    ));
    assert_eq!(fixture.session.queries.load(Ordering::SeqCst), 0);
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(
        fixture
            .context
            .restore_history(scope(&tab).session, tab.document.id(), revision, 0)
            .is_err()
    );
}

#[tokio::test]
async fn retired_history_watchers_cannot_clear_a_newer_saved_selection() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let mut history = fixture.history(&tab).await;
    for value in ["first", "second"] {
        let mut entry = QueryHistoryEntry::success(
            value.into(),
            Duration::ZERO,
            1,
            QueryResultSnapshot {
                columns: vec!["value".into()],
                rows: vec![vec![value.into()]],
                total_rows: 1,
            },
            None,
            None,
        );
        entry.executed_at = if value == "first" { 1 } else { 2 };
        history.add(entry).unwrap();
    }
    let list = fixture
        .context
        .history_list(scope(&tab).session, fixture.location())
        .await
        .unwrap();
    let revision = list.revision;
    fixture
        .context
        .restore_history(scope(&tab).session, tab.document.id(), revision, 1)
        .unwrap();
    fixture
        .context
        .restore_history(scope(&tab).session, tab.document.id(), revision, 0)
        .unwrap();
    tokio::task::yield_now().await;
    let reader = fixture.context.result_reader(scope(&tab)).unwrap();
    assert_eq!(
        reader.original_cell(0, 0).await.unwrap(),
        ResultCell::Text("second".into())
    );
    assert_eq!(
        fixture.context.outputs.document(&tab.document).history,
        Some(true)
    );
    assert_eq!(fixture.session.queries.load(Ordering::SeqCst), 0);
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn replacing_output_without_a_saved_source_never_relabels_the_previous_live_result() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    let original = fixture.reader(&tab).await;
    tab.document.replace_output();
    assert!(matches!(
        fixture.context.result_reader(scope(&tab)),
        Err(OutputError::Unavailable)
    ));
    assert!(matches!(
        original.page(0, 1).await,
        Err(OutputError::StaleOutput)
    ));
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn escaped_export_limit_is_terminal_and_cannot_publish_a_partial_row() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    tab.submit("large export".into(), Default::default(), None)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let reader = fixture.context.result_reader(scope(&tab)).unwrap();
    let mut export = reader
        .export_markdown(vec![RowRange { start: 0, end: 1 }])
        .await
        .unwrap();
    assert!(!export.next_chunk().await.unwrap().done);
    assert_eq!(export.next_chunk().await, Err(OutputError::ExportTooLarge));
    assert_eq!(export.next_chunk().await, Err(OutputError::ExportTooLarge));
}

#[tokio::test]
async fn shared_attention_tracks_completion_without_frontend_or_query_waiters() {
    use dzban_core::attention::AttentionInput;
    let fixture = Fixture::new();
    let first = fixture.connect().await;
    let second = fixture.connect().await;
    fixture
        .context
        .attention()
        .input(AttentionInput::Visibility {
            visible: false,
            viewed: None,
        });
    let mut updates = fixture.context.attention().subscribe();
    let query = first
        .submit("SELECT 1".into(), Default::default(), None)
        .unwrap();
    let id = query.query.0;
    drop(query);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if updates
                .borrow_and_update()
                .tabs
                .iter()
                .any(|tab| tab.unseen.is_some())
            {
                break;
            }
            updates.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let attention = fixture.context.attention().snapshot();
    assert_eq!(attention.request_revision, 1);
    assert_eq!(
        attention
            .tabs
            .iter()
            .find_map(|tab| tab.unseen.as_ref())
            .unwrap()
            .identity
            .query,
        id
    );
    second
        .submit("SELECT 2".into(), Default::default(), None)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(
        fixture
            .context
            .attention()
            .snapshot()
            .tabs
            .iter()
            .filter(|tab| tab.unseen.is_some())
            .count(),
        2
    );
    assert_eq!(fixture.context.attention().snapshot().request_revision, 1);
    fixture
        .context
        .start_close_tab(first.id)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(
        fixture
            .context
            .attention()
            .snapshot()
            .tabs
            .iter()
            .filter(|tab| tab.unseen.is_some())
            .count(),
        1
    );
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
    let attention = fixture.context.attention().snapshot();
    assert!(attention.tabs.is_empty() && attention.cancel_attention);
}

#[tokio::test]
async fn cancellation_during_planning_and_native_cancellation_errors_do_not_notify() {
    use dzban_core::attention::{AttentionInput, CompletionOutcome};
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    fixture
        .context
        .attention()
        .input(AttentionInput::Visibility {
            visible: false,
            viewed: None,
        });
    let query = tab
        .submit("cancel during plan".into(), Default::default(), None)
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        fixture.session.plan_entered.notified(),
    )
    .await
    .unwrap();
    tab.document
        .request_cancel(tab.engine.clone().unwrap())
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(
        query.wait().await.unwrap().result.unwrap_err(),
        ExecutionError::Canceled
    );
    assert_eq!(fixture.session.queries.load(Ordering::SeqCst), 0);
    assert!(
        fixture
            .context
            .attention()
            .snapshot()
            .tabs
            .iter()
            .all(|tab| tab.unseen.is_none())
    );
    assert!(
        tab.submit("native cancellation".into(), Default::default(), None)
            .unwrap()
            .wait()
            .await
            .unwrap()
            .result
            .is_err()
    );
    assert!(
        fixture
            .context
            .attention()
            .snapshot()
            .tabs
            .iter()
            .all(|tab| tab.unseen.is_none())
    );
    assert_eq!(fixture.context.attention().snapshot().request_revision, 0);
    assert!(
        tab.submit("failed execution".into(), Default::default(), None)
            .unwrap()
            .wait()
            .await
            .unwrap()
            .result
            .is_err()
    );
    let attention = fixture.context.attention().snapshot();
    assert_eq!(attention.request_revision, 1);
    assert_eq!(
        attention.tabs[0].unseen.as_ref().unwrap().outcome,
        CompletionOutcome::Failure
    );
    assert_eq!(fixture.session.queries.load(Ordering::SeqCst), 2);
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn restoring_saved_output_suppresses_the_replaced_live_completion() {
    use dzban_core::attention::AttentionInput;
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    fixture
        .context
        .attention()
        .input(AttentionInput::Visibility {
            visible: false,
            viewed: None,
        });
    let mut history = fixture.history(&tab).await;
    history
        .add(QueryHistoryEntry::success(
            "saved".into(),
            Duration::ZERO,
            1,
            QueryResultSnapshot {
                columns: vec!["value".into()],
                rows: vec![vec!["saved value".into()]],
                total_rows: 1,
            },
            None,
            None,
        ))
        .unwrap();
    let listed = fixture
        .context
        .history_list(scope(&tab).session, fixture.location())
        .await
        .unwrap();
    let query = tab
        .submit("cancel during plan".into(), Default::default(), None)
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        fixture.session.plan_entered.notified(),
    )
    .await
    .unwrap();
    fixture
        .context
        .restore_history(scope(&tab).session, tab.document.id(), listed.revision, 0)
        .unwrap();
    fixture.session.plan_release.notify_one();
    query.wait().await.unwrap();
    assert!(
        fixture
            .context
            .attention()
            .snapshot()
            .tabs
            .iter()
            .all(|tab| tab.unseen.is_none())
    );
    assert_eq!(fixture.context.attention().snapshot().request_revision, 0);
    let reader = fixture.context.result_reader(scope(&tab)).unwrap();
    assert_eq!(
        reader.original_cell(0, 0).await.unwrap(),
        ResultCell::Text("saved value".into())
    );
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn markdown_export_includes_all_offscreen_rows_and_columns_across_batches() {
    let fixture = Fixture::new();
    let tab = fixture.connect().await;
    tab.submit(
        "wide export".into(),
        dzban_core::result_store::ResultLimits {
            memory_rows: 1,
            ..Default::default()
        },
        None,
    )
    .unwrap()
    .wait()
    .await
    .unwrap();
    let reader = fixture.context.result_reader(scope(&tab)).unwrap();
    let (markdown, is_null) = exported(
        reader
            .export_markdown(vec![RowRange { start: 0, end: 130 }])
            .await
            .unwrap(),
    )
    .await;
    assert!(!is_null);
    let lines: Vec<_> = markdown.lines().collect();
    assert_eq!(lines.len(), 132);
    assert_eq!(
        lines[0],
        r"| column\_0 | column\_1 | column\_2 | column\_3 | column\_4 | column\_5 | column\_6 | column\_7 | column\_8 | column\_9 | column\_10 | column\_11 |"
    );
    assert_eq!(
        lines[1],
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    );
    for (row, line) in lines[2..].iter().enumerate() {
        let cells: Vec<_> = line
            .strip_prefix("| ")
            .unwrap()
            .strip_suffix(" |")
            .unwrap()
            .split(" | ")
            .collect();
        assert_eq!(cells.len(), 12);
        for (column, cell) in cells.iter().enumerate() {
            assert_eq!(*cell, format!("{row}:{column}\\|😀"));
        }
    }
    fixture
        .context
        .start_shutdown()
        .unwrap()
        .wait()
        .await
        .unwrap();
}
