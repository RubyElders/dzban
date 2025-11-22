// Reactive connection manager using GObject properties (UI-independent)
use glib;
use glib::prelude::*;
use glib::subclass::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use tracing::{info, warn, error, debug, instrument};

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

    /// Test a connection without keeping it alive
    /// Updates the ReactiveConnection's state property
    #[instrument(skip(self, config), fields(db_name = %db_name))]
    pub fn test_connection(&self, db_name: String, config: DatabaseConfig) {
        info!("Testing connection to '{}'", db_name);
        
        let manager = self.clone();
        
        glib::spawn_future_local(async move {
            // Get or create reactive connection
            let reactive_conn = manager.get_connection(&db_name)
                .unwrap_or_else(|| {
                    debug!("Creating new ReactiveConnection for '{}'", db_name);
                    let rc = ReactiveConnection::new(&db_name, config.clone());
                    manager.add_connection(rc.clone());
                    rc
                });
            
            debug!("Setting state to Connecting for '{}'", db_name);
            reactive_conn.set_state(ConnectionState::Connecting);
            
            // Create temporary connection for testing
            let conn_string = config.connection_string();
            let db_conn = DbConnection::new(db_name.clone(), conn_string);
            
            debug!("Attempting to connect to '{}'", db_name);
            match db_conn.connect().await {
                Ok(_) => {
                    info!("✅ Connection test successful for '{}'", db_name);
                    reactive_conn.set_state(ConnectionState::Connected);
                    
                    // Get version
                    if let Some(version) = db_conn.get_version().await {
                        debug!("Got version for '{}': {}", db_name, version);
                        reactive_conn.set_version(&version);
                    }
                    
                    // Disconnect after test
                    debug!("Disconnecting test connection to '{}'", db_name);
                    db_conn.disconnect().await;
                    
                    // After 2 seconds, reset to Disconnected
                    glib::timeout_add_seconds_local_once(2, {
                        let reactive_conn = reactive_conn.clone();
                        let db_name = db_name.clone();
                        move || {
                            debug!("Resetting '{}' to Disconnected state after test", db_name);
                            reactive_conn.set_state(ConnectionState::Disconnected);
                        }
                    });
                }
                Err(e) => {
                    error!("❌ Connection test failed for '{}': {}", db_name, e);
                    reactive_conn.set_state(ConnectionState::Error);
                }
            }
        });
    }

    /// Connect to a database and start monitoring
    #[instrument(skip(self, config), fields(db_name = %db_name))]
    pub fn connect_async(&self, db_name: String, config: DatabaseConfig) {
        info!("Opening connection to '{}'", db_name);
        
        let manager = self.clone();
        
        glib::spawn_future_local(async move {
            // Create or get reactive connection
            let reactive_conn = manager.get_connection(&db_name)
                .unwrap_or_else(|| {
                    debug!("Creating new ReactiveConnection for '{}'", db_name);
                    let rc = ReactiveConnection::new(&db_name, config.clone());
                    manager.add_connection(rc.clone());
                    rc
                });

            debug!("Setting state to Connecting for '{}'", db_name);
            reactive_conn.set_state(ConnectionState::Connecting);

            // Create actual connection
            let conn_string = config.connection_string();
            let db_conn = DbConnection::new(db_name.clone(), conn_string);
            
            // Store connection
            reactive_conn.set_connection(Some(db_conn.clone()));
            
            // Connect
            debug!("Establishing connection to '{}'", db_name);
            match db_conn.connect().await {
                Ok(_) => {
                    info!("✅ Connected to '{}'", db_name);
                    reactive_conn.set_state(ConnectionState::Connected);
                    
                    // Get version
                    debug!("Fetching PostgreSQL version for '{}'", db_name);
                    if let Some(version) = db_conn.get_version().await {
                        info!("PostgreSQL version for '{}': {}", db_name, version);
                        reactive_conn.set_version(&version);
                    }
                    
                    // Start monitoring loop
                    info!("Starting background monitoring for '{}'", db_name);
                    manager.start_monitoring(reactive_conn.clone());
                }
                Err(e) => {
                    error!("❌ Connection failed for '{}': {}", db_name, e);
                    reactive_conn.set_state(ConnectionState::Error);
                }
            }
        });
    }

    /// Start monitoring a connection (ping every 5 seconds)
    #[instrument(skip(self, reactive_conn), fields(db_name = %reactive_conn.db_name()))]
    fn start_monitoring(&self, reactive_conn: ReactiveConnection) {
        let db_name = reactive_conn.db_name();
        debug!("Starting monitoring loop for '{}'", db_name);
        
        glib::spawn_future_local(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            let mut ping_count = 0u64;
            
            loop {
                interval.tick().await;
                ping_count += 1;
                
                // Check if connection still exists and is connected
                if reactive_conn.state() != ConnectionState::Connected {
                    info!("Stopping monitoring for '{}' (state: {:?})", db_name, reactive_conn.state());
                    break;
                }
                
                // Check if connection still exists
                if let Some(db_conn) = reactive_conn.connection() {
                    debug!("Ping #{} for '{}'", ping_count, db_name);
                    
                    match db_conn.ping().await {
                        Ok(latency) => {
                            let latency_ms = latency.as_millis() as u64;
                            debug!("✅ Ping successful for '{}': {}ms", db_name, latency_ms);
                            reactive_conn.set_latency(latency_ms);
                            reactive_conn.set_state(ConnectionState::Connected);
                        }
                        Err(e) => {
                            error!("❌ Ping failed for '{}': {}", db_name, e);
                            reactive_conn.set_state(ConnectionState::Error);
                            break; // Stop monitoring on error
                        }
                    }
                } else {
                    warn!("Connection object removed for '{}', stopping monitoring", db_name);
                    break; // Connection removed
                }
            }
            
            info!("Monitoring stopped for '{}' after {} pings", db_name, ping_count);
        });
    }

    /// Disconnect from a database
    #[instrument(skip(self), fields(db_name = %db_name))]
    pub fn disconnect_async(&self, db_name: String) {
        info!("Disconnecting from '{}'", db_name);
        
        if let Some(reactive_conn) = self.get_connection(&db_name) {
            if let Some(db_conn) = reactive_conn.connection() {
                debug!("Closing database connection for '{}'", db_name);
                glib::spawn_future_local(async move {
                    db_conn.disconnect().await;
                });
            }
            
            debug!("Setting state to Disconnected for '{}'", db_name);
            reactive_conn.set_state(ConnectionState::Disconnected);
            reactive_conn.set_connection(None);
            reactive_conn.set_latency(0); // Reset latency
            
            info!("✅ Disconnected from '{}'", db_name);
        } else {
            warn!("Attempted to disconnect from unknown connection '{}'", db_name);
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
