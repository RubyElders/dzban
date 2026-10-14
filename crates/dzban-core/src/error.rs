use serde::{Deserialize, Serialize};

use crate::DatabaseBackend;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    Server,
    Transport,
    Canceled,
    Busy,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeErrorCode {
    SqlState(String),
    ClickHouse(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseError {
    pub backend: DatabaseBackend,
    pub kind: ErrorKind,
    pub message: String,
    pub code: Option<NativeErrorCode>,
    pub detail: Option<String>,
    pub hint: Option<String>,
}

impl std::fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)?;
        match &self.code {
            Some(NativeErrorCode::SqlState(code)) => write!(formatter, " (SQLSTATE {code})")?,
            Some(NativeErrorCode::ClickHouse(code)) => {
                write!(formatter, " (ClickHouse code {code})")?
            }
            None => {}
        }
        if let Some(detail) = &self.detail {
            write!(formatter, ". Detail: {detail}")?;
        }
        if let Some(hint) = &self.hint {
            write!(formatter, ". Hint: {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DatabaseError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionError {
    Database(DatabaseError),
    Busy,
    Canceled,
    Closed,
    Unsupported(String),
    Message(String),
}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => error.fmt(formatter),
            Self::Busy => formatter.write_str("Connection is busy"),
            Self::Canceled => formatter.write_str("Query cancelled"),
            Self::Closed => formatter.write_str("Session is closed"),
            Self::Unsupported(action) => write!(formatter, "Action is not supported: {action}"),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ExecutionError {}

impl From<String> for ExecutionError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}
