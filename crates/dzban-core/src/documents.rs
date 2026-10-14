use std::sync::Arc;

use tokio::sync::watch;

use crate::QueryPlan;
use crate::execution::{QueryId, QuerySnapshot, SessionId, StreamedQueryCompletion};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(pub uuid::Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentLifecycle {
    Open,
    Closed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DocumentExecution {
    #[default]
    Idle,
    Planning(QueryId),
    Running(QueryId),
    Canceling(QueryId),
    Finished(QueryId),
}

impl DocumentExecution {
    pub fn query(&self) -> Option<QueryId> {
        match self {
            Self::Idle => None,
            Self::Planning(query)
            | Self::Running(query)
            | Self::Canceling(query)
            | Self::Finished(query) => Some(*query),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Planning(_) | Self::Running(_) | Self::Canceling(_)
        )
    }
}

#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    pub id: DocumentId,
    pub number: u32,
    pub lifecycle: DocumentLifecycle,
    pub title: String,
    pub draft: String,
    pub draft_revision: u64,
    pub execution: DocumentExecution,
    pub session: Option<SessionId>,
    pub output_generation: u64,
    pub plan: Option<QueryPlan>,
    pub operation: Option<QuerySnapshot>,
    pub completion: Option<Result<Arc<StreamedQueryCompletion>, crate::error::ExecutionError>>,
}

#[derive(Clone, Debug)]
pub struct QueryDocument {
    state: Arc<watch::Sender<DocumentSnapshot>>,
}

impl QueryDocument {
    fn new(number: u32) -> Self {
        let snapshot = DocumentSnapshot {
            id: DocumentId(uuid::Uuid::new_v4()),
            number,
            lifecycle: DocumentLifecycle::Open,
            title: format!("Query {number}"),
            draft: String::new(),
            draft_revision: 0,
            execution: DocumentExecution::Idle,
            session: None,
            output_generation: 0,
            plan: None,
            operation: None,
            completion: None,
        };
        Self {
            state: Arc::new(watch::channel(snapshot).0),
        }
    }

    pub fn snapshot(&self) -> DocumentSnapshot {
        self.state.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<DocumentSnapshot> {
        self.state.subscribe()
    }

    pub fn id(&self) -> DocumentId {
        self.state.borrow().id
    }

    pub fn number(&self) -> u32 {
        self.state.borrow().number
    }

    pub fn rename(&self, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        self.state.send_if_modified(|state| {
            if state.lifecycle != DocumentLifecycle::Open {
                return false;
            }
            state.title = title.to_string();
            true
        })
    }

    pub fn set_draft(&self, draft: String) -> bool {
        self.state.send_if_modified(|state| {
            if state.lifecycle != DocumentLifecycle::Open {
                return false;
            }
            state.draft = draft;
            state.draft_revision = state.draft_revision.checked_add(1).unwrap();
            true
        })
    }

    pub fn draft(&self) -> String {
        self.state.borrow().draft.clone()
    }

    pub fn query_context_changed(&self) {
        self.state.send_modify(|state| {
            state.draft_revision = state.draft_revision.checked_add(1).unwrap();
        });
    }

    pub fn is_active(&self) -> bool {
        self.state.borrow().execution.is_active()
    }

    pub fn start(&self, query: QueryId) -> bool {
        self.state.send_if_modified(|state| {
            if state.lifecycle != DocumentLifecycle::Open || state.execution.is_active() {
                return false;
            }
            state.execution = DocumentExecution::Running(query);
            state.plan = None;
            state.operation = None;
            state.completion = None;
            true
        })
    }

    pub fn finish(&self, query: QueryId) -> bool {
        self.state.send_if_modified(|state| {
            if !state.execution.is_active() || state.execution.query() != Some(query) {
                return false;
            }
            state.execution = DocumentExecution::Finished(query);
            true
        })
    }

    pub fn cancel(&self) -> Option<QueryId> {
        let mut query = None;
        self.state.send_if_modified(|state| {
            let (DocumentExecution::Running(id) | DocumentExecution::Planning(id)) =
                state.execution
            else {
                return false;
            };
            state.execution = DocumentExecution::Canceling(id);
            query = Some(id);
            true
        });
        query
    }

    pub fn cancel_failed(&self, query: QueryId) -> bool {
        self.state.send_if_modified(|state| {
            if state.execution != DocumentExecution::Canceling(query) {
                return false;
            }
            state.execution = match state.operation.as_ref().map(|operation| &operation.phase) {
                Some(
                    crate::execution::QueryPhase::Queued(_)
                    | crate::execution::QueryPhase::Explaining(_),
                ) => DocumentExecution::Planning(query),
                _ => DocumentExecution::Running(query),
            };
            true
        })
    }

    pub fn replace_output(&self) -> u64 {
        let mut generation = 0;
        self.state.send_modify(|state| {
            state.output_generation = state.output_generation.checked_add(1).unwrap();
            generation = state.output_generation;
        });
        generation
    }

    pub fn accepts_output(&self, generation: u64) -> bool {
        let state = self.state.borrow();
        state.lifecycle == DocumentLifecycle::Open && state.output_generation == generation
    }

    pub fn set_execution_plan(&self, query: QueryId, plan: Option<QueryPlan>) -> bool {
        self.state.send_if_modified(|state| {
            if state.lifecycle != DocumentLifecycle::Open || state.execution.query() != Some(query)
            {
                return false;
            }
            state.plan = plan;
            true
        })
    }

    pub fn plan(&self) -> Option<QueryPlan> {
        self.state.borrow().plan.clone()
    }

    pub(crate) fn mutate(&self, mutation: impl FnOnce(&mut DocumentSnapshot) -> bool) -> bool {
        self.state.send_if_modified(mutation)
    }

    fn close(&self) -> bool {
        self.state.send_if_modified(|state| {
            if state.execution.is_active() {
                return false;
            }
            state.lifecycle = DocumentLifecycle::Closed;
            state.plan = None;
            state.operation = None;
            state.completion = None;
            true
        })
    }
}

#[derive(Clone, Debug)]
pub struct QueryExecution {
    document: QueryDocument,
    pub query: QueryId,
}

impl QueryExecution {
    pub fn start(document: QueryDocument, query: QueryId) -> Option<Self> {
        document.start(query).then_some(Self { document, query })
    }

    pub fn query_num(&self) -> u32 {
        self.document.number()
    }

    pub fn finish(&self) -> bool {
        self.document.finish(self.query)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseDocument {
    Missing,
    Busy,
    CloseWorkspace,
    Closed { last: bool },
}

#[derive(Default, Debug)]
pub struct QueryDocuments {
    documents: Vec<QueryDocument>,
    selected: Option<DocumentId>,
    counter: u32,
}

impl QueryDocuments {
    pub fn close_in_workspace(&mut self, id: DocumentId) -> CloseDocument {
        let Some(document) = self.find(id) else {
            return CloseDocument::Missing;
        };
        if document.is_active() {
            return CloseDocument::Busy;
        }
        if self.documents.len() == 1 {
            return CloseDocument::CloseWorkspace;
        }
        self.close(id)
    }

    pub fn retire_closed_session(
        &mut self,
        engine: &crate::execution::QueryEngine,
    ) -> Result<(), crate::error::ExecutionError> {
        if *engine.subscribe_session().borrow() != crate::execution::SessionPhase::Closed {
            return Err(crate::error::ExecutionError::Busy);
        }
        if self.documents.iter().any(|document| {
            document
                .snapshot()
                .session
                .is_some_and(|id| id != engine.id())
        }) {
            return Err(crate::error::ExecutionError::Message(
                "Workspace belongs to another session".into(),
            ));
        }
        for document in &self.documents {
            document.mutate(|state| {
                state.lifecycle = DocumentLifecycle::Closed;
                state.execution = DocumentExecution::Idle;
                state.plan = None;
                state.operation = None;
                state.completion = None;
                true
            });
        }
        self.documents.clear();
        self.selected = None;
        Ok(())
    }

    pub fn open(&mut self) -> QueryDocument {
        self.counter = self.counter.checked_add(1).unwrap();
        let document = QueryDocument::new(self.counter);
        self.selected = Some(document.id());
        self.documents.push(document.clone());
        document
    }

    pub fn documents(&self) -> &[QueryDocument] {
        &self.documents
    }

    pub fn selected(&self) -> Option<QueryDocument> {
        self.selected.and_then(|id| self.find(id))
    }

    pub fn find(&self, id: DocumentId) -> Option<QueryDocument> {
        self.documents.iter().find(|doc| doc.id() == id).cloned()
    }

    pub fn select(&mut self, id: DocumentId) -> bool {
        if self.find(id).is_none() {
            return false;
        }
        self.selected = Some(id);
        true
    }

    pub fn select_relative(&mut self, forward: bool) -> Option<QueryDocument> {
        if self.documents.len() < 2 {
            return None;
        }
        let index = self
            .documents
            .iter()
            .position(|doc| Some(doc.id()) == self.selected)?;
        let next = if forward {
            (index + 1) % self.documents.len()
        } else {
            (index + self.documents.len() - 1) % self.documents.len()
        };
        self.selected = Some(self.documents[next].id());
        self.selected()
    }

    pub fn reorder(&mut self, id: DocumentId, position: usize) -> bool {
        if position >= self.documents.len() {
            return false;
        }
        let Some(index) = self.documents.iter().position(|doc| doc.id() == id) else {
            return false;
        };
        let document = self.documents.remove(index);
        self.documents.insert(position, document);
        true
    }

    pub fn close(&mut self, id: DocumentId) -> CloseDocument {
        let Some(index) = self.documents.iter().position(|doc| doc.id() == id) else {
            return CloseDocument::Missing;
        };
        if !self.documents[index].close() {
            return CloseDocument::Busy;
        }
        self.documents.remove(index);
        if self.selected == Some(id) {
            self.selected = self
                .documents
                .get(index)
                .or_else(|| self.documents.last())
                .map(QueryDocument::id);
        }
        CloseDocument::Closed {
            last: self.documents.is_empty(),
        }
    }
}
