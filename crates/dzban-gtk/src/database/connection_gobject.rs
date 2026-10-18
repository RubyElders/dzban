use glib::prelude::*;
use glib::subclass::prelude::*;
use std::cell::{Cell, OnceCell, RefCell};
use tracing::{debug, error, info, instrument, trace, warn};

use crate::database::InternalConnection;
use crate::database::config::{DatabaseBackend, DatabaseConfig};
use crate::database::runtime_bridge::RuntimeBridge;
use crate::database::{QueryPlan, QueryResult};

pub type QueryReceiver<T, E = String> = tokio::sync::oneshot::Receiver<Result<T, E>>;
pub type PlannedQueryReceivers = (
    QueryReceiver<QueryResult, dzban_core::error::ExecutionError>,
    QueryReceiver<QueryPlan, dzban_core::error::ExecutionError>,
);

pub struct StreamedQueryDelivery {
    pub id: dzban_core::execution::QueryId,
    pub generation: Option<u64>,
    pub state: tokio::sync::watch::Receiver<dzban_core::execution::QuerySnapshot>,
    pub result: QueryReceiver<
        dzban_core::execution::StreamedQueryResult,
        dzban_core::error::ExecutionError,
    >,
    pub bridge: RuntimeBridge,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[enum_type(name = "DzbanConnectionOperation")]
pub enum ConnectionOperation {
    #[default]
    Idle = 0,
    Ping = 1,
    Explain = 2,
    Query = 3,
    PlanningQuery = 4,
    Version = 5,
    Catalog = 6,
    Closing = 7,
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
        pub disconnect_generation: Cell<u64>,
        pub core_observer: RefCell<Option<glib::JoinHandle<()>>>,
        pub connection_selection: RefCell<dzban_core::application::ConnectionSelection>,

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
                disconnect_generation: Cell::new(0),
                core_observer: RefCell::new(None),
                connection_selection: RefCell::new(Default::default()),
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
        fn dispose(&self) {
            if let Some(observer) = self.core_observer.borrow_mut().take() {
                observer.abort();
            }
        }
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

impl DbConnection {
    pub fn execute_streamed_recorded_query(
        &self,
        sql: String,
    ) -> Result<StreamedQueryDelivery, String> {
        self.execute_recorded_for_document(sql, None)
    }

    pub fn execute_document_query(
        &self,
        document: dzban_core::documents::QueryDocument,
        sql: String,
    ) -> Result<StreamedQueryDelivery, String> {
        self.execute_recorded_for_document(sql, Some(document))
    }

    fn execute_recorded_for_document(
        &self,
        sql: String,
        document: Option<dzban_core::documents::QueryDocument>,
    ) -> Result<StreamedQueryDelivery, String> {
        let config = self
            .imp()
            .database_config
            .get()
            .expect("Database configuration initialized");
        let history = match dzban_core::history::QueryHistoryStore::load(&config.name) {
            Ok(history) => Some(history),
            Err(error) => {
                tracing::warn!("Query history unavailable: {error}");
                None
            }
        };
        self.execute_streamed_for_document(sql, history, document)
    }

    pub fn execute_streamed_query(
        &self,
        sql: String,
        history: Option<dzban_core::history::QueryHistoryStore>,
    ) -> Result<StreamedQueryDelivery, String> {
        self.execute_streamed_for_document(sql, history, None)
    }

    fn execute_streamed_for_document(
        &self,
        sql: String,
        history: Option<dzban_core::history::QueryHistoryStore>,
        document: Option<dzban_core::documents::QueryDocument>,
    ) -> Result<StreamedQueryDelivery, String> {
        let connection = self
            .imp()
            .db_connection
            .borrow()
            .clone()
            .ok_or_else(|| "Not connected".to_string())?;
        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge initialized")
            .clone();
        let delivery = match document {
            Some(document) => bridge.start_document(connection, document, sql.clone(), history),
            None => bridge.start_streamed(connection, sql.clone(), history),
        }
        .map_err(|error| error.to_string())?;
        self.begin_query(&sql);
        let id = delivery.id;
        let generation = delivery.generation;
        let state = delivery.state;
        let (sender, result) = tokio::sync::oneshot::channel();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = delivery.result.await.unwrap_or_else(|error| {
                Err(dzban_core::error::ExecutionError::Message(
                    error.to_string(),
                ))
            });
            let result = match result {
                Ok(completion) => {
                    if let Some(error) = completion
                        .history
                        .as_ref()
                        .and_then(|history| history.error_for(id))
                    {
                        tracing::warn!("Failed to save query history: {error}");
                    }
                    completion.result
                }
                Err(error) => Err(error),
            };
            this.refresh_core_state();
            match &result {
                Ok(result) => this.emit_by_name::<()>(
                    "query-finished",
                    &[
                        &(result.capture.rows as i64),
                        &(result.duration.as_millis() as i64),
                    ],
                ),
                Err(error) => {
                    let message = error.to_string();
                    this.set_last_error(message.clone());
                    this.emit_by_name::<()>("query-error", &[&message]);
                }
            }
            let _ = sender.send(result);
        });
        Ok(StreamedQueryDelivery {
            id,
            generation,
            state,
            result,
            bridge,
        })
    }
    pub fn new(connection_string: String) -> Self {
        let config = DatabaseConfig::from_connection_string(&connection_string);
        Self::build(connection_string, config, RuntimeBridge::default())
    }

    pub fn from_config(config: DatabaseConfig) -> Self {
        Self::from_config_with_bridge(config, RuntimeBridge::default())
    }

    pub fn from_config_with_bridge(config: DatabaseConfig, bridge: RuntimeBridge) -> Self {
        let connection_string = match config.backend {
            DatabaseBackend::Postgresql => config.connection_string(),
            DatabaseBackend::Clickhouse => config.url.clone().unwrap_or_default(),
        };
        Self::build(connection_string, config, bridge)
    }

    pub fn application_context(
        &self,
    ) -> std::sync::Arc<dzban_core::application::ApplicationContext> {
        self.imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge initialized")
            .application_context()
    }

    pub(crate) fn build(
        connection_string: String,
        config: DatabaseConfig,
        bridge: RuntimeBridge,
    ) -> Self {
        let obj: Self = glib::Object::builder()
            .property("connection-string", connection_string)
            .build();

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

    pub fn profile_name(&self) -> String {
        self.imp()
            .database_config
            .get()
            .map(|config| config.name.clone())
            .unwrap_or_default()
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

    fn refresh_core_state(&self) {
        use dzban_core::execution::{OperationState, QueryPhase, SessionPhase};
        let Some(connection) = self.imp().db_connection.borrow().clone() else {
            return;
        };
        let engine = connection.engine();
        if *engine.subscribe_session().borrow() == SessionPhase::Closed {
            self.imp().connection_selection.borrow_mut().clear();
            self.imp().db_connection.replace(None);
            self.set_version(None);
            self.set_latency_micros(-1);
            self.set_operation(ConnectionOperation::Idle);
            self.set_state(ConnectionState::Disconnected);
            return;
        }
        if *engine.subscribe_session().borrow() == SessionPhase::Closing {
            self.set_operation(ConnectionOperation::Closing);
            return;
        }
        let operation = match *engine.subscribe_operation().borrow() {
            OperationState::Idle => ConnectionOperation::Idle,
            OperationState::Preview => ConnectionOperation::Explain,
            OperationState::Ping => ConnectionOperation::Ping,
            OperationState::Version => ConnectionOperation::Version,
            OperationState::Catalog => ConnectionOperation::Catalog,
            OperationState::Closing => ConnectionOperation::Closing,
            OperationState::Query(id) => match engine.subscribe().borrow().phase {
                QueryPhase::Queued(current) | QueryPhase::Explaining(current) if current == id => {
                    ConnectionOperation::PlanningQuery
                }
                _ => ConnectionOperation::Query,
            },
        };
        self.set_operation(operation);
    }

    fn observe_core(&self) {
        if let Some(observer) = self.imp().core_observer.borrow_mut().take() {
            observer.abort();
        }
        let Some(connection) = self.imp().db_connection.borrow().clone() else {
            return;
        };
        let engine = connection.engine();
        let mut operation = engine.subscribe_operation();
        let mut session = engine.subscribe_session();
        let mut query = engine.subscribe();
        let this = self.downgrade();
        self.refresh_core_state();
        let observer = glib::MainContext::ref_thread_default().spawn_local(async move {
            loop {
                let Some(this) = this.upgrade() else {
                    break;
                };
                let selected = this
                    .imp()
                    .db_connection
                    .borrow()
                    .as_ref()
                    .map(|connection| connection.engine().id());
                if selected != Some(engine.id()) {
                    break;
                }
                let phase = *session.borrow_and_update();
                this.refresh_core_state();
                if phase == dzban_core::execution::SessionPhase::Closed {
                    break;
                }
                drop(this);
                tokio::select! {
                    changed = operation.changed() => if changed.is_err() { break; },
                    changed = session.changed() => if changed.is_err() { break; },
                    changed = query.changed() => if changed.is_err() { break; },
                }
            }
        });
        self.imp().core_observer.replace(Some(observer));
    }

    fn begin_query(&self, query: &str) {
        self.refresh_core_state();
        self.set_last_error(String::new());
        self.emit_by_name::<()>("query-started", &[&query]);
    }

    /// Connect to the database asynchronously
    /// Emits state-changed signals during the process
    #[instrument(skip(self))]
    pub fn connect(&self) {
        if !self
            .imp()
            .connection_selection
            .borrow()
            .accepts_connection_request()
        {
            return;
        }
        info!("Starting connection...");

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

        let delivery = bridge.start_connect(config);
        let request = delivery.state.borrow().request;
        self.imp().connection_selection.borrow_mut().begin(request);
        self.set_state(ConnectionState::Connecting);

        glib::MainContext::ref_thread_default().spawn_local(async move {
            let result = delivery.result.await;
            if *this.imp().connection_selection.borrow()
                != dzban_core::application::ConnectionSelection::Pending(request)
            {
                if let Ok(Ok(conn)) = result {
                    let _ = bridge.disconnect(conn);
                }
                return;
            }
            match result {
                Ok(Ok(db_conn)) => {
                    info!("Connected successfully");

                    let version = match bridge.version(db_conn.clone()).await {
                        Ok(Ok(version)) => {
                            let version_short = version
                                .split_whitespace()
                                .take(2)
                                .collect::<Vec<_>>()
                                .join(" ");
                            Some(version_short)
                        }
                        Ok(Err(e)) => {
                            warn!("Failed to get database version: {}", e);
                            None
                        }
                        Err(_) => {
                            warn!("Database version request was dropped");
                            None
                        }
                    };

                    if !this
                        .imp()
                        .connection_selection
                        .borrow_mut()
                        .apply(&delivery.state.borrow())
                    {
                        let _ = bridge.disconnect(db_conn);
                        return;
                    }
                    this.imp().db_connection.replace(Some(db_conn));
                    this.set_version(version);
                    this.set_state(ConnectionState::Connected);
                    this.observe_core();
                    this.set_last_error(String::new());
                }
                Ok(Err(e)) => {
                    this.imp()
                        .connection_selection
                        .borrow_mut()
                        .apply(&delivery.state.borrow());
                    let error_msg = format!("Connection failed: {}", e);
                    error!("{}", error_msg);
                    this.set_state(ConnectionState::Error);
                    this.set_last_error(error_msg);
                }
                Err(_) => {
                    this.imp().connection_selection.borrow_mut().clear();
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
        let _ = self.disconnect_with_completion();
    }

    pub fn disconnect_with_completion(
        &self,
    ) -> Result<
        QueryReceiver<(), dzban_core::error::ExecutionError>,
        dzban_core::error::ExecutionError,
    > {
        info!("Disconnecting...");
        if matches!(
            *self.imp().connection_selection.borrow(),
            dzban_core::application::ConnectionSelection::Closing(_)
        ) {
            return Err(dzban_core::error::ExecutionError::Busy);
        }
        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();

        let db_conn = self.imp().db_connection.borrow().clone();
        if let Some(db_conn) = db_conn {
            let bridge = self
                .imp()
                .runtime_bridge
                .get()
                .expect("RuntimeBridge not initialized");

            let rx = match bridge.disconnect(db_conn.clone()) {
                Ok(rx) => rx,
                Err(error) => {
                    self.set_last_error(error.to_string());
                    return Err(error);
                }
            };
            let generation = self.imp().disconnect_generation.get().wrapping_add(1);
            let version = self.version();
            self.imp()
                .connection_selection
                .borrow_mut()
                .begin_close(db_conn.engine().id());
            let this = self.clone();
            glib::MainContext::ref_thread_default().spawn_local(async move {
                let result = rx
                    .await
                    .unwrap_or_else(|_| Err("Disconnect worker was dropped".to_string().into()));
                if this.imp().disconnect_generation.get() == generation {
                    this.imp()
                        .connection_selection
                        .borrow_mut()
                        .finish_close(db_conn.engine().id(), &result);
                }
                if let Err(error) = &result
                    && this.imp().disconnect_generation.get() == generation
                    && this.state() == ConnectionState::Disconnected
                {
                    this.imp().db_connection.replace(Some(db_conn));
                    this.set_version(version);
                    this.set_state(ConnectionState::Connected);
                    this.observe_core();
                    this.set_last_error(error.to_string());
                }
                let _ = completion_tx.send(result);
            });
        } else {
            self.imp().connection_selection.borrow_mut().clear();
            let _ = completion_tx.send(Ok(()));
        }

        if let Some(observer) = self.imp().core_observer.borrow_mut().take() {
            observer.abort();
        }
        self.imp().db_connection.replace(None);
        self.imp()
            .disconnect_generation
            .set(self.imp().disconnect_generation.get().wrapping_add(1));
        self.set_state(ConnectionState::Disconnected);
        self.set_operation(ConnectionOperation::Idle);
        self.set_version(None);
        self.set_latency_micros(-1);
        self.set_last_error(String::new());
        debug!("Disconnected");
        Ok(completion_rx)
    }

    /// Ping the database to check connectivity and measure latency
    /// Emits ping-completed or ping-failed signals
    #[instrument(skip(self))]
    pub fn ping(&self) {
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
        let rx = match bridge.start_ping(db_conn) {
            Ok(rx) => rx,
            Err(error) => {
                trace!("Skipping ping: {error}");
                return;
            }
        };
        self.refresh_core_state();

        glib::MainContext::ref_thread_default().spawn_local(async move {
            let result = rx.await;
            this.refresh_core_state();
            match result {
                Ok(Ok(latency_micros)) => {
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
        let detailed = self.execute_query_detailed(query);
        let (tx, rx) = tokio::sync::oneshot::channel();
        glib::MainContext::ref_thread_default().spawn_local(async move {
            let result = detailed.await.unwrap_or_else(|error| {
                Err(dzban_core::error::ExecutionError::Message(
                    error.to_string(),
                ))
            });
            let _ = tx.send(result.map_err(|error| error.to_string()));
        });
        rx
    }

    pub fn execute_query_detailed(
        &self,
        query: String,
    ) -> QueryReceiver<QueryResult, dzban_core::error::ExecutionError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let Some(db_conn) = self.imp().db_connection.borrow().clone() else {
            let error = "Not connected".to_string();
            self.set_last_error(error.clone());
            self.emit_by_name::<()>("query-error", &[&error]);
            let _ = tx.send(Err(error.into()));
            return rx;
        };

        debug!("Executing query...");

        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();
        let request = match bridge.start_result(db_conn, query.clone()) {
            Ok(request) => request,
            Err(error) => {
                let _ = tx.send(Err(error));
                return rx;
            }
        };
        self.begin_query(&query);
        let this = self.clone();

        glib::MainContext::ref_thread_default().spawn_local(async move {
            let result = match request.await {
                Ok(result) => result,
                Err(_) => Err("Query worker was dropped".to_string().into()),
            };
            let result = match result {
                Ok(result) => {
                    let row_count = result.result_set.rows.len() as i64;
                    let duration_ms = result.duration.as_millis() as i64;
                    this.refresh_core_state();
                    this.emit_by_name::<()>("query-finished", &[&row_count, &duration_ms]);
                    Ok(result)
                }
                Err(error) => {
                    this.refresh_core_state();
                    let message = error.to_string();
                    this.set_last_error(message.clone());
                    this.emit_by_name::<()>("query-error", &[&message]);
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
    ) -> Result<PlannedQueryReceivers, String> {
        self.execute_planned_with_history(query, None)
    }

    pub fn execute_recorded_query(&self, query: String) -> Result<PlannedQueryReceivers, String> {
        let config = self
            .imp()
            .database_config
            .get()
            .expect("Database configuration not initialized");
        let history = match dzban_core::history::QueryHistoryStore::load(&config.name) {
            Ok(history) => Some(history),
            Err(error) => {
                tracing::warn!("Query history unavailable: {}", error);
                None
            }
        };
        self.execute_planned_with_history(query, history)
    }

    fn execute_planned_with_history(
        &self,
        query: String,
        history: Option<dzban_core::history::QueryHistoryStore>,
    ) -> Result<PlannedQueryReceivers, String> {
        let (query_tx, query_rx) = tokio::sync::oneshot::channel();
        let (plan_tx, plan_rx) = tokio::sync::oneshot::channel();
        let Some(db_conn) = self.imp().db_connection.borrow().clone() else {
            let error = "Not connected".to_string();
            self.set_last_error(error.clone());
            self.emit_by_name::<()>("query-error", &[&error]);
            return Err(error);
        };

        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();
        let (_, (result_rx, core_plan_rx)) = bridge
            .start_planned_identified(db_conn.clone(), query.clone(), history)
            .map_err(|error| error.to_string())?;
        self.begin_query(&query);
        let this = self.clone();
        glib::MainContext::ref_thread_default().spawn_local(async move {
            let plan = core_plan_rx
                .await
                .unwrap_or_else(|_| Err("Plan request was dropped".to_string().into()));
            let _ = plan_tx.send(plan);
            this.refresh_core_state();
            let result = result_rx
                .await
                .unwrap_or_else(|_| Err("Query cancelled".to_string().into()));
            match result {
                Ok(result) => {
                    let row_count = result.result_set.rows.len() as i64;
                    let duration_ms = result.duration.as_millis() as i64;
                    this.emit_by_name::<()>("query-finished", &[&row_count, &duration_ms]);
                    this.refresh_core_state();
                    let _ = query_tx.send(Ok(result));
                }
                Err(error) => {
                    this.refresh_core_state();
                    let message = error.to_string();
                    this.set_last_error(message.clone());
                    this.emit_by_name::<()>("query-error", &[&message]);
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
        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();
        let request = bridge
            .start_explain(db_conn, query)
            .map_err(|error| error.to_string())?;
        self.refresh_core_state();
        let this = self.clone();
        glib::MainContext::ref_thread_default().spawn_local(async move {
            let result = request
                .await
                .unwrap_or_else(|_| Err("Plan request was dropped".to_string().into()));
            this.refresh_core_state();
            let _ = tx.send(result.map_err(|error| error.to_string()));
        });
        Ok(rx)
    }

    pub fn schedule_preview(
        &self,
        service: dzban_core::preview::PlanPreviewService,
        document: dzban_core::documents::QueryDocument,
        sql: String,
    ) -> Result<dzban_core::preview::PreviewTarget, String> {
        let connection = self
            .imp()
            .db_connection
            .borrow()
            .clone()
            .ok_or_else(|| "Not connected".to_string())?;
        self.imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge initialized")
            .schedule_preview(connection, service, document, sql)
            .map_err(|error| error.to_string())
    }

    pub fn cancel_query(&self) -> tokio::sync::oneshot::Receiver<Result<(), String>> {
        self.cancel_query_for(None)
    }

    pub fn cancel_query_identified(
        &self,
        query: dzban_core::execution::QueryId,
    ) -> tokio::sync::oneshot::Receiver<Result<(), String>> {
        self.cancel_query_for(Some(query))
    }

    pub fn cancel_document(
        &self,
        document: dzban_core::documents::QueryDocument,
    ) -> Result<QueryReceiver<()>, String> {
        let connection = self
            .imp()
            .db_connection
            .borrow()
            .clone()
            .ok_or_else(|| "Not connected".to_string())?;
        self.imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge initialized")
            .cancel_document(connection, document)
            .map_err(|error| error.to_string())
    }

    pub fn query_scope(&self) -> Option<dzban_core::execution::QueryScope> {
        self.imp()
            .db_connection
            .borrow()
            .as_ref()
            .map(|connection| connection.engine().subscribe().borrow().scope())
    }

    pub fn session_interaction(&self) -> Option<dzban_core::interaction::SessionInteraction> {
        self.imp()
            .db_connection
            .borrow()
            .as_ref()
            .map(|connection| {
                dzban_core::interaction::SessionInteraction::from_engine(&connection.engine())
            })
    }

    pub fn session_engine(&self) -> Option<std::sync::Arc<dzban_core::execution::QueryEngine>> {
        self.imp()
            .db_connection
            .borrow()
            .as_ref()
            .map(|connection| connection.engine())
    }

    pub fn can_forget(&self) -> bool {
        self.imp().connection_selection.borrow().can_forget()
    }

    fn cancel_query_for(
        &self,
        query: Option<dzban_core::execution::QueryId>,
    ) -> tokio::sync::oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
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
        let request = match query {
            Some(query) => bridge.cancel_query_identified(db_conn, query),
            None => bridge.cancel_query(db_conn),
        };
        glib::MainContext::ref_thread_default().spawn_local(async move {
            let result = request
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
