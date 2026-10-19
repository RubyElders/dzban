use dzban_core::error::ExecutionError;
use dzban_core::presentation::{DiagnosticPresentation, DocumentPresentation, ResultFeedback};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    schema: u32,
    cases: Vec<Case>,
    diagnostics: Vec<Diagnostic>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    document: DocumentPresentation,
    text: String,
}
#[derive(Deserialize)]
struct Diagnostic {
    error: ExecutionError,
    presentation: DiagnosticPresentation,
}

fn fixture() -> Fixture {
    serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/result-feedback-v1.json"
    ))
    .unwrap()
}

#[test]
fn shared_feedback_preserves_native_status_precedence_and_diagnostic_visibility() {
    let raw: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/result-feedback-v1.json"
    ))
    .unwrap();
    let fixture = fixture();
    assert_eq!(fixture.schema, 1);
    for (case, raw) in fixture.cases.iter().zip(raw["cases"].as_array().unwrap()) {
        assert!(!case.text.is_empty());
        let expected = &case.document.feedback;
        let actual = if raw["document"]["history"] == true {
            ResultFeedback::saved(
                case.document.rows,
                expected.total_rows.unwrap(),
                case.document.command_result.as_ref(),
                case.document
                    .error
                    .as_ref()
                    .map(DiagnosticPresentation::from),
            )
        } else {
            ResultFeedback::live(&case.document)
        };
        assert_eq!(&actual, expected, "{}", case.name);
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            raw["document"]["feedback"],
            "{}",
            case.name
        );
    }
}

#[test]
fn typed_diagnostics_preserve_backend_fields_and_native_default_text() {
    for sample in fixture().diagnostics {
        let actual = DiagnosticPresentation::from(&sample.error);
        assert_eq!(actual, sample.presentation);
    }
}

#[test]
fn estimates_normalize_diagnostics_on_live_saved_and_input_routes() {
    use dzban_core::presentation::{EstimatePresentation, InputHint, PlanPresentation};
    use dzban_core::{DatabaseBackend, QueryEstimate, QueryPlan};
    for sample in fixture().diagnostics {
        let estimate = QueryEstimate {
            diagnostic: Some(sample.error),
            rows: Some(u64::MAX),
            ..Default::default()
        };
        let projected = EstimatePresentation::from(&estimate);
        assert_eq!(projected.diagnostic, Some(sample.presentation));
        assert_eq!(projected.rows, Some(u64::MAX));
        let plan = QueryPlan {
            backend: DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate,
        };
        let live = PlanPresentation::from(&plan);
        assert_eq!(live.estimate, Some(projected));
        let saved = PlanPresentation::from_saved(&serde_json::to_value(&plan).unwrap());
        assert_eq!(live, saved);
        match InputHint::from(dzban_core::interaction::ActionHint::EstimateReady(plan)) {
            InputHint::EstimateReady { plan } => assert_eq!(plan, live),
            _ => panic!("wrong hint"),
        }
    }
}
