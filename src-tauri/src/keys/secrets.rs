//! Passphrases remembered for this process, wiped on drop, and optionally stored in the system
//! keychain so they survive a restart.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use specta::Type;
use zeroize::Zeroizing;

use super::keychain::{KeychainStatus, SecretStore};
use crate::error::{GitError, Result};

/// Where a passphrase is remembered: nowhere, in memory until StashPanda closes, or in the
/// system keychain (and in memory too, once read).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PassphraseMemory {
    #[default]
    None,
    Session,
    Keychain,
}

struct Remembered {
    secret: Zeroizing<String>,
    in_keychain: bool,
}

#[derive(Default)]
pub struct PassphraseCache {
    inner: Mutex<HashMap<String, Remembered>>,
    /// The system keychain, consulted when memory has nothing for a path. None for caches that
    /// never persist anything (SSH passwords).
    keychain: Option<Arc<dyn SecretStore>>,
}

impl PassphraseCache {
    pub fn with_keychain(keychain: Arc<dyn SecretStore>) -> PassphraseCache {
        PassphraseCache { inner: Mutex::default(), keychain: Some(keychain) }
    }

    /// The remembered passphrase for `path`: from memory, else from the keychain, which is then
    /// kept in memory so the keychain is asked once per session.
    pub fn get(&self, path: &str) -> Option<Zeroizing<String>> {
        if let Some(r) = self.inner.lock().ok()?.get(path) {
            return Some(r.secret.clone());
        }
        let secret = self.keychain.as_ref()?.get(path)?;
        if let Ok(mut m) = self.inner.lock() {
            m.insert(path.to_string(), Remembered { secret: secret.clone(), in_keychain: true });
        }
        Some(secret)
    }

    /// Where the passphrase for `path` is remembered, looking in the keychain when memory has
    /// nothing.
    pub fn memory(&self, path: &str) -> PassphraseMemory {
        if self.get(path).is_none() {
            return PassphraseMemory::None;
        }
        match self.inner.lock().ok().and_then(|m| m.get(path).map(|r| r.in_keychain)) {
            Some(true) => PassphraseMemory::Keychain,
            _ => PassphraseMemory::Session,
        }
    }

    /// Remembers for this session only.
    pub fn remember(&self, path: &str, passphrase: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(path.to_string(), Remembered { secret: Zeroizing::new(passphrase.to_string()), in_keychain: false });
        }
    }

    /// Remembers as `memory` asks. A keychain that cannot store it still leaves the passphrase
    /// remembered for the session, and the error says so.
    pub fn store(&self, path: &str, passphrase: &str, memory: PassphraseMemory) -> Result<()> {
        match memory {
            PassphraseMemory::None => Ok(()),
            PassphraseMemory::Session => {
                self.remember(path, passphrase);
                Ok(())
            }
            PassphraseMemory::Keychain => {
                self.remember(path, passphrase);
                let Some(keychain) = &self.keychain else {
                    return Err(GitError::internal("this passphrase cache has no keychain"));
                };
                keychain.set(path, passphrase).map_err(|e| GitError::Io {
                    detail: format!("the passphrase is remembered until StashPanda closes, but could not be stored in the keychain ({e})"),
                })?;
                if let Ok(mut m) = self.inner.lock() {
                    if let Some(r) = m.get_mut(path) {
                        r.in_keychain = true;
                    }
                }
                Ok(())
            }
        }
    }

    /// Forgets the passphrase in memory and removes it from the keychain.
    pub fn forget(&self, path: &str) -> Result<()> {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(path);
        }
        match &self.keychain {
            Some(k) => k.delete(path),
            None => Ok(()),
        }
    }

    pub fn keychain_status(&self) -> Option<KeychainStatus> {
        self.keychain.as_ref().map(|k| k.status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::keychain::MemoryStore;

    #[test]
    fn session_and_keychain_memory() {
        let store = Arc::new(MemoryStore::default());
        let cache = PassphraseCache::with_keychain(store.clone());
        assert_eq!(cache.memory("/k/a"), PassphraseMemory::None);

        cache.store("/k/a", "one", PassphraseMemory::Session).unwrap();
        assert_eq!(cache.memory("/k/a"), PassphraseMemory::Session);
        assert!(store.get("/k/a").is_none(), "a session passphrase never reaches the keychain");

        cache.store("/k/b", "two", PassphraseMemory::Keychain).unwrap();
        assert_eq!(cache.memory("/k/b"), PassphraseMemory::Keychain);
        assert_eq!(store.get("/k/b").as_deref().map(String::as_str), Some("two"));

        // A new session finds the keychain entry and reports where it came from.
        let next = PassphraseCache::with_keychain(store.clone());
        assert_eq!(next.get("/k/b").as_deref().map(String::as_str), Some("two"));
        assert_eq!(next.memory("/k/b"), PassphraseMemory::Keychain);
        assert_eq!(next.memory("/k/a"), PassphraseMemory::None);

        next.forget("/k/b").unwrap();
        assert!(store.get("/k/b").is_none());
        assert_eq!(next.memory("/k/b"), PassphraseMemory::None);

        cache.store("/k/c", "three", PassphraseMemory::None).unwrap();
        assert_eq!(cache.memory("/k/c"), PassphraseMemory::None);
    }

    #[test]
    fn without_a_keychain_only_the_session_remembers() {
        let cache = PassphraseCache::default();
        assert!(cache.store("/k/a", "one", PassphraseMemory::Keychain).is_err());
        assert_eq!(cache.memory("/k/a"), PassphraseMemory::Session, "the session copy is kept");
        cache.forget("/k/a").unwrap();
        assert!(cache.get("/k/a").is_none());
    }
}
