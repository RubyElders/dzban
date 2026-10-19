use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::application::{ApplicationContext, ApplicationError};
use crate::documents::DocumentId;
use crate::error::ExecutionError;
use crate::execution::{OperationState, QueryEngine, SessionPhase};
use crate::history::QueryHistoryStore;
use crate::interaction::{AppCommand, InputEvent};
use crate::preview::PreviewError;
use crate::tabs::TabId;

#[derive(Clone, Debug)]
pub enum HistoryLocation {
    Default,
    File(PathBuf),
}

impl HistoryLocation {
    pub async fn load(&self, profile: String) -> Result<QueryHistoryStore, CommandError> {
        let location = self.clone();
        tokio::task::spawn_blocking(move || match location {
            Self::Default => QueryHistoryStore::load(&profile),
            Self::File(path) => QueryHistoryStore::load_from_path(path, &profile),
        })
        .await
        .map_err(|error| CommandError::Worker(error.to_string()))?
        .map_err(CommandError::History)
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorIntent {
    Submit,
    Primary,
    Cancel,
    Reset,
}

#[derive(Clone, Debug)]
pub struct EditorCommand {
    pub tab: TabId,
    pub document: DocumentId,
    pub draft: String,
    pub selection: std::ops::Range<usize>,
    pub intent: EditorIntent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QuerySubmission {
    pub query: uuid::Uuid,
    pub document: uuid::Uuid,
    pub session: uuid::Uuid,
    pub generation: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct EditorOutcome {
    pub submission: Option<QuerySubmission>,
    pub cancellation_requested: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    Application(ApplicationError),
    Execution(ExecutionError),
    Preview(PreviewError),
    Selection(&'static str),
    History(String),
    Worker(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Application(error) => error.fmt(formatter),
            Self::Execution(error) => error.fmt(formatter),
            Self::Preview(error) => error.fmt(formatter),
            Self::Selection(message) => formatter.write_str(message),
            Self::History(message) | Self::Worker(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CommandError {}

pub struct InputCommand {
    pub tab: TabId,
    pub document: DocumentId,
    pub revision: u64,
    pub draft: String,
    pub selection: std::ops::Range<usize>,
    pub event: InputEvent,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InputOutcome {
    pub interaction: crate::presentation::InteractionPresentation,
    pub outcome: EditorOutcome,
}

pub struct InputCommandHandle {
    task: tokio::task::JoinHandle<Result<InputOutcome, CommandError>>,
}

impl InputCommandHandle {
    pub async fn wait(self) -> Result<InputOutcome, CommandError> {
        self.task
            .await
            .map_err(|error| CommandError::Worker(error.to_string()))?
    }
}

pub struct EditorCommandHandle {
    task: tokio::task::JoinHandle<Result<EditorOutcome, CommandError>>,
}

impl EditorCommandHandle {
    pub async fn wait(self) -> Result<EditorOutcome, CommandError> {
        self.task
            .await
            .map_err(|error| CommandError::Worker(error.to_string()))?
    }
}

impl ApplicationContext {
    pub fn start_input_command(
        self: &Arc<Self>,
        command: InputCommand,
        history: HistoryLocation,
    ) -> InputCommandHandle {
        let context = self.clone();
        InputCommandHandle {
            task: tokio::spawn(async move {
                let snapshots =
                    context
                        .tabs
                        .subscribe_interaction(command.tab)
                        .map_err(|error| {
                            CommandError::Application(match error {
                                ApplicationError::UnknownTab(_) => {
                                    ApplicationError::SelectionSuperseded
                                }
                                error => error,
                            })
                        })?;
                let projection = context
                    .tabs
                    .input(
                        command.tab,
                        command.document,
                        command.revision,
                        command.event,
                    )
                    .map_err(CommandError::Application)?;
                let outcome = context
                    .execute_effects(
                        EditorCommand {
                            tab: command.tab,
                            document: command.document,
                            draft: command.draft,
                            selection: command.selection,
                            intent: EditorIntent::Reset,
                        },
                        command.revision,
                        projection.commands,
                        history,
                    )
                    .await?;
                let interaction = snapshots.borrow().clone().unwrap();
                Ok(InputOutcome {
                    interaction,
                    outcome,
                })
            }),
        }
    }

    pub fn start_editor_command(
        self: &Arc<Self>,
        command: EditorCommand,
        history: HistoryLocation,
    ) -> EditorCommandHandle {
        let context = self.clone();
        EditorCommandHandle {
            task: tokio::spawn(async move { context.editor_command(command, history).await }),
        }
    }

    async fn editor_command(
        self: &Arc<Self>,
        command: EditorCommand,
        history: HistoryLocation,
    ) -> Result<EditorOutcome, CommandError> {
        let target = || {
            self.tabs
                .selected_target(command.tab, command.document)
                .map_err(CommandError::Application)
        };
        let tab = target()?;
        let engine = tab
            .engine
            .clone()
            .ok_or(CommandError::Execution(ExecutionError::Closed))?;
        if !matches!(command.intent, EditorIntent::Reset) {
            self.tabs
                .cancel_preview(tab.id, None)
                .map_err(CommandError::Preview)?;
            await_query_admission(&engine)
                .await
                .map_err(CommandError::Execution)?;
        }
        let tab = target()?;
        let event = match command.intent {
            EditorIntent::Primary | EditorIntent::Submit => InputEvent::PrimaryAction,
            EditorIntent::Cancel => InputEvent::Cancel,
            EditorIntent::Reset => InputEvent::Reset,
        };
        let projection = self
            .tabs
            .input_for_command(tab.id, tab.document.id(), event)
            .map_err(CommandError::Application)?;
        self.execute_effects(
            command,
            tab.document.snapshot().draft_revision,
            projection.commands,
            history,
        )
        .await
    }

    async fn execute_effects(
        self: &Arc<Self>,
        command: EditorCommand,
        source_revision: u64,
        effects: Vec<AppCommand>,
        history: HistoryLocation,
    ) -> Result<EditorOutcome, CommandError> {
        let target = || {
            self.tabs
                .selected_target(command.tab, command.document)
                .map_err(CommandError::Application)
        };
        let tab = target()?;
        let engine = tab
            .engine
            .clone()
            .ok_or(CommandError::Execution(ExecutionError::Closed))?;
        let mut outcome = EditorOutcome::default();
        for effect in effects {
            match effect {
                AppCommand::BeginPreview => {
                    let sql = match crate::sql::selected_sql(
                        &command.draft,
                        command.selection.start,
                        command.selection.end,
                    ) {
                        Ok(sql) => sql,
                        Err(message) => {
                            self.tabs
                                .input_for_command(
                                    tab.id,
                                    tab.document.id(),
                                    InputEvent::PreviewFailed {
                                        message: message.into(),
                                    },
                                )
                                .map_err(CommandError::Application)?;
                            continue;
                        }
                    };
                    let started = match await_query_admission(&engine).await {
                        Ok(_) => self.tabs.start_input_preview(
                            tab.id,
                            tab.document.id(),
                            source_revision,
                            sql,
                        ),
                        Err(error) => Err(PreviewError::Execution(error)),
                    };
                    let handle = match started {
                        Ok(handle) => handle,
                        Err(PreviewError::Execution(error)) => {
                            self.tabs
                                .input_for_command(
                                    tab.id,
                                    tab.document.id(),
                                    InputEvent::PreviewFailed {
                                        message: error.to_string(),
                                    },
                                )
                                .map_err(CommandError::Application)?;
                            continue;
                        }
                        Err(error) => return Err(CommandError::Preview(error)),
                    };
                    let request = handle.target();
                    let context = self.clone();
                    tokio::spawn(async move {
                        let result = handle.wait().await;
                        context
                            .tabs
                            .complete_input_preview(command.tab, request, result);
                    });
                }
                AppCommand::RunQuery => {
                    self.tabs
                        .cancel_preview(tab.id, None)
                        .map_err(CommandError::Preview)?;
                    self.tabs
                        .input_for_command(tab.id, tab.document.id(), InputEvent::PreviewCanceled)
                        .map_err(CommandError::Application)?;
                    await_query_admission(&engine)
                        .await
                        .map_err(CommandError::Execution)?;
                    let sql = crate::sql::selected_sql(
                        &command.draft,
                        command.selection.start,
                        command.selection.end,
                    )
                    .map_err(CommandError::Selection)?;
                    let profile = tab
                        .profile
                        .as_ref()
                        .ok_or(CommandError::Execution(ExecutionError::Closed))?;
                    let history = history.load(profile.0.clone()).await?;
                    let tab = target()?;
                    let query = tab
                        .submit(sql, Default::default(), Some(history))
                        .map_err(CommandError::Execution)?;
                    self.tabs.query_started(tab.id);
                    outcome.submission = Some(QuerySubmission {
                        query: query.query.0,
                        document: query.document.0,
                        session: query.session.0,
                        generation: query.generation,
                    });
                    let context = self.clone();
                    let mut updates = tab.document.subscribe();
                    tokio::spawn(async move {
                        let completion = query.wait();
                        tokio::pin!(completion);
                        loop {
                            context.tabs.refresh_interaction(tab.id);
                            tokio::select! {
                                _ = &mut completion => { context.tabs.refresh_interaction(tab.id); break; },
                                changed = updates.changed() => if changed.is_err() { break; },
                            }
                        }
                    });
                }
                AppCommand::CancelQuery if tab.document.is_active() => {
                    tab.document
                        .request_cancel(engine.clone())
                        .map_err(CommandError::Execution)?
                        .wait()
                        .await
                        .map_err(CommandError::Execution)?;
                    outcome.cancellation_requested = true;
                }
                AppCommand::CancelPreview => {
                    self.tabs
                        .cancel_preview(tab.id, None)
                        .map_err(CommandError::Preview)?;
                    self.tabs
                        .input_for_command(tab.id, tab.document.id(), InputEvent::PreviewCanceled)
                        .map_err(CommandError::Application)?;
                }
                _ => (),
            }
        }
        Ok(outcome)
    }
}

pub async fn await_query_admission(engine: &QueryEngine) -> Result<bool, ExecutionError> {
    let mut operation = engine.subscribe_operation();
    let mut lifecycle = engine.subscribe_session();
    loop {
        if *lifecycle.borrow_and_update() != SessionPhase::Open {
            return Err(ExecutionError::Closed);
        }
        match *operation.borrow_and_update() {
            OperationState::Idle => return Ok(true),
            OperationState::Preview | OperationState::Ping | OperationState::Version => (),
            OperationState::Query { .. } => return Ok(false),
            _ => return Err(ExecutionError::Busy),
        }
        tokio::select! {
            changed = operation.changed() => if changed.is_err() { return Err(ExecutionError::Closed); },
            changed = lifecycle.changed() => if changed.is_err() { return Err(ExecutionError::Closed); },
        }
    }
}
