//! Application operations and transaction coordination for auth resources.

use super::error::AuthError;
use super::model::{
    AUTHORIZATION_CODE_TTL, AuthPrincipal, LOGIN_TRANSACTION_TTL, LoginFlow, LoginTransaction,
    ProviderSummary, enforce_email_domain,
};
use super::oidc::OidcIdentityProvider;
use super::repository;
use crate::app::auth::dto::{
    AdmissionMode, ClientKind, MeResponse, OidcAuthorizationRequest, OidcCallbackRequest,
    OidcProviderStatus, SecretSource, SessionRevoked, TokenGrantType, TokenRequest, TokenResponse,
};
use crate::app::auth::user_capabilities;
use crate::app::installation::{InstallationError, InstallationService};
use crate::app::{organization, project};
use crate::error::ServerError;
use crate::identity::random_token;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openidconnect::PkceCodeChallenge;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::Arc;
use time::OffsetDateTime;
use url::Url;

/// Assemble the authenticated user's identity, organization, accessible projects, and
/// capabilities.
///
/// # Errors
/// Reports a missing installed organization or user and propagates database failures.
pub async fn get_me(
    pool: &sqlx::PgPool,
    principal: &AuthPrincipal,
) -> Result<MeResponse, ServerError> {
    let mut tx = pool.begin().await?;
    let user = organization::load_user_ref(&mut tx, &principal.user_id).await?;
    let org = organization::load_org_ref(&mut tx, &principal.org_id).await?;
    let projects =
        project::list_project_refs(&mut tx, &principal.org_id, &principal.user_id).await?;
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

/// Reusable authentication dependencies and login/session operations.
#[derive(Clone)]
pub struct AuthService {
    /// Shared connection pool reused by authentication persistence operations.
    pool: PgPool,
    /// Reusable identity-provider adapter; absent when login is unconfigured.
    provider: Option<Arc<dyn OidcIdentityProvider>>,
    /// Explicitly permitted client callback URLs, including loopback callback templates.
    allowed_redirects: Arc<Vec<Url>>,
    /// Non-secret provider configuration exposed to administrators.
    provider_summary: Option<ProviderSummary>,
}

impl AuthService {
    /// Construct authentication with no provider while retaining the shared database dependency.
    pub fn unconfigured(pool: PgPool) -> Self {
        Self {
            pool,
            provider: None,
            allowed_redirects: Arc::new(Vec::new()),
            provider_summary: None,
        }
    }

    /// Construct authentication from a reusable provider and explicit redirect configuration
    /// without I/O.
    pub fn with_provider(
        pool: PgPool,
        provider: Arc<dyn OidcIdentityProvider>,
        allowed_redirects: Vec<Url>,
        provider_summary: Option<ProviderSummary>,
    ) -> Self {
        Self {
            pool,
            provider: Some(provider),
            allowed_redirects: Arc::new(allowed_redirects),
            provider_summary,
        }
    }

    /// Report whether an identity-provider adapter is available for login.
    pub fn configured(&self) -> bool {
        self.provider.is_some()
    }

    /// Return non-secret identity-provider settings only to an organization administrator.
    ///
    /// # Errors
    /// Rejects principals without organization-administration privileges.
    pub fn provider_status(
        &self,
        principal: &AuthPrincipal,
    ) -> Result<OidcProviderStatus, ServerError> {
        principal.require_org_admin()?;
        Ok(OidcProviderStatus {
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
        })
    }

    /// Select setup or normal admission and construct the provider authorization redirect.
    ///
    /// # Errors
    /// Propagates invalid or expired authentication state, admission failures, and provider or
    /// persistence errors from the login operation.
    pub async fn begin_login(
        &self,
        request: OidcAuthorizationRequest,
    ) -> Result<String, AuthError> {
        self.begin_product_login(request).await
    }

    /// Validate the native client's redirect and PKCE inputs before persisting a one-time login
    /// transaction.
    ///
    /// # Errors
    /// Rejects missing provider configuration, invalid state or PKCE parameters, and callbacks
    /// outside the allowlist; propagates authorization-URL construction and persistence failures.
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

    /// Bind first-owner OIDC authorization to a previously authorized setup session.
    ///
    /// # Errors
    /// Rejects missing provider configuration, invalid state or PKCE parameters, and callbacks
    /// outside the allowlist; propagates authorization-URL construction and persistence failures.
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

    /// Consume provider correlation state, verify the identity, and issue a client authorization
    /// redirect.
    ///
    /// # Errors
    /// Propagates invalid or expired authentication state, admission failures, and provider or
    /// persistence errors from the login operation.
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

    /// Redeem a one-time authorization code or rotate a refresh credential.
    ///
    /// # Errors
    /// Rejects missing grant parameters, invalid or consumed authorization codes, mismatched
    /// redirect or PKCE proof, expired refresh credentials, and database failures.
    pub async fn exchange_token(&self, request: TokenRequest) -> Result<TokenResponse, AuthError> {
        match request.grant_type {
            TokenGrantType::AuthorizationCode => self.exchange_authorization_code(request).await,
            TokenGrantType::RefreshToken => self.rotate_refresh_token(request).await,
        }
    }

    /// Resolve a valid bearer credential into a trusted, enabled session identity.
    ///
    /// # Errors
    /// Rejects expired, revoked, missing, or disabled session identities and propagates database
    /// failures.
    pub async fn authenticate(&self, bearer_token: &str) -> Result<AuthPrincipal, AuthError> {
        repository::authenticate_bearer(&self.pool, bearer_token).await
    }

    /// Invalidate the authenticated session and all of its issued credentials.
    ///
    /// # Errors
    /// Propagates database failures while revoking the session and all of its issued credentials
    /// in one transaction.
    pub async fn revoke_session(
        &self,
        principal: &AuthPrincipal,
    ) -> Result<SessionRevoked, AuthError> {
        let mut tx = self.pool.begin().await?;
        let response = repository::revoke_session(&mut tx, principal).await?;
        tx.commit().await?;
        Ok(response)
    }

    /// Verify the redirect and PKCE proof before issuing the initial token pair.
    ///
    /// # Errors
    /// Rejects invalid, expired, or consumed grants and mismatched redirect or PKCE proof, and
    /// propagates database failures. Grant consumption and credential issuance commit atomically.
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

    /// Consume a valid refresh credential and issue a replacement token pair atomically.
    ///
    /// # Errors
    /// Rejects invalid, expired, revoked, or already-consumed refresh credentials and propagates
    /// database failures. Credential rotation commits atomically.
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

    /// Check an exact configured callback or an allowed dynamic loopback port.
    fn redirect_allowed(&self, requested: &Url) -> bool {
        self.allowed_redirects
            .iter()
            .any(|allowed| redirect_matches(allowed, requested))
    }
}

/// Append the one-time authorization result and original client state to its validated callback.
///
/// # Errors
/// Propagates invalid callback URL construction while preserving the validated client's
/// correlation state.
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

/// Require a correctly encoded S256 PKCE challenge before starting authentication.
///
/// # Errors
/// Rejects values that are not a correctly encoded S256 PKCE challenge.
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

/// Calculate the S256 PKCE challenge corresponding to a secret verifier.
fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Compare callback URLs while allowing dynamic ports only for configured loopback HTTP
/// callbacks.
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

/// Recognize supported local callback hosts without DNS resolution.
fn is_loopback_host(host: Option<&str>) -> bool {
    matches!(host, Some("127.0.0.1") | Some("[::1]") | Some("localhost"))
}

/// Encode the native client category used by persisted login transactions.
fn client_kind(kind: ClientKind) -> &'static str {
    match kind {
        ClientKind::Desktop => "desktop",
        ClientKind::Cli => "cli",
    }
}
