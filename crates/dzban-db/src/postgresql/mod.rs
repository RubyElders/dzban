use crate::config::{DatabaseBackend, DatabaseConfig};
use crate::errors::format_error_chain;
use crate::postgres_value::{ValueFormat, display_formatted, is_null, needs_money_scale};
use crate::result::{QueryEstimate, QueryPlan, ResultCell, ResultColumn, ResultSet};
use async_trait::async_trait;
use dzban_core::adapter::DatabaseSession;
use futures_util::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_postgres::Client;
use tokio_postgres_rustls::MakeRustlsConnect;
use tracing::{error, warn};

pub(crate) struct PostgresSession {
    config: DatabaseConfig,
    client: RwLock<Option<Arc<Client>>>,
}

fn result_set(
    statement: &tokio_postgres::Statement,
    rows: &[tokio_postgres::Row],
    format: ValueFormat,
) -> ResultSet {
    ResultSet {
        columns: result_columns(statement),
        rows: rows.iter().map(|row| result_row(row, format)).collect(),
    }
}

fn result_columns(statement: &tokio_postgres::Statement) -> Vec<ResultColumn> {
    statement
        .columns()
        .iter()
        .map(|column| ResultColumn {
            name: column.name().into(),
            type_name: column.type_().name().into(),
        })
        .collect()
}

fn result_row(row: &tokio_postgres::Row, format: ValueFormat) -> Vec<ResultCell> {
    row.columns()
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if is_null(row, index) {
                ResultCell::Null
            } else {
                ResultCell::Text(display_formatted(row, index, format))
            }
        })
        .collect()
}

async fn value_format(
    client: &Client,
    statement: &tokio_postgres::Statement,
) -> Result<ValueFormat, dzban_core::error::ExecutionError> {
    let money_scale = if statement
        .columns()
        .iter()
        .any(|column| needs_money_scale(column.type_()))
    {
        let row = client
            .query_one(
                "SELECT pg_catalog.scale(0::pg_catalog.money::pg_catalog.numeric)",
                &[],
            )
            .await
            .map_err(|error| dzban_core::error::ExecutionError::Database(postgres_error(&error)))?;
        Some(row.get::<_, i32>(0) as u32)
    } else {
        None
    };
    Ok(ValueFormat::with_money_scale(money_scale))
}

impl PostgresSession {
    pub(crate) fn new(config: DatabaseConfig) -> Self {
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
    fn capabilities(&self) -> dzban_core::catalog::Capabilities {
        crate::catalog::capabilities(DatabaseBackend::Postgresql)
    }

    async fn catalog(
        &self,
        request: dzban_core::catalog::CatalogRequest,
    ) -> Result<Vec<dzban_core::catalog::CatalogEntry>, dzban_core::catalog::CatalogError> {
        let query =
            crate::catalog::query(DatabaseBackend::Postgresql, &self.config.database, &request)?;
        let client = self
            .client()
            .await
            .map_err(|message| dzban_core::catalog::CatalogError::Connection { message })?;
        let statement = client.prepare(&query.sql).await.map_err(|error| {
            crate::catalog::error(dzban_core::error::ExecutionError::Database(postgres_error(
                &error,
            )))
        })?;
        let parameters = query
            .parameters
            .iter()
            .map(|value| value as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect::<Vec<_>>();
        let rows = client
            .query(&statement, &parameters)
            .await
            .map_err(|error| {
                crate::catalog::error(dzban_core::error::ExecutionError::Database(postgres_error(
                    &error,
                )))
            })?;
        let result = result_set(&statement, &rows, ValueFormat::default());
        crate::catalog::entries(
            DatabaseBackend::Postgresql,
            &self.config.database,
            &request,
            result,
        )
    }
    async fn disconnect(&self) -> Result<(), dzban_core::error::ExecutionError> {
        self.client.write().await.take();
        Ok(())
    }
    async fn connect(&self) -> Result<(), String> {
        self.connect_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    async fn connect_detailed(&self) -> Result<(), dzban_core::error::ExecutionError> {
        let tls = tls_connector().map_err(dzban_core::error::ExecutionError::from)?;
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
            Err(error) => Err(dzban_core::error::ExecutionError::Database(postgres_error(
                &error,
            ))),
        }
    }

    async fn ping(&self) -> Result<u64, String> {
        self.ping_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    async fn reconnect_if_needed(
        &self,
        started: &(dyn Fn() + Send + Sync),
    ) -> Result<bool, dzban_core::error::ExecutionError> {
        let closed = self
            .client
            .read()
            .await
            .as_ref()
            .is_none_or(|client| client.is_closed());
        if !closed {
            return Ok(false);
        }
        started();
        self.connect_detailed().await?;
        Ok(true)
    }

    async fn ping_detailed(&self) -> Result<u64, dzban_core::error::ExecutionError> {
        let client = self
            .client()
            .await
            .map_err(dzban_core::error::ExecutionError::from)?;
        let start = std::time::Instant::now();
        client
            .simple_query("SELECT 1")
            .await
            .map_err(|error| dzban_core::error::ExecutionError::Database(postgres_error(&error)))?;
        Ok(start.elapsed().as_micros() as u64)
    }

    async fn execute(&self, query: &str) -> Result<ResultSet, String> {
        self.execute_outcome(query)
            .await
            .map(|outcome| outcome.into_parts().0)
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
        use dzban_core::error::ExecutionError;
        let client = self.client().await?;
        let statement = client
            .prepare(query)
            .await
            .map_err(|error| ExecutionError::Database(postgres_error(&error)))?;
        if statement.columns().is_empty() {
            let affected_rows = client
                .execute(&statement, &[])
                .await
                .map_err(|error| ExecutionError::Database(postgres_error(&error)))?;
            return Ok(dzban_core::ExecutionOutcome::Command(
                dzban_core::CommandAcknowledgment {
                    affected_rows: Some(affected_rows),
                },
            ));
        }
        let format = value_format(&client, &statement).await?;
        let rows = client
            .query(&statement, &[])
            .await
            .map_err(|error| ExecutionError::Database(postgres_error(&error)))?;
        Ok(dzban_core::ExecutionOutcome::Rows(result_set(
            &statement, &rows, format,
        )))
    }

    async fn cancel(&self) -> Result<(), String> {
        self.cancel_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    async fn execute_streamed(
        &self,
        query: &str,
        results: &mut dzban_core::result_stream::ResultSink,
    ) -> Result<Option<dzban_core::CommandAcknowledgment>, dzban_core::error::ExecutionError> {
        use dzban_core::error::ExecutionError;
        use dzban_core::result_stream::Delivery;
        let client = self.client().await?;
        let statement = client
            .prepare(query)
            .await
            .map_err(|error| ExecutionError::Database(postgres_error(&error)))?;
        if statement.columns().is_empty() {
            let affected_rows = client
                .execute(&statement, &[])
                .await
                .map_err(|error| ExecutionError::Database(postgres_error(&error)))?;
            return Ok(Some(dzban_core::CommandAcknowledgment {
                affected_rows: Some(affected_rows),
            }));
        }
        let format = value_format(&client, &statement).await?;
        let mut delivery = results.schema(result_columns(&statement)).await?;
        let mut stream = Box::pin(
            client
                .query_raw(
                    &statement,
                    std::iter::empty::<&(dyn tokio_postgres::types::ToSql + Sync)>(),
                )
                .await
                .map_err(|error| ExecutionError::Database(postgres_error(&error)))?,
        );
        let mut storage_failure = None;
        let mut execution_failure = None;
        while let Some(item) = stream.next().await {
            let row = match item {
                Ok(row) => row,
                Err(error) => {
                    let server_error = error.as_db_error().is_some();
                    execution_failure
                        .get_or_insert_with(|| ExecutionError::Database(postgres_error(&error)));
                    if server_error {
                        continue;
                    }
                    drop(stream);
                    let _ = client.batch_execute("").await;
                    return Err(execution_failure.unwrap());
                }
            };
            if delivery == Delivery::Continue && storage_failure.is_none() {
                match results.row(result_row(&row, format)).await {
                    Ok(next) => delivery = next,
                    Err(error) => storage_failure = Some(error),
                }
            }
        }
        if let Some(error) = execution_failure {
            return Err(error);
        }
        if let Some(error) = storage_failure {
            return Err(error);
        }
        Ok(None)
    }

    async fn cancel_detailed(&self) -> Result<(), dzban_core::error::ExecutionError> {
        self.client()
            .await
            .map_err(dzban_core::error::ExecutionError::from)?
            .cancel_token()
            .cancel_query(tls_connector().map_err(dzban_core::error::ExecutionError::from)?)
            .await
            .map_err(|error| dzban_core::error::ExecutionError::Database(postgres_error(&error)))
    }

    async fn version(&self) -> Result<String, String> {
        self.version_detailed()
            .await
            .map_err(|error| error.to_string())
    }

    async fn version_detailed(&self) -> Result<String, dzban_core::error::ExecutionError> {
        self.client()
            .await
            .map_err(dzban_core::error::ExecutionError::from)?
            .query_one("SELECT version()", &[])
            .await
            .map(|row| row.get(0))
            .map_err(|error| dzban_core::error::ExecutionError::Database(postgres_error(&error)))
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
        use dzban_core::error::ExecutionError;
        let client = self.client().await?;
        let savepoint = format!("dzban_explain_{}", uuid::Uuid::new_v4().simple());
        let protected = match client
            .batch_execute(&format!("SAVEPOINT {savepoint}"))
            .await
        {
            Ok(()) => true,
            Err(error) if error.code().is_some_and(|code| code.code() == "25P01") => false,
            Err(error) => {
                return Err(planning_error(&error, "EXPLAIN unavailable"));
            }
        };
        let row = client
            .query_one(&format!("EXPLAIN (FORMAT JSON) {query}"), &[])
            .await;
        if protected {
            if row.is_err() {
                client
                    .batch_execute(&format!("ROLLBACK TO SAVEPOINT {savepoint}"))
                    .await
                    .map_err(|error| {
                        planning_error(&error, "Failed to restore EXPLAIN savepoint")
                    })?;
            }
            client
                .batch_execute(&format!("RELEASE SAVEPOINT {savepoint}"))
                .await
                .map_err(|error| planning_error(&error, "Failed to release EXPLAIN savepoint"))?;
        }
        let row = row.map_err(|error| ExecutionError::Database(postgres_error(&error)))?;
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

fn planning_error(
    error: &tokio_postgres::Error,
    context: &str,
) -> dzban_core::error::ExecutionError {
    let mut diagnostic = postgres_error(error);
    diagnostic.message = format!("{context}: {}", diagnostic.message);
    dzban_core::error::ExecutionError::Database(diagnostic)
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

fn postgres_error(error: &tokio_postgres::Error) -> dzban_core::error::DatabaseError {
    use dzban_core::error::{DatabaseError, ErrorKind, NativeErrorCode};
    if let Some(database_error) = error.as_db_error() {
        return DatabaseError {
            backend: DatabaseBackend::Postgresql,
            kind: if database_error.code() == &tokio_postgres::error::SqlState::QUERY_CANCELED {
                ErrorKind::Canceled
            } else {
                ErrorKind::Server
            },
            message: database_error.message().into(),
            code: Some(NativeErrorCode::SqlState(
                database_error.code().code().into(),
            )),
            detail: database_error.detail().map(str::to_owned),
            hint: database_error.hint().map(str::to_owned),
        };
    }
    DatabaseError {
        backend: DatabaseBackend::Postgresql,
        kind: ErrorKind::Transport,
        message: format_error_chain(error),
        code: None,
        detail: None,
        hint: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires PostgreSQL from docker-compose.test.yml"]
    async fn catalog_parameters_filter_rows_on_the_server_before_mapping() {
        let session = PostgresSession::new(DatabaseConfig {
            name: "bound-catalog-fixture".into(),
            backend: DatabaseBackend::Postgresql,
            host: Some("127.0.0.1".into()),
            port: Some(5415),
            user: "testuser".into(),
            password: Some("testpass".into()),
            database: "testdb".into(),
            url: None,
        });
        session.connect().await.unwrap();
        let request = dzban_core::catalog::CatalogRequest::List {
            kind: dzban_core::catalog::ObjectKind::Table,
            database: None,
            schema: Some("public".into()),
            glob: Some("users".into()),
            visibility: dzban_core::catalog::Visibility::UserObjects,
        };
        let query = crate::catalog::query(DatabaseBackend::Postgresql, "testdb", &request).unwrap();
        let client = session.client().await.unwrap();
        let parameters = query
            .parameters
            .iter()
            .map(|value| value as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect::<Vec<_>>();
        let rows = client.query(&query.sql, &parameters).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<_, String>("name"), "users");
        let query = crate::catalog::query(
            DatabaseBackend::Postgresql,
            "testdb",
            &dzban_core::catalog::CatalogRequest::Find {
                kind: dzban_core::catalog::ObjectKind::Table,
                name: "users".into(),
            },
        )
        .unwrap();
        let parameters = query
            .parameters
            .iter()
            .map(|value| value as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect::<Vec<_>>();
        let rows = client.query(&query.sql, &parameters).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<_, String>("name"), "users");
        session.disconnect().await.unwrap();
    }
}
