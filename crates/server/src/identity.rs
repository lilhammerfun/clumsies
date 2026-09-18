//! Random identifiers and secret hashing.

use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

pub(crate) fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub(crate) fn secret_hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

pub(crate) fn secret_hash_hex(value: &str) -> String {
    hex::encode(secret_hash(value))
}
