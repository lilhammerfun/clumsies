//! Internal values and invariants for installation resources.

use super::error::InstallationError;
use crate::app::installation::dto::{
    CreateSetupSessionResponse, InstallationState, ReplaceSetupConfigurationRequest,
    SetupConfiguration,
};
use std::collections::BTreeSet;

/// Secret setup cookie value paired with its public status response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupSessionCredentials {
    /// Public setup-session metadata associated with the issued credentials.
    pub session: CreateSetupSessionResponse,
    /// Secret setup or authorization credential; never log its plaintext value.
    pub token: String,
}

/// Organization, owner, and initial project identities created atomically during setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitializedInstallation {
    /// Organization boundary to which the resource or identity belongs.
    pub org_id: String,
    /// Stable identity of the user represented or targeted by this record.
    pub user_id: String,
    /// Project boundary containing the resource or proposal.
    pub project_id: String,
}

/// Validate and normalize first-run organization and project settings.
///
/// # Errors
/// Rejects invalid organization or project names and malformed admission domains.
pub(super) fn normalize_configuration(
    request: ReplaceSetupConfigurationRequest,
) -> Result<SetupConfiguration, InstallationError> {
    Ok(SetupConfiguration {
        org_name: normalize_name(&request.org_name, "organization")?,
        default_project_name: normalize_name(&request.default_project_name, "project")?,
        allowed_email_domains: normalize_email_domains(request.allowed_email_domains)?,
    })
}

/// Trim a human-readable name and enforce the setup contract's length and nonempty requirements.
///
/// # Errors
/// Rejects empty names and values exceeding the setup contract's size limit.
fn normalize_name(value: &str, field: &str) -> Result<String, InstallationError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 120 || value.chars().any(char::is_control) {
        return Err(InstallationError::InvalidRequest(format!(
            "{field} name must contain between 1 and 120 visible characters"
        )));
    }
    Ok(value.to_owned())
}

/// Normalize an email address and reject malformed identity input.
///
/// # Errors
/// Returns invalid input when the email has no valid local part or domain.
pub(super) fn normalize_email(email: &str) -> Result<String, InstallationError> {
    let email = email.trim().to_ascii_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(InstallationError::InvalidOwnerIdentity);
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') || !valid_domain(domain) {
        return Err(InstallationError::InvalidOwnerIdentity);
    }
    Ok(email)
}

/// Normalize and deduplicate allowed domains while rejecting invalid labels.
///
/// # Errors
/// Returns invalid input for malformed, empty, or unsupported domain entries.
fn normalize_email_domains(domains: Vec<String>) -> Result<Vec<String>, InstallationError> {
    let mut normalized = BTreeSet::new();
    for domain in domains {
        let domain = domain.trim().trim_start_matches('@').to_ascii_lowercase();
        if !valid_domain(&domain) {
            return Err(InstallationError::InvalidRequest(format!(
                "invalid allowed email domain: {domain}"
            )));
        }
        normalized.insert(domain);
    }
    Ok(normalized.into_iter().collect())
}

/// Check DNS-style email-domain labels without performing network resolution.
fn valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// Require the verified email to satisfy the configured organization admission policy.
///
/// # Errors
/// Rejects a verified email outside the configured admission allowlist.
pub(super) fn enforce_email_domain(
    email: &str,
    allowed_domains: &[String],
) -> Result<(), InstallationError> {
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
        Err(InstallationError::OwnerDomainNotAllowed)
    }
}

/// Decode the persisted singleton installation phase.
///
/// # Errors
/// Rejects unsupported persisted values instead of assigning a default state or privilege.
pub(super) fn installation_state(value: &str) -> Result<InstallationState, InstallationError> {
    match value {
        "setup_required" => Ok(InstallationState::SetupRequired),
        "initialized" => Ok(InstallationState::Initialized),
        _ => Err(InstallationError::CorruptInstallation),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_configuration_normalizes_names_and_domains() {
        let configuration = normalize_configuration(ReplaceSetupConfigurationRequest {
            org_name: "  Clumsies Lab  ".to_owned(),
            default_project_name: "  Default  ".to_owned(),
            allowed_email_domains: vec!["@Example.COM".to_owned(), "example.com".to_owned()],
        })
        .unwrap();
        assert_eq!(configuration.org_name, "Clumsies Lab");
        assert_eq!(configuration.default_project_name, "Default");
        assert_eq!(configuration.allowed_email_domains, ["example.com"]);
    }
}
