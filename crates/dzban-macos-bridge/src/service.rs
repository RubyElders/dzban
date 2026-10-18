use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dzban_core::application::{
    ApplicationContext, ApplicationError, ApplicationPhase, ConnectionSelection,
};
use dzban_core::documents::{
    CloseDocument, DocumentExecution, DocumentId, QueryDocument, QueryDocuments,
};
use dzban_core::error::ExecutionError;
use dzban_core::execution::{QueryEngine, SessionId};
use dzban_core::history::QueryHistoryStore;
use dzban_core::interaction::{
    AppCommand, AppEvent, AppState, InputRole, PrimaryAction, SessionInteraction,
};
use dzban_core::preview::{PREVIEW_DELAY, PlanPreviewService, PreviewPhase};
use dzban_core::result_store::{MAX_PAGE_ROWS, ResultLimits};
use dzban_core::result_stream::{ResultPhase, ResultTermination};
use dzban_core::{Config, ConnectionProfileId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use uuid::Uuid;

mod native_controls;

use crate::editor::{EditorIntent, selected_sql};
use crate::transport::Stream;

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Tabs {},
    #[cfg(feature = "test-fixtures")]
    TestPanic {},
    #[cfg(feature = "test-fixtures")]
    TestMetrics {},
    Preview {
        session: Uuid,
        document: Uuid,
        sql: String,
        revision: u64,
        start: Option<usize>,
        end: Option<usize>,
        request_revision: Option<u64>,
    },
    Peek {
        session: Uuid,
        surface: crate::catalog::Surface,
        event: crate::catalog::PeekEvent,
        elapsed_ms: Option<u64>,
    },
    CancelPreview {
        session: Uuid,
        request_revision: Option<u64>,
    },
    History {
        session: Uuid,
    },
    HistoryList {
        session: Uuid,
    },
    RestoreHistory {
        session: Uuid,
        document: Uuid,
        history_revision: Uuid,
        index: usize,
    },
    SavedPlan {
        session: Uuid,
        document: Uuid,
        generation: u64,
    },
    Input {
        session: Uuid,
        document: Uuid,
        revision: u64,
        input: native_controls::Input,
    },
    AwaitIdle {
        session: Uuid,
    },
    ProbeSession {
        session: Uuid,
        include_version: bool,
    },
    ProbeProfile {
        profile: String,
    },
    Profiles {},
    Connections {},
    SelectSession {
        session: Option<Uuid>,
        revision: Option<u64>,
    },
    Controls {},
    ObserveControls {
        session: Uuid,
    },
    Actions {
        session: Uuid,
    },
    Catalog {
        session: Uuid,
        request: crate::catalog::Request,
    },
    EditorAction {
        session: Uuid,
        document: Uuid,
        draft: String,
        start: usize,
        end: usize,
        intent: EditorIntent,
    },
    SelectDocument {
        session: Uuid,
        document: Uuid,
        revision: Option<u64>,
    },
    CloseDocument {
        session: Uuid,
        document: Uuid,
    },
    Connect {
        profile: String,
        revision: Option<u64>,
    },
    OpenDocument {
        session: Uuid,
        revision: Option<u64>,
    },
    ContextChanged {
        session: Uuid,
        document: Uuid,
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
    ResultSchema {
        session: Uuid,
        document: Uuid,
        generation: u64,
    },
    ReadViewport {
        session: Uuid,
        document: Uuid,
        generation: u64,
        start: usize,
        count: usize,
        column_start: usize,
        column_count: usize,
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
    selection_revision: u64,
    peeks: [dzban_core::preview::PeekState; 2],
    preview_revision: u64,
    input_document: Option<(DocumentId, u64)>,
    input_state: AppState,
}

#[derive(Default)]
struct NativeSelection {
    connection: ConnectionSelection,
    revision: u64,
}

impl NativeSelection {
    fn accept_revision(&mut self, revision: Option<u64>) -> bool {
        if let Some(revision) = revision {
            if revision < self.revision {
                return false;
            }
            self.revision = revision;
        }
        true
    }
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
    injected_config: bool,
    workspaces: Mutex<HashMap<SessionId, Workspace>>,
    observers: Mutex<Vec<Observer>>,
    history_directory: Mutex<Option<HistoryDirectory>>,
    selection: Mutex<NativeSelection>,
    interaction: Mutex<AppState>,
    readers: tokio::sync::Semaphore,
    history_views: Mutex<HashMap<SessionId, crate::history::HistoryView>>,
    saved_outputs: Mutex<HashMap<DocumentId, crate::history::SavedOutput>>,
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
        let injected_config = config.is_some();
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
                injected_config,
                workspaces: Mutex::new(HashMap::new()),
                observers: Mutex::new(Vec::new()),
                history_directory: Mutex::new(None),
                selection: Mutex::new(NativeSelection::default()),
                interaction: Mutex::new(AppState::default()),
                readers: tokio::sync::Semaphore::new(16),
                history_views: Mutex::new(HashMap::new()),
                saved_outputs: Mutex::new(HashMap::new()),
            }),
        })
    }

    #[cfg(feature = "test-fixtures")]
    pub fn with_fixture_history(self) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("dzban-bridge-history-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).map_err(|error| error.to_string())?;
        *self.state.history_directory.lock().unwrap() = Some(HistoryDirectory(path));
        Ok(self)
    }

    pub fn dispatch(self: &Arc<Self>, command: Command, output: Arc<Stream>) {
        let runtime = self.runtime.as_ref().expect("live app has a runtime");
        let state = self.state.clone();
        let lifetime = self.clone();
        if let Command::ObserveControls { session } = command {
            let mut observers = self.state.observers.lock().unwrap();
            if *self.state.context.subscribe_phase().borrow() != ApplicationPhase::Running {
                output.publish(failure("closed", "Application is closing"));
                return;
            }
            observers.retain(|observer| !observer.task.is_finished());
            let stream = Arc::downgrade(&output);
            let task = runtime.spawn(async move { state.observe_controls(session, output).await });
            observers.push(Observer { stream, task });
            return;
        }
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
    async fn submit(&self, session: Uuid, document: Uuid, sql: String) -> Result<Value, Value> {
        let (engine, document) = self.document(session, document)?;
        let history = self.history(session).await?;
        let tab = self
            .context
            .tabs
            .session(engine.id())
            .ok_or_else(|| failure("unknown_tab", "Query tab is not open"))?;
        let query = tab
            .submit(sql, ResultLimits::default(), Some(history))
            .map_err(execution_error)?;
        self.saved_outputs.lock().unwrap().remove(&document.id());
        let result = json!({"query": query.query.0, "document": query.document.0, "session": query.session.0, "generation": query.generation});
        tokio::spawn(async move {
            let _ = query.wait().await;
        });
        Ok(result)
    }

    async fn editor_action(
        &self,
        session: Uuid,
        document: Uuid,
        draft: String,
        start: usize,
        end: usize,
        intent: EditorIntent,
    ) -> Result<Value, Value> {
        let (commands, active) = {
            let selection = self.selection.lock().unwrap();
            if selection.connection != ConnectionSelection::Ready(SessionId(session)) {
                return Err(failure(
                    "selection_superseded",
                    "The workspace is no longer selected",
                ));
            }
            let workspaces = self.workspaces.lock().unwrap();
            let workspace = workspaces
                .get(&SessionId(session))
                .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
            if workspace.documents.selected().map(|value| value.id()) != Some(DocumentId(document))
            {
                return Err(failure(
                    "selection_superseded",
                    "The query is no longer selected",
                ));
            }
            let active = workspace
                .documents
                .documents()
                .iter()
                .find(|document| document.is_active())
                .cloned();
            let mut interaction = self.interaction.lock().unwrap();
            interaction.transition(AppEvent::SessionSelected(Some(
                SessionInteraction::from_engine(&workspace.engine),
            )));
            let commands = match intent {
                EditorIntent::Submit => {
                    interaction.transition(AppEvent::KeyPressed {
                        key: InputRole::PrimaryModifier,
                        at_ms: 0,
                    });
                    let commands = interaction.transition(AppEvent::KeyPressed {
                        key: InputRole::Submit,
                        at_ms: 0,
                    });
                    interaction.transition(AppEvent::KeyReleased {
                        key: InputRole::Submit,
                        at_ms: 0,
                    });
                    interaction.transition(AppEvent::KeyReleased {
                        key: InputRole::PrimaryModifier,
                        at_ms: 0,
                    });
                    commands
                }
                EditorIntent::Primary => interaction.transition(AppEvent::PrimaryActionRequested),
                EditorIntent::Cancel => interaction.transition(AppEvent::CancelRequested),
                EditorIntent::Reset => interaction.transition(AppEvent::KeyboardReset),
            };
            (commands, active)
        };
        let mut submission = None;
        let mut canceled = false;
        for command in commands {
            match command {
                AppCommand::RunQuery => {
                    let sql = selected_sql(&draft, start, end)
                        .map_err(|message| failure("selection", message))?;
                    submission = Some(self.submit(session, document, sql).await?);
                }
                AppCommand::CancelQuery => {
                    if let Some(active) = &active {
                        let (engine, _) = self.document(session, active.id().0)?;
                        active
                            .request_cancel(engine)
                            .map_err(execution_error)?
                            .wait()
                            .await
                            .map_err(execution_error)?;
                        canceled = true;
                    }
                }
                AppCommand::CancelPreview => {
                    if let Some(workspace) =
                        self.workspaces.lock().unwrap().get(&SessionId(session))
                    {
                        workspace.preview.cancel();
                    }
                }
                _ => {}
            }
        }
        Ok(json!({"submission":submission, "cancellation_requested":canceled}))
    }
    async fn close_session(&self, session: Uuid) -> Result<(), Value> {
        self.selection
            .lock()
            .unwrap()
            .connection
            .begin_close(SessionId(session));
        let already_closed = self
            .workspaces
            .lock()
            .unwrap()
            .get(&SessionId(session))
            .is_some_and(|workspace| {
                *workspace.engine.subscribe_session().borrow()
                    == dzban_core::execution::SessionPhase::Closed
            });
        let outcome = if already_closed {
            Ok(())
        } else {
            self.context.close(SessionId(session)).await
        };
        let cleanup = outcome.as_ref().map(|_| ()).map_err(|error| match error {
            ApplicationError::Operation(error) => error.clone(),
            _ => ExecutionError::Message(error.to_string()),
        });
        self.selection
            .lock()
            .unwrap()
            .connection
            .finish_close(SessionId(session), &cleanup);
        outcome.map_err(|error| application_error("close", error))?;
        let _readers = self.readers.acquire_many(16).await.unwrap();
        let mut workspaces = self.workspaces.lock().unwrap();
        if let Some(workspace) = workspaces.get_mut(&SessionId(session)) {
            let documents: Vec<_> = workspace
                .documents
                .documents()
                .iter()
                .map(QueryDocument::id)
                .collect();
            workspace
                .documents
                .retire_closed_session(&workspace.engine)
                .map_err(execution_error)?;
            let mut saved = self.saved_outputs.lock().unwrap();
            for document in documents {
                saved.remove(&document);
            }
        }
        workspaces.remove(&SessionId(session));
        self.history_views
            .lock()
            .unwrap()
            .remove(&SessionId(session));
        Ok(())
    }
    fn controls(&self) -> Value {
        let snapshot = self.context.tabs.snapshot();
        let selected = snapshot
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == snapshot.selected)
            .and_then(|tab| tab.engine.as_ref())
            .map(|engine| SessionInteraction::from_engine(engine));
        let mut interaction = self.interaction.lock().unwrap();
        interaction.transition(AppEvent::SessionSelected(selected));
        let primary_action = match interaction.primary_action() {
            PrimaryAction::Run => "run",
            PrimaryAction::Stop => "stop",
            PrimaryAction::Stopping => "stopping",
            PrimaryAction::Unavailable => "unavailable",
        };
        json!({"session":interaction.scope.map(|scope|scope.session.0), "query":interaction.scope.and_then(|scope|scope.query.map(|id|id.0)), "primary_action":primary_action})
    }
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
            .lock()
            .unwrap()
            .as_ref()
            .map(|directory| directory.0.join("history.json"));
        if self.injected_config && path.is_none() {
            return Err(failure("closed", "Fixture history is closed"));
        }
        tokio::task::spawn_blocking(move || match path {
            Some(path) => QueryHistoryStore::load_from_path(path, &profile),
            None => QueryHistoryStore::load(&profile),
        })
        .await
        .map_err(|_| failure("worker_failed", "History worker failed"))?
        .map_err(|error| failure("history", error))
    }
    fn snapshot(&self, document: &QueryDocument) -> Value {
        let snapshot = document_snapshot(document);
        let saved = self.saved_outputs.lock().unwrap();
        match saved
            .get(&document.id())
            .filter(|saved| snapshot["generation"] == saved.generation)
        {
            Some(saved) => saved.project(snapshot),
            None => snapshot,
        }
    }

    async fn result_page(
        &self,
        session: Uuid,
        id: Uuid,
        generation: u64,
        start: usize,
        count: usize,
    ) -> Result<dzban_core::ResultSet, Value> {
        let (_, document) = self.document(session, id)?;
        let snapshot = document.snapshot();
        if snapshot.output_generation != generation {
            return Err(failure("stale_output", "Output generation has changed"));
        }
        let saved = self
            .saved_outputs
            .lock()
            .unwrap()
            .get(&document.id())
            .filter(|saved| saved.generation == generation)
            .cloned();
        let page = if let Some(saved) = saved {
            saved.page(start, count)
        } else {
            let result = snapshot
                .operation
                .and_then(|operation| operation.capture)
                .and_then(|capture| capture.result)
                .ok_or_else(|| failure("result_unavailable", "Result is not available"))?;
            result
                .read_page(start, count)
                .await
                .map_err(execution_error)?
        };
        if !document.accepts_output(generation) {
            return Err(failure("stale_output", "Output generation has changed"));
        }
        Ok(page)
    }

    async fn observe_controls(&self, session: Uuid, stream: Arc<Stream>) {
        let engine = self
            .workspaces
            .lock()
            .unwrap()
            .get(&SessionId(session))
            .map(|workspace| workspace.engine.clone());
        let Some(engine) = engine else {
            stream.publish(failure("unknown_session", "Session is not open"));
            return;
        };
        let mut operation = engine.subscribe_operation();
        let mut query = engine.subscribe();
        let mut lifecycle = engine.subscribe_session();
        loop {
            stream.publish(self.controls());
            tokio::select! {
                _ = stream.closed() => break,
                changed = operation.changed() => if changed.is_err() { break; },
                changed = query.changed() => if changed.is_err() { break; },
                changed = lifecycle.changed() => if changed.is_err() { break; },
            }
        }
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
        stream.publish(self.snapshot(&document));
        loop {
            tokio::select! {
                _ = stream.closed() => break,
                changed = receiver.changed() => {
                    if changed.is_err() { break; }
                    stream.publish(self.snapshot(&document));
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
        let _reader = if matches!(
            command,
            Command::ResultSchema { .. }
                | Command::ReadViewport { .. }
                | Command::ReadPage { .. }
                | Command::ReadCell { .. }
                | Command::HistoryList { .. }
                | Command::RestoreHistory { .. }
                | Command::SavedPlan { .. }
        ) {
            let permit = self.readers.acquire().await.unwrap();
            if *self.context.subscribe_phase().borrow() != ApplicationPhase::Running {
                return Err(failure("closed", "Application is closing"));
            }
            Some(permit)
        } else {
            None
        };
        match command {
            #[cfg(feature = "test-fixtures")]
            Command::TestPanic {} => panic!("Fixture bridge worker panic"),
            #[cfg(feature = "test-fixtures")]
            Command::TestMetrics {} => {
                let (record_bytes, peak_record_bytes) = crate::transport::byte_metrics();
                let (buffer_bytes, peak_buffer_bytes) = crate::handles::byte_metrics();
                Ok(
                    json!({"record_bytes":record_bytes,"peak_record_bytes":peak_record_bytes,"buffer_bytes":buffer_bytes,"peak_buffer_bytes":peak_buffer_bytes}),
                )
            }
            Command::Tabs {} => {
                let snapshot = self.context.tabs.snapshot();
                let tabs: Vec<_> = snapshot.tabs.iter().map(|tab| json!({
                    "tab": tab.id.0, "session": tab.engine.as_ref().map(|engine| engine.id().0),
                    "profile": tab.profile.0, "backend": tab.backend,
                    "document": tab.document.id().0,
                    "execution": document_snapshot(&tab.document)["execution"],
                    "phase": match &tab.phase { dzban_core::tabs::TabPhase::Connecting => "connecting", dzban_core::tabs::TabPhase::Ready => "ready", dzban_core::tabs::TabPhase::Failed(_) => "error" }
                })).collect();
                Ok(json!({"tabs": tabs, "selected": snapshot.selected.map(|tab|tab.0)}))
            }
            Command::Controls {} => Ok(self.controls()),
            Command::Actions { session } => {
                let engine = self
                    .workspaces
                    .lock()
                    .unwrap()
                    .get(&SessionId(session))
                    .map(|workspace| workspace.engine.clone())
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                let registry = engine
                    .action_registry()
                    .map_err(|error| failure("actions", error.to_string()))?;
                let capabilities = engine.capabilities();
                let actions: Vec<_> = registry
                    .available(&capabilities)
                    .map(crate::catalog::action)
                    .collect();
                Ok(json!({"session":session,"actions":actions}))
            }
            Command::Catalog { session, request } => {
                let engine = self
                    .workspaces
                    .lock()
                    .unwrap()
                    .get(&SessionId(session))
                    .map(|workspace| workspace.engine.clone())
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                let entries = engine
                    .catalog(request.core())
                    .await
                    .map_err(crate::catalog::error)?;
                let entries: Vec<_> = entries.iter().map(crate::catalog::entry).collect();
                Ok(json!({"session":session,"entries":entries}))
            }
            Command::EditorAction {
                session,
                document,
                draft,
                start,
                end,
                intent,
            } => {
                self.editor_action(session, document, draft, start, end, intent)
                    .await
            }
            Command::Connections {} => Ok(
                json!({"connections":self.context.subscribe_pending_connections().borrow().iter().map(|snapshot|json!({"request":snapshot.request.0,"profile":snapshot.profile.0,"connecting":matches!(snapshot.phase,dzban_core::application::ConnectionPhase::Connecting)})).collect::<Vec<_>>()}),
            ),
            Command::SelectSession { session, revision } => {
                let mut selection = self.selection.lock().unwrap();
                if !selection.accept_revision(revision) {
                    return Err(failure(
                        "selection_superseded",
                        "A newer navigation request is active",
                    ));
                }
                if let Some(session) = session {
                    if !self
                        .workspaces
                        .lock()
                        .unwrap()
                        .contains_key(&SessionId(session))
                    {
                        return Err(failure("unknown_session", "Session is not open"));
                    }
                    selection.connection = ConnectionSelection::Ready(SessionId(session));
                } else {
                    selection.connection.clear();
                }
                let tab = session
                    .and_then(|id| self.context.tabs.session(SessionId(id)))
                    .map(|tab| tab.id);
                self.context.tabs.select(tab);
                for workspace in self.workspaces.lock().unwrap().values_mut() {
                    workspace.input_document = None;
                    workspace.input_state = AppState::default();
                }
                drop(selection);
                Ok(self.controls())
            }
            Command::SelectDocument {
                session,
                document,
                revision,
            } => {
                let mut workspaces = self.workspaces.lock().unwrap();
                let workspace = workspaces
                    .get_mut(&SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                if revision.is_some_and(|revision| revision < workspace.selection_revision) {
                    return Err(failure(
                        "selection_superseded",
                        "A newer document selection is active",
                    ));
                }
                if !workspace.documents.select(DocumentId(document)) {
                    return Err(failure("unknown_document", "Document is not open"));
                }
                if let Some(revision) = revision {
                    workspace.selection_revision = revision;
                }
                Ok(self.snapshot(&workspace.documents.selected().unwrap()))
            }
            Command::CloseDocument { session, document } => {
                let outcome = {
                    let _readers = self.readers.acquire_many(16).await.unwrap();
                    let mut workspaces = self.workspaces.lock().unwrap();
                    let workspace = workspaces
                        .get_mut(&SessionId(session))
                        .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                    workspace.documents.close_in_workspace(DocumentId(document))
                };
                match outcome {
                    CloseDocument::Busy => Err(execution_error(ExecutionError::Busy)),
                    CloseDocument::Missing => {
                        Err(failure("unknown_document", "Document is not open"))
                    }
                    CloseDocument::CloseWorkspace => {
                        self.close_session(session).await?;
                        Ok(json!({"workspace_closed":true, "document":document, "selected":null}))
                    }
                    CloseDocument::Closed { .. } => {
                        self.saved_outputs
                            .lock()
                            .unwrap()
                            .remove(&DocumentId(document));
                        let workspaces = self.workspaces.lock().unwrap();
                        let selected = workspaces
                            .get(&SessionId(session))
                            .and_then(|workspace| workspace.documents.selected());
                        Ok(
                            json!({"workspace_closed":false, "document":document, "selected":selected.as_ref().map(|document| self.snapshot(document))}),
                        )
                    }
                }
            }
            Command::Input {
                session,
                document,
                revision,
                input,
            } => self.input(session, document, revision, input),
            Command::AwaitIdle { session } => self.await_idle(session).await,
            Command::ProbeSession {
                session,
                include_version,
            } => self.probe_session(session, include_version).await,
            Command::ProbeProfile { profile } => self.probe_profile(profile).await,
            Command::Profiles {} => {
                let current = if self.injected_config {
                    self.config.lock().unwrap().clone()
                } else {
                    None
                };
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
            Command::Connect { profile, revision } => {
                let config =
                    self.config.lock().unwrap().clone().ok_or_else(|| {
                        failure("configuration", "Load profiles before connecting")
                    })?;
                let profile = config
                    .resolve_profile(&ConnectionProfileId(profile), None)
                    .map_err(|error| failure("profile", error))?;
                let profile_name = profile.name.clone();
                let (connection, snapshots) = {
                    let mut selection = self.selection.lock().unwrap();
                    if !selection.accept_revision(revision) {
                        return Err(failure(
                            "selection_superseded",
                            "A newer navigation request is active",
                        ));
                    }
                    if !selection.connection.accepts_connection_request() {
                        return Err(execution_error(ExecutionError::Busy));
                    }
                    let connection = self.context.start_connect_tab(profile);
                    selection.connection.begin(connection.id);
                    let snapshots = connection.subscribe();
                    (connection, snapshots)
                };
                let request = connection.id.0;
                let outcome = connection.wait().await;
                let selected = self
                    .selection
                    .lock()
                    .unwrap()
                    .connection
                    .apply(&snapshots.borrow());
                let engine = outcome.map_err(|error| application_error("connection", error))?;
                let session = engine.id();
                let tab = self
                    .context
                    .tabs
                    .session(session)
                    .ok_or_else(|| failure("closed", "Query tab is closing"))?;
                let tab_id = tab.id;
                {
                    let mut workspaces = self.workspaces.lock().unwrap();
                    if *self.context.subscribe_phase().borrow() != ApplicationPhase::Running {
                        return Err(failure("closed", "Application is closing"));
                    }
                    workspaces.insert(
                        session,
                        Workspace {
                            engine,
                            documents: QueryDocuments::from_document(tab.document),
                            preview: PlanPreviewService::default(),
                            peeks: Default::default(),
                            preview_revision: 0,
                            input_document: None,
                            input_state: AppState::default(),
                            profile: profile_name,
                            selection_revision: 0,
                        },
                    );
                }
                Ok(
                    json!({"request": request, "session": session.0, "tab": tab_id.0, "selected":selected}),
                )
            }
            Command::OpenDocument { session, revision } => {
                let mut workspaces = self.workspaces.lock().unwrap();
                let workspace = workspaces
                    .get_mut(&SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                let document = workspace
                    .documents
                    .selected()
                    .ok_or_else(|| failure("unknown_document", "Tab has no query document"))?;
                if let Some(revision) = revision {
                    workspace.selection_revision = workspace.selection_revision.max(revision);
                }
                Ok(self.snapshot(&document))
            }
            Command::ContextChanged { session, document } => {
                let (_, document) = self.document(session, document)?;
                document.query_context_changed();
                Ok(self.snapshot(&document))
            }
            Command::SetDraft {
                session,
                document,
                draft,
            } => {
                let (_, document) = self.document(session, document)?;
                document.set_draft(draft);
                Ok(self.snapshot(&document))
            }
            Command::ObserveDocument { .. } | Command::ObserveControls { .. } => Err(failure(
                "invalid_command",
                "Observation must use a streaming response",
            )),
            Command::Submit {
                session,
                document,
                sql,
            } => self.submit(session, document, sql).await,
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
            Command::ResultSchema {
                session,
                document,
                generation,
            } => {
                let page = self
                    .result_page(session, document, generation, 0, 0)
                    .await?;
                Ok(json!({"document":document,"generation":generation,"columns":page.columns}))
            }
            Command::ReadViewport {
                session,
                document,
                generation,
                start,
                count,
                column_start,
                column_count,
            } => {
                if count > MAX_PAGE_ROWS
                    || column_count == 0
                    || column_count > crate::viewport::MAX_VIEW_COLUMNS
                {
                    return Err(failure(
                        "invalid_page",
                        "Viewport exceeds the row or column limit",
                    ));
                }
                let page = self
                    .result_page(session, document, generation, start, count)
                    .await?;
                if column_start >= page.columns.len()
                    || column_count > page.columns.len() - column_start
                {
                    return Err(failure(
                        "invalid_page",
                        "Viewport columns are outside the schema",
                    ));
                }
                Ok(
                    json!({"document":document,"generation":generation,"start":start,"column_start":column_start,"rows":crate::viewport::cells(page,column_start,column_count)}),
                )
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
                let page = self
                    .result_page(session, document, generation, start, count)
                    .await?;
                let payload = json!({"document": document, "generation": generation, "start": start, "page": page});
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
                self.close_session(session).await?;
                Ok(json!({"closed": session}))
            }
            Command::History { session } => {
                let history = self.history(session).await?;
                Ok(json!({"entries": history.entries()}))
            }
            Command::HistoryList { session } => {
                let store = Arc::new(self.history(session).await?);
                let view = crate::history::HistoryView {
                    revision: Uuid::new_v4(),
                    store,
                };
                let response = crate::history::summaries(&view);
                self.history_views
                    .lock()
                    .unwrap()
                    .insert(SessionId(session), view);
                Ok(response)
            }
            Command::RestoreHistory {
                session,
                document,
                history_revision,
                index,
            } => {
                let (_, document) = self.document(session, document)?;
                let store = {
                    let views = self.history_views.lock().unwrap();
                    let view = views
                        .get(&SessionId(session))
                        .filter(|view| view.revision == history_revision)
                        .ok_or_else(|| failure("stale_history", "History has been refreshed"))?;
                    if index >= view.store.entries().len() {
                        return Err(failure("history_entry", "History entry is missing"));
                    }
                    view.store.clone()
                };
                let mut saved = self.saved_outputs.lock().unwrap();
                document.set_draft(store.entries()[index].sql.clone());
                let generation = document.replace_output();
                let output = crate::history::SavedOutput {
                    generation,
                    store,
                    index,
                };
                let snapshot = output.project(document_snapshot(&document));
                saved.insert(document.id(), output);
                Ok(snapshot)
            }
            Command::SavedPlan {
                session,
                document,
                generation,
            } => {
                let (_, document) = self.document(session, document)?;
                if !document.accepts_output(generation) {
                    return Err(failure("stale_output", "Output generation has changed"));
                }
                let saved = self.saved_outputs.lock().unwrap();
                let output = saved
                    .get(&document.id())
                    .filter(|saved| saved.generation == generation)
                    .ok_or_else(|| failure("history_entry", "Saved output is not selected"))?;
                Ok(json!({"plan":output.plan()}))
            }
            Command::Peek {
                session,
                surface,
                event,
                elapsed_ms,
            } => {
                let mut workspaces = self.workspaces.lock().unwrap();
                let workspace = workspaces
                    .get_mut(&SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                let state = &mut workspace.peeks[match surface {
                    crate::catalog::Surface::Plan => 0,
                    crate::catalog::Surface::Row => 1,
                }];
                use crate::catalog::PeekEvent;
                match event {
                    PeekEvent::Show => state.show_transient(),
                    PeekEvent::Open => {
                        state.show_transient();
                        state.pin();
                    }
                    PeekEvent::Pin => {
                        state.pin();
                    }
                    PeekEvent::Close => {
                        state.close();
                    }
                    PeekEvent::Release => match dzban_core::preview::preview_release(
                        std::time::Duration::from_millis(elapsed_ms.unwrap_or(0)),
                    ) {
                        dzban_core::preview::PreviewRelease::Pin => {
                            state.pin();
                        }
                        dzban_core::preview::PreviewRelease::Hide => state.hide_transient(),
                    },
                    PeekEvent::Reset => state.hide_transient(),
                }
                Ok(json!({"visible":state.is_visible(), "pinned":state.is_pinned()}))
            }
            Command::CancelPreview {
                session,
                request_revision,
            } => {
                let mut workspaces = self.workspaces.lock().unwrap();
                let workspace = workspaces
                    .get_mut(&SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                if let Some(revision) = request_revision {
                    if revision < workspace.preview_revision {
                        return Err(failure(
                            "preview_invalidated",
                            "A newer preview intent is active",
                        ));
                    }
                    workspace.preview_revision = revision;
                }
                workspace.preview.cancel();
                Ok(json!({"preview_canceled":true}))
            }
            Command::Preview {
                session,
                document,
                sql,
                revision,
                start,
                end,
                request_revision,
            } => {
                let sql = match (start, end) {
                    (Some(start), Some(end)) => selected_sql(&sql, start, end)
                        .map_err(|message| failure("selection", message))?,
                    (None, None) => sql,
                    _ => return Err(failure("selection", "Both selection bounds are required")),
                };
                let (engine, document) = self.document(session, document)?;
                if document.snapshot().draft_revision != revision {
                    return Err(failure("preview_invalidated", "Draft revision has changed"));
                }
                let (service, target, mut updates) = {
                    let selection = self.selection.lock().unwrap();
                    if request_revision.is_some()
                        && selection.connection != ConnectionSelection::Ready(SessionId(session))
                    {
                        return Err(failure(
                            "preview_invalidated",
                            "Workspace is no longer selected",
                        ));
                    }
                    let mut workspaces = self.workspaces.lock().unwrap();
                    let workspace = workspaces
                        .get_mut(&SessionId(session))
                        .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                    if let Some(revision) = request_revision {
                        if revision < workspace.preview_revision
                            || workspace.documents.selected().map(|document| document.id())
                                != Some(document.id())
                        {
                            return Err(failure(
                                "preview_invalidated",
                                "A newer preview context is active",
                            ));
                        }
                        workspace.preview_revision = revision;
                    }
                    let service = workspace.preview.clone();
                    let updates = service.subscribe();
                    let target = service
                        .schedule(engine, document.clone(), sql, PREVIEW_DELAY)
                        .map_err(execution_error)?;
                    (service, target, updates)
                };
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
                            let plan = json!({"backend":plan.backend, "document":plan.document, "estimate":plan.estimate, "text":plan.text()});
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
                let page = self
                    .result_page(session, document, generation, row, 1)
                    .await?;
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
                    json!({"document":document, "generation":generation, "row":row, "column":column, "offset":offset, "total":total, "text":text}),
                )
            }
            Command::Shutdown {} => {
                self.context
                    .start_shutdown()
                    .map_err(|error| application_error("shutdown", error))?
                    .wait()
                    .await
                    .map_err(|error| application_error("shutdown", error))?;
                let _readers = self.readers.acquire_many(16).await.unwrap();
                {
                    let mut workspaces = self.workspaces.lock().unwrap();
                    for workspace in workspaces.values_mut() {
                        workspace
                            .documents
                            .retire_closed_session(&workspace.engine)
                            .map_err(execution_error)?;
                    }
                    workspaces.clear();
                    self.saved_outputs.lock().unwrap().clear();
                    self.history_views.lock().unwrap().clear();
                }
                let observers = std::mem::take(&mut *self.observers.lock().unwrap());
                for observer in observers {
                    if let Some(stream) = observer.stream.upgrade() {
                        stream.close();
                    }
                    observer.task.abort();
                    let _ = observer.task.await;
                }
                self.history_directory.lock().unwrap().take();
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
    let termination = capture.and_then(|capture| match &capture.phase {
        ResultPhase::Finished(ResultTermination::Complete) => Some("complete"),
        ResultPhase::Finished(ResultTermination::Limited(_)) => Some("limited"),
        ResultPhase::Finished(ResultTermination::Canceled) => Some("canceled"),
        ResultPhase::Finished(ResultTermination::Failed(_)) => Some("failed"),
        _ => None,
    });
    let capture_error = capture.and_then(|capture| match &capture.phase {
        ResultPhase::FailedAwaitingCleanup(error)
        | ResultPhase::Finished(ResultTermination::Failed(error)) => Some(error),
        _ => None,
    });
    let command = snapshot
        .completion
        .as_ref()
        .and_then(|completion| completion.as_ref().ok())
        .and_then(|completion| completion.result.as_ref().ok())
        .and_then(|result| result.command.as_ref());
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
        "result_available": capture.is_some_and(|capture| capture.result.is_some()),
        "rows": capture.map_or(0, |capture| capture.rows),
        "received_rows": capture.map_or(0, |capture| capture.received_rows),
        "capture_bytes": capture.map_or(0, |capture| capture.bytes),
        "termination": termination,
        "capture_error": capture_error,
        "command_result": command,
        "started_at_seconds": snapshot.operation.as_ref().and_then(|operation| operation.started_wall_time).and_then(|start| start.duration_since(std::time::UNIX_EPOCH).ok()).map(|duration| duration.as_secs_f64()),
        "elapsed_seconds": snapshot.operation.as_ref().map(|operation| operation.elapsed().as_secs_f64()),
        "execution_plan": snapshot.plan.as_ref().map(|plan| json!({"backend": plan.backend, "estimate": plan.estimate})),
        "execution_seconds": snapshot.completion.as_ref().and_then(|completion|completion.as_ref().ok()).and_then(|completion|completion.result.as_ref().ok()).map(|result|result.duration.as_secs_f64()),
        "cutoff": capture.and_then(|capture| capture.cutoff),
        "error": error,
    })
}

#[cfg(all(test, feature = "test-fixtures"))]
mod tests {
    use super::*;
    use dzban_core::history::QueryHistoryEntry;

    #[test]
    fn profile_probe_closes_its_temporary_session() {
        let app = crate::fixture::app().unwrap();
        let runtime = app.runtime.as_ref().unwrap();
        let result = runtime
            .block_on(app.state.probe_profile("fixture".into()))
            .unwrap();
        assert_eq!(result["state"], "connected");
        assert!(runtime.block_on(app.state.context.session_ids()).is_empty());
        assert!(app.state.workspaces.lock().unwrap().is_empty());
        runtime
            .block_on(async { app.state.context.start_shutdown().unwrap().wait().await })
            .unwrap();
    }

    #[test]
    fn native_history_refresh_observes_external_publication_without_retargeting_saved_entries() {
        let app = crate::fixture::app().unwrap();
        let runtime = app.runtime.as_ref().unwrap();
        let request = |value: Value| {
            runtime
                .block_on(app.state.execute(serde_json::from_value(value).unwrap()))
                .unwrap()
        };
        let session = request(json!({"command":"connect","profile":"fixture"}))["session"].clone();
        let document =
            request(json!({"command":"open_document","session":session}))["document"].clone();
        let path = app
            .state
            .history_directory
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .0
            .join("history.json");
        let legacy = json!({"version":1,"database":"fixture","entries":[QueryHistoryEntry::failure("legacy CLI query".into(),"legacy failure".into(),None,None)]});
        let original_bytes = serde_json::to_vec(&legacy).unwrap();
        std::fs::write(&path, &original_bytes).unwrap();
        let before = request(json!({"command":"history_list","session":session}));
        assert_eq!(before["entries"][0]["sql_preview"], "legacy CLI query");
        let mut writer = QueryHistoryStore::load_from_path(path.clone(), "fixture").unwrap();
        writer
            .add(QueryHistoryEntry::failure(
                "external CLI query".into(),
                "external failure".into(),
                None,
                None,
            ))
            .unwrap();
        let restored = request(
            json!({"command":"restore_history","session":session,"document":document,"history_revision":before["revision"],"index":0}),
        );
        assert_eq!(restored["draft"], "legacy CLI query");
        assert_eq!(restored["termination"], "failed");
        assert_eq!(restored["rows"], 0);
        let refreshed = request(json!({"command":"history_list","session":session}));
        assert_eq!(refreshed["entries"].as_array().unwrap().len(), 2);
        assert_eq!(refreshed["entries"][0]["sql_preview"], "external CLI query");
        assert_ne!(before["revision"], refreshed["revision"]);
        let visible = app
            .state
            .document(
                serde_json::from_value(session).unwrap(),
                serde_json::from_value(document).unwrap(),
            )
            .unwrap()
            .1;
        assert_eq!(app.state.snapshot(&visible)["draft"], "legacy CLI query");
        assert_eq!(std::fs::read(path).unwrap(), original_bytes);
        request(json!({"command":"shutdown"}));
    }
}
