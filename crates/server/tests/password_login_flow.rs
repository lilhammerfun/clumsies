//! Invitation-only local accounts, password recovery, and shared session authorization.

mod common;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn invitation_password_reset_and_disable_share_one_account() {
    let postgres = common::migrated_postgres().await;
    common::initialize_installation(
        postgres.pool.clone(),
        "Test",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Default",
    )
    .await;
    let (_, owner) = common::authenticated_router(postgres.pool.clone()).await;
    let app = common::setup_router(
        postgres.pool.clone(),
        "owner@example.com",
        "oidc-subject-owner",
    );
    let (status, invitation) = request(
        &app,
        "POST",
        "/api/v1/admin/invitations",
        Some(&owner.access_token),
        json!({"role":"member"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{invitation}");
    let token = invitation["token"].as_str().unwrap();
    let input =
        json!({"token": token, "username":" Alice ", "password":"long enough local password"});
    let (status, session) = request(
        &app,
        "POST",
        "/api/v1/auth/invitations/accept",
        None,
        input.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(session["user"]["username"], "alice");
    assert!(session["user"]["email"].is_null());
    assert_eq!(session["user"]["user_id"], invitation["user_id"]);
    assert_eq!(
        request(&app, "POST", "/api/v1/auth/invitations/accept", None, input)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, login) = request(
        &app,
        "POST",
        "/api/v1/auth/password/sessions",
        None,
        json!({"username":"ALICE", "password":"long enough local password"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let old_token = login["access_token"].as_str().unwrap();
    assert_eq!(
        request(&app, "GET", "/api/v1/me", Some(old_token), Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/admin/invitations",
            Some(old_token),
            json!({"role":"owner"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let user_id = invitation["user_id"].as_str().unwrap();
    let (status, reset) = request(
        &app,
        "POST",
        &format!("/api/v1/admin/members/{user_id}/password-reset"),
        Some(&owner.access_token),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    let reset_body = json!({"token":reset["token"],"password":"a new and long password"});
    assert_eq!(request(&app,"POST","/api/v1/auth/invitations/accept",None,json!({"token":reset["token"],"username":"intruder","password":"a new and long password"})).await.0,StatusCode::BAD_REQUEST);
    let (status, recovered) = request(
        &app,
        "POST",
        "/api/v1/auth/password/reset",
        None,
        reset_body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{recovered}");
    assert_eq!(
        request(&app, "GET", "/api/v1/me", Some(old_token), Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/token",
            None,
            json!({"grant_type":"refresh_token","refresh_token":login["refresh_token"]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/password/reset",
            None,
            reset_body
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/password/sessions",
            None,
            json!({"username":"alice","password":"long enough local password"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let stored: String =
        sqlx::query_scalar("SELECT password_hash FROM password_credentials WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert!(stored.starts_with("$argon2id$"));
    assert!(!stored.contains("a new and long password"));
    sqlx::query("UPDATE users SET status='disabled' WHERE user_id=$1")
        .bind(user_id)
        .execute(&postgres.pool)
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/password/sessions",
            None,
            json!({"username":"alice","password":"a new and long password"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/me",
            recovered["access_token"].as_str(),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn invitations_expire_revoke_and_have_single_concurrent_consumer() {
    let postgres = common::migrated_postgres().await;
    common::initialize_installation(
        postgres.pool.clone(),
        "Test",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Default",
    )
    .await;
    let (_, owner) = common::authenticated_router(postgres.pool.clone()).await;
    let app = common::setup_router(
        postgres.pool.clone(),
        "owner@example.com",
        "oidc-subject-owner",
    );
    let (_, invitation) = request(
        &app,
        "POST",
        "/api/v1/admin/invitations",
        Some(&owner.access_token),
        json!({"role":"member"}),
    )
    .await;
    let user_id = invitation["user_id"].as_str().unwrap();
    let (_, replacement) = request(
        &app,
        "POST",
        &format!("/api/v1/admin/members/{user_id}/invitation"),
        Some(&owner.access_token),
        json!({}),
    )
    .await;
    let payload = |token: &Value| json!({"token":token,"username":"member","password":"a sufficiently long password"});
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/invitations/accept",
            None,
            payload(&invitation["token"])
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (first, second) = tokio::join!(
        request(
            &app,
            "POST",
            "/api/v1/auth/invitations/accept",
            None,
            payload(&replacement["token"])
        ),
        request(
            &app,
            "POST",
            "/api/v1/auth/invitations/accept",
            None,
            payload(&replacement["token"])
        )
    );
    assert!(
        (first.0 == StatusCode::OK && second.0 == StatusCode::BAD_REQUEST)
            || (second.0 == StatusCode::OK && first.0 == StatusCode::BAD_REQUEST),
        "{first:?} {second:?}"
    );
    let (_, expired) = request(
        &app,
        "POST",
        "/api/v1/admin/invitations",
        Some(&owner.access_token),
        json!({"role":"member"}),
    )
    .await;
    sqlx::query("UPDATE action_tokens SET expires_at=now()-interval '1 second' WHERE token_id=$1")
        .bind(expired["token_id"].as_str().unwrap())
        .execute(&postgres.pool)
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/invitations/accept",
            None,
            payload(&expired["token"])
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (_, revoked) = request(
        &app,
        "POST",
        "/api/v1/admin/invitations",
        Some(&owner.access_token),
        json!({"role":"member"}),
    )
    .await;
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!(
                "/api/v1/admin/action-tokens/{}",
                revoked["token_id"].as_str().unwrap()
            ),
            Some(&owner.access_token),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/invitations/accept",
            None,
            payload(&revoked["token"])
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn google_binding_is_explicit_and_password_can_be_added_to_existing_oidc_user() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let postgres = common::migrated_postgres().await;
    common::initialize_installation(
        postgres.pool.clone(),
        "Test",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Default",
    )
    .await;
    let (_, owner) = common::authenticated_router(postgres.pool.clone()).await;
    let app = common::setup_router(
        postgres.pool.clone(),
        "alice@example.com",
        "alice-google-subject",
    );
    let (status, local_owner) = request(
        &app,
        "POST",
        "/api/v1/auth/password",
        Some(&owner.access_token),
        json!({"username":"owner","password":"a long owner password"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{local_owner}");
    assert_eq!(local_owner["user"]["user_id"], owner.user.user_id);
    let (_, invite) = request(
        &app,
        "POST",
        "/api/v1/admin/invitations",
        local_owner["access_token"].as_str(),
        json!({"role":"member"}),
    )
    .await;
    let (_, alice) = request(
        &app,
        "POST",
        "/api/v1/auth/invitations/accept",
        None,
        json!({"token":invite["token"],"username":"alice","password":"a long alice password"}),
    )
    .await;
    let verifier = "test-verifier-abcdefghijklmnopqrstuvwxyz-0123456789";
    let challenge =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier));
    let binding = json!({"authorization":{"client_kind":"desktop","redirect_uri":"http://127.0.0.1:49152/callback","state":"binding-state","code_challenge":challenge,"code_challenge_method":"S256"},"current_password":"a long alice password"});
    // Merely assigning the same email to a local account must not authorize Google login.
    sqlx::query("UPDATE users SET email = 'alice@example.com' WHERE user_id = $1")
        .bind(alice["user"]["user_id"].as_str().unwrap())
        .execute(&postgres.pool)
        .await
        .unwrap();
    let start = app.clone().oneshot(Request::builder()
        .uri(format!("/oauth2/authorization/oidc?client_kind=desktop&redirect_uri=http://127.0.0.1:49152/callback&state=login&code_challenge={challenge}&code_challenge_method=S256"))
        .body(Body::empty()).unwrap()).await.unwrap();
    let url = url::Url::parse(start.headers()["location"].to_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/login/oauth2/code/oidc?code=oidc-code&state={state}"),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, authorization) = request(
        &app,
        "POST",
        "/api/v1/auth/oidc-bindings",
        alice["access_token"].as_str(),
        binding.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{authorization}");
    let url = url::Url::parse(authorization["authorization_url"].as_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let callback = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/login/oauth2/code/oidc?code=oidc-code&state={state}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::FOUND);
    let returned = url::Url::parse(callback.headers()["location"].to_str().unwrap()).unwrap();
    let code = returned
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let (status, linked) = request(&app,"POST","/api/v1/auth/token",None,json!({"grant_type":"authorization_code","code":code,"redirect_uri":"http://127.0.0.1:49152/callback","code_verifier":verifier})).await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    assert_eq!(linked["user"]["user_id"], alice["user"]["user_id"]);
    let (_, google_login) = common::authenticated_router_as(
        postgres.pool.clone(),
        "alice@example.com",
        "alice-google-subject",
        "Alice",
    )
    .await;
    assert_eq!(
        google_login.user.user_id,
        alice["user"]["user_id"].as_str().unwrap()
    );
    let (status, credentials) = request(
        &app,
        "GET",
        "/api/v1/auth/credentials",
        alice["access_token"].as_str(),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{credentials}");
    assert_eq!(credentials["oidc_email"], "alice@example.com");
    // A second account cannot claim the already-bound external subject.
    let mut conflict = binding.clone();
    conflict["current_password"] = json!("a long owner password");
    let (_, authorization) = request(
        &app,
        "POST",
        "/api/v1/auth/oidc-bindings",
        local_owner["access_token"].as_str(),
        conflict,
    )
    .await;
    let url = url::Url::parse(authorization["authorization_url"].as_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/login/oauth2/code/oidc?code=oidc-code&state={state}"),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::FOUND
    );
    // An in-flight binding loses authority as soon as its initiating session is revoked.
    let (_, authorization) = request(
        &app,
        "POST",
        "/api/v1/auth/oidc-bindings",
        alice["access_token"].as_str(),
        binding,
    )
    .await;
    let url = url::Url::parse(authorization["authorization_url"].as_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(
        request(
            &app,
            "DELETE",
            "/api/v1/auth/session",
            alice["access_token"].as_str(),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/login/oauth2/code/oidc?code=oidc-code&state={state}"),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::FOUND
    );
}

#[tokio::test]
async fn first_owner_can_initialize_without_oidc_and_password_policy_can_disable_local_login() {
    use server::app::{auth::AuthService, installation::InstallationService};
    let postgres = common::migrated_postgres().await;
    let installation =
        InstallationService::new(postgres.pool.clone(), Some(common::TEST_SETUP_CODE), false)
            .unwrap();
    let setup = installation
        .create_session(common::TEST_SETUP_CODE)
        .await
        .unwrap();
    installation
        .replace_configuration(
            &setup.token,
            &setup.session.csrf_token,
            server::app::installation::dto::ReplaceSetupConfigurationRequest {
                org_name: "Local".into(),
                default_project_name: "Default".into(),
                allowed_email_domains: vec![],
            },
        )
        .await
        .unwrap();
    let cookie_name = installation.cookie_name().to_owned();
    let app = server::build_app(
        postgres.pool.clone(),
        AuthService::unconfigured(postgres.pool.clone()),
        installation,
    );
    let request_body = json!({"username":"owner","password":"local only owner password"});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/setup/password-owner")
                .header("cookie", format!("{cookie_name}={}", setup.token))
                .header("x-csrf-token", &setup.session.csrf_token)
                .header("content-type", "application/json")
                .body(Body::from(request_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let tokens: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(tokens["user"]["role"], "owner");
    assert!(tokens["user"]["email"].is_null());
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/me",
            tokens["access_token"].as_str(),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, status) = request(&app, "GET", "/api/v1/setup", None, Value::Null).await;
    assert_eq!(status["state"], "initialized");
    let recovery = AuthService::unconfigured(postgres.pool.clone())
        .recover_owner(tokens["user"]["user_id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/password/reset",
            None,
            json!({"token":recovery.token,"password":"recovered owner password"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let disabled = server::build_app(
        postgres.pool.clone(),
        AuthService::unconfigured(postgres.pool.clone()).with_password_enabled(false),
        InstallationService::new(postgres.pool.clone(), None, false).unwrap(),
    );
    assert_eq!(
        request(
            &disabled,
            "POST",
            "/api/v1/auth/password/sessions",
            None,
            request_body
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
