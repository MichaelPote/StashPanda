//! Recognise a key file by its content. Extensions lie (several `.pem` files
//! in the wild are OpenSSH format), so only the header counts.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum KeyFormat {
    PpkV2,
    PpkV3,
    OpenSsh,
    /// `BEGIN RSA PRIVATE KEY` (PKCS#1), possibly OpenSSL-encrypted.
    PemRsa,
    /// `BEGIN EC PRIVATE KEY` (SEC1).
    PemEc,
    /// `BEGIN DSA PRIVATE KEY`.
    PemDsa,
    Pkcs8,
    Pkcs8Encrypted,
    PublicOpenSsh,
    PublicRfc4716,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub format: KeyFormat,
    /// The first line, for error messages.
    pub header: String,
    /// Known from the header alone (PPK, legacy PEM, PKCS#8); None = must parse.
    pub encrypted: Option<bool>,
    /// PPK headers name the algorithm.
    pub algorithm_hint: Option<String>,
}

pub fn detect(bytes: &[u8]) -> Detected {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let header = text.lines().next().unwrap_or("").trim().to_string();
    let mut d = Detected { format: KeyFormat::Unknown, header: header.clone(), encrypted: None, algorithm_hint: None };
    if let Some(rest) = header.strip_prefix("PuTTY-User-Key-File-") {
        let (ver, alg) = rest.split_once(':').unwrap_or((rest, ""));
        d.format = if ver.trim() == "3" { KeyFormat::PpkV3 } else { KeyFormat::PpkV2 };
        d.algorithm_hint = Some(alg.trim().to_string()).filter(|s| !s.is_empty());
        d.encrypted = Some(text.lines().any(|l| l.starts_with("Encryption:") && !l.contains("none")));
        return d;
    }
    let legacy_encrypted = text.lines().any(|l| l.starts_with("Proc-Type: 4,ENCRYPTED"));
    match header.as_str() {
        "-----BEGIN OPENSSH PRIVATE KEY-----" => d.format = KeyFormat::OpenSsh,
        "-----BEGIN RSA PRIVATE KEY-----" => {
            d.format = KeyFormat::PemRsa;
            d.encrypted = Some(legacy_encrypted);
        }
        "-----BEGIN EC PRIVATE KEY-----" => {
            d.format = KeyFormat::PemEc;
            d.encrypted = Some(legacy_encrypted);
        }
        "-----BEGIN DSA PRIVATE KEY-----" => {
            d.format = KeyFormat::PemDsa;
            d.encrypted = Some(legacy_encrypted);
        }
        "-----BEGIN PRIVATE KEY-----" => {
            d.format = KeyFormat::Pkcs8;
            d.encrypted = Some(false);
        }
        "-----BEGIN ENCRYPTED PRIVATE KEY-----" => {
            d.format = KeyFormat::Pkcs8Encrypted;
            d.encrypted = Some(true);
        }
        "---- BEGIN SSH2 PUBLIC KEY ----" => d.format = KeyFormat::PublicRfc4716,
        h if h.starts_with("ssh-") || h.starts_with("ecdsa-sha2-") || h.starts_with("sk-") => {
            d.format = KeyFormat::PublicOpenSsh
        }
        _ => {}
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_by_header_not_extension() {
        assert_eq!(detect(b"PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none\n").format, KeyFormat::PpkV3);
        let e = detect(b"PuTTY-User-Key-File-2: ssh-rsa\nEncryption: aes256-cbc\n");
        assert_eq!((e.format, e.encrypted, e.algorithm_hint.as_deref()), (KeyFormat::PpkV2, Some(true), Some("ssh-rsa")));
        let l = detect(b"-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: DES-EDE3-CBC,00\n");
        assert_eq!((l.format, l.encrypted), (KeyFormat::PemRsa, Some(true)));
        assert_eq!(detect(b"-----BEGIN OPENSSH PRIVATE KEY-----\n").format, KeyFormat::OpenSsh);
        assert_eq!(detect(b"ssh-ed25519 AAAA test").format, KeyFormat::PublicOpenSsh);
        assert_eq!(detect(b"---- BEGIN SSH2 PUBLIC KEY ----\n").format, KeyFormat::PublicRfc4716);
        assert_eq!(detect(b"hello").format, KeyFormat::Unknown);
    }
}
