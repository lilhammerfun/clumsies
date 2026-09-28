//! SQL persistence for local credentials and purpose-bound account actions.

use super::{AuthError, AuthPrincipal, dto};
use crate::identity::{prefixed_id, random_token, secret_hash_hex};
use sqlx::{PgExecutor, Postgres, Transaction};
use time::OffsetDateTime;

/// Lock an enabled member before credential reads or writes.
///
/// # Errors
/// Rejects missing or inactive accounts and database failures.
pub(super) async fn lock_active_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
) -> Result<String, AuthError> {
    sqlx::query_scalar("SELECT role FROM users WHERE user_id = $1 AND status = 'active' FOR UPDATE")
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AuthError::Unauthorized)
}

/// Store a password without ever returning its hash through public account DTOs.
///
/// # Errors
/// Propagates database failures within the caller-owned transaction.
pub(super) async fn save_password(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
    hash: &str,
) -> Result<(), AuthError> {
    sqlx::query("INSERT INTO password_credentials(user_id, password_hash) VALUES($1,$2) ON CONFLICT(user_id) DO UPDATE SET password_hash = excluded.password_hash, updated_at = now()")
        .bind(user_id).bind(hash).execute(&mut **tx).await?;
    Ok(())
}

/// Revoke old sessions and outstanding actions before issuing a replacement session.
///
/// # Errors
/// Propagates database failures within the caller-owned transaction.
pub(super) async fn revoke_credentials(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
) -> Result<(), AuthError> {
    sqlx::query(
        "UPDATE auth_sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE access_tokens SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE action_tokens SET revoked_at = now() WHERE user_id = $1 AND consumed_at IS NULL AND revoked_at IS NULL").bind(user_id).execute(&mut **tx).await?;
    Ok(())
}

/// Issue a replacement secret while revoking earlier credentials of the same purpose.
///
/// # Errors
/// Propagates persistence failures; the caller must hold the target user row lock.
pub(super) async fn issue_action(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
    purpose: &str,
    actor: Option<&str>,
) -> Result<dto::ActionTokenResponse, AuthError> {
    sqlx::query("UPDATE action_tokens SET revoked_at = now() WHERE user_id = $1 AND purpose = $2 AND consumed_at IS NULL AND revoked_at IS NULL")
        .bind(user_id).bind(purpose).execute(&mut **tx).await?;
    let token = random_token();
    let token_id = prefixed_id("act");
    let expires_at = OffsetDateTime::now_utc()
        + if purpose == "invitation" {
            time::Duration::days(7)
        } else {
            time::Duration::minutes(30)
        };
    sqlx::query("INSERT INTO action_tokens(token_id,user_id,purpose,token_hash,created_by,expires_at) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(&token_id).bind(user_id).bind(purpose).bind(secret_hash_hex(&token)).bind(actor).bind(expires_at).execute(&mut **tx).await?;
    Ok(dto::ActionTokenResponse {
        token_id,
        user_id: user_id.into(),
        token,
        expires_at,
    })
}

/// Map a concurrent username uniqueness conflict to a safe, actionable validation error.
fn username_conflict(error: sqlx::Error) -> AuthError {
    if error
        .as_database_error()
        .is_some_and(|e| e.constraint() == Some("users_username_unique"))
    {
        AuthError::InvalidRequest("username is already in use".into())
    } else {
        AuthError::Sqlx(error)
    }
}

/// Recheck initiating-session validity after acquiring the account lock.
///
/// # Errors
/// Rejects revocation during credential verification.
pub(super) async fn require_session(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AuthPrincipal,
) -> Result<(), AuthError> {
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE session_id = $1 AND user_id = $2 AND revoked_at IS NULL)")
        .bind(&actor.session_id).bind(&actor.user_id).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(AuthError::Unauthorized);
    }
    Ok(())
}

/// Read an active local credential by normalized username.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn password_account<'e>(
    executor: impl PgExecutor<'e>,
    username: &str,
) -> Result<Option<sqlx::postgres::PgRow>, AuthError> {
    sqlx::query("SELECT u.user_id, p.password_hash FROM users u JOIN password_credentials p USING(user_id) WHERE u.username = $1 AND u.status = 'active'").bind(username).fetch_optional(executor).await.map_err(AuthError::from)
}

/// Read the current local password hash without exposing it in public identity responses.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn password_hash<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
) -> Result<Option<String>, AuthError> {
    sqlx::query_scalar("SELECT password_hash FROM password_credentials WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(executor)
        .await
        .map_err(AuthError::from)
}

/// Insert a pending account with administrator-assigned privileges.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn insert_invited_user<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
    role: &str,
) -> Result<sqlx::postgres::PgQueryResult, AuthError> {
    sqlx::query("INSERT INTO users(user_id, role, status) VALUES($1, $2, 'invited')")
        .bind(user_id)
        .bind(role)
        .execute(executor)
        .await
        .map_err(AuthError::from)
}

/// Serialize administrator credential operations within the organization.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn lock_organization<'e>(
    executor: impl PgExecutor<'e>,
    org_id: &str,
) -> Result<String, AuthError> {
    sqlx::query_scalar("SELECT org_id FROM orgs WHERE org_id = $1 FOR UPDATE")
        .bind(org_id)
        .fetch_one(executor)
        .await
        .map_err(AuthError::from)
}

/// Lock target membership state before issuing credentials.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn lock_member<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
) -> Result<Option<sqlx::postgres::PgRow>, AuthError> {
    sqlx::query("SELECT role, status FROM users WHERE user_id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_optional(executor)
        .await
        .map_err(AuthError::from)
}

/// Report whether an account has local password credentials.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn has_password<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
) -> Result<bool, AuthError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM password_credentials WHERE user_id = $1)")
        .bind(user_id)
        .fetch_one(executor)
        .await
        .map_err(AuthError::from)
}

/// Revoke an action only for its issuer or an organization owner.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn revoke_action<'e>(
    executor: impl PgExecutor<'e>,
    token_id: &str,
    actor_id: &str,
    role: &str,
) -> Result<sqlx::postgres::PgQueryResult, AuthError> {
    sqlx::query("UPDATE action_tokens SET revoked_at = now() WHERE token_id = $1 AND consumed_at IS NULL AND (created_by = $2 OR $3 = 'owner')").bind(token_id).bind(actor_id).bind(role).execute(executor).await.map_err(AuthError::from)
}

/// Resolve an unexpired, unconsumed credential of the expected purpose.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn action_user<'e>(
    executor: impl PgExecutor<'e>,
    token_hash: &str,
    purpose: &str,
) -> Result<Option<String>, AuthError> {
    sqlx::query_scalar("SELECT user_id FROM action_tokens WHERE token_hash = $1 AND purpose = $2 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > now()").bind(token_hash).bind(purpose).fetch_optional(executor).await.map_err(AuthError::from)
}

/// Lock account lifecycle state before activation or recovery.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn lock_status<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
) -> Result<String, AuthError> {
    sqlx::query_scalar("SELECT status FROM users WHERE user_id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_one(executor)
        .await
        .map_err(AuthError::from)
}

/// Atomically consume a live credential of the expected purpose.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn consume_action<'e>(
    executor: impl PgExecutor<'e>,
    token_hash: &str,
    purpose: &str,
) -> Result<sqlx::postgres::PgQueryResult, AuthError> {
    sqlx::query("UPDATE action_tokens SET consumed_at = now() WHERE token_hash = $1 AND purpose = $2 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > now()").bind(token_hash).bind(purpose).execute(executor).await.map_err(AuthError::from)
}

/// Activate a pending member with a unique local username.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn activate_local_user<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
    username: &str,
) -> Result<sqlx::postgres::PgQueryResult, AuthError> {
    sqlx::query("UPDATE users SET username = $2, status = 'active', revision = revision + 1, updated_at = now() WHERE user_id = $1").bind(user_id).bind(username).execute(executor).await.map_err(username_conflict)
}

/// Read public credential availability and provider email.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn account_credentials<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
) -> Result<sqlx::postgres::PgRow, AuthError> {
    sqlx::query("SELECT username, EXISTS(SELECT 1 FROM password_credentials p WHERE p.user_id = u.user_id) AS password_set, (SELECT email_at_binding FROM external_identities i WHERE i.user_id = u.user_id ORDER BY created_at LIMIT 1) AS oidc_email FROM users u WHERE user_id = $1").bind(user_id).fetch_one(executor).await.map_err(AuthError::from)
}

/// Require a recent live session on an externally bound account.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn recent_external_session<'e>(
    executor: impl PgExecutor<'e>,
    session_id: &str,
    user_id: &str,
) -> Result<bool, AuthError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions s WHERE session_id = $1 AND user_id = $2 AND revoked_at IS NULL AND created_at > now() - interval '5 minutes' AND EXISTS(SELECT 1 FROM external_identities i WHERE i.user_id = s.user_id))").bind(session_id).bind(user_id).fetch_one(executor).await.map_err(AuthError::from)
}

/// Read the optional local login identifier.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn username<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
) -> Result<Option<String>, AuthError> {
    sqlx::query_scalar("SELECT username FROM users WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(executor)
        .await
        .map_err(AuthError::from)
}

/// Assign a unique username to an existing account.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn set_username<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
    username: &str,
) -> Result<sqlx::postgres::PgQueryResult, AuthError> {
    sqlx::query("UPDATE users SET username = $2, revision = revision + 1, updated_at = now() WHERE user_id = $1").bind(user_id).bind(username).execute(executor).await.map_err(username_conflict)
}

/// Bind provider correlation to the initiating authenticated session.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn bind_transaction<'e>(
    executor: impl PgExecutor<'e>,
    state_hash: &str,
    session_id: &str,
) -> Result<sqlx::postgres::PgQueryResult, AuthError> {
    sqlx::query("UPDATE oidc_login_transactions SET binding_session_id = $2 WHERE provider_state_hash = $1 AND consumed_at IS NULL").bind(state_hash).bind(session_id).execute(executor).await.map_err(AuthError::from)
}

/// Assign the first local owner username inside the setup transaction.
///
/// # Errors
/// Propagates database access and decoding failures.
pub(super) async fn initialize_username<'e>(
    executor: impl PgExecutor<'e>,
    user_id: &str,
    username: &str,
) -> Result<sqlx::postgres::PgQueryResult, AuthError> {
    sqlx::query("UPDATE users SET username = $2 WHERE user_id = $1")
        .bind(user_id)
        .bind(username)
        .execute(executor)
        .await
        .map_err(username_conflict)
}
