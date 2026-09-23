//! SQL queries and persistence for installation resources.

use super::error::InstallationError;
use super::model::{InitializedInstallation, installation_state};
use crate::app::auth::OidcIdentity;
use crate::app::installation::dto::{InstallationState, SetupConfiguration, SetupSessionStatus};
use crate::identity::{prefixed_id, secret_hash, secret_hash_hex};
use sqlx::{PgPool, Postgres, Row, Transaction};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;

/// Singleton key ensuring only one organization installation exists in this database.
const INSTALLATION_ID: &str = "default";

/// Read installation completion state through the supplied database executor.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn installation_state_for(
    pool: &PgPool,
) -> Result<InstallationState, InstallationError> {
    let state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM server_installations WHERE installation_id = $1",
    )
    .bind(INSTALLATION_ID)
    .fetch_one(pool)
    .await?;
    installation_state(&state)
}

/// Decode the expiry and staged settings of a live first-run setup session.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn active_session_status(
    pool: &PgPool,
    token: &str,
) -> Result<Option<SetupSessionStatus>, InstallationError> {
    let row = sqlx::query(
        "SELECT org_name, default_project_name, allowed_email_domains,
                configuration_saved_at, expires_at
         FROM setup_sessions
         WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(secret_hash_hex(token))
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let configuration = if row
            .try_get::<Option<OffsetDateTime>, _>("configuration_saved_at")?
            .is_some()
        {
            Some(SetupConfiguration {
                org_name: row.try_get("org_name")?,
                default_project_name: row.try_get("default_project_name")?,
                allowed_email_domains: row.try_get("allowed_email_domains")?,
            })
        } else {
            None
        };
        Ok(SetupSessionStatus {
            expires_at: row.try_get("expires_at")?,
            configuration,
        })
    })
    .transpose()
}

/// Persist hashed setup-session credentials and their bounded expiry.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn create_session(
    tx: &mut Transaction<'_, Postgres>,
    session_id: &str,
    token: &str,
    csrf_token: &str,
    expires_at: OffsetDateTime,
) -> Result<(), InstallationError> {
    require_setup_required(tx, false).await?;
    sqlx::query(
        "INSERT INTO setup_sessions (
            session_id, token_hash, csrf_token_hash, expires_at
         ) VALUES ($1, $2, $3, $4)",
    )
    .bind(session_id)
    .bind(secret_hash_hex(token))
    .bind(secret_hash_hex(csrf_token))
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Lock the active setup session and replace its staged organization settings.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn replace_configuration(
    tx: &mut Transaction<'_, Postgres>,
    session_token: &str,
    csrf_token: &str,
    configuration: &SetupConfiguration,
) -> Result<(), InstallationError> {
    require_setup_required(tx, false).await?;
    let session_id = require_active_session(tx, session_token, Some(csrf_token), false).await?;
    sqlx::query(
        "UPDATE setup_sessions
         SET org_name = $2,
             default_project_name = $3,
             allowed_email_domains = $4,
             configuration_saved_at = now(),
             updated_at = now()
         WHERE session_id = $1",
    )
    .bind(session_id)
    .bind(&configuration.org_name)
    .bind(&configuration.default_project_name)
    .bind(&configuration.allowed_email_domains)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Read authorized setup-session state for first-owner provider login.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn authorize_oidc(
    tx: &mut Transaction<'_, Postgres>,
    session_token: &str,
    csrf_token: &str,
) -> Result<String, InstallationError> {
    require_setup_required(tx, false).await?;
    let session_id = require_active_session(tx, session_token, Some(csrf_token), true).await?;
    Ok(session_id)
}

/// Lock a setup session and load the configuration used to initialize its owner.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects invalid or completed setup
/// state.
pub(super) async fn setup_configuration_for_update(
    tx: &mut Transaction<'_, Postgres>,
    session_id: &str,
) -> Result<SetupConfiguration, InstallationError> {
    require_setup_required(tx, true).await?;
    let session = sqlx::query(
        "SELECT org_name, default_project_name, allowed_email_domains
         FROM setup_sessions
         WHERE session_id = $1
           AND consumed_at IS NULL
           AND expires_at > now()
           AND configuration_saved_at IS NOT NULL
         FOR UPDATE",
    )
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(InstallationError::ConfigurationRequired)?;
    Ok(SetupConfiguration {
        org_name: session.try_get("org_name")?,
        default_project_name: session.try_get("default_project_name")?,
        allowed_email_domains: session.try_get("allowed_email_domains")?,
    })
}

/// Lock singleton installation state and persist initial organization, owner, and project
/// records.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
pub(super) async fn initialize_with_oidc(
    tx: &mut Transaction<'_, Postgres>,
    identity: &OidcIdentity,
    configuration: SetupConfiguration,
    owner_email: &str,
    org_id: String,
    user_id: String,
    project_id: String,
) -> Result<InitializedInstallation, InstallationError> {
    sqlx::query(
        "INSERT INTO orgs (org_id, name, allowed_email_domains)
         VALUES ($1, $2, $3)",
    )
    .bind(&org_id)
    .bind(configuration.org_name)
    .bind(configuration.allowed_email_domains)
    .execute(&mut **tx)
    .await?;
    insert_ref(tx, "org", &org_id, None).await?;
    sqlx::query(
        "INSERT INTO users (
            user_id, email, display_name, avatar_url, role, status
         ) VALUES ($1, $2, $3, $4, 'owner', 'active')",
    )
    .bind(&user_id)
    .bind(owner_email)
    .bind(identity.display_name.as_deref())
    .bind(identity.avatar_url.as_deref())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO external_identities (
            external_identity_id, user_id, protocol, issuer, subject, email_at_binding
         ) VALUES ($1, $2, 'oidc', $3, $4, $5)",
    )
    .bind(prefixed_id("idn"))
    .bind(&user_id)
    .bind(&identity.issuer)
    .bind(&identity.subject)
    .bind(&identity.email)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO projects (project_id, org_id, name)
         VALUES ($1, $2, $3)",
    )
    .bind(&project_id)
    .bind(&org_id)
    .bind(configuration.default_project_name)
    .execute(&mut **tx)
    .await?;
    insert_ref(tx, "project", &org_id, Some(&project_id)).await?;
    sqlx::query("INSERT INTO project_org_selection_states (project_id) VALUES ($1)")
        .bind(&project_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO project_members (project_id, user_id, role)
         VALUES ($1, $2, 'owner')",
    )
    .bind(&project_id)
    .bind(&user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE server_installations
         SET state = 'initialized', org_id = $2, initialized_at = now(), updated_at = now()
         WHERE installation_id = $1 AND state = 'setup_required'",
    )
    .bind(INSTALLATION_ID)
    .bind(&org_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE setup_sessions
         SET consumed_at = now(), updated_at = now()
         WHERE consumed_at IS NULL",
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (
            event_id, org_id, actor_user_id, action, target_type, target_id
         ) VALUES ($1, $2, $3, 'installation.completed', 'org', $2)",
    )
    .bind(prefixed_id("audit"))
    .bind(&org_id)
    .bind(&user_id)
    .execute(&mut **tx)
    .await?;
    Ok(InitializedInstallation {
        org_id,
        user_id,
        project_id,
    })
}

/// Reject setup writes once the singleton installation has completed.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects invalid or completed setup
/// state.
async fn require_setup_required(
    tx: &mut Transaction<'_, Postgres>,
    exclusive: bool,
) -> Result<(), InstallationError> {
    let state = if exclusive {
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM server_installations
             WHERE installation_id = $1
             FOR UPDATE",
        )
        .bind(INSTALLATION_ID)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM server_installations
             WHERE installation_id = $1
             FOR SHARE",
        )
        .bind(INSTALLATION_ID)
        .fetch_one(&mut **tx)
        .await?
    };
    if installation_state(&state)? == InstallationState::SetupRequired {
        Ok(())
    } else {
        Err(InstallationError::Locked)
    }
}

/// Require an unexpired setup session whose CSRF proof matches.
///
/// Uses the caller's transaction without committing it.
///
/// Database locks acquired here remain held until the caller ends the transaction.
///
/// # Errors
/// Propagates database access and row-decoding failures and rejects invalid or completed setup
/// state.
async fn require_active_session(
    tx: &mut Transaction<'_, Postgres>,
    session_token: &str,
    csrf_token: Option<&str>,
    require_configuration: bool,
) -> Result<String, InstallationError> {
    let row = sqlx::query(
        "SELECT session_id, csrf_token_hash, configuration_saved_at
         FROM setup_sessions
         WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now()
         FOR UPDATE",
    )
    .bind(secret_hash_hex(session_token))
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(InstallationError::InvalidSession)?;
    if let Some(csrf_token) = csrf_token {
        let expected_hash = hex::decode(row.try_get::<String, _>("csrf_token_hash")?)
            .map_err(|_| InstallationError::CorruptSession)?;
        let actual_hash = secret_hash(csrf_token);
        if expected_hash.as_slice().ct_eq(&actual_hash).unwrap_u8() != 1 {
            return Err(InstallationError::CsrfMismatch);
        }
    }
    if require_configuration
        && row
            .try_get::<Option<OffsetDateTime>, _>("configuration_saved_at")?
            .is_none()
    {
        return Err(InstallationError::ConfigurationRequired);
    }
    Ok(row.try_get("session_id")?)
}

/// Create the initial empty snapshot reference during installation initialization.
///
/// Uses the caller's transaction without committing it.
///
/// # Errors
/// Propagates database access and row-decoding failures.
async fn insert_ref(
    tx: &mut Transaction<'_, Postgres>,
    scope: &str,
    org_id: &str,
    project_id: Option<&str>,
) -> Result<(), InstallationError> {
    sqlx::query(
        "INSERT INTO refs (ref_id, ref_name, scope, org_id, project_id)
         VALUES ($1, 'refs/heads/main', $2, $3, $4)",
    )
    .bind(prefixed_id("ref"))
    .bind(scope)
    .bind(org_id)
    .bind(project_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
