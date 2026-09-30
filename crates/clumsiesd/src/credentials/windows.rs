//! The Windows user's credential vault owns the session, never a plaintext file.
use super::*;
use sha2::{Digest, Sha256};
use windows_sys::Win32::{
    Foundation::{ERROR_NOT_FOUND, GetLastError},
    Security::Credentials::*,
};

fn vault_error(operation: &str) -> CredentialStoreError {
    CredentialStoreError::new(format!(
        "Windows credential {operation} failed: {}",
        std::io::Error::last_os_error()
    ))
}

impl SystemCredentialStore {
    fn target_name(&self) -> Result<Vec<u16>, CredentialStoreError> {
        let root = crate::config::daemon_root_dir()
            .map_err(|e| CredentialStoreError::new(e.to_string()))?;
        let identity = format!("{}:{}:{}", self.service, self.account, root.display());
        Ok(
            format!("Clumsies/{:x}", Sha256::digest(identity.as_bytes()))
                .encode_utf16()
                .chain(Some(0))
                .collect(),
        )
    }
}

impl CredentialStore for SystemCredentialStore {
    fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
        let target = self.target_name()?;
        unsafe {
            let mut credential = std::ptr::null_mut();
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
                return if GetLastError() == ERROR_NOT_FOUND {
                    Ok(None)
                } else {
                    Err(vault_error("read"))
                };
            }
            let entry = &*credential;
            let result = if entry.CredentialBlobSize == 0 || entry.CredentialBlob.is_null() {
                Err(CredentialStoreError::new("Windows credential is empty"))
            } else {
                let bytes = std::slice::from_raw_parts(
                    entry.CredentialBlob,
                    entry.CredentialBlobSize as usize,
                );
                serde_json::from_slice(bytes)
                    .map(Some)
                    .map_err(|_| CredentialStoreError::new("Windows credential is invalid"))
            };
            CredFree(credential.cast());
            result
        }
    }

    fn replace(&self, credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
        let mut target = self.target_name()?;
        let mut bytes = serde_json::to_vec(credentials)
            .map_err(|_| CredentialStoreError::new("Could not encode credentials"))?;
        if bytes.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(CredentialStoreError::new(
                "Session exceeds the Windows credential vault size limit",
            ));
        }
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: bytes.len() as u32,
            CredentialBlob: bytes.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        let result = unsafe { CredWriteW(&credential, 0) };
        let error = if result == 0 {
            Some(vault_error("write"))
        } else {
            None
        };
        bytes.fill(0);
        error.map_or(Ok(()), Err)
    }

    fn clear(&self) -> Result<(), CredentialStoreError> {
        let target = self.target_name()?;
        unsafe {
            if CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) != 0
                || GetLastError() == ERROR_NOT_FOUND
            {
                Ok(())
            } else {
                Err(vault_error("delete"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vault_roundtrip_isolated_identity_and_delete() {
        let store =
            SystemCredentialStore::new(format!("test-{}", uuid::Uuid::new_v4()), "test-session");
        assert_eq!(store.load().unwrap(), None);
        let value = ServerCredentials {
            server_url: "https://example.invalid".into(),
            access_token: "test-access".into(),
            refresh_token: Some("test-refresh".into()),
        };
        store.replace(&value).unwrap();
        assert_eq!(store.load().unwrap(), Some(value.clone()));
        let mut oversized = value.clone();
        oversized.access_token = "x".repeat(CRED_MAX_CREDENTIAL_BLOB_SIZE as usize);
        assert!(store.replace(&oversized).is_err());
        assert_eq!(store.load().unwrap(), Some(value));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
    }
}
