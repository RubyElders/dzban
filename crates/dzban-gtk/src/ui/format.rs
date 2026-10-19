use dzban_core::DatabaseBackend;
use dzban_core::health::HealthState;
use dzban_core::presentation::{
    DocumentPresentation, EmptyResultReason, PlanPresentation, ResultStatusReason,
};

pub fn latency(micros: Option<u64>) -> String {
    match micros {
        None => "--".into(),
        Some(micros) if micros < 1_000 => format!("{micros}µs"),
        Some(micros) if micros < 1_000_000 => format!("{}ms", micros / 1_000),
        Some(micros) => format!("{:.2}s", micros as f64 / 1_000_000.0),
    }
}

pub fn health_label(state: HealthState) -> &'static str {
    match state {
        HealthState::Unknown => "Unknown",
        HealthState::Connected => "Connected",
        HealthState::Error => "Error",
    }
}

pub fn short_version(version: Option<&str>) -> String {
    version
        .map(|version| {
            version
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "--".into())
}

pub fn result_status(document: &DocumentPresentation) -> String {
    let feedback = &document.feedback;
    let rows = feedback.rows;
    match feedback.status {
        ResultStatusReason::Ready => "Run a query to see results".into(),
        ResultStatusReason::Planning => "Planning query".into(),
        ResultStatusReason::Waiting => "Waiting for results".into(),
        ResultStatusReason::WaitingForSchema => "Waiting for schema".into(),
        ResultStatusReason::Receiving => format!("Receiving - {rows} rows"),
        ResultStatusReason::Draining => {
            format!("Truncated - {rows} rows; waiting for cleanup")
        }
        ResultStatusReason::CaptureFailedAwaitingCleanup => {
            "Result capture failed; waiting for cleanup".into()
        }
        ResultStatusReason::Finishing => format!("Finishing - {rows} rows"),
        ResultStatusReason::Canceling => format!("Canceling - {rows} rows captured"),
        ResultStatusReason::LimitedByRows => format!("Truncated - {rows} rows (row limit)"),
        ResultStatusReason::LimitedByBytes => format!("Truncated - {rows} rows (byte limit)"),
        ResultStatusReason::Canceled => format!("Canceled - {rows} rows"),
        ResultStatusReason::Failed => format!("Failed - {rows} rows"),
        ResultStatusReason::Command => feedback
            .affected_rows
            .map(|affected| format!("{affected} rows affected"))
            .unwrap_or_else(|| "Command completed".into()),
        ResultStatusReason::Complete => format!("Complete - {rows} rows"),
        ResultStatusReason::SavedFailed => "Saved query failed".into(),
        ResultStatusReason::SavedCommand => feedback
            .affected_rows
            .map(|affected| format!("Saved command - {affected} rows affected"))
            .unwrap_or_else(|| "Saved command".into()),
        ResultStatusReason::SavedRows => format!(
            "Saved - {rows} of {} rows",
            feedback.total_rows.unwrap_or(rows)
        ),
    }
}

pub fn empty_title(reason: EmptyResultReason) -> &'static str {
    match reason {
        EmptyResultReason::Ready => "Ready to query",
        EmptyResultReason::Command => "Command complete",
        EmptyResultReason::Canceled => "Query stopped",
        EmptyResultReason::NoRows => "No rows returned",
    }
}

pub fn empty_detail(document: &DocumentPresentation) -> String {
    match document.feedback.empty {
        EmptyResultReason::Ready => "Run your SQL to explore the results.".into(),
        EmptyResultReason::Command => document
            .feedback
            .affected_rows
            .map(|affected| format!("{affected} rows affected."))
            .unwrap_or_else(|| "The command completed successfully.".into()),
        EmptyResultReason::Canceled => "No rows were returned before the query stopped.".into(),
        EmptyResultReason::NoRows => "The query completed without returning any rows.".into(),
    }
}

pub fn empty_icon(reason: EmptyResultReason) -> &'static str {
    match reason {
        EmptyResultReason::Ready => "x-office-spreadsheet-symbolic",
        EmptyResultReason::Command => "object-select-symbolic",
        EmptyResultReason::Canceled | EmptyResultReason::NoRows => "view-list-symbolic",
    }
}

pub fn compact_estimate(plan: &PlanPresentation) -> String {
    let Some(estimate) = &plan.estimate else {
        return "Estimate ready".into();
    };
    let Some(rows) = estimate.rows else {
        return "Estimate ready".into();
    };
    match (
        estimate.startup_cost,
        estimate.total_cost,
        estimate.parts,
        estimate.marks,
    ) {
        (Some(startup), Some(total), _, _) => {
            format!("~{rows} rows - cost {startup:.2}..{total:.2}")
        }
        (_, _, Some(parts), Some(marks)) => format!("~{rows} rows - {parts} parts - {marks} marks"),
        _ => format!("~{rows} rows"),
    }
}

pub fn plan_summary(plan: &PlanPresentation) -> String {
    let Some(estimate) = &plan.estimate else {
        return "Estimated query plan".into();
    };
    let mut parts = Vec::new();
    if let Some(rows) = estimate.rows {
        parts.push(format!("Estimated rows: {rows}"));
    }
    if let Some(cost) = estimate.startup_cost {
        parts.push(format!("Startup cost: {cost}"));
    }
    if let Some(cost) = estimate.total_cost {
        parts.push(format!("Total cost: {cost}"));
    }
    if let Some(value) = estimate.parts {
        parts.push(format!("Parts: {value}"));
    }
    if let Some(value) = estimate.marks {
        parts.push(format!("Marks: {value}"));
    }
    if let Some(diagnostic) = &estimate.diagnostic {
        parts.push(format!("Estimate unavailable: {}", diagnostic.text));
    }
    if parts.is_empty() {
        "Estimated query plan".into()
    } else {
        parts.join(" | ")
    }
}

pub fn row_estimate(backend: DatabaseBackend, rows: u64) -> String {
    match backend {
        DatabaseBackend::Clickhouse => format!("Scan est. ~{rows} rows"),
        DatabaseBackend::Postgresql => format!("Est. ~{rows} rows"),
    }
}

pub fn cell_preview(value: &str, limit: usize) -> String {
    let mut preview = String::with_capacity(value.len().min(limit + 3));
    let mut count = 0;
    for character in value.chars() {
        let escaped = match character {
            '\n' => "\\n",
            '\r' => "\\r",
            '\t' => "\\t",
            _ if character.is_control() => continue,
            _ => {
                preview.push(character);
                count += 1;
                if count >= limit {
                    preview.push_str("...");
                    return preview;
                }
                continue;
            }
        };
        preview.push_str(escaped);
        count += 2;
        if count >= limit {
            preview.push_str("...");
            return preview;
        }
    }
    preview
}

pub fn executed_at(seconds: u64) -> String {
    glib::DateTime::from_unix_local(i64::try_from(seconds).unwrap_or(i64::MAX))
        .and_then(|time| time.format("%Y-%m-%d %H:%M"))
        .map(|text| text.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_uses_the_native_units() {
        assert_eq!(latency(None), "--");
        assert_eq!(latency(Some(999)), "999µs");
        assert_eq!(latency(Some(12_345)), "12ms");
        assert_eq!(latency(Some(2_500_000)), "2.50s");
    }

    #[test]
    fn cell_previews_escape_line_breaks_and_mark_truncation() {
        assert_eq!(cell_preview("a\nb\tc", 100), "a\\nb\\tc");
        assert_eq!(
            cell_preview(&"x".repeat(20), 10),
            format!("{}...", "x".repeat(10))
        );
    }

    #[test]
    fn versions_keep_the_first_two_words() {
        assert_eq!(
            short_version(Some("PostgreSQL 15.4 on x86_64")),
            "PostgreSQL 15.4"
        );
        assert_eq!(short_version(None), "--");
    }
}
