#![cfg(target_os = "macos")]

use clumsiesd::{CredentialStore, IDENTIFIER_NAMESPACE, ServerCredentials, SystemCredentialStore};
use uuid::Uuid;

#[test]
fn macos_keychain_round_trip() {
    if std::env::var("CLUMSIES_TEST_MACOS_KEYCHAIN").as_deref() != Ok("1") {
        return;
    }

    let service = format!("{IDENTIFIER_NAMESPACE}.tests");
    let account = format!("server-session-{}", Uuid::new_v4().simple());
    let store = SystemCredentialStore::new(service, account);
    let cleanup = KeychainCleanup(&store);
    let credentials = ServerCredentials {
        server_url: "https://clumsies.example.com".to_owned(),
        access_token: "keychain-smoke-access".to_owned(),
        refresh_token: Some("keychain-smoke-refresh".to_owned()),
    };

    store.replace(&credentials).unwrap();
    assert_eq!(store.load().unwrap(), Some(credentials));
    store.clear().unwrap();
    assert_eq!(store.load().unwrap(), None);

    std::mem::forget(cleanup);
}

struct KeychainCleanup<'a>(&'a SystemCredentialStore);

impl Drop for KeychainCleanup<'_> {
    fn drop(&mut self) {
        let _ = self.0.clear();
    }
}
