//! Dependency health and readiness reporting.

use crate::infra::database::current_schema_migration;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

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

fn implemented_component(name: &str) -> HealthCheck {
    HealthCheck {
        status: HealthStatus::Ok,
        message: format!("{name} ready"),
    }
}

fn dependency_down(name: &str, dependency: &str) -> HealthCheck {
    HealthCheck {
        status: HealthStatus::Down,
        message: format!("{name} check skipped because {dependency} is down"),
    }
}

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminHealth {
    pub status: HealthStatus,
    pub version: String,
    pub database: HealthCheck,
    pub schema: HealthCheck,
    pub commit_service: HealthCheck,
    pub oidc: HealthCheck,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
    Degraded,
    Down,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthCheck {
    pub status: HealthStatus,
    pub message: String,
}

pub(crate) mod routes;
