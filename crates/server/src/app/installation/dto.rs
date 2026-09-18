//! Request and response data for installation resources.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallationState {
    SetupRequired,
    Initialized,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupConfiguration {
    pub org_name: String,
    pub default_project_name: String,
    pub allowed_email_domains: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupSessionStatus {
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub configuration: Option<SetupConfiguration>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupStatus {
    pub state: InstallationState,
    pub setup_code_configured: bool,
    pub oidc_configured: bool,
    pub session: Option<SetupSessionStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSetupSessionRequest {
    pub setup_code: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSetupSessionResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub csrf_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplaceSetupConfigurationRequest {
    pub org_name: String,
    pub default_project_name: String,
    pub allowed_email_domains: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupOidcAuthorizationRequest {
    pub redirect_uri: String,
    pub state: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupOidcAuthorization {
    pub authorization_url: String,
}
