use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::RefCell;
use std::time::Duration;
use dzban_db::{DbConnection, ConnectionState};

mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    pub struct ConnectionModel {
        pub db_name: RefCell<String>,
        pub connection: RefCell<Option<DbConnection>>,
        pub latency_micros: Cell<i64>, // Store as i64 for GObject compatibility
        pub version: RefCell<Option<String>>,
        pub state: Cell<i32>, // Store as i32 for GObject compatibility
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ConnectionModel {
        const NAME: &'static str = "DzbanConnectionModel";
        type Type = super::ConnectionModel;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for ConnectionModel {
        fn properties() -> &'static [glib::ParamSpec] {
            use std::sync::OnceLock;
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecString::builder("db-name")
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
                    glib::ParamSpecInt::builder("state")
                        .minimum(0)
                        .maximum(3)
                        .default_value(0)
                        .read_only()
                        .build(),
                ]
            })
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "db-name" => self.db_name.borrow().to_value(),
                "latency-micros" => self.latency_micros.get().to_value(),
                "version" => self.version.borrow().to_value(),
                "state" => self.state.get().to_value(),
                _ => unimplemented!(),
            }
        }
    }
}

glib::wrapper! {
    pub struct ConnectionModel(ObjectSubclass<imp::ConnectionModel>);
}

impl ConnectionModel {
    pub fn new(db_name: String) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().db_name.replace(db_name);
        obj
    }

    pub fn set_connection(&self, connection: DbConnection) {
        self.imp().connection.replace(Some(connection));
    }

    pub fn get_connection(&self) -> Option<DbConnection> {
        self.imp().connection.borrow().clone()
    }

    pub fn db_name(&self) -> String {
        self.property("db-name")
    }

    pub fn latency_micros(&self) -> i64 {
        self.property("latency-micros")
    }

    pub fn latency(&self) -> Option<Duration> {
        let micros = self.latency_micros();
        if micros >= 0 {
            Some(Duration::from_micros(micros as u64))
        } else {
            None
        }
    }

    pub fn version(&self) -> Option<String> {
        self.property("version")
    }

    pub fn state(&self) -> ConnectionState {
        let state_int: i32 = self.property("state");
        match state_int {
            0 => ConnectionState::Disconnected,
            1 => ConnectionState::Connecting,
            2 => ConnectionState::Connected,
            3 => ConnectionState::Error,
            _ => ConnectionState::Disconnected,
        }
    }

    /// Update latency and notify property observers
    pub fn update_latency(&self, latency: Option<Duration>) {
        let micros = latency.map(|d| d.as_micros() as i64).unwrap_or(-1);
        self.imp().latency_micros.set(micros);
        self.notify("latency-micros");
    }

    /// Update version and notify property observers
    pub fn update_version(&self, version: Option<String>) {
        self.imp().version.replace(version.clone());
        self.notify("version");
    }

    /// Update state and notify property observers
    pub fn update_state(&self, state: ConnectionState) {
        let state_int = match state {
            ConnectionState::Disconnected => 0,
            ConnectionState::Connecting => 1,
            ConnectionState::Connected => 2,
            ConnectionState::Error => 3,
        };
        self.imp().state.set(state_int);
        self.notify("state");
    }

    /// Start monitoring the connection and updating properties automatically
    pub fn start_monitoring(&self) {
        let conn = match self.get_connection() {
            Some(c) => c,
            None => return,
        };

        let model = self.clone();

        // Monitor state changes
        let conn_state = conn.clone();
        let model_state = model.clone();
        gtk4::glib::spawn_future_local(async move {
            loop {
                let state = conn_state.get_state().await;
                model_state.update_state(state);
                
                // Poll every 100ms for state changes
                gtk4::glib::timeout_future(std::time::Duration::from_millis(100)).await;
            }
        });

        // Monitor version
        let conn_version = conn.clone();
        let model_version = model.clone();
        gtk4::glib::spawn_future_local(async move {
            let version = conn_version.get_version().await;
            model_version.update_version(version);
        });

        // Monitor latency - update whenever it changes
        let conn_latency = conn.clone();
        let model_latency = model.clone();
        gtk4::glib::spawn_future_local(async move {
            let mut last_latency: Option<Duration> = None;
            loop {
                let latency = conn_latency.get_latency().await;
                if latency != last_latency {
                    model_latency.update_latency(latency);
                    last_latency = latency;
                }
                
                // Poll every 100ms for latency changes
                gtk4::glib::timeout_future(std::time::Duration::from_millis(100)).await;
            }
        });
    }
}
