use crate::config::{DatabaseBackend, DatabaseConfig};
use crate::errors::format_error_chain;
use crate::result::{QueryEstimate, QueryPlan, ResultCell, ResultColumn, ResultSet};
use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use reqwest::StatusCode;
use serde_json::Value;
use std::sync::Mutex;
use tokio::sync::OnceCell;

mod stream;

struct ActiveQuery<'a> {
    queries: &'a Mutex<Vec<QueryRegistration>>,
    id: String,
}

impl Drop for ActiveQuery<'_> {
    fn drop(&mut self) {
        self.queries
            .lock()
            .expect("active query lock")
            .retain(|query| query.id != self.id);
    }
}

#[derive(Clone, Copy)]
enum QueryStatus {
    Running,
    CancelRequested,
}

struct QueryRegistration {
    id: String,
    status: QueryStatus,
}

impl ActiveQuery<'_> {
    fn canceled(&self) -> bool {
        self.queries
            .lock()
            .expect("active query lock")
            .iter()
            .any(|query| {
                query.id == self.id && matches!(query.status, QueryStatus::CancelRequested)
            })
    }
}

pub(crate) struct ClickHouseSession {
    config: DatabaseConfig,
    client: OnceCell<reqwest::Client>,
    active_queries: Mutex<Vec<QueryRegistration>>,
    cancellation: tokio::sync::Mutex<()>,
}

impl ClickHouseSession {
    fn track(&self, id: &str) -> ActiveQuery<'_> {
        self.active_queries
            .lock()
            .expect("active query lock")
            .push(QueryRegistration {
                id: id.to_owned(),
                status: QueryStatus::Running,
            });
        ActiveQuery {
            queries: &self.active_queries,
            id: id.to_owned(),
        }
    }
    pub(crate) fn new(config: DatabaseConfig) -> Result<Self, String> {
        Ok(Self {
            config,
            client: OnceCell::new(),
            active_queries: Mutex::new(Vec::new()),
            cancellation: tokio::sync::Mutex::new(()),
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
        self.request_mode(client, query, query_id, "1")
    }

    fn request_mode(
        &self,
        client: &reqwest::Client,
        query: &str,
        query_id: &str,
        wait: &str,
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
                ("wait_end_of_query", wait),
                ("output_format_json_quote_64bit_integers", "1"),
                ("output_format_json_quote_decimals", "1"),
            ])
            .body(query.to_string())
    }

    async fn execute_with_tracking(&self, query: &str, track: bool) -> Result<ResultSet, String> {
        self.execute_tracked_detailed(query, track)
            .await
            .map_err(|error| error.to_string())
    }

    async fn execute_tracked_detailed(
        &self,
        query: &str,
        track: bool,
    ) -> Result<ResultSet, dzban_core::error::ExecutionError> {
        self.execute_parameters(query, track, &[]).await
    }

    async fn execute_parameters(
        &self,
        query: &str,
        track: bool,
        parameters: &[(String, String)],
    ) -> Result<ResultSet, dzban_core::error::ExecutionError> {
        use dzban_core::error::ExecutionError;
        let client = self.client().await?;
        let query_id = uuid::Uuid::new_v4().to_string();
        let _active = track.then(|| self.track(&query_id));
        let response = self
            .request(client, query, &query_id)
            .query(parameters)
            .send()
            .await;
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
                    .map_err(|error| ExecutionError::Database(clickhouse_transport_error(error)))
            }
            Err(error) => Err(ExecutionError::Database(clickhouse_transport_error(error))),
        };
        let (status, exception_code, body) = response_body?;
        if !status.is_success() {
            return Err(ExecutionError::Database(clickhouse_error(
                status,
                exception_code.as_deref(),
                &body,
            )));
        }
        let _cancellation = if track {
            Some(self.cancellation.lock().await)
        } else {
            None
        };
        if _active.as_ref().is_some_and(ActiveQuery::canceled) {
            return Err(ExecutionError::Canceled);
        }
        parse_clickhouse_result(&body).map_err(Into::into)
    }
}

#[async_trait]
impl DatabaseSession for ClickHouseSession {
    fn capabilities(&self) -> dzban_core::catalog::Capabilities {
        crate::catalog::capabilities(DatabaseBackend::Clickhouse)
    }

    async fn catalog(
        &self,
        request: dzban_core::catalog::CatalogRequest,
    ) -> Result<Vec<dzban_core::catalog::CatalogEntry>, dzban_core::catalog::CatalogError> {
        let query =
            crate::catalog::query(DatabaseBackend::Clickhouse, &self.config.database, &request)?;
        let result = self
            .execute_parameters(&query.sql, false, &query.clickhouse_parameters())
            .await
            .map_err(crate::catalog::error)?;
        crate::catalog::entries(
            DatabaseBackend::Clickhouse,
            &self.config.database,
            &request,
            result,
        )
    }
    async fn disconnect(&self) -> Result<(), dzban_core::error::ExecutionError> {
        Ok(())
    }
    async fn connect(&self) -> Result<(), String> {
        self.connect_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    async fn connect_detailed(&self) -> Result<(), dzban_core::error::ExecutionError> {
        self.execute_tracked_detailed("SELECT version()", false)
            .await
            .map(|_| ())
    }

    async fn ping(&self) -> Result<u64, String> {
        self.ping_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    async fn ping_detailed(&self) -> Result<u64, dzban_core::error::ExecutionError> {
        let start = std::time::Instant::now();
        self.execute_tracked_detailed("SELECT 1", false).await?;
        Ok(start.elapsed().as_micros() as u64)
    }

    async fn execute(&self, query: &str) -> Result<ResultSet, String> {
        self.execute_with_tracking(query, true).await
    }

    async fn execute_outcome(&self, query: &str) -> Result<dzban_core::ExecutionOutcome, String> {
        self.execute_detailed(query)
            .await
            .map_err(|error| error.to_string())
    }

    async fn execute_detailed(
        &self,
        query: &str,
    ) -> Result<dzban_core::ExecutionOutcome, dzban_core::error::ExecutionError> {
        let result = self.execute_tracked_detailed(query, true).await?;
        if result.columns.is_empty() && result.rows.is_empty() {
            Ok(dzban_core::ExecutionOutcome::Command(
                dzban_core::CommandAcknowledgment {
                    affected_rows: None,
                },
            ))
        } else {
            Ok(dzban_core::ExecutionOutcome::Rows(result))
        }
    }

    async fn execute_streamed(
        &self,
        query: &str,
        results: &mut dzban_core::result_stream::ResultSink,
    ) -> Result<Option<dzban_core::CommandAcknowledgment>, dzban_core::error::ExecutionError> {
        let client = self.client().await?;
        let query_id = uuid::Uuid::new_v4().to_string();
        let active = self.track(&query_id);
        let outcome = stream::execute(
            self.request_mode(client, query, &query_id, "0")
                .query(&[("buffer_size", "0")]),
            results,
        )
        .await;
        let _cancellation = self.cancellation.lock().await;
        if outcome.is_ok() && active.canceled() {
            Err(dzban_core::error::ExecutionError::Canceled)
        } else {
            outcome
        }
    }

    async fn cancel(&self) -> Result<(), String> {
        self.cancel_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    async fn cancel_detailed(&self) -> Result<(), dzban_core::error::ExecutionError> {
        let _cancellation = self.cancellation.lock().await;
        let query_id = {
            let mut queries = self.active_queries.lock().expect("active query lock");
            let query = queries.last_mut().ok_or_else(|| {
                dzban_core::error::ExecutionError::Message("No ClickHouse query is running".into())
            })?;
            query.status = QueryStatus::CancelRequested;
            query.id.clone()
        };
        let outcome = self
            .execute_tracked_detailed(
                &format!("KILL QUERY WHERE query_id = '{query_id}' SYNC"),
                false,
            )
            .await
            .map(|_| ());
        if outcome.is_err()
            && let Some(query) = self
                .active_queries
                .lock()
                .expect("active query lock")
                .iter_mut()
                .find(|query| query.id == query_id)
        {
            query.status = QueryStatus::Running;
        }
        outcome
    }

    async fn version(&self) -> Result<String, String> {
        self.version_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    async fn version_detailed(&self) -> Result<String, dzban_core::error::ExecutionError> {
        let result = self
            .execute_tracked_detailed("SELECT version()", false)
            .await?;
        result
            .rows
            .first()
            .and_then(|row| row.first())
            .map(|cell| cell.display().to_string())
            .ok_or_else(|| {
                dzban_core::error::ExecutionError::Message(
                    "ClickHouse returned no server version".into(),
                )
            })
    }

    async fn explain(&self, query: &str) -> Result<QueryPlan, String> {
        self.explain_detailed(query)
            .await
            .map_err(|error| error.to_string())
    }

    async fn explain_detailed(
        &self,
        query: &str,
    ) -> Result<QueryPlan, dzban_core::error::ExecutionError> {
        let plan_result = self
            .execute_tracked_detailed(
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
            .execute_tracked_detailed(&format!("EXPLAIN ESTIMATE {query}"), false)
            .await;
        let estimate = match estimate_result {
            Ok(result) => clickhouse_estimate(&result),
            Err(error) => QueryEstimate {
                diagnostic: Some(error),
                ..Default::default()
            },
        };
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
        column(name).and_then(|index| {
            result.rows.iter().try_fold(0u64, |total, row| {
                total.checked_add(row.get(index)?.display().parse::<u64>().ok()?)
            })
        })
    };
    QueryEstimate {
        rows: sum("rows"),
        parts: sum("parts"),
        marks: sum("marks"),
        ..QueryEstimate::default()
    }
}

fn clickhouse_transport_error(error: reqwest::Error) -> dzban_core::error::DatabaseError {
    let error = error.without_url();
    dzban_core::error::DatabaseError {
        backend: DatabaseBackend::Clickhouse,
        kind: dzban_core::error::ErrorKind::Transport,
        message: format_error_chain(&error),
        code: None,
        detail: None,
        hint: None,
    }
}

fn clickhouse_error(
    status: StatusCode,
    code: Option<&str>,
    body: &str,
) -> dzban_core::error::DatabaseError {
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
    let message = match code {
        Some(_) => format!("ClickHouse error ({status}): {message}"),
        None => format!("ClickHouse request failed ({status}): {message}"),
    };
    dzban_core::error::DatabaseError {
        backend: DatabaseBackend::Clickhouse,
        kind: dzban_core::error::ErrorKind::Server,
        message,
        code: code
            .and_then(|code| code.parse().ok())
            .map(dzban_core::error::NativeErrorCode::ClickHouse),
        detail: None,
        hint: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandoned_queries_leave_no_active_registration() {
        let queries = Mutex::new(vec![
            QueryRegistration {
                id: "older".into(),
                status: QueryStatus::Running,
            },
            QueryRegistration {
                id: "dropped".into(),
                status: QueryStatus::Running,
            },
        ]);
        let active = ActiveQuery {
            queries: &queries,
            id: "dropped".into(),
        };
        drop(active);
        assert_eq!(
            queries
                .lock()
                .unwrap()
                .iter()
                .map(|query| query.id.clone())
                .collect::<Vec<_>>(),
            ["older"]
        );
    }

    #[tokio::test]
    #[ignore = "requires ClickHouse from docker-compose.test.yml"]
    async fn catalog_parameters_filter_rows_on_the_server_before_mapping() {
        let session = ClickHouseSession::new(DatabaseConfig {
            name: "bound-catalog-fixture".into(),
            backend: DatabaseBackend::Clickhouse,
            host: None,
            port: None,
            user: "dzban".into(),
            password: Some("dzban-test".into()),
            database: "dzban_test".into(),
            url: Some("http://127.0.0.1:58123".into()),
        })
        .unwrap();
        session.connect().await.unwrap();
        let request = dzban_core::catalog::CatalogRequest::List {
            kind: dzban_core::catalog::ObjectKind::Table,
            database: None,
            schema: None,
            glob: Some("events".into()),
            visibility: dzban_core::catalog::Visibility::UserObjects,
        };
        let query =
            crate::catalog::query(DatabaseBackend::Clickhouse, "dzban_test", &request).unwrap();
        let result = session
            .execute_parameters(&query.sql, false, &query.clickhouse_parameters())
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][1].display(), "events");
        let query = crate::catalog::query(
            DatabaseBackend::Clickhouse,
            "dzban_test",
            &dzban_core::catalog::CatalogRequest::Find {
                kind: dzban_core::catalog::ObjectKind::Table,
                name: "events".into(),
            },
        )
        .unwrap();
        let result = session
            .execute_parameters(&query.sql, false, &query.clickhouse_parameters())
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][1].display(), "events");
        session.disconnect().await.unwrap();
    }

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
    fn invalid_or_overflowing_estimates_remain_unknown() {
        for values in [vec!["18446744073709551615", "1"], vec!["10", "invalid"]] {
            let result = ResultSet {
                columns: vec![ResultColumn {
                    name: "rows".into(),
                    type_name: "UInt64".into(),
                }],
                rows: values
                    .into_iter()
                    .map(|value| vec![ResultCell::Text(value.into())])
                    .collect(),
            };
            assert_eq!(clickhouse_estimate(&result).rows, None);
        }
    }

    #[test]
    fn clickhouse_error_omits_format_header_rows() {
        let error = clickhouse_error(
            StatusCode::NOT_FOUND,
            Some("60"),
            "[]\n[]\n[\"Code: 60. Unknown table events\"]\n",
        );

        assert_eq!(
            error.to_string(),
            "ClickHouse error (404 Not Found): Code: 60. Unknown table events (ClickHouse code 60)"
        );
        assert_eq!(
            error.code,
            Some(dzban_core::error::NativeErrorCode::ClickHouse(60))
        );
    }
}
