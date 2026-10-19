use dzban_core::ResultColumn;
use dzban_core::outputs::view::{RestorationState, ResultPosition, ResultViewRequest};

fn position(row: usize, column: usize) -> ResultPosition {
    ResultPosition { row, column }
}
fn schema() -> Vec<ResultColumn> {
    vec![ResultColumn {
        name: "Value".into(),
        type_name: "text".into(),
    }]
}

#[test]
fn selection_rejects_invalid_indices_and_clamps_only_when_requested() {
    for (row, column) in [(-1, 0), (0, -1), (3, 0), (0, 2), (i64::MAX, i64::MAX)] {
        assert!(
            ResultViewRequest::Select {
                row,
                column,
                rows: 3,
                columns: 2
            }
            .evaluate()
            .selection
            .is_none()
        );
    }
    assert_eq!(
        ResultViewRequest::Select {
            row: 2,
            column: 1,
            rows: 3,
            columns: 2
        }
        .evaluate()
        .selection,
        Some(position(2, 1))
    );
    for (rows, columns) in [(0, 2), (3, 0), (0, 0)] {
        assert!(
            ResultViewRequest::Clamp {
                position: position(5, 5),
                rows,
                columns
            }
            .evaluate()
            .selection
            .is_none()
        );
    }
    assert_eq!(
        ResultViewRequest::Clamp {
            position: position(usize::MAX, usize::MAX),
            rows: 3,
            columns: 2
        }
        .evaluate()
        .selection,
        Some(position(2, 1))
    );
}

#[test]
fn row_navigation_handles_edges_empty_results_and_extreme_directions() {
    let moved = |row, direction, rows| {
        ResultViewRequest::MoveRow {
            position: position(row, 7),
            direction,
            rows,
            columns: 8,
        }
        .evaluate()
        .selection
    };
    assert_eq!(moved(0, -1, 3), None);
    assert_eq!(moved(2, 1, 3), None);
    assert_eq!(moved(1, 0, 3), None);
    assert_eq!(moved(1, 1, 3), Some(position(2, 7)));
    assert_eq!(moved(1, -1, 3), Some(position(0, 7)));
    assert_eq!(moved(1, i64::MAX, 3), Some(position(2, 7)));
    assert_eq!(moved(1, i64::MIN, 3), Some(position(0, 7)));
    assert_eq!(moved(usize::MAX, i64::MAX, 3), Some(position(2, 7)));
    assert_eq!(moved(0, 1, 0), None);
}

fn restore(
    current: Vec<ResultColumn>,
    rows: usize,
    terminal: bool,
    row: Option<usize>,
) -> dzban_core::outputs::view::ResultViewDecision {
    ResultViewRequest::Restore {
        previous: schema(),
        current,
        row,
        column: 7,
        rows,
        terminal,
    }
    .evaluate()
}

#[test]
fn restoration_waits_for_schema_and_rows_without_selecting_a_placeholder() {
    let unknown = restore(vec![], 0, false, Some(100));
    assert_eq!(unknown.restoration, Some(RestorationState::WaitingSchema));
    assert_eq!(unknown.selection, None);
    let empty = restore(schema(), 0, true, Some(100));
    assert_eq!(empty.restoration, Some(RestorationState::WaitingRows));
    assert_eq!(empty.selection, None);
    let partial = restore(schema(), 20, false, Some(100));
    assert_eq!(partial.restoration, Some(RestorationState::Apply));
    assert!(partial.selection_pending);
    assert_eq!(partial.selection, None);
    let grown = restore(schema(), 101, false, Some(100));
    assert!(!grown.selection_pending);
    assert_eq!(grown.selection, Some(position(100, 0)));
}

#[test]
fn terminal_shorter_results_clamp_but_preserve_absent_selection() {
    let finished = restore(schema(), 20, true, Some(100));
    assert_eq!(finished.selection, Some(position(19, 0)));
    assert!(!finished.selection_pending);
    let unselected = restore(schema(), 20, true, None);
    assert_eq!(unselected.restoration, Some(RestorationState::Apply));
    assert_eq!(unselected.selection, None);
    assert!(!unselected.selection_pending);
}

#[test]
fn schema_name_type_order_and_duplicate_columns_are_compared_exactly() {
    for current in [
        vec![ResultColumn {
            name: "value".into(),
            type_name: "text".into(),
        }],
        vec![ResultColumn {
            name: "Value".into(),
            type_name: "varchar".into(),
        }],
        vec![schema()[0].clone(), schema()[0].clone()],
    ] {
        let changed = restore(current, 20, true, Some(1));
        assert_eq!(changed.restoration, Some(RestorationState::SchemaChanged));
        assert_eq!(changed.selection, None);
    }
    let columns = vec![
        ResultColumn {
            name: "😀".into(),
            type_name: "text".into(),
        },
        schema()[0].clone(),
    ];
    let mut reversed = columns.clone();
    reversed.reverse();
    assert_eq!(
        ResultViewRequest::Restore {
            previous: columns.clone(),
            current: reversed,
            row: Some(0),
            column: 0,
            rows: 1,
            terminal: true
        }
        .evaluate()
        .restoration,
        Some(RestorationState::SchemaChanged)
    );
    assert_eq!(
        ResultViewRequest::Restore {
            previous: columns.clone(),
            current: columns,
            row: Some(0),
            column: 1,
            rows: 1,
            terminal: true
        }
        .evaluate()
        .selection,
        Some(position(0, 1))
    );
}

#[test]
fn schema_matching_preserves_native_canonical_unicode_equality() {
    let previous = vec![ResultColumn {
        name: "caf\u{e9}".into(),
        type_name: "caf\u{e9}".into(),
    }];
    let current = vec![ResultColumn {
        name: "cafe\u{301}".into(),
        type_name: "cafe\u{301}".into(),
    }];
    let decision = ResultViewRequest::Restore {
        previous,
        current,
        row: Some(0),
        column: 0,
        rows: 1,
        terminal: true,
    }
    .evaluate();
    assert_eq!(decision.restoration, Some(RestorationState::Apply));
    assert_eq!(decision.selection, Some(position(0, 0)));
}
