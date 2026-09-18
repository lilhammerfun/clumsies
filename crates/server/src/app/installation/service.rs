//! Application operations and transaction coordination for installation resources.

use super::error::InstallationError;
use super::model::{
    InitializedInstallation, SetupSessionCredentials, enforce_email_domain,
    normalize_configuration, normalize_email,
};
use super::repository;
use crate::app::auth::OidcIdentity;
use crate::app::installation::dto::{
    CreateSetupSessionResponse, InstallationState, ReplaceSetupConfigurationRequest,
    SetupConfiguration, SetupStatus,
};
use crate::identity::{prefixed_id, random_token, secret_hash};
use sqlx::{PgPool, Postgres, Transaction};
use std::env;
use subtle::ConstantTimeEq;
use time::{Duration, OffsetDateTime};

const SETUP_SESSION_TTL: Duration = Duration::minutes(15);
const MINIMUM_SETUP_CODE_LENGTH: usize = 32;
const LOCAL_SETUP_COOKIE_NAME: &str = "clumsies_setup_session";
const SECURE_SETUP_COOKIE_NAME: &str = "__Host-clumsies_setup_session";

#[derive(Clone)]
pub struct InstallationService {
    pool: PgPool,
    setup_code_hash: Option<[u8; 32]>,
    secure_cookie: bool,
}

impl InstallationService {
    pub fn from_env(pool: PgPool, secure_cookie: bool) -> Result<Self, InstallationError> {
        let setup_code = optional_env("CLUMSIES_SETUP_CODE");
        Self::new(pool, setup_code.as_deref(), secure_cookie)
    }

    pub fn new(
        pool: PgPool,
        setup_code: Option<&str>,
        secure_cookie: bool,
    ) -> Result<Self, InstallationError> {
        let setup_code_hash = setup_code
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                if value.len() < MINIMUM_SETUP_CODE_LENGTH {
                    return Err(InstallationError::Configuration(format!(
                        "CLUMSIES_SETUP_CODE must contain at least {MINIMUM_SETUP_CODE_LENGTH} characters"
                    )));
                }
                Ok(secret_hash(value))
            })
            .transpose()?;
        Ok(Self {
            pool,
            setup_code_hash,
            secure_cookie,
        })
    }

    pub fn setup_code_configured(&self) -> bool {
        self.setup_code_hash.is_some()
    }

    pub fn cookie_name(&self) -> &'static str {
        if self.secure_cookie {
            SECURE_SETUP_COOKIE_NAME
        } else {
            LOCAL_SETUP_COOKIE_NAME
        }
    }

    pub fn cookie_secure(&self) -> bool {
        self.secure_cookie
    }

    pub async fn require_initialized(&self) -> Result<(), InstallationError> {
        if repository::installation_state_for(&self.pool).await? == InstallationState::Initialized {
            Ok(())
        } else {
            Err(InstallationError::SetupRequired)
        }
    }

    pub async fn status(
        &self,
        session_token: Option<&str>,
        oidc_configured: bool,
    ) -> Result<SetupStatus, InstallationError> {
        let state = repository::installation_state_for(&self.pool).await?;
        let session = if state == InstallationState::SetupRequired {
            match session_token {
                Some(token) => repository::active_session_status(&self.pool, token).await?,
                None => None,
            }
        } else {
            None
        };
        Ok(SetupStatus {
            state,
            setup_code_configured: self.setup_code_configured(),
            oidc_configured,
            session,
        })
    }

    pub async fn create_session(
        &self,
        setup_code: &str,
    ) -> Result<SetupSessionCredentials, InstallationError> {
        let expected_hash = self
            .setup_code_hash
            .as_ref()
            .ok_or(InstallationError::SetupUnavailable)?;
        let actual_hash = secret_hash(setup_code.trim());
        if expected_hash.ct_eq(&actual_hash).unwrap_u8() != 1 {
            return Err(InstallationError::InvalidSetupCode);
        }

        let session_id = prefixed_id("setup");
        let token = random_token();
        let csrf_token = random_token();
        let expires_at = OffsetDateTime::now_utc() + SETUP_SESSION_TTL;
        let mut tx = self.pool.begin().await?;
        repository::create_session(&mut tx, &session_id, &token, &csrf_token, expires_at).await?;
        tx.commit().await?;
        Ok(SetupSessionCredentials {
            session: CreateSetupSessionResponse {
                expires_at,
                csrf_token,
            },
            token,
        })
    }

    pub async fn replace_configuration(
        &self,
        session_token: &str,
        csrf_token: &str,
        request: ReplaceSetupConfigurationRequest,
    ) -> Result<SetupConfiguration, InstallationError> {
        let configuration = normalize_configuration(request)?;
        let mut tx = self.pool.begin().await?;
        repository::replace_configuration(&mut tx, session_token, csrf_token, &configuration)
            .await?;
        tx.commit().await?;
        Ok(configuration)
    }

    pub async fn authorize_oidc(
        &self,
        session_token: &str,
        csrf_token: &str,
    ) -> Result<String, InstallationError> {
        let mut tx = self.pool.begin().await?;
        let session_id = repository::authorize_oidc(&mut tx, session_token, csrf_token).await?;
        tx.commit().await?;
        Ok(session_id)
    }

    pub async fn initialize_with_oidc(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        session_id: &str,
        identity: &OidcIdentity,
    ) -> Result<InitializedInstallation, InstallationError> {
        let configuration = repository::setup_configuration_for_update(tx, session_id).await?;
        enforce_email_domain(&identity.email, &configuration.allowed_email_domains)?;
        let owner_email = normalize_email(&identity.email)?;
        let org_id = prefixed_id("org");
        let user_id = prefixed_id("usr");
        let project_id = prefixed_id("prj");
        repository::initialize_with_oidc(
            tx,
            identity,
            configuration,
            &owner_email,
            org_id,
            user_id,
            project_id,
        )
        .await
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn short_setup_code_is_rejected() {
        let pool = PgPool::connect_lazy("postgres://clumsies@127.0.0.1/clumsies").unwrap();
        let result = InstallationService::new(pool, Some("too-short"), false);
        assert!(matches!(result, Err(InstallationError::Configuration(_))));
    }
}
