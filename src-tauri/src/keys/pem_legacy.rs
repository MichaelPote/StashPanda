//! Traditional OpenSSL-encrypted PEM (`Proc-Type: 4,ENCRYPTED` with a
//! `DEK-Info` line): key derived with the MD5 EVP_BytesToKey scheme, body
//! decrypted with the named cipher in CBC mode. Only the RSA (PKCS#1) body
//! is converted; EC and DSA legacy keys are reported as unsupported.

use aes::cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};
use base64::Engine;
use md5::{Digest, Md5};
use zeroize::Zeroizing;

use crate::error::{GitError, Result};

pub struct LegacyPem {
    pub cipher: String,
    pub iv: Vec<u8>,
    pub body: Vec<u8>,
    pub label: String,
}

pub fn parse(text: &str) -> Option<LegacyPem> {
    let mut lines = text.lines().map(str::trim);
    let first = lines.next()?;
    let label = first.strip_prefix("-----BEGIN ")?.strip_suffix("-----")?.to_string();
    let mut cipher = None;
    let mut iv = None;
    let mut b64 = String::new();
    let mut in_body = false;
    for l in lines {
        if l.starts_with("-----END") {
            break;
        }
        if !in_body {
            if let Some(v) = l.strip_prefix("DEK-Info:") {
                let (c, ivhex) = v.trim().split_once(',')?;
                cipher = Some(c.trim().to_ascii_uppercase());
                iv = Some(hex(ivhex.trim())?);
                continue;
            }
            if l.starts_with("Proc-Type:") {
                continue;
            }
            if l.is_empty() {
                in_body = true;
                continue;
            }
            in_body = true;
        }
        b64.push_str(l);
    }
    let body = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).ok()?;
    Some(LegacyPem { cipher: cipher?, iv: iv?, body, label })
}

fn hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// OpenSSL's EVP_BytesToKey with MD5, one iteration, salt = first 8 IV bytes.
fn derive_key(password: &[u8], salt: &[u8], key_len: usize) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(key_len + 16));
    let mut prev: Vec<u8> = Vec::new();
    while out.len() < key_len {
        let mut h = Md5::new();
        h.update(&prev);
        h.update(password);
        h.update(salt);
        prev = h.finalize().to_vec();
        out.extend_from_slice(&prev);
    }
    out.truncate(key_len);
    out
}

fn wrong_passphrase(path: &str) -> GitError {
    GitError::SshWrongPassphrase { path: path.to_string() }
}

/// Decrypt to the DER body.
pub fn decrypt(path: &str, pem: &LegacyPem, passphrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    let salt = pem.iv.get(..8).ok_or_else(|| GitError::KeyParseError {
        path: path.to_string(),
        format: "legacy PEM".into(),
        detail: "IV shorter than 8 bytes".into(),
    })?;
    let (key_len, iv_len) = match pem.cipher.as_str() {
        "AES-128-CBC" => (16, 16),
        "AES-192-CBC" => (24, 16),
        "AES-256-CBC" => (32, 16),
        "DES-EDE3-CBC" => (24, 8),
        other => {
            return Err(GitError::SshKeyUnsupported {
                path: path.to_string(),
                detected_header: format!("DEK-Info: {other}"),
            })
        }
    };
    if pem.iv.len() != iv_len {
        return Err(GitError::KeyParseError {
            path: path.to_string(),
            format: "legacy PEM".into(),
            detail: format!("IV has {} bytes, {} expected", pem.iv.len(), iv_len),
        });
    }
    let key = derive_key(passphrase.as_bytes(), salt, key_len);
    let mut buf = Zeroizing::new(pem.body.clone());
    let n = match pem.cipher.as_str() {
        "AES-128-CBC" => cbc::Decryptor::<aes::Aes128>::new_from_slices(&key, &pem.iv)
            .map_err(|_| wrong_passphrase(path))?
            .decrypt_padded::<Pkcs7>(&mut buf)
            .map_err(|_| wrong_passphrase(path))?
            .len(),
        "AES-192-CBC" => cbc::Decryptor::<aes::Aes192>::new_from_slices(&key, &pem.iv)
            .map_err(|_| wrong_passphrase(path))?
            .decrypt_padded::<Pkcs7>(&mut buf)
            .map_err(|_| wrong_passphrase(path))?
            .len(),
        "AES-256-CBC" => cbc::Decryptor::<aes::Aes256>::new_from_slices(&key, &pem.iv)
            .map_err(|_| wrong_passphrase(path))?
            .decrypt_padded::<Pkcs7>(&mut buf)
            .map_err(|_| wrong_passphrase(path))?
            .len(),
        _ => cbc::Decryptor::<des::TdesEde3>::new_from_slices(&key, &pem.iv)
            .map_err(|_| wrong_passphrase(path))?
            .decrypt_padded::<Pkcs7>(&mut buf)
            .map_err(|_| wrong_passphrase(path))?
            .len(),
    };
    buf.truncate(n);
    let plain: Vec<u8> = buf.to_vec();
    Ok(Zeroizing::new(plain))
}

/// Decrypt and convert a legacy RSA PEM into an `ssh_key::PrivateKey`.
pub fn load_rsa(path: &str, text: &str, passphrase: Option<&str>) -> Result<ssh_key::PrivateKey> {
    let pem = parse(text).ok_or_else(|| GitError::KeyParseError {
        path: path.to_string(),
        format: "legacy PEM".into(),
        detail: "malformed PEM envelope".into(),
    })?;
    if pem.label != "RSA PRIVATE KEY" {
        return Err(GitError::SshKeyUnsupported {
            path: path.to_string(),
            detected_header: format!("encrypted {} (convert it once with ssh-keygen -p -m RFC4716)", pem.label),
        });
    }
    let Some(pass) = passphrase else {
        return Err(GitError::SshNeedsPassphrase { path: path.to_string() });
    };
    let der = decrypt(path, &pem, pass)?;
    use pkcs1::DecodeRsaPrivateKey;
    let rsa_key = rsa::RsaPrivateKey::from_pkcs1_der(&der).map_err(|_| wrong_passphrase(path))?;
    let pair = ssh_key::private::RsaKeypair::try_from(rsa_key).map_err(|e| GitError::KeyParseError {
        path: path.to_string(),
        format: "legacy PEM".into(),
        detail: e.to_string(),
    })?;
    Ok(ssh_key::PrivateKey::from(pair))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_openssl_key_material() {
        // Known answer: EVP_BytesToKey(md5, salt=0x0102030405060708, "secret", 24 bytes)
        // equals MD5(pass+salt) || MD5(MD5(pass+salt)+pass+salt)[..8].
        let salt = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let k = derive_key(b"secret", &salt, 24);
        let mut h = Md5::new();
        h.update(b"secret");
        h.update(salt);
        let d0 = h.finalize();
        assert_eq!(&k[..16], &d0[..]);
        assert_eq!(k.len(), 24);
    }

    #[test]
    fn parses_envelope() {
        let p = parse("-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: DES-EDE3-CBC,0102030405060708\n\nAAAA\n-----END RSA PRIVATE KEY-----\n").unwrap();
        assert_eq!(p.cipher, "DES-EDE3-CBC");
        assert_eq!(p.iv, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(p.body, vec![0, 0, 0]);
    }
}
