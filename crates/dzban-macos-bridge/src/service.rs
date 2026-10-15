use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dzban_core::application::{ApplicationContext, ApplicationError, ApplicationPhase};
use dzban_core::documents::{DocumentExecution, DocumentId, QueryDocument, QueryDocuments};
use dzban_core::error::ExecutionError;
use dzban_core::execution::{QueryEngine, SessionId};
use dzban_core::history::QueryHistoryStore;
use dzban_core::preview::{PREVIEW_DELAY, PlanPreviewService, PreviewPhase};
use dzban_core::result_store::{MAX_PAGE_ROWS, ResultLimits};
use dzban_core::result_stream::ResultPhase;
use dzban_core::{Config, ConnectionProfileId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::transport::Stream;

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    #[cfg(feature = "test-fixtures")]
    TestPanic {},
    Preview {
        session: Uuid,
        document: Uuid,
        sql: String,
        revision: u64,
    },
    CancelPreview {
        session: Uuid,
    },
    History {
        session: Uuid,
    },
    Profiles {},
    Connect {
        profile: String,
    },
    OpenDocument {
        session: Uuid,
    },
    SetDraft {
        session: Uuid,
        document: Uuid,
        draft: String,
    },
    ObserveDocument {
        session: Uuid,
        document: Uuid,
    },
    Submit {
        session: Uuid,
        document: Uuid,
        sql: String,
    },
    Cancel {
        session: Uuid,
        document: Uuid,
    },
    ReadPage {
        session: Uuid,
        document: Uuid,
        generation: u64,
        start: usize,
        count: usize,
    },
    ReadCell {
        session: Uuid,
        document: Uuid,
        generation: u64,
        row: usize,
        column: usize,
        offset: usize,
        count: usize,
    },
    CloseSession {
        session: Uuid,
    },
    Shutdown {},
}

pub struct App {
    runtime: Option<Runtime>,
    state: Arc<State>,
}

struct Workspace {
    engine: Arc<QueryEngine>,
    documents: QueryDocuments,
    preview: PlanPreviewService,
    profile: String,
}

struct HistoryDirectory(std::path::PathBuf);

impl Drop for HistoryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct State {
    context: Arc<ApplicationContext>,
    config: Mutex<Option<Config>>,
    workspaces: Mutex<HashMap<SessionId, Workspace>>,
    observers: Mutex<Vec<Observer>>,
    history_directory: Option<HistoryDirectory>,
}

struct Observer {
    stream: std::sync::Weak<Stream>,
    task: tokio::task::JoinHandle<()>,
}

fn failure(kind: &str, message: impl ToString) -> Value {
    json!({"failure": {"kind": kind, "message": message.to_string()}})
}

fn execution_error(error: ExecutionError) -> Value {
    json!({"failure": {"kind": "execution", "message": error.to_string(), "diagnostic": error}})
}

fn application_error(kind: &str, error: ApplicationError) -> Value {
    let diagnostic = match &error {
        ApplicationError::Operation(error)
        | ApplicationError::Adapter(dzban_core::adapter::AdapterError::Connect { error, .. }) => {
            Some(error)
        }
        _ => None,
    };
    json!({"failure": {"kind":kind, "message":error.to_string(), "diagnostic":diagnostic}})
}

impl App {
    pub fn new(
        registry: dzban_core::adapter::AdapterRegistry,
        config: Option<Config>,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            runtime: Some(runtime),
            state: Arc::new(State {
                context: Arc::new(ApplicationContext::new(registry)),
                config: Mutex::new(config),
                workspaces: Mutex::new(HashMap::new()),
                observers: Mutex::new(Vec::new()),
                history_directory: None,
            }),
        })
    }

    #[cfg(feature = "test-fixtures")]
    pub fn with_fixture_history(mut self) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("dzban-bridge-history-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).map_err(|error| error.to_string())?;
        Arc::get_mut(&mut self.state)
            .expect("new app has one state owner")
            .history_directory = Some(HistoryDirectory(path));
        Ok(self)
    }

    pub fn dispatch(self: &Arc<Self>, command: Command, output: Arc<Stream>) {
        let runtime = self.runtime.as_ref().expect("live app has a runtime");
        let state = self.state.clone();
        let lifetime = self.clone();
        if let Command::ObserveDocument { session, document } = command {
            let mut observers = self.state.observers.lock().unwrap();
            if *self.state.context.subscribe_phase().borrow() != ApplicationPhase::Running {
                output.publish(failure("closed", "Application is closing"));
                return;
            }
            observers.retain(|observer| !observer.task.is_finished());
            let stream = Arc::downgrade(&output);
            let task = runtime
                .spawn(async move { state.observe_document(session, document, output).await });
            observers.push(Observer { stream, task });
            return;
        }
        let task = runtime.spawn(async move { state.command(command).await });
        runtime.spawn(async move {
            let result = match task.await {
                Ok(result) => result,
                Err(_) => failure("worker_failed", "Bridge worker failed"),
            };
            output.publish(result);
            drop(lifetime);
        });
    }

    pub fn is_closed(&self) -> bool {
        *self.state.context.subscribe_phase().borrow() == ApplicationPhase::Closed
    }
}

impl Drop for App {
    fn drop(&mut self) {
        for observer in self.state.observers.lock().unwrap().drain(..) {
            if let Some(stream) = observer.stream.upgrade() {
                stream.close();
            }
            observer.task.abort();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl State {
    async fn history(&self, session: Uuid) -> Result<QueryHistoryStore, Value> {
        let profile = self
            .workspaces
            .lock()
            .unwrap()
            .get(&SessionId(session))
            .map(|workspace| workspace.profile.clone())
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        let path = self
            .history_directory
            .as_ref()
            .map(|directory| directory.0.join("history.json"));
        tokio::task::spawn_blocking(move || match path {
            Some(path) => QueryHistoryStore::load_from_path(path, &profile),
            None => QueryHistoryStore::load(&profile),
        })
        .await
        .map_err(|_| failure("worker_failed", "History worker failed"))?
        .map_err(|error| failure("history", error))
    }
    async fn observe_document(&self, session: Uuid, document: Uuid, stream: Arc<Stream>) {
        let document = match self.document(session, document) {
            Ok((_, document)) => document,
            Err(error) => {
                stream.publish(error);
                return;
            }
        };
        let mut receiver = document.subscribe();
        stream.publish(document_snapshot(&document));
        loop {
            tokio::select! {
                _ = stream.closed() => break,
                changed = receiver.changed() => {
                    if changed.is_err() { break; }
                    stream.publish(document_snapshot(&document));
                }
            }
        }
    }
    fn document(
        &self,
        session: Uuid,
        document: Uuid,
    ) -> Result<(Arc<QueryEngine>, QueryDocument), Value> {
        let workspaces = self.workspaces.lock().unwrap();
        let workspace = workspaces
            .get(&SessionId(session))
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        let document = workspace
            .documents
            .documents()
            .iter()
            .find(|item| item.id() == DocumentId(document))
            .cloned()
            .ok_or_else(|| failure("unknown_document", "Document is not open"))?;
        Ok((workspace.engine.clone(), document))
    }

    async fn command(&self, command: Command) -> Value {
        match self.execute(command).await {
            Ok(value) => value,
            Err(value) => value,
        }
    }

    async fn execute(&self, command: Command) -> Result<Value, Value> {
        if *self.context.subscribe_phase().borrow() == ApplicationPhase::Closed {
            return Err(failure("closed", "Application is closed"));
        }
        match command {
            #[cfg(feature = "test-fixtures")]
            Command::TestPanic {} => panic!("Fixture bridge worker panic"),
            Command::Profiles {} => {
                let current = self.config.lock().unwrap().clone();
                let config = if let Some(config) = current {
                    config
                } else {
                    let config = tokio::task::spawn_blocking(Config::load)
                        .await
                        .map_err(|_| failure("worker_failed", "Configuration worker failed"))?
                        .map_err(|error| failure("configuration", error))?;
                    *self.config.lock().unwrap() = Some(config.clone());
                    config
                };
                Ok(
                    json!({"profiles": config.databases.iter().map(|profile| json!({"name": profile.name, "backend": profile.backend})).collect::<Vec<_>>()}),
                )
            }
            Command::Connect { profile } => {
                let config =
                    self.config.lock().unwrap().clone().ok_or_else(|| {
                        failure("configuration", "Load profiles before connecting")
                    })?;
                let profile = config
                    .resolve_profile(&ConnectionProfileId(profile), None)
                    .map_err(|error| failure("profile", error))?;
                let profile_name = profile.name.clone();
                let connection = self.context.start_connect(profile);
                let request = connection.id.0;
                let engine = connection
                    .wait()
                    .await
                    .map_err(|error| application_error("connection", error))?;
                let session = engine.id();
                self.workspaces.lock().unwrap().insert(
                    session,
                    Workspace {
                        engine,
                        documents: QueryDocuments::default(),
                        preview: PlanPreviewService::default(),
                        profile: profile_name,
                    },
                );
                Ok(json!({"request": request, "session": session.0}))
            }
            Command::OpenDocument { session } => {
                let mut workspaces = self.workspaces.lock().unwrap();
                let workspace = workspaces
                    .get_mut(&SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                let document = workspace.documents.open();
                Ok(document_snapshot(&document))
            }
            Command::SetDraft {
                session,
                document,
                draft,
            } => {
                let (_, document) = self.document(session, document)?;
                document.set_draft(draft);
                Ok(document_snapshot(&document))
            }
            Command::ObserveDocument { .. } => Err(failure(
                "invalid_command",
                "Observation must use a streaming response",
            )),
            Command::Submit {
                session,
                document,
                sql,
            } => {
                let (engine, document) = self.document(session, document)?;
                let history = self.history(session).await?;
                let query = document
                    .submit(engine, sql, ResultLimits::default(), Some(history))
                    .map_err(execution_error)?;
                let result = json!({"query": query.query.0, "document": query.document.0, "session": query.session.0, "generation": query.generation});
                tokio::spawn(async move {
                    let _ = query.wait().await;
                });
                Ok(result)
            }
            Command::Cancel { session, document } => {
                let (engine, document) = self.document(session, document)?;
                document
                    .request_cancel(engine)
                    .map_err(execution_error)?
                    .wait()
                    .await
                    .map_err(execution_error)?;
                Ok(json!({"cancellation_requested": true}))
            }
            Command::ReadPage {
                session,
                document,
                generation,
                start,
                count,
            } => {
                if count > MAX_PAGE_ROWS {
                    return Err(failure("invalid_page", "Page exceeds MAX_PAGE_ROWS"));
                }
                let (_, document) = self.document(session, document)?;
                let snapshot = document.snapshot();
                if snapshot.output_generation != generation {
                    return Err(failure("stale_output", "Output generation has changed"));
                }
                let result = snapshot
                    .operation
                    .and_then(|operation| operation.capture)
                    .and_then(|capture| capture.result)
                    .ok_or_else(|| failure("result_unavailable", "Result is not available"))?;
                let page = result
                    .read_page(start, count)
                    .await
                    .map_err(execution_error)?;
                if !document.accepts_output(generation) {
                    return Err(failure("stale_output", "Output generation has changed"));
                }
                let payload = json!({"document": document.id().0, "generation": generation, "start": start, "page": page});
                if serde_json::to_vec(&payload).unwrap().len()
                    > crate::transport::MAX_RECORD_BYTES - 256
                {
                    return Err(failure(
                        "page_too_large",
                        "Request a smaller page or a cell chunk",
                    ));
                }
                Ok(payload)
            }
            Command::CloseSession { session } => {
                self.context
                    .close(SessionId(session))
                    .await
                    .map_err(|error| application_error("close", error))?;
                let mut workspaces = self.workspaces.lock().unwrap();
                if let Some(workspace) = workspaces.get_mut(&SessionId(session)) {
                    workspace
                        .documents
                        .retire_closed_session(&workspace.engine)
                        .map_err(execution_error)?;
                }
                workspaces.remove(&SessionId(session));
                Ok(json!({"closed": session}))
            }
            Command::History { session } => {
                let history = self.history(session).await?;
                Ok(json!({"entries": history.entries()}))
            }
            Command::CancelPreview { session } => {
                let workspaces = self.workspaces.lock().unwrap();
                let workspace = workspaces
                    .get(&SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                workspace.preview.cancel();
                Ok(json!({"preview_canceled":true}))
            }
            Command::Preview {
                session,
                document,
                sql,
                revision,
            } => {
                let (engine, document) = self.document(session, document)?;
                if document.snapshot().draft_revision != revision {
                    return Err(failure("preview_invalidated", "Draft revision has changed"));
                }
                let service = self
                    .workspaces
                    .lock()
                    .unwrap()
                    .get(&SessionId(session))
                    .map(|workspace| workspace.preview.clone())
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                let mut updates = service.subscribe();
                let target = service
                    .schedule(engine, document.clone(), sql, PREVIEW_DELAY)
                    .map_err(execution_error)?;
                loop {
                    let snapshot = updates.borrow_and_update().clone();
                    if snapshot.target != Some(target)
                        || matches!(snapshot.phase, PreviewPhase::Empty)
                    {
                        return Err(failure(
                            "preview_invalidated",
                            "Preview was dismissed or superseded",
                        ));
                    }
                    match &snapshot.phase {
                        PreviewPhase::Ready(plan) if service.accepts(&snapshot, &document) => {
                            return Ok(
                                json!({"request":target.request.0, "session":target.session.0, "document":target.document.0, "draft_revision":target.revision, "plan":plan}),
                            );
                        }
                        PreviewPhase::Ready(_) => {
                            return Err(failure(
                                "preview_invalidated",
                                "Preview no longer matches the document",
                            ));
                        }
                        PreviewPhase::Failed(error) => return Err(execution_error(error.clone())),
                        _ => (),
                    }
                    updates
                        .changed()
                        .await
                        .map_err(|_| failure("preview_invalidated", "Preview observation ended"))?;
                }
            }
            Command::ReadCell {
                session,
                document,
                generation,
                row,
                column,
                offset,
                count,
            } => {
                if count > 65_536 {
                    return Err(failure(
                        "invalid_chunk",
                        "Cell chunks are limited to 65536 Unicode scalars",
                    ));
                }
                let (_, document) = self.document(session, document)?;
                let snapshot = document.snapshot();
                if snapshot.output_generation != generation {
                    return Err(failure("stale_output", "Output generation has changed"));
                }
                let result = snapshot
                    .operation
                    .and_then(|operation| operation.capture)
                    .and_then(|capture| capture.result)
                    .ok_or_else(|| failure("result_unavailable", "Result is not available"))?;
                let page = result.read_page(row, 1).await.map_err(execution_error)?;
                if !document.accepts_output(generation) {
                    return Err(failure("stale_output", "Output generation has changed"));
                }
                let cell = page
                    .rows
                    .first()
                    .and_then(|row| row.get(column))
                    .ok_or_else(|| failure("invalid_cell", "Cell is outside the result"))?;
                let (text, total) = match cell {
                    dzban_core::ResultCell::Null => (None, 0),
                    dzban_core::ResultCell::Text(text) => (
                        Some(text.chars().skip(offset).take(count).collect::<String>()),
                        text.chars().count(),
                    ),
                };
                if offset > total {
                    return Err(failure("invalid_chunk", "Cell offset is outside the value"));
                }
                Ok(
                    json!({"document":document.id().0, "generation":generation, "row":row, "column":column, "offset":offset, "total":total, "text":text}),
                )
            }
            Command::Shutdown {} => {
                self.context
                    .start_shutdown()
                    .map_err(|error| application_error("shutdown", error))?
                    .wait()
                    .await
                    .map_err(|error| application_error("shutdown", error))?;
                {
                    let mut workspaces = self.workspaces.lock().unwrap();
                    for workspace in workspaces.values_mut() {
                        workspace
                            .documents
                            .retire_closed_session(&workspace.engine)
                            .map_err(execution_error)?;
                    }
                    workspaces.clear();
                }
                let observers = std::mem::take(&mut *self.observers.lock().unwrap());
                for observer in observers {
                    if let Some(stream) = observer.stream.upgrade() {
                        stream.close();
                    }
                    observer.task.abort();
                    let _ = observer.task.await;
                }
                Ok(json!({"shutdown": true}))
            }
        }
    }
}

fn document_snapshot(document: &QueryDocument) -> Value {
    let snapshot = document.snapshot();
    let execution = match snapshot.execution {
        DocumentExecution::Idle => "idle",
        DocumentExecution::Planning(_) => "planning",
        DocumentExecution::Running(_) => "running",
        DocumentExecution::Canceling(_) => "canceling",
        DocumentExecution::Finished(_) => "finished",
    };
    let capture = snapshot
        .operation
        .as_ref()
        .and_then(|operation| operation.capture.as_ref());
    let result_phase = capture.map(|capture| match capture.phase {
        ResultPhase::WaitingForSchema => "waiting_for_schema",
        ResultPhase::Receiving => "receiving",
        ResultPhase::Draining(_) => "draining",
        ResultPhase::FailedAwaitingCleanup(_) => "failed_awaiting_cleanup",
        ResultPhase::Finished(_) => "finished",
    });
    let error = snapshot
        .completion
        .as_ref()
        .and_then(|completion| match completion {
            Err(error) => Some(error.clone()),
            Ok(completion) => completion.result.as_ref().err().cloned(),
        });
    json!({
        "document": snapshot.id.0,
        "session": snapshot.session.map(|session| session.0),
        "query": snapshot.execution.query().map(|query| query.0),
        "generation": snapshot.output_generation,
        "draft_revision": snapshot.draft_revision,
        "title": snapshot.title,
        "draft": snapshot.draft,
        "execution": execution,
        "result_phase": result_phase,
        "rows": capture.map_or(0, |capture| capture.rows),
        "cutoff": capture.and_then(|capture| capture.cutoff),
        "error": error,
    })
}
