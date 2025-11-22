// Reactive connection manager using GObject properties (UI-independent)
use glib;
use glib::prelude::*;
use glib::subclass::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::config::DatabaseConfig;
use super::connection::{DbConnection, ConnectionState};

// GObject wrapper for DbConnection to make it reactive
glib::wrapper! {
    pub struct ReactiveConnection(ObjectSubclass<imp::ReactiveConnection>);
}

impl ReactiveConnection {
    pub fn new(db_name: &str, _config: DatabaseConfig) -> Self {
        glib::Object::builder()
            .property("db-name", db_name)
            .property("state", ConnectionState::Disconnected as i32)
            .property("version", "")
            .property("latency", 0u64)
            .build()
    }

    pub fn db_name(&self) -> String {
        self.property("db-name")
    }

    pub fn state(&self) -> ConnectionState {
        let val: i32 = self.property("state");
        match val {
            0 => ConnectionState::Disconnected,
            1 => ConnectionState::Connecting,
            2 => ConnectionState::Connected,
            3 => ConnectionState::Error,
            _ => ConnectionState::Disconnected,
        }
    }

    pub fn set_state(&self, state: ConnectionState) {
        let val = state as i32;
        self.set_property("state", val);
    }

    pub fn version(&self) -> String {
        self.property("version")
    }

    pub fn set_version(&self, version: &str) {
        self.set_property("version", version);
    }

    pub fn latency(&self) -> u64 {
        self.property("latency")
    }

    pub fn set_latency(&self, latency_ms: u64) {
        self.set_property("latency", latency_ms);
    }

    pub fn connection(&self) -> Option<DbConnection> {
        self.imp().connection.borrow().clone()
    }

    pub fn set_connection(&self, conn: Option<DbConnection>) {
        *self.imp().connection.borrow_mut() = conn;
    }
}

mod imp {
    use super::*;
    use std::cell::Cell;

    pub struct ReactiveConnection {
        pub db_name: RefCell<String>,
        pub state: Cell<i32>,
        pub version: RefCell<String>,
        pub latency: Cell<u64>,
        pub connection: RefCell<Option<DbConnection>>,
    }

    impl Default for ReactiveConnection {
        fn default() -> Self {
            Self {
                db_name: RefCell::new(String::new()),
                state: Cell::new(0),
                version: RefCell::new(String::new()),
                latency: Cell::new(0),
                connection: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ReactiveConnection {
        const NAME: &'static str = "DzbanReactiveConnection";
        type Type = super::ReactiveConnection;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for ReactiveConnection {
        fn properties() -> &'static [glib::ParamSpec] {
            use std::sync::OnceLock;
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecString::builder("db-name")
                        .construct_only()
                        .build(),
                    glib::ParamSpecInt::builder("state")
                        .minimum(0)
                        .maximum(3)
                        .default_value(0)
                        .readwrite()
                        .build(),
                    glib::ParamSpecString::builder("version")
                        .default_value(Some(""))
                        .readwrite()
                        .build(),
                    glib::ParamSpecUInt64::builder("latency")
                        .default_value(0)
                        .readwrite()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "db-name" => {
                    let val = value.get().expect("type checked upstream");
                    self.db_name.replace(val);
                }
                "state" => {
                    let val = value.get().expect("type checked upstream");
                    self.state.set(val);
                }
                "version" => {
                    let val = value.get().expect("type checked upstream");
                    self.version.replace(val);
                }
                "latency" => {
                    let val = value.get().expect("type checked upstream");
                    self.latency.set(val);
                }
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "db-name" => self.db_name.borrow().to_value(),
                "state" => self.state.get().to_value(),
                "version" => self.version.borrow().to_value(),
                "latency" => self.latency.get().to_value(),
                _ => unimplemented!(),
            }
        }
    }
}

// The reactive manager that holds all connections
glib::wrapper! {
    pub struct ReactiveConnectionManager(ObjectSubclass<imp_manager::ReactiveConnectionManager>);
}

impl ReactiveConnectionManager {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn add_connection(&self, conn: ReactiveConnection) {
        let db_name = conn.db_name();
        self.imp().connections.borrow_mut().insert(db_name.clone(), conn.clone());
        self.emit_by_name::<()>("connection-added", &[&db_name]);
    }

    pub fn remove_connection(&self, db_name: &str) {
        if self.imp().connections.borrow_mut().remove(db_name).is_some() {
            self.emit_by_name::<()>("connection-removed", &[&db_name]);
        }
    }

    pub fn get_connection(&self, db_name: &str) -> Option<ReactiveConnection> {
        self.imp().connections.borrow().get(db_name).cloned()
    }

    pub fn all_connections(&self) -> Vec<ReactiveConnection> {
        self.imp().connections.borrow().values().cloned().collect()
    }

    /// Connect to a database and start monitoring
    pub fn connect_async(&self, db_name: String, config: DatabaseConfig) {
        let manager = self.clone();
        
        gtk4::glib::spawn_future_local(async move {
            // Create or get reactive connection
            let reactive_conn = manager.get_connection(&db_name)
                .unwrap_or_else(|| {
                    let rc = ReactiveConnection::new(&db_name, config.clone());
                    manager.add_connection(rc.clone());
                    rc
                });

            reactive_conn.set_state(ConnectionState::Connecting);

            // Create actual connection
            let conn_string = config.connection_string();
            let db_conn = DbConnection::new(db_name.clone(), conn_string);
            
            // Store connection
            reactive_conn.set_connection(Some(db_conn.clone()));
            
            // Connect
            match db_conn.connect().await {
                Ok(_) => {
                    reactive_conn.set_state(ConnectionState::Connected);
                    
                    // Get version
                    if let Some(version) = db_conn.get_version().await {
                        reactive_conn.set_version(&version);
                    }
                    
                    // Start monitoring loop
                    manager.start_monitoring(reactive_conn.clone());
                }
                Err(e) => {
                    eprintln!("Connection failed: {}", e);
                    reactive_conn.set_state(ConnectionState::Error);
                }
            }
        });
    }

    /// Start monitoring a connection (ping every 5 seconds)
    fn start_monitoring(&self, reactive_conn: ReactiveConnection) {
        gtk4::glib::spawn_future_local(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            
            loop {
                interval.tick().await;
                
                // Check if connection still exists
                if let Some(db_conn) = reactive_conn.connection() {
                    match db_conn.ping().await {
                        Ok(latency) => {
                            reactive_conn.set_latency(latency.as_millis() as u64);
                            reactive_conn.set_state(ConnectionState::Connected);
                        }
                        Err(e) => {
                            eprintln!("Ping failed: {}", e);
                            reactive_conn.set_state(ConnectionState::Error);
                            break; // Stop monitoring on error
                        }
                    }
                } else {
                    break; // Connection removed
                }
            }
        });
    }

    /// Disconnect from a database
    pub fn disconnect_async(&self, db_name: String) {
        if let Some(reactive_conn) = self.get_connection(&db_name) {
            if let Some(db_conn) = reactive_conn.connection() {
                gtk4::glib::spawn_future_local(async move {
                    db_conn.disconnect().await;
                });
            }
            reactive_conn.set_state(ConnectionState::Disconnected);
            reactive_conn.set_connection(None);
        }
    }
}

impl Default for ReactiveConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

mod imp_manager {
    use super::*;

    #[derive(Default)]
    pub struct ReactiveConnectionManager {
        pub connections: RefCell<HashMap<String, ReactiveConnection>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ReactiveConnectionManager {
        const NAME: &'static str = "DzbanReactiveConnectionManager";
        type Type = super::ReactiveConnectionManager;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for ReactiveConnectionManager {
        fn signals() -> &'static [glib::subclass::Signal] {
            use std::sync::OnceLock;
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("connection-added")
                        .param_types([String::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("connection-removed")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }
    }
}
