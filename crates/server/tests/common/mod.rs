//! PostgreSQL fixtures and production application construction for API scenarios.
#![allow(dead_code)]

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::header::{AUTHORIZATION, LOCATION};
use axum::http::{HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::{Router, middleware};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openidconnect::PkceCodeChallenge;
use server::app::auth::dto::{TokenRequest, TokenResponse};
use server::app::auth::{AuthError, AuthService, OidcIdentity, OidcIdentityProvider};
use server::app::installation::dto::ReplaceSetupConfigurationRequest;
use server::app::installation::{InitializedInstallation, InstallationService};
use server::build_app;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::{Arc, OnceLock};
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower::ServiceExt;
use url::Url;

pub const TEST_SETUP_CODE: &str = "clumsies-test-setup-code-00000001";
const TEST_ISSUER: &str = "https://identity.example.test";

pub struct TestPostgres {
    _permit: OwnedSemaphorePermit,
    _container: ContainerAsync<Postgres>,
    pub pool: PgPool,
}

fn postgres_slots() -> &'static Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4)))
}

pub async fn postgres_without_migrations() -> TestPostgres {
    let permit = postgres_slots().clone().acquire_owned().await.unwrap();
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let host = container.get_host().await.unwrap();
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let pool = PgPool::connect(&url).await.unwrap();

    TestPostgres {
        _permit: permit,
        _container: container,
        pool,
    }
}

pub async fn migrated_postgres() -> TestPostgres {
    let postgres = postgres_without_migrations().await;
    server::infra::database::run_migrations(&postgres.pool)
        .await
        .unwrap();
    postgres
}

pub async fn authenticated_router(pool: PgPool) -> (Router, TokenResponse) {
    authenticated_router_as(pool, "owner@example.com", "oidc-subject-owner", "Owner").await
}

pub fn setup_router(pool: PgPool, owner_email: &str, owner_subject: &str) -> Router {
    let auth = AuthService::with_provider(
        pool.clone(),
        Arc::new(FakeOidcProvider {
            email: owner_email.to_owned(),
            subject: owner_subject.to_owned(),
            display_name: "Owner".to_owned(),
        }),
        vec![Url::parse("http://127.0.0.1/callback").unwrap()],
    );
    let installation =
        InstallationService::new(pool.clone(), Some(TEST_SETUP_CODE), false).unwrap();
    build_app(pool, auth, installation)
}

pub async fn initialize_installation(
    pool: PgPool,
    org_name: &str,
    owner_email: &str,
    owner_display_name: &str,
    owner_subject: &str,
    project_name: &str,
) -> InitializedInstallation {
    let installation =
        InstallationService::new(pool.clone(), Some(TEST_SETUP_CODE), false).unwrap();
    let credentials = installation.create_session(TEST_SETUP_CODE).await.unwrap();
    installation
        .replace_configuration(
            &credentials.token,
            &credentials.session.csrf_token,
            ReplaceSetupConfigurationRequest {
                org_name: org_name.to_owned(),
                default_project_name: project_name.to_owned(),
                allowed_email_domains: Vec::new(),
            },
        )
        .await
        .unwrap();
    let session_id = installation
        .authorize_oidc(&credentials.token, &credentials.session.csrf_token)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let initialized = installation
        .initialize_with_oidc(
            &mut tx,
            &session_id,
            &OidcIdentity {
                issuer: TEST_ISSUER.to_owned(),
                subject: owner_subject.to_owned(),
                email: owner_email.to_owned(),
                email_verified: true,
                display_name: Some(owner_display_name.to_owned()),
                avatar_url: Some("https://images.example.test/avatar.png".to_owned()),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    initialized
}

pub async fn authenticated_router_as(
    pool: PgPool,
    email: &str,
    subject: &str,
    display_name: &str,
) -> (Router, TokenResponse) {
    let auth = AuthService::with_provider(
        pool.clone(),
        Arc::new(FakeOidcProvider {
            email: email.to_owned(),
            subject: subject.to_owned(),
            display_name: display_name.to_owned(),
        }),
        vec![Url::parse("http://127.0.0.1/callback").unwrap()],
    );
    let installation = InstallationService::new(pool.clone(), None, true).unwrap();
    let app = build_app(pool, auth, installation);
    let verifier = "test-verifier-abcdefghijklmnopqrstuvwxyz-0123456789";
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let mut start_url = Url::parse("http://server.test/oauth2/authorization/oidc").unwrap();
    start_url
        .query_pairs_mut()
        .append_pair("client_kind", "desktop")
        .append_pair("redirect_uri", "http://127.0.0.1:49152/callback")
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", "desktop-state");
    let start_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri_path(&start_url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(start_response.status(), StatusCode::FOUND);
    let provider_url = Url::parse(
        start_response
            .headers()
            .get(LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let provider_state = query_value(&provider_url, "state");

    let mut callback_url = Url::parse("http://server.test/login/oauth2/code/oidc").unwrap();
    callback_url
        .query_pairs_mut()
        .append_pair("code", "oidc-code")
        .append_pair("state", &provider_state);
    let callback_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri_path(&callback_url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    if callback_response.status() != StatusCode::FOUND {
        let status = callback_response.status();
        let body = to_bytes(callback_response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        panic!(
            "OIDC callback returned {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let client_url = Url::parse(
        callback_response
            .headers()
            .get(LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(query_value(&client_url, "state"), "desktop-state");
    let code = query_value(&client_url, "code");
    let token_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&TokenRequest {
                        grant_type: server::app::auth::dto::TokenGrantType::AuthorizationCode,
                        code: Some(code),
                        redirect_uri: Some("http://127.0.0.1:49152/callback".to_owned()),
                        code_verifier: Some(verifier.to_owned()),
                        refresh_token: None,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(token_response.status(), StatusCode::OK);
    let token: TokenResponse = serde_json::from_slice(
        &to_bytes(token_response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let bearer = HeaderValue::from_str(&format!("Bearer {}", token.access_token)).unwrap();
    let authenticated = app.layer(middleware::from_fn(
        move |mut request: Request<Body>, next: Next| {
            let bearer = bearer.clone();
            async move {
                request.headers_mut().insert(AUTHORIZATION, bearer);
                next.run(request).await
            }
        },
    ));
    (authenticated, token)
}

struct FakeOidcProvider {
    email: String,
    subject: String,
    display_name: String,
}

#[async_trait]
impl OidcIdentityProvider for FakeOidcProvider {
    fn authorization_url(
        &self,
        provider_state: &str,
        nonce: &str,
        _provider_pkce_challenge: PkceCodeChallenge,
        _login_hint: Option<&str>,
    ) -> Result<String, AuthError> {
        let mut url = Url::parse("https://accounts.example.test/authorize").unwrap();
        url.query_pairs_mut()
            .append_pair("state", provider_state)
            .append_pair("nonce", nonce);
        Ok(url.to_string())
    }

    async fn exchange_code(
        &self,
        code: &str,
        _nonce: &str,
        _provider_pkce_verifier: &str,
    ) -> Result<OidcIdentity, AuthError> {
        if code != "oidc-code" {
            return Err(AuthError::ProviderInvalid(
                "unexpected mock authorization code".to_owned(),
            ));
        }
        Ok(OidcIdentity {
            issuer: TEST_ISSUER.to_owned(),
            subject: self.subject.clone(),
            email: self.email.clone(),
            email_verified: true,
            display_name: Some(self.display_name.clone()),
            avatar_url: Some("https://images.example.test/avatar.png".to_owned()),
        })
    }
}

fn query_value(url: &Url, name: &str) -> String {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| panic!("missing {name} query parameter in {url}"))
}

fn uri_path(url: &Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    }
}

/// Build the production router with authentication left unconfigured.
pub fn router(pool: PgPool) -> Router {
    let auth = AuthService::unconfigured(pool.clone());
    let installation = InstallationService::new(pool.clone(), None, true).unwrap();
    build_app(pool, auth, installation)
}

impl TestPostgres {
    /// Close database connections before removing this scenario's container.
    pub async fn shutdown(self) {
        self.pool.close().await;
        self._container
            .rm()
            .await
            .expect("remove PostgreSQL test container");
    }
}
