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
    /// `user` and `port` are known when the built-in SSH client tried; they are None when the
    /// failure came from Git's own `ssh` (a fetch to an SSH remote). `password_allowed` says the
    /// server accepts password authentication, so the UI can ask for one and pass it to
    /// `ssh_set_password`; `password_rejected` says a password supplied that way was tried and
    /// refused, and has been forgotten.
    #[error("authentication to {host} failed (tried: {methods_tried:?})")]
    SshAuthFailed {
        host: String,
        port: Option<u16>,
        user: Option<String>,
        methods_tried: Vec<String>,
        password_allowed: bool,
        password_rejected: bool,
    },
    /// `ssh_trust_host(host, port)` records the key that was offered.
    #[error("unknown host key for {host} ({fingerprint})")]
    SshHostKeyUnknown { host: String, port: u16, fingerprint: String },
    #[error("host key for {host} has CHANGED ({fingerprint})")]
    SshHostKeyChanged { host: String, port: u16, fingerprint: String },
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
    /// Git could not change `.git/config` because `config.lock` is there: another Git process is
    /// writing the config, or one died and left the file behind. `lock_path` is the lock file as
    /// Git names it, relative to the repository when Git gives a relative path.
    #[error("the Git config is locked ({lock_path})")]
    ConfigLocked { lock_path: String },
    #[error("invalid path {path}: {reason}")]
    InvalidPath { path: String, reason: String },
    #[error("object not found: {spec}")]
    ObjectNotFound { spec: String },
    #[error("content is too large ({bytes} bytes, limit {limit})")]
    TooLarge { bytes: u64, limit: u64 },
    /// A file was about to be saved over, but its contents on disk are no longer the ones it was
    /// opened with: another program, or Git, has changed it since. Nothing was written.
    #[error("{path} has changed on disk since it was opened")]
    ChangedOnDisk { path: String },
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
        // Git quotes the path without escaping it, so it runs to the last quote on its line: a
        // folder such as `C:/Users/O'Brien/repo` has a quote of its own.
        let path = text
            .lines()
            .find_map(|l| l.split_once("repository at '"))
            .and_then(|(_, rest)| rest.rsplit_once('\''))
            .map(|(p, _)| p.to_string())
            .unwrap_or_default();
        return GitError::UnsafeRepository { path };
    }
    // Git's own wording, `Unable to create '<git dir>/index.lock': File exists.`, and not any
    // message that merely names something with `index.lock` in it, or the lock of a ref
    // (`refs/heads/index.lock` for a branch named `index`).
    let index_lock = text.lines().find_map(|l| {
        let (_, rest) = l.split_once("Unable to create '")?;
        rest.strip_suffix("': File exists.").filter(|p| p.ends_with("/index.lock") && !p.contains("/refs/"))
    });
    if let Some(lock_path) = index_lock {
        return GitError::IndexLocked { lock_path: lock_path.to_string() };
    }
    // `could not lock config file <path>: File exists`, Git's wording when `<path>.lock` is
    // there. Other reasons (`Permission denied`) are not a lock to remove.
    let config_lock = text.lines().find_map(|l| {
        let (_, rest) = l.split_once("could not lock config file ")?;
        rest.strip_suffix(": File exists")
    });
    if let Some(config_path) = config_lock {
        return GitError::ConfigLocked { lock_path: format!("{config_path}.lock") };
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
                port: None,
                user: None,
                methods_tried: Vec::new(),
                password_allowed: false,
                password_rejected: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(stderr: &str) -> GitError {
        classify_git_failure("op", vec![], Some(128), stderr.as_bytes(), &crate::transport::TransportKind::Local)
    }

    #[test]
    fn dubious_ownership_keeps_a_quote_in_the_path() {
        let stderr = "fatal: detected dubious ownership in repository at 'C:/Users/O'Brien/repo'\nTo add an exception for this directory, call:\n\n\tgit config --global --add safe.directory 'C:/Users/O'\\''Brien/repo'\n";
        assert!(matches!(classify(stderr), GitError::UnsafeRepository { path } if path == "C:/Users/O'Brien/repo"));
    }

    #[test]
    fn config_lock_from_gits_own_wording() {
        let locked = "error: could not lock config file .git/config: File exists\nerror: unable to write upstream branch configuration\nhint: ";
        assert!(matches!(classify(locked), GitError::ConfigLocked { lock_path } if lock_path == ".git/config.lock"));
        let denied = "error: could not lock config file .git/config: Permission denied";
        assert!(matches!(classify(denied), GitError::CommandFailed { .. }));
    }

    #[test]
    fn index_lock_only_from_gits_own_wording() {
        let locked = "fatal: Unable to create 'C:/Users/O'Brien/repo/.git/index.lock': File exists.\n\nAnother git process seems to be running in this repository, e.g.\nan editor opened by 'git commit'.";
        assert!(matches!(classify(locked), GitError::IndexLocked { lock_path } if lock_path == "C:/Users/O'Brien/repo/.git/index.lock"));
        // A branch whose name contains `index.lock`, and the lock of a branch named `index`.
        assert!(matches!(classify("fatal: a branch named 'fix-index.lock-race' already exists"), GitError::CommandFailed { .. }));
        let ref_lock = "error: cannot lock ref 'refs/heads/index': Unable to create '/r/.git/refs/heads/index.lock': File exists.";
        assert!(matches!(classify(ref_lock), GitError::CommandFailed { .. }));
    }
}
