//! Bounded Argon2id work and local credential input rules.

use super::AuthError;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;

/// Bound memory-hard work even when a request is cancelled before its blocking worker finishes.
static HASH_SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();

/// Normalize a local login name without accepting ambiguous Unicode identifiers.
///
/// # Errors
/// Rejects names outside the documented 3–32 ASCII character alphabet.
pub(super) fn username(raw: &str) -> Result<String, AuthError> {
    let value = raw.trim().to_ascii_lowercase();
    if !(3..=32).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        || !value.as_bytes()[0].is_ascii_alphanumeric()
    {
        return Err(AuthError::InvalidRequest(
            "username must contain 3–32 ASCII letters, digits, dots, underscores or hyphens".into(),
        ));
    }
    Ok(value)
}

/// Hash a new password off the async executor using Argon2id and a fresh random salt.
///
/// # Errors
/// Rejects short or oversized passwords, saturated workers, and hashing failures.
pub(super) async fn hash(password: String) -> Result<String, AuthError> {
    if password.chars().count() < 15 || password.len() > 1024 {
        return Err(AuthError::InvalidRequest(
            "password must contain at least 15 characters and at most 1024 bytes".into(),
        ));
    }
    let permit = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        HASH_SLOTS
            .get_or_init(|| Arc::new(Semaphore::new(4)))
            .clone()
            .acquire_owned(),
    )
    .await
    .map_err(|_| AuthError::RateLimited)?
    .map_err(|_| AuthError::PasswordUnavailable)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let salt = SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes())
            .map_err(|_| AuthError::PasswordUnavailable)?;
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| AuthError::PasswordUnavailable)
    })
    .await
    .map_err(|_| AuthError::PasswordUnavailable)?
}

/// Verify a password off the executor with the same bounded memory budget as hashing.
///
/// # Errors
/// Rejects oversized input, saturated workers, and malformed stored credentials.
pub(super) async fn verify(password: String, encoded: String) -> Result<bool, AuthError> {
    if password.len() > 1024 {
        return Ok(false);
    }
    let permit = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        HASH_SLOTS
            .get_or_init(|| Arc::new(Semaphore::new(4)))
            .clone()
            .acquire_owned(),
    )
    .await
    .map_err(|_| AuthError::RateLimited)?
    .map_err(|_| AuthError::PasswordUnavailable)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let hash = PasswordHash::new(&encoded).map_err(|_| AuthError::PasswordUnavailable)?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok())
    })
    .await
    .map_err(|_| AuthError::PasswordUnavailable)?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn password_round_trip_and_username_rules() {
        let hash = hash("a sufficiently long password".into()).await.unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(
            verify("a sufficiently long password".into(), hash.clone())
                .await
                .unwrap()
        );
        assert!(!verify("another password".into(), hash).await.unwrap());
        assert_eq!(username(" Alice_1 ").unwrap(), "alice_1");
        assert!(username("a@b").is_err());
        assert!(username("").is_err());
    }
}
