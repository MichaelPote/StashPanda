//! SSH key management: read any private key format, inspect, convert,
//! export and generate. The heavy lifting is `ssh-key` (OpenSSH, PPK v2/v3,
//! PKCS#8) and `russh` (PEM decoding); `pem_legacy` covers what they do not.

pub mod agent;
pub mod detect;
pub mod pem_legacy;
pub mod secrets;

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};
use specta::Type;
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey, PublicKey};
use zeroize::Zeroizing;

use crate::error::{GitError, Result};
use crate::platform::Host;
use detect::{detect, Detected, KeyFormat};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    pub path: String,
    pub file_name: String,
    pub format: KeyFormat,
    pub encrypted: bool,
    pub algorithm: Option<String>,
    pub bits: Option<u32>,
    /// `SHA256:...` of the public key, when it can be read without a passphrase.
    pub fingerprint: Option<String>,
    pub comment: Option<String>,
    /// Sibling `.pub` file, if present.
    pub public_key_file: Option<String>,
    pub error: Option<String>,
    /// Filled by the caller from the agent's identity list.
    pub in_agent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    OpenSsh,
    Pkcs8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum GenerateAlgorithm {
    Ed25519,
    Rsa4096,
}

fn parse_err(path: &str, format: &str, e: impl std::fmt::Display) -> GitError {
    GitError::KeyParseError { path: path.to_string(), format: format.into(), detail: e.to_string() }
}

/// Load a private key of any supported format. `path` is only used in errors.
pub fn load_private_key(path: &str, bytes: &[u8], passphrase: Option<&str>) -> Result<PrivateKey> {
    let d = detect(bytes);
    let text = String::from_utf8_lossy(bytes);
    match d.format {
        KeyFormat::PpkV2 | KeyFormat::PpkV3 => {
            if d.encrypted == Some(true) && passphrase.is_none() {
                return Err(GitError::SshNeedsPassphrase { path: path.into() });
            }
            PrivateKey::from_ppk(text.as_ref(), passphrase.map(String::from)).map_err(|e| {
                let msg = e.to_string().to_ascii_lowercase();
                if passphrase.is_some() && (msg.contains("mac") || msg.contains("incorrect") || msg.contains("crypto")) {
                    GitError::SshWrongPassphrase { path: path.into() }
                } else if msg.contains("encrypted") {
                    GitError::SshNeedsPassphrase { path: path.into() }
                } else {
                    parse_err(path, "PPK", e)
                }
            })
        }
        KeyFormat::OpenSsh => {
            let key = PrivateKey::from_openssh(bytes).map_err(|e| parse_err(path, "OpenSSH", e))?;
            if !key.is_encrypted() {
                return Ok(key);
            }
            let Some(p) = passphrase else {
                return Err(GitError::SshNeedsPassphrase { path: path.into() });
            };
            key.decrypt(p.as_bytes()).map_err(|_| GitError::SshWrongPassphrase { path: path.into() })
        }
        KeyFormat::PemRsa | KeyFormat::PemEc | KeyFormat::PemDsa if d.encrypted == Some(true) => {
            pem_legacy::load_rsa(path, &text, passphrase)
        }
        KeyFormat::PemRsa | KeyFormat::PemEc | KeyFormat::PemDsa | KeyFormat::Pkcs8 | KeyFormat::Pkcs8Encrypted => {
            if d.format == KeyFormat::Pkcs8Encrypted && passphrase.is_none() {
                return Err(GitError::SshNeedsPassphrase { path: path.into() });
            }
            russh::keys::decode_secret_key(&text, passphrase).map_err(|e| match e {
                russh::keys::Error::KeyIsEncrypted => GitError::SshNeedsPassphrase { path: path.into() },
                other => {
                    if d.format == KeyFormat::Pkcs8Encrypted {
                        GitError::SshWrongPassphrase { path: path.into() }
                    } else {
                        parse_err(path, "PEM", other)
                    }
                }
            })
        }
        KeyFormat::PublicOpenSsh | KeyFormat::PublicRfc4716 => Err(GitError::SshKeyUnsupported {
            path: path.into(),
            detected_header: format!("{} (this is a public key)", d.header),
        }),
        KeyFormat::Unknown => Err(GitError::SshKeyUnsupported { path: path.into(), detected_header: d.header }),
    }
}

fn algorithm_name(a: &Algorithm) -> String {
    a.as_str().to_string()
}

fn key_bits(key: &PublicKey) -> Option<u32> {
    use ssh_key::public::KeyData;
    match key.key_data() {
        KeyData::Rsa(r) => Some((r.n().as_positive_bytes().unwrap_or(&[]).len() * 8) as u32),
        KeyData::Ed25519(_) => Some(256),
        KeyData::Ecdsa(e) => Some(match e.curve() {
            ssh_key::EcdsaCurve::NistP256 => 256,
            ssh_key::EcdsaCurve::NistP384 => 384,
            ssh_key::EcdsaCurve::NistP521 => 521,
        }),
        KeyData::Dsa(_) => Some(1024),
        _ => None,
    }
}

/// Public key of a PPK without its passphrase: the `Public-Lines` block is
/// the SSH wire encoding.
fn ppk_public(text: &str) -> Option<PublicKey> {
    let mut lines = text.lines();
    let mut comment = None;
    while let Some(l) = lines.next() {
        if let Some(c) = l.strip_prefix("Comment:") {
            comment = Some(c.trim().to_string());
        }
        if let Some(n) = l.strip_prefix("Public-Lines:") {
            let n: usize = n.trim().parse().ok()?;
            let b64: String = lines.by_ref().take(n).map(str::trim).collect();
            let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).ok()?;
            let mut pk = PublicKey::from_bytes(&bytes).ok()?;
            if let Some(c) = comment {
                pk.set_comment(c);
            }
            return Some(pk);
        }
    }
    None
}

fn fill_from_public(info: &mut KeyInfo, pk: &PublicKey) {
    info.algorithm = Some(algorithm_name(&pk.algorithm()));
    info.bits = key_bits(pk);
    info.fingerprint = Some(pk.fingerprint(HashAlg::Sha256).to_string());
    let c = pk.comment().to_string();
    if !c.is_empty() {
        info.comment = Some(c);
    }
}

/// Everything we can say about a key file without a passphrase.
pub fn inspect(path: &Path) -> KeyInfo {
    let path_s = path.to_string_lossy().into_owned();
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut info = KeyInfo {
        path: path_s.clone(),
        file_name,
        format: KeyFormat::Unknown,
        encrypted: false,
        algorithm: None,
        bits: None,
        fingerprint: None,
        comment: None,
        public_key_file: None,
        error: None,
        in_agent: false,
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            info.error = Some(e.to_string());
            return info;
        }
    };
    let d: Detected = detect(&bytes);
    info.format = d.format;
    info.encrypted = d.encrypted.unwrap_or(false);
    info.algorithm = d.algorithm_hint.clone();
    let pub_path = PathBuf::from(format!("{path_s}.pub"));
    if pub_path.is_file() {
        info.public_key_file = Some(pub_path.to_string_lossy().into_owned());
    }
    let text = String::from_utf8_lossy(&bytes);
    match load_private_key(&path_s, &bytes, None) {
        Ok(key) => {
            fill_from_public(&mut info, key.public_key());
            let c = key.comment().to_string();
            if !c.is_empty() {
                info.comment = Some(c);
            }
        }
        Err(GitError::SshNeedsPassphrase { .. }) => {
            info.encrypted = true;
            match d.format {
                KeyFormat::PpkV2 | KeyFormat::PpkV3 => {
                    if let Some(pk) = ppk_public(&text) {
                        fill_from_public(&mut info, &pk);
                    }
                }
                KeyFormat::OpenSsh => {
                    if let Ok(k) = PrivateKey::from_openssh(&bytes) {
                        fill_from_public(&mut info, k.public_key());
                    }
                }
                _ => {
                    if let Some(p) = &info.public_key_file {
                        if let Ok(pk) = PublicKey::read_openssh_file(Path::new(p)) {
                            fill_from_public(&mut info, &pk);
                        }
                    }
                }
            }
        }
        Err(e) => info.error = Some(e.to_string()),
    }
    info
}

const SKIP_NAMES: &[&str] = &["known_hosts", "known_hosts.old", "config", "authorized_keys", "environment", "rc"];

/// Every private key candidate in `dirs` (public-only files are skipped;
/// they are reported through `public_key_file` on their private twin).
pub fn scan(dirs: &[PathBuf]) -> Vec<KeyInfo> {
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        paths.sort();
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if SKIP_NAMES.contains(&name.as_str()) || name.ends_with(".pub") {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&p) else { continue };
            if meta.len() > 64 * 1024 {
                continue;
            }
            let Ok(bytes) = std::fs::read(&p) else { continue };
            let d = detect(&bytes);
            if matches!(d.format, KeyFormat::Unknown | KeyFormat::PublicOpenSsh | KeyFormat::PublicRfc4716) {
                continue;
            }
            out.push(inspect(&p));
        }
    }
    out
}

/// Where keys usually live on this machine, including the other side of the
/// Windows/WSL boundary when it is reachable.
pub fn default_key_dirs(host: &Host) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        dirs.push(PathBuf::from(home).join(".ssh"));
    }
    if let Host::LinuxWsl { .. } = host {
        if let Ok(rd) = std::fs::read_dir("/mnt/c/Users") {
            for e in rd.flatten() {
                let p = e.path().join(".ssh");
                if p.is_dir() {
                    dirs.push(p);
                }
            }
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

/// Serialise a private key, optionally encrypted with a new passphrase.
pub fn export_private(key: &PrivateKey, format: ExportFormat, passphrase: Option<&str>) -> Result<Zeroizing<String>> {
    match format {
        ExportFormat::OpenSsh => {
            let k = match passphrase.filter(|p| !p.is_empty()) {
                Some(p) => key
                    .encrypt(&mut ssh_key::getrandom::SysRng, p.as_bytes())
                    .map_err(|e| GitError::internal(format!("could not encrypt key: {e}")))?,
                None => key.clone(),
            };
            k.to_openssh(LineEnding::LF).map_err(|e| GitError::internal(format!("could not encode key: {e}")))
        }
        ExportFormat::Pkcs8 => {
            let mut out = Vec::new();
            match passphrase.filter(|p| !p.is_empty()) {
                Some(p) => russh::keys::encode_pkcs8_pem_encrypted(key, p.as_bytes(), 100_000, &mut out),
                None => russh::keys::encode_pkcs8_pem(key, &mut out),
            }
            .map_err(|e| GitError::internal(format!("could not encode PKCS#8: {e}")))?;
            Ok(Zeroizing::new(String::from_utf8_lossy(&out).into_owned()))
        }
    }
}

/// One `authorized_keys` line.
pub fn public_line(key: &PrivateKey) -> Result<String> {
    key.public_key().to_openssh().map_err(|e| GitError::internal(format!("could not encode public key: {e}")))
}

pub fn generate(alg: GenerateAlgorithm, comment: &str) -> Result<PrivateKey> {
    let algorithm = match alg {
        GenerateAlgorithm::Ed25519 => Algorithm::Ed25519,
        GenerateAlgorithm::Rsa4096 => Algorithm::Rsa { hash: None },
    };
    let mut rng = ssh_key::rand_core::UnwrapErr(ssh_key::getrandom::SysRng);
    let mut key = PrivateKey::random(&mut rng, algorithm)
        .map_err(|e| GitError::internal(format!("could not generate key: {e}")))?;
    if !comment.is_empty() {
        key.set_comment(comment);
    }
    Ok(key)
}

/// Write a private key file with owner-only permissions. Never overwrites
/// unless asked.
pub fn write_private_file(path: &Path, contents: &str, overwrite: bool) -> Result<()> {
    if path.exists() && !overwrite {
        return Err(GitError::Io { detail: format!("{} already exists", path.display()) });
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_export_reload_round_trip() {
        let key = generate(GenerateAlgorithm::Ed25519, "michael@test").unwrap();
        let plain = export_private(&key, ExportFormat::OpenSsh, None).unwrap();
        let back = load_private_key("mem", plain.as_bytes(), None).unwrap();
        assert_eq!(back.fingerprint(HashAlg::Sha256), key.fingerprint(HashAlg::Sha256));
        assert_eq!(back.comment().to_string(), "michael@test");

        let enc = export_private(&key, ExportFormat::OpenSsh, Some("pw")).unwrap();
        assert!(matches!(load_private_key("mem", enc.as_bytes(), None), Err(GitError::SshNeedsPassphrase { .. })));
        assert!(matches!(load_private_key("mem", enc.as_bytes(), Some("nope")), Err(GitError::SshWrongPassphrase { .. })));
        let dec = load_private_key("mem", enc.as_bytes(), Some("pw")).unwrap();
        assert_eq!(dec.fingerprint(HashAlg::Sha256), key.fingerprint(HashAlg::Sha256));

        let p8 = export_private(&key, ExportFormat::Pkcs8, None).unwrap();
        assert!(p8.starts_with("-----BEGIN PRIVATE KEY-----"));
        let back8 = load_private_key("mem", p8.as_bytes(), None).unwrap();
        assert_eq!(back8.fingerprint(HashAlg::Sha256), key.fingerprint(HashAlg::Sha256));
        let p8e = export_private(&key, ExportFormat::Pkcs8, Some("pw")).unwrap();
        assert!(p8e.starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----"));
        assert!(matches!(load_private_key("mem", p8e.as_bytes(), None), Err(GitError::SshNeedsPassphrase { .. })));
        let back8e = load_private_key("mem", p8e.as_bytes(), Some("pw")).unwrap();
        assert_eq!(back8e.fingerprint(HashAlg::Sha256), key.fingerprint(HashAlg::Sha256));

        assert!(public_line(&key).unwrap().starts_with("ssh-ed25519 "));
        assert!(matches!(load_private_key("mem", b"ssh-ed25519 AAAA", None), Err(GitError::SshKeyUnsupported { .. })));
    }
}
