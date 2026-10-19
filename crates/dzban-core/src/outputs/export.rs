use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use super::{ORIGINAL_CELL_BYTES, OutputError, ResultReader};
use crate::ResultCell;

const CHUNK_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportChunk {
    pub text: String,
    pub done: bool,
    pub is_null: bool,
}

enum Content {
    Cell,
    Markdown {
        ranges: VecDeque<RowRange>,
        columns: usize,
        rows: VecDeque<Vec<ResultCell>>,
    },
}

pub struct OutputExport {
    reader: ResultReader,
    content: Content,
    pending: String,
    offset: usize,
    bytes: usize,
    is_null: bool,
    finished: bool,
    failure: Option<OutputError>,
}

impl ResultReader {
    pub async fn export_cell(
        &self,
        row: usize,
        column: usize,
    ) -> Result<OutputExport, OutputError> {
        let cell = self.original_cell(row, column).await?;
        let is_null = cell == ResultCell::Null;
        let pending = match cell {
            ResultCell::Null => String::new(),
            ResultCell::Text(text) => text,
        };
        Ok(OutputExport {
            reader: self.clone(),
            content: Content::Cell,
            bytes: pending.len(),
            pending,
            offset: 0,
            is_null,
            finished: false,
            failure: None,
        })
    }

    pub async fn export_markdown(
        &self,
        ranges: Vec<RowRange>,
    ) -> Result<OutputExport, OutputError> {
        let mut previous = 0;
        for range in &ranges {
            if range.start >= range.end || range.start < previous {
                return Err(OutputError::InvalidSelection);
            }
            previous = range.end;
        }
        let schema = self.page(0, 0).await?;
        let mut pending = String::new();
        append_line(
            &mut pending,
            schema.columns.iter().map(|column| column.name.as_str()),
        )?;
        append_line(&mut pending, schema.columns.iter().map(|_| "---"))?;
        Ok(OutputExport {
            reader: self.clone(),
            content: Content::Markdown {
                ranges: ranges.into(),
                columns: schema.columns.len(),
                rows: VecDeque::new(),
            },
            bytes: pending.len(),
            pending,
            offset: 0,
            is_null: false,
            finished: false,
            failure: None,
        })
    }
}

impl OutputExport {
    pub async fn next_chunk(&mut self) -> Result<ExportChunk, OutputError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.read_chunk().await;
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    async fn read_chunk(&mut self) -> Result<ExportChunk, OutputError> {
        self.reader.validate()?;
        if self.finished {
            return Ok(ExportChunk {
                text: String::new(),
                done: true,
                is_null: self.is_null,
            });
        }
        if self.offset == self.pending.len() {
            self.pending.clear();
            self.offset = 0;
            if let Content::Markdown {
                ranges,
                columns,
                rows,
            } = &mut self.content
            {
                while self.pending.len() < CHUNK_BYTES {
                    let Some(range) = ranges.front_mut() else {
                        break;
                    };
                    if rows.is_empty() {
                        let count = (range.end - range.start).min(64);
                        let page = self.reader.page(range.start, count).await?;
                        if page.rows.len() != count {
                            return Err(OutputError::InvalidSelection);
                        }
                        *rows = page.rows.into();
                    }
                    let row = rows.pop_front().ok_or(OutputError::InvalidSelection)?;
                    if row.len() != *columns {
                        return Err(OutputError::InvalidSelection);
                    }
                    let before = self.pending.len();
                    append_line(
                        &mut self.pending,
                        row.iter().map(|cell| match cell {
                            ResultCell::Null => "NULL",
                            ResultCell::Text(text) => text.as_str(),
                        }),
                    )?;
                    self.bytes = self
                        .bytes
                        .checked_add(self.pending.len() - before)
                        .ok_or(OutputError::ExportTooLarge)?;
                    if self.bytes > ORIGINAL_CELL_BYTES {
                        return Err(OutputError::ExportTooLarge);
                    }
                    range.start += 1;
                    if range.start == range.end {
                        ranges.pop_front();
                    }
                }
            }
        }
        let mut end = self.pending.len().min(self.offset + CHUNK_BYTES);
        while !self.pending.is_char_boundary(end) {
            end -= 1;
        }
        let text = self.pending[self.offset..end].to_owned();
        self.offset = end;
        self.finished = self.offset == self.pending.len()
            && match &self.content {
                Content::Cell => true,
                Content::Markdown { ranges, .. } => ranges.is_empty(),
            };
        self.reader.validate()?;
        Ok(ExportChunk {
            text,
            done: self.finished,
            is_null: self.is_null,
        })
    }
}

fn append_line<'a>(
    output: &mut String,
    cells: impl Iterator<Item = &'a str>,
) -> Result<(), OutputError> {
    output.push_str("| ");
    for (index, cell) in cells.enumerate() {
        if index != 0 {
            output.push_str(" | ");
        }
        let mut characters = cell.chars().peekable();
        while let Some(character) = characters.next() {
            match character {
                '&' => output.push_str("&amp;"),
                '<' => output.push_str("&lt;"),
                '>' => output.push_str("&gt;"),
                '\r' => {
                    if characters.peek() == Some(&'\n') {
                        characters.next();
                    }
                    output.push_str("<br>");
                }
                '\n' => output.push_str("<br>"),
                '\\' | '|' | '`' | '*' | '_' | '[' | ']' => {
                    output.push('\\');
                    output.push(character);
                }
                _ => output.push(character),
            }
            if output.len() > ORIGINAL_CELL_BYTES {
                return Err(OutputError::ExportTooLarge);
            }
        }
    }
    output.push_str(" |\n");
    if output.len() > ORIGINAL_CELL_BYTES {
        return Err(OutputError::ExportTooLarge);
    }
    Ok(())
}
