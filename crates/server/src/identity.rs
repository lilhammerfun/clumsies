//! Random identifiers and secret hashing.

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Generate an opaque random identifier in the requested resource namespace.
pub(crate) fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

/// Generate an unpredictable credential suitable for one-time authorization.
pub(crate) fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Compute the fixed-length digest used to compare or persist credentials.
pub(crate) fn secret_hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

/// Encode a credential digest for storage without retaining its plaintext value.
pub(crate) fn secret_hash_hex(value: &str) -> String {
    hex::encode(secret_hash(value))
}
