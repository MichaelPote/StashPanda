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

/// Stop a rebase that stopped on conflicts (including one a rebasing pull started) and put the
/// branch back where it was.
pub async fn rebase_abort(client: &GitClient) -> Result<OpResult> {
    let out = client.exec_allow_failure(GitReq::new("rebase", ["rebase", "--abort"]).user()).await?;
    Ok(result(out))
}

/// How a pull brings the fetched commits into the current branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PullMode {
    /// Fast-forward when possible, otherwise a merge commit (`--no-rebase`).
    Merge,
    /// Replay local commits on top of the fetched ones (`--rebase`).
    Rebase,
    /// Only move the branch forward; refuse when both sides have new commits (`--ff-only`).
    FastForwardOnly,
}

/// The first half of a pull: `git fetch --progress <remote> <branch>`, which writes `FETCH_HEAD`
/// and updates `refs/remotes/<remote>/<branch>`. `--progress` makes Git report progress even
/// though stderr is a pipe; each update goes to `progress`. No timeout: a slow remote is stopped
/// through `cancel`. Fetch touches no `index.lock` and changes no branch, so stopping it is safe:
/// Git removes its ref lock files on SIGTERM, and a stopped fetch only leaves the remote-tracking
/// branch where it was.
///
/// StashPanda pulls in two commands rather than `git pull` so that Stop only ever lands here.
/// Stopping `git pull` in its merge or rebase step leaves a half-done rebase, and on Windows,
/// where Git can only be killed outright, an `index.lock` too.
pub async fn pull_fetch(
    client: &GitClient,
    remote: &str,
    branch: &str,
    progress: &(dyn Fn(String) + Send + Sync),
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<OpResult> {
    valid_ref_name(remote)?;
    valid_ref_name(branch)?;
    let req = GitReq::new("fetch", ["fetch", "--progress", remote, branch]).user().timeout(None).own_session();
    let out = client.exec_streaming(req, progress, cancel).await?;
    Ok(result(out))
}

/// The second half of a pull: bring `FETCH_HEAD` into the current branch the way `git pull`
/// would, checked against it with Git 2.43.
///
/// - Merge: `git merge --no-edit FETCH_HEAD`, which writes the same "Merge branch 'dev' of
///   <url>" message. `pull.ff` is not read, so `pull.ff=only` does not override the choice.
/// - Fast-forward only: `git merge --ff-only FETCH_HEAD`.
/// - Rebase: when the branch has no commits of its own this fast-forwards, as `git pull
///   --rebase` does. Otherwise `git rebase --onto FETCH_HEAD <fork point>`, where the fork point
///   comes from `git merge-base --fork-point refs/remotes/<remote>/<branch> HEAD`, so commits the
///   remote has since rewritten are dropped rather than replayed; without one it rebases onto
///   `FETCH_HEAD` itself.
///
/// `autostash` stashes uncommitted changes first and reapplies them afterwards: when reapplying
/// conflicts, Git still exits 0, prints "Applying autostash resulted in conflicts" and keeps the
/// changes as `stash@{0}`. Not cancellable. Timeout 10 minutes, for slow hooks.
pub async fn pull_integrate(client: &GitClient, remote: &str, branch: &str, mode: PullMode, autostash: bool) -> Result<OpResult> {
    valid_ref_name(remote)?;
    valid_ref_name(branch)?;
    let timeout = Some(std::time::Duration::from_secs(600));
    let autostash_flag = autostash.then_some("--autostash");
    if mode == PullMode::Rebase && !fast_forwards(client).await? {
        let tracking = format!("refs/remotes/{remote}/{branch}");
        let fork = client
            .exec_allow_failure(GitReq::new("merge-base", ["merge-base", "--fork-point", &tracking, "HEAD"]))
            .await?;
        let fork_point = if fork.success() { String::from_utf8_lossy(&fork.stdout).trim().to_string() } else { String::new() };
        let upstream = if fork_point.is_empty() { "FETCH_HEAD" } else { fork_point.as_str() };
        let mut args = vec!["rebase"];
        args.extend(autostash_flag);
        args.extend(["--onto", "FETCH_HEAD", upstream]);
        let out = client.exec_allow_failure(GitReq::new("rebase", args).user().timeout(timeout)).await?;
        return Ok(result(out));
    }
    let mut args = vec!["merge", "--no-edit"];
    if mode != PullMode::Merge {
        args.push("--ff-only");
    }
    args.extend(autostash_flag);
    args.push("FETCH_HEAD");
    let out = client.exec_allow_failure(GitReq::new("merge", args).user().timeout(timeout)).await?;
    Ok(result(out))
}

/// True when HEAD is `FETCH_HEAD` or behind it, so the branch has no commits of its own and
/// bringing the fetched commits in is a fast-forward. Also true on an unborn branch, where `git
/// merge` checks the fetched commit out.
async fn fast_forwards(client: &GitClient) -> Result<bool> {
    let out = client
        .exec_allow_failure(GitReq::new("merge-base", ["merge-base", "--is-ancestor", "HEAD", "FETCH_HEAD"]))
        .await?;
    // 0: an ancestor; 1: not; anything else: HEAD has no commit yet.
    Ok(out.exit_code != Some(1))
}

/// The pull mode the repository's Git config asks for, in the order `git pull` reads it:
/// `branch.<branch>.rebase`, then `pull.rebase`, then `pull.ff`. None when none of them is set.
pub async fn configured_pull_mode(client: &GitClient, branch: Option<&str>) -> Result<Option<PullMode>> {
    let mut pattern = String::from(r"^pull\.(rebase|ff)$");
    if let Some(b) = branch {
        pattern.push_str(&format!(r"|^branch\.{}\.rebase$", regex_escape(b)));
    }
    let out = client.exec_allow_failure(GitReq::new("config", ["config".to_string(), "-z".into(), "--get-regexp".into(), pattern])).await?;
    // Exit 1 means nothing matched.
    if !out.success() && out.exit_code != Some(1) {
        return Err(classify_git_failure("config", vec![], out.exit_code, &out.stderr, &client.transport.kind()));
    }
    Ok(pull_mode_from_config(&out.stdout, branch))
}

/// Escape a branch name for the POSIX extended regular expression `git config --get-regexp` takes.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r".^$*+?()[]{}|\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Parse `git config -z --get-regexp` output (`key\nvalue\0`, or `key\0` for a key with no
/// value, which Git reads as true). Git prints section and key names in lower case; the branch
/// name in the middle keeps its case. A later entry for the same key wins, as in Git.
fn pull_mode_from_config(bytes: &[u8], branch: Option<&str>) -> Option<PullMode> {
    let (mut branch_rebase, mut pull_rebase, mut pull_ff) = (None, None, None);
    let branch_key = branch.map(|b| format!("branch.{b}.rebase"));
    for entry in bytes.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let text = String::from_utf8_lossy(entry);
        let (key, value) = match text.split_once('\n') {
            Some((k, v)) => (k.to_string(), v.trim().to_ascii_lowercase()),
            None => (text.to_string(), "true".to_string()),
        };
        match key.as_str() {
            "pull.rebase" => pull_rebase = Some(value),
            "pull.ff" => pull_ff = Some(value),
            k if Some(k) == branch_key.as_deref() => branch_rebase = Some(value),
            _ => {}
        }
    }
    // `merges`, `interactive` and their one-letter forms all rebase; StashPanda runs a plain one.
    let rebases = |v: &str| matches!(v, "true" | "yes" | "on" | "1" | "merges" | "m" | "interactive" | "i");
    let ff_only = pull_ff.as_deref() == Some("only");
    match branch_rebase.or(pull_rebase) {
        Some(v) if rebases(&v) => Some(PullMode::Rebase),
        Some(_) if ff_only => Some(PullMode::FastForwardOnly),
        Some(_) => Some(PullMode::Merge),
        None if ff_only => Some(PullMode::FastForwardOnly),
        None if pull_ff.is_some() => Some(PullMode::Merge),
        None => None,
    }
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

/// `stash@{index}`, after checking that it still names the stash whose commit is `oid`. The list
/// the user clicked in can be stale: a stash made in a terminal shifts every index by one, and
/// acting on the index alone would then apply or drop a different stash.
async fn stash_selector(client: &GitClient, index: u32, oid: &str) -> Result<String> {
    let selector = format!("stash@{{{index}}}");
    let out = client
        .exec_allow_failure(GitReq::new("stash", ["rev-parse".to_string(), "--verify".into(), "--quiet".into(), selector.clone()]))
        .await?;
    if !out.success() || String::from_utf8_lossy(&out.stdout).trim() != oid {
        return Err(GitError::InvalidPath {
            path: selector,
            reason: "the stash list has changed since it was shown; check the list and try again".into(),
        });
    }
    Ok(selector)
}

/// Save the working tree's changes as a new stash (`stash@{0}`) and reset the working tree to
/// HEAD. With nothing to save, Git exits 0 and prints `No local changes to save`.
pub async fn stash_push(client: &GitClient, message: Option<&str>, include_untracked: bool) -> Result<OpResult> {
    let mut args: Vec<String> = vec!["stash".into(), "push".into()];
    if include_untracked {
        args.push("--include-untracked".into());
    }
    if let Some(m) = message.filter(|m| !m.trim().is_empty()) {
        args.push("-m".into());
        args.push(m.to_string());
    }
    let out = client.exec_allow_failure(GitReq::new("stash", args).user()).await?;
    Ok(result(out))
}

/// Apply a stash's changes to the working tree and keep the stash.
pub async fn stash_apply(client: &GitClient, index: u32, oid: &str) -> Result<OpResult> {
    let selector = stash_selector(client, index, oid).await?;
    let out = client.exec_allow_failure(GitReq::new("stash", ["stash".to_string(), "apply".into(), selector]).user()).await?;
    Ok(result(out))
}

/// Apply a stash, then drop it. When applying stops on conflicts Git keeps the stash.
pub async fn stash_pop(client: &GitClient, index: u32, oid: &str) -> Result<OpResult> {
    let selector = stash_selector(client, index, oid).await?;
    let out = client.exec_allow_failure(GitReq::new("stash", ["stash".to_string(), "pop".into(), selector]).user()).await?;
    Ok(result(out))
}

/// Delete a stash without applying it.
pub async fn stash_drop(client: &GitClient, index: u32, oid: &str) -> Result<OpResult> {
    let selector = stash_selector(client, index, oid).await?;
    let out = client.exec_allow_failure(GitReq::new("stash", ["stash".to_string(), "drop".into(), selector]).user()).await?;
    Ok(result(out))
}

/// Repository-relative paths from the status list. They reach Git as
/// literal pathspecs, so `:x`, `*.txt` and `-f` name exactly that file.
fn valid_paths(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Err(GitError::InvalidPath { path: String::new(), reason: "no paths given".into() });
    }
    if let Some(p) = paths.iter().find(|p| p.is_empty() || p.contains('\0')) {
        return Err(GitError::InvalidPath { path: p.clone(), reason: "not a valid path".into() });
    }
    Ok(())
}

/// `git --literal-pathspecs <args> --pathspec-from-file=- --pathspec-file-nul`
/// with the paths on stdin, so a long selection never hits the Windows
/// command-line limit.
async fn exec_with_paths(client: &GitClient, label: &'static str, args: &[&str], paths: &[String]) -> Result<OpResult> {
    let mut argv: Vec<String> = vec!["--literal-pathspecs".into()];
    argv.extend(args.iter().map(|a| a.to_string()));
    argv.push("--pathspec-from-file=-".into());
    argv.push("--pathspec-file-nul".into());
    let mut stdin = Vec::new();
    for p in paths {
        stdin.extend_from_slice(p.as_bytes());
        stdin.push(0);
    }
    let out = client.exec_allow_failure(GitReq::new(label, argv).stdin(stdin).user()).await?;
    Ok(result(out))
}

/// Both steps' output, succeeding only when both did.
pub(crate) fn combine(a: OpResult, b: OpResult) -> OpResult {
    let join = |x: String, y: String| match (x.is_empty(), y.is_empty()) {
        (true, _) => y,
        (_, true) => x,
        _ => format!("{x}\n{y}"),
    };
    OpResult {
        ok: a.ok && b.ok,
        exit_code: if a.ok { b.exit_code } else { a.exit_code },
        stdout: join(a.stdout, b.stdout),
        stderr: join(a.stderr, b.stderr),
        conflicts: a.conflicts || b.conflicts,
    }
}

/// Stage additions, modifications and deletions of `paths`. For a rename,
/// pass both the new and the original path.
pub async fn stage(client: &GitClient, paths: &[String]) -> Result<OpResult> {
    valid_paths(paths)?;
    exec_with_paths(client, "add", &["add", "-A"], paths).await
}

/// Move `paths` out of the index, back to their HEAD state; the working tree
/// is untouched. On an unborn branch there is no HEAD, so the paths are
/// removed from the index instead. For a staged rename, pass both paths.
pub async fn unstage(client: &GitClient, paths: &[String]) -> Result<OpResult> {
    valid_paths(paths)?;
    let head = client.exec_allow_failure(GitReq::new("rev-parse", ["rev-parse", "--verify", "-q", "HEAD"])).await?;
    if head.success() {
        exec_with_paths(client, "restore", &["restore", "--staged"], paths).await
    } else {
        exec_with_paths(client, "rm", &["rm", "--cached", "-q"], paths).await
    }
}

/// Commit hooks (lint, tests) can run for a while; still bounded, in case a
/// hook or GPG waits for input that can never arrive.
const COMMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Most bytes of paths passed to `git commit --only` on the command line.
/// The message already uses stdin, so the paths cannot go there as well;
/// this stays well under the Windows command-line limit of 32,767 characters.
const ONLY_ARGV_BYTES: usize = 24 * 1024;

/// Every path in the index that differs from HEAD, with renames split into
/// their two paths so they match what the frontend passes for a rename.
async fn staged_paths(client: &GitClient) -> Result<Vec<String>> {
    let out = client
        .exec_allow_failure(GitReq::new("diff", ["diff", "--cached", "--name-only", "-z", "--no-renames"]))
        .await?;
    if !out.success() {
        return Err(classify_git_failure("diff", vec![], out.exit_code, &out.stderr, &TransportKind::Local));
    }
    Ok(out
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect())
}

/// Commit the index, or with `paths` exactly those paths. The message goes
/// through stdin (`-F -`), so its length and characters never matter; Git
/// itself refuses an empty one. With `amend` and an empty message, the
/// previous message is kept.
///
/// `paths` are staged first (`add -A`, so new, changed and deleted files all
/// count; pass both paths of a rename). When other paths are staged as well,
/// the commit adds `--only` so they stay staged and out of the commit. If the
/// commit itself fails (a hook refuses, the message is empty), `paths` stay
/// staged. An empty `paths` with `amend` changes only the message.
pub async fn commit(client: &GitClient, message: &str, paths: Option<&[String]>, amend: bool, sign_off: bool) -> Result<OpResult> {
    let mut staged_first: Option<OpResult> = None;
    let mut only: Option<&[String]> = None;
    // An amend naming no paths rewords the last commit and leaves whatever is staged alone.
    if let Some([]) = paths {
        if !amend {
            return Err(GitError::InvalidPath { path: String::new(), reason: "no paths given".into() });
        }
        only = Some(&[]);
    } else if let Some(paths) = paths {
        valid_paths(paths)?;
        let added = stage(client, paths).await?;
        if !added.ok {
            return Ok(added);
        }
        let chosen: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        if staged_paths(client).await?.iter().any(|p| !chosen.contains(p.as_str())) {
            let bytes: usize = paths.iter().map(|p| p.len() + 1).sum();
            if bytes > ONLY_ARGV_BYTES {
                return Err(GitError::InvalidPath {
                    path: String::new(),
                    reason: format!(
                        "{} files are selected while other files are staged too, which is more than one commit command can name. Unstage the other files, or commit everything that is staged.",
                        paths.len()
                    ),
                });
            }
            only = Some(paths);
        }
        staged_first = Some(added);
    }

    let mut args: Vec<&str> = Vec::new();
    if only.is_some() {
        args.push("--literal-pathspecs");
    }
    args.push("commit");
    if amend {
        args.push("--amend");
    }
    if sign_off {
        args.push("--signoff");
    }
    let keep_message = amend && message.trim().is_empty();
    if keep_message {
        args.push("--no-edit");
    } else {
        args.extend(["-F", "-"]);
    }
    if let Some(paths) = only {
        args.extend(["--only", "--"]);
        args.extend(paths.iter().map(String::as_str));
    }
    let mut req = GitReq::new("commit", args);
    if !keep_message {
        req = req.stdin(message.as_bytes().to_vec());
    }
    req = req.user().timeout(Some(COMMIT_TIMEOUT));
    let committed = result(client.exec_allow_failure(req).await?);
    Ok(match staged_first {
        Some(added) => combine(added, committed),
        None => committed,
    })
}

/// Most untracked paths per `git clean` call, which has no stdin form.
const CLEAN_CHUNK_BYTES: usize = 16 * 1024;

/// Throw away changes. `tracked` go back to the index (`to_head` false) or to
/// HEAD in both the index and the working tree (`to_head` true, which also
/// removes files that are staged but not in HEAD). `untracked` are deleted.
/// Nothing here can be undone; the frontend confirms first.
pub async fn discard(client: &GitClient, tracked: &[String], untracked: &[String], to_head: bool) -> Result<OpResult> {
    if tracked.is_empty() && untracked.is_empty() {
        return Err(GitError::InvalidPath { path: String::new(), reason: "no paths given".into() });
    }
    let mut res: Option<OpResult> = None;
    if !tracked.is_empty() {
        valid_paths(tracked)?;
        let args: &[&str] = if to_head {
            &["restore", "--source=HEAD", "--staged", "--worktree"]
        } else {
            &["restore", "--worktree"]
        };
        let r = exec_with_paths(client, "restore", args, tracked).await?;
        if !r.ok {
            return Ok(r);
        }
        res = Some(r);
    }
    if !untracked.is_empty() {
        valid_paths(untracked)?;
        let mut chunk: Vec<&str> = Vec::new();
        let mut bytes = 0;
        for (i, p) in untracked.iter().enumerate() {
            chunk.push(p);
            bytes += p.len() + 1;
            if bytes < CLEAN_CHUNK_BYTES && i + 1 < untracked.len() {
                continue;
            }
            let mut argv = vec!["--literal-pathspecs", "clean", "-f", "-q", "--"];
            argv.append(&mut chunk);
            bytes = 0;
            let out = client.exec_allow_failure(GitReq::new("clean", argv).user()).await?;
            let r = result(out);
            let ok = r.ok;
            res = Some(match res.take() {
                Some(prev) => combine(prev, r),
                None => r,
            });
            if !ok {
                break;
            }
        }
    }
    Ok(res.expect("at least one path list is non-empty"))
}

#[cfg(test)]
mod e2e {
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;

    use super::*;
    use crate::git::console::ConsoleRecorder;
    use crate::git::queries::refs::get_refs;
    use crate::git::versions::Capabilities;
    use crate::transport::local::LocalTransport;
    use crate::transport::paths::{IdentityMapper, NativePath};
    use crate::transport::SharedTransport;

    fn git(dir: &Path, args: &[&str]) {
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
    async fn stash_push_apply_pop_drop_and_stale_index() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-stash-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "one"]);
        let client = client_for(&dir);
        let stashes = || async { crate::git::queries::log::get_stashes(&client).await.unwrap() };

        // Push: nothing to save is not an error; untracked files stay unless asked for.
        let nothing = stash_push(&client, None, false).await.unwrap();
        assert!(nothing.ok && nothing.stdout.contains("No local changes to save"), "{nothing:?}");
        std::fs::write(dir.join("new.txt"), "untracked\n").unwrap();
        assert!(stash_push(&client, Some("with untracked"), true).await.unwrap().ok);
        assert!(!dir.join("new.txt").exists());
        assert_eq!(stashes().await[0].message, "On main: with untracked");
        assert!(stash_pop(&client, 0, &stashes().await[0].oid).await.unwrap().ok);
        assert!(dir.join("new.txt").exists());
        std::fs::remove_file(dir.join("new.txt")).unwrap();

        // Two stashes: `first` ends up as stash@{1}, `second` as stash@{0}.
        std::fs::write(dir.join("a.txt"), "first\n").unwrap();
        git(&dir, &["stash", "push", "-qm", "first"]);
        std::fs::write(dir.join("a.txt"), "second\n").unwrap();
        git(&dir, &["stash", "push", "-qm", "second"]);
        let list = stashes().await;
        assert_eq!(list.len(), 2);
        let (second, first) = (list[0].clone(), list[1].clone());

        // An index that no longer matches its commit is refused, and nothing is dropped.
        assert!(matches!(stash_drop(&client, 0, &first.oid).await, Err(GitError::InvalidPath { .. })));
        assert_eq!(stashes().await.len(), 2);

        // Apply keeps the stash; drop removes it without touching the working tree.
        assert!(stash_apply(&client, 1, &first.oid).await.unwrap().ok);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "first\n");
        assert_eq!(stashes().await.len(), 2);
        git(&dir, &["checkout", "-q", "--", "a.txt"]);
        assert!(stash_drop(&client, 1, &first.oid).await.unwrap().ok);
        let list = stashes().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].oid, second.oid);

        // Pop applies and removes.
        assert!(stash_pop(&client, 0, &second.oid).await.unwrap().ok);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "second\n");
        assert!(stashes().await.is_empty());

        // A pop that conflicts reports it and keeps the stash.
        git(&dir, &["stash", "push", "-qm", "third"]);
        let third = stashes().await[0].clone();
        std::fs::write(dir.join("a.txt"), "clash\n").unwrap();
        git(&dir, &["commit", "-qam", "clash"]);
        let popped = stash_pop(&client, 0, &third.oid).await.unwrap();
        assert!(!popped.ok && popped.conflicts, "{popped:?}");
        assert_eq!(stashes().await.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn client_for(dir: &Path) -> GitClient {
        let transport: SharedTransport = Arc::new(LocalTransport { git: which::which("git").unwrap(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false });
        let version = crate::git::versions::GitVersion::parse(&Command::new("git").arg("version").output().unwrap().stdout).unwrap();
        GitClient { transport, repo: NativePath(dir.to_string_lossy().into_owned()), caps: Capabilities::for_version(version), console: Arc::new(ConsoleRecorder::new(None, "ops".into(), 4096)) }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// (path, staged, unstaged, untracked) for every status entry, sorted.
    async fn status_of(client: &GitClient) -> Vec<(String, bool, bool, bool)> {
        let mut v: Vec<_> = crate::git::queries::status::get_status(client).await.unwrap()
            .entries.into_iter().map(|e| (e.path, e.staged, e.unstaged, e.untracked)).collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn stage_unstage_and_discard() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-stage-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        // Names that would be pathspec magic, an option, a glob or split on a space.
        let odd = [":colon.txt", "-dash.txt", "*.txt", "sp ace.txt"];
        for name in odd.iter().chain(["keep.txt", "old.txt"].iter()) {
            std::fs::write(dir.join(name), "base\n").unwrap();
        }
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let client = client_for(&dir);

        // Modify the odd names, add an untracked file, rename one.
        for name in odd {
            std::fs::write(dir.join(name), "changed\n").unwrap();
        }
        std::fs::write(dir.join("new.txt"), "new\n").unwrap();
        std::fs::rename(dir.join("old.txt"), dir.join("renamed.txt")).unwrap();

        // Staging `*.txt` stages only that file, not every .txt.
        assert!(stage(&client, &strs(&["*.txt"])).await.unwrap().ok);
        let st = status_of(&client).await;
        assert!(st.contains(&("*.txt".into(), true, false, false)), "{st:?}");
        assert!(st.contains(&(":colon.txt".into(), false, true, false)), "{st:?}");

        // Stage the rest, including the rename as a pair and the untracked file.
        let all = strs(&[":colon.txt", "-dash.txt", "sp ace.txt", "new.txt", "renamed.txt", "old.txt"]);
        assert!(stage(&client, &all).await.unwrap().ok);
        let st = status_of(&client).await;
        assert!(st.iter().all(|(_, staged, unstaged, untracked)| *staged && !unstaged && !untracked), "{st:?}");
        assert!(st.iter().any(|(p, ..)| p == "renamed.txt"));

        // Unstage the rename pair and one odd name; the files stay changed on disk.
        assert!(unstage(&client, &strs(&["renamed.txt", "old.txt", "-dash.txt"])).await.unwrap().ok);
        let st = status_of(&client).await;
        assert!(st.contains(&("-dash.txt".into(), false, true, false)), "{st:?}");
        assert!(st.contains(&("renamed.txt".into(), false, false, true)), "{st:?}");
        assert!(st.contains(&("old.txt".into(), false, true, false)), "{st:?}");
        assert_eq!(std::fs::read_to_string(dir.join("-dash.txt")).unwrap(), "changed\n");

        // Discard to the index: the unstaged change goes, the staged one stays.
        std::fs::write(dir.join(":colon.txt"), "changed again\n").unwrap();
        assert!(discard(&client, &strs(&[":colon.txt", "-dash.txt"]), &[], false).await.unwrap().ok);
        assert_eq!(std::fs::read_to_string(dir.join(":colon.txt")).unwrap(), "changed\n");
        assert_eq!(std::fs::read_to_string(dir.join("-dash.txt")).unwrap(), "base\n");

        // Discard to HEAD: staged and unstaged both go; a staged new file is removed.
        let r = discard(&client, &strs(&[":colon.txt", "sp ace.txt", "*.txt", "new.txt", "old.txt"]), &strs(&["renamed.txt"]), true).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(status_of(&client).await, vec![], "clean tree");
        assert_eq!(std::fs::read_to_string(dir.join(":colon.txt")).unwrap(), "base\n");
        assert!(!dir.join("new.txt").exists() && !dir.join("renamed.txt").exists());

        // Enough untracked files to need more than one `clean` call.
        let many: Vec<String> = (0..400).map(|i| format!("untracked-{i:04}-{}.txt", "x".repeat(40))).collect();
        for p in &many {
            std::fs::write(dir.join(p), "u\n").unwrap();
        }
        let r = discard(&client, &[], &many, false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(status_of(&client).await, vec![]);

        assert!(matches!(stage(&client, &[]).await, Err(GitError::InvalidPath { .. })));
        assert!(matches!(discard(&client, &[], &[], false).await, Err(GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn commit_amend_and_sign_off() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-commit-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "user.name", "T"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        let client = client_for(&dir);
        let body = |rev: &'static str| {
            let c = &client;
            async move { crate::git::queries::log::get_commit_body(c, rev).await.unwrap() }
        };

        // Nothing staged: Git refuses.
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        assert!(!commit(&client, "nothing", None, false, false).await.unwrap().ok);

        // A message with a leading dash, an issue number and non-ASCII text arrives intact.
        stage(&client, &strs(&["a.txt"])).await.unwrap();
        let message = "-leading dash\n\n#123 is the issue\nnaïve café ✓";
        let r = commit(&client, message, None, false, false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(body("HEAD").await, message);

        // Empty message: Git refuses, the index keeps its change.
        std::fs::write(dir.join("a.txt"), "2\n").unwrap();
        stage(&client, &strs(&["a.txt"])).await.unwrap();
        let r = commit(&client, "  \n", None, false, false).await.unwrap();
        assert!(!r.ok && r.stderr.contains("empty commit message"), "{r:?}");

        // Amend with no message keeps the old one and takes the staged change.
        assert!(commit(&client, "", None, true, false).await.unwrap().ok);
        assert_eq!(body("HEAD").await, message);
        assert_eq!(status_of(&client).await, vec![]);

        // Amend with a new message and a sign-off.
        let r = commit(&client, "reworded", None, true, true).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(body("HEAD").await, "reworded\n\nSigned-off-by: T <t@t>");
        let count = Command::new("git").arg("-C").arg(&dir).args(["rev-list", "--count", "HEAD"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "1", "amend did not add a commit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn commit_selected_paths_only() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-commit-paths-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "user.name", "T"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        for name in ["a.txt", "b.txt", "old.txt", "gone.txt"] {
            std::fs::write(dir.join(name), "base\n").unwrap();
        }
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let client = client_for(&dir);
        let files_in = |rev: &str| {
            let out = Command::new("git").arg("-C").arg(&dir)
                .args(["diff-tree", "-r", "--name-only", "--no-commit-id", "--no-renames", rev]).output().unwrap();
            let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect();
            v.sort();
            v
        };

        // b.txt is staged but not selected. The selection covers an unstaged change,
        // an untracked file, a deletion and a rename (both paths).
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.join("b.txt"), "changed\n").unwrap();
        git(&dir, &["add", "b.txt"]);
        std::fs::write(dir.join("new.txt"), "new\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::rename(dir.join("old.txt"), dir.join("renamed.txt")).unwrap();
        let chosen = strs(&["a.txt", "new.txt", "gone.txt", "renamed.txt", "old.txt"]);
        let r = commit(&client, "selected", Some(&chosen), false, false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(files_in("HEAD"), strs(&["a.txt", "gone.txt", "new.txt", "old.txt", "renamed.txt"]));
        assert_eq!(status_of(&client).await, vec![("b.txt".into(), true, false, false)], "b.txt stays staged");

        // With nothing else staged, the selection is the whole commit.
        std::fs::write(dir.join("a.txt"), "again\n").unwrap();
        let r = commit(&client, "just a", Some(&strs(&["a.txt", "b.txt"])), false, false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(files_in("HEAD"), strs(&["a.txt", "b.txt"]));
        assert_eq!(status_of(&client).await, vec![]);

        // An amend naming no paths changes the message and leaves b.txt staged.
        std::fs::write(dir.join("b.txt"), "staged, not amended\n").unwrap();
        git(&dir, &["add", "b.txt"]);
        let r = commit(&client, "just a, reworded", Some(&[]), true, false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(files_in("HEAD"), strs(&["a.txt", "b.txt"]));
        assert_eq!(status_of(&client).await, vec![("b.txt".into(), true, false, false)]);
        assert!(matches!(commit(&client, "x", Some(&[]), false, false).await, Err(GitError::InvalidPath { .. })));
        git(&dir, &["restore", "--staged", "--worktree", "b.txt"]);

        // A failed commit leaves the selection staged.
        std::fs::write(dir.join("a.txt"), "third\n").unwrap();
        let r = commit(&client, "", Some(&strs(&["a.txt"])), false, false).await.unwrap();
        assert!(!r.ok, "{r:?}");
        assert_eq!(status_of(&client).await, vec![("a.txt".into(), true, false, false)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unstage_on_an_unborn_branch() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-unborn-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        let client = client_for(&dir);
        assert!(stage(&client, &strs(&["a.txt"])).await.unwrap().ok);
        assert_eq!(status_of(&client).await, vec![("a.txt".into(), true, false, false)]);
        let r = unstage(&client, &strs(&["a.txt"])).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(status_of(&client).await, vec![("a.txt".into(), false, false, true)]);
        assert!(dir.join("a.txt").exists());
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

    #[tokio::test]
    async fn pull_modes_conflicts_autostash_and_cancel() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-pull-{}", uuid::Uuid::new_v4().simple()));
        let (origin, local) = (root.join("origin"), root.join("local"));
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        std::fs::write(origin.join("a.txt"), "1\n").unwrap();
        git(&origin, &["add", "-A"]); git(&origin, &["commit", "-qm", "base"]);
        let st = Command::new("git").arg("clone").arg("-q").arg(&origin).arg(&local).status().unwrap();
        assert!(st.success());
        // The client adds no identity of its own, and pulls here make merge and rebased commits.
        for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&local, &["config", k, v]);
        }
        let client = client_for(&local);
        let lines = std::sync::Mutex::new(Vec::<String>::new());
        let progress = |l: String| lines.lock().unwrap().push(l);
        let cancel = tokio_util::sync::CancellationToken::new();
        // Both halves, as `op_pull` runs them.
        let pull = |remote: &'static str, branch: &'static str, mode: PullMode, autostash: bool| {
            let (client, progress, cancel) = (&client, &progress, &cancel);
            async move {
                let fetched = pull_fetch(client, remote, branch, progress, cancel).await?;
                if !fetched.ok {
                    return Ok(fetched);
                }
                Ok::<_, GitError>(combine(fetched, pull_integrate(client, remote, branch, mode, autostash).await?))
            }
        };
        let head = |dir: &Path, rev: &str| String::from_utf8(Command::new("git").arg("-C").arg(dir).args(["rev-parse", rev]).output().unwrap().stdout).unwrap();

        // Fast-forward, with progress on stderr and the remote-tracking branch updated.
        std::fs::write(origin.join("b.txt"), "b\n").unwrap();
        git(&origin, &["add", "-A"]); git(&origin, &["commit", "-qm", "remote one"]);
        let ff = pull("origin", "main", PullMode::FastForwardOnly, false).await.unwrap();
        assert!(ff.ok, "{ff:?}");
        assert_eq!(head(&local, "HEAD"), head(&origin, "HEAD"));
        assert_eq!(head(&local, "origin/main"), head(&origin, "HEAD"));
        assert!(lines.lock().unwrap().iter().any(|l| l.contains("FETCH_HEAD")), "{:?}", lines.lock().unwrap());
        assert!(!ff.stderr.contains('\r'));

        // Both sides move on: fast-forward only refuses, merge makes a merge commit.
        std::fs::write(origin.join("c.txt"), "c\n").unwrap();
        git(&origin, &["add", "-A"]); git(&origin, &["commit", "-qm", "remote two"]);
        std::fs::write(local.join("d.txt"), "d\n").unwrap();
        git(&local, &["add", "-A"]); git(&local, &["commit", "-qm", "local one"]);
        let refused = pull("origin", "main", PullMode::FastForwardOnly, false).await.unwrap();
        assert!(!refused.ok && !refused.conflicts && refused.stderr.contains("Not possible to fast-forward"), "{refused:?}");
        // `--no-rebase` wins over `pull.ff=only` in the repository's config.
        git(&local, &["config", "pull.ff", "only"]);
        let merged = pull("origin", "main", PullMode::Merge, false).await.unwrap();
        assert!(merged.ok, "{merged:?}");
        assert_eq!(head(&local, "HEAD^2"), head(&origin, "HEAD"));
        git(&local, &["config", "--unset", "pull.ff"]);

        // A rebase that conflicts stops with conflicts, and aborting puts the branch back.
        std::fs::write(origin.join("a.txt"), "remote\n").unwrap();
        git(&origin, &["commit", "-qam", "remote three"]);
        std::fs::write(local.join("a.txt"), "local\n").unwrap();
        git(&local, &["commit", "-qam", "local two"]);
        let before = head(&local, "HEAD");
        let stopped = pull("origin", "main", PullMode::Rebase, false).await.unwrap();
        assert!(!stopped.ok && stopped.conflicts, "{stopped:?}");
        assert!(local.join(".git/rebase-merge").exists());
        assert!(rebase_abort(&client).await.unwrap().ok);
        assert_eq!(head(&local, "HEAD"), before);

        // The remote rewrites a commit the branch already has: the fork point drops the old copy
        // instead of replaying it, as `git pull --rebase` does.
        git(&local, &["reset", "-q", "--hard", "origin/main"]);
        std::fs::write(origin.join("e.txt"), "old\n").unwrap();
        git(&origin, &["add", "-A"]); git(&origin, &["commit", "-qm", "rewritten later"]);
        assert!(pull("origin", "main", PullMode::Rebase, false).await.unwrap().ok);
        std::fs::write(local.join("f.txt"), "f\n").unwrap();
        git(&local, &["add", "-A"]); git(&local, &["commit", "-qm", "mine"]);
        git(&origin, &["reset", "-q", "--hard", "HEAD~1"]);
        std::fs::write(origin.join("g.txt"), "g\n").unwrap();
        git(&origin, &["add", "-A"]); git(&origin, &["commit", "-qm", "replacement"]);
        let rebased = pull("origin", "main", PullMode::Rebase, false).await.unwrap();
        assert!(rebased.ok, "{rebased:?}");
        let subjects = String::from_utf8(Command::new("git").arg("-C").arg(&local).args(["log", "--format=%s", "-3"]).output().unwrap().stdout).unwrap();
        assert_eq!(subjects, "mine\nreplacement\nremote three\n");

        // Autostash: uncommitted changes that clash with the pulled commits stay in a stash, and
        // Git still exits 0.
        // Two back, so the pulled commits include "remote three", which changes a.txt.
        git(&local, &["reset", "-q", "--hard", "origin/main~2"]);
        std::fs::write(local.join("a.txt"), "dirty\n").unwrap();
        let stashed = pull("origin", "main", PullMode::Merge, true).await.unwrap();
        assert!(stashed.ok && !stashed.conflicts, "{stashed:?}");
        assert!(format!("{}{}", stashed.stdout, stashed.stderr).contains("Applying autostash resulted in conflicts"), "{stashed:?}");
        assert_eq!(crate::git::queries::log::get_stashes(&client).await.unwrap().len(), 1);

        // A cancelled pull stops with `Cancelled`.
        cancel.cancel();
        let cancelled = pull("origin", "main", PullMode::Merge, false).await;
        assert!(matches!(cancelled, Err(GitError::Cancelled)), "{cancelled:?}");
        assert!(matches!(pull("-x", "main", PullMode::Merge, false).await, Err(GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_mode_follows_git_config_precedence() {
        let mode = |cfg: &[u8], branch: Option<&str>| pull_mode_from_config(cfg, branch);
        assert_eq!(mode(b"", Some("main")), None);
        assert_eq!(mode(b"pull.rebase\ntrue\0", None), Some(PullMode::Rebase));
        assert_eq!(mode(b"pull.rebase\nmerges\0", None), Some(PullMode::Rebase));
        assert_eq!(mode(b"pull.rebase\nfalse\0", None), Some(PullMode::Merge));
        assert_eq!(mode(b"pull.ff\nonly\0", None), Some(PullMode::FastForwardOnly));
        assert_eq!(mode(b"pull.ff\nfalse\0", None), Some(PullMode::Merge));
        assert_eq!(mode(b"pull.rebase\nfalse\0pull.ff\nonly\0", None), Some(PullMode::FastForwardOnly));
        // The branch's own setting comes first; a key with no value means true.
        assert_eq!(mode(b"pull.rebase\ntrue\0branch.Dev.x.rebase\nfalse\0", Some("Dev.x")), Some(PullMode::Merge));
        assert_eq!(mode(b"branch.dev.rebase\0", Some("dev")), Some(PullMode::Rebase));
        // The last value for a key wins, as in Git (system, then global, then local config).
        assert_eq!(mode(b"pull.rebase\ntrue\0pull.rebase\nfalse\0", None), Some(PullMode::Merge));
        assert_eq!(regex_escape("feat.x+1"), r"feat\.x\+1");
    }
}
