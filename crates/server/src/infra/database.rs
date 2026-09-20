//! PostgreSQL connections and embedded schema migrations.

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use time::Duration;

/// Ordered schema migrations embedded in this server build.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Explicit PostgreSQL connection settings consumed during application startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseConfig {
    /// Validated address of the dependency or public origin.
    pub url: String,
    /// Upper bound on simultaneously open database connections.
    pub max_connections: u32,
    /// Maximum seconds to wait for a pooled database connection.
    pub acquire_timeout_seconds: u64,
}

impl DatabaseConfig {
    /// Build explicit database connection settings using the server's pool defaults.
    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            max_connections: 10,
            acquire_timeout_seconds: 5,
        }
    }
}

/// Open the configured PostgreSQL pool with bounded connection acquisition.
///
/// # Errors
/// Returns an error when the configured PostgreSQL pool cannot be established within its
/// connection limits.
pub async fn connect(config: &DatabaseConfig) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(std::time::Duration::from_secs(
            config.acquire_timeout_seconds,
        ))
        .connect(&config.url)
        .await
}

/// Apply the bundled schema migrations using the configured database connection.
///
/// # Errors
/// Propagates migration, database, or migration-timeout failures; partial statements are not
/// treated as successful migrations.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

/// Return the latest migration expected by this server build.
pub fn current_schema_migration() -> i64 {
    MIGRATOR
        .iter()
        .next_back()
        .expect("Server must embed at least one migration")
        .version
}

/// Choose the PostgreSQL statement timeout used while applying migrations.
pub fn migration_statement_timeout() -> Duration {
    Duration::seconds(30)
}
