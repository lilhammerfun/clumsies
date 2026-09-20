//! Resource-specific failures for auth resources.

use crate::app::installation::InstallationError;
use thiserror::Error;

/// Authentication and identity-provider failures with stable public codes.
#[derive(Debug, Error)]
pub enum AuthError {
    /// No identity-provider dependency was configured for this server.
    #[error("OIDC is not configured")]
    NotConfigured,
    /// Deployment credentials or callback settings are invalid.
    #[error("invalid authentication configuration: {0}")]
    Configuration(String),
    /// Login input violates the callback, state, or PKCE contract.
    #[error("invalid authentication request: {0}")]
    InvalidRequest(String),
    /// The native callback is outside the configured redirect allowlist.
    #[error("redirect URI is not allowed")]
    RedirectNotAllowed,
    /// The provider could not be reached or its metadata could not be refreshed.
    #[error("OIDC provider is unavailable: {0}")]
    ProviderUnavailable(String),
    /// The provider rejected authorization-code redemption or its transport failed.
    #[error("OIDC authorization code exchange failed: {0}")]
    ProviderCodeExchangeFailed(String),
    /// The provider response fails cryptographic or identity-claim validation.
    #[error("OIDC identity response is invalid: {0}")]
    ProviderInvalid(String),
    /// Correlation state has expired, was consumed, or cannot be found.
    #[error("OIDC login transaction is expired or already consumed")]
    LoginTransactionExpired,
    /// Persisted login state cannot be decoded into the required flow.
    #[error("stored OIDC login transaction is corrupt")]
    CorruptLoginTransaction,
    /// The provider has not verified the identity's email address.
    #[error("OIDC email is not verified")]
    EmailNotVerified,
    /// The verified identity is not an enabled admitted member.
    #[error("member is not admitted to this Server")]
    MemberNotAllowed,
    /// The verified email is outside the organization's allowed domains.
    #[error("email domain is not allowed")]
    DomainNotAllowed,
    /// A provider subject or local email is already bound to another identity.
    #[error("OIDC identity conflicts with the admitted member")]
    ProviderIdentityConflict,
    /// The client code, PKCE proof, or refresh credential is invalid, expired, or consumed.
    #[error("authorization grant is invalid or expired")]
    InvalidGrant,
    /// No valid bearer credential resolves to an enabled session.
    #[error("authentication is required")]
    Unauthorized,
    /// The login flow cannot proceed because first-run installation validation failed.
    #[error(transparent)]
    Installation(#[from] InstallationError),
    /// Authentication persistence failed without exposing stored credentials to the client.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl AuthError {
    /// Return the stable public error category without leaking internal diagnostics.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "oidc_not_configured",
            Self::Configuration(_) => "auth_configuration_invalid",
            Self::InvalidRequest(_) => "validation_failed",
            Self::RedirectNotAllowed => "redirect_uri_not_allowed",
            Self::ProviderUnavailable(_) => "oidc_provider_unavailable",
            Self::ProviderCodeExchangeFailed(_) => "oidc_code_exchange_failed",
            Self::ProviderInvalid(_) => "oidc_id_token_invalid",
            Self::LoginTransactionExpired => "login_transaction_expired",
            Self::CorruptLoginTransaction => "login_transaction_corrupt",
            Self::EmailNotVerified => "email_not_verified",
            Self::MemberNotAllowed => "member_not_allowed",
            Self::DomainNotAllowed => "domain_not_allowed",
            Self::ProviderIdentityConflict => "oidc_identity_conflict",
            Self::InvalidGrant => "invalid_grant",
            Self::Unauthorized => "unauthorized",
            Self::Installation(error) => error.code(),
            Self::Sqlx(_) => "internal_error",
        }
    }
}
