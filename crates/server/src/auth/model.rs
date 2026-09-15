use time::Duration;

use super::error::AuthError;

pub(super) const ACCESS_TOKEN_TTL: Duration = Duration::minutes(15);
pub(super) const REFRESH_TOKEN_TTL: Duration = Duration::days(30);
pub(super) const LOGIN_TRANSACTION_TTL: Duration = Duration::minutes(10);
pub(super) const AUTHORIZATION_CODE_TTL: Duration = Duration::minutes(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthPrincipal {
    pub user_id: String,
    pub org_id: String,
    pub session_id: String,
    pub token_id: String,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OidcIdentity {
    pub issuer: String,
    pub subject: String,
    pub email: String,
    pub email_verified: bool,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct ProviderSummary {
    pub(super) issuer: String,
    pub(super) callback_url: String,
}

#[derive(Debug)]
pub(super) struct LoginTransaction {
    pub(super) transaction_id: String,
    pub(super) nonce: String,
    pub(super) provider_pkce_verifier: String,
    pub(super) client_redirect_uri: String,
    pub(super) client_state: Option<String>,
    pub(super) client_code_challenge: Option<String>,
    pub(super) flow: LoginFlow,
    pub(super) setup_session_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LoginFlow {
    ProductLogin,
    InstallationSetup,
}

pub(super) struct OrganizationAdmission {
    pub(super) org_id: String,
    pub(super) allowed_email_domains: Vec<String>,
}

pub(super) fn login_flow(value: &str) -> Result<LoginFlow, AuthError> {
    match value {
        "product_login" => Ok(LoginFlow::ProductLogin),
        "installation_setup" => Ok(LoginFlow::InstallationSetup),
        _ => Err(AuthError::CorruptLoginTransaction),
    }
}

pub(super) fn ensure_member_enabled(status: String) -> Result<(), AuthError> {
    if status == "disabled" {
        Err(AuthError::MemberNotAllowed)
    } else {
        Ok(())
    }
}

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
