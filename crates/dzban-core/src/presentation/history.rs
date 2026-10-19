use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryOutcomeKind {
    Success,
    Failure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntryPresentation {
    pub index: usize,
    pub sql_preview: String,
    pub executed_at: u64,
    pub status: HistoryOutcomeKind,
    pub rows: Option<usize>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryPresentation {
    pub revision: Uuid,
    pub entries: Vec<HistoryEntryPresentation>,
}
