//! Resource-specific failures for auth resources.

use crate::app::installation::InstallationError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("OIDC is not configured")]
    NotConfigured,
    #[error("invalid authentication configuration: {0}")]
    Configuration(String),
    #[error("invalid authentication request: {0}")]
    InvalidRequest(String),
    #[error("redirect URI is not allowed")]
    RedirectNotAllowed,
    #[error("OIDC provider is unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("OIDC authorization code exchange failed: {0}")]
    ProviderCodeExchangeFailed(String),
    #[error("OIDC identity response is invalid: {0}")]
    ProviderInvalid(String),
    #[error("OIDC login transaction is expired or already consumed")]
    LoginTransactionExpired,
    #[error("stored OIDC login transaction is corrupt")]
    CorruptLoginTransaction,
    #[error("OIDC email is not verified")]
    EmailNotVerified,
    #[error("member is not admitted to this Server")]
    MemberNotAllowed,
    #[error("email domain is not allowed")]
    DomainNotAllowed,
    #[error("OIDC identity conflicts with the admitted member")]
    ProviderIdentityConflict,
    #[error("authorization grant is invalid or expired")]
    InvalidGrant,
    #[error("authentication is required")]
    Unauthorized,
    #[error(transparent)]
    Installation(#[from] InstallationError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl AuthError {
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
