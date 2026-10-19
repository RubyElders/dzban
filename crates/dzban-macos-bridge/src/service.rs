use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dzban_core::application::{ApplicationContext, ApplicationError, ApplicationPhase};
use dzban_core::commands::{CommandError, EditorCommand, HistoryLocation};
use dzban_core::documents::{DocumentId, QueryDocument};
use dzban_core::error::ExecutionError;
use dzban_core::execution::{QueryEngine, SessionId};
use dzban_core::history::QueryHistoryStore;
use dzban_core::outputs::{OutputError, OutputExport, ResultScope, RowRange};
use dzban_core::presentation::PlanPresentation;
use dzban_core::preview::PreviewError;
use dzban_core::result_store::ResultLimits;
use dzban_core::workspace::WorkspaceLocation;
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
    OpenTab {
        profile: Option<String>,
        draft: Option<String>,
        revision: Option<u64>,
    },
    RestoreWorkspace {
        location: WorkspaceLocation,
    },
    SaveWorkspace {
        location: WorkspaceLocation,
        drafts: Vec<WorkspaceDraft>,
    },
    CycleTabs {
        direction: i64,
        revision: Option<u64>,
    },
    SelectTab {
        tab: Uuid,
        revision: Option<u64>,
    },
    CloseTab {
        tab: Uuid,
    },
    CancelTabConnection {
        tab: Uuid,
    },
    SetTabDraft {
        tab: Uuid,
        draft: String,
    },
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
    Interact {
        session: Uuid,
        document: Uuid,
        revision: u64,
        draft: String,
        start: usize,
        end: usize,
        input: native_controls::Input,
    },
    ObserveInteraction {
        session: Uuid,
    },
    AwaitPlan {
        session: Uuid,
        revision: u64,
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
    ObserveHealth {},
    ObserveAttention {},
    CheckHealth {
        session: Uuid,
    },
    CheckProfileHealth {
        profile: String,
    },
    CheckProfilesHealth {},
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
    CloseDocument {
        session: Uuid,
        document: Uuid,
    },
    Connect {
        tab: Option<Uuid>,
        profile: String,
        revision: Option<u64>,
    },
    OpenDocument {
        session: Uuid,
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
    StartExport {
        export: Uuid,
        session: Uuid,
        document: Uuid,
        generation: u64,
        row: Option<usize>,
        column: Option<usize>,
        ranges: Option<Vec<RowRange>>,
    },
    ReadExport {
        export: Uuid,
    },
    CloseExport {
        export: Uuid,
    },
    CloseSession {
        session: Uuid,
    },
    Shutdown {},
}

#[derive(Deserialize)]
pub struct WorkspaceDraft {
    tab: Uuid,
    draft: String,
}

pub struct App {
    runtime: Option<Runtime>,
    state: Arc<State>,
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
    observers: Mutex<Vec<Observer>>,
    history_directory: Mutex<Option<HistoryDirectory>>,
    readers: tokio::sync::Semaphore,
    exports: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<OutputExport>>>>,
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

fn output_error(error: OutputError) -> Value {
    let kind = match &error {
        OutputError::Application(ApplicationError::UnknownSession(_)) => {
            return failure("unknown_session", "Session is not open");
        }
        OutputError::Application(error) => return application_error("output", error.clone()),
        OutputError::UnknownDocument => "unknown_document",
        OutputError::StaleOutput => "stale_output",
        OutputError::Unavailable => "result_unavailable",
        OutputError::StaleHistory => "stale_history",
        OutputError::HistoryEntryMissing | OutputError::SavedOutputMissing => "history_entry",
        OutputError::InvalidCell => "invalid_cell",
        OutputError::InvalidPage(_) => "invalid_page",
        OutputError::InvalidChunk(_) => "invalid_chunk",
        OutputError::CellTooLarge => "cell_too_large",
        OutputError::ExportTooLarge => "export_too_large",
        OutputError::InvalidSelection => "invalid_selection",
        OutputError::Execution(error) => return execution_error(error.clone()),
        OutputError::History(error) => return command_error(error.clone()),
    };
    failure(kind, error)
}

fn command_error(error: CommandError) -> Value {
    match error {
        CommandError::Application(error) => application_error("command", error),
        CommandError::Execution(error) => execution_error(error),
        CommandError::Preview(error) => preview_error(error),
        CommandError::Selection(message) => failure("selection", message),
        CommandError::History(message) => failure("history", message),
        CommandError::Worker(message) => failure("worker_failed", message),
    }
}

fn preview_error(error: PreviewError) -> Value {
    match error {
        PreviewError::Invalidated(message) => failure("preview_invalidated", message),
        PreviewError::Execution(error) => execution_error(error),
        PreviewError::Application(error) => application_error("preview", error),
    }
}

fn application_error(kind: &str, error: ApplicationError) -> Value {
    let kind = if error == ApplicationError::SelectionSuperseded {
        "selection_superseded"
    } else {
        kind
    };
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
                observers: Mutex::new(Vec::new()),
                history_directory: Mutex::new(None),
                readers: tokio::sync::Semaphore::new(16),
                exports: Mutex::default(),
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
        if let Command::RestoreWorkspace { location } = command {
            let mut observers = self.state.observers.lock().unwrap();
            if *self.state.context.subscribe_phase().borrow() != ApplicationPhase::Running {
                output.publish(failure("closed", "Application is closing"));
                return;
            }
            observers.retain(|observer| !observer.task.is_finished());
            let stream = Arc::downgrade(&output);
            let task =
                runtime.spawn(async move { state.restore_workspace(location, output).await });
            observers.push(Observer { stream, task });
            return;
        }
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
        if let Command::ObserveInteraction { session } = command {
            let mut observers = self.state.observers.lock().unwrap();
            if *self.state.context.subscribe_phase().borrow() != ApplicationPhase::Running {
                output.publish(failure("closed", "Application is closing"));
                return;
            }
            observers.retain(|observer| !observer.task.is_finished());
            let stream = Arc::downgrade(&output);
            let task =
                runtime.spawn(async move { state.observe_interaction(session, output).await });
            observers.push(Observer { stream, task });
            return;
        }
        if let Command::ObserveAttention {} = command {
            let mut observers = self.state.observers.lock().unwrap();
            if *self.state.context.subscribe_phase().borrow() != ApplicationPhase::Running {
                output.publish(failure("closed", "Application is closing"));
                return;
            }
            observers.retain(|observer| !observer.task.is_finished());
            let stream = Arc::downgrade(&output);
            let task = runtime.spawn(async move { state.observe_attention(output).await });
            observers.push(Observer { stream, task });
            return;
        }
        if let Command::ObserveHealth {} = command {
            let mut observers = self.state.observers.lock().unwrap();
            if *self.state.context.subscribe_phase().borrow() != ApplicationPhase::Running {
                output.publish(failure("closed", "Application is closing"));
                return;
            }
            observers.retain(|observer| !observer.task.is_finished());
            let stream = Arc::downgrade(&output);
            let task = runtime.spawn(async move { state.observe_health(output).await });
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

    pub fn attention_input(
        &self,
        input: dzban_core::attention::AttentionInput,
    ) -> dzban_core::attention::AttentionSnapshot {
        self.state.context.attention().input(input)
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
    fn tab_inventory(&self) -> Value {
        json!(
            self.context
                .outputs
                .workspace(&self.context.tabs.snapshot())
        )
    }

    async fn observe_attention(&self, output: Arc<Stream>) {
        let mut snapshots = self.context.attention().subscribe();
        loop {
            output.publish(json!(snapshots.borrow_and_update().clone()));
            tokio::select! {
                _ = output.closed() => return,
                changed = snapshots.changed() => if changed.is_err() { return; },
            }
        }
    }

    fn track_restored_health(&self) {
        for tab in self.context.tabs.snapshot().tabs {
            if tab.phase == dzban_core::tabs::TabPhase::Ready {
                let _ = self.context.health.track_tab(&self.context, tab.id);
            }
        }
    }

    async fn restore_workspace(&self, location: WorkspaceLocation, output: Arc<Stream>) {
        let config = self.config.lock().unwrap().clone();
        let restore = self.context.start_restore_workspace(location, config);
        let mut progress = restore.subscribe();
        let mut tabs = self.context.tabs.subscribe();
        loop {
            let current = progress.borrow_and_update().clone();
            tabs.borrow_and_update();
            self.track_restored_health();
            output.publish(json!({"workspace":self.tab_inventory(),"progress":current}));
            if current.finished {
                return;
            }
            tokio::select! {
                _ = output.closed() => return,
                changed = progress.changed() => if changed.is_err() { return; },
                changed = tabs.changed() => if changed.is_err() { return; },
            }
        }
    }

    async fn submit(&self, session: Uuid, document: Uuid, sql: String) -> Result<Value, Value> {
        let (engine, _) = self.document(session, document)?;
        let history = self.history(session).await?;
        let tab = self
            .context
            .tabs
            .session(engine.id())
            .ok_or_else(|| failure("unknown_tab", "Query tab is not open"))?;
        let query = tab
            .submit(sql, ResultLimits::default(), Some(history))
            .map_err(execution_error)?;
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
        let tab = self
            .context
            .tabs
            .session(SessionId(session))
            .ok_or_else(|| {
                failure(
                    "selection_superseded",
                    "The workspace is no longer selected",
                )
            })?;
        let history = self.history_location()?;
        let result = self
            .context
            .start_editor_command(
                EditorCommand {
                    tab: tab.id,
                    document: DocumentId(document),
                    draft,
                    selection: start..end,
                    intent,
                },
                history,
            )
            .wait()
            .await
            .map_err(command_error)?;
        Ok(json!(result))
    }
    async fn close_session(&self, session: Uuid) -> Result<(), Value> {
        self.context
            .close(SessionId(session))
            .await
            .map_err(|error| application_error("close", error))?;
        let _readers = self.readers.acquire_many(16).await.unwrap();
        Ok(())
    }
    fn controls(&self) -> Value {
        json!(self.context.tabs.controls())
    }

    fn history_location(&self) -> Result<HistoryLocation, Value> {
        let path = self
            .history_directory
            .lock()
            .unwrap()
            .as_ref()
            .map(|directory| directory.0.join("history.json"));
        match path {
            Some(path) => Ok(HistoryLocation::File(path)),
            None if self.injected_config => Err(failure("closed", "Fixture history is closed")),
            None => Ok(HistoryLocation::Default),
        }
    }

    async fn history(&self, session: Uuid) -> Result<QueryHistoryStore, Value> {
        let tab = self
            .context
            .tabs
            .session(SessionId(session))
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        let profile = tab
            .profile
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        self.history_location()?
            .load(profile.0)
            .await
            .map_err(command_error)
    }
    fn snapshot(&self, document: &QueryDocument) -> Value {
        json!(self.context.outputs.document(document))
    }

    fn output_reader(
        &self,
        session: Uuid,
        document: Uuid,
        generation: u64,
    ) -> Result<dzban_core::outputs::ResultReader, Value> {
        self.context
            .result_reader(ResultScope {
                session: SessionId(session),
                document: DocumentId(document),
                generation,
            })
            .map_err(output_error)
    }

    async fn result_page(
        &self,
        session: Uuid,
        document: Uuid,
        generation: u64,
        start: usize,
        count: usize,
    ) -> Result<dzban_core::ResultSet, Value> {
        self.output_reader(session, document, generation)?
            .page(start, count)
            .await
            .map_err(output_error)
    }

    async fn observe_controls(&self, session: Uuid, stream: Arc<Stream>) {
        let engine = self
            .context
            .tabs
            .session(SessionId(session))
            .and_then(|tab| tab.engine);
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
        let tab = self
            .context
            .tabs
            .session(SessionId(session))
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        if tab.document.id() != DocumentId(document) {
            return Err(failure("unknown_document", "Document is not open"));
        }
        let engine = tab
            .engine
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        Ok((engine, tab.document))
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
                | Command::StartExport { .. }
                | Command::ReadExport { .. }
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
                let observers = self
                    .observers
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|observer| !observer.task.is_finished())
                    .count();
                Ok(
                    json!({"record_bytes":record_bytes,"peak_record_bytes":peak_record_bytes,"buffer_bytes":buffer_bytes,"peak_buffer_bytes":peak_buffer_bytes,"observers":observers,"export_readers":self.exports.lock().unwrap().len()}),
                )
            }
            Command::Tabs {} => Ok(self.tab_inventory()),
            Command::RestoreWorkspace { .. } => Err(failure(
                "invalid_command",
                "Restoration must use a streaming response",
            )),
            Command::SaveWorkspace { location, drafts } => {
                let drafts = drafts
                    .into_iter()
                    .map(|draft| (dzban_core::tabs::TabId(draft.tab), draft.draft))
                    .collect();
                self.context
                    .save_workspace(location, drafts)
                    .await
                    .map_err(|error| failure("workspace", error))?;
                Ok(json!({"saved":true}))
            }

            Command::OpenTab {
                profile,
                draft,
                revision,
            } => {
                let tab = self
                    .context
                    .open_tab_at(
                        profile.map(ConnectionProfileId),
                        draft.unwrap_or_default(),
                        revision,
                    )
                    .map_err(|error| application_error("tab", error))?;
                Ok(json!({"tab":tab.0,"workspace":self.tab_inventory()}))
            }
            Command::CycleTabs {
                direction,
                revision,
            } => {
                if !self.context.tabs.cycle(direction, revision) {
                    return Err(failure(
                        "selection_superseded",
                        "A newer navigation request is active",
                    ));
                }
                Ok(self.tab_inventory())
            }
            Command::SelectTab { tab, revision } => {
                if !self
                    .context
                    .tabs
                    .select_at(Some(dzban_core::tabs::TabId(tab)), revision)
                {
                    return Err(failure(
                        "selection_superseded",
                        "Tab is missing or a newer navigation request is active",
                    ));
                }
                Ok(self.tab_inventory())
            }
            Command::CloseTab { tab } | Command::CancelTabConnection { tab } => {
                let retain = matches!(command, Command::CancelTabConnection { .. });
                let tab = dzban_core::tabs::TabId(tab);
                if self.context.tabs.find(tab).is_none() {
                    return Err(failure("unknown_tab", "Tab is not open"));
                }
                let closing = if retain {
                    self.context.start_cancel_connection(tab)
                } else {
                    self.context.start_close_tab(tab)
                };
                closing
                    .map_err(|error| application_error("close", error))?
                    .wait()
                    .await
                    .map_err(|error| application_error("close", error))?;
                let _readers = self.readers.acquire_many(16).await.unwrap();
                Ok(self.tab_inventory())
            }
            Command::SetTabDraft { tab, draft } => {
                let tab = self
                    .context
                    .tabs
                    .find(dzban_core::tabs::TabId(tab))
                    .ok_or_else(|| failure("unknown_tab", "Tab is not open"))?;
                tab.document.set_draft(draft);
                Ok(self.tab_inventory())
            }
            Command::Controls {} => Ok(self.controls()),
            Command::Actions { session } => {
                let engine = self
                    .context
                    .tabs
                    .session(SessionId(session))
                    .and_then(|tab| tab.engine)
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
                    .context
                    .tabs
                    .session(SessionId(session))
                    .and_then(|tab| tab.engine)
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
                let tab = match session {
                    Some(session) => Some(
                        self.context
                            .tabs
                            .session(SessionId(session))
                            .ok_or_else(|| failure("unknown_session", "Session is not open"))?
                            .id,
                    ),
                    None => None,
                };
                if !self.context.tabs.select_at(tab, revision) {
                    return Err(failure(
                        "selection_superseded",
                        "A newer navigation request is active",
                    ));
                }
                Ok(self.controls())
            }
            Command::CloseDocument { session, document } => {
                let (_, current) = self.document(session, document)?;
                if current.is_active() {
                    return Err(execution_error(ExecutionError::Busy));
                }
                self.close_session(session).await?;
                Ok(json!({"workspace_closed":true, "document":document, "selected":null}))
            }
            Command::AwaitPlan { session, revision } => {
                let tab = self
                    .context
                    .tabs
                    .session(SessionId(session))
                    .ok_or_else(|| {
                        failure(
                            "selection_superseded",
                            "The workspace is no longer selected",
                        )
                    })?;
                self.context
                    .tabs
                    .await_plan(tab.id, revision)
                    .await
                    .map(|snapshot| json!(snapshot))
                    .map_err(|error| application_error("selection_superseded", error))
            }
            Command::Interact {
                session,
                document,
                revision,
                draft,
                start,
                end,
                input,
            } => {
                let tab = self
                    .context
                    .tabs
                    .session(SessionId(session))
                    .ok_or_else(|| {
                        failure(
                            "selection_superseded",
                            "The workspace is no longer selected",
                        )
                    })?;
                let outcome = self
                    .context
                    .start_input_command(
                        dzban_core::commands::InputCommand {
                            tab: tab.id,
                            document: DocumentId(document),
                            revision,
                            draft,
                            selection: start..end,
                            event: input,
                        },
                        self.history_location()?,
                    )
                    .wait()
                    .await
                    .map_err(command_error)?;
                Ok(json!(outcome))
            }
            Command::Input {
                session,
                document,
                revision,
                input,
            } => self.input(session, document, revision, input),
            Command::AwaitIdle { session } => self.await_idle(session).await,
            Command::CheckHealth { session } => {
                if let Some(tab) = self.context.tabs.session(SessionId(session)) {
                    let _ = self
                        .context
                        .health
                        .check_tab(&self.context, tab.id)
                        .wait()
                        .await;
                }
                Ok(json!(self.context.health.snapshot()))
            }
            Command::CheckProfileHealth { profile } => {
                let config = self.config.lock().unwrap().clone();
                self.context
                    .health
                    .check_named_profile(
                        self.context.clone(),
                        config.as_ref(),
                        ConnectionProfileId(profile),
                    )
                    .await;
                Ok(json!(self.context.health.snapshot()))
            }
            Command::CheckProfilesHealth {} => {
                let config = self.config.lock().unwrap().clone();
                if let Some(config) = config {
                    self.context
                        .health
                        .check_all(self.context.clone(), config)
                        .await;
                }
                Ok(json!(self.context.health.snapshot()))
            }
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
            Command::Connect {
                tab,
                profile,
                revision,
            } => {
                let config =
                    self.config.lock().unwrap().clone().ok_or_else(|| {
                        failure("configuration", "Load profiles before connecting")
                    })?;
                let profile = config
                    .resolve_profile(&ConnectionProfileId(profile), None)
                    .map_err(|error| failure("profile", error))?;
                let current = self.context.tabs.snapshot();
                if !self.context.tabs.select_at(current.selected, revision) {
                    return Err(failure(
                        "selection_superseded",
                        "A newer navigation request is active",
                    ));
                }
                if tab.is_none()
                    && current
                        .tabs
                        .iter()
                        .any(|tab| Some(tab.id) == current.selected && tab.engine.is_some())
                {
                    return Err(execution_error(ExecutionError::Busy));
                }
                let connection = if let Some(tab) = tab {
                    self.context
                        .start_connect_in_tab(dzban_core::tabs::TabId(tab), profile)
                } else {
                    self.context.start_connect_tab(profile)
                };
                let tab_id = connection.tab;
                let request = connection.id.0;
                let outcome = connection.wait().await;
                let selected = self.context.tabs.snapshot().selected == tab_id;
                let engine = outcome.map_err(|error| application_error("connection", error))?;
                let session = engine.id();
                let tab = self
                    .context
                    .tabs
                    .session(session)
                    .ok_or_else(|| failure("closed", "Query tab is closing"))?;
                let tab_id = tab.id;
                if tab.phase != dzban_core::tabs::TabPhase::Ready {
                    return Err(failure("closed", "Query tab is closing"));
                }
                if *self.context.subscribe_phase().borrow() != ApplicationPhase::Running
                    || !self
                        .context
                        .tabs
                        .session(session)
                        .is_some_and(|tab| tab.phase == dzban_core::tabs::TabPhase::Ready)
                {
                    return Err(failure("closed", "Application or query tab is closing"));
                }
                self.context
                    .health
                    .track_tab(&self.context, tab_id)
                    .map_err(|error| application_error("health", error))?;
                Ok(
                    json!({"request": request, "session": session.0, "tab": tab_id.0, "selected":selected}),
                )
            }
            Command::OpenDocument { session } => {
                let document = self
                    .context
                    .tabs
                    .session(SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?
                    .document;
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
            Command::ObserveDocument { .. }
            | Command::ObserveControls { .. }
            | Command::ObserveInteraction { .. }
            | Command::ObserveHealth {}
            | Command::ObserveAttention {} => Err(failure(
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
            } => self
                .output_reader(session, document, generation)?
                .viewport(start, count, column_start, column_count)
                .await
                .map(|page| json!(page))
                .map_err(output_error),
            Command::ReadPage {
                session,
                document,
                generation,
                start,
                count,
            } => {
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
            Command::HistoryList { session } => self
                .context
                .history_list(SessionId(session), self.history_location()?)
                .await
                .map(|projection| json!(projection))
                .map_err(output_error),
            Command::RestoreHistory {
                session,
                document,
                history_revision,
                index,
            } => self
                .context
                .restore_history(
                    SessionId(session),
                    DocumentId(document),
                    history_revision,
                    index,
                )
                .map(|projection| json!(projection))
                .map_err(output_error),
            Command::SavedPlan {
                session,
                document,
                generation,
            } => self
                .context
                .saved_plan(ResultScope {
                    session: SessionId(session),
                    document: DocumentId(document),
                    generation,
                })
                .map(|plan| json!({"plan":plan}))
                .map_err(output_error),
            Command::Peek {
                session,
                surface,
                event,
                elapsed_ms,
            } => {
                let tab = self
                    .context
                    .tabs
                    .session(SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                self.context
                    .tabs
                    .peek(
                        tab.id,
                        surface,
                        event,
                        std::time::Duration::from_millis(elapsed_ms.unwrap_or(0)),
                    )
                    .map(|result| json!(result))
                    .map_err(|error| application_error("peek", error))
            }
            Command::CancelPreview {
                session,
                request_revision,
            } => {
                let tab = self
                    .context
                    .tabs
                    .session(SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                self.context
                    .tabs
                    .cancel_preview(tab.id, request_revision)
                    .map_err(preview_error)?;
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
                let tab = self
                    .context
                    .tabs
                    .session(SessionId(session))
                    .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
                let result = self
                    .context
                    .tabs
                    .start_preview(
                        tab.id,
                        DocumentId(document),
                        revision,
                        sql,
                        request_revision,
                    )
                    .map_err(preview_error)?
                    .wait()
                    .await
                    .map_err(preview_error)?;
                let target = result.target;
                Ok(
                    json!({"request":target.request.0, "session":target.session.0, "document":target.document.0, "draft_revision":target.revision, "plan":PlanPresentation::from(&result.plan)}),
                )
            }
            Command::StartExport {
                export,
                session,
                document,
                generation,
                row,
                column,
                ranges,
            } => {
                let reader = self.output_reader(session, document, generation)?;
                let output = match (row, column, ranges) {
                    (Some(row), Some(column), None) => reader.export_cell(row, column).await,
                    (None, None, Some(ranges)) => reader.export_markdown(ranges).await,
                    _ => return Err(failure("invalid_selection", "Specify a cell or row ranges")),
                }
                .map_err(output_error)?;
                let mut exports = self.exports.lock().unwrap();
                if exports.len() >= 16 || exports.contains_key(&export) {
                    return Err(failure("admission", "Export reader limit reached"));
                }
                exports.insert(export, Arc::new(tokio::sync::Mutex::new(output)));
                Ok(json!({"opened": true}))
            }
            Command::ReadExport { export } => {
                let output = self
                    .exports
                    .lock()
                    .unwrap()
                    .get(&export)
                    .cloned()
                    .ok_or_else(|| failure("stale_output", "Export is closed"))?;
                let result = output.lock().await.next_chunk().await;
                result.map(|chunk| json!(chunk)).map_err(output_error)
            }
            Command::CloseExport { export } => {
                self.exports.lock().unwrap().remove(&export);
                Ok(json!({"closed": true}))
            }
            Command::ReadCell {
                session,
                document,
                generation,
                row,
                column,
                offset,
                count,
            } => self
                .output_reader(session, document, generation)?
                .cell(row, column, offset, count)
                .await
                .map(|chunk| json!(chunk))
                .map_err(output_error),
            Command::Shutdown {} => {
                self.context
                    .start_shutdown()
                    .map_err(|error| application_error("shutdown", error))?
                    .wait()
                    .await
                    .map_err(|error| application_error("shutdown", error))?;
                let _readers = self.readers.acquire_many(16).await.unwrap();
                self.exports.lock().unwrap().clear();
                let observers = std::mem::take(&mut *self.observers.lock().unwrap());
                for observer in observers {
                    if let Some(stream) = observer.stream.upgrade() {
                        stream.close();
                    }
                    observer.task.abort();
                    let _ = observer.task.await;
                }
                self.history_directory.lock().unwrap().take();
                Ok(json!({"shutdown": true, "attention":self.context.attention().snapshot()}))
            }
        }
    }
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
        assert!(app.state.context.tabs.snapshot().tabs.is_empty());
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
        let legacy = json!({"version":1,"database":"fixture","entries":[QueryHistoryEntry::failure("legacy CLI query".into(),"legacy failure".into(),Some(json!([{"Plan":{"Node Type":"Result","Plan Rows":1}}])),None)]});
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
        let plan = request(
            json!({"command":"saved_plan","session":session,"document":document,"generation":restored["generation"]}),
        );
        assert_eq!(plan["plan"]["text"], "Result  (rows=1)");
        assert!(plan["plan"].get("backend").is_none());
        assert!(plan["plan"].get("estimate").is_none());
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
