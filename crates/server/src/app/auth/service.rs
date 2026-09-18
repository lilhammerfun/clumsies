//! Application operations and transaction coordination for auth resources.

use super::error::AuthError;
use super::model::{
    AUTHORIZATION_CODE_TTL, AuthPrincipal, LOGIN_TRANSACTION_TTL, LoginFlow, LoginTransaction,
    ProviderSummary, enforce_email_domain,
};
use super::oidc::{DiscoveredOidcProvider, OidcIdentityProvider};
use super::repository;
use crate::app::auth::dto::{
    AdmissionMode, ClientKind, MeResponse, OidcAuthorizationRequest, OidcCallbackRequest,
    OidcProviderStatus, SecretSource, SessionRevoked, TokenGrantType, TokenRequest, TokenResponse,
};
use crate::app::auth::user_capabilities;
use crate::app::installation::{InstallationError, InstallationService};
use crate::app::{organization, project};
use crate::config::PublicOrigin;
use crate::error::ServerError;
use crate::identity::random_token;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openidconnect::PkceCodeChallenge;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::env;
use std::sync::Arc;
use time::OffsetDateTime;
use url::Url;

pub async fn get_me(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
) -> Result<MeResponse, ServerError> {
    let mut tx = pool.begin().await?;
    let user = organization::service::load_user_ref(&mut tx, &principal.user_id).await?;
    let org = organization::service::load_org_ref(&mut tx, &principal.org_id).await?;
    let projects =
        project::service::list_project_refs(&mut tx, &principal.org_id, &principal.user_id).await?;
    let response = MeResponse {
        user,
        org,
        default_project_id: projects.first().map(|project| project.project_id.clone()),
        projects,
        capabilities: user_capabilities(&principal.role),
    };
    tx.commit().await?;
    Ok(response)
}

#[derive(Clone)]
pub struct AuthService {
    pool: PgPool,
    provider: Option<Arc<dyn OidcIdentityProvider>>,
    allowed_redirects: Arc<Vec<Url>>,
    provider_summary: Option<ProviderSummary>,
}

impl AuthService {
    pub fn unconfigured(pool: PgPool) -> Self {
        Self {
            pool,
            provider: None,
            allowed_redirects: Arc::new(Vec::new()),
            provider_summary: None,
        }
    }

    pub fn with_provider(
        pool: PgPool,
        provider: Arc<dyn OidcIdentityProvider>,
        allowed_redirects: Vec<Url>,
    ) -> Self {
        Self {
            pool,
            provider: Some(provider),
            allowed_redirects: Arc::new(allowed_redirects),
            provider_summary: None,
        }
    }

    pub async fn from_env(pool: PgPool, public_origin: &PublicOrigin) -> Result<Self, AuthError> {
        let issuer = optional_env("CLUMSIES_OIDC_ISSUER");
        let client_id = optional_env("CLUMSIES_OIDC_CLIENT_ID");
        let client_secret = optional_env("CLUMSIES_OIDC_CLIENT_SECRET");
        if issuer.is_none() && client_id.is_none() && client_secret.is_none() {
            return Ok(Self::unconfigured(pool));
        }
        let issuer = required_oidc_value("CLUMSIES_OIDC_ISSUER", issuer)?;
        let client_id = required_oidc_value("CLUMSIES_OIDC_CLIENT_ID", client_id)?;
        let client_secret = required_oidc_value("CLUMSIES_OIDC_CLIENT_SECRET", client_secret)?;
        let callback_url = public_origin.oidc_callback_url();
        let mut allowed_redirects = Vec::new();
        if let Some(configured_redirects) = optional_env("CLUMSIES_CLIENT_REDIRECT_URIS") {
            for value in configured_redirects
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let redirect = Url::parse(value).map_err(|error| {
                    AuthError::Configuration(format!(
                        "invalid client redirect URI {value}: {error}"
                    ))
                })?;
                if !allowed_redirects.contains(&redirect) {
                    allowed_redirects.push(redirect);
                }
            }
        }
        let provider = DiscoveredOidcProvider::discover(
            &issuer,
            client_id,
            client_secret,
            callback_url.clone(),
        )
        .await?;
        Ok(Self {
            pool,
            provider: Some(Arc::new(provider)),
            allowed_redirects: Arc::new(allowed_redirects),
            provider_summary: Some(ProviderSummary {
                issuer,
                callback_url,
            }),
        })
    }

    pub fn configured(&self) -> bool {
        self.provider.is_some()
    }

    pub fn provider_status(&self) -> OidcProviderStatus {
        OidcProviderStatus {
            protocol: "oidc".to_owned(),
            configured: self.configured(),
            issuer: self
                .provider_summary
                .as_ref()
                .map(|summary| summary.issuer.clone()),
            callback_url: self
                .provider_summary
                .as_ref()
                .map(|summary| summary.callback_url.clone()),
            admission_mode: AdmissionMode::InviteOnly,
            secret_source: SecretSource::DeploymentEnvironment,
        }
    }

    pub async fn begin_login(
        &self,
        request: OidcAuthorizationRequest,
    ) -> Result<String, AuthError> {
        self.begin_product_login(request).await
    }

    async fn begin_product_login(
        &self,
        request: OidcAuthorizationRequest,
    ) -> Result<String, AuthError> {
        let provider = self.provider.as_ref().ok_or(AuthError::NotConfigured)?;
        let code_challenge_method = request.code_challenge_method.as_deref().ok_or_else(|| {
            AuthError::InvalidRequest("code_challenge_method is required".to_owned())
        })?;
        if code_challenge_method != "S256" {
            return Err(AuthError::InvalidRequest(
                "code_challenge_method must be S256".to_owned(),
            ));
        }
        let code_challenge = request
            .code_challenge
            .as_deref()
            .ok_or_else(|| AuthError::InvalidRequest("code_challenge is required".to_owned()))?;
        validate_code_challenge(code_challenge)?;
        let redirect_uri = request
            .redirect_uri
            .as_deref()
            .ok_or_else(|| AuthError::InvalidRequest("redirect_uri is required".to_owned()))?;
        let client_redirect = Url::parse(redirect_uri)
            .map_err(|error| AuthError::InvalidRequest(error.to_string()))?;
        if !self.redirect_allowed(&client_redirect) {
            return Err(AuthError::RedirectNotAllowed);
        }
        let provider_state = random_token();
        let nonce = random_token();
        let (provider_pkce_challenge, provider_pkce_verifier) =
            PkceCodeChallenge::new_random_sha256();
        let authorization_url = provider.authorization_url(
            &provider_state,
            &nonce,
            provider_pkce_challenge,
            request.login_hint.as_deref(),
        )?;
        repository::insert_product_login_transaction(
            &self.pool,
            repository::ProductLoginTransaction {
                provider_state: &provider_state,
                nonce: &nonce,
                provider_pkce_verifier: provider_pkce_verifier.secret(),
                client_kind: client_kind(request.client_kind),
                client_redirect_uri: client_redirect.as_str(),
                client_state: request.state.as_deref(),
                client_code_challenge: code_challenge,
                expires_at: OffsetDateTime::now_utc() + LOGIN_TRANSACTION_TTL,
            },
        )
        .await?;
        Ok(authorization_url)
    }

    pub async fn begin_setup_login(
        &self,
        setup_session_id: &str,
        redirect_uri: &str,
        state: &str,
        code_challenge: &str,
        code_challenge_method: &str,
    ) -> Result<String, AuthError> {
        let provider = self.provider.as_ref().ok_or(AuthError::NotConfigured)?;
        if state.is_empty() {
            return Err(AuthError::InvalidRequest("state is required".to_owned()));
        }
        if code_challenge_method != "S256" {
            return Err(AuthError::InvalidRequest(
                "code_challenge_method must be S256".to_owned(),
            ));
        }
        validate_code_challenge(code_challenge)?;
        let client_redirect = Url::parse(redirect_uri)
            .map_err(|error| AuthError::InvalidRequest(error.to_string()))?;
        if !self.redirect_allowed(&client_redirect) {
            return Err(AuthError::RedirectNotAllowed);
        }

        let provider_state = random_token();
        let nonce = random_token();
        let (provider_pkce_challenge, provider_pkce_verifier) =
            PkceCodeChallenge::new_random_sha256();
        let authorization_url =
            provider.authorization_url(&provider_state, &nonce, provider_pkce_challenge, None)?;
        repository::insert_setup_login_transaction(
            &self.pool,
            repository::SetupLoginTransaction {
                provider_state: &provider_state,
                nonce: &nonce,
                provider_pkce_verifier: provider_pkce_verifier.secret(),
                client_redirect_uri: client_redirect.as_str(),
                client_state: state,
                client_code_challenge: code_challenge,
                expires_at: OffsetDateTime::now_utc() + LOGIN_TRANSACTION_TTL,
                setup_session_id,
            },
        )
        .await?;
        Ok(authorization_url)
    }

    pub async fn complete_login(
        &self,
        request: OidcCallbackRequest,
        installation: &InstallationService,
    ) -> Result<String, AuthError> {
        let provider = self.provider.as_ref().ok_or(AuthError::NotConfigured)?;
        let transaction = repository::login_transaction(&self.pool, &request.state).await?;
        if let Some(provider_error) = request.error {
            repository::consume_login_transaction(&self.pool, &transaction.transaction_id).await?;
            return callback_redirect(
                &transaction,
                None,
                Some((&provider_error, request.error_description.as_deref())),
            );
        }
        let code = request.code.ok_or_else(|| {
            AuthError::InvalidRequest("OIDC callback requires code or error".to_owned())
        })?;
        let identity = provider
            .exchange_code(
                &code,
                &transaction.nonce,
                &transaction.provider_pkce_verifier,
            )
            .await?;
        if !identity.email_verified {
            return Err(AuthError::EmailNotVerified);
        }
        let mut tx = self.pool.begin().await?;
        if !repository::consume_login_transaction_in(&mut tx, &transaction.transaction_id).await? {
            return Err(AuthError::LoginTransactionExpired);
        }
        let (user_id, org_id) = if transaction.flow == LoginFlow::InstallationSetup {
            let setup_session_id = transaction
                .setup_session_id
                .as_deref()
                .ok_or(AuthError::CorruptLoginTransaction)?;
            let initialized = match installation
                .initialize_with_oidc(&mut tx, setup_session_id, &identity)
                .await
            {
                Ok(initialized) => initialized,
                Err(error) => {
                    tx.rollback().await?;
                    if matches!(
                        error,
                        InstallationError::ConfigurationRequired
                            | InstallationError::OwnerDomainNotAllowed
                            | InstallationError::InvalidOwnerIdentity
                    ) {
                        repository::consume_login_transaction(
                            &self.pool,
                            &transaction.transaction_id,
                        )
                        .await?;
                        return callback_redirect(&transaction, None, Some((error.code(), None)));
                    }
                    return Err(error.into());
                }
            };
            (initialized.user_id, initialized.org_id)
        } else {
            let org = repository::organization_admission(&mut tx).await?;
            enforce_email_domain(&identity.email, &org.allowed_email_domains)?;
            let user_id = repository::resolve_external_identity(&mut tx, &identity).await?;
            (user_id, org.org_id)
        };

        let authorization_code = random_token();
        let client_code_challenge = transaction
            .client_code_challenge
            .as_deref()
            .ok_or(AuthError::CorruptLoginTransaction)?;
        repository::insert_authorization_code(
            &mut tx,
            &authorization_code,
            &user_id,
            &org_id,
            &transaction.client_redirect_uri,
            client_code_challenge,
            OffsetDateTime::now_utc() + AUTHORIZATION_CODE_TTL,
        )
        .await?;
        repository::insert_audit_event(
            &mut tx,
            &org_id,
            Some(&user_id),
            "auth.oidc_login_completed",
            "session",
            None,
        )
        .await?;
        tx.commit().await?;
        callback_redirect(&transaction, Some(&authorization_code), None)
    }

    pub async fn exchange_token(&self, request: TokenRequest) -> Result<TokenResponse, AuthError> {
        match request.grant_type {
            TokenGrantType::AuthorizationCode => self.exchange_authorization_code(request).await,
            TokenGrantType::RefreshToken => self.rotate_refresh_token(request).await,
        }
    }

    pub async fn authenticate(&self, bearer_token: &str) -> Result<AuthPrincipal, AuthError> {
        repository::authenticate_bearer(&self.pool, bearer_token).await
    }

    pub async fn revoke_session(
        &self,
        principal: &AuthPrincipal,
    ) -> Result<SessionRevoked, AuthError> {
        let mut tx = self.pool.begin().await?;
        let response = repository::revoke_session(&mut tx, principal).await?;
        tx.commit().await?;
        Ok(response)
    }

    async fn exchange_authorization_code(
        &self,
        request: TokenRequest,
    ) -> Result<TokenResponse, AuthError> {
        let code = request.code.ok_or(AuthError::InvalidGrant)?;
        let redirect_uri = request.redirect_uri.ok_or(AuthError::InvalidGrant)?;
        let verifier = request.code_verifier.ok_or(AuthError::InvalidGrant)?;
        let actual_challenge = code_challenge(&verifier);
        let mut tx = self.pool.begin().await?;
        let response = repository::exchange_authorization_code(
            &mut tx,
            &code,
            &redirect_uri,
            &actual_challenge,
        )
        .await?;
        tx.commit().await?;
        Ok(response)
    }

    async fn rotate_refresh_token(
        &self,
        request: TokenRequest,
    ) -> Result<TokenResponse, AuthError> {
        let refresh_token = request.refresh_token.ok_or(AuthError::InvalidGrant)?;
        let mut tx = self.pool.begin().await?;
        let response = repository::rotate_refresh_token(&mut tx, &refresh_token).await?;
        tx.commit().await?;
        Ok(response)
    }

    fn redirect_allowed(&self, requested: &Url) -> bool {
        self.allowed_redirects
            .iter()
            .any(|allowed| redirect_matches(allowed, requested))
    }
}

fn callback_redirect(
    transaction: &LoginTransaction,
    code: Option<&str>,
    error: Option<(&str, Option<&str>)>,
) -> Result<String, AuthError> {
    let mut url = Url::parse(&transaction.client_redirect_uri)
        .map_err(|parse_error| AuthError::InvalidRequest(parse_error.to_string()))?;
    let has_query_values = code.is_some() || error.is_some() || transaction.client_state.is_some();
    if !has_query_values {
        return Ok(url.to_string());
    }
    {
        let mut query = url.query_pairs_mut();
        if let Some(code) = code {
            query.append_pair("code", code);
            query.append_pair(
                "expires_in",
                &AUTHORIZATION_CODE_TTL.whole_seconds().to_string(),
            );
        }
        if let Some((error, description)) = error {
            query.append_pair("error", error);
            if let Some(description) = description {
                query.append_pair("error_description", description);
            }
        }
        if let Some(state) = &transaction.client_state {
            query.append_pair("state", state);
        }
    }
    Ok(url.to_string())
}

fn validate_code_challenge(value: &str) -> Result<(), AuthError> {
    if value.len() != 43
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(AuthError::InvalidRequest(
            "code_challenge must be a SHA-256 base64url value".to_owned(),
        ));
    }
    Ok(())
}

fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn redirect_matches(allowed: &Url, requested: &Url) -> bool {
    if allowed == requested {
        return true;
    }
    allowed.scheme() == "http"
        && requested.scheme() == "http"
        && allowed.port().is_none()
        && is_loopback_host(allowed.host_str())
        && allowed.host_str() == requested.host_str()
        && allowed.path() == requested.path()
        && allowed.query() == requested.query()
        && requested.port().is_some()
}

fn is_loopback_host(host: Option<&str>) -> bool {
    matches!(host, Some("127.0.0.1") | Some("[::1]") | Some("localhost"))
}

fn client_kind(kind: ClientKind) -> &'static str {
    match kind {
        ClientKind::Desktop => "desktop",
        ClientKind::Cli => "cli",
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn required_oidc_value(name: &str, value: Option<String>) -> Result<String, AuthError> {
    match value {
        Some(value) if !value.eq_ignore_ascii_case("null") => Ok(value),
        _ => Err(AuthError::Configuration(format!(
            "{name} is required and must not be null"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oidc_configuration_rejects_null_placeholders() {
        let error = required_oidc_value("CLUMSIES_OIDC_CLIENT_ID", Some("null".to_owned()))
            .expect_err("null must not be accepted as an OIDC credential");

        assert!(matches!(error, AuthError::Configuration(_)));
    }
}
