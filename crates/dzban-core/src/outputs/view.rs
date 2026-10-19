use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::ResultColumn;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultPosition {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestorationState {
    WaitingSchema,
    SchemaChanged,
    WaitingRows,
    Apply,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultViewDecision {
    pub selection: Option<ResultPosition>,
    pub restoration: Option<RestorationState>,
    pub selection_pending: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResultViewRequest {
    Select {
        row: i64,
        column: i64,
        rows: usize,
        columns: usize,
    },
    Clamp {
        position: ResultPosition,
        rows: usize,
        columns: usize,
    },
    MoveRow {
        position: ResultPosition,
        direction: i64,
        rows: usize,
        columns: usize,
    },
    Restore {
        previous: Vec<ResultColumn>,
        current: Vec<ResultColumn>,
        row: Option<usize>,
        column: usize,
        rows: usize,
        terminal: bool,
    },
}

impl ResultViewRequest {
    pub fn evaluate(&self) -> ResultViewDecision {
        let mut decision = ResultViewDecision {
            selection: None,
            restoration: None,
            selection_pending: false,
        };
        match self {
            Self::Select {
                row,
                column,
                rows,
                columns,
            } => {
                if let (Ok(row), Ok(column)) = (usize::try_from(*row), usize::try_from(*column))
                    && row < *rows
                    && column < *columns
                {
                    decision.selection = Some(ResultPosition { row, column });
                }
            }
            Self::Clamp {
                position,
                rows,
                columns,
            } => {
                decision.selection = clamp(*position, *rows, *columns);
            }
            Self::MoveRow {
                position,
                direction,
                rows,
                columns,
            } => {
                let moved = ResultPosition {
                    row: move_index(position.row, *direction),
                    column: position.column,
                };
                decision.selection =
                    clamp(moved, *rows, *columns).filter(|next| next.row != position.row);
            }
            Self::Restore {
                previous,
                current,
                row,
                column,
                rows,
                terminal,
            } => {
                decision.restoration = Some(if current.is_empty() {
                    RestorationState::WaitingSchema
                } else if !schema_matches(previous, current) {
                    RestorationState::SchemaChanged
                } else if *rows == 0 {
                    RestorationState::WaitingRows
                } else {
                    decision.selection_pending = row.is_some_and(|row| row >= *rows) && !terminal;
                    if !decision.selection_pending {
                        decision.selection = row.and_then(|row| {
                            clamp(
                                ResultPosition {
                                    row,
                                    column: *column,
                                },
                                *rows,
                                current.len(),
                            )
                        });
                    }
                    RestorationState::Apply
                });
            }
        }
        decision
    }
}

fn clamp(position: ResultPosition, rows: usize, columns: usize) -> Option<ResultPosition> {
    if rows == 0 || columns == 0 {
        return None;
    }
    Some(ResultPosition {
        row: position.row.min(rows - 1),
        column: position.column.min(columns - 1),
    })
}

fn move_index(index: usize, direction: i64) -> usize {
    let distance = usize::try_from(direction.unsigned_abs()).unwrap_or(usize::MAX);
    if direction < 0 {
        index.saturating_sub(distance)
    } else {
        index.saturating_add(distance)
    }
}

fn schema_matches(previous: &[ResultColumn], current: &[ResultColumn]) -> bool {
    previous.len() == current.len()
        && previous.iter().zip(current).all(|(left, right)| {
            (left.name == right.name || left.name.nfc().eq(right.name.nfc()))
                && (left.type_name == right.type_name
                    || left.type_name.nfc().eq(right.type_name.nfc()))
        })
}
