//! The single error type every Tauri command returns.
//!
//! It is serialised as a tagged union (`{ "kind": "...", ...fields }`) so the
//! TypeScript side can `switch` on `kind`. Variants that come from Git output
//! are produced by [`classify`], everything else by the `From` impls below.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GitError {
    #[error("git executable not found (searched: {searched:?})")]
    GitNotFound { searched: Vec<String> },
    #[error("git {found} is too old, StashPanda needs {required}")]
    GitVersionTooOld { found: String, required: String },
    #[error("wsl.exe is not available on this machine")]
    WslNotAvailable,
    #[error("WSL distro {distro} was not found")]
    WslDistroNotFound { distro: String, available: Vec<String> },
    #[error("cannot connect to {host}: {detail}")]
    SshConnectFailed { host: String, detail: String },
    #[error("authentication to {host} failed (tried: {methods_tried:?})")]
    SshAuthFailed { host: String, methods_tried: Vec<String> },
    #[error("unknown host key for {host} ({fingerprint})")]
    SshHostKeyUnknown { host: String, fingerprint: String },
    #[error("host key for {host} has CHANGED ({fingerprint})")]
    SshHostKeyChanged { host: String, fingerprint: String },
    #[error("unsupported key format in {path}: {detected_header}")]
    SshKeyUnsupported { path: String, detected_header: String },
    #[error("key {path} needs a passphrase")]
    SshNeedsPassphrase { path: String },
    #[error("wrong passphrase for {path}")]
    SshWrongPassphrase { path: String },
    #[error("could not parse key {path} ({format}): {detail}")]
    KeyParseError { path: String, format: String, detail: String },
    #[error("{feature} is not supported by the built-in SSH client")]
    SshUnsupportedFeature { feature: String },
    #[error("connection lost: {detail}")]
    TransportDisconnected { detail: String },
    #[error("{path} is not a git repository")]
    NotARepository { path: String },
    #[error("{path} does not exist")]
    RepoPathNotFound { path: String },
    #[error("repository at {path} is owned by another user (git safe.directory)")]
    UnsafeRepository { path: String },
    #[error("the index is locked ({lock_path})")]
    IndexLocked { lock_path: String },
    #[error("invalid path {path}: {reason}")]
    InvalidPath { path: String, reason: String },
    #[error("object not found: {spec}")]
    ObjectNotFound { spec: String },
    #[error("content is too large ({bytes} bytes, limit {limit})")]
    TooLarge { bytes: u64, limit: u64 },
    #[error("git {label} failed with exit code {exit_code:?}: {stderr}")]
    CommandFailed {
        label: String,
        argv: Vec<String>,
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("unexpected output from {what}: {detail}")]
    ParseError { what: String, detail: String },
    #[error("timed out after {secs}s")]
    Timeout { secs: u64 },
    #[error("cancelled")]
    Cancelled,
    #[error("repository is not open: {repo_id}")]
    RepoNotOpen { repo_id: String },
    #[error("stale graph version {requested}, current is {current}")]
    StaleVersion { requested: u64, current: u64 },
    #[error("I/O error: {detail}")]
    Io { detail: String },
    #[error("{detail}")]
    Internal { detail: String },
}

impl From<std::io::Error> for GitError {
    fn from(e: std::io::Error) -> Self {
        GitError::Io { detail: e.to_string() }
    }
}

impl From<serde_json::Error> for GitError {
    fn from(e: serde_json::Error) -> Self {
        GitError::Internal { detail: format!("JSON error: {e}") }
    }
}

impl From<rusqlite::Error> for GitError {
    fn from(e: rusqlite::Error) -> Self {
        GitError::Internal { detail: format!("SQLite error: {e}") }
    }
}

impl From<tauri::Error> for GitError {
    fn from(e: tauri::Error) -> Self {
        GitError::Internal { detail: format!("Tauri error: {e}") }
    }
}

impl GitError {
    pub fn internal(detail: impl Into<String>) -> Self {
        GitError::Internal { detail: detail.into() }
    }
}

pub type Result<T> = std::result::Result<T, GitError>;

/// Turn a failed Git invocation into the most specific error we can name.
pub fn classify_git_failure(
    label: &str,
    argv: Vec<String>,
    exit_code: Option<i32>,
    stderr: &[u8],
    transport: &crate::transport::TransportKind,
) -> GitError {
    use crate::transport::TransportKind;
    let text = String::from_utf8_lossy(stderr);
    let lower = text.to_ascii_lowercase();

    if lower.contains("not a git repository") {
        let path = argv
            .iter()
            .position(|a| a == "-C")
            .and_then(|i| argv.get(i + 1))
            .cloned()
            .unwrap_or_default();
        return GitError::NotARepository { path };
    }
    if lower.contains("dubious ownership") {
        let path = text
            .split('\'')
            .nth(1)
            .map(|s| s.to_string())
            .unwrap_or_default();
        return GitError::UnsafeRepository { path };
    }
    if lower.contains("index.lock") && lower.contains("exists") {
        let lock_path = text
            .split('\'')
            .nth(1)
            .map(|s| s.to_string())
            .unwrap_or_else(|| ".git/index.lock".into());
        return GitError::IndexLocked { lock_path };
    }
    if lower.contains("no such file or directory") && lower.contains("cannot change to") {
        let path = text
            .split('\'')
            .nth(1)
            .map(|s| s.to_string())
            .unwrap_or_default();
        return GitError::RepoPathNotFound { path };
    }
    if let TransportKind::Wsl { distro } = transport {
        if exit_code == Some(255)
            && (lower.contains("no distribution") || text.contains("Wsl/Service/"))
        {
            return GitError::WslDistroNotFound {
                distro: distro.clone(),
                available: Vec::new(),
            };
        }
    }
    if let TransportKind::Ssh { host } = transport {
        if lower.contains("permission denied (publickey")
            || lower.contains("too many authentication failures")
        {
            return GitError::SshAuthFailed {
                host: host.clone(),
                methods_tried: Vec::new(),
            };
        }
        if exit_code == Some(255) {
            return GitError::TransportDisconnected {
                detail: text.trim().to_string(),
            };
        }
    }
    GitError::CommandFailed {
        label: label.to_string(),
        argv,
        exit_code,
        stderr: text.trim().to_string(),
    }
}
