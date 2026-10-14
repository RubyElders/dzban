use std::io::{self, Write};
use std::time::Duration;

use dzban_cli::args::OutputFormat;
use dzban_core::execution::StreamedQueryResult;
use dzban_core::result_store::ResultLimits;
use dzban_core::result_stream::start_results;
use dzban_core::{ResultCell, ResultColumn, ResultSet};

async fn captured(result: &ResultSet, rows: usize) -> StreamedQueryResult {
    let (mut producer, reader) = start_results(ResultLimits {
        rows,
        memory_rows: 1,
        ..Default::default()
    });
    producer.schema(result.columns.clone()).await.unwrap();
    for row in &result.rows {
        producer.row(row.clone()).await.unwrap();
    }
    producer.finish(Ok(())).await.unwrap();
    StreamedQueryResult {
        capture: reader.borrow().clone(),
        command: None,
        duration: Duration::ZERO,
    }
}

struct SmallWrites(Vec<u8>);
impl Write for SmallWrites {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert!(
            bytes.len() < 4096,
            "output was collected into a large write"
        );
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn fixture() -> ResultSet {
    ResultSet {
        columns: vec![
            ResultColumn {
                name: "id".into(),
                type_name: "Int64".into(),
            },
            ResultColumn {
                name: "value".into(),
                type_name: "Nullable(String)".into(),
            },
        ],
        rows: (0..601)
            .map(|index| {
                vec![
                    ResultCell::Text(index.to_string()),
                    if index == 200 {
                        ResultCell::Null
                    } else {
                        ResultCell::Text(format!("žluť,\"quoted\"\n{index}"))
                    },
                ]
            })
            .collect(),
    }
}

#[tokio::test]
async fn formats_cross_disk_pages_without_losing_order_null_or_framing() {
    let fixture = fixture();
    let result = captured(&fixture, 10_000).await;
    for format in [
        OutputFormat::Json,
        OutputFormat::Csv,
        OutputFormat::Expanded,
        OutputFormat::Table,
    ] {
        let mut writer = SmallWrites(Vec::new());
        dzban_cli::streamed_output::write_to(&result, format, &mut writer)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(writer.0).unwrap(),
            dzban_cli::output::render(&fixture, format).unwrap()
        );
    }
}

#[tokio::test]
async fn cutoff_is_explicit_and_table_cells_cannot_expand_output_without_bound() {
    let mut fixture = fixture();
    fixture.rows[0][1] = ResultCell::Text("x".repeat(50_000));
    let result = captured(&fixture, 3).await;
    let mut table = Vec::new();
    dzban_cli::streamed_output::write_to(&result, OutputFormat::Table, &mut table)
        .await
        .unwrap();
    assert!(table.len() < 2000);
    let table = String::from_utf8(table).unwrap();
    assert!(table.contains("..."));
    assert!(table.contains("Result limited by row limit"));
    let mut json = Vec::new();
    dzban_cli::streamed_output::write_to(&result, OutputFormat::Json, &mut json)
        .await
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(document["cutoff"], "Rows");
    assert_eq!(document["rows"][0][1].as_str().unwrap().len(), 50_000);
    assert_eq!(document["rows"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn empty_schema_rows_and_pager_early_exit_preserve_contracts() {
    let mut fixture = fixture();
    fixture.rows.clear();
    let result = captured(&fixture, 10_000).await;
    let mut writer = Vec::new();
    dzban_cli::streamed_output::write_to(&result, OutputFormat::Json, &mut writer)
        .await
        .unwrap();
    let decoded: ResultSet = serde_json::from_slice(&writer).unwrap();
    assert_eq!(decoded, fixture);
    let result = captured(&self::fixture(), 10_000).await;
    dzban_cli::pager::run_streamed("/bin/sh -c 'exit 0'", &result, OutputFormat::Json)
        .await
        .unwrap();
    assert!(
        dzban_cli::pager::run_streamed("/nonexistent-dzb-test-pager", &result, OutputFormat::Json)
            .await
            .unwrap_err()
            .contains("Cannot start pager")
    );
}

#[test]
fn live_preview_fits_small_terminals_and_escapes_controls() {
    let fixture = fixture();
    let lines = dzban_cli::streamed_output::preview_lines(&fixture, 40, 3);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("Row 1: id: 0"));
    for line in lines {
        assert!(unicode_width::UnicodeWidthStr::width(line.as_str()) <= 39);
        assert!(!line.contains('\n'));
        assert!(!line.contains('\x1b'));
    }
}
