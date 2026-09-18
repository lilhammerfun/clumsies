//! SQL queries and persistence for auth resources.

use super::OidcIdentity;
use super::error::AuthError;
use super::model::{
    ACCESS_TOKEN_TTL, AuthPrincipal, LoginTransaction, OrganizationAdmission, REFRESH_TOKEN_TTL,
    ensure_member_enabled, login_flow, user_capabilities,
};
use crate::app::auth::dto::{SessionRevoked, TokenResponse};
use crate::app::organization::dto::{OrgRef, UserRef};
use crate::identity::{prefixed_id, random_token, secret_hash_hex};
use sqlx::{PgPool, Postgres, Row, Transaction};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;

pub(super) struct ProductLoginTransaction<'a> {
    pub(super) provider_state: &'a str,
    pub(super) nonce: &'a str,
    pub(super) provider_pkce_verifier: &'a str,
    pub(super) client_kind: &'a str,
    pub(super) client_redirect_uri: &'a str,
    pub(super) client_state: Option<&'a str>,
    pub(super) client_code_challenge: &'a str,
    pub(super) expires_at: OffsetDateTime,
}

pub(super) struct SetupLoginTransaction<'a> {
    pub(super) provider_state: &'a str,
    pub(super) nonce: &'a str,
    pub(super) provider_pkce_verifier: &'a str,
    pub(super) client_redirect_uri: &'a str,
    pub(super) client_state: &'a str,
    pub(super) client_code_challenge: &'a str,
    pub(super) expires_at: OffsetDateTime,
    pub(super) setup_session_id: &'a str,
}

pub(super) async fn insert_product_login_transaction(
    pool: &PgPool,
    transaction: ProductLoginTransaction<'_>,
) -> Result<(), AuthError> {
    sqlx::query(
        "INSERT INTO oidc_login_transactions (
            transaction_id, provider_state_hash, nonce, provider_pkce_verifier,
            client_kind, client_redirect_uri, client_state, client_code_challenge,
            return_to, expires_at, flow
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9, 'product_login')",
    )
    .bind(prefixed_id("login"))
    .bind(secret_hash_hex(transaction.provider_state))
    .bind(transaction.nonce)
    .bind(transaction.provider_pkce_verifier)
    .bind(transaction.client_kind)
    .bind(transaction.client_redirect_uri)
    .bind(transaction.client_state)
    .bind(transaction.client_code_challenge)
    .bind(transaction.expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn insert_setup_login_transaction(
    pool: &PgPool,
    transaction: SetupLoginTransaction<'_>,
) -> Result<(), AuthError> {
    sqlx::query(
        "INSERT INTO oidc_login_transactions (
            transaction_id, provider_state_hash, nonce, provider_pkce_verifier,
            client_kind, client_redirect_uri, client_state, client_code_challenge,
            return_to, expires_at, flow, setup_session_id
         ) VALUES (
            $1, $2, $3, $4, 'desktop', $5, $6, $7,
            NULL, $8, 'installation_setup', $9
         )",
    )
    .bind(prefixed_id("login"))
    .bind(secret_hash_hex(transaction.provider_state))
    .bind(transaction.nonce)
    .bind(transaction.provider_pkce_verifier)
    .bind(transaction.client_redirect_uri)
    .bind(transaction.client_state)
    .bind(transaction.client_code_challenge)
    .bind(transaction.expires_at)
    .bind(transaction.setup_session_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn login_transaction(
    pool: &PgPool,
    provider_state: &str,
) -> Result<LoginTransaction, AuthError> {
    let row = sqlx::query(
        "SELECT transaction_id, nonce, provider_pkce_verifier, client_redirect_uri,
                client_state, client_code_challenge, flow, setup_session_id
         FROM oidc_login_transactions
         WHERE provider_state_hash = $1 AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(secret_hash_hex(provider_state))
    .fetch_optional(pool)
    .await?
    .ok_or(AuthError::LoginTransactionExpired)?;
    Ok(LoginTransaction {
        transaction_id: row.try_get("transaction_id")?,
        nonce: row.try_get("nonce")?,
        provider_pkce_verifier: row.try_get("provider_pkce_verifier")?,
        client_redirect_uri: row.try_get("client_redirect_uri")?,
        client_state: row.try_get("client_state")?,
        client_code_challenge: row.try_get("client_code_challenge")?,
        flow: login_flow(row.try_get::<String, _>("flow")?.as_str())?,
        setup_session_id: row.try_get("setup_session_id")?,
    })
}

pub(super) async fn consume_login_transaction(
    pool: &PgPool,
    transaction_id: &str,
) -> Result<(), AuthError> {
    let result = sqlx::query(
        "UPDATE oidc_login_transactions SET consumed_at = now()
         WHERE transaction_id = $1 AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(transaction_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AuthError::LoginTransactionExpired)
    }
}

pub(super) async fn consume_login_transaction_in(
    tx: &mut Transaction<'_, Postgres>,
    transaction_id: &str,
) -> Result<bool, AuthError> {
    let result = sqlx::query(
        "UPDATE oidc_login_transactions
         SET consumed_at = now()
         WHERE transaction_id = $1 AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(transaction_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub(super) async fn organization_admission(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<OrganizationAdmission, AuthError> {
    let row =
        sqlx::query("SELECT org_id, allowed_email_domains FROM orgs ORDER BY created_at LIMIT 1")
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(AuthError::MemberNotAllowed)?;
    Ok(OrganizationAdmission {
        org_id: row.try_get("org_id")?,
        allowed_email_domains: row.try_get("allowed_email_domains")?,
    })
}

pub(super) async fn insert_authorization_code(
    tx: &mut Transaction<'_, Postgres>,
    authorization_code: &str,
    user_id: &str,
    org_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    expires_at: OffsetDateTime,
) -> Result<(), AuthError> {
    sqlx::query(
        "INSERT INTO authorization_codes (
            code_id, code_hash, user_id, org_id, redirect_uri, code_challenge, expires_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(prefixed_id("code"))
    .bind(secret_hash_hex(authorization_code))
    .bind(user_id)
    .bind(org_id)
    .bind(redirect_uri)
    .bind(code_challenge)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn authenticate_bearer(
    pool: &PgPool,
    bearer_token: &str,
) -> Result<AuthPrincipal, AuthError> {
    let row = sqlx::query(
        "SELECT t.token_id, t.session_id, t.user_id, s.org_id, u.role
         FROM access_tokens t
         JOIN auth_sessions s ON s.session_id = t.session_id
         JOIN users u ON u.user_id = t.user_id
         WHERE t.token_hash = $1
           AND t.kind IN ('access', 'integration')
           AND t.revoked_at IS NULL
           AND (t.expires_at IS NULL OR t.expires_at > now())
           AND s.revoked_at IS NULL
           AND u.status = 'active'",
    )
    .bind(secret_hash_hex(bearer_token))
    .fetch_optional(pool)
    .await?
    .ok_or(AuthError::Unauthorized)?;
    Ok(AuthPrincipal {
        token_id: row.try_get("token_id")?,
        session_id: row.try_get("session_id")?,
        user_id: row.try_get("user_id")?,
        org_id: row.try_get("org_id")?,
        role: row.try_get("role")?,
    })
}

pub(super) async fn revoke_session(
    tx: &mut Transaction<'_, Postgres>,
    principal: &AuthPrincipal,
) -> Result<SessionRevoked, AuthError> {
    let result = sqlx::query(
        "UPDATE auth_sessions SET revoked_at = now()
         WHERE session_id = $1 AND revoked_at IS NULL",
    )
    .bind(&principal.session_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE access_tokens SET revoked_at = now()
         WHERE session_id = $1 AND revoked_at IS NULL",
    )
    .bind(&principal.session_id)
    .execute(&mut **tx)
    .await?;
    insert_audit_event(
        tx,
        &principal.org_id,
        Some(&principal.user_id),
        "auth.session_revoked",
        "session",
        Some(&principal.session_id),
    )
    .await?;
    Ok(SessionRevoked {
        revoked: result.rows_affected() == 1,
    })
}

pub(super) async fn exchange_authorization_code(
    tx: &mut Transaction<'_, Postgres>,
    code: &str,
    redirect_uri: &str,
    actual_challenge: &str,
) -> Result<TokenResponse, AuthError> {
    let row = sqlx::query(
        "SELECT code_id, user_id, org_id, redirect_uri, code_challenge
         FROM authorization_codes
         WHERE code_hash = $1 AND consumed_at IS NULL AND expires_at > now()
         FOR UPDATE",
    )
    .bind(secret_hash_hex(code))
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(AuthError::InvalidGrant)?;
    let expected_redirect: String = row.try_get("redirect_uri")?;
    let expected_challenge: String = row.try_get("code_challenge")?;
    if expected_redirect != redirect_uri
        || expected_challenge.len() != actual_challenge.len()
        || expected_challenge
            .as_bytes()
            .ct_eq(actual_challenge.as_bytes())
            .unwrap_u8()
            != 1
    {
        return Err(AuthError::InvalidGrant);
    }
    let code_id: String = row.try_get("code_id")?;
    let user_id: String = row.try_get("user_id")?;
    let org_id: String = row.try_get("org_id")?;
    sqlx::query("UPDATE authorization_codes SET consumed_at = now() WHERE code_id = $1")
        .bind(&code_id)
        .execute(&mut **tx)
        .await?;
    let session_id = prefixed_id("ses");
    sqlx::query("INSERT INTO auth_sessions (session_id, user_id, org_id) VALUES ($1, $2, $3)")
        .bind(&session_id)
        .bind(&user_id)
        .bind(&org_id)
        .execute(&mut **tx)
        .await?;
    let response = issue_token_pair(tx, &session_id, &user_id, &org_id).await?;
    insert_audit_event(
        tx,
        &org_id,
        Some(&user_id),
        "auth.session_created",
        "session",
        Some(&session_id),
    )
    .await?;
    Ok(response)
}

pub(super) async fn rotate_refresh_token(
    tx: &mut Transaction<'_, Postgres>,
    refresh_token: &str,
) -> Result<TokenResponse, AuthError> {
    let row = sqlx::query(
        "SELECT t.token_id, t.session_id, t.user_id, s.org_id
         FROM access_tokens t
         JOIN auth_sessions s ON s.session_id = t.session_id
         JOIN users u ON u.user_id = t.user_id
         WHERE t.token_hash = $1 AND t.kind = 'refresh'
           AND t.revoked_at IS NULL AND t.expires_at > now()
           AND s.revoked_at IS NULL AND u.status = 'active'
         FOR UPDATE OF t",
    )
    .bind(secret_hash_hex(refresh_token))
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(AuthError::InvalidGrant)?;
    let token_id: String = row.try_get("token_id")?;
    let session_id: String = row.try_get("session_id")?;
    let user_id: String = row.try_get("user_id")?;
    let org_id: String = row.try_get("org_id")?;
    sqlx::query("UPDATE access_tokens SET revoked_at = now() WHERE token_id = $1")
        .bind(&token_id)
        .execute(&mut **tx)
        .await?;
    issue_token_pair(tx, &session_id, &user_id, &org_id).await
}

async fn issue_token_pair(
    tx: &mut Transaction<'_, Postgres>,
    session_id: &str,
    user_id: &str,
    org_id: &str,
) -> Result<TokenResponse, AuthError> {
    let access_token = random_token();
    let refresh_token = random_token();
    let now = OffsetDateTime::now_utc();
    for (kind, token, expires_at) in [
        ("access", &access_token, now + ACCESS_TOKEN_TTL),
        ("refresh", &refresh_token, now + REFRESH_TOKEN_TTL),
    ] {
        sqlx::query(
            "INSERT INTO access_tokens (
                token_id, session_id, user_id, kind, token_hash, expires_at
             ) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(prefixed_id("tok"))
        .bind(session_id)
        .bind(user_id)
        .bind(kind)
        .bind(secret_hash_hex(token))
        .bind(expires_at)
        .execute(&mut **tx)
        .await?;
    }
    let user_row = sqlx::query(
        "SELECT user_id, email, display_name, avatar_url, role FROM users WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    let org_row = sqlx::query("SELECT org_id, name FROM orgs WHERE org_id = $1")
        .bind(org_id)
        .fetch_one(&mut **tx)
        .await?;
    let role: String = user_row.try_get("role")?;
    let user = UserRef {
        user_id: user_row.try_get("user_id")?,
        email: user_row.try_get("email")?,
        display_name: user_row.try_get("display_name")?,
        avatar_url: user_row.try_get("avatar_url")?,
        role: role.clone(),
    };
    Ok(TokenResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_owned(),
        expires_in: ACCESS_TOKEN_TTL.whole_seconds(),
        capabilities: user_capabilities(&role),
        user,
        org: OrgRef {
            org_id: org_row.try_get("org_id")?,
            name: org_row.try_get("name")?,
        },
    })
}

pub(super) async fn resolve_external_identity(
    tx: &mut Transaction<'_, Postgres>,
    identity: &OidcIdentity,
) -> Result<String, AuthError> {
    let bound_user = sqlx::query(
        "SELECT u.user_id, u.status
         FROM external_identities i
         JOIN users u ON u.user_id = i.user_id
         WHERE i.protocol = 'oidc' AND i.issuer = $1 AND i.subject = $2
         FOR UPDATE OF i, u",
    )
    .bind(&identity.issuer)
    .bind(&identity.subject)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(user) = bound_user {
        let user_id: String = user.try_get("user_id")?;
        ensure_member_enabled(user.try_get("status")?)?;
        activate_member(
            tx,
            &user_id,
            identity.display_name.as_deref(),
            identity.avatar_url.as_deref(),
        )
        .await?;
        return Ok(user_id);
    }

    let user = sqlx::query(
        "SELECT user_id, status
         FROM users
         WHERE lower(email) = lower($1)
         FOR UPDATE",
    )
    .bind(&identity.email)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(AuthError::MemberNotAllowed)?;
    let user_id: String = user.try_get("user_id")?;
    ensure_member_enabled(user.try_get("status")?)?;

    let existing_subject = sqlx::query_scalar::<_, String>(
        "SELECT subject
         FROM external_identities
         WHERE user_id = $1 AND protocol = 'oidc' AND issuer = $2
         FOR UPDATE",
    )
    .bind(&user_id)
    .bind(&identity.issuer)
    .fetch_optional(&mut **tx)
    .await?;
    match existing_subject {
        Some(subject) if subject != identity.subject => {
            return Err(AuthError::ProviderIdentityConflict);
        }
        Some(_) => {}
        None => {
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
        }
    }
    activate_member(
        tx,
        &user_id,
        identity.display_name.as_deref(),
        identity.avatar_url.as_deref(),
    )
    .await?;
    Ok(user_id)
}

async fn activate_member(
    tx: &mut Transaction<'_, Postgres>,
    user_id: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
) -> Result<(), AuthError> {
    sqlx::query(
        "UPDATE users
         SET status = 'active',
             display_name = COALESCE($2, display_name),
             avatar_url = COALESCE($3, avatar_url),
             revision = revision + 1, updated_at = now()
         WHERE user_id = $1
           AND (
               status <> 'active'
               OR ($2 IS NOT NULL AND display_name IS DISTINCT FROM $2)
               OR ($3 IS NOT NULL AND avatar_url IS DISTINCT FROM $3)
           )",
    )
    .bind(user_id)
    .bind(display_name)
    .bind(avatar_url)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn insert_audit_event(
    tx: &mut Transaction<'_, Postgres>,
    org_id: &str,
    actor_user_id: Option<&str>,
    action: &str,
    target_type: &str,
    target_id: Option<&str>,
) -> Result<(), AuthError> {
    sqlx::query(
        "INSERT INTO audit_events (
            event_id, org_id, actor_user_id, action, target_type, target_id
         ) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(prefixed_id("evt"))
    .bind(org_id)
    .bind(actor_user_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
