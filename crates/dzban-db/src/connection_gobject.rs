use glib::prelude::*;
use glib::subclass::prelude::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::time::Duration;
use tracing::{debug, error, info, instrument, trace, warn};

use crate::internal_connection::InternalConnection;
use crate::runtime_bridge::RuntimeBridge;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[enum_type(name = "DzbanConnectionState")]
pub enum ConnectionState {
    #[default]
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
    Error = 3,
}

pub struct QueryResult {
    pub rows: Vec<tokio_postgres::Row>,
    pub duration: Duration,
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
        pub query_running: Cell<bool>,
        pub active_query_count: Cell<u32>,

        pub(super) runtime_bridge: OnceCell<RuntimeBridge>,
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
                query_running: Cell::new(false),
                active_query_count: Cell::new(0),
                runtime_bridge: OnceCell::new(),
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
                "query-running" => self.query_running.get().to_value(),
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
    pub fn new(connection_string: String) -> Self {
        let obj: Self = glib::Object::builder()
            .property("connection-string", connection_string)
            .build();

        let bridge = RuntimeBridge::new().expect("Failed to create RuntimeBridge");
        obj.imp()
            .runtime_bridge
            .set(bridge)
            .expect("RuntimeBridge already initialized");

        obj
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

    fn set_state(&self, new_state: ConnectionState) {
        let old_state = self.imp().state.get();
        if old_state != new_state {
            self.imp().state.set(new_state);
            self.notify("state");
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

    fn begin_query(&self, query: &str) {
        let count = self.imp().active_query_count.get();
        self.imp().active_query_count.set(count + 1);
        if count == 0 {
            self.imp().query_running.set(true);
            self.notify("query-running");
        }
        self.set_last_error(String::new());
        self.emit_by_name::<()>("query-started", &[&query]);
    }

    fn finish_query(&self) {
        let count = self.imp().active_query_count.get();
        let new_count = count.saturating_sub(1);
        self.imp().active_query_count.set(new_count);
        if count > 0 && new_count == 0 {
            self.imp().query_running.set(false);
            self.notify("query-running");
        }
    }

    /// Connect to the database asynchronously
    /// Emits state-changed signals during the process
    #[instrument(skip(self))]
    pub fn connect(&self) {
        info!("Starting connection...");
        self.set_state(ConnectionState::Connecting);

        let conn_string = self.connection_string();
        let this = self.clone();

        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized");

        let config = crate::DatabaseConfig::from_connection_string(&conn_string);

        let rx = bridge.connect(config);

        glib::MainContext::ref_thread_default().spawn_local(async move {
            match rx.await {
                Ok(Ok(db_conn)) => {
                    info!("Connected successfully");

                    match db_conn.get_version().await {
                        Ok(version) => {
                            let version_short = version
                                .split_whitespace()
                                .take(2)
                                .collect::<Vec<_>>()
                                .join(" ");
                            this.set_version(Some(version_short));
                        }
                        Err(e) => {
                            warn!("Failed to get PostgreSQL version: {}", e);
                        }
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
        self.set_state(ConnectionState::Disconnected);
        self.set_version(None);
        self.set_latency_micros(-1);
        self.set_last_error(String::new());
        debug!("Disconnected");
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

        glib::MainContext::ref_thread_default().spawn_local(async move {
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

        debug!("Executing query...");
        self.begin_query(&query);

        let bridge = self
            .imp()
            .runtime_bridge
            .get()
            .expect("RuntimeBridge not initialized")
            .clone();
        let this = self.clone();

        glib::MainContext::ref_thread_default().spawn_local(async move {
            let start = std::time::Instant::now();
            let result = match bridge.execute_query_rows(db_conn, query).await {
                Ok(result) => result,
                Err(_) => Err("Query cancelled".to_string()),
            };
            let result = match result {
                Ok(rows) => {
                    let duration = start.elapsed();
                    let row_count = rows.len() as i64;
                    let duration_ms = duration.as_millis() as i64;
                    this.finish_query();
                    this.emit_by_name::<()>("query-finished", &[&row_count, &duration_ms]);
                    Ok(QueryResult { rows, duration })
                }
                Err(error) => {
                    this.finish_query();
                    this.set_last_error(error.clone());
                    this.emit_by_name::<()>("query-error", &[&error]);
                    Err(error)
                }
            };
            let _ = tx.send(result);
        });

        rx
    }

    pub fn cancel_query(&self) -> tokio::sync::oneshot::Receiver<Result<(), String>> {
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
