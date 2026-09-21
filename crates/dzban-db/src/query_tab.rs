use glib::prelude::*;
use glib::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use tracing::{info, trace};

use crate::connection_gobject::ConnectionState;
use crate::connection_gobject::DbConnection;

mod imp {
    use super::*;

    pub struct DbQueryTab {
        pub connection: RefCell<Option<DbConnection>>,
        pub owns_connection: Cell<bool>,

        pub tab_label: RefCell<String>,
        pub sql_query: RefCell<String>,
        pub is_executing: Cell<bool>,
        pub result_count: Cell<i64>,
        pub is_active: Cell<bool>,
    }

    impl Default for DbQueryTab {
        fn default() -> Self {
            Self {
                connection: RefCell::new(None),
                owns_connection: Cell::new(false),
                tab_label: RefCell::new("Untitled".to_string()),
                sql_query: RefCell::new(String::new()),
                is_executing: Cell::new(false),
                result_count: Cell::new(0),
                is_active: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DbQueryTab {
        const NAME: &'static str = "DzbanDbQueryTab";
        type Type = super::DbQueryTab;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for DbQueryTab {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: std::sync::OnceLock<Vec<glib::ParamSpec>> =
                std::sync::OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecString::builder("tab-label").build(),
                    glib::ParamSpecString::builder("sql-query").build(),
                    glib::ParamSpecBoolean::builder("is-executing")
                        .read_only()
                        .build(),
                    glib::ParamSpecInt64::builder("result-count")
                        .minimum(0)
                        .default_value(0)
                        .read_only()
                        .build(),
                    glib::ParamSpecBoolean::builder("is-active").build(),
                ]
            })
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "tab-label" => self.tab_label.borrow().to_value(),
                "sql-query" => self.sql_query.borrow().to_value(),
                "is-executing" => self.is_executing.get().to_value(),
                "result-count" => self.result_count.get().to_value(),
                "is-active" => self.is_active.get().to_value(),
                _ => unimplemented!(),
            }
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "tab-label" => {
                    let label = value.get().unwrap();
                    self.tab_label.replace(label);
                }
                "sql-query" => {
                    let query = value.get().unwrap();
                    self.sql_query.replace(query);
                }
                "is-active" => {
                    let active = value.get().unwrap();
                    self.is_active.set(active);
                }
                _ => unimplemented!(),
            }
        }

        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: std::sync::OnceLock<Vec<glib::subclass::Signal>> =
                std::sync::OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("query-started")
                        .param_types([String::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("query-finished")
                        .param_types([i64::static_type(), i64::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("query-error")
                        .param_types([String::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("connection-state-changed")
                        .param_types([
                            ConnectionState::static_type(),
                            ConnectionState::static_type(),
                        ])
                        .build(),
                    glib::subclass::Signal::builder("tab-activated").build(),
                    glib::subclass::Signal::builder("tab-closed").build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    pub struct DbQueryTab(ObjectSubclass<imp::DbQueryTab>);
}

impl DbQueryTab {
    /// Create a new query tab with a connection to the specified database
    pub fn new(connection_string: String, tab_label: Option<String>) -> Self {
        let connection = DbConnection::new(connection_string);
        let obj = Self::with_connection(connection, tab_label);
        obj.imp().owns_connection.set(true);
        obj.connect_query_signals();
        obj
    }

    pub fn with_connection(connection: DbConnection, tab_label: Option<String>) -> Self {
        let obj: Self = glib::Object::builder()
            .property(
                "tab-label",
                tab_label.unwrap_or_else(|| "Untitled".to_string()),
            )
            .build();

        let obj_clone = obj.clone();
        connection.connect_closure(
            "state-changed",
            false,
            glib::closure_local!(
                #[watch]
                obj_clone,
                move |_conn: &DbConnection, old: ConnectionState, new: ConnectionState| {
                    obj_clone.emit_by_name::<()>("connection-state-changed", &[&old, &new]);
                }
            ),
        );

        obj.imp().connection.replace(Some(connection));

        obj
    }

    fn connect_query_signals(&self) {
        let connection = self
            .connection()
            .expect("DbQueryTab connection is initialized");

        let tab = self.clone();
        connection.connect_closure(
            "query-started",
            false,
            glib::closure_local!(
                #[watch]
                tab,
                move |_connection: &DbConnection, query: String| {
                    tab.set_is_executing(true);
                    tab.emit_by_name::<()>("query-started", &[&query]);
                }
            ),
        );

        let tab = self.clone();
        connection.connect_closure(
            "query-finished",
            false,
            glib::closure_local!(
                #[watch]
                tab,
                move |_connection: &DbConnection, rows: i64, duration_ms: i64| {
                    tab.set_is_executing(false);
                    tab.set_result_count(rows);
                    tab.emit_by_name::<()>("query-finished", &[&rows, &duration_ms]);
                }
            ),
        );

        let tab = self.clone();
        connection.connect_closure(
            "query-error",
            false,
            glib::closure_local!(
                #[watch]
                tab,
                move |_connection: &DbConnection, error: String| {
                    tab.set_is_executing(false);
                    tab.emit_by_name::<()>("query-error", &[&error]);
                }
            ),
        );
    }

    pub fn tab_label(&self) -> String {
        self.property("tab-label")
    }

    pub fn sql_query(&self) -> String {
        self.property("sql-query")
    }

    pub fn is_executing(&self) -> bool {
        self.property("is-executing")
    }

    pub fn result_count(&self) -> i64 {
        self.property("result-count")
    }

    pub fn is_active(&self) -> bool {
        self.property("is-active")
    }

    pub fn set_tab_label(&self, label: String) {
        self.imp().tab_label.replace(label);
        self.notify("tab-label");
    }

    pub fn set_sql_query(&self, query: String) {
        self.imp().sql_query.replace(query);
        self.notify("sql-query");
    }

    fn set_is_executing(&self, executing: bool) {
        self.imp().is_executing.set(executing);
        self.notify("is-executing");
    }

    fn set_result_count(&self, count: i64) {
        self.imp().result_count.set(count);
        self.notify("result-count");
    }

    pub fn set_is_active(&self, active: bool) {
        let was_active = self.imp().is_active.get();
        self.imp().is_active.set(active);
        self.notify("is-active");

        if active && !was_active {
            self.emit_by_name::<()>("tab-activated", &[]);
            info!("Tab activated: {}", self.tab_label());
        }
    }

    pub fn connection(&self) -> Option<DbConnection> {
        self.imp().connection.borrow().clone()
    }

    pub fn connection_state(&self) -> ConnectionState {
        self.connection()
            .map(|c| c.state())
            .unwrap_or(ConnectionState::Disconnected)
    }

    pub fn connect(&self) {
        if let Some(conn) = self.connection() {
            info!("Connecting tab: {}", self.tab_label());
            conn.connect();
        }
    }

    pub fn disconnect(&self) {
        if let Some(conn) = self.connection() {
            info!("Disconnecting tab: {}", self.tab_label());
            conn.disconnect();
        }
    }

    pub fn ping(&self) {
        if let Some(conn) = self.connection() {
            trace!("Pinging from tab: {}", self.tab_label());
            conn.ping();
        }
    }

    /// Execute the current SQL query
    pub fn execute_query(&self) {
        let query = self.sql_query();
        if query.trim().is_empty() {
            self.emit_by_name::<()>("query-error", &[&"Empty query".to_string()]);
            return;
        }

        if let Some(conn) = self.connection() {
            info!(
                "Executing query on tab: {} - {}",
                self.tab_label(),
                query.chars().take(50).collect::<String>()
            );
            if self.imp().owns_connection.get() {
                conn.execute_query(query);
                return;
            }
            if conn.state() != ConnectionState::Connected {
                drop(conn.execute_query_rows(query));
                let error = conn.last_error();
                self.emit_by_name::<()>("query-error", &[&error]);
                return;
            }

            self.set_is_executing(true);
            self.emit_by_name::<()>("query-started", &[&query]);
            let receiver = conn.execute_query_rows(query);
            let tab = self.clone();
            glib::MainContext::ref_thread_default().spawn_local(async move {
                match receiver.await {
                    Ok(Ok(result)) => {
                        let rows = result.rows.len() as i64;
                        let duration_ms = result.duration.as_millis() as i64;
                        tab.set_is_executing(false);
                        tab.set_result_count(rows);
                        tab.emit_by_name::<()>("query-finished", &[&rows, &duration_ms]);
                    }
                    Ok(Err(error)) => {
                        tab.set_is_executing(false);
                        tab.emit_by_name::<()>("query-error", &[&error]);
                    }
                    Err(_) => {
                        let error = "Query result channel closed".to_string();
                        tab.set_is_executing(false);
                        tab.emit_by_name::<()>("query-error", &[&error]);
                    }
                }
            });
        } else {
            self.emit_by_name::<()>("query-error", &[&"No connection".to_string()]);
        }
    }

    /// Close this tab (emit signal and cleanup)
    pub fn close(&self) {
        info!("Closing tab: {}", self.tab_label());
        if self.imp().owns_connection.get() {
            self.disconnect();
        }
        self.emit_by_name::<()>("tab-closed", &[]);
    }
}

impl Default for DbQueryTab {
    fn default() -> Self {
        Self::new("".to_string(), None)
    }
}
