//! Local password and single-use account-action operations sharing the existing session model.

use super::local_repository::{
    self, issue_action, lock_active_user, require_session, revoke_credentials, save_password,
};
use super::{AuthError, AuthPrincipal, AuthService, dto, password, repository};
use crate::identity::{prefixed_id, random_token, secret_hash_hex};
use sqlx::Row;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Process-wide ceiling protects anonymous hashing and credential redemption from unbounded work.
static ATTEMPTS: OnceLock<Mutex<(Instant, u32)>> = OnceLock::new();

/// Reserve an anonymous authentication attempt before any expensive work.
///
/// # Errors
/// Rejects traffic above sixty attempts per minute per server process.
fn throttle() -> Result<(), AuthError> {
    // ponytail: per-process budget for the current deployment; distributed limiting if scaled out.
    let mut budget = ATTEMPTS
        .get_or_init(|| Mutex::new((Instant::now(), 0)))
        .lock()
        .map_err(|_| AuthError::PasswordUnavailable)?;
    if budget.0.elapsed() >= Duration::from_secs(60) {
        *budget = (Instant::now(), 0);
    }
    if budget.1 >= 60 {
        return Err(AuthError::RateLimited);
    }
    budget.1 += 1;
    Ok(())
}

impl AuthService {
    /// Reject local credential operations when deployment requires external authentication.
    ///
    /// # Errors
    /// Returns forbidden when password authentication is disabled.
    fn require_password_enabled(&self) -> Result<(), AuthError> {
        if self.password_enabled {
            Ok(())
        } else {
            Err(AuthError::Forbidden)
        }
    }

    /// Expose provider branding and supported methods without credentials.
    pub fn login_methods(&self) -> dto::LoginMethods {
        dto::LoginMethods {
            password_enabled: self.password_enabled,
            oidc_enabled: self.configured(),
            google: self
                .provider_summary
                .as_ref()
                .is_some_and(|p| p.issuer.trim_end_matches('/') == "https://accounts.google.com"),
        }
    }

    /// Verify a password and recheck its current hash under the account lock before issuance.
    ///
    /// # Errors
    /// Rejects invalid credentials, disabled membership, rate limits, or persistence failures.
    pub async fn password_login(
        &self,
        request: dto::PasswordLoginRequest,
    ) -> Result<dto::TokenResponse, AuthError> {
        self.require_password_enabled()?;
        throttle()?;
        let username =
            password::username(&request.username).map_err(|_| AuthError::Unauthorized)?;
        let row = local_repository::password_account(&self.pool, &username).await?;
        let Some(row) = row else {
            // Equal-cost work prevents cheap username enumeration through timing.
            let _ = password::hash(random_token()).await?;
            return Err(AuthError::Unauthorized);
        };
        let user_id: String = row.try_get("user_id")?;
        let hash: String = row.try_get("password_hash")?;
        if !password::verify(request.password, hash.clone()).await? {
            return Err(AuthError::Unauthorized);
        }
        let mut tx = self.pool.begin().await?;
        lock_active_user(&mut tx, &user_id).await?;
        let current: String = local_repository::password_hash(&mut *tx, &user_id)
            .await?
            .ok_or(AuthError::Unauthorized)?;
        if current != hash {
            return Err(AuthError::Unauthorized);
        }
        let org = repository::organization_admission(&mut tx).await?;
        let result = repository::create_session(&mut tx, &user_id, &org.org_id).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Create a pending account and its one-time invitation atomically.
    ///
    /// # Errors
    /// Rejects non-administrators, unauthorized owner invitations, or persistence failures.
    pub async fn invite(
        &self,
        actor: &AuthPrincipal,
        request: dto::InvitationRequest,
    ) -> Result<dto::ActionTokenResponse, AuthError> {
        self.require_password_enabled()?;
        let mut tx = self.pool.begin().await?;
        let actor_role = lock_active_user(&mut tx, &actor.user_id).await?;
        let role = match request.role {
            crate::app::organization::dto::OrgRole::Owner => "owner",
            crate::app::organization::dto::OrgRole::Admin => "admin",
            crate::app::organization::dto::OrgRole::Member => "member",
        };
        if !matches!(actor_role.as_str(), "owner" | "admin")
            || (role != "member" && actor_role != "owner")
        {
            return Err(AuthError::Forbidden);
        }
        let user_id = prefixed_id("usr");
        local_repository::insert_invited_user(&mut *tx, &user_id, role).await?;
        let result = issue_action(&mut tx, &user_id, "invitation", Some(&actor.user_id)).await?;
        repository::insert_audit_event(
            &mut tx,
            &actor.org_id,
            Some(&actor.user_id),
            "auth.invitation_created",
            "user",
            Some(&user_id),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Replace outstanding invitations or issue an administrator-approved password reset.
    ///
    /// # Errors
    /// Rejects disabled targets, insufficient roles, invalid lifecycle, or database failures.
    pub async fn issue_member_action(
        &self,
        actor: &AuthPrincipal,
        user_id: &str,
        invitation: bool,
    ) -> Result<dto::ActionTokenResponse, AuthError> {
        self.require_password_enabled()?;
        let mut tx = self.pool.begin().await?;
        // Serialize credential administration to avoid opposite-order actor/target row deadlocks.
        local_repository::lock_organization(&mut *tx, &actor.org_id).await?;
        let actor_role = lock_active_user(&mut tx, &actor.user_id).await?;
        let target = local_repository::lock_member(&mut *tx, user_id)
            .await?
            .ok_or(AuthError::Forbidden)?;
        let role: String = target.try_get("role")?;
        let status: String = target.try_get("status")?;
        if !matches!(actor_role.as_str(), "owner" | "admin")
            || (role != "member" && actor_role != "owner")
        {
            return Err(AuthError::Forbidden);
        }
        if status != if invitation { "invited" } else { "active" } {
            return Err(AuthError::InvalidGrant);
        }
        if !invitation {
            let exists: bool = local_repository::has_password(&mut *tx, user_id).await?;
            if !exists {
                return Err(AuthError::InvalidGrant);
            }
        }
        let result = issue_action(
            &mut tx,
            user_id,
            if invitation {
                "invitation"
            } else {
                "password_reset"
            },
            Some(&actor.user_id),
        )
        .await?;
        repository::insert_audit_event(
            &mut tx,
            &actor.org_id,
            Some(&actor.user_id),
            "auth.action_issued",
            "user",
            Some(user_id),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Revoke a credential by its non-secret identifier.
    ///
    /// # Errors
    /// Rejects unauthorized administrators or persistence failures.
    pub async fn revoke_action(
        &self,
        actor: &AuthPrincipal,
        token_id: &str,
    ) -> Result<(), AuthError> {
        let mut tx = self.pool.begin().await?;
        let role = lock_active_user(&mut tx, &actor.user_id).await?;
        if !matches!(role.as_str(), "owner" | "admin") {
            return Err(AuthError::Forbidden);
        }
        let revoked =
            local_repository::revoke_action(&mut *tx, token_id, &actor.user_id, &role).await?;
        if revoked.rows_affected() > 0 {
            repository::insert_audit_event(
                &mut tx,
                &actor.org_id,
                Some(&actor.user_id),
                "auth.action_revoked",
                "action_token",
                Some(token_id),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Consume one purpose-bound action and activate or reset its account in one transaction.
    ///
    /// # Errors
    /// Rejects expired, replayed, revoked or wrong-purpose tokens and invalid account state.
    pub async fn redeem_action(
        &self,
        request: dto::RedeemActionRequest,
        invitation: bool,
    ) -> Result<dto::TokenResponse, AuthError> {
        self.require_password_enabled()?;
        throttle()?;
        if request.token.len() > 256 {
            return Err(AuthError::InvalidGrant);
        }
        let purpose = if invitation {
            "invitation"
        } else {
            "password_reset"
        };
        let token_hash = secret_hash_hex(&request.token);
        let user_id: String = local_repository::action_user(&self.pool, &token_hash, purpose)
            .await?
            .ok_or(AuthError::InvalidGrant)?;
        let username = if invitation {
            Some(password::username(
                request.username.as_deref().ok_or(AuthError::InvalidGrant)?,
            )?)
        } else {
            None
        };
        let hash = password::hash(request.password).await?;
        let mut tx = self.pool.begin().await?;
        let status: String = local_repository::lock_status(&mut *tx, &user_id).await?;
        if status != if invitation { "invited" } else { "active" } {
            return Err(AuthError::InvalidGrant);
        }
        let consumed = local_repository::consume_action(&mut *tx, &token_hash, purpose).await?;
        if consumed.rows_affected() != 1 {
            return Err(AuthError::InvalidGrant);
        }
        if let Some(username) = username {
            local_repository::activate_local_user(&mut *tx, &user_id, &username).await?;
        }
        save_password(&mut tx, &user_id, &hash).await?;
        revoke_credentials(&mut tx, &user_id).await?;
        let org = repository::organization_admission(&mut tx).await?;
        repository::insert_audit_event(
            &mut tx,
            &org.org_id,
            Some(&user_id),
            if invitation {
                "auth.invitation_accepted"
            } else {
                "auth.password_reset"
            },
            "user",
            Some(&user_id),
        )
        .await?;
        let result = repository::create_session(&mut tx, &user_id, &org.org_id).await?;
        tx.commit().await?;
        Ok(result)
    }
}

impl AuthService {
    /// Read the current account's configured login methods without credential material.
    ///
    /// # Errors
    /// Propagates persistence failures or a missing account.
    pub async fn account_credentials(
        &self,
        actor: &AuthPrincipal,
    ) -> Result<dto::AccountCredentials, AuthError> {
        let row = local_repository::account_credentials(&self.pool, &actor.user_id).await?;
        Ok(dto::AccountCredentials {
            username: row.try_get("username")?,
            password_set: row.try_get("password_set")?,
            oidc_email: row.try_get("oidc_email")?,
        })
    }

    /// Verify local password ownership or require a recently created OIDC session.
    ///
    /// # Errors
    /// Rejects incorrect credentials and stale or revoked sessions.
    async fn confirm_account(
        &self,
        actor: &AuthPrincipal,
        current_password: Option<String>,
    ) -> Result<Option<String>, AuthError> {
        throttle()?;
        let hash: Option<String> =
            local_repository::password_hash(&self.pool, &actor.user_id).await?;
        if let Some(hash) = &hash {
            if !password::verify(
                current_password.ok_or(AuthError::Unauthorized)?,
                hash.clone(),
            )
            .await?
            {
                return Err(AuthError::Unauthorized);
            }
        } else {
            let recent: bool = local_repository::recent_external_session(
                &self.pool,
                &actor.session_id,
                &actor.user_id,
            )
            .await?;
            if !recent {
                return Err(AuthError::InvalidRequest(
                    "sign in again before changing login methods".into(),
                ));
            }
        }
        Ok(hash)
    }

    /// Establish or change local credentials and replace every previous session.
    ///
    /// # Errors
    /// Rejects stale proof, duplicate usernames and invalid password policy.
    pub async fn change_password(
        &self,
        actor: &AuthPrincipal,
        request: dto::ChangePasswordRequest,
    ) -> Result<dto::TokenResponse, AuthError> {
        self.require_password_enabled()?;
        let expected = self
            .confirm_account(actor, request.current_password)
            .await?;
        let hash = password::hash(request.password).await?;
        let mut tx = self.pool.begin().await?;
        lock_active_user(&mut tx, &actor.user_id).await?;
        let current: Option<String> =
            local_repository::password_hash(&mut *tx, &actor.user_id).await?;
        if expected != current {
            return Err(AuthError::Unauthorized);
        }
        require_session(&mut tx, actor).await?;
        let username: Option<String> = local_repository::username(&mut *tx, &actor.user_id).await?;
        if username.is_none() {
            let username = password::username(
                request
                    .username
                    .as_deref()
                    .ok_or_else(|| AuthError::InvalidRequest("username is required".into()))?,
            )?;
            local_repository::set_username(&mut *tx, &actor.user_id, &username).await?;
        }
        save_password(&mut tx, &actor.user_id, &hash).await?;
        revoke_credentials(&mut tx, &actor.user_id).await?;
        repository::insert_audit_event(
            &mut tx,
            &actor.org_id,
            Some(&actor.user_id),
            "auth.password_changed",
            "user",
            Some(&actor.user_id),
        )
        .await?;
        let result = repository::create_session(&mut tx, &actor.user_id, &actor.org_id).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Start provider verification bound to the authenticated initiating session.
    ///
    /// # Errors
    /// Rejects missing proof, unsafe callbacks, changed credentials or revoked sessions.
    pub async fn begin_binding(
        &self,
        actor: &AuthPrincipal,
        request: dto::BindOidcRequest,
    ) -> Result<dto::OidcBindingResponse, AuthError> {
        let expected = self
            .confirm_account(actor, request.current_password)
            .await?;
        let authorization_url = self.begin_login(request.authorization).await?;
        let url =
            url::Url::parse(&authorization_url).map_err(|_| AuthError::PasswordUnavailable)?;
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .ok_or(AuthError::PasswordUnavailable)?;
        let mut tx = self.pool.begin().await?;
        lock_active_user(&mut tx, &actor.user_id).await?;
        require_session(&mut tx, actor).await?;
        let current: Option<String> =
            local_repository::password_hash(&mut *tx, &actor.user_id).await?;
        if expected != current {
            return Err(AuthError::Unauthorized);
        }
        local_repository::bind_transaction(&mut *tx, &secret_hash_hex(&state), &actor.session_id)
            .await?;
        tx.commit().await?;
        Ok(dto::OidcBindingResponse { authorization_url })
    }
}

impl AuthService {
    /// Establish a local owner using an already-authorized installation setup session.
    ///
    /// # Errors
    /// Rejects invalid credentials, consumed setup sessions and completed installations.
    pub async fn initialize_password_owner(
        &self,
        installation: &crate::app::installation::InstallationService,
        setup_session_id: &str,
        request: dto::PasswordLoginRequest,
    ) -> Result<dto::TokenResponse, AuthError> {
        self.require_password_enabled()?;
        throttle()?;
        let username = password::username(&request.username)?;
        let hash = password::hash(request.password).await?;
        let mut tx = self.pool.begin().await?;
        let initialized = installation
            .initialize_local(&mut tx, setup_session_id)
            .await?;
        local_repository::initialize_username(&mut *tx, &initialized.user_id, &username).await?;
        save_password(&mut tx, &initialized.user_id, &hash).await?;
        let result =
            repository::create_session(&mut tx, &initialized.user_id, &initialized.org_id).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Issue recovery for an existing active owner through the deployment-only CLI.
    ///
    /// # Errors
    /// Rejects non-owner targets and propagates persistence failures.
    pub async fn recover_owner(
        &self,
        user_id: &str,
    ) -> Result<dto::ActionTokenResponse, AuthError> {
        let mut tx = self.pool.begin().await?;
        if lock_active_user(&mut tx, user_id).await? != "owner" {
            return Err(AuthError::Forbidden);
        }
        let local: bool = local_repository::has_password(&mut *tx, user_id).await?;
        if !local {
            return Err(AuthError::InvalidRequest(
                "owner has no local password; use the configured identity provider".into(),
            ));
        }
        let result = issue_action(&mut tx, user_id, "password_reset", None).await?;
        let org = repository::organization_admission(&mut tx).await?;
        repository::insert_audit_event(
            &mut tx,
            &org.org_id,
            None,
            "auth.owner_recovery_issued",
            "user",
            Some(user_id),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }
}
