use std::io::Write;

use dzban_core::{ResultCell, ResultSet};
use unicode_width::UnicodeWidthStr;

use crate::{CliError, args::OutputFormat};

pub fn catalog(entries: Vec<dzban_core::catalog::CatalogEntry>) -> ResultSet {
    let attributes = entries
        .iter()
        .flat_map(|entry| entry.attributes.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    let mut names = vec![
        "Database".to_string(),
        "Schema".into(),
        "Name".into(),
        "Type".into(),
    ];
    names.extend(attributes.iter().cloned());
    ResultSet {
        columns: names
            .into_iter()
            .map(|name| dzban_core::ResultColumn {
                name,
                type_name: "text".into(),
            })
            .collect(),
        rows: entries
            .into_iter()
            .map(|entry| {
                let mut row = vec![
                    ResultCell::Text(entry.identity.database),
                    entry
                        .identity
                        .schema
                        .map_or(ResultCell::Null, ResultCell::Text),
                    ResultCell::Text(entry.identity.name),
                    ResultCell::Text(format!("{:?}", entry.identity.kind)),
                ];
                row.extend(attributes.iter().map(|name| {
                    entry
                        .attributes
                        .get(name)
                        .cloned()
                        .unwrap_or(ResultCell::Null)
                }));
                row
            })
            .collect(),
    }
}

pub fn timing(
    total: std::time::Duration,
    planning: std::time::Duration,
    execution: std::time::Duration,
) -> String {
    format!(
        "Time: {:.3} ms total | Planning: {:.3} ms | Execution: {:.3} ms",
        total.as_secs_f64() * 1000.0,
        planning.as_secs_f64() * 1000.0,
        execution.as_secs_f64() * 1000.0
    )
}

pub fn render_query(
    result: &dzban_core::QueryResult,
    format: OutputFormat,
) -> Result<String, CliError> {
    let Some(command) = &result.command else {
        return render(&result.result_set, format);
    };
    match format {
        OutputFormat::Json => serde_json::to_string(&serde_json::json!({ "command": command }))
            .map(|text| format!("{text}\n"))
            .map_err(|error| CliError::runtime(error.to_string())),
        OutputFormat::Csv => Ok(format!(
            "affected_rows\n{}\n",
            command
                .affected_rows
                .map_or_else(String::new, |rows| rows.to_string())
        )),
        _ => Ok(command.affected_rows.map_or_else(
            || "Command completed\n".into(),
            |rows| format!("Command completed ({rows} affected rows)\n"),
        )),
    }
}

pub fn connection_summary(profile: &dzban_core::DatabaseConfig) -> String {
    let field = |text: &str| crate::editor_view::single_line(text, usize::MAX);
    format!(
        "{} | {:?} | database: {} | user: {} | host: {} | port: {}",
        field(&profile.name),
        profile.backend,
        field(&profile.database),
        field(&profile.user),
        field(profile.host.as_deref().unwrap_or("HTTP/local")),
        profile
            .port
            .map_or_else(|| "default".into(), |port| port.to_string()),
    )
}

pub fn estimate(plan: &dzban_core::QueryPlan) -> String {
    let mut fields = Vec::new();
    if let Some(rows) = plan.estimate.rows {
        let label = match plan.backend {
            dzban_core::DatabaseBackend::Postgresql => "estimated rows",
            dzban_core::DatabaseBackend::Clickhouse => "estimated scan rows",
        };
        fields.push(format!("{label}: {rows}"));
    }
    if let Some(cost) = plan.estimate.total_cost {
        fields.push(format!("cost: {cost:.2}"));
    }
    if let Some(parts) = plan.estimate.parts {
        fields.push(format!("parts: {parts}"));
    }
    if let Some(marks) = plan.estimate.marks {
        fields.push(format!("marks: {marks}"));
    }
    fields.join(" | ")
}

pub fn render(result: &ResultSet, format: OutputFormat) -> Result<String, CliError> {
    match format {
        OutputFormat::Json => serde_json::to_string(result)
            .map(|text| format!("{text}\n"))
            .map_err(|error| CliError::runtime(error.to_string())),
        OutputFormat::Csv => {
            fn field(text: &str) -> String {
                format!("\"{}\"", text.replace('"', "\"\""))
            }
            let mut text = result
                .columns
                .iter()
                .map(|column| field(&column.name))
                .collect::<Vec<_>>()
                .join(",");
            text.push('\n');
            for row in &result.rows {
                text.push_str(
                    &row.iter()
                        .map(|cell| match cell {
                            ResultCell::Null => String::new(),
                            ResultCell::Text(value) => field(value),
                        })
                        .collect::<Vec<_>>()
                        .join(","),
                );
                text.push('\n');
            }
            Ok(text)
        }
        OutputFormat::Expanded => {
            let mut text = String::new();
            for (index, row) in result.rows.iter().enumerate() {
                text.push_str(&format!("Record {}\n", index + 1));
                for (column, cell) in result.columns.iter().zip(row) {
                    text.push_str(&format!("{}: {}\n", column.name, cell.display()));
                }
            }
            Ok(text)
        }
        OutputFormat::Table => {
            fn single_line(text: &str) -> String {
                text.chars()
                    .flat_map(|character| match character {
                        '\n' => "\\n".chars().collect::<Vec<_>>(),
                        '\r' => "\\r".chars().collect(),
                        '\t' => "\\t".chars().collect(),
                        c if c.is_control() => format!("\\u{{{:x}}}", c as u32).chars().collect(),
                        c => vec![c],
                    })
                    .collect()
            }
            let headings: Vec<_> = result
                .columns
                .iter()
                .map(|column| single_line(&column.name))
                .collect();
            let rows: Vec<Vec<_>> = result
                .rows
                .iter()
                .map(|row| row.iter().map(|cell| single_line(cell.display())).collect())
                .collect();
            let widths: Vec<_> = headings
                .iter()
                .enumerate()
                .map(|(index, heading)| {
                    rows.iter()
                        .filter_map(|row| row.get(index))
                        .map(|value| value.width())
                        .chain(std::iter::once(heading.width()))
                        .max()
                        .unwrap_or(0)
                })
                .collect();
            let border = format!(
                "+{}+\n",
                widths
                    .iter()
                    .map(|width| "-".repeat(width + 2))
                    .collect::<Vec<_>>()
                    .join("+")
            );
            let mut text = border.clone();
            for row in std::iter::once(&headings).chain(rows.iter()) {
                text.push('|');
                for (index, width) in widths.iter().enumerate() {
                    let value = row.get(index).map(String::as_str).unwrap_or("");
                    text.push_str(&format!(
                        " {value}{} |",
                        " ".repeat(width.saturating_sub(value.width()))
                    ));
                }
                text.push('\n');
                if std::ptr::eq(row, &headings) {
                    text.push_str(&border);
                }
            }
            text.push_str(&border);
            text.push_str(&format!("({} rows)\n", result.rows.len()));
            Ok(text)
        }
    }
}

pub fn write(text: &str) -> Result<(), CliError> {
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(CliError::runtime(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_output_preserves_native_attributes_missing_values_and_empty_schema() {
        use dzban_core::catalog::{CatalogEntry, ObjectIdentity, ObjectKind};
        let entry = |name: &str, attributes| CatalogEntry {
            identity: ObjectIdentity {
                database: "fixture".into(),
                schema: None,
                kind: ObjectKind::Table,
                name: name.into(),
            },
            attributes,
        };
        let result = catalog(vec![
            entry(
                "odd\nname",
                [("engine".into(), ResultCell::Text("Memory".into()))].into(),
            ),
            entry(
                "other",
                [("owner".into(), ResultCell::Text("reader".into()))].into(),
            ),
        ]);
        assert_eq!(
            result
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["Database", "Schema", "Name", "Type", "engine", "owner"]
        );
        assert_eq!(result.rows[0][1], ResultCell::Null);
        assert_eq!(result.rows[0][4], ResultCell::Text("Memory".into()));
        assert_eq!(result.rows[0][5], ResultCell::Null);
        assert_eq!(result.rows[1][4], ResultCell::Null);
        assert_eq!(result.rows[1][5], ResultCell::Text("reader".into()));
        let restored: ResultSet =
            serde_json::from_str(&render(&result, OutputFormat::Json).unwrap()).unwrap();
        assert_eq!(restored, result);
        assert!(
            render(&result, OutputFormat::Csv)
                .unwrap()
                .contains("\"odd\nname\"")
        );
        assert!(
            render(&result, OutputFormat::Table)
                .unwrap()
                .contains("odd\\nname")
        );
        let empty = catalog(Vec::new());
        assert_eq!(empty.columns.len(), 4);
        assert!(
            render(&empty, OutputFormat::Table)
                .unwrap()
                .contains("(0 rows)")
        );
    }

    #[test]
    fn commands_are_acknowledged_instead_of_rendered_as_empty_tables() {
        let mut result = dzban_core::QueryResult {
            result_set: ResultSet::default(),
            duration: std::time::Duration::ZERO,
            command: Some(dzban_core::CommandAcknowledgment {
                affected_rows: Some(2),
            }),
        };
        assert_eq!(
            render_query(&result, OutputFormat::Table).unwrap(),
            "Command completed (2 affected rows)\n"
        );
        assert_eq!(
            render_query(&result, OutputFormat::Csv).unwrap(),
            "affected_rows\n2\n"
        );
        let json: serde_json::Value =
            serde_json::from_str(&render_query(&result, OutputFormat::Json).unwrap()).unwrap();
        assert_eq!(json["command"]["affected_rows"], 2);
        result.command.as_mut().unwrap().affected_rows = None;
        assert_eq!(
            render_query(&result, OutputFormat::Expanded).unwrap(),
            "Command completed\n"
        );
        assert_eq!(
            render_query(&result, OutputFormat::Csv).unwrap(),
            "affected_rows\n\n"
        );
    }

    #[test]
    fn timing_labels_total_and_planning_without_claiming_an_estimate() {
        use std::time::Duration;
        assert_eq!(
            timing(
                Duration::from_micros(1234),
                Duration::from_micros(234),
                Duration::from_micros(800)
            ),
            "Time: 1.234 ms total | Planning: 0.234 ms | Execution: 0.800 ms"
        );
        assert_eq!(
            timing(Duration::ZERO, Duration::ZERO, Duration::ZERO),
            "Time: 0.000 ms total | Planning: 0.000 ms | Execution: 0.000 ms"
        );
    }

    #[test]
    fn connection_details_exclude_authentication_and_escape_controls() {
        let profile = dzban_core::DatabaseConfig {
            name: "local\u{1b}[31m".into(),
            backend: dzban_core::DatabaseBackend::Clickhouse,
            host: Some("localhost".into()),
            port: Some(8123),
            user: "reader".into(),
            database: "default".into(),
            password: Some("secret-password".into()),
            url: Some("http://reader:secret-url@localhost:8123/?token=private".into()),
        };
        let text = connection_summary(&profile);
        assert!(text.contains("database: default"));
        assert!(text.contains("port: 8123"));
        assert!(text.contains("user: reader"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("token"));
        assert!(!text.contains('\u{1b}'));
    }
    use dzban_core::ResultColumn;

    #[test]
    fn estimates_keep_backend_meaning_and_omit_unknown_values() {
        let mut plan = dzban_core::QueryPlan {
            backend: dzban_core::DatabaseBackend::Postgresql,
            document: serde_json::Value::Null,
            estimate: dzban_core::QueryEstimate {
                rows: Some(42),
                total_cost: Some(12.5),
                ..Default::default()
            },
        };
        assert_eq!(estimate(&plan), "estimated rows: 42 | cost: 12.50");
        plan.backend = dzban_core::DatabaseBackend::Clickhouse;
        plan.estimate.total_cost = None;
        plan.estimate.parts = Some(2);
        assert_eq!(estimate(&plan), "estimated scan rows: 42 | parts: 2");
    }

    #[test]
    fn csv_distinguishes_null_empty_and_multiline_values() {
        let result = ResultSet {
            columns: vec![ResultColumn {
                name: "value".into(),
                type_name: "text".into(),
            }],
            rows: vec![
                vec![ResultCell::Null],
                vec![ResultCell::Text(String::new())],
                vec![ResultCell::Text("a\n\"b".into())],
            ],
        };
        assert_eq!(
            render(&result, OutputFormat::Csv).unwrap(),
            "\"value\"\n\n\"\"\n\"a\n\"\"b\"\n"
        );
        assert!(
            render(&result, OutputFormat::Table)
                .unwrap()
                .contains("a\\n\"b")
        );
    }
}
