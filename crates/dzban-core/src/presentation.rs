mod diagnostic;
mod document;
mod feedback;
mod history;
pub use diagnostic::{DiagnosticCode, DiagnosticKind, DiagnosticPresentation};
pub use document::{
    CapturePhase, CaptureTermination, DocumentError, DocumentPresentation, ExecutionPhase,
    PlanEstimate,
};
pub use feedback::{EmptyResultReason, ResultFeedback, ResultProgress, ResultStatusReason};
pub use history::{HistoryEntryPresentation, HistoryOutcomeKind, HistoryPresentation};

use crate::{DatabaseBackend, QueryEstimate, QueryPlan};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstimatePresentation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<DiagnosticPresentation>,
    pub rows: Option<u64>,
    pub startup_cost: Option<f64>,
    pub total_cost: Option<f64>,
    pub parts: Option<u64>,
    pub marks: Option<u64>,
}

impl From<&QueryEstimate> for EstimatePresentation {
    fn from(estimate: &QueryEstimate) -> Self {
        Self {
            diagnostic: estimate
                .diagnostic
                .as_ref()
                .map(DiagnosticPresentation::from),
            rows: estimate.rows,
            startup_cost: estimate.startup_cost,
            total_cost: estimate.total_cost,
            parts: estimate.parts,
            marks: estimate.marks,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanPresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<DatabaseBackend>,
    pub document: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimate: Option<EstimatePresentation>,
    pub text: String,
}

impl From<&QueryPlan> for PlanPresentation {
    fn from(plan: &QueryPlan) -> Self {
        Self {
            backend: Some(plan.backend),
            document: plan.document.clone(),
            estimate: Some(EstimatePresentation::from(&plan.estimate)),
            text: plan.text(),
        }
    }
}

impl PlanPresentation {
    pub fn from_saved(value: &Value) -> Self {
        if let Ok(plan) = serde_json::from_value::<QueryPlan>(value.clone()) {
            return Self::from(&plan);
        }
        Self {
            backend: None,
            document: value.clone(),
            estimate: None,
            text: crate::plan_text::plan_text(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputHint {
    Default,
    PrimaryGuide,
    Estimating,
    EstimateReady { plan: PlanPresentation },
    EstimateError { error: String },
    Planning,
    Running { plan: Option<PlanPresentation> },
    Canceling { plan: Option<PlanPresentation> },
}

impl From<crate::interaction::ActionHint> for InputHint {
    fn from(hint: crate::interaction::ActionHint) -> Self {
        use crate::interaction::ActionHint;
        match hint {
            ActionHint::Default => Self::Default,
            ActionHint::PrimaryGuide => Self::PrimaryGuide,
            ActionHint::Estimating => Self::Estimating,
            ActionHint::EstimateReady(plan) => Self::EstimateReady {
                plan: PlanPresentation::from(&plan),
            },
            ActionHint::EstimateError(error) => Self::EstimateError { error },
            ActionHint::Planning => Self::Planning,
            ActionHint::Running(plan) => Self::Running {
                plan: plan.as_ref().map(PlanPresentation::from),
            },
            ActionHint::Canceling(plan) => Self::Canceling {
                plan: plan.as_ref().map(PlanPresentation::from),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputPresentation {
    pub session: uuid::Uuid,
    pub document: uuid::Uuid,
    pub commands: Vec<crate::interaction::AppCommand>,
    pub hint: InputHint,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InteractionPresentation {
    pub revision: u64,
    pub draft_revision: u64,
    pub retired: bool,
    pub input: InputPresentation,
    pub visible: bool,
    pub loading: bool,
    pub plan: Option<PlanPresentation>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlsPresentation {
    pub revision: u64,
    pub session: Option<uuid::Uuid>,
    pub query: Option<uuid::Uuid>,
    pub primary_action: crate::interaction::PrimaryAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabPhase {
    Choosing,
    Connecting,
    Ready,
    Closing,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabPresentation {
    pub tab: uuid::Uuid,
    pub session: Option<uuid::Uuid>,
    pub profile: Option<String>,
    pub backend: Option<DatabaseBackend>,
    pub document: uuid::Uuid,
    pub execution: ExecutionPhase,
    pub phase: TabPhase,
    pub failure: Option<String>,
    pub snapshot: DocumentPresentation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspacePresentation {
    pub revision: u64,
    pub tabs: Vec<TabPresentation>,
    pub selected: Option<uuid::Uuid>,
}

impl From<&crate::tabs::TabsSnapshot> for WorkspacePresentation {
    fn from(workspace: &crate::tabs::TabsSnapshot) -> Self {
        Self {
            revision: workspace.revision,
            selected: workspace.selected.map(|id| id.0),
            tabs: workspace
                .tabs
                .iter()
                .map(|tab| {
                    use crate::tabs::TabPhase as Phase;
                    let snapshot = DocumentPresentation::from(&tab.document.snapshot());
                    TabPresentation {
                        tab: tab.id.0,
                        session: tab.engine.as_ref().map(|engine| engine.id().0),
                        profile: tab.profile.as_ref().map(|profile| profile.0.clone()),
                        backend: tab.backend,
                        document: tab.document.id().0,
                        execution: snapshot.execution,
                        phase: match tab.phase {
                            Phase::Choosing => TabPhase::Choosing,
                            Phase::Connecting => TabPhase::Connecting,
                            Phase::Ready => TabPhase::Ready,
                            Phase::Closing => TabPhase::Closing,
                            Phase::Failed(_) => TabPhase::Error,
                        },
                        failure: if let Phase::Failed(error) = &tab.phase {
                            Some(error.to_string())
                        } else {
                            None
                        },
                        snapshot,
                    }
                })
                .collect(),
        }
    }
}
