//! Passphrases kept in the operating system's credential store through the
//! `keyring` crate: Windows Credential Manager on Windows, the Secret Service
//! (GNOME Keyring, KWallet) on Linux. Inside WSL there is usually no Secret
//! Service on the session bus, so the store reports itself unavailable and
//! passphrases are only remembered for the session.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use specta::Type;
use zeroize::Zeroizing;

use crate::error::{GitError, Result};

/// The service name every StashPanda entry is filed under; the key file's path is the user name.
const SERVICE: &str = "StashPanda SSH key passphrase";

/// Whether the system keychain can be used, and what it is called on this platform.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct KeychainStatus {
    pub available: bool,
    /// "Windows Credential Manager", "Secret Service" or "macOS Keychain".
    pub name: String,
    /// Why it is not available, when it is not.
    pub detail: Option<String>,
}

/// Where passphrases can be stored beyond the session. `OsKeychain` is the real one; tests use
/// an in-memory store.
pub trait SecretStore: Send + Sync {
    fn status(&self) -> KeychainStatus;
    /// The stored passphrase, or None when there is none or the store cannot be reached.
    fn get(&self, path: &str) -> Option<Zeroizing<String>>;
    fn set(&self, path: &str, passphrase: &str) -> Result<()>;
    /// Removes the stored passphrase. Succeeds when there was none.
    fn delete(&self, path: &str) -> Result<()>;
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows Credential Manager"
    } else if cfg!(target_os = "macos") {
        "macOS Keychain"
    } else {
        "Secret Service"
    }
}

fn keychain_err(e: keyring::Error) -> GitError {
    GitError::Io { detail: format!("{}: {e}", platform_name()) }
}

/// The operating system's store. Whether it works is found out once, on first use, by reading
/// an entry that does not exist: "no such entry" means the store answered.
#[derive(Default)]
pub struct OsKeychain {
    probe: OnceLock<std::result::Result<(), String>>,
}

impl OsKeychain {
    fn probe(&self) -> &std::result::Result<(), String> {
        self.probe.get_or_init(|| {
            if let Err(e) = keyring::Entry::store_status() {
                return Err(e.to_string());
            }
            let entry = keyring::Entry::new(SERVICE, "availability check").map_err(|e| e.to_string())?;
            match entry.get_password() {
                Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        })
    }

    fn entry(&self, path: &str) -> Result<keyring::Entry> {
        if let Err(detail) = self.probe() {
            return Err(GitError::Io { detail: format!("{} is not available: {detail}", platform_name()) });
        }
        keyring::Entry::new(SERVICE, path).map_err(keychain_err)
    }
}

impl SecretStore for OsKeychain {
    fn status(&self) -> KeychainStatus {
        let probe = self.probe();
        KeychainStatus { available: probe.is_ok(), name: platform_name().into(), detail: probe.clone().err() }
    }

    fn get(&self, path: &str) -> Option<Zeroizing<String>> {
        self.entry(path).ok()?.get_password().ok().map(Zeroizing::new)
    }

    fn set(&self, path: &str, passphrase: &str) -> Result<()> {
        self.entry(path)?.set_password(passphrase).map_err(keychain_err)
    }

    fn delete(&self, path: &str) -> Result<()> {
        if self.probe().is_err() {
            return Ok(());
        }
        match self.entry(path)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_err(e)),
        }
    }
}

/// A store that keeps everything in memory, standing in for the keychain in tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    pub entries: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn status(&self) -> KeychainStatus {
        KeychainStatus { available: true, name: "test store".into(), detail: None }
    }
    fn get(&self, path: &str) -> Option<Zeroizing<String>> {
        self.entries.lock().unwrap().get(path).cloned().map(Zeroizing::new)
    }
    fn set(&self, path: &str, passphrase: &str) -> Result<()> {
        self.entries.lock().unwrap().insert(path.into(), passphrase.into());
        Ok(())
    }
    fn delete(&self, path: &str) -> Result<()> {
        self.entries.lock().unwrap().remove(path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only reports what this machine has: inside WSL there is usually no Secret Service, and
    /// the check must then say so rather than fail or hang.
    #[test]
    fn os_keychain_status_answers() {
        let status = OsKeychain::default().status();
        eprintln!("keychain: {status:?}");
        assert_eq!(status.available, status.detail.is_none());
    }
}
