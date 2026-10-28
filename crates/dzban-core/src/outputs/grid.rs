use std::collections::BTreeSet;

use super::RowRange;
use super::view::ResultPosition;
use crate::ResultColumn;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionMode {
    #[default]
    Cells,
    Rows,
    Columns,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionGesture {
    Replace,
    Extend,
    Toggle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridCopy {
    pub rows: Vec<RowRange>,
    pub columns: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StickyLayout {
    pub left: Vec<usize>,
    pub right: Vec<usize>,
    pub visible: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct ResultGrid {
    schema: Vec<ResultColumn>,
    rows: usize,
    cursor: Option<ResultPosition>,
    anchor: Option<ResultPosition>,
    mode: SelectionMode,
    selected_rows: BTreeSet<usize>,
    selected_columns: BTreeSet<usize>,
    frozen: BTreeSet<usize>,
}

impl ResultGrid {
    pub fn new(schema: Vec<ResultColumn>, rows: usize) -> Self {
        Self {
            schema,
            rows,
            cursor: None,
            anchor: None,
            mode: SelectionMode::Cells,
            selected_rows: BTreeSet::new(),
            selected_columns: BTreeSet::new(),
            frozen: BTreeSet::from([0]),
        }
    }

    pub fn cursor(&self) -> Option<ResultPosition> {
        self.cursor
    }
    pub fn mode(&self) -> SelectionMode {
        self.mode
    }
    pub fn rows(&self) -> &BTreeSet<usize> {
        &self.selected_rows
    }
    pub fn columns(&self) -> &BTreeSet<usize> {
        &self.selected_columns
    }
    pub fn is_frozen(&self, column: usize) -> bool {
        self.frozen.contains(&column)
    }
    pub fn schema(&self) -> &[ResultColumn] {
        &self.schema
    }

    pub fn resize(&mut self, rows: usize) {
        self.rows = rows;
        self.selected_rows.retain(|row| *row < rows);
        if self.mode == SelectionMode::Columns {
            self.selected_rows = (0..rows).collect();
        }
        if let Some(cursor) = &mut self.cursor {
            if rows == 0 {
                self.cursor = None;
            } else {
                cursor.row = cursor.row.min(rows - 1);
            }
        }
        if let Some(anchor) = &mut self.anchor {
            anchor.row = anchor.row.min(rows.saturating_sub(1));
        }
    }

    pub fn select_cell(&mut self, position: ResultPosition, extend: bool) {
        if position.row >= self.rows || position.column > self.schema.len() {
            return;
        }
        if !extend || self.anchor.is_none() {
            self.anchor = Some(position);
        }
        let anchor = self.anchor.unwrap_or(position);
        self.mode = SelectionMode::Cells;
        self.cursor = Some(position);
        self.selected_rows =
            (anchor.row.min(position.row)..=anchor.row.max(position.row)).collect();
        self.selected_columns =
            (anchor.column.min(position.column)..=anchor.column.max(position.column)).collect();
    }

    pub fn select_row(&mut self, row: usize, gesture: SelectionGesture) {
        if row >= self.rows {
            return;
        }
        let column = self
            .cursor
            .map_or(1.min(self.schema.len()), |cursor| cursor.column);
        let position = ResultPosition { row, column };
        if gesture != SelectionGesture::Extend || self.anchor.is_none() {
            self.anchor = Some(position);
        }
        match gesture {
            SelectionGesture::Replace => self.selected_rows = BTreeSet::from([row]),
            SelectionGesture::Toggle => {
                if !self.selected_rows.remove(&row) {
                    self.selected_rows.insert(row);
                }
            }
            SelectionGesture::Extend => {
                let anchor = self.anchor.unwrap_or(position).row;
                self.selected_rows = (anchor.min(row)..=anchor.max(row)).collect();
            }
        }
        self.cursor = Some(position);
        self.mode = SelectionMode::Rows;
        self.selected_columns = (0..=self.schema.len()).collect();
    }

    pub fn select_column(&mut self, column: usize, gesture: SelectionGesture) {
        if column > self.schema.len() {
            return;
        }
        let position = ResultPosition {
            row: self.cursor.map_or(0, |cursor| cursor.row),
            column,
        };
        if gesture != SelectionGesture::Extend || self.anchor.is_none() {
            self.anchor = Some(position);
        }
        match gesture {
            SelectionGesture::Replace => self.selected_columns = BTreeSet::from([column]),
            SelectionGesture::Toggle => {
                if !self.selected_columns.remove(&column) {
                    self.selected_columns.insert(column);
                }
            }
            SelectionGesture::Extend => {
                let anchor = self.anchor.unwrap_or(position).column;
                self.selected_columns = (anchor.min(column)..=anchor.max(column)).collect();
            }
        }
        self.selected_rows = (0..self.rows).collect();
        self.cursor = (self.rows > 0).then_some(position);
        self.mode = SelectionMode::Columns;
    }

    pub fn sync_rows(&mut self, rows: impl IntoIterator<Item = usize>) {
        if self.mode == SelectionMode::Columns {
            return;
        }
        self.selected_rows = rows.into_iter().filter(|row| *row < self.rows).collect();
        if let Some(row) = self.selected_rows.first().copied() {
            if self
                .cursor
                .is_none_or(|cursor| !self.selected_rows.contains(&cursor.row))
            {
                self.cursor = Some(ResultPosition {
                    row,
                    column: self
                        .cursor
                        .map_or(1.min(self.schema.len()), |cursor| cursor.column),
                });
            }
            if self.selected_columns.is_empty() {
                self.selected_columns.insert(self.cursor.unwrap().column);
            }
        }
    }

    pub fn move_cursor(&mut self, horizontal: i64, vertical: i64, extend: bool) {
        if self.rows == 0 {
            return;
        }
        let previous = self.cursor.unwrap_or(ResultPosition {
            row: 0,
            column: 1.min(self.schema.len()),
        });
        fn moved(index: usize, delta: i64) -> usize {
            let distance = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
            if delta < 0 {
                index.saturating_sub(distance)
            } else {
                index.saturating_add(distance)
            }
        }
        let moved = ResultPosition {
            row: moved(previous.row, vertical).min(self.rows - 1),
            column: moved(previous.column, horizontal).min(self.schema.len()),
        };
        if self.mode == SelectionMode::Rows && horizontal == 0 {
            self.select_row(
                moved.row,
                if extend {
                    SelectionGesture::Extend
                } else {
                    SelectionGesture::Replace
                },
            );
        } else {
            self.select_cell(moved, extend);
        }
    }

    pub fn toggle_rows(&mut self, visible: &[usize]) {
        let Some(mut cursor) = self.cursor else {
            return;
        };
        if self.mode == SelectionMode::Rows {
            if !visible.contains(&cursor.column)
                && let Some(column) = visible
                    .iter()
                    .copied()
                    .filter(|column| *column != 0)
                    .min_by_key(|column| column.abs_diff(cursor.column))
                    .or_else(|| visible.first().copied())
            {
                cursor.column = column;
            }
            self.mode = SelectionMode::Cells;
            self.selected_columns = BTreeSet::from([cursor.column]);
            self.cursor = Some(cursor);
            self.anchor = Some(cursor);
        } else {
            self.mode = SelectionMode::Rows;
            self.selected_columns = (0..=self.schema.len()).collect();
        }
    }

    pub fn select_all(&mut self) {
        self.selected_rows = (0..self.rows).collect();
        self.selected_columns = (0..=self.schema.len()).collect();
        self.mode = SelectionMode::Rows;
        if self.cursor.is_none() && self.rows > 0 {
            self.cursor = Some(ResultPosition {
                row: 0,
                column: 1.min(self.schema.len()),
            });
        }
    }

    pub fn toggle_frozen(&mut self, column: usize) {
        if column <= self.schema.len() && !self.frozen.remove(&column) {
            self.frozen.insert(column);
        }
    }

    pub fn selected(&self, row: usize, column: usize) -> bool {
        self.selected_rows.contains(&row) && self.selected_columns.contains(&column)
    }

    pub fn copy(&self) -> GridCopy {
        let mut ranges: Vec<RowRange> = Vec::new();
        for row in &self.selected_rows {
            if let Some(range) = ranges.last_mut()
                && range.end == *row
            {
                range.end += 1;
            } else {
                ranges.push(RowRange {
                    start: *row,
                    end: row + 1,
                });
            }
        }
        GridCopy {
            rows: ranges,
            columns: self.selected_columns.iter().copied().collect(),
        }
    }

    pub fn sticky_layout(&self, widths: &[f64], scroll: f64, viewport: f64) -> StickyLayout {
        if !scroll.is_finite()
            || !viewport.is_finite()
            || viewport <= 0.0
            || widths
                .iter()
                .any(|width| !width.is_finite() || *width <= 0.0)
        {
            return StickyLayout::default();
        }
        let mut offsets = Vec::new();
        let mut offset = -scroll;
        for width in widths {
            offsets.push(offset);
            offset += width;
        }
        let mut layout = StickyLayout::default();
        let mut left = 0.0;
        for column in self
            .frozen
            .iter()
            .copied()
            .filter(|column| *column < widths.len())
        {
            if offsets[column] < left && left + widths[column] <= viewport {
                layout.left.push(column);
                left += widths[column];
            }
        }
        let mut right = viewport;
        for column in self
            .frozen
            .iter()
            .rev()
            .copied()
            .filter(|column| *column < widths.len())
        {
            if offsets[column] + widths[column] > right
                && !layout.left.contains(&column)
                && right - widths[column] >= left
            {
                layout.right.push(column);
                right -= widths[column];
            }
        }
        layout.right.reverse();
        layout.visible = widths
            .iter()
            .enumerate()
            .filter_map(|(column, width)| {
                (layout.left.contains(&column)
                    || layout.right.contains(&column)
                    || (offsets[column] < right && offsets[column] + width > left))
                    .then_some(column)
            })
            .collect();
        layout
    }

    pub fn reveal_column(&self, widths: &[f64], scroll: f64, viewport: f64, column: usize) -> f64 {
        if column >= widths.len()
            || !viewport.is_finite()
            || viewport <= 0.0
            || !scroll.is_finite()
            || widths
                .iter()
                .any(|width| !width.is_finite() || *width <= 0.0)
        {
            return scroll;
        }
        let layout = self.sticky_layout(widths, scroll, viewport);
        if layout.left.contains(&column) || layout.right.contains(&column) {
            return scroll;
        }
        let left = layout.left.iter().map(|index| widths[*index]).sum::<f64>();
        let right = layout.right.iter().map(|index| widths[*index]).sum::<f64>();
        let start = widths[..column].iter().sum::<f64>();
        let end = start + widths[column];
        let target = if start < scroll + left {
            start - left
        } else if end > scroll + viewport - right {
            end - viewport + right
        } else {
            scroll
        };
        target.clamp(0.0, (widths.iter().sum::<f64>() - viewport).max(0.0))
    }
}
