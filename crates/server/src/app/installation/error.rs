//! Resource-specific failures for installation resources.

use thiserror::Error;

/// Setup, bootstrap authorization, and installation consistency failures.
#[derive(Debug, Error)]
pub enum InstallationError {
    /// Normal application access requires completed first-owner setup.
    #[error("Server setup must be completed before product login")]
    SetupRequired,
    /// The installation is complete and cannot be initialized again.
    #[error("Server setup is permanently locked")]
    Locked,
    /// No deployment bootstrap secret was configured to authorize setup.
    #[error("Server setup is unavailable because CLUMSIES_SETUP_CODE is not configured")]
    SetupUnavailable,
    /// The presented bootstrap secret does not match the configured digest.
    #[error("setup code is invalid")]
    InvalidSetupCode,
    /// The setup cookie is absent, invalid, expired, or consumed.
    #[error("setup session is invalid or expired")]
    InvalidSession,
    /// The presented setup CSRF proof does not match the active session.
    #[error("setup CSRF token is invalid")]
    CsrfMismatch,
    /// Owner authentication was attempted before organization settings were staged.
    #[error("setup configuration must be saved before owner login")]
    ConfigurationRequired,
    /// The proposed owner's verified email violates the staged domain allowlist.
    #[error("the OIDC owner identity does not match an allowed email domain")]
    OwnerDomainNotAllowed,
    /// The proposed owner has no valid email identity.
    #[error("the OIDC owner identity contains an invalid email address")]
    InvalidOwnerIdentity,
    /// Setup input violates a name, domain, or configuration constraint.
    #[error("invalid setup request: {0}")]
    InvalidRequest(String),
    /// The deployment bootstrap secret is not acceptable for enabling setup.
    #[error("invalid Server setup configuration: {0}")]
    Configuration(String),
    /// Persisted setup-session configuration cannot be decoded.
    #[error("stored setup session is corrupt")]
    CorruptSession,
    /// Persisted singleton installation state is inconsistent or unsupported.
    #[error("stored Server installation state is corrupt")]
    CorruptInstallation,
    /// Setup persistence failed; initialization writes remain transactionally protected.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl InstallationError {
    /// Return the stable public error category without leaking internal diagnostics.
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
