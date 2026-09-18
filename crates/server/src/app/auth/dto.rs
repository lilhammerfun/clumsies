//! Request and response data for auth resources.

use crate::app::organization::dto::{OrgRef, UserRef};
use crate::app::project::dto::ProjectRef;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeResponse {
    pub user: UserRef,
    pub org: OrgRef,
    pub projects: Vec<ProjectRef>,
    pub default_project_id: Option<String>,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Desktop,
    Cli,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct OidcAuthorizationRequest {
    pub client_kind: ClientKind,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub login_hint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct OidcCallbackRequest {
    pub code: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenGrantType {
    AuthorizationCode,
    RefreshToken,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenRequest {
    pub grant_type: TokenGrantType,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
    pub refresh_token: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub user: UserRef,
    pub org: OrgRef,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRevoked {
    pub revoked: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionMode {
    InviteOnly,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SecretSource {
    DeploymentEnvironment,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OidcProviderStatus {
    pub protocol: String,
    pub configured: bool,
    pub issuer: Option<String>,
    pub callback_url: Option<String>,
    pub admission_mode: AdmissionMode,
    pub secret_source: SecretSource,
}
