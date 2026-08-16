// dzban-db: UI-independent PostgreSQL connection management library
// Uses GLib GObject system for reactive properties and signals
// No GTK dependencies - can be used in TUI, CLI, or other projects

pub mod connection_gobject;
pub mod query_tab;
pub mod config_gobject;
pub mod manager_gobject;

pub mod config;

pub mod runtime_bridge;

mod internal_connection;

pub use config::{Config, DatabaseConfig};
pub use connection_gobject::{ConnectionState, DbConnection, QueryResult};
pub use query_tab::DbQueryTab;
pub use config_gobject::DbConfig;
pub use manager_gobject::DbManager;
pub use runtime_bridge::RuntimeBridge;
