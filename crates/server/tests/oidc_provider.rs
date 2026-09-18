use chrono::{Duration, Utc};
use openidconnect::core::{
    CoreIdToken, CoreIdTokenClaims, CoreJsonWebKeySet, CoreJwsSigningAlgorithm,
    CoreRsaPrivateSigningKey,
};
use openidconnect::{
    AccessToken, Audience, EmptyAdditionalClaims, EndUserEmail, EndUserName, EndUserPictureUrl,
    IssuerUrl, JsonWebKeyId, Nonce, PkceCodeChallenge, PrivateSigningKey, StandardClaims,
    SubjectIdentifier, reqwest,
};
use rand::rngs::OsRng;
use rsa::RsaPrivateKey;
use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};
use serde_json::{Value, json};
use server::app::auth::{AuthError, DiscoveredOidcProvider, OidcIdentityProvider};
use std::sync::OnceLock;
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLIENT_ID: &str = "clumsies-test-client";
const CLIENT_SECRET: &str = "clumsies-test-secret";
const CALLBACK_URL: &str = "http://127.0.0.1:8080/login/oauth2/code/oidc";
const PROVIDER_ACCESS_TOKEN: &str = "provider-access-token";
const PROVIDER_NONCE: &str = "provider-nonce";
const PROVIDER_PKCE_VERIFIER: &str = "provider-pkce-verifier";
const LOCAL_FAKE_OIDC_CONFIG: &str = include_str!("../../../dev/oidc/config.json");
const LOCAL_FAKE_OIDC_IMAGE: &str = "ghcr.io/navikt/mock-oauth2-server";
const LOCAL_FAKE_OIDC_TAG: &str = "4.0.0";

#[tokio::test]
async fn local_fake_provider_completes_the_oidc_protocol() {
    let container = GenericImage::new(LOCAL_FAKE_OIDC_IMAGE, LOCAL_FAKE_OIDC_TAG)
        .with_exposed_port(8080.tcp())
        .with_env_var("JSON_CONFIG", LOCAL_FAKE_OIDC_CONFIG)
        .start()
        .await
        .unwrap();
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(8080).await.unwrap();
    let origin = format!("http://{host}:{port}");
    wait_for_fake_provider(&origin).await;

    let issuer = format!("{origin}/clumsies");
    let provider = DiscoveredOidcProvider::discover(
        &issuer,
        "clumsies-local".to_owned(),
        "clumsies-local-secret".to_owned(),
        CALLBACK_URL.to_owned(),
    )
    .await
    .unwrap();
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let authorization_url = provider
        .authorization_url("provider-state", PROVIDER_NONCE, challenge, None)
        .unwrap();
    let http = reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let response = http.get(authorization_url).send().await.unwrap();
    assert!(response.status().is_redirection());
    let callback = Url::parse(
        response
            .headers()
            .get(reqwest::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(query(&callback, "state"), "provider-state");

    let identity = provider
        .exchange_code(&query(&callback, "code"), PROVIDER_NONCE, verifier.secret())
        .await
        .unwrap();
    assert_eq!(identity.issuer, issuer);
    assert_eq!(identity.subject, "local-owner");
    assert_eq!(identity.email, "owner@clumsies.local");
    assert!(identity.email_verified);
    assert_eq!(identity.display_name.as_deref(), Some("Local Owner"));
    assert_eq!(identity.avatar_url, None);
}

async fn wait_for_fake_provider(origin: &str) {
    let http = reqwest::Client::new();
    let health_url = format!("{origin}/isalive");
    for _ in 0..300 {
        if http
            .get(&health_url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("local fake OIDC provider did not become ready at {health_url}");
}

#[tokio::test]
async fn discovered_provider_completes_the_oidc_protocol() {
    let server = MockServer::start().await;
    let signing_key = signing_key("key-1");
    mount_discovery(&server, &server.uri(), &signing_key, 1).await;
    mount_token(
        &server,
        token_response(&signing_key, &server.uri(), PROVIDER_NONCE),
        1,
    )
    .await;

    let provider = discover(&server).await.unwrap();
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let authorization_url = Url::parse(
        &provider
            .authorization_url(
                "provider-state",
                PROVIDER_NONCE,
                challenge,
                Some("person@example.com"),
            )
            .unwrap(),
    )
    .unwrap();

    assert_eq!(authorization_url.path(), "/authorize");
    assert_eq!(query(&authorization_url, "client_id"), CLIENT_ID);
    assert_eq!(query(&authorization_url, "response_type"), "code");
    assert_eq!(query(&authorization_url, "redirect_uri"), CALLBACK_URL);
    assert_eq!(query(&authorization_url, "state"), "provider-state");
    assert_eq!(query(&authorization_url, "nonce"), PROVIDER_NONCE);
    assert_eq!(query(&authorization_url, "code_challenge_method"), "S256");
    assert_eq!(
        query(&authorization_url, "login_hint"),
        "person@example.com"
    );
    let scopes = query(&authorization_url, "scope");
    assert!(
        scopes
            .split(' ')
            .all(|scope| ["openid", "email", "profile"].contains(&scope))
    );
    assert_eq!(scopes.split(' ').count(), 3);

    let identity = provider
        .exchange_code("provider-code", PROVIDER_NONCE, verifier.secret())
        .await
        .unwrap();
    assert_eq!(identity.issuer, server.uri());
    assert_eq!(identity.subject, "oidc-subject");
    assert_eq!(identity.email, "person@example.com");
    assert!(identity.email_verified);
    assert_eq!(identity.display_name.as_deref(), Some("Person Example"));
    assert_eq!(
        identity.avatar_url.as_deref(),
        Some("https://images.example.test/person.png")
    );
}

#[tokio::test]
async fn discovery_rejects_an_issuer_mismatch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(discovery_document(
            "https://different-issuer.example.test",
            &server.uri(),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let error = match discover(&server).await {
        Ok(_) => panic!("issuer mismatch should fail discovery"),
        Err(error) => error,
    };
    assert!(matches!(&error, AuthError::ProviderUnavailable(_)));
    assert_eq!(error.code(), "oidc_provider_unavailable");
}

#[tokio::test]
async fn token_endpoint_protocol_errors_are_invalid_provider_responses() {
    let server = MockServer::start().await;
    let signing_key = signing_key("key-1");
    mount_discovery(&server, &server.uri(), &signing_key, 1).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "authorization code is invalid"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider = discover(&server).await.unwrap();
    let error = provider
        .exchange_code(
            "invalid-provider-code",
            PROVIDER_NONCE,
            PROVIDER_PKCE_VERIFIER,
        )
        .await
        .unwrap_err();
    assert!(matches!(&error, AuthError::ProviderCodeExchangeFailed(_)));
    assert_eq!(error.code(), "oidc_code_exchange_failed");
}

#[tokio::test]
async fn id_token_rejects_a_nonce_mismatch() {
    let server = MockServer::start().await;
    let signing_key = signing_key("key-1");
    mount_discovery(&server, &server.uri(), &signing_key, 1).await;
    mount_token(
        &server,
        token_response(&signing_key, &server.uri(), "different-nonce"),
        1,
    )
    .await;

    let provider = discover(&server).await.unwrap();
    let error = provider
        .exchange_code("provider-code", PROVIDER_NONCE, PROVIDER_PKCE_VERIFIER)
        .await
        .unwrap_err();
    assert!(matches!(&error, AuthError::ProviderInvalid(_)));
    assert_eq!(error.code(), "oidc_id_token_invalid");
}

#[tokio::test]
async fn id_token_rejects_an_invalid_signature() {
    let server = MockServer::start().await;
    let published_key = signing_key("stable-key-id");
    let invalid_signing_key = alternate_signing_key("stable-key-id");
    mount_discovery(&server, &server.uri(), &published_key, 1).await;
    mount_token(
        &server,
        token_response(&invalid_signing_key, &server.uri(), PROVIDER_NONCE),
        1,
    )
    .await;

    let provider = discover(&server).await.unwrap();
    let error = provider
        .exchange_code("provider-code", PROVIDER_NONCE, PROVIDER_PKCE_VERIFIER)
        .await
        .unwrap_err();
    assert!(matches!(&error, AuthError::ProviderInvalid(_)));
    assert_eq!(error.code(), "oidc_id_token_invalid");
}

#[tokio::test]
async fn unknown_signing_key_refreshes_jwks_without_redeeming_the_code_twice() {
    let server = MockServer::start().await;
    let original_key = signing_key("key-1");
    mount_discovery(&server, &server.uri(), &original_key, 1).await;
    let provider = discover(&server).await.unwrap();

    server.reset().await;
    let rotated_key = signing_key("key-2");
    mount_discovery(&server, &server.uri(), &rotated_key, 1).await;
    mount_token(
        &server,
        token_response(&rotated_key, &server.uri(), PROVIDER_NONCE),
        1,
    )
    .await;

    let identity = provider
        .exchange_code("provider-code", PROVIDER_NONCE, PROVIDER_PKCE_VERIFIER)
        .await
        .unwrap();
    assert_eq!(identity.subject, "oidc-subject");
}

async fn discover(server: &MockServer) -> Result<DiscoveredOidcProvider, AuthError> {
    DiscoveredOidcProvider::discover(
        &server.uri(),
        CLIENT_ID.to_owned(),
        CLIENT_SECRET.to_owned(),
        CALLBACK_URL.to_owned(),
    )
    .await
}

async fn mount_discovery(
    server: &MockServer,
    document_issuer: &str,
    signing_key: &CoreRsaPrivateSigningKey,
    expected_calls: u64,
) {
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(discovery_document(document_issuer, &server.uri())),
        )
        .expect(expected_calls)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(CoreJsonWebKeySet::new(vec![
                signing_key.as_verification_key(),
            ])),
        )
        .expect(expected_calls)
        .mount(server)
        .await;
}

async fn mount_token(server: &MockServer, response: Value, expected_calls: u64) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(expected_calls)
        .mount(server)
        .await;
}

fn discovery_document(issuer: &str, endpoint_base: &str) -> Value {
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{endpoint_base}/authorize"),
        "token_endpoint": format!("{endpoint_base}/token"),
        "jwks_uri": format!("{endpoint_base}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"]
    })
}

fn signing_key(kid: &str) -> CoreRsaPrivateSigningKey {
    signing_key_from_pem(primary_key_pem(), kid)
}

fn alternate_signing_key(kid: &str) -> CoreRsaPrivateSigningKey {
    signing_key_from_pem(alternate_key_pem(), kid)
}

fn signing_key_from_pem(pem: &str, kid: &str) -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(pem, Some(JsonWebKeyId::new(kid.to_owned()))).unwrap()
}

fn primary_key_pem() -> &'static str {
    static PEM: OnceLock<String> = OnceLock::new();
    PEM.get_or_init(generate_rsa_key_pem)
}

fn alternate_key_pem() -> &'static str {
    static PEM: OnceLock<String> = OnceLock::new();
    PEM.get_or_init(generate_rsa_key_pem)
}

fn generate_rsa_key_pem() -> String {
    let private_key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
    private_key
        .to_pkcs1_pem(LineEnding::LF)
        .unwrap()
        .to_string()
}

fn token_response(signing_key: &CoreRsaPrivateSigningKey, issuer: &str, nonce: &str) -> Value {
    let access_token = AccessToken::new(PROVIDER_ACCESS_TOKEN.to_owned());
    let standard_claims = StandardClaims::new(SubjectIdentifier::new("oidc-subject".to_owned()))
        .set_email(Some(EndUserEmail::new("person@example.com".to_owned())))
        .set_email_verified(Some(true))
        .set_name(Some(EndUserName::new("Person Example".to_owned()).into()))
        .set_picture(Some(
            EndUserPictureUrl::new("https://images.example.test/person.png".to_owned()).into(),
        ));
    let claims = CoreIdTokenClaims::new(
        IssuerUrl::new(issuer.to_owned()).unwrap(),
        vec![Audience::new(CLIENT_ID.to_owned())],
        Utc::now() + Duration::minutes(5),
        Utc::now(),
        standard_claims,
        EmptyAdditionalClaims {},
    )
    .set_nonce(Some(Nonce::new(nonce.to_owned())));
    let id_token = CoreIdToken::new(
        claims,
        signing_key,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        Some(&access_token),
        None,
    )
    .unwrap();

    json!({
        "access_token": PROVIDER_ACCESS_TOKEN,
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": id_token.to_string()
    })
}

fn query(url: &Url, name: &str) -> String {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| panic!("missing {name} query parameter in {url}"))
}
