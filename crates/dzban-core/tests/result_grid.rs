use dzban_core::{
    ResultColumn,
    outputs::{
        RowRange,
        grid::{ResultGrid, SelectionGesture, SelectionMode},
        view::ResultPosition,
    },
};

fn grid() -> ResultGrid {
    ResultGrid::new(
        (0..5)
            .map(|index| ResultColumn {
                name: format!("column{index}"),
                type_name: "text".into(),
            })
            .collect(),
        100,
    )
}

#[test]
fn cursor_range_and_row_selection_are_distinct_and_copy_in_original_order() {
    let mut grid = grid();
    grid.select_cell(ResultPosition { row: 3, column: 2 }, false);
    grid.move_cursor(2, 2, true);
    assert_eq!(grid.copy().rows, [RowRange { start: 3, end: 6 }]);
    assert_eq!(grid.copy().columns, [2, 3, 4]);
    grid.toggle_rows(&[0, 3, 4]);
    assert_eq!(grid.mode(), SelectionMode::Rows);
    grid.toggle_rows(&[0, 3, 4]);
    assert_eq!(grid.copy().columns, [4]);
    grid.select_row(8, SelectionGesture::Replace);
    grid.select_row(10, SelectionGesture::Toggle);
    assert_eq!(
        grid.copy().rows,
        [
            RowRange { start: 8, end: 9 },
            RowRange { start: 10, end: 11 }
        ]
    );
}

#[test]
fn leaving_rows_uses_a_visible_data_column_and_headers_follow_streaming_growth() {
    let mut grid = grid();
    grid.select_cell(ResultPosition { row: 0, column: 1 }, false);
    grid.toggle_rows(&[0, 3, 4]);
    grid.toggle_rows(&[0, 3, 4]);
    assert_eq!(grid.cursor().unwrap().column, 3);
    grid.select_column(4, SelectionGesture::Replace);
    grid.select_column(2, SelectionGesture::Toggle);
    assert_eq!(grid.copy().columns, [2, 4]);
    grid.resize(110);
    assert_eq!(grid.copy().rows, [RowRange { start: 0, end: 110 }]);
}

#[test]
fn locked_columns_stick_at_either_edge_and_return_without_reordering_navigation() {
    let mut grid = grid();
    grid.toggle_frozen(4);
    let widths = [50.0; 6];
    assert_eq!(grid.sticky_layout(&widths, 0.0, 150.0).right, [4]);
    assert_eq!(grid.sticky_layout(&widths, 125.0, 150.0).left, [0]);
    assert!(grid.sticky_layout(&widths, 125.0, 150.0).right.is_empty());
    assert_eq!(grid.sticky_layout(&widths, 240.0, 150.0).left, [0, 4]);
    grid.select_cell(ResultPosition { row: 0, column: 3 }, false);
    grid.move_cursor(1, 0, false);
    assert_eq!(grid.cursor().unwrap().column, 4);
    grid.move_cursor(1, 0, false);
    assert_eq!(grid.cursor().unwrap().column, 5);
}

#[test]
fn navigation_accounts_for_sticky_occlusion_without_scrolling_a_locked_cursor() {
    let mut grid = grid();
    grid.toggle_frozen(5);
    let widths = [50.0; 6];
    assert_eq!(grid.reveal_column(&widths, 0.0, 150.0, 5), 0.0);
    assert_eq!(grid.reveal_column(&widths, 0.0, 150.0, 2), 50.0);
    assert_eq!(grid.reveal_column(&widths, 150.0, 150.0, 3), 100.0);
    assert_eq!(grid.reveal_column(&widths, 150.0, 150.0, 0), 150.0);
}
