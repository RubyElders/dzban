use std::collections::BTreeMap;

use crate::QueryPlan;

pub const PREVIEW_HOLD_MS: u64 = crate::preview::PREVIEW_HOLD.as_millis() as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputRole {
    PrimaryModifier,
    PreviewModifier,
    Inspect,
    Submit,
    Cancel,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyboardState {
    pressed: BTreeMap<InputRole, u64>,
}

impl KeyboardState {
    pub fn is_pressed(&self, key: InputRole) -> bool {
        self.pressed.contains_key(&key)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum QueryExecutionState {
    #[default]
    Idle,
    Planning,
    Running {
        plan: Option<QueryPlan>,
    },
    Canceling {
        plan: Option<QueryPlan>,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum PlanPreviewState {
    #[default]
    Empty,
    Scheduled,
    Loading,
    Ready(QueryPlan),
    Error(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlanVisibility {
    #[default]
    Hidden,
    Transient,
    Pinned,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppState {
    pub scope: Option<crate::execution::QueryScope>,
    pub keyboard: KeyboardState,
    pub execution: QueryExecutionState,
    pub preview: PlanPreviewState,
    pub plan_visibility: PlanVisibility,
    pub query_available: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppEvent {
    SessionSelected(Option<SessionInteraction>),
    DocumentExecutionChanged {
        scope: crate::execution::QueryScope,
        execution: QueryExecutionState,
    },
    KeyPressed {
        key: InputRole,
        at_ms: u64,
    },
    KeyReleased {
        key: InputRole,
        at_ms: u64,
    },
    KeyboardReset,
    QueryAvailabilityChanged(bool),
    PreviewLoading,
    PreviewReady(QueryPlan),
    PreviewFailed(String),
    PreviewCancelled,
    QueryPlanning,
    QueryRunning(Option<QueryPlan>),
    QueryCanceling,
    CancelRequested,
    PrimaryActionRequested,
    QueryFinished,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionInteraction {
    pub scope: crate::execution::QueryScope,
    pub execution: QueryExecutionState,
    pub query_available: bool,
}

impl SessionInteraction {
    pub fn from_engine(engine: &crate::execution::QueryEngine) -> Self {
        use crate::execution::{OperationState, QueryPhase, SessionPhase};
        let snapshot = engine.subscribe().borrow().clone();
        let operation = *engine.subscribe_operation().borrow();
        let phase = *engine.subscribe_session().borrow();
        let scope = snapshot.scope();
        let execution = match (&snapshot.phase, operation) {
            (
                QueryPhase::Queued(id) | QueryPhase::Explaining(id),
                OperationState::Query(active),
            ) if *id == active => QueryExecutionState::Planning,
            (QueryPhase::Executing(id), OperationState::Query(active)) if *id == active => {
                QueryExecutionState::Running {
                    plan: snapshot.plan,
                }
            }
            (QueryPhase::Canceling(id), OperationState::Query(active)) if *id == active => {
                QueryExecutionState::Canceling {
                    plan: snapshot.plan,
                }
            }
            _ => QueryExecutionState::Idle,
        };
        Self {
            scope,
            execution,
            query_available: phase == SessionPhase::Open && operation == OperationState::Idle,
        }
    }
}

impl AppEvent {
    pub fn scoped_document_execution(
        snapshot: &crate::documents::DocumentSnapshot,
        current: Option<crate::execution::QueryScope>,
    ) -> Option<Self> {
        let current = current?;
        if snapshot.session != Some(current.session) || snapshot.execution.query() != current.query
        {
            return None;
        }
        Some(Self::DocumentExecutionChanged {
            scope: current,
            execution: Self::document_execution(snapshot),
        })
    }

    fn document_execution(snapshot: &crate::documents::DocumentSnapshot) -> QueryExecutionState {
        use crate::documents::DocumentExecution;
        match snapshot.execution {
            DocumentExecution::Idle | DocumentExecution::Finished(_) => QueryExecutionState::Idle,
            DocumentExecution::Planning(_) => QueryExecutionState::Planning,
            DocumentExecution::Running(_) => QueryExecutionState::Running {
                plan: snapshot.plan.clone(),
            },
            DocumentExecution::Canceling(_) => QueryExecutionState::Canceling {
                plan: snapshot.plan.clone(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCommand {
    BeginPreview,
    CancelPreview,
    ShowPlan,
    HidePlan,
    PinPlan,
    RunQuery,
    CancelQuery,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActionHint {
    Default,
    PrimaryGuide,
    Estimating,
    EstimateReady(QueryPlan),
    EstimateError(String),
    Planning,
    Running(Option<QueryPlan>),
    Canceling(Option<QueryPlan>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimaryAction {
    Unavailable,
    Run,
    Stop,
    Stopping,
}

impl AppState {
    pub fn transition(&mut self, event: AppEvent) -> Vec<AppCommand> {
        match event {
            AppEvent::SessionSelected(selection) => {
                let scope = selection.as_ref().map(|selection| selection.scope);
                let changed =
                    self.scope.map(|scope| scope.session) != scope.map(|scope| scope.session);
                self.scope = scope;
                self.query_available = selection
                    .as_ref()
                    .is_some_and(|selection| selection.query_available);
                self.execution =
                    selection.map_or(QueryExecutionState::Idle, |selection| selection.execution);
                if changed {
                    self.preview = PlanPreviewState::Empty;
                    self.plan_visibility = PlanVisibility::Hidden;
                    vec![AppCommand::HidePlan, AppCommand::CancelPreview]
                } else {
                    Vec::new()
                }
            }
            AppEvent::DocumentExecutionChanged { scope, execution } => {
                if self.scope != Some(scope) {
                    return Vec::new();
                }
                self.execution = execution;
                if self.execution == QueryExecutionState::Planning {
                    self.preview = PlanPreviewState::Empty;
                    self.plan_visibility = PlanVisibility::Hidden;
                    vec![AppCommand::HidePlan]
                } else {
                    Vec::new()
                }
            }
            AppEvent::PrimaryActionRequested => match self.primary_action() {
                PrimaryAction::Run => vec![AppCommand::RunQuery],
                PrimaryAction::Stop => vec![AppCommand::CancelQuery],
                PrimaryAction::Unavailable | PrimaryAction::Stopping => Vec::new(),
            },
            AppEvent::KeyPressed { key, at_ms } => self.key_pressed(key, at_ms),
            AppEvent::KeyReleased { key, at_ms } => self.key_released(key, at_ms),
            AppEvent::KeyboardReset => {
                self.keyboard.pressed.clear();
                if self.plan_visibility == PlanVisibility::Pinned {
                    Vec::new()
                } else {
                    self.plan_visibility = PlanVisibility::Hidden;
                    vec![AppCommand::HidePlan, AppCommand::CancelPreview]
                }
            }
            AppEvent::QueryAvailabilityChanged(available) => {
                self.query_available = available;
                Vec::new()
            }
            AppEvent::PreviewLoading => {
                self.preview = PlanPreviewState::Loading;
                Vec::new()
            }
            AppEvent::PreviewReady(plan) => {
                self.preview = PlanPreviewState::Ready(plan);
                match self.plan_visibility {
                    PlanVisibility::Transient if self.keyboard.is_pressed(InputRole::Inspect) => {
                        vec![AppCommand::ShowPlan]
                    }
                    PlanVisibility::Pinned => {
                        vec![AppCommand::ShowPlan, AppCommand::PinPlan]
                    }
                    PlanVisibility::Hidden | PlanVisibility::Transient => Vec::new(),
                }
            }
            AppEvent::PreviewFailed(error) => {
                self.preview = PlanPreviewState::Error(error);
                Vec::new()
            }
            AppEvent::PreviewCancelled => {
                self.preview = PlanPreviewState::Empty;
                self.plan_visibility = PlanVisibility::Hidden;
                Vec::new()
            }
            AppEvent::QueryPlanning => {
                self.execution = QueryExecutionState::Planning;
                self.preview = PlanPreviewState::Empty;
                self.plan_visibility = PlanVisibility::Hidden;
                vec![AppCommand::HidePlan]
            }
            AppEvent::QueryRunning(plan) => {
                self.execution = QueryExecutionState::Running { plan };
                Vec::new()
            }
            AppEvent::QueryCanceling => {
                let plan = match &self.execution {
                    QueryExecutionState::Running { plan }
                    | QueryExecutionState::Canceling { plan } => plan.clone(),
                    QueryExecutionState::Idle | QueryExecutionState::Planning => None,
                };
                self.execution = QueryExecutionState::Canceling { plan };
                Vec::new()
            }
            AppEvent::CancelRequested
                if matches!(
                    self.execution,
                    QueryExecutionState::Planning | QueryExecutionState::Running { .. }
                ) =>
            {
                vec![AppCommand::CancelQuery]
            }
            AppEvent::CancelRequested => Vec::new(),
            AppEvent::QueryFinished => {
                self.execution = QueryExecutionState::Idle;
                Vec::new()
            }
        }
    }

    pub fn action_hint(&self) -> ActionHint {
        match &self.execution {
            QueryExecutionState::Planning => return ActionHint::Planning,
            QueryExecutionState::Running { plan } => return ActionHint::Running(plan.clone()),
            QueryExecutionState::Canceling { plan } => return ActionHint::Canceling(plan.clone()),
            QueryExecutionState::Idle => {}
        }
        if !self.preview_available() || !self.keyboard.is_pressed(InputRole::PrimaryModifier) {
            return ActionHint::Default;
        }
        if !self.keyboard.is_pressed(InputRole::PreviewModifier) {
            return ActionHint::PrimaryGuide;
        }
        match &self.preview {
            PlanPreviewState::Empty => ActionHint::PrimaryGuide,
            PlanPreviewState::Scheduled | PlanPreviewState::Loading => ActionHint::Estimating,
            PlanPreviewState::Ready(plan) => ActionHint::EstimateReady(plan.clone()),
            PlanPreviewState::Error(error) => ActionHint::EstimateError(error.clone()),
        }
    }

    pub fn primary_action(&self) -> PrimaryAction {
        match self.execution {
            QueryExecutionState::Planning | QueryExecutionState::Running { .. } => {
                PrimaryAction::Stop
            }
            QueryExecutionState::Canceling { .. } => PrimaryAction::Stopping,
            QueryExecutionState::Idle if self.query_available => PrimaryAction::Run,
            QueryExecutionState::Idle => PrimaryAction::Unavailable,
        }
    }

    pub fn captures_key(&self, key: InputRole) -> bool {
        if key == InputRole::Cancel {
            return self.execution != QueryExecutionState::Idle;
        }
        self.keyboard.is_pressed(InputRole::PrimaryModifier)
            && (key == InputRole::Submit
                || (key == InputRole::Inspect
                    && self.keyboard.is_pressed(InputRole::PreviewModifier)))
    }

    fn preview_available(&self) -> bool {
        self.execution == QueryExecutionState::Idle
            && (self.query_available || self.preview != PlanPreviewState::Empty)
    }

    fn key_pressed(&mut self, key: InputRole, at_ms: u64) -> Vec<AppCommand> {
        if self.keyboard.pressed.contains_key(&key) {
            return Vec::new();
        }
        self.keyboard.pressed.insert(key, at_ms);
        let primary = self.keyboard.is_pressed(InputRole::PrimaryModifier);
        let preview_modifier = self.keyboard.is_pressed(InputRole::PreviewModifier);
        match key {
            InputRole::Cancel => self.transition(AppEvent::CancelRequested),
            InputRole::PreviewModifier | InputRole::PrimaryModifier
                if primary && preview_modifier && self.preview_available() =>
            {
                if matches!(self.preview, PlanPreviewState::Empty) {
                    self.preview = PlanPreviewState::Scheduled;
                    vec![AppCommand::BeginPreview]
                } else {
                    Vec::new()
                }
            }
            InputRole::Inspect if primary && preview_modifier && self.preview_available() => {
                self.plan_visibility = PlanVisibility::Transient;
                let mut commands = Vec::new();
                if matches!(self.preview, PlanPreviewState::Empty) {
                    self.preview = PlanPreviewState::Scheduled;
                    commands.push(AppCommand::BeginPreview);
                }
                if matches!(self.preview, PlanPreviewState::Ready(_)) {
                    commands.push(AppCommand::ShowPlan);
                }
                commands
            }
            InputRole::Submit if primary => self.transition(AppEvent::PrimaryActionRequested),
            _ => Vec::new(),
        }
    }

    fn key_released(&mut self, key: InputRole, at_ms: u64) -> Vec<AppCommand> {
        let Some(pressed_at) = self.keyboard.pressed.remove(&key) else {
            return Vec::new();
        };
        match key {
            InputRole::Inspect if self.plan_visibility == PlanVisibility::Transient => {
                if at_ms.saturating_sub(pressed_at) < PREVIEW_HOLD_MS {
                    self.plan_visibility = PlanVisibility::Pinned;
                    vec![AppCommand::PinPlan]
                } else {
                    self.plan_visibility = PlanVisibility::Hidden;
                    vec![AppCommand::HidePlan, AppCommand::CancelPreview]
                }
            }
            InputRole::PrimaryModifier | InputRole::PreviewModifier
                if self.plan_visibility != PlanVisibility::Pinned =>
            {
                self.plan_visibility = PlanVisibility::Hidden;
                vec![AppCommand::HidePlan, AppCommand::CancelPreview]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatabaseBackend, QueryEstimate};

    fn plan() -> QueryPlan {
        QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate: QueryEstimate {
                rows: Some(42),
                ..QueryEstimate::default()
            },
        }
    }

    #[test]
    fn primary_exposes_guidance_and_preview_modifier_requests_an_estimate() {
        let mut state = AppState {
            query_available: true,
            ..AppState::default()
        };
        state.transition(AppEvent::KeyPressed {
            key: InputRole::PrimaryModifier,
            at_ms: 0,
        });
        assert_eq!(state.action_hint(), ActionHint::PrimaryGuide);
        assert_eq!(
            state.transition(AppEvent::KeyPressed {
                key: InputRole::PreviewModifier,
                at_ms: 10,
            }),
            vec![AppCommand::BeginPreview]
        );
        assert_eq!(state.action_hint(), ActionHint::Estimating);
    }

    #[test]
    fn inspect_tap_pins_but_hold_hides_the_plan() {
        for (released_at, expected, visibility) in [
            (100, AppCommand::PinPlan, PlanVisibility::Pinned),
            (
                PREVIEW_HOLD_MS,
                AppCommand::HidePlan,
                PlanVisibility::Hidden,
            ),
        ] {
            let mut state = AppState::default();
            state.keyboard.pressed.insert(InputRole::PrimaryModifier, 0);
            state.keyboard.pressed.insert(InputRole::PreviewModifier, 0);
            state.preview = PlanPreviewState::Ready(plan());
            assert_eq!(
                state.transition(AppEvent::KeyPressed {
                    key: InputRole::Inspect,
                    at_ms: 0,
                }),
                vec![AppCommand::ShowPlan]
            );
            let commands = state.transition(AppEvent::KeyReleased {
                key: InputRole::Inspect,
                at_ms: released_at,
            });
            assert_eq!(commands[0], expected);
            assert_eq!(state.plan_visibility, visibility);
        }
    }

    #[test]
    fn a_tap_before_the_estimate_arrives_opens_and_pins_it_when_ready() {
        let mut state = AppState {
            query_available: true,
            ..AppState::default()
        };
        state.keyboard.pressed.insert(InputRole::PrimaryModifier, 0);
        state.keyboard.pressed.insert(InputRole::PreviewModifier, 0);
        assert_eq!(
            state.transition(AppEvent::KeyPressed {
                key: InputRole::Inspect,
                at_ms: 10,
            }),
            vec![AppCommand::BeginPreview]
        );
        assert_eq!(
            state.transition(AppEvent::KeyReleased {
                key: InputRole::Inspect,
                at_ms: 100,
            }),
            vec![AppCommand::PinPlan]
        );
        assert_eq!(
            state.transition(AppEvent::PreviewReady(plan())),
            vec![AppCommand::ShowPlan, AppCommand::PinPlan]
        );
    }

    #[test]
    fn query_start_always_closes_a_plan() {
        let mut state = AppState {
            preview: PlanPreviewState::Ready(plan()),
            plan_visibility: PlanVisibility::Pinned,
            ..AppState::default()
        };
        assert_eq!(
            state.transition(AppEvent::QueryPlanning),
            vec![AppCommand::HidePlan]
        );
        assert_eq!(state.plan_visibility, PlanVisibility::Hidden);
        assert_eq!(state.execution, QueryExecutionState::Planning);
    }

    #[test]
    fn canceling_is_an_explicit_execution_phase() {
        let mut state = AppState::default();
        state.transition(AppEvent::QueryRunning(Some(plan())));
        assert_eq!(
            state.transition(AppEvent::CancelRequested),
            vec![AppCommand::CancelQuery]
        );
        state.transition(AppEvent::QueryCanceling);
        assert!(matches!(
            state.action_hint(),
            ActionHint::Canceling(Some(_))
        ));
        state.transition(AppEvent::QueryFinished);
        assert_eq!(state.execution, QueryExecutionState::Idle);
    }

    #[test]
    fn cancel_role_stops_active_work_and_leaves_idle_preview_handling_to_the_frontend() {
        for execution in [
            QueryExecutionState::Planning,
            QueryExecutionState::Running { plan: None },
        ] {
            let mut state = AppState {
                execution,
                ..AppState::default()
            };
            assert!(state.captures_key(InputRole::Cancel));
            assert_eq!(
                state.transition(AppEvent::KeyPressed {
                    key: InputRole::Cancel,
                    at_ms: 0
                }),
                vec![AppCommand::CancelQuery]
            );
            assert!(
                state
                    .transition(AppEvent::KeyPressed {
                        key: InputRole::Cancel,
                        at_ms: 1
                    })
                    .is_empty()
            );
            state.transition(AppEvent::QueryCanceling);
            state.transition(AppEvent::KeyReleased {
                key: InputRole::Cancel,
                at_ms: 2,
            });
            assert!(state.captures_key(InputRole::Cancel));
            assert!(
                state
                    .transition(AppEvent::KeyPressed {
                        key: InputRole::Cancel,
                        at_ms: 3
                    })
                    .is_empty()
            );
            state.transition(AppEvent::QueryFinished);
            state.transition(AppEvent::KeyReleased {
                key: InputRole::Cancel,
                at_ms: 4,
            });
            assert!(!state.captures_key(InputRole::Cancel));
            assert!(
                state
                    .transition(AppEvent::KeyPressed {
                        key: InputRole::Cancel,
                        at_ms: 5
                    })
                    .is_empty()
            );
        }
    }

    #[test]
    fn one_chord_schedules_one_estimate_and_captures_inspect_repeats() {
        let mut state = AppState {
            query_available: true,
            ..AppState::default()
        };
        state.transition(AppEvent::KeyPressed {
            key: InputRole::PrimaryModifier,
            at_ms: 0,
        });
        assert_eq!(
            state.transition(AppEvent::KeyPressed {
                key: InputRole::PreviewModifier,
                at_ms: 1
            }),
            vec![AppCommand::BeginPreview]
        );
        assert_eq!(state.preview, PlanPreviewState::Scheduled);
        assert!(
            state
                .transition(AppEvent::KeyPressed {
                    key: InputRole::Inspect,
                    at_ms: 10
                })
                .is_empty()
        );
        assert!(state.captures_key(InputRole::Inspect));
        assert!(
            state
                .transition(AppEvent::KeyPressed {
                    key: InputRole::Inspect,
                    at_ms: 500
                })
                .is_empty()
        );
        assert_eq!(
            state.transition(AppEvent::KeyReleased {
                key: InputRole::Inspect,
                at_ms: 600
            }),
            vec![AppCommand::HidePlan, AppCommand::CancelPreview]
        );
    }

    #[test]
    fn execute_shortcut_follows_primary_action_and_ignores_repeats() {
        let mut state = AppState::default();
        state.transition(AppEvent::KeyPressed {
            key: InputRole::PrimaryModifier,
            at_ms: 0,
        });
        for (event, expected) in [
            (AppEvent::QueryAvailabilityChanged(false), vec![]),
            (
                AppEvent::QueryAvailabilityChanged(true),
                vec![AppCommand::RunQuery],
            ),
            (AppEvent::QueryPlanning, vec![AppCommand::CancelQuery]),
            (AppEvent::QueryRunning(None), vec![AppCommand::CancelQuery]),
            (AppEvent::QueryCanceling, vec![]),
            (AppEvent::QueryFinished, vec![AppCommand::RunQuery]),
        ] {
            state.transition(event);
            assert_eq!(
                state.transition(AppEvent::KeyPressed {
                    key: InputRole::Submit,
                    at_ms: 1
                }),
                expected
            );
            assert!(
                state
                    .transition(AppEvent::KeyPressed {
                        key: InputRole::Submit,
                        at_ms: 2
                    })
                    .is_empty()
            );
            state.transition(AppEvent::KeyReleased {
                key: InputRole::Submit,
                at_ms: 3,
            });
        }
    }

    #[test]
    fn primary_action_runs_stops_and_waits_for_cancellation() {
        let mut state = AppState::default();
        assert_eq!(state.primary_action(), PrimaryAction::Unavailable);
        state.transition(AppEvent::QueryAvailabilityChanged(true));
        assert_eq!(
            state.transition(AppEvent::PrimaryActionRequested),
            vec![AppCommand::RunQuery]
        );
        state.transition(AppEvent::QueryPlanning);
        assert_eq!(state.primary_action(), PrimaryAction::Stop);
        assert_eq!(
            state.transition(AppEvent::PrimaryActionRequested),
            vec![AppCommand::CancelQuery]
        );
        state.transition(AppEvent::QueryRunning(None));
        assert_eq!(state.primary_action(), PrimaryAction::Stop);
        state.transition(AppEvent::QueryCanceling);
        assert_eq!(state.primary_action(), PrimaryAction::Stopping);
        assert!(
            state
                .transition(AppEvent::PrimaryActionRequested)
                .is_empty()
        );
        state.transition(AppEvent::QueryFinished);
        assert_eq!(state.primary_action(), PrimaryAction::Run);
    }
}
