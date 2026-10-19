use crate::documents::{DocumentExecution, DocumentSnapshot};
use crate::error::ExecutionError;
use crate::result_store::ResultCutoff;
use crate::result_stream::{ResultPhase, ResultTermination};
use crate::{CommandAcknowledgment, DatabaseBackend};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    Idle,
    Planning,
    Running,
    Canceling,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePhase {
    WaitingForSchema,
    Receiving,
    Draining,
    FailedAwaitingCleanup,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTermination {
    Complete,
    Limited,
    Canceled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanEstimate {
    pub backend: DatabaseBackend,
    pub estimate: super::EstimatePresentation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DocumentError {
    Execution(ExecutionError),
    Legacy(String),
}

impl From<&DocumentError> for super::DiagnosticPresentation {
    fn from(error: &DocumentError) -> Self {
        match error {
            DocumentError::Execution(error) => Self::from(error),
            DocumentError::Legacy(message) => Self::from(&ExecutionError::Message(message.clone())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentPresentation {
    pub document: Uuid,
    pub session: Option<Uuid>,
    pub query: Option<Uuid>,
    pub generation: u64,
    pub draft_revision: u64,
    pub title: String,
    pub draft: String,
    pub execution: ExecutionPhase,
    pub result_phase: Option<CapturePhase>,
    pub result_available: bool,
    pub rows: usize,
    pub received_rows: Option<usize>,
    pub capture_bytes: Option<usize>,
    pub termination: Option<CaptureTermination>,
    pub capture_error: Option<ExecutionError>,
    pub command_result: Option<CommandAcknowledgment>,
    pub started_at_seconds: Option<f64>,
    pub elapsed_seconds: Option<f64>,
    pub execution_plan: Option<PlanEstimate>,
    pub execution_seconds: Option<f64>,
    pub cutoff: Option<ResultCutoff>,
    pub error: Option<DocumentError>,
    pub feedback: super::ResultFeedback,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_executed_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_total_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_plan_available: Option<bool>,
}

impl From<&DocumentSnapshot> for DocumentPresentation {
    fn from(snapshot: &DocumentSnapshot) -> Self {
        let execution = match snapshot.execution {
            DocumentExecution::Idle => ExecutionPhase::Idle,
            DocumentExecution::Planning(_) => ExecutionPhase::Planning,
            DocumentExecution::Running(_) => ExecutionPhase::Running,
            DocumentExecution::Canceling(_) => ExecutionPhase::Canceling,
            DocumentExecution::Finished(_) => ExecutionPhase::Finished,
        };
        let capture = snapshot
            .operation
            .as_ref()
            .and_then(|operation| operation.capture.as_ref());
        let result_phase = capture.map(|capture| match capture.phase {
            ResultPhase::WaitingForSchema => CapturePhase::WaitingForSchema,
            ResultPhase::Receiving => CapturePhase::Receiving,
            ResultPhase::Draining(_) => CapturePhase::Draining,
            ResultPhase::FailedAwaitingCleanup(_) => CapturePhase::FailedAwaitingCleanup,
            ResultPhase::Finished(_) => CapturePhase::Finished,
        });
        let termination = capture.and_then(|capture| match &capture.phase {
            ResultPhase::Finished(ResultTermination::Complete) => {
                Some(CaptureTermination::Complete)
            }
            ResultPhase::Finished(ResultTermination::Limited(_)) => {
                Some(CaptureTermination::Limited)
            }
            ResultPhase::Finished(ResultTermination::Canceled) => {
                Some(CaptureTermination::Canceled)
            }
            ResultPhase::Finished(ResultTermination::Failed(_)) => Some(CaptureTermination::Failed),
            _ => None,
        });
        let capture_error = capture.and_then(|capture| match &capture.phase {
            ResultPhase::FailedAwaitingCleanup(error)
            | ResultPhase::Finished(ResultTermination::Failed(error)) => Some(error.clone()),
            _ => None,
        });
        let completion = snapshot
            .completion
            .as_ref()
            .and_then(|completion| completion.as_ref().ok());
        let result = completion.and_then(|completion| completion.result.as_ref().ok());
        let error = snapshot
            .completion
            .as_ref()
            .and_then(|completion| match completion {
                Err(error) => Some(error.clone()),
                Ok(completion) => completion.result.as_ref().err().cloned(),
            });
        let mut result = Self {
            document: snapshot.id.0,
            session: snapshot.session.map(|session| session.0),
            query: snapshot.execution.query().map(|query| query.0),
            generation: snapshot.output_generation,
            draft_revision: snapshot.draft_revision,
            title: snapshot.title.clone(),
            draft: snapshot.draft.clone(),
            execution,
            result_phase,
            result_available: capture.is_some_and(|capture| capture.result.is_some()),
            rows: capture.map_or(0, |capture| capture.rows),
            received_rows: Some(capture.map_or(0, |capture| capture.received_rows)),
            capture_bytes: Some(capture.map_or(0, |capture| capture.bytes)),
            termination,
            capture_error,
            command_result: result.and_then(|result| result.command.clone()),
            started_at_seconds: snapshot
                .operation
                .as_ref()
                .and_then(|operation| operation.started_wall_time)
                .and_then(|start| start.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs_f64()),
            elapsed_seconds: snapshot
                .operation
                .as_ref()
                .map(|operation| operation.elapsed().as_secs_f64()),
            execution_plan: snapshot.plan.as_ref().map(|plan| PlanEstimate {
                backend: plan.backend,
                estimate: super::EstimatePresentation::from(&plan.estimate),
            }),
            execution_seconds: result.map(|result| result.duration.as_secs_f64()),
            cutoff: capture.and_then(|capture| capture.cutoff),
            error: error.map(DocumentError::Execution),
            history: None,
            history_executed_at: None,
            history_total_rows: None,
            history_plan_available: None,
            feedback: super::ResultFeedback::default(),
        };
        result.feedback = super::ResultFeedback::live(&result);
        result
    }
}
