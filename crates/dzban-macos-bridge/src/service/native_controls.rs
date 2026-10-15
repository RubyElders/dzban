use super::*;
use dzban_core::interaction::{ActionHint, PlanVisibility};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Primary,
    Preview,
    Inspect,
    Submit,
    Cancel,
}

impl Key {
    fn role(self) -> InputRole {
        match self {
            Self::Primary => InputRole::PrimaryModifier,
            Self::Preview => InputRole::PreviewModifier,
            Self::Inspect => InputRole::Inspect,
            Self::Submit => InputRole::Submit,
            Self::Cancel => InputRole::Cancel,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Press { key: Key, at_ms: u64 },
    Release { key: Key, at_ms: u64 },
    Reset,
    Pin,
    ContextChanged,
    PreviewCanceled,
    PreviewReady { plan: dzban_core::QueryPlan },
    PreviewFailed { message: String },
}

impl State {
    pub(super) fn input(
        &self,
        session: Uuid,
        document: Uuid,
        revision: u64,
        input: Input,
    ) -> Result<Value, Value> {
        let selection = self.selection.lock().unwrap();
        if selection.connection != ConnectionSelection::Ready(SessionId(session)) {
            return Err(failure(
                "selection_superseded",
                "The workspace is no longer selected",
            ));
        }
        let mut workspaces = self.workspaces.lock().unwrap();
        let workspace = workspaces
            .get_mut(&SessionId(session))
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        let selected = workspace
            .documents
            .selected()
            .ok_or_else(|| failure("unknown_document", "No query is selected"))?;
        if selected.id() != DocumentId(document) || selected.snapshot().draft_revision != revision {
            return Err(failure(
                "selection_superseded",
                "The query context has changed",
            ));
        }
        let state = &mut workspace.input_state;
        let mut interaction = SessionInteraction::from_engine(&workspace.engine);
        if *workspace.engine.subscribe_session().borrow()
            == dzban_core::execution::SessionPhase::Open
            && matches!(
                *workspace.engine.subscribe_operation().borrow(),
                dzban_core::execution::OperationState::Preview
                    | dzban_core::execution::OperationState::Ping
                    | dzban_core::execution::OperationState::Version
            )
        {
            interaction.query_available = true;
        }
        state.transition(AppEvent::SessionSelected(Some(interaction)));
        if workspace.input_document != Some((DocumentId(document), revision)) {
            state.transition(AppEvent::KeyboardReset);
            state.transition(AppEvent::PreviewCancelled);
            workspace.input_document = Some((DocumentId(document), revision));
        }
        let commands = match input {
            Input::Press { key, at_ms } => state.transition(AppEvent::KeyPressed {
                key: key.role(),
                at_ms,
            }),
            Input::Release { key, at_ms } => state.transition(AppEvent::KeyReleased {
                key: key.role(),
                at_ms,
            }),
            Input::Reset => state.transition(AppEvent::KeyboardReset),
            Input::Pin => {
                state.plan_visibility = PlanVisibility::Pinned;
                vec![AppCommand::PinPlan]
            }
            Input::ContextChanged => {
                state.transition(AppEvent::KeyboardReset);
                state.transition(AppEvent::PreviewCancelled);
                vec![AppCommand::HidePlan, AppCommand::CancelPreview]
            }
            Input::PreviewCanceled => state.transition(AppEvent::PreviewCancelled),
            Input::PreviewReady { plan } => state.transition(AppEvent::PreviewReady(plan)),
            Input::PreviewFailed { message } => state.transition(AppEvent::PreviewFailed(message)),
        };
        let commands: Vec<_> = commands
            .into_iter()
            .map(|command| match command {
                AppCommand::BeginPreview => "begin_preview",
                AppCommand::CancelPreview => "cancel_preview",
                AppCommand::ShowPlan => "show_plan",
                AppCommand::HidePlan => "hide_plan",
                AppCommand::PinPlan => "pin_plan",
                AppCommand::RunQuery => "run_query",
                AppCommand::CancelQuery => "cancel_query",
            })
            .collect();
        let hint = match state.action_hint() {
            ActionHint::Default => json!({"kind":"default"}),
            ActionHint::PrimaryGuide => json!({"kind":"primary_guide"}),
            ActionHint::Estimating => json!({"kind":"estimating"}),
            ActionHint::EstimateReady(plan) => json!({"kind":"estimate_ready","plan":plan}),
            ActionHint::EstimateError(error) => json!({"kind":"estimate_error","error":error}),
            ActionHint::Planning => json!({"kind":"planning"}),
            ActionHint::Running(plan) => json!({"kind":"running","plan":plan}),
            ActionHint::Canceling(plan) => json!({"kind":"canceling","plan":plan}),
        };
        Ok(
            json!({"session":session,"document":document,"commands":commands,"hint":hint,"pinned":state.plan_visibility == PlanVisibility::Pinned}),
        )
    }

    pub(super) async fn await_idle(&self, session: Uuid) -> Result<Value, Value> {
        use dzban_core::execution::{OperationState, SessionPhase};
        let engine = self
            .workspaces
            .lock()
            .unwrap()
            .get(&SessionId(session))
            .map(|workspace| workspace.engine.clone())
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        let mut operation = engine.subscribe_operation();
        let mut lifecycle = engine.subscribe_session();
        loop {
            if *lifecycle.borrow_and_update() != SessionPhase::Open {
                return Err(execution_error(ExecutionError::Closed));
            }
            let state = *operation.borrow_and_update();
            match state {
                OperationState::Idle => return Ok(json!({"idle":true})),
                OperationState::Preview | OperationState::Ping | OperationState::Version => (),
                OperationState::Query { .. } => return Ok(json!({"idle":false})),
                _ => return Err(execution_error(ExecutionError::Busy)),
            }
            tokio::select! {
                changed = operation.changed() => if changed.is_err() { return Err(execution_error(ExecutionError::Closed)); },
                changed = lifecycle.changed() => if changed.is_err() { return Err(execution_error(ExecutionError::Closed)); },
            }
        }
    }

    pub(super) async fn probe_session(
        &self,
        session: Uuid,
        include_version: bool,
    ) -> Result<Value, Value> {
        let engine = self
            .workspaces
            .lock()
            .unwrap()
            .get(&SessionId(session))
            .map(|workspace| workspace.engine.clone())
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        Ok(probe(&engine, include_version).await)
    }

    pub(super) async fn probe_profile(&self, profile: String) -> Result<Value, Value> {
        let config = self
            .config
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| failure("configuration", "Load profiles before probing"))?;
        let config = config
            .resolve_profile(&ConnectionProfileId(profile), None)
            .map_err(|error| failure("profile", error))?;
        let engine = match self.context.connect_profile(config).await {
            Ok(engine) => engine,
            Err(error) => return Ok(json!({"state":"error","error":error.to_string()})),
        };
        let mut result = probe(&engine, true).await;
        self.context
            .close(engine.id())
            .await
            .map_err(|error| application_error("probe_cleanup", error))?;
        if result["state"] == "connected" {
            result["state"] = json!("reachable");
        }
        Ok(result)
    }
}

async fn probe(engine: &QueryEngine, include_version: bool) -> Value {
    let latency = match engine.start_ping() {
        Ok(handle) => match handle.wait().await {
            Ok(latency) => latency,
            Err(error) => return json!({"state":"error","error":error.to_string()}),
        },
        Err(ExecutionError::Busy) => return json!({"state":"busy"}),
        Err(error) => return json!({"state":"error","error":error.to_string()}),
    };
    let version = if include_version {
        match engine.start_version() {
            Ok(handle) => match handle.wait().await {
                Ok(version) => Some(version),
                Err(error) => return json!({"state":"error","error":error.to_string()}),
            },
            Err(ExecutionError::Busy) => None,
            Err(error) => return json!({"state":"error","error":error.to_string()}),
        }
    } else {
        None
    };
    json!({"state":"connected","latency_micros":latency,"version":version})
}
