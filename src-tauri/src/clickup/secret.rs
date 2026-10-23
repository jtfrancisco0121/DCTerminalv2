//! Where the ClickUp tokens are kept: the macOS keychain (through the
//! `keyring` crate), behind [`SecretStore`] so tests use memory and never
//! touch the real keychain.
//!
//! One token per Claude account, because each account works in its own
//! ClickUp workspace (work and personal). The keychain item's account field
//! is the Claude account id. The service name is v2's own, so DCTerminal's
//! items (`com.jtfrancisco.dcterminal.clickup`) are never read or changed.
//! There is no env override in v2: nothing to strip from children's env.

use super::ClickUpToken;

/// Keychain item service; the item's account field is the Claude account id.
pub const KEYCHAIN_SERVICE: &str = "com.jtfrancisco.dcterminalv2.clickup";

/// One token slot per Claude account id. Errors are plain English and never
/// hold the token.
pub trait SecretStore: Send + Sync {
    fn get(&self, account_id: &str) -> Result<Option<ClickUpToken>, String>;
    fn set(&self, account_id: &str, token: &ClickUpToken) -> Result<(), String>;
    /// Clearing an empty slot is fine.
    fn clear(&self, account_id: &str) -> Result<(), String>;
}

/// The OS keychain, through the `keyring` crate.
pub struct KeychainStore;

impl KeychainStore {
    fn entry(account_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account_id).map_err(|err| keychain_error(&err))
    }
}

/// What went wrong, by kind only. Some keyring errors carry the stored bytes
/// (`BadEncoding`), so the error's own text is never used.
fn keychain_error(err: &keyring::Error) -> String {
    use keyring::Error;
    let what = match err {
        Error::NoStorageAccess(_) => "DCTerminalv2 was not allowed to use it",
        Error::NoDefaultStore => "this system has no keychain DCTerminalv2 can use",
        Error::BadEncoding(_) | Error::BadDataFormat(..) => "the saved item is unreadable",
        Error::PlatformFailure(_) => "the keychain reported an error",
        _ => "the keychain refused the request",
    };
    format!("Could not use the system keychain: {what}.")
}

impl SecretStore for KeychainStore {
    fn get(&self, account_id: &str) -> Result<Option<ClickUpToken>, String> {
        match Self::entry(account_id)?.get_password() {
            Ok(saved) => Ok(ClickUpToken::parse(&saved).ok()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(keychain_error(&err)),
        }
    }

    fn set(&self, account_id: &str, token: &ClickUpToken) -> Result<(), String> {
        Self::entry(account_id)?
            .set_password(token.expose())
            .map_err(|err| keychain_error(&err))
    }

    fn clear(&self, account_id: &str) -> Result<(), String> {
        match Self::entry(account_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(keychain_error(&err)),
        }
    }
}

/// In-memory slots for tests, keyed by account id like the keychain.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    pub slots: std::sync::Mutex<std::collections::HashMap<String, ClickUpToken>>,
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn get(&self, account_id: &str) -> Result<Option<ClickUpToken>, String> {
        Ok(self
            .slots
            .lock()
            .map_err(|e| e.to_string())?
            .get(account_id)
            .cloned())
    }

    fn set(&self, account_id: &str, token: &ClickUpToken) -> Result<(), String> {
        self.slots
            .lock()
            .map_err(|e| e.to_string())?
            .insert(account_id.to_string(), token.clone());
        Ok(())
    }

    fn clear(&self, account_id: &str) -> Result<(), String> {
        self.slots
            .lock()
            .map_err(|e| e.to_string())?
            .remove(account_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_account_has_its_own_slot() {
        let store = MemoryStore::default();
        store
            .set("work", &ClickUpToken::parse("pk_work").unwrap())
            .unwrap();
        store
            .set("personal", &ClickUpToken::parse("pk_personal").unwrap())
            .unwrap();
        assert_eq!(store.get("work").unwrap().unwrap().expose(), "pk_work");
        assert_eq!(
            store.get("personal").unwrap().unwrap().expose(),
            "pk_personal"
        );
        store.clear("work").unwrap();
        assert_eq!(store.get("work").unwrap(), None);
        assert_eq!(
            store.get("personal").unwrap().unwrap().expose(),
            "pk_personal"
        );
    }

    #[test]
    fn v2_has_its_own_keychain_service() {
        assert_eq!(KEYCHAIN_SERVICE, "com.jtfrancisco.dcterminalv2.clickup");
        assert_ne!(KEYCHAIN_SERVICE, "com.jtfrancisco.dcterminal.clickup");
    }

    #[test]
    fn keychain_errors_never_carry_the_stored_bytes() {
        let err = keychain_error(&keyring::Error::BadEncoding(b"pk_SECRET".to_vec()));
        assert!(!err.contains("pk_SECRET"));
        assert!(err.starts_with("Could not use the system keychain"));
    }
}
