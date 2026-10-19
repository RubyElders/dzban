pub mod action;
pub mod adapter;
pub mod application;
pub mod attention;
pub mod catalog;
pub mod commands;
pub mod config;
pub mod document_execution;
pub mod documents;
pub mod error;
pub mod execution;
pub mod health;
pub mod history;
pub mod interaction;
pub mod outputs;
mod plan_text;
pub mod presentation;
pub mod preview;
pub mod query_selection;
pub mod result;
pub mod result_store;
pub mod result_stream;
pub mod sql;
pub mod tabs;
pub mod workspace;

pub use config::{Config, ConnectionProfileId, DatabaseBackend, DatabaseConfig, ProfileError};
pub use result::{
    CommandAcknowledgment, ExecutionOutcome, QueryEstimate, QueryPlan, QueryResult, ResultCell,
    ResultColumn, ResultSet,
};
