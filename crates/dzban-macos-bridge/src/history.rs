use std::sync::Arc;

use dzban_core::history::{
    QueryHistoryEntry, QueryHistoryOutcome, QueryHistoryStore, QueryResultSnapshot,
};
use dzban_core::{ResultCell, ResultColumn, ResultSet};
use serde_json::{Value, json};
use uuid::Uuid;

pub struct HistoryView {
    pub revision: Uuid,
    pub store: Arc<QueryHistoryStore>,
}

#[derive(Clone)]
pub struct SavedOutput {
    pub generation: u64,
    pub store: Arc<QueryHistoryStore>,
    pub index: usize,
}

fn preview(value: &str) -> String {
    value.chars().take(256).collect()
}

pub fn summaries(view: &HistoryView) -> Value {
    let entries: Vec<_> = view.store.entries().iter().enumerate().map(|(index, entry)| {
        let (status, rows, duration) = match &entry.outcome {
            QueryHistoryOutcome::Success { rows, duration_ms, .. } => ("success", Some(*rows), Some(*duration_ms)),
            QueryHistoryOutcome::Failure { .. } => ("failure", None, None),
        };
        json!({"index":index,"sql_preview":preview(&entry.sql),"executed_at":entry.executed_at,"status":status,"rows":rows,"duration_ms":duration})
    }).collect();
    json!({"revision":view.revision,"entries":entries})
}

impl SavedOutput {
    pub fn entry(&self) -> &QueryHistoryEntry {
        &self.store.entries()[self.index]
    }
    fn result(&self) -> Option<&QueryResultSnapshot> {
        match &self.entry().outcome {
            QueryHistoryOutcome::Success { result, .. } => result.as_ref(),
            _ => None,
        }
    }
    pub fn page(&self, start: usize, count: usize) -> ResultSet {
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
    pub fn plan(&self) -> Option<&Value> {
        match &self.entry().outcome {
            QueryHistoryOutcome::Success { explain, .. }
            | QueryHistoryOutcome::Failure { explain, .. } => explain.as_ref(),
        }
    }
    pub fn project(&self, mut snapshot: Value) -> Value {
        snapshot["history"] = json!(true);
        snapshot["history_executed_at"] = json!(self.entry().executed_at);
        snapshot["history_plan_available"] = json!(self.plan().is_some());
        snapshot["rows"] = json!(self.result().map_or(0, |result| result.rows.len()));
        snapshot["result_available"] = json!(self.result().is_some());
        snapshot["result_phase"] = json!("finished");
        snapshot["capture_error"] = Value::Null;
        snapshot["received_rows"] = Value::Null;
        snapshot["capture_bytes"] = Value::Null;
        match &self.entry().outcome {
            QueryHistoryOutcome::Success {
                rows,
                cutoff,
                command,
                duration_ms,
                ..
            } => {
                snapshot["execution_seconds"] = json!(*duration_ms as f64 / 1000.0);
                snapshot["history_total_rows"] = json!(rows);
                snapshot["cutoff"] = json!(cutoff);
                snapshot["command_result"] = json!(command);
                snapshot["termination"] = json!("complete");
                snapshot["error"] = Value::Null;
            }
            QueryHistoryOutcome::Failure {
                error, diagnostic, ..
            } => {
                snapshot["execution_seconds"] = Value::Null;
                snapshot["history_total_rows"] = json!(0);
                snapshot["cutoff"] = Value::Null;
                snapshot["command_result"] = Value::Null;
                snapshot["termination"] = json!("failed");
                snapshot["error"] = diagnostic
                    .as_ref()
                    .map_or_else(|| json!(error), |error| json!(error));
            }
        }
        snapshot
    }
}
