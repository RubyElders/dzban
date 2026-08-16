use glib::prelude::*;
use glib::subclass::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::config_gobject::DbConfig;
use crate::connection_gobject::ConnectionState;

glib::wrapper! {
    pub struct DbManager(ObjectSubclass<imp::DbManager>);
}

impl DbManager {
    /// Create a new database manager
    pub fn new() -> Self {
        glib::Object::builder()
            .property("config-count", 0i32)
            .property("active-config-name", "")
            .build()
    }

    pub fn config_count(&self) -> i32 {
        self.property("config-count")
    }

    pub fn active_config_name(&self) -> String {
        self.property("active-config-name")
    }

    pub fn set_active_config_name(&self, name: String) {
        if name.is_empty() || self.has_config(&name) {
            self.set_property("active-config-name", name);
        }
    }

    /// Add a new database configuration
    /// Returns true if added successfully, false if name already exists
    pub fn add_config(&self, config_name: String, connection_string: String) -> bool {
        let configs = self.imp().configs.borrow();
        if configs.contains_key(&config_name) {
            return false;
        }
        drop(configs);

        let config = DbConfig::new(config_name.clone(), connection_string);

        self.connect_config_signals(&config);

        let mut configs = self.imp().configs.borrow_mut();
        configs.insert(config_name.clone(), config.clone());
        drop(configs);

        self.set_property("config-count", self.config_count() + 1);

        if self.config_count() == 1 {
            self.set_active_config_name(config_name.clone());
        }

        self.emit_by_name::<()>("config-added", &[&config_name, &config]);

        true
    }

    /// Remove a configuration by name
    pub fn remove_config(&self, config_name: &str) -> bool {
        let mut configs = self.imp().configs.borrow_mut();

        if let Some(config) = configs.remove(config_name) {
            drop(configs);

            config.close();

            self.set_property("config-count", self.config_count() - 1);

            let active = self.active_config_name();
            if active == config_name {
                let new_active = self
                    .get_all_config_names()
                    .first()
                    .cloned()
                    .unwrap_or_default();
                self.set_active_config_name(new_active);
            }

            self.emit_by_name::<()>("config-removed", &[&config_name]);

            true
        } else {
            false
        }
    }

    /// Get a configuration by name
    pub fn get_config(&self, config_name: &str) -> Option<DbConfig> {
        let configs = self.imp().configs.borrow();
        configs.get(config_name).cloned()
    }

    /// Get the currently active configuration
    pub fn active_config(&self) -> Option<DbConfig> {
        let name = self.active_config_name();
        if name.is_empty() {
            None
        } else {
            self.get_config(&name)
        }
    }

    /// Switch to a specific configuration
    pub fn switch_to_config(&self, config_name: &str) -> bool {
        if self.has_config(config_name) {
            let old_name = self.active_config_name();
            self.set_active_config_name(config_name.to_string());

            if old_name != config_name {
                self.emit_by_name::<()>("config-activated", &[&config_name]);
            }

            true
        } else {
            false
        }
    }

    /// Check if a configuration exists
    pub fn has_config(&self, config_name: &str) -> bool {
        let configs = self.imp().configs.borrow();
        configs.contains_key(config_name)
    }

    /// Get all configuration names
    pub fn get_all_config_names(&self) -> Vec<String> {
        let configs = self.imp().configs.borrow();
        configs.keys().cloned().collect()
    }

    /// Get all configurations
    pub fn get_all_configs(&self) -> Vec<DbConfig> {
        let configs = self.imp().configs.borrow();
        configs.values().cloned().collect()
    }

    /// Close all configurations and cleanup
    pub fn close_all(&self) {
        let configs = self.get_all_configs();
        for config in configs {
            config.close();
        }

        self.imp().configs.borrow_mut().clear();
        self.set_property("config-count", 0);
        self.set_property("active-config-name", "");

        self.emit_by_name::<()>("all-closed", &[]);
    }

    fn connect_config_signals(&self, config: &DbConfig) {
        let manager = self.clone();

        config.connect_closure(
            "tab-added",
            false,
            glib::closure_local!(
                #[watch]
                manager,
                move |_config: &DbConfig, index: i32, tab: crate::query_tab::DbQueryTab| {
                    manager.emit_by_name::<()>("tab-added", &[&index, &tab]);
                }
            ),
        );

        let manager = self.clone();

        config.connect_closure(
            "tab-removed",
            false,
            glib::closure_local!(
                #[watch]
                manager,
                move |_config: &DbConfig, index: i32| {
                    manager.emit_by_name::<()>("tab-removed", &[&index]);
                }
            ),
        );

        let manager = self.clone();

        config.connect_closure(
            "tab-switched",
            false,
            glib::closure_local!(
                #[watch]
                manager,
                move |_config: &DbConfig, index: i32| {
                    manager.emit_by_name::<()>("tab-switched", &[&index]);
                }
            ),
        );

        let manager = self.clone();

        config.connect_closure(
            "query-started",
            false,
            glib::closure_local!(
                #[watch]
                manager,
                move |_config: &DbConfig, query: String| {
                    manager.emit_by_name::<()>("query-started", &[&query]);
                }
            ),
        );

        let manager = self.clone();

        config.connect_closure(
            "query-finished",
            false,
            glib::closure_local!(
                #[watch]
                manager,
                move |_config: &DbConfig, rows: i64, duration: i64| {
                    manager.emit_by_name::<()>("query-finished", &[&rows, &duration]);
                }
            ),
        );

        let manager = self.clone();

        config.connect_closure(
            "query-error",
            false,
            glib::closure_local!(
                #[watch]
                manager,
                move |_config: &DbConfig, error: String| {
                    manager.emit_by_name::<()>("query-error", &[&error]);
                }
            ),
        );

        let manager = self.clone();

        config.connect_closure(
            "connection-state-changed",
            false,
            glib::closure_local!(
                #[watch]
                manager,
                move |_config: &DbConfig, old: ConnectionState, new: ConnectionState| {
                    manager.emit_by_name::<()>("connection-state-changed", &[&old, &new]);
                }
            ),
        );
    }
}

impl Default for DbManager {
    fn default() -> Self {
        Self::new()
    }
}

mod imp {
    use super::*;
    use std::cell::Cell;
    use std::sync::OnceLock;

    #[derive(Default)]
    pub struct DbManager {
        pub config_count: Cell<i32>,
        pub active_config_name: RefCell<String>,
        pub configs: RefCell<HashMap<String, DbConfig>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DbManager {
        const NAME: &'static str = "DzbanDbManager";
        type Type = super::DbManager;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for DbManager {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecInt::builder("config-count")
                        .minimum(0)
                        .default_value(0)
                        .readwrite()
                        .blurb("Number of database configurations")
                        .build(),
                    glib::ParamSpecString::builder("active-config-name")
                        .default_value(Some(""))
                        .readwrite()
                        .blurb("Name of the currently active configuration")
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "config-count" => {
                    let val: i32 = value.get().expect("type checked upstream");
                    self.config_count.set(val);
                }
                "active-config-name" => {
                    let val: String = value.get().expect("type checked upstream");
                    self.active_config_name.replace(val);
                }
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "config-count" => self.config_count.get().to_value(),
                "active-config-name" => self.active_config_name.borrow().to_value(),
                _ => unimplemented!(),
            }
        }

        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("config-added")
                        .param_types([String::static_type(), DbConfig::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("config-removed")
                        .param_types([String::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("config-activated")
                        .param_types([String::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("tab-added")
                        .param_types([
                            i32::static_type(),
                            crate::query_tab::DbQueryTab::static_type(),
                        ])
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
                    glib::subclass::Signal::builder("all-closed").build(),
                ]
            })
        }
    }
}
