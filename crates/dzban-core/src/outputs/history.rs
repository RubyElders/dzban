use std::sync::Arc;

use crate::history::{
    QueryHistoryEntry, QueryHistoryOutcome, QueryHistoryStore, QueryResultSnapshot,
};
use crate::presentation::{
    CapturePhase, CaptureTermination, DiagnosticPresentation, DocumentError, DocumentPresentation,
    HistoryEntryPresentation, HistoryOutcomeKind, HistoryPresentation, ResultFeedback,
};
use crate::{ResultCell, ResultColumn, ResultSet};
use serde_json::Value;
use uuid::Uuid;

pub(super) struct HistoryView {
    pub revision: Uuid,
    pub store: Arc<QueryHistoryStore>,
}

#[derive(Clone)]
pub(super) struct SavedOutput {
    pub generation: u64,
    pub store: Arc<QueryHistoryStore>,
    pub index: usize,
}

fn preview(value: &str) -> String {
    value.chars().take(256).collect()
}

pub(super) fn summaries(view: &HistoryView) -> HistoryPresentation {
    let entries = view
        .store
        .entries()
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let (status, rows, duration_ms) = match &entry.outcome {
                QueryHistoryOutcome::Success {
                    rows, duration_ms, ..
                } => (HistoryOutcomeKind::Success, Some(*rows), Some(*duration_ms)),
                QueryHistoryOutcome::Failure { .. } => (HistoryOutcomeKind::Failure, None, None),
            };
            HistoryEntryPresentation {
                index,
                sql_preview: preview(&entry.sql),
                executed_at: entry.executed_at,
                status,
                rows,
                duration_ms,
            }
        })
        .collect();
    HistoryPresentation {
        revision: view.revision,
        entries,
    }
}

impl SavedOutput {
    pub(super) fn entry(&self) -> &QueryHistoryEntry {
        &self.store.entries()[self.index]
    }
    fn result(&self) -> Option<&QueryResultSnapshot> {
        match &self.entry().outcome {
            QueryHistoryOutcome::Success { result, .. } => result.as_ref(),
            _ => None,
        }
    }
    pub(super) fn page(&self, start: usize, count: usize) -> ResultSet {
        let Some(result) = self.result() else {
            return ResultSet::default();
        };
        ResultSet {
            columns: result
                .columns
                .iter()
                .map(|name| ResultColumn {
                    name: name.clone(),
                    type_name: "saved text".into(),
                })
                .collect(),
            rows: result
                .rows
                .iter()
                .skip(start)
                .take(count)
                .map(|row| row.iter().cloned().map(ResultCell::Text).collect())
                .collect(),
        }
    }
    pub(super) fn plan(&self) -> Option<&Value> {
        match &self.entry().outcome {
            QueryHistoryOutcome::Success { explain, .. }
            | QueryHistoryOutcome::Failure { explain, .. } => explain.as_ref(),
        }
    }
    pub(super) fn project(&self, mut snapshot: DocumentPresentation) -> DocumentPresentation {
        let progress = snapshot.feedback.progress;
        snapshot.history = Some(true);
        snapshot.history_executed_at = Some(self.entry().executed_at);
        snapshot.history_plan_available = Some(self.plan().is_some());
        snapshot.rows = self.result().map_or(0, |result| result.rows.len());
        snapshot.result_available = self.result().is_some();
        snapshot.result_phase = Some(CapturePhase::Finished);
        snapshot.capture_error = None;
        snapshot.received_rows = None;
        snapshot.capture_bytes = None;
        match &self.entry().outcome {
            QueryHistoryOutcome::Success {
                rows,
                cutoff,
                command,
                duration_ms,
                ..
            } => {
                snapshot.execution_seconds = Some(*duration_ms as f64 / 1000.0);
                snapshot.history_total_rows = Some(*rows);
                snapshot.cutoff = *cutoff;
                snapshot.command_result = command.clone();
                snapshot.termination = Some(CaptureTermination::Complete);
                snapshot.error = None;
            }
            QueryHistoryOutcome::Failure {
                error, diagnostic, ..
            } => {
                snapshot.execution_seconds = None;
                snapshot.history_total_rows = Some(0);
                snapshot.cutoff = None;
                snapshot.command_result = None;
                snapshot.termination = Some(CaptureTermination::Failed);
                snapshot.error = Some(diagnostic.as_ref().map_or_else(
                    || DocumentError::Legacy(error.clone()),
                    |error| DocumentError::Execution(error.clone()),
                ));
            }
        }
        snapshot.feedback = ResultFeedback::saved(
            snapshot.rows,
            snapshot.history_total_rows.unwrap_or(0),
            snapshot.command_result.as_ref(),
            snapshot.error.as_ref().map(DiagnosticPresentation::from),
        );
        snapshot.feedback.progress = progress;
        snapshot
    }
}
