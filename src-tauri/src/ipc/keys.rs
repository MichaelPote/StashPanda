//! SSH key manager commands.

use std::path::{Path, PathBuf};

use tauri::State;
use zeroize::Zeroizing;

use crate::error::{GitError, Result};
use crate::keys::keychain::KeychainStatus;
use crate::keys::secrets::{PassphraseCache, PassphraseMemory};
use crate::keys::{self, ExportFormat, GenerateAlgorithm, KeyInfo};
use crate::platform::Host;
use crate::state::AppState;

fn read_key(state: &AppState, path: &str, passphrase: Option<&str>) -> Result<ssh_key::PrivateKey> {
    read_key_with_passphrase(state, path, passphrase).map(|(key, _)| key)
}

/// The key, and the passphrase that decrypted it (given, or remembered) when it is encrypted.
fn read_key_with_passphrase(state: &AppState, path: &str, passphrase: Option<&str>) -> Result<(ssh_key::PrivateKey, Option<Zeroizing<String>>)> {
    let bytes = std::fs::read(path)?;
    match keys::load_private_key(path, &bytes, None) {
        Err(GitError::SshNeedsPassphrase { .. }) => {}
        other => return other.map(|key| (key, None)),
    }
    let pass = match passphrase {
        Some(p) => Zeroizing::new(p.to_string()),
        None => state.passphrases.get(path).ok_or_else(|| GitError::SshNeedsPassphrase { path: path.to_string() })?,
    };
    let key = keys::load_private_key(path, &bytes, Some(pass.as_str()))?;
    Ok((key, Some(pass)))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| GitError::internal(e.to_string()))
}

/// Where an encrypted key's passphrase is remembered. Looks in the keychain, so it runs off the
/// async threads.
fn fill_memory(cache: &PassphraseCache, info: &mut KeyInfo) {
    if info.encrypted {
        info.passphrase_remembered = cache.memory(&info.path);
    }
}

/// The default folders. On Windows these include the home folders of the running WSL distros.
pub(crate) async fn default_dirs(host: &Host) -> Vec<PathBuf> {
    let distros = match host {
        Host::Windows => crate::transport::wsl::list_running_distros().await.unwrap_or_default(),
        _ => Vec::new(),
    };
    let host = host.clone();
    blocking(move || keys::default_key_dirs(&host, &distros)).await.unwrap_or_default()
}

/// Every key in the default folders plus `extra_dirs`, with agent presence and where each
/// encrypted key's passphrase is remembered.
#[tauri::command]
#[specta::specta]
pub async fn keys_scan(state: State<'_, AppState>, extra_dirs: Vec<String>) -> Result<Vec<KeyInfo>> {
    let mut dirs = default_dirs(&state.host).await;
    dirs.extend(extra_dirs.into_iter().filter(|d| !d.trim().is_empty()).map(PathBuf::from));
    let dirs = keys::unique_paths(dirs);
    let cache = state.passphrases.clone();
    let mut infos = blocking(move || {
        let mut infos = keys::scan(&dirs);
        for i in &mut infos {
            fill_memory(&cache, i);
        }
        infos
    })
    .await?;
    crate::ipc::key_places::fill_places(&state.host, &mut infos).await;
    Ok(infos)
}

#[tauri::command]
#[specta::specta]
pub async fn keys_inspect(state: State<'_, AppState>, path: String) -> Result<KeyInfo> {
    let cache = state.passphrases.clone();
    blocking(move || {
        let mut info = keys::inspect(Path::new(&path));
        fill_memory(&cache, &mut info);
        info
    })
    .await
}

/// Whether passphrases can be stored in the system keychain on this machine.
#[tauri::command]
#[specta::specta]
pub async fn keys_keychain_status(state: State<'_, AppState>) -> Result<KeychainStatus> {
    let cache = state.passphrases.clone();
    blocking(move || cache.keychain_status())
        .await?
        .ok_or_else(|| GitError::internal("the passphrase cache has no keychain"))
}

/// Check a passphrase and remember it as `remember` says: not at all, until StashPanda closes, or
/// in the system keychain as well. The returned `passphraseRemembered` says where it ended up: a
/// keychain that cannot store it leaves it remembered for the session only.
#[tauri::command]
#[specta::specta]
pub async fn keys_unlock(state: State<'_, AppState>, path: String, passphrase: String, remember: PassphraseMemory) -> Result<KeyInfo> {
    let bytes = std::fs::read(&path)?;
    let key = keys::load_private_key(&path, &bytes, Some(&passphrase))?;
    let cache = state.passphrases.clone();
    blocking(move || {
        if let Err(e) = cache.store(&path, &passphrase, remember) {
            tracing::warn!("{e}");
        }
        let mut info = keys::inspect(Path::new(&path));
        info.fingerprint = Some(key.fingerprint(ssh_key::HashAlg::Sha256).to_string());
        info.algorithm = Some(key.algorithm().as_str().to_string());
        info.passphrase_remembered = cache.memory(&path);
        info
    })
    .await
}

/// Forget the passphrase for this session and remove it from the system keychain.
#[tauri::command]
#[specta::specta]
pub async fn keys_forget_passphrase(state: State<'_, AppState>, path: String) -> Result<()> {
    let cache = state.passphrases.clone();
    blocking(move || cache.forget(&path)).await?
}

/// The `authorized_keys` line. Encrypted keys need no passphrase when the file holds its public
/// key in the clear (OpenSSH, PPK) or has a `.pub` beside it.
#[tauri::command]
#[specta::specta]
pub async fn keys_export_public(state: State<'_, AppState>, path: String, passphrase: Option<String>) -> Result<String> {
    if let Some(public) = keys::public_key_of(Path::new(&path)) {
        return public.to_openssh().map_err(|e| GitError::internal(format!("could not encode public key: {e}")));
    }
    let key = read_key(&state, &path, passphrase.as_deref())?;
    keys::public_line(&key)
}

/// Convert to OpenSSH or PKCS#8 next to the original (or at `out_path`).
/// `new_passphrase` encrypts the new file: None keeps the original's
/// protection (encrypted with the same passphrase, or not encrypted), an empty
/// string writes it unencrypted. Writes a `.pub` alongside. Returns the path
/// written. Without `out_path` the new file is named after the original
/// (`id_rsa.ppk` becomes `id_rsa`), with `_2`, `_3` and so on added when that
/// name is taken, so the original and other keys are never overwritten.
#[tauri::command]
#[specta::specta]
pub async fn keys_convert(
    state: State<'_, AppState>,
    path: String,
    format: ExportFormat,
    out_path: Option<String>,
    passphrase: Option<String>,
    new_passphrase: Option<String>,
    overwrite: bool,
) -> Result<String> {
    let (key, used) = read_key_with_passphrase(&state, &path, passphrase.as_deref())?;
    let new_passphrase = new_passphrase.map(Zeroizing::new).or(used);
    let out = match out_path.filter(|p| !p.trim().is_empty()) {
        Some(p) => PathBuf::from(p),
        None => {
            let src = Path::new(&path);
            let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "key".into());
            let ext = match format {
                ExportFormat::OpenSsh => "",
                ExportFormat::Pkcs8 => ".pem",
            };
            keys::unused_key_path(src.parent().unwrap_or(Path::new(".")), &stem, ext)
        }
    };
    if out == Path::new(&path) && !overwrite {
        return Err(GitError::Io { detail: "refusing to overwrite the original key; choose another name or tick overwrite".into() });
    }
    let text = keys::export_private(&key, format, new_passphrase.as_deref().map(String::as_str))?;
    keys::write_private_file(&out, &text, overwrite)?;
    let pub_line = keys::public_line(&key)?;
    let pub_path = PathBuf::from(format!("{}.pub", out.to_string_lossy()));
    if overwrite {
        std::fs::write(&pub_path, format!("{pub_line}\n"))?;
    } else {
        keys::write_public_file(&out, &pub_line)?;
    }
    Ok(out.to_string_lossy().into_owned())
}

#[tauri::command]
#[specta::specta]
pub async fn keys_generate(
    algorithm: GenerateAlgorithm,
    comment: String,
    out_path: String,
    passphrase: Option<String>,
    overwrite: bool,
) -> Result<KeyInfo> {
    let key = tokio::task::spawn_blocking(move || keys::generate(algorithm, &comment))
        .await
        .map_err(|e| GitError::internal(e.to_string()))??;
    let text = keys::export_private(&key, ExportFormat::OpenSsh, passphrase.as_deref())?;
    let out = Path::new(&out_path);
    keys::write_private_file(out, &text, overwrite)?;
    if overwrite {
        std::fs::write(format!("{out_path}.pub"), format!("{}\n", keys::public_line(&key)?))?;
    } else {
        keys::write_public_file(out, &keys::public_line(&key)?)?;
    }
    Ok(keys::inspect(out))
}

/// The comment the Generate form starts with: `user@machine`.
#[tauri::command]
#[specta::specta]
pub async fn keys_default_comment() -> Result<String> {
    Ok(keys::default_comment())
}

/// Re-encrypt an OpenSSH-format key in place with a new passphrase (or none). A passphrase that
/// was remembered is replaced by the new one in the same place (session or keychain).
#[tauri::command]
#[specta::specta]
pub async fn keys_change_passphrase(
    state: State<'_, AppState>,
    path: String,
    passphrase: Option<String>,
    new_passphrase: Option<String>,
) -> Result<KeyInfo> {
    let key = read_key(&state, &path, passphrase.as_deref())?;
    let d = keys::detect::detect(&std::fs::read(&path)?);
    if d.format != keys::detect::KeyFormat::OpenSsh {
        return Err(GitError::Io { detail: "only OpenSSH-format keys can be re-encrypted in place; convert it first".into() });
    }
    let text = keys::export_private(&key, ExportFormat::OpenSsh, new_passphrase.as_deref())?;
    keys::write_private_file(Path::new(&path), &text, true)?;
    let cache = state.passphrases.clone();
    blocking(move || {
        let was = cache.memory(&path);
        let _ = cache.forget(&path);
        if let Some(new) = new_passphrase.filter(|p| !p.is_empty()) {
            if let Err(e) = cache.store(&path, &new, was) {
                tracing::warn!("{e}");
            }
        }
        let mut info = keys::inspect(Path::new(&path));
        fill_memory(&cache, &mut info);
        info
    })
    .await
}

/// Import pasted key text into a new OpenSSH file.
#[tauri::command]
#[specta::specta]
pub async fn keys_import(text: String, out_path: String, passphrase: Option<String>, new_passphrase: Option<String>, overwrite: bool) -> Result<KeyInfo> {
    let key = keys::load_private_key("pasted key", text.as_bytes(), passphrase.as_deref())?;
    let out = keys::export_private(&key, ExportFormat::OpenSsh, new_passphrase.as_deref())?;
    keys::write_private_file(Path::new(&out_path), &out, overwrite)?;
    if overwrite {
        std::fs::write(format!("{out_path}.pub"), format!("{}\n", keys::public_line(&key)?))?;
    } else {
        keys::write_public_file(Path::new(&out_path), &keys::public_line(&key)?)?;
    }
    Ok(keys::inspect(Path::new(&out_path)))
}

/// The text of a key file dropped on the key manager, to pass to `keys_import`. Only private
/// keys in a format the key manager reads are returned.
#[tauri::command]
#[specta::specta]
pub async fn keys_read_file(path: String) -> Result<String> {
    blocking(move || keys::read_key_text(Path::new(&path))).await?
}

/// Delete a private key file, and its `.pub` when `include_public` is set, and forget its
/// passphrase. Refuses a file that is not a private key.
#[tauri::command]
#[specta::specta]
pub async fn keys_delete(state: State<'_, AppState>, path: String, include_public: bool) -> Result<()> {
    let cache = state.passphrases.clone();
    blocking(move || {
        keys::delete_key(Path::new(&path), include_public)?;
        let _ = cache.forget(&path);
        Ok(())
    })
    .await?
}

#[tauri::command]
#[specta::specta]
pub async fn keys_agent_list() -> Result<Vec<String>> {
    keys::agent::list_fingerprints().await
}

#[tauri::command]
#[specta::specta]
pub async fn keys_agent_add(state: State<'_, AppState>, path: String, passphrase: Option<String>, lifetime_secs: Option<u32>) -> Result<()> {
    let key = read_key(&state, &path, passphrase.as_deref())?;
    keys::agent::add_key(&key, lifetime_secs).await
}
