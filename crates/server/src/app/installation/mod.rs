//! Initial organization setup and bootstrap authentication.

pub mod dto;
mod error;
mod handler;
pub(crate) mod model;
mod repository;
pub(crate) mod routes;
pub mod service;
pub use error::InstallationError;
pub use model::{InitializedInstallation, SetupSessionCredentials};
pub use service::InstallationService;
