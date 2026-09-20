//! Request and response data for auth resources.

use crate::app::organization::dto::{OrgRef, UserRef};
use crate::app::project::dto::ProjectRef;
use serde::{Deserialize, Serialize};

/// Authenticated identity, organization, accessible projects, and granted capabilities.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeResponse {
    /// Public identity of the authenticated user.
    pub user: UserRef,
    /// Public organization identity associated with this response.
    pub org: OrgRef,
    /// Projects for which the authenticated user has explicit membership.
    pub projects: Vec<ProjectRef>,
    /// Initial project offered to this identity, when it has accessible projects.
    pub default_project_id: Option<String>,
    /// Operations granted to the authenticated identity by its organization role.
    pub capabilities: Vec<String>,
}

/// Native application category initiating the browser login flow.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// Desktop client using the native browser login flow.
    Desktop,
    /// Command-line client using the native browser login flow.
    Cli,
}

/// Client callback, state, and PKCE inputs for starting browser authentication.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct OidcAuthorizationRequest {
    /// Native client category that initiated authentication.
    pub client_kind: ClientKind,
    /// Client callback URL checked against the configured allowlist.
    pub redirect_uri: Option<String>,
    /// S256 PKCE challenge supplied by the client.
    pub code_challenge: Option<String>,
    /// PKCE transform name; the server accepts S256.
    pub code_challenge_method: Option<String>,
    /// Opaque native-client correlation value echoed in its authorization response.
    pub state: Option<String>,
    /// Optional email hint sent to the provider during installation setup.
    pub login_hint: Option<String>,
}

/// Provider callback carrying a one-time code or an authorization failure.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct OidcCallbackRequest {
    /// One-time provider authorization code redeemed by the server.
    pub code: Option<String>,
    /// Provider correlation value bound to the persisted login attempt.
    pub state: String,
    /// Stable error information returned at the protocol boundary.
    pub error: Option<String>,
    /// Provider-supplied explanation of an unsuccessful authorization response.
    pub error_description: Option<String>,
}

/// Supported exchange at the application's token endpoint.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenGrantType {
    /// Redeem a one-time client code with its original PKCE verifier.
    AuthorizationCode,
    /// Rotate a refresh credential and issue a new access credential.
    RefreshToken,
}

/// Authorization-code redemption or refresh rotation inputs for the token endpoint.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenRequest {
    /// Credential exchange being requested at the token endpoint.
    pub grant_type: TokenGrantType,
    /// One-time client authorization code, required for the authorization-code grant.
    pub code: Option<String>,
    /// Client callback URL checked against the configured allowlist.
    pub redirect_uri: Option<String>,
    /// Secret PKCE proof presented when redeeming an authorization code.
    pub code_verifier: Option<String>,
    /// One-time rotation credential used to obtain a fresh token pair.
    pub refresh_token: Option<String>,
}

/// New bearer and refresh credentials with their validity duration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenResponse {
    /// Bearer credential authorizing API requests; never include it in logs.
    pub access_token: String,
    /// One-time rotation credential used to obtain a fresh token pair.
    pub refresh_token: String,
    /// Authorization scheme to use with the issued access token.
    pub token_type: String,
    /// Credential lifetime in seconds from issuance.
    pub expires_in: i64,
    /// Public identity of the authenticated user.
    pub user: UserRef,
    /// Public organization identity associated with this response.
    pub org: OrgRef,
    /// Operations granted to the authenticated identity by its organization role.
    pub capabilities: Vec<String>,
}

/// Confirmation that the authenticated session's credentials were invalidated.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRevoked {
    /// Whether the requested credentials or session have been invalidated.
    pub revoked: bool,
}

/// Policy governing admission of externally authenticated users.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionMode {
    /// Only previously invited organization members may sign in.
    InviteOnly,
}

/// Deployment location from which provider credentials are supplied.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SecretSource {
    /// Provider credentials are supplied by the deployment configuration.
    DeploymentEnvironment,
}

/// Non-secret administrative view of identity-provider deployment settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OidcProviderStatus {
    /// Identity protocol used by the configured provider.
    pub protocol: String,
    /// Whether the required deployment settings have been provided.
    pub configured: bool,
    /// Verified or configured OIDC issuer identifying the external identity authority.
    pub issuer: Option<String>,
    /// Server callback URL registered with the identity provider.
    pub callback_url: Option<String>,
    /// Policy controlling whether an external identity may join the organization.
    pub admission_mode: AdmissionMode,
    /// Deployment mechanism supplying the identity-provider credentials.
    pub secret_source: SecretSource,
}
