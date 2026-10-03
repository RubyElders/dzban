use glib::prelude::*;
use glib::subclass::prelude::*;
use std::cell::{Cell, OnceCell, RefCell};
use tracing::{debug, error, info, instrument, trace, warn};

use crate::config::{DatabaseBackend, DatabaseConfig};
use crate::internal_connection::InternalConnection;
use crate::runtime_bridge::RuntimeBridge;
use crate::{QueryPlan, QueryResult};

pub type QueryReceiver<T> = tokio::sync::oneshot::Receiver<Result<T, String>>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[enum_type(name = "DzbanConnectionOperation")]
pub enum ConnectionOperation {
    #[default]
    Idle = 0,
    Ping = 1,
    Explain = 2,
    Query = 3,
    PlanningQuery = 4,
}

impl ConnectionOperation {
    pub fn is_query(self) -> bool {
        matches!(self, Self::Query | Self::PlanningQuery)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[enum_type(name = "DzbanConnectionState")]
pub enum ConnectionState {
    #[default]
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
    Error = 3,
}

mod imp {
    use super::*;

    pub struct DbConnection {
        pub connection_string: RefCell<String>,

        pub state: Cell<ConnectionState>,
        pub latency_micros: Cell<i64>,
        pub version: RefCell<Option<String>>,
        pub ping_count: Cell<u32>,
        pub last_error: RefCell<String>,
        pub operation: Cell<ConnectionOperation>,
        pub operation_generation: Cell<u64>,
        pub cancelled_generation: Cell<Option<u64>>,

        pub(super) runtime_bridge: OnceCell<RuntimeBridge>,
        pub(super) database_config: OnceCell<DatabaseConfig>,
        pub(super) db_connection: RefCell<Option<InternalConnection>>,
    }

    impl Default for DbConnection {
        fn default() -> Self {
            Self {
                connection_string: RefCell::new(String::new()),
                state: Cell::new(ConnectionState::Disconnected),
                latency_micros: Cell::new(-1),
                version: RefCell::new(None),
                ping_count: Cell::new(0),
                last_error: RefCell::new(String::new()),
                operation: Cell::new(ConnectionOperation::Idle),
                operation_generation: Cell::new(0),
                cancelled_generation: Cell::new(None),
                runtime_bridge: OnceCell::new(),
                database_config: OnceCell::new(),
                db_connection: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DbConnection {
        const NAME: &'static str = "DzbanDbConnection";
        type Type = super::DbConnection;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for DbConnection {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: std::sync::OnceLock<Vec<glib::ParamSpec>> =
                std::sync::OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecString::builder("connection-string")
                        .construct_only()
                        .build(),
                    glib::ParamSpecEnum::builder::<ConnectionState>("state")
                        .read_only()
                        .build(),
                    glib::ParamSpecInt64::builder("latency-micros")
                        .minimum(-1)
                        .default_value(-1)
                        .read_only()
                        .build(),
                    glib::ParamSpecString::builder("version")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt::builder("ping-count")
                        .read_only()
                        .build(),
                    glib::ParamSpecString::builder("last-error")
                        .read_only()
                        .build(),
                    glib::ParamSpecBoolean::builder("query-running")
                        .read_only()
                        .build(),
                    glib::ParamSpecEnum::builder::<ConnectionOperation>("operation")
                        .read_only()
                        .build(),
                    glib::ParamSpecBoolean::builder("operation-available")
                        .read_only()
                        .build(),
                ]
            })
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "connection-string" => self.connection_string.borrow().to_value(),
                "state" => self.state.get().to_value(),
                "latency-micros" => self.latency_micros.get().to_value(),
                "version" => self.version.borrow().to_value(),
                "ping-count" => self.ping_count.get().to_value(),
                "last-error" => self.last_error.borrow().to_value(),
                "query-running" => self.operation.get().is_query().to_value(),
                "operation" => self.operation.get().to_value(),
                "operation-available" => (self.state.get() == ConnectionState::Connected
                    && self.operation.get() == ConnectionOperation::Idle)
                    .to_value(),
                _ => unimplemented!(),
            }
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "connection-string" => {
                    let conn_str: String = value.get().unwrap();
                    self.connection_string.replace(conn_str);
                }
                _ => unimplemented!(),
            }
        }

        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: std::sync::OnceLock<Vec<glib::subclass::Signal>> =
                std::sync::OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("state-changed")
                        .param_types([
                            ConnectionState::static_type(),
                            ConnectionState::static_type(),
                        ])
                        .build(),
                    glib::subclass::Signal::builder("ping-completed")
                        .param_types([i64::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("ping-failed")
                        .param_types([String::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("query-started")
                        .param_types([String::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("query-finished")
                        .param_types([i64::static_type(), i64::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("query-error")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    pub struct DbConnection(ObjectSubclass<imp::DbConnection>);
}

struct OperationLease {
    connection: glib::WeakRef<DbConnection>,
    generation: u64,
}

impl OperationLease {
    fn transition(&self, operation: ConnectionOperation) {
        if let Some(connection) = self.connection.upgrade() {
            connection.transition_operation(self.generation, operation);
        }
    }

    fn cancellation_requested(&self) -> bool {
        self.connection.upgrade().is_some_and(|connection| {
            connection.imp().cancelled_generation.get() == Some(self.generation)
        })
    }
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.upgrade() {
            connection.finish_operation(self.generation);
        }
    }
}

impl DbConnection {
    pub fn new(connection_string: String) -> Self {
        let config = DatabaseConfig::from_connection_string(&connection_string);
        Self::build(connection_string, config)
    }

    pub fn from_config(config: DatabaseConfig) -> Self {
        let connection_string = match config.backend {
            DatabaseBackend::Postgresql => config.connection_string(),
            DatabaseBackend::Clickhouse => config.url.clone().unwrap_or_default(),
        };
        Self::build(connection_string, config)
    }

    fn build(connection_string: String, config: DatabaseConfig) -> Self {
        let obj: Self = glib::Object::builder()
            .property("connection-string", connection_string)
            .build();

        let bridge = RuntimeBridge::new().expect("Failed to create RuntimeBridge");
        obj.imp()
            .runtime_bridge
            .set(bridge)
            .expect("RuntimeBridge already initialized");
        obj.imp()
            .database_config
            .set(config)
            .expect("DatabaseConfig already initialized");

        obj
    }

    pub fn backend(&self) -> DatabaseBackend {
        self.imp()
            .database_config
            .get()
            .map(|config| config.backend)
            .unwrap_or_default()
    }

    pub fn connection_string(&self) -> String {
        self.property("connection-string")
    }

    pub fn state(&self) -> ConnectionState {
        self.property("state")
    }

    pub fn latency_micros(&self) -> i64 {
        self.property("latency-micros")
    }

    pub fn latency(&self) -> Option<std::time::Duration> {
        let micros = self.latency_micros();
        if micros >= 0 {
            Some(std::time::Duration::from_micros(micros as u64))
        } else {
            None
        }
    }

    pub fn version(&self) -> Option<String> {
        self.property("version")
    }

    pub fn ping_count(&self) -> u32 {
        self.property("ping-count")
    }

    pub fn last_error(&self) -> String {
        self.property("last-error")
    }

    pub fn query_running(&self) -> bool {
        self.property("query-running")
    }

    pub fn operation(&self) -> ConnectionOperation {
        self.property("operation")
    }

    pub fn operation_available(&self) -> bool {
        self.property("operation-available")
    }

    fn set_state(&self, new_state: ConnectionState) {
        let old_state = self.imp().state.get();
        if old_state != new_state {
            let was_available = self.operation_available();
            self.imp().state.set(new_state);
            self.notify("state");
            if was_available != self.operation_available() {
                self.notify("operation-available");
            }
            self.emit_by_name::<()>("state-changed", &[&old_state, &new_state]);
            debug!("Connection state: {:?} -> {:?}", old_state, new_state);
        }
    }

    fn set_latency_micros(&self, micros: i64) {
        self.imp().latency_micros.set(micros);
        self.notify("latency-micros");
    }

    fn set_version(&self, version: Option<String>) {
        self.imp().version.replace(version);
        self.notify("version");
    }

    fn increment_ping_count(&self) {
        let count = self.imp().ping_count.get();
        self.imp().ping_count.set(count + 1);
        self.notify("ping-count");
    }

    fn set_last_error(&self, error: String) {
        self.imp().last_error.replace(error);
        self.notify("last-error");
    }

    fn set_operation(&self, operation: ConnectionOperation) {
        let previous = self.imp().operation.replace(operation);
        if previous == operation {
            return;
        }
        self.notify("operation");
        if self.state() == ConnectionState::Connected
            && (previous == ConnectionOperation::Idle) != (operation == ConnectionOperation::Idle)
        {
            self.notify("operation-available");
        }
        if previous.is_query() != operation.is_query() {
            self.notify("query-running");
        }
    }

    fn try_begin_operation(
        &self,
        operation: ConnectionOperation,
    ) -> Result<OperationLease, String> {
        if self.state() != ConnectionState::Connected {
            return Err("Not connected".to_string());
        }
        if self.operation() != ConnectionOperation::Idle {
            return Err("Connection is busy".to_string());
        }
        let generation = self.imp().operation_generation.get().wrapping_add(1);
        self.imp().operation_generation.set(generation);
        self.imp().cancelled_generation.set(None);
        self.set_operation(operation);
        Ok(OperationLease {
            connection: self.downgrade(),
            generation,
        })
    }

    fn transition_operation(&self, generation: u64, operation: ConnectionOperation) {
        if self.imp().operation_generation.get() == generation {
            self.set_operation(operation);
        }
    }

    fn finish_operation(&self, generation: u64) {
        if self.imp().operation_generation.get() == generation {
            self.imp().cancelled_generation.set(None);
            self.set_operation(ConnectionOperation::Idle);
        }
    }

    fn begin_query(&self, query: &str) -> Result<OperationLease, String> {
        let lease = self.try_begin_operation(ConnectionOperation::Query)?;
        self.set_last_error(String::new());
        self.emit_by_name::<()>("query-started", &[&query]);
        Ok(lease)
    }

    fn begin_planned_query(&self, query: &str) -> Result<OperationLease, String> {
        let lease = self.try_begin_operation(ConnectionOperation::PlanningQuery)?;
        self.set_last_error(String::new());
        self.emit_by_name::<()>("query-started", &[&query]);
        Ok(lease)
    }

    /// Connect to the database asynchronously
    /// Emits state-changed signals during the process
    #[instrument(skip(self))]
    pub fn connect(&self) {
        info!("Starting connection...");
        self.set_state(ConnectionState::Connecting);

        let this = self.clone();

        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();

        let config = self
            .imp()
            .database_config
            .get()
            .cloned()
            .unwrap_or_else(|| DatabaseConfig::from_connection_string(&self.connection_string()));

        let rx = bridge.connect(config);

        glib::MainContext::ref_thread_default().spawn_local(async move {
            match rx.await {
                Ok(Ok(db_conn)) => {
                    info!("Connected successfully");

                    match bridge.version(db_conn.clone()).await {
                        Ok(Ok(version)) => {
                            let version_short = version
                                .split_whitespace()
                                .take(2)
                                .collect::<Vec<_>>()
                                .join(" ");
                            this.set_version(Some(version_short));
                        }
                        Ok(Err(e)) => {
                            warn!("Failed to get database version: {}", e);
                        }
                        Err(_) => warn!("Database version request was dropped"),
                    }

                    this.imp().db_connection.replace(Some(db_conn));
                    this.set_state(ConnectionState::Connected);
                    this.set_last_error(String::new());
                }
                Ok(Err(e)) => {
                    let error_msg = format!("Connection failed: {}", e);
                    error!("{}", error_msg);
                    this.set_state(ConnectionState::Error);
                    this.set_last_error(error_msg);
                }
                Err(_) => {
                    let error_msg = "Connection cancelled".to_string();
                    error!("{}", error_msg);
                    this.set_state(ConnectionState::Error);
                    this.set_last_error(error_msg);
                }
            }
        });
    }

    /// Disconnect from the database
    pub fn disconnect(&self) {
        info!("Disconnecting...");

        let db_conn = self.imp().db_connection.take();
        if let Some(db_conn) = db_conn {
            let bridge = self
                .imp()
                .runtime_bridge
                .get()
                .expect("RuntimeBridge not initialized");

            let rx = bridge.disconnect(db_conn);

            glib::MainContext::ref_thread_default().spawn_local(async move {
                let _ = rx.await;
                debug!("Background disconnect completed");
            });
        }

        self.imp().db_connection.replace(None);
        self.imp()
            .operation_generation
            .set(self.imp().operation_generation.get().wrapping_add(1));
        self.imp().cancelled_generation.set(None);
        self.set_state(ConnectionState::Disconnected);
        self.set_operation(ConnectionOperation::Idle);
        self.set_version(None);
        self.set_latency_micros(-1);
        self.set_last_error(String::new());
        debug!("Disconnected");
    }

    /// Ping the database to check connectivity and measure latency
    /// Emits ping-completed or ping-failed signals
    #[instrument(skip(self))]
    pub fn ping(&self) {
        let Ok(operation) = self.try_begin_operation(ConnectionOperation::Ping) else {
            trace!("Skipping ping while the connection is busy");
            return;
        };
        let db_conn = self.imp().db_connection.borrow().clone();

        if db_conn.is_none() {
            let error = "Not connected".to_string();
            warn!("{}", error);
            self.emit_by_name::<()>("ping-failed", &[&error]);
            return;
        }

        let this = self.clone();
        let db_conn = db_conn.unwrap();
        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();

        glib::MainContext::ref_thread_default().spawn_local(async move {
            let _operation = operation;
            let rx = bridge.ping(db_conn);

            match rx.await {
                Ok(Ok((latency_micros, _))) => {
                    trace!("Ping successful: {} us", latency_micros);

                    this.set_latency_micros(latency_micros as i64);
                    this.increment_ping_count();
                    this.emit_by_name::<()>("ping-completed", &[&(latency_micros as i64)]);
                }
                Ok(Err(e)) => {
                    let error_msg = format!("Ping failed: {}", e);
                    error!("{}", error_msg);

                    this.set_state(ConnectionState::Error);
                    this.set_last_error(error_msg.clone());
                    this.emit_by_name::<()>("ping-failed", &[&error_msg]);
                }
                Err(_) => {
                    let error_msg = "Ping cancelled".to_string();
                    error!("{}", error_msg);
                    this.emit_by_name::<()>("ping-failed", &[&error_msg]);
                }
            }
        });
    }

    /// Execute a SQL query asynchronously
    /// Emits query-started, then query-finished or query-error
    #[instrument(skip(self, query), fields(query_preview = %query.chars().take(50).collect::<String>()))]
    pub fn execute_query(&self, query: String) {
        drop(self.execute_query_rows(query));
    }

    pub fn execute_query_rows(
        &self,
        query: String,
    ) -> tokio::sync::oneshot::Receiver<Result<QueryResult, String>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let Some(db_conn) = self.imp().db_connection.borrow().clone() else {
            let error = "Not connected".to_string();
            self.set_last_error(error.clone());
            self.emit_by_name::<()>("query-error", &[&error]);
            let _ = tx.send(Err(error));
            return rx;
        };

        let operation = match self.begin_query(&query) {
            Ok(operation) => operation,
            Err(error) => {
                let _ = tx.send(Err(error));
                return rx;
            }
        };
        debug!("Executing query...");

        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();
        let this = self.clone();

        glib::MainContext::ref_thread_default().spawn_local(async move {
            let operation = operation;
            let start = std::time::Instant::now();
            let result = match bridge.execute_query_rows(db_conn, query).await {
                Ok(result) => result,
                Err(_) => Err("Query cancelled".to_string()),
            };
            let result = match result {
                Ok(result_set) => {
                    let duration = start.elapsed();
                    let row_count = result_set.rows.len() as i64;
                    let duration_ms = duration.as_millis() as i64;
                    drop(operation);
                    this.emit_by_name::<()>("query-finished", &[&row_count, &duration_ms]);
                    Ok(QueryResult {
                        result_set,
                        duration,
                    })
                }
                Err(error) => {
                    drop(operation);
                    this.set_last_error(error.clone());
                    this.emit_by_name::<()>("query-error", &[&error]);
                    Err(error)
                }
            };
            let _ = tx.send(result);
        });

        rx
    }

    pub fn execute_query_rows_with_explain(
        &self,
        query: String,
    ) -> Result<(QueryReceiver<QueryResult>, QueryReceiver<QueryPlan>), String> {
        let (query_tx, query_rx) = tokio::sync::oneshot::channel();
        let (plan_tx, plan_rx) = tokio::sync::oneshot::channel();
        let Some(db_conn) = self.imp().db_connection.borrow().clone() else {
            let error = "Not connected".to_string();
            self.set_last_error(error.clone());
            self.emit_by_name::<()>("query-error", &[&error]);
            return Err(error);
        };
        let operation = self.begin_planned_query(&query)?;

        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();
        let this = self.clone();
        glib::MainContext::ref_thread_default().spawn_local(async move {
            let plan = bridge
                .explain(db_conn.clone(), query.clone())
                .await
                .unwrap_or_else(|_| Err("Plan request was dropped".to_string()));
            let cancellation_requested = operation.cancellation_requested();
            let _ = plan_tx.send(plan);
            if cancellation_requested {
                let error = "Query cancelled".to_string();
                drop(operation);
                this.set_last_error(error.clone());
                this.emit_by_name::<()>("query-error", &[&error]);
                let _ = query_tx.send(Err(error));
                return;
            }

            operation.transition(ConnectionOperation::Query);
            let start = std::time::Instant::now();
            let result = bridge
                .execute_query_rows(db_conn, query)
                .await
                .unwrap_or_else(|_| Err("Query cancelled".to_string()));
            match result {
                Ok(result_set) => {
                    let duration = start.elapsed();
                    let row_count = result_set.rows.len() as i64;
                    let duration_ms = duration.as_millis() as i64;
                    this.emit_by_name::<()>("query-finished", &[&row_count, &duration_ms]);
                    drop(operation);
                    let _ = query_tx.send(Ok(QueryResult {
                        result_set,
                        duration,
                    }));
                }
                Err(error) => {
                    drop(operation);
                    this.set_last_error(error.clone());
                    this.emit_by_name::<()>("query-error", &[&error]);
                    let _ = query_tx.send(Err(error));
                }
            }
        });

        Ok((query_rx, plan_rx))
    }

    pub fn explain_query(&self, query: String) -> Result<QueryReceiver<QueryPlan>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let Some(db_conn) = self.imp().db_connection.borrow().clone() else {
            let _ = tx.send(Err("Not connected".to_string()));
            return Ok(rx);
        };
        let operation = self.try_begin_operation(ConnectionOperation::Explain)?;
        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();
        glib::MainContext::ref_thread_default().spawn_local(async move {
            let _operation = operation;
            let result = bridge
                .explain(db_conn, query)
                .await
                .unwrap_or_else(|_| Err("Plan request was dropped".to_string()));
            let _ = tx.send(result);
        });
        Ok(rx)
    }

    pub fn cancel_query(&self) -> tokio::sync::oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.operation().is_query() {
            self.imp()
                .cancelled_generation
                .set(Some(self.imp().operation_generation.get()));
        }
        let Some(db_conn) = self.imp().db_connection.borrow().clone() else {
            let _ = tx.send(Err("Not connected".to_string()));
            return rx;
        };
        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();
        glib::MainContext::ref_thread_default().spawn_local(async move {
            let result = bridge
                .cancel_query(db_conn)
                .await
                .unwrap_or_else(|_| Err("Cancellation request was dropped".to_string()));
            let _ = tx.send(result);
        });
        rx
    }
}

impl Default for DbConnection {
    fn default() -> Self {
        Self::new(String::new())
    }
}
