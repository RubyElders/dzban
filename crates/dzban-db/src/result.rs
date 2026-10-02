use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::DatabaseBackend;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultColumn {
    pub name: String,
    pub type_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResultCell {
    Null,
    Text(String),
}

impl ResultCell {
    pub fn display(&self) -> &str {
        match self {
            Self::Null => "NULL",
            Self::Text(value) => value,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultSet {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<ResultCell>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryEstimate {
    pub rows: Option<u64>,
    pub startup_cost: Option<f64>,
    pub total_cost: Option<f64>,
    pub parts: Option<u64>,
    pub marks: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryPlan {
    pub backend: DatabaseBackend,
    pub document: serde_json::Value,
    pub estimate: QueryEstimate,
}

#[derive(Debug)]
pub struct QueryResult {
    pub result_set: ResultSet,
    pub duration: Duration,
}
