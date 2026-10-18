use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::application::ApplicationError;
use crate::documents::QueryDocument;
use crate::execution::{QueryEngine, SessionId};
use crate::{ConnectionProfileId, DatabaseBackend, DatabaseConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TabId(pub uuid::Uuid);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabPhase {
    Connecting,
    Ready,
    Failed(ApplicationError),
}

#[derive(Clone)]
pub struct QueryTab {
    pub id: TabId,
    pub profile: ConnectionProfileId,
    pub backend: DatabaseBackend,
    pub document: QueryDocument,
    pub engine: Option<Arc<QueryEngine>>,
    pub phase: TabPhase,
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
        self.document.submit(engine, sql, limits, history)
    }
}

#[derive(Clone, Default)]
pub struct TabsSnapshot {
    pub tabs: Vec<QueryTab>,
    pub selected: Option<TabId>,
}

#[derive(Default)]
struct TabState {
    next_number: u32,
    snapshot: TabsSnapshot,
}

pub struct QueryTabs {
    state: Mutex<TabState>,
    snapshots: watch::Sender<TabsSnapshot>,
}

impl Default for QueryTabs {
    fn default() -> Self {
        Self {
            state: Mutex::new(TabState::default()),
            snapshots: watch::channel(TabsSnapshot::default()).0,
        }
    }
}

impl QueryTabs {
    pub fn subscribe(&self) -> watch::Receiver<TabsSnapshot> {
        self.snapshots.subscribe()
    }

    pub fn snapshot(&self) -> TabsSnapshot {
        self.state.lock().unwrap().snapshot.clone()
    }

    pub(crate) fn begin(&self, profile: &DatabaseConfig) -> TabId {
        let mut state = self.state.lock().unwrap();
        state.next_number = state.next_number.checked_add(1).unwrap();
        let id = TabId(uuid::Uuid::new_v4());
        let document = QueryDocument::new(1);
        document.rename(&format!("Query {}", state.next_number));
        state.snapshot.tabs.push(QueryTab {
            id,
            profile: ConnectionProfileId(profile.name.clone()),
            backend: profile.backend,
            document,
            engine: None,
            phase: TabPhase::Connecting,
        });
        state.snapshot.selected = Some(id);
        self.snapshots.send_replace(state.snapshot.clone());
        id
    }

    pub(crate) fn complete(&self, id: TabId, result: &Result<Arc<QueryEngine>, ApplicationError>) {
        let mut state = self.state.lock().unwrap();
        if let Some(tab) = state.snapshot.tabs.iter_mut().find(|tab| tab.id == id) {
            match result {
                Ok(engine) => {
                    tab.engine = Some(engine.clone());
                    tab.phase = TabPhase::Ready;
                }
                Err(error) => tab.phase = TabPhase::Failed(error.clone()),
            }
            self.snapshots.send_replace(state.snapshot.clone());
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
        let mut state = self.state.lock().unwrap();
        if id.is_some_and(|id| !state.snapshot.tabs.iter().any(|tab| tab.id == id)) {
            return false;
        }
        state.snapshot.selected = id;
        self.snapshots.send_replace(state.snapshot.clone());
        true
    }

    pub(crate) fn finish_shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.snapshot = TabsSnapshot::default();
        self.snapshots.send_replace(state.snapshot.clone());
    }

    pub(crate) fn retire(&self, session: SessionId) -> Result<(), crate::error::ExecutionError> {
        let mut state = self.state.lock().unwrap();
        if let Some(index) = state.snapshot.tabs.iter().position(|tab| {
            tab.engine
                .as_ref()
                .is_some_and(|engine| engine.id() == session)
        }) {
            let tab = &state.snapshot.tabs[index];
            if let Some(engine) = &tab.engine {
                crate::documents::QueryDocuments::from_document(tab.document.clone())
                    .retire_closed_session(engine)?;
            }
            let removed = state.snapshot.tabs.remove(index);
            if state.snapshot.selected == Some(removed.id) {
                state.snapshot.selected = state
                    .snapshot
                    .tabs
                    .get(index)
                    .or_else(|| state.snapshot.tabs.last())
                    .map(|tab| tab.id);
            }
            self.snapshots.send_replace(state.snapshot.clone());
        }
        Ok(())
    }
}
