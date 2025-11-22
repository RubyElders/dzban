// dzban-db: UI-independent PostgreSQL connection management library
// Uses GLib GObject system for reactive properties and signals
// No GTK dependencies - can be used in TUI, CLI, or other projects

pub mod config;
pub mod connection;
pub mod manager;
pub mod reactive_manager;
pub mod runtime_bridge;

pub use config::{Config, DatabaseConfig};
pub use connection::{DbConnection, ConnectionState};
pub use manager::ConnectionManager;
pub use reactive_manager::{ReactiveConnection, ReactiveConnectionManager};
pub use runtime_bridge::RuntimeBridge;
