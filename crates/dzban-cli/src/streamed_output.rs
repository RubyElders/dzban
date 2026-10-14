use std::io::{self, Write};

use dzban_core::execution::StreamedQueryResult;
use dzban_core::result_store::{MAX_PAGE_ROWS, ResultCutoff};
use dzban_core::result_stream::{ResultPhase, ResultSnapshot};
use dzban_core::{ResultCell, ResultColumn};
use unicode_width::UnicodeWidthStr;

use crate::args::OutputFormat;

const TABLE_CELL_WIDTH: usize = 120;

pub fn preview_lines(page: &dzban_core::ResultSet, width: usize, count: usize) -> Vec<String> {
    let limit = width.saturating_sub(1);
    page.rows
        .iter()
        .take(count)
        .enumerate()
        .map(|(index, row)| {
            let mut line = crate::editor_view::single_line(&format!("Row {}: ", index + 1), limit);
            for (column, value) in page.columns.iter().zip(row) {
                if line.width() >= limit {
                    break;
                }
                let prefix = format!(
                    "{}: ",
                    crate::editor_view::single_line(
                        &column.name,
                        limit.saturating_sub(line.width())
                    )
                );
                line.push_str(&crate::editor_view::single_line(
                    &prefix,
                    limit.saturating_sub(line.width()),
                ));
                line.push_str(&crate::editor_view::single_line(
                    value.display(),
                    limit.saturating_sub(line.width()),
                ));
                line.push_str(&crate::editor_view::single_line(
                    " | ",
                    limit.saturating_sub(line.width()),
                ));
            }
            line
        })
        .collect()
}

pub fn cutoff_label(cutoff: ResultCutoff) -> &'static str {
    match cutoff {
        ResultCutoff::Rows => "row limit",
        ResultCutoff::Bytes => "byte limit",
    }
}

pub fn progress(capture: &ResultSnapshot) -> String {
    match capture.phase {
        ResultPhase::WaitingForSchema => "waiting for first row".into(),
        ResultPhase::Receiving if capture.rows == 0 => "waiting for first row".into(),
        ResultPhase::Receiving => format!("{} rows received", capture.rows),
        ResultPhase::Draining(cutoff) => format!(
            "{} rows retained ({}) | draining",
            capture.rows,
            cutoff_label(cutoff)
        ),
        ResultPhase::FailedAwaitingCleanup(_) => "cleaning up failed query".into(),
        ResultPhase::Finished(_) => String::new(),
    }
}

fn cell(text: &str) -> String {
    let mut value = String::new();
    for character in text.chars() {
        let part = match character {
            '\n' => "\\n".into(),
            '\r' => "\\r".into(),
            '\t' => "\\t".into(),
            c if c.is_control() => format!("\\u{{{:x}}}", c as u32),
            c => c.to_string(),
        };
        if value.width() + part.width() > TABLE_CELL_WIDTH {
            while value.width() > TABLE_CELL_WIDTH - 3 {
                value.pop();
            }
            value.push_str("...");
            break;
        }
        value.push_str(&part);
    }
    value
}

fn csv_field(writer: &mut impl Write, text: &str) -> io::Result<()> {
    writer.write_all(b"\"")?;
    let mut start = 0;
    for (index, _) in text.match_indices('"') {
        writer.write_all(&text.as_bytes()[start..index])?;
        writer.write_all(b"\"\"")?;
        start = index + 1;
    }
    writer.write_all(&text.as_bytes()[start..])?;
    writer.write_all(b"\"")
}

fn table_row(writer: &mut impl Write, values: &[String], widths: &[usize]) -> io::Result<()> {
    writer.write_all(b"|")?;
    for (value, width) in values.iter().zip(widths) {
        write!(
            writer,
            " {value}{} |",
            " ".repeat(width.saturating_sub(value.width()))
        )?;
    }
    writer.write_all(b"\n")
}

pub async fn write_to(
    result: &StreamedQueryResult,
    format: OutputFormat,
    writer: &mut impl Write,
) -> io::Result<()> {
    if let Some(command) = &result.command {
        let text = crate::output::render_query(
            &dzban_core::QueryResult {
                result_set: Default::default(),
                duration: result.duration,
                command: Some(command.clone()),
            },
            format,
        )
        .map_err(|error| io::Error::other(error.message))?;
        return writer.write_all(text.as_bytes());
    }
    let reader = result
        .capture
        .result
        .as_ref()
        .ok_or_else(|| io::Error::other("Query completed without a result schema"))?;
    let schema = reader.read_page(0, 0).await.map_err(io::Error::other)?;
    let columns: &[ResultColumn] = &schema.columns;
    let headings: Vec<_> = columns.iter().map(|column| cell(&column.name)).collect();
    let mut widths: Vec<_> = headings.iter().map(|name| name.width()).collect();
    if matches!(format, OutputFormat::Table) {
        let mut start = 0;
        while start < result.capture.rows {
            let page = reader
                .read_page(start, MAX_PAGE_ROWS)
                .await
                .map_err(io::Error::other)?;
            if page.rows.is_empty() {
                return Err(io::Error::other("Result page is missing"));
            }
            start += page.rows.len();
            for row in page.rows {
                for (width, value) in widths.iter_mut().zip(row) {
                    *width = (*width).max(cell(value.display()).width());
                }
            }
        }
    }
    let border = format!(
        "+{}+\n",
        widths
            .iter()
            .map(|width| "-".repeat(width + 2))
            .collect::<Vec<_>>()
            .join("+")
    );
    match format {
        OutputFormat::Json => {
            writer.write_all(b"{\"columns\":")?;
            serde_json::to_writer(&mut *writer, columns)?;
            writer.write_all(b",\"rows\":[")?;
        }
        OutputFormat::Csv => {
            for (index, column) in columns.iter().enumerate() {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                csv_field(writer, &column.name)?;
            }
            writer.write_all(b"\n")?;
        }
        OutputFormat::Table => {
            writer.write_all(border.as_bytes())?;
            table_row(writer, &headings, &widths)?;
            writer.write_all(border.as_bytes())?;
        }
        OutputFormat::Expanded => {}
    }
    let mut start = 0;
    while start < result.capture.rows {
        let page = reader
            .read_page(start, MAX_PAGE_ROWS)
            .await
            .map_err(io::Error::other)?;
        if page.rows.is_empty() {
            return Err(io::Error::other("Result page is missing"));
        }
        for (offset, row) in page.rows.iter().enumerate() {
            match format {
                OutputFormat::Json => {
                    if start + offset != 0 {
                        writer.write_all(b",")?;
                    }
                    serde_json::to_writer(&mut *writer, row)?;
                }
                OutputFormat::Csv => {
                    for (index, value) in row.iter().enumerate() {
                        if index != 0 {
                            writer.write_all(b",")?;
                        }
                        if let ResultCell::Text(text) = value {
                            csv_field(writer, text)?;
                        }
                    }
                    writer.write_all(b"\n")?;
                }
                OutputFormat::Expanded => {
                    writeln!(writer, "Record {}", start + offset + 1)?;
                    for (column, value) in columns.iter().zip(row) {
                        writeln!(writer, "{}: {}", column.name, value.display())?;
                    }
                }
                OutputFormat::Table => table_row(
                    writer,
                    &row.iter()
                        .map(|value| cell(value.display()))
                        .collect::<Vec<_>>(),
                    &widths,
                )?,
            }
        }
        start += page.rows.len();
    }
    match format {
        OutputFormat::Json => {
            writer.write_all(b"]")?;
            if let Some(cutoff) = result.capture.cutoff {
                writer.write_all(b",\"cutoff\":")?;
                serde_json::to_writer(&mut *writer, &cutoff)?;
            }
            writer.write_all(b"}\n")?;
        }
        OutputFormat::Table => {
            writer.write_all(border.as_bytes())?;
            writeln!(writer, "({} rows)", result.capture.rows)?;
            if widths.contains(&TABLE_CELL_WIDTH) {
                writeln!(
                    writer,
                    "Table cells shortened at {TABLE_CELL_WIDTH} columns; use expanded, JSON or CSV for full values."
                )?;
            }
        }
        _ => {}
    }
    if let Some(cutoff) = result.capture.cutoff
        && matches!(format, OutputFormat::Table | OutputFormat::Expanded)
    {
        writeln!(writer, "Result limited by {}.", cutoff_label(cutoff))?;
    }
    writer.flush()
}

pub async fn write_stdout(
    result: &StreamedQueryResult,
    format: OutputFormat,
) -> Result<(), crate::CliError> {
    if let Some(cutoff) = result.capture.cutoff
        && matches!(format, OutputFormat::Csv)
    {
        eprintln!("dzb: result limited by {}", cutoff_label(cutoff));
    }
    match write_to(result, format, &mut std::io::stdout().lock()).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(crate::CliError::runtime(error.to_string())),
    }
}
