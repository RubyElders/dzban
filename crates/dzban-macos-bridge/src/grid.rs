use dzban_core::outputs::grid::{ResultGrid, SelectionGesture, SelectionMode};
use dzban_core::outputs::view::ResultPosition;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Create {
        columns: Vec<dzban_core::ResultColumn>,
        rows: usize,
    },
    Resize {
        rows: usize,
    },
    Cell {
        row: usize,
        column: usize,
        extend: bool,
    },
    Row {
        row: usize,
        gesture: String,
    },
    Column {
        column: usize,
        gesture: String,
    },
    Move {
        horizontal: i64,
        vertical: i64,
        extend: bool,
    },
    ToggleRows {
        visible: Vec<usize>,
    },
    Freeze {
        column: usize,
    },
    All,
    Clone,
    Layout {
        widths: Vec<f64>,
        scroll: f64,
        viewport: f64,
        column: Option<usize>,
    },
}

pub fn apply(handle: u64, bytes: &[u8]) -> Option<Value> {
    let request: Request = serde_json::from_slice(bytes).ok()?;
    let (handle, request) = match request {
        Request::Create { columns, rows } if handle == 0 => (
            crate::handles::insert_grid(ResultGrid::new(columns, rows))?,
            None,
        ),
        Request::Create { .. } => return None,
        Request::Clone => {
            let grid = crate::handles::grid(handle)?;
            let copy = grid.lock().ok()?.clone();
            (crate::handles::insert_grid(copy)?, None)
        }
        request => (handle, Some(request)),
    };
    let grid = crate::handles::grid(handle)?;
    let mut grid = grid.lock().ok()?;
    let gesture = |value: &str| match value {
        "extend" => Some(SelectionGesture::Extend),
        "toggle" => Some(SelectionGesture::Toggle),
        "replace" => Some(SelectionGesture::Replace),
        _ => None,
    };
    let mut layout = Default::default();
    let mut reveal = None;
    if let Some(request) = request {
        match request {
            Request::Resize { rows } => grid.resize(rows),
            Request::Cell {
                row,
                column,
                extend,
            } => grid.select_cell(ResultPosition { row, column }, extend),
            Request::Row {
                row,
                gesture: value,
            } => grid.select_row(row, gesture(&value)?),
            Request::Column {
                column,
                gesture: value,
            } => grid.select_column(column, gesture(&value)?),
            Request::Move {
                horizontal,
                vertical,
                extend,
            } => grid.move_cursor(horizontal, vertical, extend),
            Request::ToggleRows { visible } => grid.toggle_rows(&visible),
            Request::Freeze { column } => grid.toggle_frozen(column),
            Request::All => grid.select_all(),
            Request::Layout {
                widths,
                scroll,
                viewport,
                column,
            } => {
                layout = grid.sticky_layout(&widths, scroll, viewport);
                reveal = column.map(|column| grid.reveal_column(&widths, scroll, viewport, column));
            }
            Request::Create { .. } | Request::Clone => unreachable!(),
        }
    }
    let copy = grid.copy();
    let mode = match grid.mode() {
        SelectionMode::Cells => "cells",
        SelectionMode::Rows => "rows",
        SelectionMode::Columns => "columns",
    };
    Some(
        json!({"handle":handle,"cursor":grid.cursor(),"mode":mode,"ranges":copy.rows,"columns":copy.columns,
        "frozen":(0..=grid.schema().len()).filter(|column| grid.is_frozen(*column)).collect::<Vec<_>>(),
        "left":layout.left,"right":layout.right,"visible":layout.visible,"reveal":reveal}),
    )
}
