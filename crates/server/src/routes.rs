//! Route composition and authorization groups.

use crate::app::auth::AuthService;
use crate::app::installation::InstallationService;
use crate::middleware::{require_admin_auth, require_auth, security_headers};
use crate::state::AppState;
use axum::{Router, middleware};
use sqlx::PgPool;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct HttpOperation {
    /// HTTP method bound to the registered route.
    pub(crate) method: &'static str,
    /// Registered HTTP path pattern, including named resource parameters.
    pub(crate) path: &'static str,
}

/// Register resource routes and derive test-only operation metadata from the same declarations.
macro_rules! define_routes {
    ($function:ident, $operations:ident, {
        $(
            $path:literal => {
                $first_method:ident: $first_handler:path
                $(, $method:ident: $handler:path)*
                $(,)?
            };
        )*
    }) => {
        #[cfg(test)]
        pub(crate) const $operations: &[$crate::routes::HttpOperation] = &[
            $(
                $crate::routes::HttpOperation {
                    method: stringify!($first_method),
                    path: $path,
                },
                $(
                    $crate::routes::HttpOperation {
                        method: stringify!($method),
                        path: $path,
                    },
                )*
            )*
        ];

        pub(crate) fn $function() -> Router<AppState> {
            Router::new()
                $(.route($path, $first_method($first_handler)$(.$method($handler))*))*
        }
    };
}

pub(crate) use define_routes;
/// Combine resource routes while preserving their authentication boundaries.
pub(crate) fn router_with_services(
    pool: PgPool,
    auth: AuthService,
    installation: InstallationService,
) -> Router {
    let state = AppState {
        auth,
        installation,
        pool,
        version: env!("CARGO_PKG_VERSION"),
    };
    let public_routes = Router::new()
        .merge(crate::app::health::routes::public_routes())
        .merge(crate::app::installation::routes::public_routes())
        .merge(crate::app::auth::routes::public_routes());
    let admin_routes = Router::new()
        .merge(crate::app::auth::routes::admin_routes())
        .merge(crate::app::organization::routes::admin_routes())
        .merge(crate::app::project::routes::admin_routes())
        .merge(crate::app::token::routes::admin_routes())
        .merge(crate::app::audit_event::routes::admin_routes())
        .merge(crate::app::memory::routes::admin_routes())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_auth,
        ));
    let protected_routes = Router::new()
        .merge(crate::app::auth::routes::protected_routes())
        .merge(crate::app::inbox::routes::protected_routes())
        .merge(crate::app::project::routes::protected_routes())
        .merge(crate::app::memory::routes::protected_routes())
        .merge(crate::app::bundle::routes::protected_routes())
        .merge(crate::app::draft::routes::protected_routes())
        .merge(crate::app::review::routes::protected_routes())
        .merge(crate::app::commit::routes::protected_routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    Router::new()
        .merge(public_routes)
        .merge(admin_routes)
        .merge(protected_routes)
        .with_state(state)
        .layer(middleware::from_fn(security_headers))
}

#[cfg(test)]
fn all_operations() -> Vec<HttpOperation> {
    let mut operations = Vec::new();
    operations.extend_from_slice(crate::app::health::routes::PUBLIC_OPERATIONS);
    operations.extend_from_slice(crate::app::installation::routes::PUBLIC_OPERATIONS);
    operations.extend_from_slice(crate::app::auth::routes::PUBLIC_OPERATIONS);
    operations.extend_from_slice(crate::app::auth::routes::ADMIN_OPERATIONS);
    operations.extend_from_slice(crate::app::auth::routes::PROTECTED_OPERATIONS);
    operations.extend_from_slice(crate::app::inbox::routes::PROTECTED_OPERATIONS);
    operations.extend_from_slice(crate::app::organization::routes::ADMIN_OPERATIONS);
    operations.extend_from_slice(crate::app::project::routes::ADMIN_OPERATIONS);
    operations.extend_from_slice(crate::app::project::routes::PROTECTED_OPERATIONS);
    operations.extend_from_slice(crate::app::token::routes::ADMIN_OPERATIONS);
    operations.extend_from_slice(crate::app::audit_event::routes::ADMIN_OPERATIONS);
    operations.extend_from_slice(crate::app::memory::routes::ADMIN_OPERATIONS);
    operations.extend_from_slice(crate::app::memory::routes::PROTECTED_OPERATIONS);
    operations.extend_from_slice(crate::app::bundle::routes::PROTECTED_OPERATIONS);
    operations.extend_from_slice(crate::app::draft::routes::PROTECTED_OPERATIONS);
    operations.extend_from_slice(crate::app::review::routes::PROTECTED_OPERATIONS);
    operations.extend_from_slice(crate::app::commit::routes::PROTECTED_OPERATIONS);
    operations
}
#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde::Deserialize;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;
    use crate::app::health::{AdminHealth, HealthStatus};

    #[derive(Debug, Deserialize)]
    struct OpenApiDocument {
        paths: BTreeMap<String, BTreeMap<String, serde_yaml_ng::Value>>,
    }

    #[test]
    fn axum_routes_match_public_and_admin_openapi() {
        let public = include_str!("../openapi/clumsies.public.v1.yaml");
        let admin = include_str!("../openapi/clumsies.admin.v1.yaml");
        let contract_operations = openapi_operations(public)
            .into_iter()
            .chain(openapi_operations(admin))
            .collect::<BTreeSet<_>>();
        let server_operations = all_operations()
            .into_iter()
            .map(|operation| (operation.method.to_owned(), operation.path.to_owned()))
            .collect::<BTreeSet<_>>();

        assert_eq!(server_operations, contract_operations);
    }

    #[tokio::test]
    async fn admin_health_matches_contract_shape_when_database_is_down() {
        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(100))
            .connect_lazy("postgres://clumsies:clumsies@127.0.0.1:1/clumsies")
            .unwrap();
        let auth = AuthService::unconfigured(pool.clone());
        let installation = InstallationService::new(pool.clone(), None, true).unwrap();
        let app = crate::build_app(pool, auth, installation);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let health: AdminHealth = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, HealthStatus::Down);
        assert_eq!(health.database.status, HealthStatus::Down);
        assert_eq!(health.schema.status, HealthStatus::Down);
        assert_eq!(health.commit_service.status, HealthStatus::Down);
        assert_eq!(health.oidc.status, HealthStatus::Down);
    }

    fn openapi_operations(source: &str) -> BTreeSet<(String, String)> {
        const HTTP_METHODS: [&str; 8] = [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ];
        let document: OpenApiDocument = serde_yaml_ng::from_str(source).unwrap();
        document
            .paths
            .into_iter()
            .flat_map(|(path, item)| {
                item.into_keys()
                    .filter(|method| HTTP_METHODS.contains(&method.as_str()))
                    .map(move |method| (method, path.clone()))
            })
            .collect()
    }
}
