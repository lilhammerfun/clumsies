//! Internal values and invariants for auth resources.

use super::error::AuthError;
use time::Duration;

/// Lifetime of an issued bearer credential before a refresh is required.
pub(super) const ACCESS_TOKEN_TTL: Duration = Duration::minutes(15);
/// Maximum lifetime of an issued refresh credential before reauthentication.
pub(super) const REFRESH_TOKEN_TTL: Duration = Duration::days(30);
/// Maximum lifetime of provider correlation and PKCE login state.
pub(super) const LOGIN_TRANSACTION_TTL: Duration = Duration::minutes(10);
/// Maximum age of a single-use code returned to the native client.
pub(super) const AUTHORIZATION_CODE_TTL: Duration = Duration::minutes(2);

/// Trusted session identity passed to resource operations for authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthPrincipal {
    /// Stable user identity; it must originate from trusted authentication context for
    /// authorization.
    pub user_id: String,
    /// Organization boundary to which the resource or identity belongs.
    pub org_id: String,
    /// Stable identifier of the authenticated or setup session.
    pub session_id: String,
    /// Stable identifier of the access or refresh credential record.
    pub token_id: String,
    /// Organization-wide privileges; project membership is checked separately.
    pub role: String,
}

impl AuthPrincipal {
    /// Reject principals whose organization role grants no administration privileges.
    ///
    /// # Errors
    /// Returns forbidden unless the principal holds an organization owner or administrator role.
    pub fn require_org_admin(&self) -> Result<(), crate::error::ServerError> {
        if matches!(self.role.as_str(), "owner" | "admin") {
            Ok(())
        } else {
            Err(crate::error::ServerError::Forbidden(
                "organization administrator role required".to_owned(),
            ))
        }
    }
}

/// Verified provider identity used for admission and local account binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OidcIdentity {
    /// Verified or configured OIDC issuer identifying the external identity authority.
    pub issuer: String,
    /// Stable issuer-local identity from a verified OIDC token.
    pub subject: String,
    /// User email used for organization admission and identity display.
    pub email: String,
    /// Whether the identity provider cryptographically attests to the email.
    pub email_verified: bool,
    /// Optional human-readable name supplied by the identity provider or administrator.
    pub display_name: Option<String>,
    /// Optional image URL supplied by the verified identity provider.
    pub avatar_url: Option<String>,
}

/// Non-secret issuer and server callback information exposed to administrators.
#[derive(Clone, Debug)]
pub struct ProviderSummary {
    /// Verified or configured OIDC issuer identifying the external identity authority.
    pub issuer: String,
    /// Server callback URL registered with the identity provider.
    pub callback_url: String,
}

/// One-time correlation and PKCE state connecting the client and provider login flows.
#[derive(Debug)]
pub(super) struct LoginTransaction {
    /// One-time login transaction binding state, nonce, and PKCE proofs.
    pub(super) transaction_id: String,
    /// Unpredictable value binding the provider's ID token to this login attempt.
    pub(super) nonce: String,
    /// Secret proof used only when exchanging the provider authorization code.
    pub(super) provider_pkce_verifier: String,
    /// Validated destination for returning the one-time client authorization code.
    pub(super) client_redirect_uri: String,
    /// Opaque client value echoed to correlate the authorization response.
    pub(super) client_state: Option<String>,
    /// PKCE challenge binding the authorization code to the requesting client.
    pub(super) client_code_challenge: Option<String>,
    /// Whether the login continues installation setup or normal product access.
    pub(super) flow: LoginFlow,
    /// Setup session authorized to continue first-run OIDC initialization.
    pub(super) setup_session_id: Option<String>,
}

/// Distinction between ordinary product login and first-owner setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LoginFlow {
    /// Provider verification will authenticate an admitted organization member.
    ProductLogin,
    /// Provider verification will establish the installation's first owner.
    InstallationSetup,
}

/// Organization identity and email policy applied when admitting an external user.
pub(super) struct OrganizationAdmission {
    /// Organization boundary to which the resource or identity belongs.
    pub(super) org_id: String,
    /// Permitted email domains; an empty collection imposes no domain restriction.
    pub(super) allowed_email_domains: Vec<String>,
}

/// Decode the persisted login branch without accepting unknown flow values.
///
/// # Errors
/// Rejects unsupported persisted login branches as corrupt authentication state.
pub(super) fn login_flow(value: &str) -> Result<LoginFlow, AuthError> {
    match value {
        "product_login" => Ok(LoginFlow::ProductLogin),
        "installation_setup" => Ok(LoginFlow::InstallationSetup),
        _ => Err(AuthError::CorruptLoginTransaction),
    }
}

/// Reject identities whose organization membership is disabled.
///
/// # Errors
/// Rejects disabled membership regardless of whether an otherwise valid external identity exists.
pub(super) fn ensure_member_enabled(status: String) -> Result<(), AuthError> {
    if status == "disabled" {
        Err(AuthError::MemberNotAllowed)
    } else {
        Ok(())
    }
}

/// Require the verified email to satisfy the configured organization admission policy.
///
/// # Errors
/// Rejects a verified email outside the configured admission allowlist.
pub(super) fn enforce_email_domain(
    email: &str,
    allowed_domains: &[String],
) -> Result<(), AuthError> {
    if allowed_domains.is_empty() {
        return Ok(());
    }
    let domain = email.rsplit_once('@').map(|(_, domain)| domain);
    if domain.is_some_and(|domain| {
        allowed_domains
            .iter()
            .any(|allowed| domain.eq_ignore_ascii_case(allowed))
    }) {
        Ok(())
    } else {
        Err(AuthError::DomainNotAllowed)
    }
}

/// Translate an organization role into the public set of permitted capabilities.
pub(crate) fn user_capabilities(role: &str) -> Vec<String> {
    let mut capabilities = vec![
        "memory:read".to_owned(),
        "draft:write".to_owned(),
        "review:write".to_owned(),
        "project:create".to_owned(),
    ];
    if role == "owner" || role == "admin" {
        capabilities.push("review:decide".to_owned());
        capabilities.push("review:merge".to_owned());
        capabilities.push("admin:write".to_owned());
    }
    capabilities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_org_administrators_receive_review_authority_capabilities() {
        let member = user_capabilities("member");
        assert!(!member.iter().any(|value| value == "review:decide"));
        assert!(!member.iter().any(|value| value == "review:merge"));

        let admin = user_capabilities("admin");
        assert!(admin.iter().any(|value| value == "review:decide"));
        assert!(admin.iter().any(|value| value == "review:merge"));
    }
}
