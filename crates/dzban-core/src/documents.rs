use std::sync::{Arc, Mutex};

use crate::QueryPlan;
use crate::execution::QueryId;

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
    Running(QueryId),
    Canceling(QueryId),
    Finished(QueryId),
}

impl DocumentExecution {
    pub fn query(&self) -> Option<QueryId> {
        match self {
            Self::Idle => None,
            Self::Running(query) | Self::Canceling(query) | Self::Finished(query) => Some(*query),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running(_) | Self::Canceling(_))
    }
}

#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    pub id: DocumentId,
    pub number: u32,
    pub lifecycle: DocumentLifecycle,
    pub title: String,
    pub draft: String,
    pub execution: DocumentExecution,
    pub output_generation: u64,
    pub plan: Option<QueryPlan>,
}

#[derive(Clone, Debug)]
pub struct QueryDocument {
    state: Arc<Mutex<DocumentSnapshot>>,
}

impl QueryDocument {
    fn new(number: u32) -> Self {
        Self {
            state: Arc::new(Mutex::new(DocumentSnapshot {
                id: DocumentId(uuid::Uuid::new_v4()),
                number,
                lifecycle: DocumentLifecycle::Open,
                title: format!("Query {number}"),
                draft: String::new(),
                execution: DocumentExecution::Idle,
                output_generation: 0,
                plan: None,
            })),
        }
    }

    pub fn snapshot(&self) -> DocumentSnapshot {
        self.state.lock().unwrap().clone()
    }

    pub fn id(&self) -> DocumentId {
        self.state.lock().unwrap().id
    }

    pub fn number(&self) -> u32 {
        self.state.lock().unwrap().number
    }

    pub fn rename(&self, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        let mut state = self.state.lock().unwrap();
        if state.lifecycle != DocumentLifecycle::Open {
            return false;
        }
        state.title = title.to_string();
        true
    }

    pub fn set_draft(&self, draft: String) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.lifecycle != DocumentLifecycle::Open {
            return false;
        }
        state.draft = draft;
        true
    }

    pub fn draft(&self) -> String {
        self.state.lock().unwrap().draft.clone()
    }

    pub fn is_active(&self) -> bool {
        self.state.lock().unwrap().execution.is_active()
    }

    pub fn start(&self, query: QueryId) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.lifecycle != DocumentLifecycle::Open || state.execution.is_active() {
            return false;
        }
        state.execution = DocumentExecution::Running(query);
        state.plan = None;
        true
    }

    pub fn finish(&self, query: QueryId) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.execution.is_active() || state.execution.query() != Some(query) {
            return false;
        }
        state.execution = DocumentExecution::Finished(query);
        true
    }

    pub fn cancel(&self) -> Option<QueryId> {
        let mut state = self.state.lock().unwrap();
        let DocumentExecution::Running(query) = state.execution else {
            return None;
        };
        state.execution = DocumentExecution::Canceling(query);
        Some(query)
    }

    pub fn cancel_failed(&self, query: QueryId) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.execution != DocumentExecution::Canceling(query) {
            return false;
        }
        state.execution = DocumentExecution::Running(query);
        true
    }

    pub fn replace_output(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.output_generation = state.output_generation.checked_add(1).unwrap();
        state.output_generation
    }

    pub fn accepts_output(&self, generation: u64) -> bool {
        let state = self.state.lock().unwrap();
        state.lifecycle == DocumentLifecycle::Open && state.output_generation == generation
    }

    pub fn set_execution_plan(&self, query: QueryId, plan: Option<QueryPlan>) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.lifecycle != DocumentLifecycle::Open || state.execution.query() != Some(query) {
            return false;
        }
        state.plan = plan;
        true
    }

    pub fn plan(&self) -> Option<QueryPlan> {
        self.state.lock().unwrap().plan.clone()
    }

    fn close(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.execution.is_active() {
            return false;
        }
        state.lifecycle = DocumentLifecycle::Closed;
        state.plan = None;
        true
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
    Closed { last: bool },
}

#[derive(Default, Debug)]
pub struct QueryDocuments {
    documents: Vec<QueryDocument>,
    selected: Option<DocumentId>,
    counter: u32,
}

impl QueryDocuments {
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
