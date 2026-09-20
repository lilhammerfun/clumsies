//! OIDC login, session credentials, and the current-user profile.

pub mod dto;
mod error;
mod handler;
pub(crate) mod model;
mod oidc;
mod repository;
pub(crate) mod routes;
mod service;
pub use error::AuthError;
pub(crate) use model::user_capabilities;
pub use model::{AuthPrincipal, OidcIdentity, ProviderSummary};
pub use oidc::{DiscoveredOidcProvider, OidcIdentityProvider};
pub use service::AuthService;

pub use service::get_me;
