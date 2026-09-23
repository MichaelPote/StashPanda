//! Passphrases remembered for this process only, wiped on drop.

use std::collections::HashMap;
use std::sync::Mutex;

use zeroize::Zeroizing;

#[derive(Default)]
pub struct PassphraseCache {
    inner: Mutex<HashMap<String, Zeroizing<String>>>,
}

impl PassphraseCache {
    pub fn get(&self, path: &str) -> Option<Zeroizing<String>> {
        self.inner.lock().ok()?.get(path).cloned()
    }
    pub fn remember(&self, path: &str, passphrase: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(path.to_string(), Zeroizing::new(passphrase.to_string()));
        }
    }
    pub fn forget(&self, path: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(path);
        }
    }
}
