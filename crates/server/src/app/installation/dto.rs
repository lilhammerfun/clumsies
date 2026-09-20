//! Request and response data for installation resources.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Whether first-run owner initialization has completed.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallationState {
    /// First-owner setup must complete before ordinary application access.
    SetupRequired,
    /// The organization and first owner have been established.
    Initialized,
}

/// Normalized organization settings held by a first-run setup session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupConfiguration {
    /// Organization name to establish during first-run setup.
    pub org_name: String,
    /// Name of the project created during first-run initialization.
    pub default_project_name: String,
    /// Permitted email domains; an empty collection imposes no domain restriction.
    pub allowed_email_domains: Vec<String>,
}

/// Expiration, CSRF proof, and staged configuration of a live setup session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupSessionStatus {
    /// UTC deadline after which this credential or session is invalid.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    /// First-run organization settings awaiting owner initialization.
    pub configuration: Option<SetupConfiguration>,
}

/// Installation readiness and available setup or login capabilities.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupStatus {
    /// Whether first-owner setup has completed.
    pub state: InstallationState,
    /// Whether the deployment has supplied a usable bootstrap secret.
    pub setup_code_configured: bool,
    /// Whether normal OIDC login can be offered by this deployment.
    pub oidc_configured: bool,
    /// Public setup-session metadata associated with the issued credentials.
    pub session: Option<SetupSessionStatus>,
}

/// Bootstrap secret used to establish a first-run setup session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSetupSessionRequest {
    /// Deployment-provided bootstrap secret; never expose it in responses or logs.
    pub setup_code: String,
}

/// Public setup-session state returned alongside its secure cookie.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSetupSessionResponse {
    /// UTC deadline after which this credential or session is invalid.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    /// Unpredictable proof binding state-changing setup requests to their session.
    pub csrf_token: String,
}

/// Organization settings staged before the first owner completes OIDC login.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplaceSetupConfigurationRequest {
    /// Organization name to establish during first-run setup.
    pub org_name: String,
    /// Name of the project created during first-run initialization.
    pub default_project_name: String,
    /// Permitted email domains; an empty collection imposes no domain restriction.
    pub allowed_email_domains: Vec<String>,
}

/// Setup-session CSRF proof required before starting owner authentication.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupOidcAuthorizationRequest {
    /// Client callback URL checked against the configured allowlist.
    pub redirect_uri: String,
    /// Current protocol or resource state represented by this record.
    pub state: String,
    /// S256 PKCE challenge supplied by the client.
    pub code_challenge: String,
    /// PKCE transform name; the server accepts S256.
    pub code_challenge_method: String,
}

/// Parameters linking browser authorization to a live setup session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupOidcAuthorization {
    /// Identity-provider URL to open in the system browser.
    pub authorization_url: String,
}
