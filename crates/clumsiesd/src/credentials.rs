use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const KEYCHAIN_ACCOUNT: &str = "server-session";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ServerCredentials {
    pub server_url: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl fmt::Debug for ServerCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServerCredentials")
            .field("server_url", &self.server_url)
            .field("access_token", &"[REDACTED]")
            .field("has_refresh_token", &self.refresh_token.is_some())
            .finish()
    }
}

pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError>;
    fn replace(&self, credentials: &ServerCredentials) -> Result<(), CredentialStoreError>;
    fn clear(&self) -> Result<(), CredentialStoreError>;
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct CredentialStoreError {
    message: String,
}

impl CredentialStoreError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SystemCredentialStore {
    /// The Keychain service name, and the account inside it. Linux keeps one
    /// owner-only file per daemon root instead, so these two only form the
    /// identity where there is a Keychain to key on.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    service: String,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    account: String,
}

impl Default for SystemCredentialStore {
    fn default() -> Self {
        Self::new(crate::IDENTIFIER_NAMESPACE, KEYCHAIN_ACCOUNT)
    }
}

impl SystemCredentialStore {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }
}

#[cfg(target_os = "macos")]
impl CredentialStore for SystemCredentialStore {
    fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
        use security_framework::passwords::get_generic_password;
        use security_framework_sys::base::errSecItemNotFound;

        let bytes = match get_generic_password(&self.service, &self.account) {
            Ok(bytes) => bytes,
            Err(error) if error.code() == errSecItemNotFound => return Ok(None),
            Err(error) => {
                return Err(CredentialStoreError::new(format!(
                    "failed to read the macOS Keychain item: {error}"
                )));
            }
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            CredentialStoreError::new(format!(
                "the macOS Keychain item contains invalid credentials: {error}"
            ))
        })
    }

    fn replace(&self, credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
        use security_framework::passwords::set_generic_password;

        let bytes = serde_json::to_vec(credentials).map_err(|error| {
            CredentialStoreError::new(format!("failed to encode Server credentials: {error}"))
        })?;
        set_generic_password(&self.service, &self.account, &bytes).map_err(|error| {
            CredentialStoreError::new(format!("failed to update the macOS Keychain item: {error}"))
        })
    }

    fn clear(&self) -> Result<(), CredentialStoreError> {
        use security_framework::passwords::delete_generic_password;
        use security_framework_sys::base::errSecItemNotFound;

        match delete_generic_password(&self.service, &self.account) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == errSecItemNotFound => Ok(()),
            Err(error) => Err(CredentialStoreError::new(format!(
                "failed to delete the macOS Keychain item: {error}"
            ))),
        }
    }
}

/// Linux has no Keychain. Credentials live in one owner-only file inside the
/// daemon's own root, which is the same trust boundary the Keychain item has:
/// only the user who runs the daemon can read it.
///
/// The file is written through a temporary file and renamed, so an interrupted
/// write cannot leave a half-serialized credential behind, and it is created
/// with mode 0600 rather than tightened afterwards.
#[cfg(all(unix, not(target_os = "macos")))]
impl CredentialStore for SystemCredentialStore {
    fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
        match std::fs::read(self.path()?) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
                CredentialStoreError::new(format!(
                    "the credential file contains invalid credentials: {error}"
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(CredentialStoreError::new(format!(
                "failed to read the credential file: {error}"
            ))),
        }
    }

    fn replace(&self, credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
        let path = self.path()?;
        let bytes = serde_json::to_vec(credentials).map_err(|error| {
            CredentialStoreError::new(format!("failed to encode Server credentials: {error}"))
        })?;
        let temporary = path.with_extension("json.tmp");
        write_owner_only(&temporary, &bytes)?;
        std::fs::rename(&temporary, &path).map_err(|error| {
            CredentialStoreError::new(format!("failed to store the credential file: {error}"))
        })
    }

    fn clear(&self) -> Result<(), CredentialStoreError> {
        match std::fs::remove_file(self.path()?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CredentialStoreError::new(format!(
                "failed to delete the credential file: {error}"
            ))),
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
impl SystemCredentialStore {
    fn path(&self) -> Result<std::path::PathBuf, CredentialStoreError> {
        crate::config::daemon_root_dir()
            .map(|root| root.join("credentials.json"))
            .map_err(|error| {
                CredentialStoreError::new(format!("failed to resolve the daemon root: {error}"))
            })
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn write_owner_only(path: &std::path::Path, bytes: &[u8]) -> Result<(), CredentialStoreError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            CredentialStoreError::new(format!("failed to create the credential file: {error}"))
        })?;
    file.write_all(bytes).map_err(|error| {
        CredentialStoreError::new(format!("failed to write the credential file: {error}"))
    })
}

#[cfg(not(unix))]
impl CredentialStore for SystemCredentialStore {
    fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
        Err(unsupported_platform_error())
    }

    fn replace(&self, _credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
        Err(unsupported_platform_error())
    }

    fn clear(&self) -> Result<(), CredentialStoreError> {
        Err(unsupported_platform_error())
    }
}

#[cfg(not(unix))]
fn unsupported_platform_error() -> CredentialStoreError {
    CredentialStoreError::new(
        "this platform has no credential store yet; macOS uses the Keychain and Linux uses an owner-only file",
    )
}

#[cfg(test)]
mod tests {
    use super::ServerCredentials;

    #[test]
    fn credential_debug_output_redacts_secrets() {
        let credentials = ServerCredentials {
            server_url: "https://clumsies.example.com".to_owned(),
            access_token: "access-secret".to_owned(),
            refresh_token: Some("refresh-secret".to_owned()),
        };

        let output = format!("{credentials:?}");

        assert!(output.contains("https://clumsies.example.com"));
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("access-secret"));
        assert!(!output.contains("refresh-secret"));
    }
}
