use dzban_core::interaction::{
    AppCommand, AppEvent, AppState, InputKey, PlanVisibility, PrimaryAction,
};

fn press(state: &mut AppState, key: InputKey, at_ms: u64) -> Vec<AppCommand> {
    state.transition(AppEvent::KeyPressed { key, at_ms })
}

#[test]
fn submit_does_not_start_a_second_operation_during_execution_or_cancel() {
    let mut state = AppState::default();
    state.transition(AppEvent::QueryAvailabilityChanged(true));
    press(&mut state, InputKey::Control, 0);
    assert_eq!(
        press(&mut state, InputKey::Enter, 1),
        [AppCommand::RunQuery]
    );
    for phase in [
        AppEvent::QueryPlanning,
        AppEvent::QueryRunning(None),
        AppEvent::QueryCanceling,
    ] {
        state.transition(phase);
        state.transition(AppEvent::KeyReleased {
            key: InputKey::Enter,
            at_ms: 2,
        });
        assert!(press(&mut state, InputKey::Enter, 3).is_empty());
    }
    state.transition(AppEvent::QueryFinished);
    state.transition(AppEvent::KeyboardReset);
    assert_eq!(state.primary_action(), PrimaryAction::Run);
    press(&mut state, InputKey::Control, 4);
    assert_eq!(
        press(&mut state, InputKey::Enter, 5),
        [AppCommand::RunQuery]
    );
}

#[test]
fn focus_loss_releases_a_held_preview_without_stuck_modifiers() {
    let mut state = AppState::default();
    press(&mut state, InputKey::Control, 0);
    press(&mut state, InputKey::Shift, 1);
    press(&mut state, InputKey::Space, 2);
    assert_eq!(state.plan_visibility, PlanVisibility::Transient);
    assert_eq!(
        state.transition(AppEvent::KeyboardReset),
        [AppCommand::HidePlan, AppCommand::CancelPreview]
    );
    assert_eq!(state.plan_visibility, PlanVisibility::Hidden);
    assert!(!state.keyboard.is_pressed(InputKey::Control));
    assert!(!state.captures_key(InputKey::Space));
}

#[test]
fn selecting_the_chooser_clears_execution_controls_and_old_preview_state() {
    use dzban_core::execution::SessionId;
    use dzban_core::interaction::{ActionHint, QueryExecutionState, SessionInteraction};
    let session = SessionId(uuid::Uuid::new_v4());
    let mut state = AppState::default();
    let running = SessionInteraction {
        scope: dzban_core::execution::QueryScope {
            session,
            query: None,
        },
        execution: QueryExecutionState::Running { plan: None },
        query_available: false,
    };
    state.transition(AppEvent::SessionSelected(Some(running.clone())));
    assert_eq!(state.primary_action(), PrimaryAction::Stop);
    assert_eq!(state.scope.unwrap().session, session);
    state.preview = dzban_core::interaction::PlanPreviewState::Error("old estimate".into());
    state.plan_visibility = PlanVisibility::Pinned;
    state.transition(AppEvent::SessionSelected(None));
    assert_eq!(state.primary_action(), PrimaryAction::Unavailable);
    assert_eq!(state.action_hint(), ActionHint::Default);
    assert_eq!(state.scope, None);
    assert_eq!(
        state.preview,
        dzban_core::interaction::PlanPreviewState::Empty
    );
    assert_eq!(state.plan_visibility, PlanVisibility::Hidden);
    assert!(state.transition(AppEvent::CancelRequested).is_empty());
    state.transition(AppEvent::SessionSelected(Some(running)));
    assert_eq!(state.primary_action(), PrimaryAction::Stop);
    let mut other = SessionInteraction {
        scope: dzban_core::execution::QueryScope {
            session: SessionId(uuid::Uuid::new_v4()),
            query: None,
        },
        execution: QueryExecutionState::Idle,
        query_available: true,
    };
    state.transition(AppEvent::SessionSelected(Some(other.clone())));
    assert_eq!(state.primary_action(), PrimaryAction::Run);
    other.query_available = false;
    state.transition(AppEvent::SessionSelected(Some(other)));
    assert_eq!(state.primary_action(), PrimaryAction::Unavailable);
}

#[test]
fn queued_document_updates_cannot_change_another_selected_session_or_query() {
    use dzban_core::execution::{QueryId, QueryScope, SessionId};
    use dzban_core::interaction::{QueryExecutionState, SessionInteraction};
    let scope = QueryScope {
        session: SessionId(uuid::Uuid::new_v4()),
        query: Some(QueryId(uuid::Uuid::new_v4())),
    };
    let mut state = AppState::default();
    state.transition(AppEvent::SessionSelected(Some(SessionInteraction {
        scope,
        execution: QueryExecutionState::Running { plan: None },
        query_available: false,
    })));
    for old in [
        QueryScope {
            query: None,
            ..scope
        },
        QueryScope {
            session: SessionId(uuid::Uuid::new_v4()),
            ..scope
        },
    ] {
        state.transition(AppEvent::DocumentExecutionChanged {
            scope: old,
            execution: QueryExecutionState::Idle,
        });
        assert_eq!(state.primary_action(), PrimaryAction::Stop);
    }
    state.transition(AppEvent::SessionSelected(None));
    state.transition(AppEvent::DocumentExecutionChanged {
        scope,
        execution: QueryExecutionState::Running { plan: None },
    });
    assert_eq!(state.primary_action(), PrimaryAction::Unavailable);
    state.transition(AppEvent::SessionSelected(Some(SessionInteraction {
        scope,
        execution: QueryExecutionState::Running { plan: None },
        query_available: false,
    })));
    state.transition(AppEvent::DocumentExecutionChanged {
        scope,
        execution: QueryExecutionState::Canceling { plan: None },
    });
    assert_eq!(state.primary_action(), PrimaryAction::Stopping);
}
