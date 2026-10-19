use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

use crate::documents::{DocumentExecution, DocumentLifecycle, DocumentSnapshot};
use crate::error::{ErrorKind, ExecutionError};
use crate::result_stream::{ResultPhase, ResultTermination};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewedOutput {
    pub tab: Uuid,
    pub document: Uuid,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionIdentity {
    pub output: ViewedOutput,
    pub session: Uuid,
    pub query: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionOutcome {
    Success,
    Failure,
    Canceled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnseenCompletion {
    pub identity: CompletionIdentity,
    pub outcome: CompletionOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabAttention {
    pub tab: Uuid,
    pub document: Uuid,
    pub completion_revision: u64,
    pub unseen: Option<UnseenCompletion>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionSnapshot {
    pub revision: u64,
    pub application_visible: bool,
    pub request_revision: u64,
    pub cancel_attention: bool,
    pub tabs: Vec<TabAttention>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttentionInput {
    Visibility {
        visible: bool,
        viewed: Option<ViewedOutput>,
    },
    Viewed {
        output: Option<ViewedOutput>,
    },
    Lifecycle {
        restoring: bool,
        quitting: bool,
    },
}

struct Entry {
    snapshot: TabAttention,
    active: Option<CompletionIdentity>,
    processed: Option<CompletionIdentity>,
    closing: bool,
}

pub struct AttentionPolicy {
    tabs: BTreeMap<Uuid, Entry>,
    visible: bool,
    viewed: Option<ViewedOutput>,
    requested_while_away: bool,
    request_revision: u64,
    revision: u64,
    suspended: bool,
    host_suppressed: bool,
}

impl Default for AttentionPolicy {
    fn default() -> Self {
        Self {
            tabs: BTreeMap::new(),
            visible: true,
            viewed: None,
            requested_while_away: false,
            request_revision: 0,
            revision: 0,
            suspended: false,
            host_suppressed: false,
        }
    }
}

impl AttentionPolicy {
    pub fn snapshot(&self) -> AttentionSnapshot {
        AttentionSnapshot {
            revision: self.revision,
            application_visible: self.visible,
            request_revision: self.request_revision,
            cancel_attention: self.visible
                || self.suspended
                || self.host_suppressed
                || self
                    .tabs
                    .values()
                    .all(|entry| entry.snapshot.unseen.is_none()),
            tabs: self
                .tabs
                .values()
                .map(|entry| entry.snapshot.clone())
                .collect(),
        }
    }

    pub fn register(&mut self, tab: Uuid, document: Uuid) {
        self.tabs.entry(tab).or_insert_with(|| Entry {
            snapshot: TabAttention {
                tab,
                document,
                completion_revision: 0,
                unseen: None,
            },
            active: None,
            processed: None,
            closing: false,
        });
    }

    pub fn started(&mut self, identity: CompletionIdentity) {
        if let Some(entry) = self.tabs.get_mut(&identity.output.tab)
            && entry.snapshot.document == identity.output.document
            && entry
                .active
                .is_none_or(|active| identity.output.generation > active.output.generation)
        {
            entry.active = Some(identity);
        }
    }

    pub fn completed(&mut self, identity: CompletionIdentity, outcome: CompletionOutcome) {
        let Some(entry) = self.tabs.get_mut(&identity.output.tab) else {
            return;
        };
        if entry.active != Some(identity) || entry.processed == Some(identity) {
            return;
        }
        entry.processed = Some(identity);
        if self.suspended
            || self.host_suppressed
            || entry.closing
            || outcome == CompletionOutcome::Canceled
        {
            return;
        }
        if self.visible
            && self.viewed.is_some_and(|viewed| {
                viewed.tab == identity.output.tab && viewed.document == identity.output.document
            })
        {
            entry.snapshot.unseen = None;
            return;
        }
        entry.snapshot.completion_revision =
            entry.snapshot.completion_revision.checked_add(1).unwrap();
        entry.snapshot.unseen = Some(UnseenCompletion { identity, outcome });
        if !self.visible && !self.requested_while_away {
            self.requested_while_away = true;
            self.request_revision = self.request_revision.checked_add(1).unwrap();
        }
    }

    pub fn input(&mut self, input: AttentionInput) {
        match input {
            AttentionInput::Visibility { visible, viewed } => {
                self.visible = visible;
                self.viewed = viewed;
                if visible {
                    self.requested_while_away = false;
                    if let Some(viewed) = viewed {
                        self.acknowledge(viewed);
                    }
                }
            }
            AttentionInput::Viewed { output } => {
                self.viewed = output;
                if self.visible
                    && let Some(output) = output
                {
                    self.acknowledge(output);
                }
            }
            AttentionInput::Lifecycle {
                restoring,
                quitting,
            } => {
                self.host_suppressed = restoring || quitting;
            }
        }
    }

    fn acknowledge(&mut self, output: ViewedOutput) {
        if let Some(entry) = self.tabs.get_mut(&output.tab)
            && entry.snapshot.unseen.as_ref().is_some_and(|unseen| {
                unseen.identity.output.tab == output.tab
                    && unseen.identity.output.document == output.document
                    && unseen.identity.output.generation <= output.generation
            })
        {
            entry.snapshot.unseen = None;
        }
    }

    pub fn closing(&mut self, tab: Uuid, closing: bool) {
        if let Some(entry) = self.tabs.get_mut(&tab) {
            entry.closing = closing;
        }
    }
    pub fn suspend(&mut self, suspended: bool) {
        self.suspended = suspended;
    }
    pub fn retire(&mut self, tab: Uuid) {
        self.tabs.remove(&tab);
    }
    pub fn clear(&mut self) {
        self.tabs.clear();
        self.requested_while_away = false;
        self.suspended = false;
    }
}

pub struct AttentionService {
    policy: Mutex<AttentionPolicy>,
    snapshots: watch::Sender<AttentionSnapshot>,
}

impl Default for AttentionService {
    fn default() -> Self {
        let policy = AttentionPolicy::default();
        Self {
            snapshots: watch::channel(policy.snapshot()).0,
            policy: Mutex::new(policy),
        }
    }
}

impl AttentionService {
    fn change(&self, change: impl FnOnce(&mut AttentionPolicy)) -> AttentionSnapshot {
        let mut policy = self.policy.lock().unwrap();
        let previous = policy.snapshot();
        change(&mut policy);
        if policy.snapshot() != previous {
            policy.revision = policy.revision.checked_add(1).unwrap();
            self.snapshots.send_replace(policy.snapshot());
        }
        policy.snapshot()
    }
    pub fn snapshot(&self) -> AttentionSnapshot {
        self.policy.lock().unwrap().snapshot()
    }
    pub fn subscribe(&self) -> watch::Receiver<AttentionSnapshot> {
        self.snapshots.subscribe()
    }
    pub fn input(&self, input: AttentionInput) -> AttentionSnapshot {
        self.change(|policy| policy.input(input))
    }
    pub(crate) fn register(&self, tab: Uuid, document: Uuid) {
        self.change(|policy| policy.register(tab, document));
    }
    pub(crate) fn started(&self, identity: CompletionIdentity) {
        self.change(|policy| policy.started(identity));
    }
    pub(crate) fn closing(&self, tab: Uuid, closing: bool) {
        self.change(|policy| policy.closing(tab, closing));
    }
    pub(crate) fn suspend(&self, suspended: bool) {
        self.change(|policy| policy.suspend(suspended));
    }
    pub(crate) fn retire(&self, tab: Uuid) {
        self.change(|policy| policy.retire(tab));
    }
    pub(crate) fn clear(&self) {
        self.change(AttentionPolicy::clear);
    }

    pub(crate) fn document_finished(
        &self,
        identity: CompletionIdentity,
        snapshot: &DocumentSnapshot,
    ) {
        if snapshot.lifecycle != DocumentLifecycle::Open
            || snapshot.execution
                != DocumentExecution::Finished(crate::execution::QueryId(identity.query))
            || snapshot.output_generation != identity.output.generation
            || snapshot.execution_generation != identity.output.generation
        {
            return;
        }
        let error = snapshot
            .completion
            .as_ref()
            .and_then(|completion| match completion {
                Err(error) => Some(error),
                Ok(completion) => completion.result.as_ref().err(),
            });
        let capture = snapshot
            .operation
            .as_ref()
            .and_then(|operation| operation.capture.as_ref());
        let canceled = error.is_some_and(|error| {
            matches!(
                error,
                ExecutionError::Canceled
                    | ExecutionError::Database(crate::error::DatabaseError {
                        kind: ErrorKind::Canceled,
                        ..
                    })
            )
        }) || capture.is_some_and(|capture| {
            matches!(
                capture.phase,
                ResultPhase::Finished(ResultTermination::Canceled)
            )
        });
        let failed = error.is_some()
            || capture.is_some_and(|capture| {
                matches!(
                    capture.phase,
                    ResultPhase::Finished(ResultTermination::Failed(_))
                )
            });
        let outcome = if canceled {
            CompletionOutcome::Canceled
        } else if failed {
            CompletionOutcome::Failure
        } else {
            CompletionOutcome::Success
        };
        self.change(|policy| policy.completed(identity, outcome));
    }
}
