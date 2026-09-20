//! Dependency health and readiness reporting.

use crate::infra::database::current_schema_migration;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// Report dependency and capability readiness without requiring a usable database.
pub(crate) async fn admin_health(State(state): State<AppState>) -> Json<AdminHealth> {
    let database = check_database(&state.pool).await;
    let schema = if database.status == HealthStatus::Ok {
        check_schema(&state.pool).await
    } else {
        dependency_down("schema", "database")
    };
    let commit_service = if schema.status == HealthStatus::Ok {
        implemented_component("commit service")
    } else {
        dependency_down("commit service", "schema")
    };
    let oidc = if state.auth.configured() {
        implemented_component("OIDC")
    } else {
        HealthCheck {
            status: HealthStatus::Down,
            message: "OIDC is not configured".to_owned(),
        }
    };
    let status = overall_status([
        database.status,
        schema.status,
        commit_service.status,
        oidc.status,
    ]);

    Json(AdminHealth {
        status,
        version: state.version.to_owned(),
        database,
        schema,
        commit_service,
        oidc,
    })
}

/// Probe database availability and preserve a diagnostic when the dependency is down.
async fn check_database(pool: &PgPool) -> HealthCheck {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await
    {
        Ok(1) => HealthCheck {
            status: HealthStatus::Ok,
            message: "postgres reachable".to_owned(),
        },
        Ok(_) => HealthCheck {
            status: HealthStatus::Down,
            message: "postgres returned an unexpected health value".to_owned(),
        },
        Err(error) => HealthCheck {
            status: HealthStatus::Down,
            message: error.to_string(),
        },
    }
}

/// Check that the database has applied the latest migration expected by this build.
async fn check_schema(pool: &PgPool) -> HealthCheck {
    let current_schema_migration = current_schema_migration();
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM _sqlx_migrations
            WHERE version = $1 AND success = true
        )",
    )
    .bind(current_schema_migration)
    .fetch_one(pool)
    .await
    {
        Ok(true) => HealthCheck {
            status: HealthStatus::Ok,
            message: format!("migration {current_schema_migration} applied"),
        },
        Ok(false) => HealthCheck {
            status: HealthStatus::Down,
            message: format!("migration {current_schema_migration} is not applied"),
        },
        Err(error) => HealthCheck {
            status: HealthStatus::Down,
            message: error.to_string(),
        },
    }
}

/// Describe an implemented capability whose prerequisites are ready.
fn implemented_component(name: &str) -> HealthCheck {
    HealthCheck {
        status: HealthStatus::Ok,
        message: format!("{name} ready"),
    }
}

/// Report a skipped readiness check with the failed prerequisite identified.
fn dependency_down(name: &str, dependency: &str) -> HealthCheck {
    HealthCheck {
        status: HealthStatus::Down,
        message: format!("{name} check skipped because {dependency} is down"),
    }
}

/// Aggregate component readiness, preferring down over degraded over healthy.
fn overall_status(statuses: impl IntoIterator<Item = HealthStatus>) -> HealthStatus {
    let mut has_degraded = false;
    for status in statuses {
        match status {
            HealthStatus::Ok => {}
            HealthStatus::Degraded => has_degraded = true,
            HealthStatus::Down => return HealthStatus::Down,
        }
    }
    if has_degraded {
        HealthStatus::Degraded
    } else {
        HealthStatus::Ok
    }
}

/// Readiness report separating database, schema, and implemented capabilities.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminHealth {
    /// Aggregate readiness of the server dependencies and implemented capabilities.
    pub status: HealthStatus,
    /// Server package version reported by this build.
    pub version: String,
    /// Result of checking database connectivity.
    pub database: HealthCheck,
    /// Result of comparing the installed database schema with expected migrations.
    pub schema: HealthCheck,
    /// Health status of the snapshot publication capability.
    pub commit_service: HealthCheck,
    /// Readiness of the configured identity-provider capability.
    pub oidc: HealthCheck,
}

/// Public readiness category of a dependency or server capability.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// The dependency or capability is ready.
    Ok,
    /// The capability is available with a readiness limitation.
    Degraded,
    /// A required dependency or capability is unavailable.
    Down,
}

/// Readiness result and public diagnostic for one server dependency or capability.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthCheck {
    /// Readiness of this dependency or capability.
    pub status: HealthStatus,
    /// Human-readable diagnostic safe for the enclosing output boundary.
    pub message: String,
}

pub(crate) mod routes;
