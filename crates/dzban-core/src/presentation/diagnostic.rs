use crate::DatabaseBackend;
use crate::error::{ErrorKind, ExecutionError, NativeErrorCode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticKind {
    Database,
    Busy,
    Canceled,
    Closed,
    Unsupported,
    Message,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DiagnosticCode {
    SqlState(String),
    Clickhouse(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticPresentation {
    pub kind: DiagnosticKind,
    pub backend: Option<DatabaseBackend>,
    pub database_kind: Option<ErrorKind>,
    pub message: String,
    pub code: Option<DiagnosticCode>,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub text: String,
}

fn message(value: &str) -> String {
    if value == "Canceled" {
        "Query canceled".into()
    } else {
        value.into()
    }
}

impl From<&ExecutionError> for DiagnosticPresentation {
    fn from(error: &ExecutionError) -> Self {
        let mut result = Self {
            kind: DiagnosticKind::Message,
            backend: None,
            database_kind: None,
            message: String::new(),
            code: None,
            detail: None,
            hint: None,
            text: String::new(),
        };
        match error {
            ExecutionError::Database(error) => {
                result.kind = DiagnosticKind::Database;
                result.backend = Some(error.backend);
                result.database_kind = Some(error.kind.clone());
                result.message = error.message.clone();
                result.code = error.code.as_ref().map(|code| match code {
                    NativeErrorCode::SqlState(code) => DiagnosticCode::SqlState(code.clone()),
                    NativeErrorCode::ClickHouse(code) => DiagnosticCode::Clickhouse(*code),
                });
                result.detail = error.detail.clone();
                result.hint = error.hint.clone();
            }
            ExecutionError::Message(value) => result.message = value.clone(),
            ExecutionError::Unsupported(value) => {
                result.kind = DiagnosticKind::Unsupported;
                result.message = value.clone();
            }
            ExecutionError::Busy => {
                result.kind = DiagnosticKind::Busy;
                result.message = "Busy".into();
            }
            ExecutionError::Canceled => {
                result.kind = DiagnosticKind::Canceled;
                result.message = "Canceled".into();
            }
            ExecutionError::Closed => {
                result.kind = DiagnosticKind::Closed;
                result.message = "Closed".into();
            }
        }
        let primary = if result.kind == DiagnosticKind::Unsupported {
            format!("Unsupported: {}", message(&result.message))
        } else {
            message(&result.message)
        };
        let mut lines = vec![primary];
        match &result.code {
            Some(DiagnosticCode::SqlState(code)) => lines.push(message(code)),
            Some(DiagnosticCode::Clickhouse(code)) => lines.push(format!("{code}.0")),
            None => {}
        }
        lines.extend(
            result
                .detail
                .iter()
                .chain(&result.hint)
                .map(|value| message(value)),
        );
        result.text = lines.join("\n");
        result
    }
}
