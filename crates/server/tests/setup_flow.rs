//! First-owner setup, CSRF protection, domain admission, and concurrent initialization.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use axum::http::{Request, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use server::app::auth::dto::{TokenGrantType, TokenRequest, TokenResponse};
use server::app::installation::dto::{
    CreateSetupSessionRequest, CreateSetupSessionResponse, InstallationState,
    ReplaceSetupConfigurationRequest, SetupConfiguration, SetupOidcAuthorization,
    SetupOidcAuthorizationRequest, SetupStatus,
};
use server::app::organization::dto::AdminOrg;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use url::Url;

mod common;

const SETUP_CALLBACK: &str = "http://127.0.0.1:49152/callback";
const SETUP_STATE: &str = "native-setup-state";
const SETUP_VERIFIER: &str = "setup-verifier-abcdefghijklmnopqrstuvwxyz-0123456789";

#[tokio::test]
async fn setup_claim_creates_one_oidc_bound_installation_and_locks_it() {
    let postgres = common::migrated_postgres().await;
    let app = common::setup_router(
        postgres.pool.clone(),
        "owner@example.com",
        "setup-owner-subject",
    );

    let initial: SetupStatus = get_json(app.clone(), "/api/v1/setup", None).await;
    assert_eq!(initial.state, InstallationState::SetupRequired);
    assert!(initial.setup_code_configured);
    assert!(initial.oidc_configured);
    assert_eq!(initial.session, None);

    let product_login = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(product_login_uri())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(product_login.status(), StatusCode::CONFLICT);

    let invalid_code = post_json(
        app.clone(),
        "/api/v1/setup/sessions",
        &CreateSetupSessionRequest {
            setup_code: "invalid-setup-code".to_owned(),
        },
        None,
        None,
    )
    .await;
    assert_eq!(invalid_code.status(), StatusCode::UNAUTHORIZED);

    let (cookie, session) = create_setup_session(app.clone()).await;
    let with_session: SetupStatus = get_json(app.clone(), "/api/v1/setup", Some(&cookie)).await;
    assert!(with_session.session.is_some());

    let missing_csrf = put_json(
        app.clone(),
        "/api/v1/setup/configuration",
        &ReplaceSetupConfigurationRequest {
            org_name: "Clumsies Lab".to_owned(),
            default_project_name: "Default".to_owned(),
            allowed_email_domains: vec!["example.com".to_owned()],
        },
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let configuration: SetupConfiguration = decode_json(
        put_json(
            app.clone(),
            "/api/v1/setup/configuration",
            &ReplaceSetupConfigurationRequest {
                org_name: "  Clumsies Lab  ".to_owned(),
                default_project_name: "  Default  ".to_owned(),
                allowed_email_domains: vec!["@EXAMPLE.COM".to_owned()],
            },
            Some(&cookie),
            Some(&session.csrf_token),
        )
        .await,
    )
    .await;
    assert_eq!(configuration.org_name, "Clumsies Lab");
    assert_eq!(configuration.allowed_email_domains, ["example.com"]);

    let provider_state =
        begin_setup_oidc(app.clone(), &cookie, &session.csrf_token, SETUP_CALLBACK).await;
    let callback = complete_provider_login(app.clone(), &provider_state).await;
    assert_eq!(callback.status(), StatusCode::FOUND);
    assert!(callback.headers().get(SET_COOKIE).is_none());
    let client_callback =
        Url::parse(callback.headers().get(LOCATION).unwrap().to_str().unwrap()).unwrap();
    assert_eq!(client_callback.path(), "/callback");
    assert_eq!(query_value(&client_callback, "state"), SETUP_STATE);
    let authorization_code = query_value(&client_callback, "code");

    let invalid_grant =
        exchange_setup_token(app.clone(), &authorization_code, "wrong-verifier").await;
    assert_eq!(invalid_grant.status(), StatusCode::BAD_REQUEST);
    let token: TokenResponse =
        decode_json(exchange_setup_token(app.clone(), &authorization_code, SETUP_VERIFIER).await)
            .await;
    assert_eq!(token.user.email, "owner@example.com");
    assert_eq!(token.user.role, "owner");

    let admin_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/admin/org")
                .header("authorization", format!("Bearer {}", token.access_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_response.status(), StatusCode::OK);
    let admin_org: AdminOrg = decode_json(admin_response).await;
    assert_eq!(admin_org.name, "Clumsies Lab");

    let completed: SetupStatus = get_json(app.clone(), "/api/v1/setup", Some(&cookie)).await;
    assert_eq!(completed.state, InstallationState::Initialized);
    assert_eq!(completed.session, None);

    let locked = post_json(
        app.clone(),
        "/api/v1/setup/sessions",
        &CreateSetupSessionRequest {
            setup_code: common::TEST_SETUP_CODE.to_owned(),
        },
        None,
        None,
    )
    .await;
    assert_eq!(locked.status(), StatusCode::CONFLICT);

    let org = sqlx::query("SELECT org_id, name, allowed_email_domains FROM orgs")
        .fetch_one(&postgres.pool)
        .await
        .unwrap();
    let org_id: String = sqlx::Row::try_get(&org, "org_id").unwrap();
    assert_eq!(
        sqlx::Row::try_get::<String, _>(&org, "name").unwrap(),
        "Clumsies Lab"
    );
    assert_eq!(
        sqlx::Row::try_get::<Vec<String>, _>(&org, "allowed_email_domains").unwrap(),
        ["example.com"]
    );
    let owner_binding_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
         FROM users u
         JOIN external_identities i ON i.user_id = u.user_id
         WHERE u.role = 'owner' AND u.status = 'active'
           AND i.subject = 'setup-owner-subject'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(owner_binding_count, 1);
    let project_ref_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
         FROM projects p
         JOIN refs r ON r.project_id = p.project_id
         JOIN project_members m ON m.project_id = p.project_id
         WHERE p.org_id = $1 AND r.ref_name = 'refs/heads/main' AND m.role = 'owner'",
    )
    .bind(&org_id)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(project_ref_count, 1);
    let second_org = sqlx::query("INSERT INTO orgs (org_id, name) VALUES ('org_second', 'Second')")
        .execute(&postgres.pool)
        .await;
    assert!(second_org.is_err());

    assert_eq!(token.org.org_id, org_id);
    postgres.shutdown().await;
}

#[tokio::test]
async fn concurrent_setup_claims_allow_exactly_one_owner() {
    let postgres = common::migrated_postgres().await;
    let app = common::setup_router(
        postgres.pool.clone(),
        "owner@example.com",
        "concurrent-owner",
    );
    let (first_cookie, first_session) = configured_session(app.clone(), Vec::new()).await;
    let (second_cookie, second_session) = configured_session(app.clone(), Vec::new()).await;
    let first_state = begin_setup_oidc(
        app.clone(),
        &first_cookie,
        &first_session.csrf_token,
        SETUP_CALLBACK,
    )
    .await;
    let second_state = begin_setup_oidc(
        app.clone(),
        &second_cookie,
        &second_session.csrf_token,
        SETUP_CALLBACK,
    )
    .await;

    let first = complete_provider_login(app.clone(), &first_state);
    let second = complete_provider_login(app.clone(), &second_state);
    let (first, second) = tokio::join!(first, second);
    let mut statuses = [first.status(), second.status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::FOUND, StatusCode::CONFLICT]);

    let org_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM orgs")
        .fetch_one(&postgres.pool)
        .await
        .unwrap();
    let owner_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE role = 'owner'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(org_count, 1);
    assert_eq!(owner_count, 1);
    postgres.shutdown().await;
}

#[tokio::test]
async fn disallowed_setup_owner_can_correct_configuration_and_retry() {
    let postgres = common::migrated_postgres().await;
    let app = common::setup_router(
        postgres.pool.clone(),
        "owner@example.com",
        "recoverable-owner",
    );
    let (cookie, session) = configured_session(app.clone(), vec!["example.org".to_owned()]).await;
    let denied_state =
        begin_setup_oidc(app.clone(), &cookie, &session.csrf_token, SETUP_CALLBACK).await;
    let denied = complete_provider_login(app.clone(), &denied_state).await;
    assert_eq!(denied.status(), StatusCode::FOUND);
    let denied_callback =
        Url::parse(denied.headers().get(LOCATION).unwrap().to_str().unwrap()).unwrap();
    assert_eq!(
        query_value(&denied_callback, "error"),
        "setup_owner_domain_not_allowed"
    );
    assert_eq!(query_value(&denied_callback, "state"), SETUP_STATE);

    let status: SetupStatus = get_json(app.clone(), "/api/v1/setup", Some(&cookie)).await;
    assert_eq!(status.state, InstallationState::SetupRequired);
    let corrected = put_json(
        app.clone(),
        "/api/v1/setup/configuration",
        &ReplaceSetupConfigurationRequest {
            org_name: "Clumsies Lab".to_owned(),
            default_project_name: "Default".to_owned(),
            allowed_email_domains: vec!["example.com".to_owned()],
        },
        Some(&cookie),
        Some(&session.csrf_token),
    )
    .await;
    assert_eq!(corrected.status(), StatusCode::OK);

    let retry_state =
        begin_setup_oidc(app.clone(), &cookie, &session.csrf_token, SETUP_CALLBACK).await;
    let completed = complete_provider_login(app, &retry_state).await;
    assert_eq!(completed.status(), StatusCode::FOUND);
    postgres.shutdown().await;
}

async fn configured_session(
    app: Router,
    allowed_email_domains: Vec<String>,
) -> (String, CreateSetupSessionResponse) {
    let (cookie, session) = create_setup_session(app.clone()).await;
    let response = put_json(
        app,
        "/api/v1/setup/configuration",
        &ReplaceSetupConfigurationRequest {
            org_name: "Clumsies Lab".to_owned(),
            default_project_name: "Default".to_owned(),
            allowed_email_domains,
        },
        Some(&cookie),
        Some(&session.csrf_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    (cookie, session)
}

async fn create_setup_session(app: Router) -> (String, CreateSetupSessionResponse) {
    let response = post_json(
        app,
        "/api/v1/setup/sessions",
        &CreateSetupSessionRequest {
            setup_code: common::TEST_SETUP_CODE.to_owned(),
        },
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let cookie = response
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    (cookie, decode_json(response).await)
}

async fn begin_setup_oidc(
    app: Router,
    cookie: &str,
    csrf_token: &str,
    redirect_uri: &str,
) -> String {
    let response = post_json(
        app,
        "/api/v1/setup/oidc-authorizations",
        &SetupOidcAuthorizationRequest {
            redirect_uri: redirect_uri.to_owned(),
            state: SETUP_STATE.to_owned(),
            code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(SETUP_VERIFIER.as_bytes())),
            code_challenge_method: "S256".to_owned(),
        },
        Some(cookie),
        Some(csrf_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let authorization: SetupOidcAuthorization = decode_json(response).await;
    Url::parse(&authorization.authorization_url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .unwrap()
}

async fn exchange_setup_token(app: Router, code: &str, verifier: &str) -> axum::response::Response {
    post_json(
        app,
        "/api/v1/auth/token",
        &TokenRequest {
            grant_type: TokenGrantType::AuthorizationCode,
            code: Some(code.to_owned()),
            redirect_uri: Some(SETUP_CALLBACK.to_owned()),
            code_verifier: Some(verifier.to_owned()),
            refresh_token: None,
        },
        None,
        None,
    )
    .await
}

async fn complete_provider_login(app: Router, provider_state: &str) -> axum::response::Response {
    let mut callback = Url::parse("http://server.test/login/oauth2/code/oidc").unwrap();
    callback
        .query_pairs_mut()
        .append_pair("code", "oidc-code")
        .append_pair("state", provider_state);
    app.oneshot(
        Request::builder()
            .uri(format!("{}?{}", callback.path(), callback.query().unwrap()))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn get_json<T>(app: Router, uri: &str, cookie: Option<&str>) -> T
where
    T: serde::de::DeserializeOwned,
{
    let mut request = Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        request = request.header(COOKIE, cookie);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    decode_json(response).await
}

async fn post_json<T: Serialize>(
    app: Router,
    uri: &str,
    body: &T,
    cookie: Option<&str>,
    csrf_token: Option<&str>,
) -> axum::response::Response {
    request_json(app, "POST", uri, body, cookie, csrf_token).await
}

async fn put_json<T: Serialize>(
    app: Router,
    uri: &str,
    body: &T,
    cookie: Option<&str>,
    csrf_token: Option<&str>,
) -> axum::response::Response {
    request_json(app, "PUT", uri, body, cookie, csrf_token).await
}

async fn request_json<T: Serialize>(
    app: Router,
    method: &str,
    uri: &str,
    body: &T,
    cookie: Option<&str>,
    csrf_token: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header(COOKIE, cookie);
    }
    if let Some(csrf_token) = csrf_token {
        request = request.header("x-csrf-token", csrf_token);
    }
    app.oneshot(
        request
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn decode_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn product_login_uri() -> String {
    let mut url = Url::parse("http://server.test/oauth2/authorization/oidc").unwrap();
    url.query_pairs_mut()
        .append_pair("client_kind", "desktop")
        .append_pair("redirect_uri", "http://127.0.0.1:49152/callback")
        .append_pair("code_challenge", &"a".repeat(43))
        .append_pair("code_challenge_method", "S256");
    format!("{}?{}", url.path(), url.query().unwrap())
}

fn query_value(url: &Url, name: &str) -> String {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap()
}
