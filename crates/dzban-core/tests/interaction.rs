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
