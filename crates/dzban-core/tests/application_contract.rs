use dzban_core::documents::{DocumentExecution, QueryDocuments};
use dzban_core::execution::QueryId;
use dzban_core::interaction::{AppCommand, PrimaryAction};
use dzban_core::presentation::{
    CapturePhase, CaptureTermination, DocumentPresentation, ExecutionPhase, InputHint,
    InputPresentation, PlanPresentation,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Fixture {
    schema: u32,
    tab_phases: Vec<dzban_core::presentation::TabPhase>,
    execution_phases: Vec<ExecutionPhase>,
    capture_phases: Vec<CapturePhase>,
    capture_terminations: Vec<CaptureTermination>,
    primary_actions: Vec<PrimaryAction>,
    input_effects: Vec<AppCommand>,
    input_hints: Vec<InputHint>,
    document_record: Record,
    plan: PlanPresentation,
    legacy_plan: PlanPresentation,
    input: InputPresentation,
    interaction: dzban_core::presentation::InteractionPresentation,
    health: dzban_core::health::HealthSnapshot,
    controls: dzban_core::presentation::ControlsPresentation,
    attention: dzban_core::attention::AttentionSnapshot,
    history: dzban_core::presentation::HistoryPresentation,
}

#[derive(Deserialize)]
struct Record {
    schema: u32,
    revision: u64,
    payload: DocumentPresentation,
}

#[test]
fn shared_protocol_fixture_keeps_types_nulls_and_full_integer_precision() {
    let bytes = include_bytes!("../../../tests/fixtures/application-contract-v1.json");
    let fixture: Fixture = serde_json::from_slice(bytes).unwrap();
    let raw: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(fixture.interaction.revision, u64::MAX - 1);
    assert_eq!(fixture.interaction.draft_revision, u64::MAX);
    assert!(fixture.interaction.visible && fixture.interaction.input.commands.is_empty());
    assert_eq!(
        serde_json::to_value(&fixture.interaction).unwrap(),
        raw["interaction"]
    );
    assert_eq!(fixture.health.revision, u64::MAX - 2);
    assert_eq!(
        fixture.health.sessions[0].status.latency_micros,
        Some(u64::MAX - 1)
    );
    assert_eq!(
        fixture.health.sessions[0].status.checked_at_millis,
        Some(1_800_000_000_123)
    );
    assert_eq!(
        serde_json::to_value(&fixture.health).unwrap(),
        raw["health"]
    );
    assert_eq!(fixture.controls.revision, u64::MAX - 3);
    assert_eq!(
        serde_json::to_value(&fixture.controls).unwrap(),
        raw["controls"]
    );
    assert_eq!(fixture.attention.revision, u64::MAX - 4);
    assert_eq!(fixture.attention.request_revision, u64::MAX - 5);
    assert_eq!(fixture.attention.tabs[0].completion_revision, u64::MAX - 1);
    assert_eq!(
        fixture.attention.tabs[0]
            .unseen
            .as_ref()
            .unwrap()
            .identity
            .output
            .generation,
        u64::MAX
    );
    assert_eq!(
        serde_json::to_value(&fixture.attention).unwrap(),
        raw["attention"]
    );
    assert_eq!(fixture.history.entries[0].executed_at, u64::MAX - 1);
    assert_eq!(fixture.history.entries[0].duration_ms, Some(u64::MAX));
    assert_eq!(
        fixture.history.entries[0].status,
        dzban_core::presentation::HistoryOutcomeKind::Success
    );
    assert_eq!(
        fixture.history.entries[1].status,
        dzban_core::presentation::HistoryOutcomeKind::Failure
    );
    assert_eq!(
        serde_json::to_value(&fixture.history).unwrap(),
        raw["history"]
    );
    assert_eq!(fixture.schema, 1);
    use dzban_core::presentation::TabPhase;
    assert_eq!(
        fixture.tab_phases,
        [
            TabPhase::Choosing,
            TabPhase::Connecting,
            TabPhase::Ready,
            TabPhase::Closing,
            TabPhase::Error
        ]
    );
    assert_eq!(fixture.document_record.schema, 1);
    assert_eq!(fixture.document_record.revision, u64::MAX);
    assert_eq!(fixture.document_record.payload.generation, u64::MAX - 1);
    assert_eq!(fixture.document_record.payload.draft_revision, u64::MAX);
    assert_eq!(
        fixture.plan.estimate.as_ref().unwrap().rows,
        Some(u64::MAX - 1)
    );
    assert_eq!(fixture.legacy_plan.backend, None);
    assert_eq!(fixture.legacy_plan.estimate, None);
    assert_eq!(fixture.input.commands, fixture.input_effects);
    assert_eq!(serde_json::to_value(fixture.input).unwrap(), raw["input"]);
    assert_eq!(serde_json::to_value(fixture.plan).unwrap(), raw["plan"]);
    assert_eq!(
        fixture.execution_phases,
        [
            ExecutionPhase::Idle,
            ExecutionPhase::Planning,
            ExecutionPhase::Running,
            ExecutionPhase::Canceling,
            ExecutionPhase::Finished
        ]
    );
    assert_eq!(
        fixture.capture_phases,
        [
            CapturePhase::WaitingForSchema,
            CapturePhase::Receiving,
            CapturePhase::Draining,
            CapturePhase::FailedAwaitingCleanup,
            CapturePhase::Finished
        ]
    );
    assert_eq!(
        fixture.capture_terminations,
        [
            CaptureTermination::Complete,
            CaptureTermination::Limited,
            CaptureTermination::Canceled,
            CaptureTermination::Failed
        ]
    );
    assert_eq!(
        fixture.primary_actions,
        [
            PrimaryAction::Unavailable,
            PrimaryAction::Run,
            PrimaryAction::Stop,
            PrimaryAction::Stopping
        ]
    );
    assert_eq!(fixture.input_hints.len(), 8);
}

#[test]
fn document_projection_preserves_query_identity_and_terminal_phases() {
    let document = QueryDocuments::default().open();
    document.set_draft("SELECT '😀';".into());
    let mut snapshot = document.snapshot();
    let query = QueryId(uuid::Uuid::new_v4());
    for (execution, phase) in [
        (DocumentExecution::Idle, ExecutionPhase::Idle),
        (DocumentExecution::Planning(query), ExecutionPhase::Planning),
        (DocumentExecution::Running(query), ExecutionPhase::Running),
        (
            DocumentExecution::Canceling(query),
            ExecutionPhase::Canceling,
        ),
        (DocumentExecution::Finished(query), ExecutionPhase::Finished),
    ] {
        snapshot.execution = execution;
        let projected = DocumentPresentation::from(&snapshot);
        assert_eq!(projected.execution, phase);
        assert_eq!(projected.query, execution.query().map(|id| id.0));
        assert_eq!(projected.draft, "SELECT '😀';");
        assert!(!projected.result_available);
        assert_eq!(projected.result_phase, None);
        assert_eq!(projected.termination, None);
    }
}
