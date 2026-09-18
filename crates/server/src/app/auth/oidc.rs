//! OIDC discovery, token exchange, and identity verification for auth resources.

use super::error::AuthError;
use super::model::OidcIdentity;
use async_trait::async_trait;
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AccessTokenHash, AuthorizationCode, ClaimsVerificationError, ClientId, ClientSecret, CsrfToken,
    EndpointMaybeSet, EndpointNotSet, EndpointSet, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RequestTokenError, Scope,
    SignatureVerificationError, TokenResponse as _, reqwest,
};
use std::sync::RwLock;
use url::Url;

const OIDC_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const OIDC_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

type DiscoveredCoreClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

#[async_trait]
pub trait OidcIdentityProvider: Send + Sync {
    fn authorization_url(
        &self,
        provider_state: &str,
        nonce: &str,
        provider_pkce_challenge: PkceCodeChallenge,
        login_hint: Option<&str>,
    ) -> Result<String, AuthError>;

    async fn exchange_code(
        &self,
        code: &str,
        nonce: &str,
        provider_pkce_verifier: &str,
    ) -> Result<OidcIdentity, AuthError>;
}

pub struct DiscoveredOidcProvider {
    issuer: String,
    client_id: String,
    client_secret: String,
    callback_url: String,
    client: RwLock<DiscoveredCoreClient>,
    http_client: reqwest::Client,
}

impl DiscoveredOidcProvider {
    pub async fn discover(
        issuer: &str,
        client_id: String,
        client_secret: String,
        callback_url: String,
    ) -> Result<Self, AuthError> {
        let issuer = IssuerUrl::new(issuer.to_owned())
            .map_err(|error| AuthError::Configuration(error.to_string()))?
            .to_string();
        let http_client = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(OIDC_CONNECT_TIMEOUT)
            .timeout(OIDC_REQUEST_TIMEOUT)
            .build()
            .map_err(|error| AuthError::ProviderUnavailable(error.to_string()))?;
        let client = discover_oidc_client(
            &issuer,
            &client_id,
            &client_secret,
            &callback_url,
            &http_client,
        )
        .await?;
        Ok(Self {
            issuer,
            client_id,
            client_secret,
            callback_url,
            client: RwLock::new(client),
            http_client,
        })
    }

    fn client(&self) -> Result<DiscoveredCoreClient, AuthError> {
        self.client
            .read()
            .map_err(|_| AuthError::ProviderUnavailable("OIDC client lock is poisoned".to_owned()))
            .map(|client| client.clone())
    }

    async fn refresh_client(&self) -> Result<DiscoveredCoreClient, AuthError> {
        let client = discover_oidc_client(
            &self.issuer,
            &self.client_id,
            &self.client_secret,
            &self.callback_url,
            &self.http_client,
        )
        .await?;
        *self.client.write().map_err(|_| {
            AuthError::ProviderUnavailable("OIDC client lock is poisoned".to_owned())
        })? = client.clone();
        Ok(client)
    }
}

async fn discover_oidc_client(
    issuer: &str,
    client_id: &str,
    client_secret: &str,
    callback_url: &str,
    http_client: &reqwest::Client,
) -> Result<DiscoveredCoreClient, AuthError> {
    let issuer = IssuerUrl::new(issuer.to_owned())
        .map_err(|error| AuthError::Configuration(error.to_string()))?;
    let metadata = CoreProviderMetadata::discover_async(issuer, http_client)
        .await
        .map_err(|error| AuthError::ProviderUnavailable(error.to_string()))?;
    Ok(CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(client_id.to_owned()),
        Some(ClientSecret::new(client_secret.to_owned())),
    )
    .set_redirect_uri(
        RedirectUrl::new(callback_url.to_owned())
            .map_err(|error| AuthError::Configuration(error.to_string()))?,
    ))
}

#[async_trait]
impl OidcIdentityProvider for DiscoveredOidcProvider {
    fn authorization_url(
        &self,
        provider_state: &str,
        nonce: &str,
        provider_pkce_challenge: PkceCodeChallenge,
        login_hint: Option<&str>,
    ) -> Result<String, AuthError> {
        let state = provider_state.to_owned();
        let nonce = nonce.to_owned();
        let client = self.client()?;
        let mut request = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                move || CsrfToken::new(state),
                move || Nonce::new(nonce),
            )
            .add_scope(Scope::new("email".to_owned()))
            .add_scope(Scope::new("profile".to_owned()))
            .set_pkce_challenge(provider_pkce_challenge);
        if let Some(login_hint) = login_hint {
            request = request.add_extra_param("login_hint", login_hint);
        }
        let (url, _, _) = request.url();
        Ok(url.to_string())
    }

    async fn exchange_code(
        &self,
        code: &str,
        nonce: &str,
        provider_pkce_verifier: &str,
    ) -> Result<OidcIdentity, AuthError> {
        let client = self.client()?;
        let response = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|error| AuthError::ProviderCodeExchangeFailed(error.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(provider_pkce_verifier.to_owned()))
            .request_async(&self.http_client)
            .await
            .map_err(|error| match error {
                RequestTokenError::Request(error) => {
                    AuthError::ProviderUnavailable(error.to_string())
                }
                RequestTokenError::ServerResponse(error) => {
                    AuthError::ProviderCodeExchangeFailed(error.to_string())
                }
                RequestTokenError::Parse(error, _) => {
                    AuthError::ProviderCodeExchangeFailed(error.to_string())
                }
                RequestTokenError::Other(error) => AuthError::ProviderCodeExchangeFailed(error),
            })?;
        let id_token = response.id_token().ok_or_else(|| {
            AuthError::ProviderInvalid("provider returned no ID token".to_owned())
        })?;
        let nonce = Nonce::new(nonce.to_owned());
        let initial_verifier = client.id_token_verifier();
        let verification_client = match id_token.claims(&initial_verifier, &nonce) {
            Ok(_) => client.clone(),
            Err(error) if needs_jwks_refresh(&error) => self.refresh_client().await?,
            Err(error) => return Err(AuthError::ProviderInvalid(error.to_string())),
        };
        let verifier = verification_client.id_token_verifier();
        let claims = id_token
            .claims(&verifier, &nonce)
            .map_err(|error| AuthError::ProviderInvalid(error.to_string()))?;
        if let Some(expected_hash) = claims.access_token_hash() {
            let actual_hash = AccessTokenHash::from_token(
                response.access_token(),
                id_token
                    .signing_alg()
                    .map_err(|error| AuthError::ProviderInvalid(error.to_string()))?,
                id_token
                    .signing_key(&verifier)
                    .map_err(|error| AuthError::ProviderInvalid(error.to_string()))?,
            )
            .map_err(|error| AuthError::ProviderInvalid(error.to_string()))?;
            if actual_hash != *expected_hash {
                return Err(AuthError::ProviderInvalid(
                    "provider access token hash does not match the ID token".to_owned(),
                ));
            }
        }
        let email = claims
            .email()
            .map(|email| email.as_str().to_owned())
            .ok_or(AuthError::EmailNotVerified)?;
        let display_name = claims
            .name()
            .and_then(|claim| claim.get(None))
            .map(|name| name.as_str().to_owned())
            .filter(|name| !name.trim().is_empty());
        let avatar_url = claims
            .picture()
            .and_then(|claim| claim.get(None))
            .map(|picture| picture.as_str().to_owned())
            .filter(|value| Url::parse(value).is_ok_and(|url| url.scheme() == "https"));
        Ok(OidcIdentity {
            issuer: self.issuer.clone(),
            subject: claims.subject().as_str().to_owned(),
            email,
            email_verified: claims.email_verified().unwrap_or(false),
            display_name,
            avatar_url,
        })
    }
}

fn needs_jwks_refresh(error: &ClaimsVerificationError) -> bool {
    matches!(
        error,
        ClaimsVerificationError::SignatureVerification(SignatureVerificationError::NoMatchingKey)
    )
}
