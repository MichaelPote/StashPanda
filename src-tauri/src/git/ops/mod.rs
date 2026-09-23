//! Write operations. Every one runs as `Category::UserAction`, expects the
//! caller to hold the repository's exclusive lock, and reports Git's own
//! output so the UI can show exactly what happened.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{classify_git_failure, GitError, Result};
use crate::git::{GitClient, GitReq};
use crate::transport::TransportKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OpResult {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// True when a merge stopped on conflicts (the working tree now has
    /// unmerged paths; the user resolves them or aborts).
    pub conflicts: bool,
}

fn result(out: crate::transport::Output) -> OpResult {
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let conflicts = !out.success() && (stdout.contains("CONFLICT") || stderr.contains("CONFLICT") || stdout.contains("Automatic merge failed"));
    OpResult { ok: out.success(), exit_code: out.exit_code, stdout, stderr, conflicts }
}

/// The lock path when Git refused to run because `index.lock` already exists
/// (another Git process is running, or one died and left the file behind).
pub fn index_lock_in(res: &OpResult) -> Option<String> {
    if res.ok {
        return None;
    }
    match classify_git_failure("op", vec![], res.exit_code, res.stderr.as_bytes(), &TransportKind::Local) {
        GitError::IndexLocked { lock_path } => Some(lock_path),
        _ => None,
    }
}

fn valid_ref_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.starts_with('-')
        || name.contains("..")
        || name.ends_with('/')
        || name.ends_with(".lock")
        || name.chars().any(|c| c.is_control() || " ~^:?*[\\".contains(c));
    if bad {
        return Err(GitError::InvalidPath { path: name.to_string(), reason: "not a valid Git reference name".into() });
    }
    Ok(())
}

/// A revision the user selected (`HEAD~1`, `abc123`, `origin/dev`): only guard
/// against option injection and control characters.
fn valid_revision(rev: &str) -> Result<()> {
    if rev.is_empty() || rev.starts_with('-') || rev.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(GitError::InvalidPath { path: rev.to_string(), reason: "not a valid Git revision".into() });
    }
    Ok(())
}

/// Check out a branch name or a commit (detached when `target` is not a branch).
pub async fn checkout(client: &GitClient, target: &str) -> Result<OpResult> {
    valid_revision(target)?;
    let out = client.exec_allow_failure(GitReq::new("checkout", ["checkout", target]).user()).await?;
    Ok(result(out))
}

pub async fn branch_create(client: &GitClient, name: &str, start_point: &str, checkout: bool) -> Result<OpResult> {
    valid_ref_name(name)?;
    valid_revision(start_point)?;
    let args: Vec<&str> = if checkout {
        vec!["checkout", "-b", name, start_point]
    } else {
        vec!["branch", name, start_point]
    };
    let out = client.exec_allow_failure(GitReq::new("branch", args).user()).await?;
    Ok(result(out))
}

/// Merge `target` (a branch name or commit) into the current branch.
pub async fn merge(client: &GitClient, target: &str, no_ff: bool) -> Result<OpResult> {
    valid_revision(target)?;
    let mut args = vec!["merge", "--no-edit"];
    if no_ff {
        args.push("--no-ff");
    }
    args.push(target);
    let out = client.exec_allow_failure(GitReq::new("merge", args).user()).await?;
    Ok(result(out))
}

pub async fn merge_abort(client: &GitClient) -> Result<OpResult> {
    let out = client.exec_allow_failure(GitReq::new("merge", ["merge", "--abort"]).user()).await?;
    Ok(result(out))
}

/// Lightweight tag, or annotated when `message` is given.
pub async fn tag_create(client: &GitClient, name: &str, target: &str, message: Option<&str>, force: bool) -> Result<OpResult> {
    valid_ref_name(name)?;
    valid_revision(target)?;
    let mut args: Vec<String> = vec!["tag".into()];
    if force {
        args.push("-f".into());
    }
    if let Some(m) = message.filter(|m| !m.trim().is_empty()) {
        args.push("-a".into());
        args.push("-m".into());
        args.push(m.to_string());
    }
    args.push(name.to_string());
    args.push(target.to_string());
    let out = client.exec_allow_failure(GitReq::new("tag", args).user()).await?;
    Ok(result(out))
}

pub async fn tag_delete(client: &GitClient, name: &str) -> Result<OpResult> {
    valid_ref_name(name)?;
    let out = client.exec_allow_failure(GitReq::new("tag", ["tag", "-d", name]).user()).await?;
    Ok(result(out))
}

pub async fn branch_delete(client: &GitClient, name: &str, force: bool) -> Result<OpResult> {
    valid_ref_name(name)?;
    let flag = if force { "-D" } else { "-d" };
    let out = client.exec_allow_failure(GitReq::new("branch", ["branch", flag, name]).user()).await?;
    Ok(result(out))
}

#[cfg(test)]
mod e2e {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Arc;

    use super::*;
    use crate::git::console::ConsoleRecorder;
    use crate::git::queries::refs::get_refs;
    use crate::git::versions::Capabilities;
    use crate::transport::local::LocalTransport;
    use crate::transport::paths::{IdentityMapper, NativePath};
    use crate::transport::{SharedTransport, Transport as _};

    fn git(dir: &PathBuf, args: &[&str]) {
        let st = Command::new("git").arg("-C").arg(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=T", "-c", "commit.gpgsign=false"]).args(args).status().unwrap();
        assert!(st.success(), "git {args:?}");
    }

    #[tokio::test]
    async fn branch_tag_checkout_and_merge_round_trip() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-ops-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "one"]);
        let transport: SharedTransport = Arc::new(LocalTransport { git: which::which("git").unwrap(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false });
        let info = transport.probe().await.unwrap();
        let client = GitClient { transport, repo: NativePath(dir.to_string_lossy().into_owned()), caps: Capabilities::for_version(info.git_version), console: Arc::new(ConsoleRecorder::new(None, "ops".into(), 4096)) };

        // Branch from HEAD and check it out, commit, go back, merge (fast-forward not allowed).
        assert!(branch_create(&client, "topic", "HEAD", true).await.unwrap().ok);
        std::fs::write(dir.join("b.txt"), "2\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "two"]);
        assert!(checkout(&client, "main").await.unwrap().ok);
        let m = merge(&client, "topic", true).await.unwrap();
        assert!(m.ok && !m.conflicts, "{m:?}");
        let refs = get_refs(&client).await.unwrap();
        assert!(refs.refs.iter().any(|r| r.short == "topic"));

        // Tags: lightweight, annotated, delete, invalid name.
        assert!(tag_create(&client, "v1", "HEAD", None, false).await.unwrap().ok);
        assert!(tag_create(&client, "v1-notes", "HEAD~1", Some("release notes"), false).await.unwrap().ok);
        let dup = tag_create(&client, "v1", "HEAD", None, false).await.unwrap();
        assert!(!dup.ok && dup.stderr.contains("already exists"));
        assert!(tag_delete(&client, "v1").await.unwrap().ok);
        assert!(matches!(tag_create(&client, "bad name", "HEAD", None, false).await, Err(GitError::InvalidPath { .. })));
        let refs = get_refs(&client).await.unwrap();
        assert!(refs.refs.iter().any(|r| r.short == "v1-notes" && matches!(r.kind, crate::git::queries::refs::RefKind::Tag { annotated: true })));
        assert!(!refs.refs.iter().any(|r| r.short == "v1"));

        // Conflicting merge reports conflicts instead of an error; abort restores.
        assert!(branch_create(&client, "clash", "HEAD", true).await.unwrap().ok);
        std::fs::write(dir.join("a.txt"), "clash\n").unwrap();
        git(&dir, &["commit", "-qam", "clash"]);
        assert!(checkout(&client, "main").await.unwrap().ok);
        std::fs::write(dir.join("a.txt"), "main\n").unwrap();
        git(&dir, &["commit", "-qam", "main-change"]);
        let c = merge(&client, "clash", false).await.unwrap();
        assert!(!c.ok && c.conflicts, "{c:?}");
        assert!(merge_abort(&client).await.unwrap().ok);
        assert!(branch_delete(&client, "clash", true).await.unwrap().ok);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn index_lock_is_detected_and_removable() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-lock-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "one"]);
        git(&dir, &["branch", "other"]);
        std::fs::write(dir.join("a.txt"), "2\n").unwrap();
        git(&dir, &["commit", "-qam", "two"]);
        let transport: SharedTransport = Arc::new(LocalTransport { git: which::which("git").unwrap(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false });
        let info = transport.probe().await.unwrap();
        let client = GitClient { transport: transport.clone(), repo: NativePath(dir.to_string_lossy().into_owned()), caps: Capabilities::for_version(info.git_version), console: Arc::new(ConsoleRecorder::new(None, "ops".into(), 4096)) };

        let lock = dir.join(".git/index.lock");
        std::fs::write(&lock, "").unwrap();
        let blocked = checkout(&client, "other").await.unwrap();
        let reported = index_lock_in(&blocked).expect("index.lock recognised");
        assert!(reported.ends_with("index.lock"), "{reported}");

        transport.remove_file(&NativePath(lock.to_string_lossy().into_owned())).await.unwrap();
        assert!(!lock.exists());
        // Removing it again is not an error.
        transport.remove_file(&NativePath(lock.to_string_lossy().into_owned())).await.unwrap();

        let ok = checkout(&client, "other").await.unwrap();
        assert!(ok.ok && index_lock_in(&ok).is_none(), "{ok:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
