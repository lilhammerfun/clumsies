//! PostgreSQL connections and embedded schema migrations.

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use time::Duration;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub acquire_timeout_seconds: u64,
}

impl DatabaseConfig {
    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            max_connections: 10,
            acquire_timeout_seconds: 5,
        }
    }
}

pub async fn connect(config: &DatabaseConfig) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(std::time::Duration::from_secs(
            config.acquire_timeout_seconds,
        ))
        .connect(&config.url)
        .await
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

pub fn current_schema_migration() -> i64 {
    MIGRATOR
        .iter()
        .next_back()
        .expect("Server must embed at least one migration")
        .version
}

pub fn migration_statement_timeout() -> Duration {
    Duration::seconds(30)
}
