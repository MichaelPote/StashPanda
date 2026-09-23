//! SSH key manager commands.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::error::{GitError, Result};
use crate::keys::{self, ExportFormat, GenerateAlgorithm, KeyInfo};
use crate::state::AppState;

fn read_key(state: &AppState, path: &str, passphrase: Option<&str>) -> Result<ssh_key::PrivateKey> {
    let bytes = std::fs::read(path)?;
    let cached = state.passphrases.get(path);
    let pass = passphrase.or(cached.as_deref().map(|s| s.as_str()));
    keys::load_private_key(path, &bytes, pass)
}

#[tauri::command]
#[specta::specta]
pub async fn keys_default_dirs(state: State<'_, AppState>) -> Result<Vec<String>> {
    Ok(keys::default_key_dirs(&state.host).into_iter().map(|p| p.to_string_lossy().into_owned()).collect())
}

/// Every key in the default folders plus `extra_dirs`, with agent presence.
#[tauri::command]
#[specta::specta]
pub async fn keys_scan(state: State<'_, AppState>, extra_dirs: Vec<String>) -> Result<Vec<KeyInfo>> {
    let mut dirs = keys::default_key_dirs(&state.host);
    dirs.extend(extra_dirs.into_iter().map(PathBuf::from));
    dirs.dedup();
    let mut infos = tokio::task::spawn_blocking(move || keys::scan(&dirs))
        .await
        .map_err(|e| GitError::internal(e.to_string()))?;
    if let Ok(in_agent) = keys::agent::list_fingerprints().await {
        for i in &mut infos {
            i.in_agent = i.fingerprint.as_ref().is_some_and(|f| in_agent.contains(f));
        }
    }
    Ok(infos)
}

#[tauri::command]
#[specta::specta]
pub async fn keys_inspect(path: String) -> Result<KeyInfo> {
    Ok(tokio::task::spawn_blocking(move || keys::inspect(Path::new(&path)))
        .await
        .map_err(|e| GitError::internal(e.to_string()))?)
}

/// Check a passphrase and optionally remember it for this session.
#[tauri::command]
#[specta::specta]
pub async fn keys_unlock(state: State<'_, AppState>, path: String, passphrase: String, remember: bool) -> Result<KeyInfo> {
    let bytes = std::fs::read(&path)?;
    let key = keys::load_private_key(&path, &bytes, Some(&passphrase))?;
    if remember {
        state.passphrases.remember(&path, &passphrase);
    }
    let mut info = keys::inspect(Path::new(&path));
    info.fingerprint = Some(key.fingerprint(ssh_key::HashAlg::Sha256).to_string());
    info.algorithm = Some(key.algorithm().as_str().to_string());
    Ok(info)
}

#[tauri::command]
#[specta::specta]
pub async fn keys_forget_passphrase(state: State<'_, AppState>, path: String) -> Result<()> {
    state.passphrases.forget(&path);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn keys_export_public(state: State<'_, AppState>, path: String, passphrase: Option<String>) -> Result<String> {
    let key = read_key(&state, &path, passphrase.as_deref())?;
    keys::public_line(&key)
}

/// Convert to OpenSSH or PKCS#8 next to the original (or at `out_path`),
/// with an optional new passphrase. Writes a `.pub` alongside. Returns the
/// path written.
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
    let key = read_key(&state, &path, passphrase.as_deref())?;
    let out = match out_path.filter(|p| !p.trim().is_empty()) {
        Some(p) => PathBuf::from(p),
        None => {
            let src = Path::new(&path);
            let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "key".into());
            let ext = match format {
                ExportFormat::OpenSsh => "",
                ExportFormat::Pkcs8 => ".pem",
            };
            src.with_file_name(format!("{stem}{ext}"))
        }
    };
    if out == Path::new(&path) && !overwrite {
        return Err(GitError::Io { detail: "refusing to overwrite the original key; choose another name or tick overwrite".into() });
    }
    let text = keys::export_private(&key, format, new_passphrase.as_deref())?;
    keys::write_private_file(&out, &text, overwrite)?;
    let pub_line = keys::public_line(&key)?;
    let pub_path = PathBuf::from(format!("{}.pub", out.to_string_lossy()));
    if overwrite || !pub_path.exists() {
        std::fs::write(&pub_path, format!("{pub_line}\n"))?;
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
    std::fs::write(format!("{out_path}.pub"), format!("{}\n", keys::public_line(&key)?))?;
    Ok(keys::inspect(out))
}

/// Re-encrypt an OpenSSH-format key in place with a new passphrase (or none).
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
    state.passphrases.forget(&path);
    Ok(keys::inspect(Path::new(&path)))
}

/// Import pasted key text into a new OpenSSH file.
#[tauri::command]
#[specta::specta]
pub async fn keys_import(text: String, out_path: String, passphrase: Option<String>, new_passphrase: Option<String>, overwrite: bool) -> Result<KeyInfo> {
    let key = keys::load_private_key("pasted key", text.as_bytes(), passphrase.as_deref())?;
    let out = keys::export_private(&key, ExportFormat::OpenSsh, new_passphrase.as_deref())?;
    keys::write_private_file(Path::new(&out_path), &out, overwrite)?;
    std::fs::write(format!("{out_path}.pub"), format!("{}\n", keys::public_line(&key)?))?;
    Ok(keys::inspect(Path::new(&out_path)))
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
