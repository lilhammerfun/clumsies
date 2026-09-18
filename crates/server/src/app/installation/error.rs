//! Resource-specific failures for installation resources.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum InstallationError {
    #[error("Server setup must be completed before product login")]
    SetupRequired,
    #[error("Server setup is permanently locked")]
    Locked,
    #[error("Server setup is unavailable because CLUMSIES_SETUP_CODE is not configured")]
    SetupUnavailable,
    #[error("setup code is invalid")]
    InvalidSetupCode,
    #[error("setup session is invalid or expired")]
    InvalidSession,
    #[error("setup CSRF token is invalid")]
    CsrfMismatch,
    #[error("setup configuration must be saved before owner login")]
    ConfigurationRequired,
    #[error("the OIDC owner identity does not match an allowed email domain")]
    OwnerDomainNotAllowed,
    #[error("the OIDC owner identity contains an invalid email address")]
    InvalidOwnerIdentity,
    #[error("invalid setup request: {0}")]
    InvalidRequest(String),
    #[error("invalid Server setup configuration: {0}")]
    Configuration(String),
    #[error("stored setup session is corrupt")]
    CorruptSession,
    #[error("stored Server installation state is corrupt")]
    CorruptInstallation,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl InstallationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SetupRequired => "setup_required",
            Self::Locked => "setup_locked",
            Self::SetupUnavailable => "setup_unavailable",
            Self::InvalidSetupCode => "setup_code_invalid",
            Self::InvalidSession => "setup_session_invalid",
            Self::CsrfMismatch => "setup_csrf_invalid",
            Self::ConfigurationRequired => "setup_configuration_required",
            Self::OwnerDomainNotAllowed => "setup_owner_domain_not_allowed",
            Self::InvalidOwnerIdentity => "setup_owner_identity_invalid",
            Self::InvalidRequest(_) => "validation_failed",
            Self::Configuration(_) => "setup_configuration_invalid",
            Self::CorruptSession | Self::CorruptInstallation | Self::Sqlx(_) => "internal_error",
        }
    }
}
