use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell, RwLock};
use tokio_postgres::Client;
use tokio_postgres_rustls::MakeRustlsConnect;
use tracing::{error, warn};

use crate::config::{DatabaseBackend, DatabaseConfig};
use crate::postgres_value::{display_value, is_null};
use crate::result::{QueryEstimate, QueryPlan, ResultCell, ResultColumn, ResultSet};

#[async_trait]
trait DatabaseSession: Send + Sync {
    async fn connect(&self) -> Result<(), String>;
    async fn ping(&self) -> Result<u64, String>;
    async fn execute(&self, query: &str) -> Result<ResultSet, String>;
    async fn cancel(&self) -> Result<(), String>;
    async fn version(&self) -> Result<String, String>;
    async fn explain(&self, query: &str) -> Result<QueryPlan, String>;
}

#[derive(Clone)]
pub struct InternalConnection {
    session: Arc<dyn DatabaseSession>,
    backend: DatabaseBackend,
}

impl InternalConnection {
    pub fn new(config: DatabaseConfig) -> Result<Self, String> {
        config.validate()?;
        let backend = config.backend;
        let session: Arc<dyn DatabaseSession> = match backend {
            DatabaseBackend::Postgresql => Arc::new(PostgresSession::new(config)),
            DatabaseBackend::Clickhouse => Arc::new(ClickHouseSession::new(config)?),
        };
        Ok(Self { session, backend })
    }

    pub fn backend(&self) -> DatabaseBackend {
        self.backend
    }

    pub async fn connect(&self) -> Result<(), String> {
        self.session.connect().await
    }

    pub async fn ping(&self) -> Result<u64, String> {
        self.session.ping().await
    }

    pub async fn execute_query_rows(&self, query: &str) -> Result<ResultSet, String> {
        self.session.execute(query).await
    }

    pub async fn cancel_query(&self) -> Result<(), String> {
        self.session.cancel().await
    }

    pub async fn get_version(&self) -> Result<String, String> {
        self.session.version().await
    }

    pub async fn explain(&self, query: &str) -> Result<QueryPlan, String> {
        self.session.explain(query).await
    }

    pub async fn test_connection(config: &DatabaseConfig) -> Result<String, String> {
        let connection = Self::new(config.clone())?;
        connection.connect().await?;
        connection.get_version().await
    }
}

struct PostgresSession {
    config: DatabaseConfig,
    client: RwLock<Option<Arc<Client>>>,
}

impl PostgresSession {
    fn new(config: DatabaseConfig) -> Self {
        Self {
            config,
            client: RwLock::new(None),
        }
    }

    async fn client(&self) -> Result<Arc<Client>, String> {
        self.client
            .read()
            .await
            .clone()
            .ok_or_else(|| "Not connected".to_string())
    }
}

#[async_trait]
impl DatabaseSession for PostgresSession {
    async fn connect(&self) -> Result<(), String> {
        let tls = tls_connector()?;
        match tokio_postgres::connect(&self.config.connection_string(), tls).await {
            Ok((client, connection)) => {
                tokio::spawn(async move {
                    if let Err(error) = connection.await {
                        error!("Connection error: {}", error);
                    }
                });
                *self.client.write().await = Some(Arc::new(client));
                Ok(())
            }
            Err(error) => Err(format_postgres_error(&error)),
        }
    }

    async fn ping(&self) -> Result<u64, String> {
        let client = self.client().await?;
        let start = std::time::Instant::now();
        client
            .simple_query("SELECT 1")
            .await
            .map_err(|error| format_postgres_error(&error))?;
        Ok(start.elapsed().as_micros() as u64)
    }

    async fn execute(&self, query: &str) -> Result<ResultSet, String> {
        let rows = self
            .client()
            .await?
            .query(query, &[])
            .await
            .map_err(|error| format_postgres_error(&error))?;
        let columns = rows
            .first()
            .map(|row| {
                row.columns()
                    .iter()
                    .map(|column| ResultColumn {
                        name: column.name().to_string(),
                        type_name: column.type_().name().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let rows = rows
            .iter()
            .map(|row| {
                row.columns()
                    .iter()
                    .enumerate()
                    .map(|(index, _)| {
                        if is_null(row, index) {
                            ResultCell::Null
                        } else {
                            ResultCell::Text(display_value(row, index))
                        }
                    })
                    .collect()
            })
            .collect();
        Ok(ResultSet { columns, rows })
    }

    async fn cancel(&self) -> Result<(), String> {
        self.client()
            .await?
            .cancel_token()
            .cancel_query(tls_connector()?)
            .await
            .map_err(|error| format_postgres_error(&error))
    }

    async fn version(&self) -> Result<String, String> {
        self.client()
            .await?
            .query_one("SELECT version()", &[])
            .await
            .map(|row| row.get(0))
            .map_err(|error| format_postgres_error(&error))
    }

    async fn explain(&self, query: &str) -> Result<QueryPlan, String> {
        let client = self.client().await?;
        let row = client
            .query_one(&format!("EXPLAIN (FORMAT JSON) {query}"), &[])
            .await
            .map_err(|error| format_postgres_error(&error))?;
        let document: Value = row
            .try_get(0)
            .map_err(|error| format!("EXPLAIN returned an invalid JSON plan: {error}"))?;
        let root = document
            .as_array()
            .and_then(|plans| plans.first())
            .and_then(|root| root.get("Plan"));
        Ok(QueryPlan {
            backend: DatabaseBackend::Postgresql,
            estimate: QueryEstimate {
                rows: root
                    .and_then(|plan| plan.get("Plan Rows"))
                    .and_then(Value::as_u64),
                startup_cost: root
                    .and_then(|plan| plan.get("Startup Cost"))
                    .and_then(Value::as_f64),
                total_cost: root
                    .and_then(|plan| plan.get("Total Cost"))
                    .and_then(Value::as_f64),
                ..QueryEstimate::default()
            },
            document,
        })
    }
}

struct ClickHouseSession {
    config: DatabaseConfig,
    client: OnceCell<reqwest::Client>,
    active_queries: Mutex<Vec<String>>,
}

impl ClickHouseSession {
    fn new(config: DatabaseConfig) -> Result<Self, String> {
        Ok(Self {
            config,
            client: OnceCell::new(),
            active_queries: Mutex::new(Vec::new()),
        })
    }

    async fn client(&self) -> Result<&reqwest::Client, String> {
        self.client
            .get_or_try_init(|| async {
                reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .build()
                    .map_err(|error| format!("Could not create ClickHouse HTTP client: {error}"))
            })
            .await
    }

    fn request(
        &self,
        client: &reqwest::Client,
        query: &str,
        query_id: &str,
    ) -> reqwest::RequestBuilder {
        client
            .post(
                self.config
                    .url
                    .as_deref()
                    .expect("validated ClickHouse URL"),
            )
            .basic_auth(&self.config.user, self.config.password.as_deref())
            .header("X-ClickHouse-Format", "JSONCompactEachRowWithNamesAndTypes")
            .query(&[
                ("database", self.config.database.as_str()),
                ("query_id", query_id),
                ("wait_end_of_query", "1"),
                ("output_format_json_quote_64bit_integers", "1"),
                ("output_format_json_quote_decimals", "1"),
            ])
            .body(query.to_string())
    }

    async fn execute_with_tracking(&self, query: &str, track: bool) -> Result<ResultSet, String> {
        let client = self.client().await?;
        let query_id = uuid::Uuid::new_v4().to_string();
        if track {
            self.active_queries.lock().await.push(query_id.clone());
        }
        let response = self.request(client, query, &query_id).send().await;
        let response_body = match response {
            Ok(response) => {
                let status = response.status();
                let exception_code = response
                    .headers()
                    .get("X-ClickHouse-Exception-Code")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                response
                    .text()
                    .await
                    .map(|body| (status, exception_code, body))
                    .map_err(|error| format_error_chain(&error))
            }
            Err(error) => Err(format_error_chain(&error)),
        };
        if track {
            self.active_queries
                .lock()
                .await
                .retain(|active| active != &query_id);
        }
        let (status, exception_code, body) = response_body?;
        if !status.is_success() {
            return Err(format_clickhouse_error(
                status,
                exception_code.as_deref(),
                &body,
            ));
        }
        parse_clickhouse_result(&body)
    }
}

#[async_trait]
impl DatabaseSession for ClickHouseSession {
    async fn connect(&self) -> Result<(), String> {
        self.version().await.map(|_| ())
    }

    async fn ping(&self) -> Result<u64, String> {
        let start = std::time::Instant::now();
        self.execute_with_tracking("SELECT 1", false).await?;
        Ok(start.elapsed().as_micros() as u64)
    }

    async fn execute(&self, query: &str) -> Result<ResultSet, String> {
        self.execute_with_tracking(query, true).await
    }

    async fn cancel(&self) -> Result<(), String> {
        let query_id = self
            .active_queries
            .lock()
            .await
            .last()
            .cloned()
            .ok_or_else(|| "No ClickHouse query is running".to_string())?;
        self.execute_with_tracking(
            &format!("KILL QUERY WHERE query_id = '{query_id}' SYNC"),
            false,
        )
        .await
        .map(|_| ())
    }

    async fn version(&self) -> Result<String, String> {
        let result = self
            .execute_with_tracking("SELECT version()", false)
            .await?;
        result
            .rows
            .first()
            .and_then(|row| row.first())
            .map(|cell| cell.display().to_string())
            .ok_or_else(|| "ClickHouse returned no server version".to_string())
    }

    async fn explain(&self, query: &str) -> Result<QueryPlan, String> {
        let plan_result = self
            .execute_with_tracking(
                &format!("EXPLAIN json = 1, description = 1, indexes = 1 {query}"),
                false,
            )
            .await?;
        let plan = plan_result
            .rows
            .first()
            .and_then(|row| row.first())
            .map(ResultCell::display)
            .ok_or_else(|| "ClickHouse EXPLAIN returned no plan".to_string())?;
        let document = serde_json::from_str(plan)
            .map_err(|error| format!("ClickHouse EXPLAIN returned invalid JSON: {error}"))?;
        let estimate_result = self
            .execute_with_tracking(&format!("EXPLAIN ESTIMATE {query}"), false)
            .await;
        let estimate = estimate_result
            .as_ref()
            .map(clickhouse_estimate)
            .unwrap_or_default();
        Ok(QueryPlan {
            backend: DatabaseBackend::Clickhouse,
            document,
            estimate,
        })
    }
}

fn parse_clickhouse_result(body: &str) -> Result<ResultSet, String> {
    if body.trim().is_empty() {
        return Ok(ResultSet::default());
    }
    let mut lines = body.lines();
    let names: Vec<String> = serde_json::from_str(
        lines
            .next()
            .ok_or_else(|| "ClickHouse response omitted column names".to_string())?,
    )
    .map_err(|error| format!("Could not parse ClickHouse column names: {error}"))?;
    let types: Vec<String> = serde_json::from_str(
        lines
            .next()
            .ok_or_else(|| "ClickHouse response omitted column types".to_string())?,
    )
    .map_err(|error| format!("Could not parse ClickHouse column types: {error}"))?;
    if names.len() != types.len() {
        return Err("ClickHouse returned mismatched column names and types".to_string());
    }
    let columns = names
        .into_iter()
        .zip(types)
        .map(|(name, type_name)| ResultColumn { name, type_name })
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let values: Vec<Value> = serde_json::from_str(line)
            .map_err(|error| format!("Could not parse ClickHouse result row: {error}"))?;
        if values.len() != columns.len() {
            return Err("ClickHouse returned a row with the wrong column count".to_string());
        }
        rows.push(values.into_iter().map(clickhouse_cell).collect());
    }
    Ok(ResultSet { columns, rows })
}

fn clickhouse_cell(value: Value) -> ResultCell {
    match value {
        Value::Null => ResultCell::Null,
        Value::String(value) => ResultCell::Text(value),
        value => ResultCell::Text(value.to_string()),
    }
}

fn clickhouse_estimate(result: &ResultSet) -> QueryEstimate {
    let column = |name: &str| result.columns.iter().position(|column| column.name == name);
    let sum = |name: &str| {
        column(name).map(|index| {
            result
                .rows
                .iter()
                .filter_map(|row| row.get(index))
                .filter_map(|cell| cell.display().parse::<u64>().ok())
                .sum()
        })
    };
    QueryEstimate {
        rows: sum("rows"),
        parts: sum("parts"),
        marks: sum("marks"),
        ..QueryEstimate::default()
    }
}

fn format_clickhouse_error(status: StatusCode, code: Option<&str>, body: &str) -> String {
    let decoded = body
        .lines()
        .filter_map(|line| serde_json::from_str::<Vec<String>>(line).ok())
        .flatten()
        .filter(|message| !message.trim().is_empty())
        .collect::<Vec<_>>();
    let message = decoded
        .iter()
        .find(|message| message.contains("Code:"))
        .or_else(|| decoded.last())
        .cloned()
        .unwrap_or_else(|| body.trim().to_string())
        .chars()
        .take(4096)
        .collect::<String>();
    match code {
        Some(code) => format!("ClickHouse error {code} ({status}): {message}"),
        None => format!("ClickHouse request failed ({status}): {message}"),
    }
}

fn tls_connector() -> Result<MakeRustlsConnect, String> {
    match MakeRustlsConnect::with_native_certs() {
        Ok((connector, errors)) => {
            if !errors.is_empty() {
                warn!(
                    "Some system TLS certificates could not be loaded: {:?}",
                    errors
                );
            }
            Ok(connector)
        }
        Err(errors) => Err(format!(
            "Could not load the system TLS certificate store: {:?}",
            errors
        )),
    }
}

fn format_postgres_error(error: &tokio_postgres::Error) -> String {
    if let Some(database_error) = error.as_db_error() {
        let mut message = format!(
            "{} (SQLSTATE {})",
            database_error.message(),
            database_error.code().code()
        );
        if let Some(detail) = database_error.detail() {
            message.push_str(&format!(". Detail: {detail}"));
        }
        if let Some(hint) = database_error.hint() {
            message.push_str(&format!(". Hint: {hint}"));
        }
        return message;
    }
    format_error_chain(error)
}

fn format_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut messages = Vec::new();
    let mut current = Some(error);
    while let Some(error) = current {
        let message = error.to_string();
        if messages.last() != Some(&message) {
            messages.push(message);
        }
        current = error.source();
    }
    messages.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clickhouse_compact_rows() {
        let result = parse_clickhouse_result(
            "[\"id\",\"payload\",\"missing\"]\n[\"UInt64\",\"Map(String, UInt8)\",\"Nullable(String)\"]\n[\"18446744073709551615\",{\"a\":1},null]\n",
        )
        .unwrap();
        assert_eq!(result.columns[0].type_name, "UInt64");
        assert_eq!(result.rows[0][0].display(), "18446744073709551615");
        assert_eq!(result.rows[0][1].display(), "{\"a\":1}");
        assert_eq!(result.rows[0][2], ResultCell::Null);
    }

    #[test]
    fn clickhouse_estimates_are_summed_across_tables() {
        let result = ResultSet {
            columns: ["database", "table", "parts", "rows", "marks"]
                .into_iter()
                .map(|name| ResultColumn {
                    name: name.to_string(),
                    type_name: "UInt64".to_string(),
                })
                .collect(),
            rows: vec![
                ["default", "a", "1", "10", "2"]
                    .into_iter()
                    .map(|value| ResultCell::Text(value.to_string()))
                    .collect(),
                ["default", "b", "2", "20", "3"]
                    .into_iter()
                    .map(|value| ResultCell::Text(value.to_string()))
                    .collect(),
            ],
        };
        let estimate = clickhouse_estimate(&result);
        assert_eq!(estimate.rows, Some(30));
        assert_eq!(estimate.parts, Some(3));
        assert_eq!(estimate.marks, Some(5));
    }

    #[test]
    fn clickhouse_error_omits_format_header_rows() {
        let error = format_clickhouse_error(
            StatusCode::NOT_FOUND,
            Some("60"),
            "[]\n[]\n[\"Code: 60. Unknown table events\"]\n",
        );

        assert_eq!(
            error,
            "ClickHouse error 60 (404 Not Found): Code: 60. Unknown table events"
        );
    }
}
