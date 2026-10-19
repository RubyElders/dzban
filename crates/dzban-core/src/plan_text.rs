use crate::QueryPlan;
use serde_json::{Map, Value};
use std::fmt::Write;

impl QueryPlan {
    pub fn text(&self) -> String {
        plan_text(&self.document)
    }
}

pub fn plan_text(value: &Value) -> String {
    let mut output = String::new();
    document(value, 0, &mut output);
    if output.is_empty() {
        return "No plan details available".into();
    }
    output.trim_end().to_string()
}

fn document(value: &Value, depth: usize, output: &mut String) {
    match value {
        Value::Array(values) => {
            for value in values {
                document(value, depth, output);
            }
        }
        Value::Object(fields) if fields.contains_key("Plan") => {
            document(&fields["Plan"], depth, output);
            properties(fields, &["Plan"], depth, output);
        }
        Value::Object(fields) if fields.contains_key("Node Type") => node(fields, depth, output),
        Value::Object(fields) => properties(fields, &[], depth, output),
        value => {
            let _ = writeln!(output, "{}{}", "  ".repeat(depth), scalar(value));
        }
    }
}

fn node(fields: &Map<String, Value>, depth: usize, output: &mut String) {
    let mut title = scalar(&fields["Node Type"]);
    if fields.get("Parallel Aware").and_then(Value::as_bool) == Some(true) {
        title.insert_str(0, "Parallel ");
    }
    if let Some(index) = fields.get("Index Name") {
        let _ = write!(title, " using {}", scalar(index));
    }
    if let Some(relation) = fields.get("Relation Name") {
        title.push_str(" on ");
        if let Some(schema) = fields.get("Schema") {
            let _ = write!(title, "{}.", scalar(schema));
        }
        title.push_str(&scalar(relation));
        if let Some(alias) = fields.get("Alias").filter(|alias| *alias != relation) {
            let _ = write!(title, " {}", scalar(alias));
        }
    }
    let mut metrics = Vec::new();
    if let (Some(start), Some(total)) = (
        fields.get("Startup Cost").and_then(Value::as_f64),
        fields.get("Total Cost").and_then(Value::as_f64),
    ) {
        metrics.push(format!("cost={start:.2}..{total:.2}"));
    }
    if let Some(rows) = fields.get("Plan Rows") {
        metrics.push(format!("rows={}", scalar(rows)));
    }
    if let Some(width) = fields.get("Plan Width") {
        metrics.push(format!("width={}", scalar(width)));
    }
    if !metrics.is_empty() {
        let _ = write!(title, "  ({})", metrics.join(" "));
    }
    let _ = writeln!(
        output,
        "{}{}{title}",
        "  ".repeat(depth),
        if depth > 0 { "->  " } else { "" }
    );
    let mut rendered = vec!["Node Type", "Parallel Aware", "Plans"];
    if fields.contains_key("Index Name") {
        rendered.push("Index Name");
    }
    if fields.contains_key("Relation Name") {
        rendered.extend(["Relation Name", "Schema", "Alias"]);
    }
    if fields.get("Startup Cost").and_then(Value::as_f64).is_some()
        && fields.get("Total Cost").and_then(Value::as_f64).is_some()
    {
        rendered.extend(["Startup Cost", "Total Cost"]);
    }
    rendered.extend(["Plan Rows", "Plan Width"]);
    properties(fields, &rendered, depth + 1, output);
    if let Some(children) = fields.get("Plans") {
        document(children, depth + 1, output);
    }
}

fn properties(fields: &Map<String, Value>, excluded: &[&str], depth: usize, output: &mut String) {
    for (name, value) in fields {
        if excluded.contains(&name.as_str()) {
            continue;
        }
        let indent = "  ".repeat(depth);
        match value {
            Value::Object(_) => {
                let _ = writeln!(output, "{indent}{name}:");
                document(value, depth + 1, output);
            }
            Value::Array(values) if values.iter().any(|v| v.is_object() || v.is_array()) => {
                let _ = writeln!(output, "{indent}{name}:");
                document(value, depth + 1, output);
            }
            value => {
                let _ = writeln!(output, "{indent}{name}: {}", scalar(value));
            }
        }
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(values) => values.iter().map(scalar).collect::<Vec<_>>().join(", "),
        Value::Null => "NULL".into(),
        value => value.to_string(),
    }
}
