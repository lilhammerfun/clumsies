//! Shared application dependencies.

use crate::app::auth::AuthService;
use crate::app::installation::InstallationService;
use sqlx::PgPool;

/// Long-lived dependencies cloned into the application's HTTP handlers.
#[derive(Clone)]
pub(crate) struct AppState {
    /// Shared PostgreSQL connection pool; callers retain responsibility for shutdown.
    pub(crate) pool: PgPool,
    /// Shared authentication service and its configured identity provider.
    pub(crate) auth: AuthService,
    /// Shared first-run setup service and installation state access.
    pub(crate) installation: InstallationService,
    /// Server package version exposed by health reporting.
    pub(crate) version: &'static str,
}
