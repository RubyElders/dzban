use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::application::{ApplicationError, ConnectionRequestId};
use crate::documents::QueryDocument;
use crate::execution::{QueryEngine, SessionId};
use crate::{ConnectionProfileId, DatabaseBackend, DatabaseConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TabId(pub uuid::Uuid);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabPhase {
    Choosing,
    Connecting,
    Closing,
    Ready,
    Failed(ApplicationError),
}

pub(crate) enum ConnectionCompletion {
    Attached,
    Closing,
    Superseded,
}

#[derive(Clone)]
pub struct QueryTab {
    pub id: TabId,
    pub profile: Option<ConnectionProfileId>,
    pub backend: Option<DatabaseBackend>,
    pub document: QueryDocument,
    pub engine: Option<Arc<QueryEngine>>,
    pub phase: TabPhase,
    pub connection_request: Option<ConnectionRequestId>,
    retain_on_close: bool,
    attention: Arc<crate::attention::AttentionService>,
    input: Arc<Mutex<crate::interaction::TabInput>>,
    preview: Arc<Mutex<crate::preview::TabPreview>>,
}

impl QueryTab {
    pub fn submit(
        &self,
        sql: String,
        limits: crate::result_store::ResultLimits,
        history: Option<crate::history::QueryHistoryStore>,
    ) -> Result<crate::document_execution::DocumentQueryHandle, crate::error::ExecutionError> {
        let engine = self
            .engine
            .clone()
            .ok_or(crate::error::ExecutionError::Closed)?;
        self.document
            .submit(engine, sql, limits, history)
            .map(|query| {
                query.with_attention(self.attention.clone(), self.id, self.document.clone())
            })
    }
}

#[derive(Clone, Default)]
pub struct TabsSnapshot {
    pub revision: u64,
    pub tabs: Vec<QueryTab>,
    pub selected: Option<TabId>,
}

#[derive(Default)]
struct TabState {
    next_number: u32,
    selection_revision: u64,
    snapshot: TabsSnapshot,
    controls: Option<crate::presentation::ControlsPresentation>,
}

pub struct QueryTabs {
    attention: Arc<crate::attention::AttentionService>,
    state: Mutex<TabState>,
    snapshots: watch::Sender<TabsSnapshot>,
}

impl Default for QueryTabs {
    fn default() -> Self {
        Self {
            attention: Arc::new(Default::default()),
            state: Mutex::new(TabState::default()),
            snapshots: watch::channel(TabsSnapshot::default()).0,
        }
    }
}

impl QueryTabs {
    pub fn attention(&self) -> &crate::attention::AttentionService {
        &self.attention
    }
    pub(crate) fn health_available(&self, tab: TabId) -> bool {
        let state = self.state.lock().unwrap();
        let Some(tab) = state
            .snapshot
            .tabs
            .iter()
            .find(|candidate| candidate.id == tab)
        else {
            return false;
        };
        tab.phase == TabPhase::Ready
            && !tab.document.is_active()
            && (state.snapshot.selected != Some(tab.id)
                || tab.input.lock().unwrap().health_available())
    }
    pub async fn await_plan(
        &self,
        tab: TabId,
        revision: u64,
    ) -> Result<crate::presentation::InteractionPresentation, ApplicationError> {
        let mut snapshots = self.subscribe_interaction(tab)?;
        loop {
            if let Some(snapshot) = snapshots.borrow_and_update().clone()
                && (snapshot.retired || (snapshot.revision >= revision && !snapshot.loading))
            {
                return Ok(snapshot);
            }
            snapshots
                .changed()
                .await
                .map_err(|_| ApplicationError::Operation(crate::error::ExecutionError::Closed))?;
        }
    }

    pub fn subscribe_interaction(
        &self,
        tab: TabId,
    ) -> Result<
        watch::Receiver<Option<crate::presentation::InteractionPresentation>>,
        ApplicationError,
    > {
        let tab = self.find(tab).ok_or(ApplicationError::UnknownTab(tab))?;
        let engine = tab.engine.as_ref().ok_or(ApplicationError::Operation(
            crate::error::ExecutionError::Closed,
        ))?;
        Ok(tab.input.lock().unwrap().subscribe(
            engine.id(),
            tab.document.id(),
            tab.document.snapshot().draft_revision,
        ))
    }

    pub(crate) fn query_started(&self, tab: TabId) {
        if let Some(tab) = self.find(tab)
            && let Some(engine) = &tab.engine
        {
            tab.preview.lock().unwrap().query_started();
            tab.input.lock().unwrap().query_started(
                engine.id(),
                tab.document.id(),
                tab.document.snapshot().draft_revision,
            );
        }
    }

    pub(crate) fn refresh_interaction(&self, tab: TabId) {
        if let Some(tab) = self.find(tab)
            && let Some(engine) = &tab.engine
        {
            tab.input.lock().unwrap().refresh(
                engine,
                tab.document.id(),
                tab.document.snapshot().draft_revision,
            );
        }
    }

    pub(crate) fn selected_target(
        &self,
        tab: TabId,
        document: crate::documents::DocumentId,
    ) -> Result<QueryTab, ApplicationError> {
        let state = self.state.lock().unwrap();
        if state.snapshot.selected != Some(tab) {
            return Err(ApplicationError::SelectionSuperseded);
        }
        let tab = state
            .snapshot
            .tabs
            .iter()
            .find(|candidate| candidate.id == tab)
            .ok_or(ApplicationError::UnknownTab(tab))?;
        if tab.document.id() != document {
            return Err(ApplicationError::SelectionSuperseded);
        }
        Ok(tab.clone())
    }

    pub fn controls(&self) -> crate::presentation::ControlsPresentation {
        use crate::interaction::{AppEvent, AppState, SessionInteraction};
        let mut state = self.state.lock().unwrap();
        let snapshot = &state.snapshot;
        let selected = snapshot
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == snapshot.selected)
            .and_then(|tab| tab.engine.as_ref())
            .map(|engine| SessionInteraction::from_engine(engine));
        let mut interaction = AppState::default();
        interaction.transition(AppEvent::SessionSelected(selected));
        let mut projection = crate::presentation::ControlsPresentation {
            revision: state
                .controls
                .as_ref()
                .map_or(0, |controls| controls.revision),
            session: interaction.scope.map(|scope| scope.session.0),
            query: interaction
                .scope
                .and_then(|scope| scope.query.map(|id| id.0)),
            primary_action: interaction.primary_action(),
        };
        if state.controls.as_ref() != Some(&projection) {
            projection.revision = projection.revision.checked_add(1).unwrap();
            state.controls = Some(projection.clone());
        }
        projection
    }

    pub fn start_preview(
        &self,
        tab: TabId,
        document: crate::documents::DocumentId,
        draft_revision: u64,
        sql: String,
        request_revision: Option<u64>,
    ) -> Result<crate::preview::PreviewHandle, crate::preview::PreviewError> {
        self.preview_request(
            crate::preview::PreviewCommand {
                tab,
                document,
                draft_revision,
                sql,
                request_revision,
            },
            request_revision.is_some(),
        )
    }

    pub(crate) fn start_input_preview(
        &self,
        tab: TabId,
        document: crate::documents::DocumentId,
        draft_revision: u64,
        sql: String,
    ) -> Result<crate::preview::PreviewHandle, crate::preview::PreviewError> {
        let result = self.preview_request(
            crate::preview::PreviewCommand {
                tab,
                document,
                draft_revision,
                sql,
                request_revision: None,
            },
            true,
        );
        if matches!(result, Err(crate::preview::PreviewError::Invalidated(_))) {
            let state = self.state.lock().unwrap();
            if state.snapshot.selected == Some(tab)
                && let Some(tab) = state.snapshot.tabs.iter().find(|target| target.id == tab)
                && tab.document.id() == document
                && let Some(engine) = &tab.engine
            {
                let current_revision = tab.document.snapshot().draft_revision;
                if current_revision != draft_revision {
                    tab.input.lock().unwrap().invalidate_draft(
                        engine,
                        document,
                        draft_revision,
                        current_revision,
                    );
                }
            }
        }
        result
    }

    fn preview_request(
        &self,
        command: crate::preview::PreviewCommand,
        require_selected: bool,
    ) -> Result<crate::preview::PreviewHandle, crate::preview::PreviewError> {
        let crate::preview::PreviewCommand {
            tab,
            document,
            draft_revision,
            sql,
            request_revision,
        } = command;
        use crate::preview::PreviewError;
        let state = self.state.lock().unwrap();
        if require_selected && state.snapshot.selected != Some(tab) {
            return Err(PreviewError::Invalidated("Workspace is no longer selected"));
        }
        let tab = state
            .snapshot
            .tabs
            .iter()
            .find(|candidate| candidate.id == tab)
            .ok_or(PreviewError::Application(ApplicationError::UnknownTab(tab)))?;
        if tab.document.id() != document || tab.document.snapshot().draft_revision != draft_revision
        {
            return Err(PreviewError::Invalidated("Draft revision has changed"));
        }
        let engine = tab.engine.clone().ok_or(PreviewError::Execution(
            crate::error::ExecutionError::Closed,
        ))?;
        tab.preview.lock().unwrap().start(
            engine,
            tab.document.clone(),
            draft_revision,
            sql,
            request_revision,
        )
    }

    pub(crate) fn complete_input_preview(
        &self,
        tab: TabId,
        target: crate::preview::PreviewTarget,
        result: Result<crate::preview::PreviewResult, crate::preview::PreviewError>,
    ) {
        let state = self.state.lock().unwrap();
        if state.snapshot.selected != Some(tab) {
            return;
        }
        let Some(tab) = state
            .snapshot
            .tabs
            .iter()
            .find(|candidate| candidate.id == tab)
        else {
            return;
        };
        let document = tab.document.snapshot();
        if document.id != target.document
            || document.draft_revision != target.revision
            || document.lifecycle != crate::documents::DocumentLifecycle::Open
            || document.execution.is_active()
            || !tab.preview.lock().unwrap().is_current(target)
        {
            return;
        }
        let Some(engine) = &tab.engine else {
            return;
        };
        let event = match result {
            Ok(result) => crate::interaction::InputEvent::PreviewReady { plan: result.plan },
            Err(crate::preview::PreviewError::Execution(error)) => {
                crate::interaction::InputEvent::PreviewFailed {
                    message: error.to_string(),
                }
            }
            Err(_) => return,
        };
        tab.input
            .lock()
            .unwrap()
            .transition(engine, target.document, target.revision, event);
    }

    pub fn cancel_preview(
        &self,
        tab: TabId,
        revision: Option<u64>,
    ) -> Result<(), crate::preview::PreviewError> {
        let tab = self
            .find(tab)
            .ok_or(crate::preview::PreviewError::Application(
                ApplicationError::UnknownTab(tab),
            ))?;
        tab.preview.lock().unwrap().cancel(revision)
    }

    pub fn peek(
        &self,
        tab: TabId,
        surface: crate::preview::PreviewSurface,
        event: crate::preview::PeekEvent,
        elapsed: std::time::Duration,
    ) -> Result<crate::preview::PeekVisibility, ApplicationError> {
        let tab = self.find(tab).ok_or(ApplicationError::UnknownTab(tab))?;
        Ok(tab.preview.lock().unwrap().peek(surface, event, elapsed))
    }

    pub fn input(
        &self,
        tab: TabId,
        document: crate::documents::DocumentId,
        revision: u64,
        event: crate::interaction::InputEvent,
    ) -> Result<crate::presentation::InputPresentation, ApplicationError> {
        self.transition_input(tab, document, Some(revision), event)
    }

    pub(crate) fn input_for_command(
        &self,
        tab: TabId,
        document: crate::documents::DocumentId,
        event: crate::interaction::InputEvent,
    ) -> Result<crate::presentation::InputPresentation, ApplicationError> {
        self.transition_input(tab, document, None, event)
    }

    fn transition_input(
        &self,
        tab: TabId,
        document: crate::documents::DocumentId,
        expected_revision: Option<u64>,
        event: crate::interaction::InputEvent,
    ) -> Result<crate::presentation::InputPresentation, ApplicationError> {
        let state = self.state.lock().unwrap();
        if state.snapshot.selected != Some(tab) {
            return Err(ApplicationError::SelectionSuperseded);
        }
        let tab = state
            .snapshot
            .tabs
            .iter()
            .find(|candidate| candidate.id == tab)
            .ok_or(ApplicationError::UnknownTab(tab))?;
        let revision = tab.document.snapshot().draft_revision;
        if tab.document.id() != document
            || expected_revision.is_some_and(|expected| expected != revision)
        {
            return Err(ApplicationError::SelectionSuperseded);
        }
        let engine = tab.engine.as_ref().ok_or(ApplicationError::Operation(
            crate::error::ExecutionError::Closed,
        ))?;
        Ok(tab
            .input
            .lock()
            .unwrap()
            .transition(engine, document, revision, event))
    }

    pub fn reset_input(&self) {
        Self::reset_state_input(&self.state.lock().unwrap());
    }

    fn reset_state_input(state: &TabState) {
        for tab in &state.snapshot.tabs {
            tab.input.lock().unwrap().reset();
            tab.preview.lock().unwrap().reset();
        }
    }

    fn publish(&self, state: &mut TabState) {
        state.snapshot.revision = state.snapshot.revision.checked_add(1).unwrap();
        self.snapshots.send_replace(state.snapshot.clone());
    }

    pub fn subscribe(&self) -> watch::Receiver<TabsSnapshot> {
        self.snapshots.subscribe()
    }

    pub fn snapshot(&self) -> TabsSnapshot {
        self.state.lock().unwrap().snapshot.clone()
    }

    pub(crate) fn open(
        &self,
        profile: Option<ConnectionProfileId>,
        draft: String,
        revision: Option<u64>,
    ) -> Result<TabId, ApplicationError> {
        let mut state = self.state.lock().unwrap();
        self.open_locked(&mut state, profile, draft, revision)
    }

    pub(crate) fn open_if_empty(
        &self,
        profile: Option<ConnectionProfileId>,
        draft: String,
    ) -> Option<TabId> {
        let mut state = self.state.lock().unwrap();
        if !state.snapshot.tabs.is_empty() {
            return None;
        }
        self.open_locked(&mut state, profile, draft, None).ok()
    }

    fn open_locked(
        &self,
        state: &mut TabState,
        profile: Option<ConnectionProfileId>,
        draft: String,
        revision: Option<u64>,
    ) -> Result<TabId, ApplicationError> {
        if revision.is_some_and(|revision| revision < state.selection_revision) {
            return Err(ApplicationError::SelectionSuperseded);
        }
        if let Some(revision) = revision {
            state.selection_revision = revision;
        }
        Self::reset_state_input(state);
        state.next_number = state.next_number.checked_add(1).unwrap();
        let id = TabId(uuid::Uuid::new_v4());
        let document = QueryDocument::new(1);
        document.rename(&format!("Query {}", state.next_number));
        document.set_draft(draft);
        let document_id = document.id().0;
        state.snapshot.tabs.push(QueryTab {
            id,
            profile,
            backend: None,
            document,
            engine: None,
            phase: TabPhase::Choosing,
            connection_request: None,
            retain_on_close: false,
            attention: self.attention.clone(),
            input: Default::default(),
            preview: Default::default(),
        });
        state.snapshot.selected = Some(id);
        self.attention.register(id.0, document_id);
        self.publish(state);
        Ok(id)
    }

    pub fn find(&self, id: TabId) -> Option<QueryTab> {
        self.state
            .lock()
            .unwrap()
            .snapshot
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .cloned()
    }

    pub(crate) fn begin(
        &self,
        id: TabId,
        request: ConnectionRequestId,
        profile: &DatabaseConfig,
    ) -> Result<(), ApplicationError> {
        let mut state = self.state.lock().unwrap();
        let tab = state
            .snapshot
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .ok_or(ApplicationError::UnknownTab(id))?;
        if !matches!(tab.phase, TabPhase::Choosing | TabPhase::Failed(_)) {
            return Err(ApplicationError::Operation(
                crate::error::ExecutionError::Busy,
            ));
        }
        tab.profile = Some(ConnectionProfileId(profile.name.clone()));
        tab.backend = Some(profile.backend);
        tab.connection_request = Some(request);
        tab.phase = TabPhase::Connecting;
        self.publish(&mut state);
        Ok(())
    }

    pub(crate) fn complete(
        &self,
        id: TabId,
        request: ConnectionRequestId,
        result: &Result<Arc<QueryEngine>, ApplicationError>,
    ) -> ConnectionCompletion {
        let mut state = self.state.lock().unwrap();
        let Some(tab) = state
            .snapshot
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id && tab.connection_request == Some(request))
        else {
            return ConnectionCompletion::Superseded;
        };
        tab.connection_request = None;
        let closing = tab.phase == TabPhase::Closing;
        match result {
            Ok(engine) => {
                tab.engine = Some(engine.clone());
                if !closing {
                    tab.phase = TabPhase::Ready;
                }
            }
            Err(error) => {
                if !closing {
                    tab.phase = TabPhase::Failed(error.clone());
                }
            }
        }
        self.publish(&mut state);
        if closing {
            ConnectionCompletion::Closing
        } else {
            ConnectionCompletion::Attached
        }
    }

    pub(crate) fn begin_close(&self, id: TabId, retain: bool) -> Result<(), ApplicationError> {
        let mut state = self.state.lock().unwrap();
        let tab = state
            .snapshot
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .ok_or(ApplicationError::UnknownTab(id))?;
        if tab.phase == TabPhase::Closing {
            return Err(ApplicationError::Operation(
                crate::error::ExecutionError::Busy,
            ));
        }
        if retain && tab.document.snapshot().output_generation != 0 {
            return Err(ApplicationError::Operation(
                crate::error::ExecutionError::Busy,
            ));
        }
        tab.retain_on_close = retain;
        self.attention.closing(id.0, true);
        tab.phase = TabPhase::Closing;
        self.publish(&mut state);
        Ok(())
    }

    pub(crate) fn mark_closing(&self, session: SessionId) {
        let mut state = self.state.lock().unwrap();
        if let Some(tab) = state.snapshot.tabs.iter_mut().find(|tab| {
            tab.engine
                .as_ref()
                .is_some_and(|engine| engine.id() == session)
        }) && tab.phase != TabPhase::Closing
        {
            self.attention.closing(tab.id.0, true);
            tab.phase = TabPhase::Closing;
            self.publish(&mut state);
        }
    }

    pub(crate) fn close_failed(&self, id: TabId) {
        let mut state = self.state.lock().unwrap();
        if let Some(tab) = state.snapshot.tabs.iter_mut().find(|tab| tab.id == id) {
            self.attention.closing(id.0, false);
            tab.retain_on_close = false;
            tab.phase = if tab.engine.is_some() {
                TabPhase::Ready
            } else if tab.connection_request.is_some() {
                TabPhase::Connecting
            } else {
                TabPhase::Choosing
            };
            self.publish(&mut state);
        }
    }

    pub(crate) fn finish_unconnected_close(&self, id: TabId) -> Result<(), ApplicationError> {
        let mut state = self.state.lock().unwrap();
        let index = state
            .snapshot
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or(ApplicationError::UnknownTab(id))?;
        if state.snapshot.tabs[index].engine.is_some()
            || state.snapshot.tabs[index].connection_request.is_some()
        {
            return Err(ApplicationError::Operation(
                crate::error::ExecutionError::Busy,
            ));
        }
        if state.snapshot.tabs[index].retain_on_close {
            let tab = &mut state.snapshot.tabs[index];
            tab.phase = TabPhase::Choosing;
            self.attention.closing(tab.id.0, false);
            tab.retain_on_close = false;
            tab.input.lock().unwrap().reset();
            self.publish(&mut state);
            return Ok(());
        }
        if !state.snapshot.tabs[index].document.close() {
            return Err(ApplicationError::Operation(
                crate::error::ExecutionError::Busy,
            ));
        }
        self.remove(&mut state, index);
        self.publish(&mut state);
        Ok(())
    }

    fn remove(&self, state: &mut TabState, index: usize) {
        let removed = state.snapshot.tabs.remove(index);
        self.attention.retire(removed.id.0);
        removed.input.lock().unwrap().retire();
        removed.preview.lock().unwrap().reset();
        if state.snapshot.selected == Some(removed.id) {
            state.snapshot.selected = state
                .snapshot
                .tabs
                .get(index)
                .or_else(|| state.snapshot.tabs.last())
                .map(|tab| tab.id);
        }
    }

    pub fn session(&self, session: SessionId) -> Option<QueryTab> {
        self.state
            .lock()
            .unwrap()
            .snapshot
            .tabs
            .iter()
            .find(|tab| {
                tab.engine
                    .as_ref()
                    .is_some_and(|engine| engine.id() == session)
            })
            .cloned()
    }

    pub fn select_session(&self, session: Option<SessionId>) -> bool {
        match session {
            Some(session) => self
                .session(session)
                .is_some_and(|tab| self.select(Some(tab.id))),
            None => self.select(None),
        }
    }

    pub fn select(&self, id: Option<TabId>) -> bool {
        self.select_at(id, None)
    }

    pub fn select_at(&self, id: Option<TabId>, revision: Option<u64>) -> bool {
        let mut state = self.state.lock().unwrap();
        if revision.is_some_and(|revision| revision < state.selection_revision) {
            return false;
        }
        if id.is_some_and(|id| !state.snapshot.tabs.iter().any(|tab| tab.id == id)) {
            return false;
        }
        if let Some(revision) = revision {
            state.selection_revision = revision;
        }
        Self::reset_state_input(&state);
        state.snapshot.selected = id;
        self.publish(&mut state);
        true
    }

    pub fn cycle(&self, direction: i64, revision: Option<u64>) -> bool {
        let mut state = self.state.lock().unwrap();
        if revision.is_some_and(|revision| revision < state.selection_revision) {
            return false;
        }
        if let Some(revision) = revision {
            state.selection_revision = revision;
        }
        if state.snapshot.tabs.is_empty() {
            return true;
        }
        let index = state
            .snapshot
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == state.snapshot.selected)
            .unwrap_or(0);
        let index = (index as i128 + i128::from(direction))
            .rem_euclid(state.snapshot.tabs.len() as i128) as usize;
        Self::reset_state_input(&state);
        state.snapshot.selected = Some(state.snapshot.tabs[index].id);
        self.publish(&mut state);
        true
    }

    pub fn move_tab(&self, id: TabId, index: usize) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(current) = state.snapshot.tabs.iter().position(|tab| tab.id == id) else {
            return false;
        };
        let index = index.min(state.snapshot.tabs.len() - 1);
        if current != index {
            let tab = state.snapshot.tabs.remove(current);
            state.snapshot.tabs.insert(index, tab);
            self.publish(&mut state);
        }
        true
    }

    pub(crate) fn finish_shutdown(&self) {
        self.attention.clear();
        let mut state = self.state.lock().unwrap();
        for tab in &state.snapshot.tabs {
            tab.document.close();
            tab.input.lock().unwrap().retire();
            tab.preview.lock().unwrap().reset();
        }
        state.snapshot = TabsSnapshot {
            revision: state.snapshot.revision,
            ..Default::default()
        };
        self.publish(&mut state);
    }

    pub(crate) fn retire(&self, session: SessionId) -> Result<(), crate::error::ExecutionError> {
        let mut state = self.state.lock().unwrap();
        if let Some(index) = state.snapshot.tabs.iter().position(|tab| {
            tab.engine
                .as_ref()
                .is_some_and(|engine| engine.id() == session)
        }) {
            let tab = &mut state.snapshot.tabs[index];
            if tab.retain_on_close {
                tab.engine = None;
                tab.connection_request = None;
                tab.phase = TabPhase::Choosing;
                self.attention.closing(tab.id.0, false);
                tab.retain_on_close = false;
                tab.input.lock().unwrap().reset();
                self.publish(&mut state);
                return Ok(());
            }
            if let Some(engine) = &tab.engine {
                crate::documents::QueryDocuments::from_document(tab.document.clone())
                    .retire_closed_session(engine)?;
            }
            self.remove(&mut state, index);
            self.publish(&mut state);
        }
        Ok(())
    }
}
