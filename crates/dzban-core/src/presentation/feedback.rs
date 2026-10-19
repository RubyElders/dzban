use super::{
    CapturePhase, CaptureTermination, DiagnosticPresentation, DocumentPresentation, ExecutionPhase,
};
use crate::CommandAcknowledgment;
use crate::result_store::ResultCutoff;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatusReason {
    #[default]
    Ready,
    Planning,
    Waiting,
    WaitingForSchema,
    Receiving,
    Draining,
    CaptureFailedAwaitingCleanup,
    Finishing,
    Canceling,
    LimitedByRows,
    LimitedByBytes,
    Canceled,
    Failed,
    Command,
    Complete,
    SavedFailed,
    SavedCommand,
    SavedRows,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmptyResultReason {
    #[default]
    Ready,
    Command,
    Canceled,
    NoRows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultProgress {
    Planning,
    Running,
    Canceling,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultFeedback {
    pub status: ResultStatusReason,
    pub empty: EmptyResultReason,
    pub progress: Option<ResultProgress>,
    pub rows: usize,
    pub total_rows: Option<usize>,
    pub affected_rows: Option<u64>,
    pub query_diagnostic: Option<DiagnosticPresentation>,
    pub capture_diagnostic: Option<DiagnosticPresentation>,
}

impl ResultFeedback {
    pub fn live(document: &DocumentPresentation) -> Self {
        use ResultStatusReason as Status;
        let status = if document.generation == 0 {
            Status::Ready
        } else if document.execution == ExecutionPhase::Canceling {
            Status::Canceling
        } else {
            match document.result_phase {
                Some(CapturePhase::WaitingForSchema) => Status::WaitingForSchema,
                Some(CapturePhase::Receiving) => Status::Receiving,
                Some(CapturePhase::Draining) => Status::Draining,
                Some(CapturePhase::FailedAwaitingCleanup) => Status::CaptureFailedAwaitingCleanup,
                Some(CapturePhase::Finished) => {
                    if document.execution != ExecutionPhase::Finished {
                        Status::Finishing
                    } else if document.termination == Some(CaptureTermination::Limited)
                        || document.cutoff.is_some()
                    {
                        if document.cutoff == Some(ResultCutoff::Bytes) {
                            Status::LimitedByBytes
                        } else {
                            Status::LimitedByRows
                        }
                    } else if document.termination == Some(CaptureTermination::Canceled) {
                        Status::Canceled
                    } else if document.termination == Some(CaptureTermination::Failed) {
                        Status::Failed
                    } else if document.command_result.is_some() {
                        Status::Command
                    } else {
                        Status::Complete
                    }
                }
                _ if document.execution == ExecutionPhase::Planning => Status::Planning,
                _ => Status::Waiting,
            }
        };
        Self {
            status,
            empty: if document.generation == 0 {
                EmptyResultReason::Ready
            } else if document.command_result.is_some() {
                EmptyResultReason::Command
            } else if document.termination == Some(CaptureTermination::Canceled) {
                EmptyResultReason::Canceled
            } else {
                EmptyResultReason::NoRows
            },
            progress: match document.execution {
                ExecutionPhase::Planning => Some(ResultProgress::Planning),
                ExecutionPhase::Running => Some(ResultProgress::Running),
                ExecutionPhase::Canceling => Some(ResultProgress::Canceling),
                _ => None,
            },
            rows: document.rows,
            total_rows: None,
            affected_rows: document
                .command_result
                .as_ref()
                .and_then(|command| command.affected_rows),
            query_diagnostic: document
                .error
                .as_ref()
                .filter(|_| document.termination != Some(CaptureTermination::Canceled))
                .map(DiagnosticPresentation::from),
            capture_diagnostic: document
                .capture_error
                .as_ref()
                .filter(|_| document.error.is_none())
                .map(DiagnosticPresentation::from),
        }
    }

    pub fn saved(
        rows: usize,
        total_rows: usize,
        command: Option<&CommandAcknowledgment>,
        error: Option<DiagnosticPresentation>,
    ) -> Self {
        Self {
            status: if error.is_some() {
                ResultStatusReason::SavedFailed
            } else if command.is_some() {
                ResultStatusReason::SavedCommand
            } else {
                ResultStatusReason::SavedRows
            },
            empty: if command.is_some() {
                EmptyResultReason::Command
            } else {
                EmptyResultReason::NoRows
            },
            rows,
            total_rows: Some(total_rows),
            affected_rows: command.and_then(|command| command.affected_rows),
            query_diagnostic: error,
            ..Default::default()
        }
    }
}
