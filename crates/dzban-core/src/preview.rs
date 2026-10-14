use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::QueryPlan;
use crate::documents::{DocumentId, DocumentLifecycle, QueryDocument};
use crate::error::ExecutionError;
use crate::execution::{QueryEngine, SessionId};

pub const PREVIEW_DELAY: Duration = Duration::from_millis(250);
pub const PREVIEW_HOLD: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewRelease {
    Pin,
    Hide,
}

pub fn preview_release(elapsed: Duration) -> PreviewRelease {
    if elapsed < PREVIEW_HOLD {
        PreviewRelease::Pin
    } else {
        PreviewRelease::Hide
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeekState {
    visibility: crate::interaction::PlanVisibility,
}

impl PeekState {
    pub fn show_transient(&mut self) {
        if self.visibility == crate::interaction::PlanVisibility::Hidden {
            self.visibility = crate::interaction::PlanVisibility::Transient;
        }
    }
    pub fn hide_transient(&mut self) {
        if self.visibility == crate::interaction::PlanVisibility::Transient {
            self.visibility = crate::interaction::PlanVisibility::Hidden;
        }
    }
    pub fn pin(&mut self) -> bool {
        if !self.is_visible() {
            return false;
        }
        self.visibility = crate::interaction::PlanVisibility::Pinned;
        true
    }
    pub fn close(&mut self) -> bool {
        if !self.is_visible() {
            return false;
        }
        self.visibility = crate::interaction::PlanVisibility::Hidden;
        true
    }
    pub fn is_visible(&self) -> bool {
        self.visibility != crate::interaction::PlanVisibility::Hidden
    }
    pub fn is_pinned(&self) -> bool {
        self.visibility == crate::interaction::PlanVisibility::Pinned
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewRequestId(pub uuid::Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewTarget {
    pub request: PreviewRequestId,
    pub session: SessionId,
    pub document: DocumentId,
    pub revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum PreviewPhase {
    #[default]
    Empty,
    Scheduled,
    Loading,
    Ready(QueryPlan),
    Failed(ExecutionError),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewSnapshot {
    pub target: Option<PreviewTarget>,
    pub phase: PreviewPhase,
}

impl PreviewSnapshot {
    pub fn is_current(&self, target: PreviewTarget) -> bool {
        self.target == Some(target) && self.phase != PreviewPhase::Empty
    }
}

#[derive(Clone, Debug)]
pub struct PlanPreviewService {
    state: Arc<watch::Sender<PreviewSnapshot>>,
}

impl Default for PlanPreviewService {
    fn default() -> Self {
        Self {
            state: Arc::new(watch::channel(PreviewSnapshot::default()).0),
        }
    }
}

impl PlanPreviewService {
    pub fn subscribe(&self) -> watch::Receiver<PreviewSnapshot> {
        self.state.subscribe()
    }
    pub fn snapshot(&self) -> PreviewSnapshot {
        self.state.borrow().clone()
    }
    pub fn accepts(&self, snapshot: &PreviewSnapshot, document: &QueryDocument) -> bool {
        let Some(target) = snapshot.target else {
            return false;
        };
        let source = document.snapshot();
        self.state.borrow().is_current(target)
            && source.id == target.document
            && source.draft_revision == target.revision
            && source.lifecycle == DocumentLifecycle::Open
            && !source.execution.is_active()
    }
    pub fn cancel(&self) {
        self.state.send_if_modified(|state| {
            if state.phase == PreviewPhase::Empty {
                return false;
            }
            state.phase = PreviewPhase::Empty;
            true
        });
    }

    pub fn workspace_changed(&self, workspace: &crate::documents::QueryDocuments) {
        let target = self.state.borrow().target;
        if target.is_some_and(|target| workspace.find(target.document).is_some()) {
            self.cancel();
        }
    }

    fn cancel_request(&self, target: PreviewTarget) {
        self.state.send_if_modified(|state| {
            if state.target != Some(target) {
                return false;
            }
            state.phase = PreviewPhase::Empty;
            true
        });
    }

    pub fn schedule(
        &self,
        engine: Arc<QueryEngine>,
        document: QueryDocument,
        sql: String,
        delay: Duration,
    ) -> Result<PreviewTarget, ExecutionError> {
        let source = document.snapshot();
        if source.lifecycle != DocumentLifecycle::Open {
            return Err(ExecutionError::Closed);
        }
        if source.execution.is_active() {
            return Err(ExecutionError::Busy);
        }
        if sql.trim().is_empty() {
            return Err(ExecutionError::Message("No query to explain".into()));
        }
        let target = PreviewTarget {
            request: PreviewRequestId(uuid::Uuid::new_v4()),
            session: engine.id(),
            document: source.id,
            revision: source.draft_revision,
        };
        self.state.send_replace(PreviewSnapshot {
            target: Some(target),
            phase: PreviewPhase::Scheduled,
        });
        let weak = Arc::downgrade(&self.state);
        let mut lifecycle_requests = self.subscribe();
        let mut lifecycle_documents = document.subscribe();
        tokio::spawn(async move {
            loop {
                if !lifecycle_requests.borrow_and_update().is_current(target) {
                    return;
                }
                let source = lifecycle_documents.borrow_and_update().clone();
                if source.lifecycle != DocumentLifecycle::Open
                    || source.draft_revision != target.revision
                    || source.execution.is_active()
                {
                    if let Some(state) = weak.upgrade() {
                        state.send_if_modified(|state| {
                            if state.target != Some(target) {
                                return false;
                            }
                            state.phase = PreviewPhase::Empty;
                            true
                        });
                    }
                    return;
                }
                tokio::select! {
                    changed = lifecycle_requests.changed() => if changed.is_err() { return; },
                    changed = lifecycle_documents.changed() => {
                        if changed.is_err() {
                            if let Some(state) = weak.upgrade() {
                                state.send_if_modified(|state| {
                                    if state.target != Some(target) { return false; }
                                    state.phase = PreviewPhase::Empty;
                                    true
                                });
                            }
                            return;
                        }
                    },
                }
            }
        });
        let service = self.clone();
        let mut requests = self.subscribe();
        let mut documents = document.subscribe();
        tokio::spawn(async move {
            let deadline = tokio::time::sleep(delay);
            tokio::pin!(deadline);
            loop {
                let source = documents.borrow_and_update().clone();
                if !requests.borrow_and_update().is_current(target) {
                    return;
                }
                if source.lifecycle != DocumentLifecycle::Open
                    || source.draft_revision != target.revision
                    || source.execution.is_active()
                {
                    service.cancel_request(target);
                    return;
                }
                tokio::select! {
                    _ = &mut deadline => break,
                    changed = requests.changed() => if changed.is_err() { return; },
                    changed = documents.changed() => if changed.is_err() { return; },
                }
            }
            let request = match engine.start_preview(sql) {
                Ok(request) => request,
                Err(error) => {
                    service.state.send_if_modified(|state| {
                        if !state.is_current(target) {
                            return false;
                        }
                        state.phase = PreviewPhase::Failed(error);
                        true
                    });
                    return;
                }
            };
            service.state.send_if_modified(|state| {
                if !state.is_current(target) {
                    return false;
                }
                state.phase = PreviewPhase::Loading;
                true
            });
            let result = request.wait();
            tokio::pin!(result);
            loop {
                let source = documents.borrow_and_update().clone();
                if !requests.borrow_and_update().is_current(target) {
                    return;
                }
                if source.lifecycle != DocumentLifecycle::Open
                    || source.draft_revision != target.revision
                    || source.execution.is_active()
                {
                    service.cancel_request(target);
                    return;
                }
                tokio::select! {
                    result = &mut result => {
                        service.state.send_if_modified(|state| {
                            if !state.is_current(target) { return false; }
                            state.phase = match result {
                                Ok(plan) => PreviewPhase::Ready(plan),
                                Err(error) => PreviewPhase::Failed(error),
                            };
                            true
                        });
                        return;
                    }
                    changed = requests.changed() => if changed.is_err() { return; },
                    changed = documents.changed() => if changed.is_err() { return; },
                }
            }
        });
        Ok(target)
    }
}
