//! Shared application dependencies.

use crate::app::auth::AuthService;
use crate::app::installation::InstallationService;
use sqlx::PgPool;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) pool: PgPool,
    pub(crate) auth: AuthService,
    pub(crate) installation: InstallationService,
    pub(crate) version: &'static str,
}
