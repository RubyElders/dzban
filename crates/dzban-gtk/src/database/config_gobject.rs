use glib::prelude::*;
use glib::subclass::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::database::DatabaseConfig;
use crate::database::connection_gobject::ConnectionState;
use crate::database::connection_gobject::DbConnection;
use crate::database::query_tab::DbQueryTab;

glib::wrapper! {
    pub struct DbConfig(ObjectSubclass<imp::DbConfig>);
}

impl DbConfig {
    /// Create a new database configuration
    ///
    /// # Arguments
    /// * `config_name` - Unique name for this configuration (e.g., "Production", "Staging")
    /// * `connection_string` - PostgreSQL connection string for this config
    pub fn new(config_name: String, connection_string: String) -> Self {
        let mut database_config = DatabaseConfig::from_connection_string(&connection_string);
        database_config.name = config_name.clone();
        Self::build(
            config_name,
            connection_string,
            database_config,
            crate::database::RuntimeBridge::default(),
        )
    }

    pub fn from_database_config(database_config: DatabaseConfig) -> Self {
        Self::from_database_config_with_bridge(
            database_config,
            crate::database::RuntimeBridge::default(),
        )
    }

    pub fn from_database_config_with_bridge(
        database_config: DatabaseConfig,
        bridge: crate::database::RuntimeBridge,
    ) -> Self {
        let config_name = database_config.name.clone();
        let connection_string = database_config.connection_string();
        Self::build(config_name, connection_string, database_config, bridge)
    }

    fn build(
        config_name: String,
        connection_string: String,
        database_config: DatabaseConfig,
        bridge: crate::database::RuntimeBridge,
    ) -> Self {
        let obj: Self = glib::Object::builder()
            .property("config-name", config_name.clone())
            .property("connection-string", connection_string.clone())
            .property("tab-count", 0i32)
            .property("active-tab-index", -1i32)
            .build();

        let connection = DbConnection::build(connection_string, database_config, bridge);
        let config = obj.clone();
        connection.connect_closure(
            "state-changed",
            false,
            glib::closure_local!(
                #[watch]
                config,
                move |_connection: &DbConnection, old: ConnectionState, new: ConnectionState| {
                    config.emit_by_name::<()>("connection-state-changed", &[&old, &new]);
                }
            ),
        );
        obj.imp().connection.replace(Some(connection));

        obj
    }

    pub fn config_name(&self) -> String {
        self.property("config-name")
    }

    pub fn connection_string(&self) -> String {
        self.property("connection-string")
    }

    pub fn tab_count(&self) -> i32 {
        self.property("tab-count")
    }

    pub fn active_tab_index(&self) -> i32 {
        self.property("active-tab-index")
    }

    pub fn connection(&self) -> DbConnection {
        self.imp()
            .connection
            .borrow()
            .clone()
            .expect("DbConfig connection is initialized")
    }

    pub fn set_active_tab_index(&self, index: i32) {
        if index >= -1 && index < self.tab_count() {
            self.set_property("active-tab-index", index);
            if index >= 0 {
                self.emit_by_name::<()>("tab-switched", &[&index]);
            }
        }
    }

    /// Add a new query tab with optional label
    /// Returns the index of the new tab
    pub fn add_tab(&self, tab_label: Option<String>) -> i32 {
        let tab = DbQueryTab::with_connection(
            self.connection(),
            tab_label.or_else(|| Some(format!("Tab {}", self.tab_count() + 1))),
        );

        let tab_id = format!("tab_{}", self.tab_count());

        self.connect_tab_signals(&tab);

        let mut tabs = self.imp().tabs.borrow_mut();
        let index = self.tab_count();
        tabs.insert(tab_id, tab.clone());
        drop(tabs);

        self.set_property("tab-count", index + 1);

        if index == 0 {
            self.set_active_tab_index(0);
        }

        self.emit_by_name::<()>("tab-added", &[&index, &tab]);

        index
    }

    /// Remove a tab by index
    pub fn remove_tab(&self, index: i32) -> bool {
        let old_count = self.tab_count();
        if index < 0 || index >= old_count {
            return false;
        }

        let tab_id = format!("tab_{}", index);
        let mut tabs = self.imp().tabs.borrow_mut();

        if let Some(tab) = tabs.remove(&tab_id) {
            for old_index in (index + 1)..old_count {
                let old_id = format!("tab_{}", old_index);
                if let Some(tab) = tabs.remove(&old_id) {
                    tabs.insert(format!("tab_{}", old_index - 1), tab);
                }
            }
            drop(tabs);

            tab.close();

            let new_count = old_count - 1;
            self.set_property("tab-count", new_count);

            let active = self.active_tab_index();
            if active == index {
                self.set_active_tab_index(if new_count == 0 {
                    -1
                } else if index > 0 {
                    index - 1
                } else {
                    0
                });
            } else if active > index {
                self.set_active_tab_index(active - 1);
            }

            self.emit_by_name::<()>("tab-removed", &[&index]);

            true
        } else {
            false
        }
    }

    /// Get a tab by index
    pub fn get_tab(&self, index: i32) -> Option<DbQueryTab> {
        if index < 0 || index >= self.tab_count() {
            return None;
        }

        let tab_id = format!("tab_{}", index);
        let tabs = self.imp().tabs.borrow();
        tabs.get(&tab_id).cloned()
    }

    /// Get the currently active tab
    pub fn active_tab(&self) -> Option<DbQueryTab> {
        let index = self.active_tab_index();
        if index >= 0 {
            self.get_tab(index)
        } else {
            None
        }
    }

    /// Switch to a specific tab
    pub fn switch_to_tab(&self, index: i32) -> bool {
        if index >= 0 && index < self.tab_count() {
            self.set_active_tab_index(index);

            if let Some(tab) = self.get_tab(index) {
                tab.set_is_active(true);
            }

            for i in 0..self.tab_count() {
                if i != index
                    && let Some(tab) = self.get_tab(i)
                {
                    tab.set_is_active(false);
                }
            }

            true
        } else {
            false
        }
    }

    /// Get all tabs
    pub fn get_all_tabs(&self) -> Vec<DbQueryTab> {
        let tabs = self.imp().tabs.borrow();
        let mut result = Vec::new();

        for i in 0..self.tab_count() {
            let tab_id = format!("tab_{}", i);
            if let Some(tab) = tabs.get(&tab_id) {
                result.push(tab.clone());
            }
        }

        result
    }

    /// Connect the shared database connection
    pub fn connect_all(&self) {
        self.connection().connect();
    }

    /// Disconnect the shared database connection
    pub fn disconnect_all(&self) {
        self.connection().disconnect();
    }

    /// Check whether the shared database connection is connected
    pub fn all_connected(&self) -> bool {
        self.connection().state() == ConnectionState::Connected
    }

    /// Close all tabs and cleanup
    pub fn close(&self) {
        let tabs = self.get_all_tabs();
        for tab in tabs {
            tab.close();
        }
        self.connection().disconnect();

        self.imp().tabs.borrow_mut().clear();
        self.set_property("tab-count", 0);
        self.set_active_tab_index(-1);

        self.emit_by_name::<()>("config-closed", &[]);
    }

    fn connect_tab_signals(&self, tab: &DbQueryTab) {
        let config = self.clone();

        tab.connect_closure(
            "query-started",
            false,
            glib::closure_local!(
                #[watch]
                config,
                move |_tab: &DbQueryTab, query: String| {
                    config.emit_by_name::<()>("query-started", &[&query]);
                }
            ),
        );

        let config = self.clone();

        tab.connect_closure(
            "query-finished",
            false,
            glib::closure_local!(
                #[watch]
                config,
                move |_tab: &DbQueryTab, rows: i64, duration: i64| {
                    config.emit_by_name::<()>("query-finished", &[&rows, &duration]);
                }
            ),
        );

        let config = self.clone();

        tab.connect_closure(
            "query-error",
            false,
            glib::closure_local!(
                #[watch]
                config,
                move |_tab: &DbQueryTab, error: String| {
                    config.emit_by_name::<()>("query-error", &[&error]);
                }
            ),
        );
    }
}

mod imp {
    use super::*;
    use std::cell::Cell;
    use std::sync::OnceLock;

    #[derive(Default)]
    pub struct DbConfig {
        pub config_name: RefCell<String>,
        pub connection_string: RefCell<String>,
        pub tab_count: Cell<i32>,
        pub active_tab_index: Cell<i32>,
        pub tabs: RefCell<HashMap<String, DbQueryTab>>,
        pub connection: RefCell<Option<DbConnection>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DbConfig {
        const NAME: &'static str = "DzbanDbConfig";
        type Type = super::DbConfig;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for DbConfig {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecString::builder("config-name")
                        .construct_only()
                        .blurb("Unique name for this database configuration")
                        .build(),
                    glib::ParamSpecString::builder("connection-string")
                        .construct_only()
                        .blurb("PostgreSQL connection string")
                        .build(),
                    glib::ParamSpecInt::builder("tab-count")
                        .minimum(0)
                        .default_value(0)
                        .readwrite()
                        .blurb("Number of query tabs in this config")
                        .build(),
                    glib::ParamSpecInt::builder("active-tab-index")
                        .minimum(-1)
                        .default_value(-1)
                        .readwrite()
                        .blurb("Index of the currently active tab (-1 if none)")
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "config-name" => {
                    let val: String = value.get().expect("type checked upstream");
                    self.config_name.replace(val);
                }
                "connection-string" => {
                    let val: String = value.get().expect("type checked upstream");
                    self.connection_string.replace(val);
                }
                "tab-count" => {
                    let val: i32 = value.get().expect("type checked upstream");
                    self.tab_count.set(val);
                }
                "active-tab-index" => {
                    let val: i32 = value.get().expect("type checked upstream");
                    self.active_tab_index.set(val);
                }
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "config-name" => self.config_name.borrow().to_value(),
                "connection-string" => self.connection_string.borrow().to_value(),
                "tab-count" => self.tab_count.get().to_value(),
                "active-tab-index" => self.active_tab_index.get().to_value(),
                _ => unimplemented!(),
            }
        }

        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("tab-added")
                        .param_types([i32::static_type(), DbQueryTab::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("tab-removed")
                        .param_types([i32::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("tab-switched")
                        .param_types([i32::static_type()])
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
                    glib::subclass::Signal::builder("connection-state-changed")
                        .param_types([
                            ConnectionState::static_type(),
                            ConnectionState::static_type(),
                        ])
                        .build(),
                    glib::subclass::Signal::builder("config-closed").build(),
                ]
            })
        }
    }
}
