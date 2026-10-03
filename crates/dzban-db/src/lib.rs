// dzban-db: UI-independent database connection management library
// Uses GLib GObject system for reactive properties and signals
// No GTK dependencies - can be used in TUI, CLI, or other projects

pub mod app_state;
pub mod config_gobject;
pub mod connection_gobject;
pub mod manager_gobject;
pub mod query_tab;
pub mod result;

pub mod config;

pub mod runtime_bridge;

mod internal_connection;
mod postgres_value;

pub use app_state::{
    ActionHint, AppCommand, AppEvent, AppState, InputKey, KeyboardState, PlanPreviewState,
    PlanVisibility, PrimaryAction, QueryExecutionState,
};
pub use config::{Config, DatabaseBackend, DatabaseConfig};
pub use config_gobject::DbConfig;
pub use connection_gobject::{ConnectionOperation, ConnectionState, DbConnection};
pub use manager_gobject::DbManager;
pub use query_tab::DbQueryTab;
pub use result::{QueryEstimate, QueryPlan, QueryResult, ResultCell, ResultColumn, ResultSet};
pub use runtime_bridge::RuntimeBridge;
