//! Clumsies HTTP backend: resource operations, application construction, and startup.

pub mod app;
mod bootstrap;
pub mod config;
pub mod dto;
pub mod error;
mod http;
mod identity;
pub mod infra;
pub mod maintenance;
mod metrics;
mod middleware;
pub mod pagination;
mod routes;
mod state;
mod telemetry;

pub use app::build_app;
pub use bootstrap::{run, run_project_authority_migration};
