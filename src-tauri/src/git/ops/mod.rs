//! Write operations. Every one runs as `Category::UserAction`, expects the
//! caller to hold the repository's exclusive lock, and reports Git's own
//! output so the UI can show exactly what happened.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{classify_git_failure, GitError, Result};
use crate::git::queries::in_progress::InProgressKind;
use crate::git::{GitClient, GitReq};
use crate::transport::TransportKind;

pub mod lines;

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
pub(crate) fn valid_revision(rev: &str) -> Result<()> {
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

/// `git fetch --all --progress`: every remote, honouring `fetch.prune` and the remotes' own
/// settings. Stopping it is safe, as for `pull_fetch`.
pub async fn fetch_all(client: &GitClient, progress: &(dyn Fn(String) + Send + Sync), cancel: &tokio_util::sync::CancellationToken) -> Result<OpResult> {
    let req = GitReq::new("fetch", ["fetch", "--all", "--progress"]).user().timeout(None).own_session();
    let out = client.exec_streaming(req, progress, cancel).await?;
    Ok(result(out))
}

/// Where a branch is pushed: a remote and the branch's name there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PushTarget {
    pub remote: String,
    /// The branch name on the remote, without `refs/heads/`.
    pub branch: String,
}

/// The upstream `branch` tracks, from `branch.<branch>.remote` and `branch.<branch>.merge`. None
/// when either is missing, or when the upstream is another local branch (remote `.`).
pub async fn upstream_of(client: &GitClient, branch: &str) -> Result<Option<PushTarget>> {
    valid_ref_name(branch)?;
    let pattern = format!(r"^branch\.{}\.(remote|merge)$", regex_escape(branch));
    let out = client.exec_allow_failure(GitReq::new("config", ["config".to_string(), "-z".into(), "--get-regexp".into(), pattern])).await?;
    if !out.success() && out.exit_code != Some(1) {
        return Err(classify_git_failure("config", vec![], out.exit_code, &out.stderr, &client.transport.kind()));
    }
    Ok(upstream_from_config(&out.stdout, branch))
}

fn upstream_from_config(bytes: &[u8], branch: &str) -> Option<PushTarget> {
    let (mut remote, mut merge) = (None, None);
    for entry in bytes.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let text = String::from_utf8_lossy(entry);
        let Some((key, value)) = text.split_once('\n') else { continue };
        if key == format!("branch.{branch}.remote") {
            remote = Some(value.trim().to_string());
        } else if key == format!("branch.{branch}.merge") {
            merge = Some(value.trim().to_string());
        }
    }
    let remote = remote.filter(|r| !r.is_empty() && r != ".")?;
    let branch = merge?.strip_prefix("refs/heads/")?.to_string();
    Some(PushTarget { remote, branch })
}

/// `git push --progress <remote> refs/heads/<branch>:refs/heads/<target branch>`, with
/// `--set-upstream` when `set_upstream`. Full ref names on both sides, so a tag with the same
/// name as the branch cannot be pushed by mistake. Never forces: a push the remote rejects
/// because it has commits the branch lacks fails with `[rejected]` in stderr (see
/// `push_rejected`). Stopping it is safe for the local repository; if the remote had already
/// taken the update, the next fetch shows it.
pub async fn push(
    client: &GitClient,
    branch: &str,
    target: &PushTarget,
    set_upstream: bool,
    progress: &(dyn Fn(String) + Send + Sync),
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<OpResult> {
    valid_ref_name(branch)?;
    valid_ref_name(&target.remote)?;
    valid_ref_name(&target.branch)?;
    let refspec = format!("refs/heads/{branch}:refs/heads/{}", target.branch);
    let mut args = vec!["push", "--progress"];
    if set_upstream {
        args.push("--set-upstream");
    }
    args.extend([target.remote.as_str(), refspec.as_str()]);
    let out = client.exec_streaming(GitReq::new("push", args).user().timeout(None).own_session(), progress, cancel).await?;
    Ok(result(out))
}

/// How many commits HEAD has that `tracking` (`refs/remotes/origin/dev`) does not: what a push
/// would send.
pub async fn commits_ahead(client: &GitClient, tracking: &str) -> Result<u32> {
    let range = format!("{tracking}..HEAD");
    let out = client.exec(GitReq::new("rev-list", ["rev-list", "--count", &range])).await?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().parse().map_err(|_| GitError::ParseError { what: "rev-list --count".into(), detail: text.trim().to_string() })
}

/// What a sync did: the pull, then the push if it got that far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    /// The upstream the branch was synced with.
    pub target: PushTarget,
    /// The fetch and the merge or rebase, combined as for a pull.
    pub pull: OpResult,
    /// The push, or null when it did not run: the pull failed or stopped on conflicts, there was
    /// nothing to push, or it was stopped.
    pub push: Option<OpResult>,
    /// The pull succeeded and the branch had no commits the upstream lacks, so no push ran.
    pub nothing_to_push: bool,
    /// Stop was pressed after the pull had merged or rebased: the push did not run or did not
    /// finish.
    pub push_stopped: bool,
}

/// True when the remote refused a push because it has commits the pushed branch does not:
/// `! [rejected] dev -> dev (fetch first)` or `(non-fast-forward)`.
pub fn push_rejected(res: &OpResult) -> bool {
    !res.ok && res.stderr.contains("[rejected]")
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

/// Rename local branch `old` to `new`. Git moves its upstream setting and its reflog with it,
/// and refuses when `new` already exists.
pub async fn branch_rename(client: &GitClient, old: &str, new: &str) -> Result<OpResult> {
    valid_ref_name(old)?;
    valid_ref_name(new)?;
    let out = client.exec_allow_failure(GitReq::new("branch", ["branch", "-m", old, new]).user()).await?;
    Ok(result(out))
}

/// Make local branch `branch` track the remote branch `upstream`, given by the full name of its
/// remote-tracking branch (`refs/remotes/origin/dev`), with `git branch --set-upstream-to=<upstream>
/// -- <branch>`. Git then writes `branch.<branch>.remote` and `branch.<branch>.merge`. The full
/// name cannot be mistaken for a local branch that happens to be called `origin/dev`, and Git
/// refuses one that does not exist. With no `upstream`, `git branch --unset-upstream -- <branch>`
/// stops the branch tracking anything; Git fails when it tracks nothing already.
pub async fn branch_set_upstream(client: &GitClient, branch: &str, upstream: Option<&str>) -> Result<OpResult> {
    valid_ref_name(branch)?;
    let args: Vec<String> = match upstream {
        Some(u) => {
            valid_ref_name(u)?;
            if !u.starts_with("refs/remotes/") {
                return Err(GitError::InvalidPath { path: u.to_string(), reason: "not a remote branch (refs/remotes/…)".into() });
            }
            vec!["branch".into(), format!("--set-upstream-to={u}"), "--".into(), branch.to_string()]
        }
        None => vec!["branch".into(), "--unset-upstream".into(), "--".into(), branch.to_string()],
    };
    let out = client.exec_allow_failure(GitReq::new("branch", args).user()).await?;
    Ok(result(out))
}

/// Delete `branch` on `remote`: `git push --progress <remote> --delete refs/heads/<branch>`. The
/// full ref name means a tag of the same name on the remote cannot be deleted instead, and it also
/// makes Git remove the remote-tracking branch `<remote>/<branch>` here. When the remote no longer
/// has the branch, Git 2.43 still exits 0 and the remote prints "warning: deleting a non-existent
/// ref" in stderr; the frontend looks for that. A remote can refuse the deletion (a protected
/// branch, the branch its HEAD points at, no permission), which comes back as `[remote rejected]`
/// in stderr. Stopping it is safe for the local repository; if the remote had already deleted the
/// branch, the next fetch shows it.
pub async fn delete_remote_branch(
    client: &GitClient,
    remote: &str,
    branch: &str,
    progress: &(dyn Fn(String) + Send + Sync),
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<OpResult> {
    valid_ref_name(remote)?;
    valid_ref_name(branch)?;
    let refname = format!("refs/heads/{branch}");
    let req = GitReq::new("push", ["push", "--progress", remote, "--delete", refname.as_str()]).user().timeout(None).own_session();
    let out = client.exec_streaming(req, progress, cancel).await?;
    Ok(result(out))
}

/// How much `reset` puts back besides moving the branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ResetMode {
    /// Move the branch only: the index and the files keep their content, so the undone commits'
    /// changes show as staged.
    Soft,
    /// Move the branch and reset the index: the undone changes show as unstaged.
    Mixed,
    /// Move the branch and reset the index and the files: every uncommitted change is lost.
    Hard,
}

/// Move the current branch (or a detached HEAD) to `target`.
pub async fn reset(client: &GitClient, mode: ResetMode, target: &str) -> Result<OpResult> {
    valid_revision(target)?;
    let flag = match mode {
        ResetMode::Soft => "--soft",
        ResetMode::Mixed => "--mixed",
        ResetMode::Hard => "--hard",
    };
    // `--` keeps a target that is also a file name from being read as a path.
    let out = client.exec_allow_failure(GitReq::new("reset", ["reset", "-q", flag, target, "--"]).user()).await?;
    Ok(result(out))
}

/// `-m <n>` for a merge commit: which parent its changes are measured against (1 is the branch
/// that was merged into). Git refuses a merge commit without it, and a plain commit with it.
fn mainline_args(mainline: Option<u32>) -> Vec<String> {
    match mainline {
        Some(n) => vec!["-m".into(), n.to_string()],
        None => Vec::new(),
    }
}

/// Apply the change `commit` made as a new commit on the current branch. When it stops on
/// conflicts (`conflicts` in the result) or because the change is already there (Git then
/// leaves the cherry-pick open and says so in stderr), `cherry_pick_abort` puts the branch back.
pub async fn cherry_pick(client: &GitClient, commit: &str, mainline: Option<u32>) -> Result<OpResult> {
    valid_revision(commit)?;
    let mut args: Vec<String> = vec!["cherry-pick".into()];
    args.extend(mainline_args(mainline));
    args.push(commit.to_string());
    let out = client.exec_allow_failure(GitReq::new("cherry-pick", args).user().timeout(Some(COMMIT_TIMEOUT))).await?;
    Ok(result(out))
}

pub async fn cherry_pick_abort(client: &GitClient) -> Result<OpResult> {
    let out = client.exec_allow_failure(GitReq::new("cherry-pick", ["cherry-pick", "--abort"]).user()).await?;
    Ok(result(out))
}

/// Make a new commit that undoes `commit`, with Git's own "Revert …" message. Stops on
/// conflicts like `cherry_pick`; `revert_abort` puts the branch back.
pub async fn revert(client: &GitClient, commit: &str, mainline: Option<u32>) -> Result<OpResult> {
    valid_revision(commit)?;
    let mut args: Vec<String> = vec!["revert".into(), "--no-edit".into()];
    args.extend(mainline_args(mainline));
    args.push(commit.to_string());
    let out = client.exec_allow_failure(GitReq::new("revert", args).user().timeout(Some(COMMIT_TIMEOUT))).await?;
    Ok(result(out))
}

pub async fn revert_abort(client: &GitClient) -> Result<OpResult> {
    let out = client.exec_allow_failure(GitReq::new("revert", ["revert", "--abort"]).user()).await?;
    Ok(result(out))
}

/// Replay the current branch's own commits on top of `onto` (`git rebase <onto>`, not
/// interactive). When a commit conflicts, Git stops with `conflicts` in the result and the rebase
/// stays open: `continue_operation`, `skip_operation` or `rebase_abort` go on from there.
pub async fn rebase(client: &GitClient, onto: &str) -> Result<OpResult> {
    valid_revision(onto)?;
    let out = client.exec_allow_failure(GitReq::new("rebase", ["rebase", onto]).user().timeout(Some(COMMIT_TIMEOUT))).await?;
    Ok(result(out))
}

/// The Git command that owns an operation in progress.
fn operation_command(kind: InProgressKind) -> &'static str {
    match kind {
        InProgressKind::Merge => "merge",
        InProgressKind::Rebase => "rebase",
        InProgressKind::CherryPick => "cherry-pick",
        InProgressKind::Revert => "revert",
    }
}

/// `git <merge|rebase|cherry-pick|revert> --continue`, once every conflict is resolved and staged.
/// Each makes a commit, and Git would open an editor for its message; `GIT_EDITOR=true` keeps
/// the message Git prepared (the merge message, or the original commit's) without one. Git
/// refuses while a file is still unmerged, and a rebase or cherry-pick whose resolution left no
/// change stops with "nothing to commit": skip that commit instead. A rebase that continues into
/// the next commit's conflict reports `conflicts` again.
pub async fn continue_operation(client: &GitClient, kind: InProgressKind) -> Result<OpResult> {
    let command = operation_command(kind);
    let req = GitReq::new(command, [command, "--continue"]).env("GIT_EDITOR", "true").user().timeout(Some(COMMIT_TIMEOUT));
    Ok(result(client.exec_allow_failure(req).await?))
}

/// Leave out the commit a rebase or cherry-pick stopped at and go on with the next one
/// (`--skip`), throwing away its changes and any resolution so far. A merge or revert has nothing
/// to skip to.
pub async fn skip_operation(client: &GitClient, kind: InProgressKind) -> Result<OpResult> {
    if !matches!(kind, InProgressKind::Rebase | InProgressKind::CherryPick) {
        return Err(GitError::InvalidPath { path: operation_command(kind).into(), reason: "only a rebase or a cherry-pick can skip a commit".into() });
    }
    let command = operation_command(kind);
    let req = GitReq::new(command, [command, "--skip"]).env("GIT_EDITOR", "true").user().timeout(Some(COMMIT_TIMEOUT));
    Ok(result(client.exec_allow_failure(req).await?))
}

/// Which version of a conflicted file to keep. In Git's terms: `Ours` is stage 2, the branch
/// being merged into (for a rebase, the branch being rebased onto, with the commits already
/// replayed); `Theirs` is stage 3, what is coming in (for a rebase, the commit being replayed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ResolveSide {
    Ours,
    Theirs,
}

/// The stages (1 base, 2 ours, 3 theirs) of every unmerged path in the index, from
/// `git ls-files -u -z`: `<mode> <oid> <stage>\t<path>`.
pub(crate) fn parse_unmerged(bytes: &[u8]) -> std::collections::HashMap<String, Vec<u8>> {
    let mut stages: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    for rec in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(rec);
        let Some((meta, path)) = text.split_once('\t') else { continue };
        let Some(stage) = meta.split(' ').nth(2).and_then(|s| s.parse::<u8>().ok()) else { continue };
        stages.entry(path.to_string()).or_default().push(stage);
    }
    stages
}

/// Every unmerged path and its stages. Lists the whole index, since `ls-files` takes no pathspec
/// on stdin; the unmerged entries are few.
pub async fn unmerged_stages(client: &GitClient) -> Result<std::collections::HashMap<String, Vec<u8>>> {
    let out = client.exec(GitReq::new("ls-files", ["ls-files", "-u", "-z"])).await?;
    Ok(parse_unmerged(&out.stdout))
}

/// Resolve conflicted `paths` by keeping one side's version whole, and mark them resolved.
/// Where that side has the file, `git checkout --ours|--theirs` writes it and `git add` stages
/// it; where that side deleted it (deleted by us or by them), `git rm` deletes it. Refuses, and
/// changes nothing, when a path is not conflicted any more.
pub async fn resolve_conflicts(client: &GitClient, paths: &[String], side: ResolveSide) -> Result<OpResult> {
    valid_paths(paths)?;
    let stages = unmerged_stages(client).await?;
    let wanted = match side {
        ResolveSide::Ours => 2,
        ResolveSide::Theirs => 3,
    };
    let (mut keep, mut delete) = (Vec::new(), Vec::new());
    for p in paths {
        match stages.get(p) {
            Some(s) if s.contains(&wanted) => keep.push(p.clone()),
            Some(_) => delete.push(p.clone()),
            None => return Err(GitError::InvalidPath { path: p.clone(), reason: "the file is not conflicted any more".into() }),
        }
    }
    let flag = match side {
        ResolveSide::Ours => "--ours",
        ResolveSide::Theirs => "--theirs",
    };
    let mut res: Option<OpResult> = None;
    let steps: [(&'static str, &[&str], &Vec<String>); 3] = [
        ("checkout", &["checkout", flag], &keep),
        ("add", &["add", "-A"], &keep),
        ("rm", &["rm", "-q"], &delete),
    ];
    for (label, args, list) in steps {
        if list.is_empty() {
            continue;
        }
        let r = exec_with_paths(client, label, args, list).await?;
        let ok = r.ok;
        res = Some(match res.take() {
            Some(prev) => combine(prev, r),
            None => r,
        });
        if !ok {
            break;
        }
    }
    Ok(res.expect("at least one path is given"))
}

/// Write Git's merge of conflicted `path` to the working tree again, with conflict markers in the
/// `diff3` style, which carry each block's base text (`git checkout --conflict=diff3`). Whatever
/// has been done to the file since is lost, so the frontend asks first. Refuses, and changes
/// nothing, when the path is not conflicted any more: Git would then write the index's version
/// over the file instead.
pub async fn restore_conflict(client: &GitClient, path: &str) -> Result<OpResult> {
    let paths = [path.to_string()];
    valid_paths(&paths)?;
    if !unmerged_stages(client).await?.contains_key(path) {
        return Err(GitError::InvalidPath { path: path.to_string(), reason: "the file is not conflicted any more".into() });
    }
    exec_with_paths(client, "checkout", &["checkout", "--conflict=diff3"], &paths).await
}

/// Save `content` as working tree file `path` (relative to the repository), byte for byte, and
/// return the hash of what was written. `expected_hash` is the `hash` the file had when the merge
/// editor opened it (see `conflict_file::content_hash`); when the file on disk no longer has it,
/// nothing is written and this fails with `changedOnDisk`, so a change made meanwhile by another
/// program is never overwritten unasked. The file is written in place, keeping its mode.
pub async fn write_worktree_file(client: &GitClient, path: &str, content: &str, expected_hash: &str) -> Result<String> {
    use crate::git::queries::conflict_file::content_hash;
    valid_worktree_path(path)?;
    let full = crate::git::queries::diff::join_repo_path(&client.repo, path);
    // A file that has grown past the cap since it was opened has certainly changed.
    let on_disk = match client.transport.read_file(&full, crate::git::queries::blob::BLOB_CAP).await {
        Ok(bytes) => Some(bytes),
        Err(GitError::TooLarge { .. }) => None,
        Err(e) => return Err(e),
    };
    if on_disk.is_none_or(|bytes| content_hash(&bytes) != expected_hash) {
        return Err(GitError::ChangedOnDisk { path: path.to_string() });
    }
    client.transport.write_file(&full, content.as_bytes()).await?;
    Ok(content_hash(content.as_bytes()))
}

/// A repository-relative path that is written or read as a file directly, not handed to Git as
/// a pathspec: on top of `valid_paths`, it must stay inside the working tree, so it may not be
/// absolute or step up with `..`.
pub(crate) fn valid_worktree_path(path: &str) -> Result<()> {
    valid_paths(&[path.to_string()])?;
    let bytes = path.as_bytes();
    // `C:` starts a Windows path on a drive, even without a slash after it.
    let absolute = path.starts_with(['/', '\\']) || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':');
    if absolute || path.split(['/', '\\']).any(|part| part == "..") {
        return Err(GitError::InvalidPath { path: path.to_string(), reason: "not a path inside the working tree".into() });
    }
    Ok(())
}

/// `git config --global --add safe.directory <path>`, which tells Git to trust a repository owned
/// by another user. Run through the repository's own transport with the path exactly as Git
/// printed it in its "dubious ownership" message, so the Git that refused gets the setting.
/// Git runs this even inside the repository it refuses.
pub fn safe_directory_req(path: &str) -> GitReq {
    GitReq::new("config", ["config", "--global", "--add", "safe.directory", path]).user()
}

pub async fn add_safe_directory(client: &GitClient, path: &str) -> Result<OpResult> {
    if path.is_empty() || path.chars().any(char::is_control) {
        return Err(GitError::InvalidPath { path: path.to_string(), reason: "not a valid folder".into() });
    }
    Ok(result(client.exec_allow_failure(safe_directory_req(path)).await?))
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
        // Names that would be pathspec magic, an option, a glob or split on a space. Windows
        // forbids `:` and `*` in file names, so there the glob is `[ab].txt` and the other name
        // is `a.txt`, which that glob would match if Git read it as one.
        let (glob, magic) = if cfg!(windows) { ("[ab].txt", "a.txt") } else { ("*.txt", ":colon.txt") };
        let odd = [magic, "-dash.txt", glob, "sp ace.txt"];
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

        // Staging the glob name stages only that file, not every file it would match.
        assert!(stage(&client, &strs(&[glob])).await.unwrap().ok);
        let st = status_of(&client).await;
        assert!(st.contains(&(glob.into(), true, false, false)), "{st:?}");
        assert!(st.contains(&(magic.into(), false, true, false)), "{st:?}");

        // Stage the rest, including the rename as a pair and the untracked file.
        let all = strs(&[magic, "-dash.txt", "sp ace.txt", "new.txt", "renamed.txt", "old.txt"]);
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
        std::fs::write(dir.join(magic), "changed again\n").unwrap();
        assert!(discard(&client, &strs(&[magic, "-dash.txt"]), &[], false).await.unwrap().ok);
        assert_eq!(std::fs::read_to_string(dir.join(magic)).unwrap(), "changed\n");
        assert_eq!(std::fs::read_to_string(dir.join("-dash.txt")).unwrap(), "base\n");

        // Discard to HEAD: staged and unstaged both go; a staged new file is removed.
        let r = discard(&client, &strs(&[magic, "sp ace.txt", glob, "new.txt", "old.txt"]), &strs(&["renamed.txt"]), true).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(status_of(&client).await, vec![], "clean tree");
        assert_eq!(std::fs::read_to_string(dir.join(magic)).unwrap(), "base\n");
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

    fn out(dir: &Path, args: &[&str]) -> String {
        String::from_utf8(Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().stdout).unwrap().trim().to_string()
    }

    #[tokio::test]
    async fn rename_reset_cherry_pick_and_revert() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-history-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&dir, &["config", k, v]);
        }
        let commit = |file: &str, text: &str, msg: &str| {
            std::fs::write(dir.join(file), text).unwrap();
            git(&dir, &["add", "-A"]);
            git(&dir, &["commit", "-qm", msg]);
        };
        commit("a.txt", "1\n", "one");
        commit("a.txt", "2\n", "two");
        let client = client_for(&dir);

        // Rename: the branch keeps its commits; a taken name and a bad name are refused.
        git(&dir, &["branch", "topic"]);
        assert!(branch_rename(&client, "topic", "feature/x").await.unwrap().ok);
        assert_eq!(out(&dir, &["branch", "--list", "feature/x"]), "feature/x");
        assert!(!branch_rename(&client, "feature/x", "main").await.unwrap().ok);
        assert!(matches!(branch_rename(&client, "feature/x", "bad name").await, Err(GitError::InvalidPath { .. })));

        // Reset: soft keeps the change staged, mixed unstaged, hard drops it.
        let two = out(&dir, &["rev-parse", "HEAD"]);
        assert!(reset(&client, ResetMode::Soft, "HEAD~1").await.unwrap().ok);
        assert_eq!(out(&dir, &["status", "--porcelain"]), "M  a.txt");
        git(&dir, &["reset", "-q", "--hard", &two]);
        assert!(reset(&client, ResetMode::Mixed, "HEAD~1").await.unwrap().ok);
        assert_eq!(out(&dir, &["status", "--porcelain"]), "M a.txt");
        assert!(reset(&client, ResetMode::Hard, &two).await.unwrap().ok);
        assert_eq!(out(&dir, &["status", "--porcelain"]), "");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "2\n");
        // A target that is also a file name still means the revision.
        git(&dir, &["tag", "b.txt", "HEAD~1"]);
        std::fs::write(dir.join("b.txt"), "file\n").unwrap();
        assert!(reset(&client, ResetMode::Mixed, "b.txt").await.unwrap().ok);
        assert_eq!(out(&dir, &["log", "-1", "--format=%s"]), "one");
        git(&dir, &["reset", "-q", "--hard", &two]);
        std::fs::remove_file(dir.join("b.txt")).unwrap();

        // Cherry-pick a commit from another branch, then one that is already there.
        git(&dir, &["checkout", "-q", "-b", "side", "HEAD~1"]);
        commit("c.txt", "c\n", "add c");
        let add_c = out(&dir, &["rev-parse", "HEAD"]);
        git(&dir, &["checkout", "-q", "main"]);
        let picked = cherry_pick(&client, &add_c, None).await.unwrap();
        assert!(picked.ok, "{picked:?}");
        assert_eq!(out(&dir, &["log", "-1", "--format=%s"]), "add c");
        let again = cherry_pick(&client, &add_c, None).await.unwrap();
        assert!(!again.ok && !again.conflicts, "{again:?}");
        assert!(cherry_pick_abort(&client).await.unwrap().ok);
        assert_eq!(out(&dir, &["status", "--porcelain"]), "");

        // A conflicting cherry-pick reports conflicts, and abort puts the branch back.
        git(&dir, &["checkout", "-q", "side"]);
        commit("a.txt", "side\n", "side edit");
        let side_edit = out(&dir, &["rev-parse", "HEAD"]);
        git(&dir, &["checkout", "-q", "main"]);
        let head = out(&dir, &["rev-parse", "HEAD"]);
        let clash = cherry_pick(&client, &side_edit, None).await.unwrap();
        assert!(!clash.ok && clash.conflicts, "{clash:?}");
        assert!(cherry_pick_abort(&client).await.unwrap().ok);
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), head);
        assert_eq!(out(&dir, &["status", "--porcelain"]), "");

        // A merge commit needs a mainline for both.
        git(&dir, &["merge", "-q", "--no-ff", "--no-edit", "-X", "theirs", "side"]);
        let merge = out(&dir, &["rev-parse", "HEAD"]);
        assert!(!revert(&client, &merge, None).await.unwrap().ok);
        let reverted = revert(&client, &merge, Some(1)).await.unwrap();
        assert!(reverted.ok, "{reverted:?}");
        assert!(out(&dir, &["log", "-1", "--format=%s"]).starts_with("Revert \"Merge branch 'side'"));
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "2\n");

        // Reverting a commit whose lines changed again later conflicts; abort restores HEAD.
        commit("a.txt", "3\n", "three");
        let clash = revert(&client, &two, None).await.unwrap();
        assert!(!clash.ok && clash.conflicts, "{clash:?}");
        assert!(revert_abort(&client).await.unwrap().ok);
        assert_eq!(out(&dir, &["log", "-1", "--format=%s"]), "three");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rebase_continue_skip_abort_and_the_operation_in_progress() {
        use crate::git::queries::in_progress::get_in_progress;
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-rebase-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&dir, &["config", k, v]);
        }
        let commit = |file: &str, text: &str, msg: &str| {
            std::fs::write(dir.join(file), text).unwrap();
            git(&dir, &["add", "-A"]);
            git(&dir, &["commit", "-qm", msg]);
        };
        let git_dir = dir.join(".git").to_string_lossy().into_owned();
        let client = client_for(&dir);
        commit("a.txt", "base\n", "base");
        commit("b.txt", "base b\n", "base b");
        // topic: three commits; the second and third clash with main.
        git(&dir, &["checkout", "-q", "-b", "topic"]);
        commit("t1.txt", "t1\n", "topic one");
        commit("a.txt", "topic\n", "topic two");
        commit("b.txt", "topic b\n", "topic three");
        let topic_two = out(&dir, &["rev-parse", "HEAD~1"]);
        // clean: one commit that touches nothing main touches.
        git(&dir, &["checkout", "-q", "-b", "clean", "main"]);
        commit("c.txt", "c\n", "clean one");
        git(&dir, &["checkout", "-q", "main"]);
        commit("a.txt", "main\n", "main a");
        commit("b.txt", "main b\n", "main b");
        let main = out(&dir, &["rev-parse", "main"]);

        // Nothing in progress.
        assert_eq!(get_in_progress(&client, &git_dir).await, None);

        // A clean rebase replays the commit on top of main.
        git(&dir, &["checkout", "-q", "clean"]);
        let clean = rebase(&client, "main").await.unwrap();
        assert!(clean.ok && !clean.conflicts, "{clean:?}");
        assert_eq!(out(&dir, &["rev-parse", "HEAD~1"]), main);
        assert_eq!(get_in_progress(&client, &git_dir).await, None);

        // A conflicting rebase stops at the second of three commits, and says so; abort puts
        // the branch back.
        git(&dir, &["checkout", "-q", "topic"]);
        let before = out(&dir, &["rev-parse", "HEAD"]);
        let stopped = rebase(&client, "main").await.unwrap();
        assert!(!stopped.ok && stopped.conflicts, "{stopped:?}");
        let op = get_in_progress(&client, &git_dir).await.expect("a rebase in progress");
        assert_eq!(op.kind, InProgressKind::Rebase);
        assert_eq!(op.branch.as_deref(), Some("refs/heads/topic"));
        assert_eq!(op.onto.as_deref(), Some(main.as_str()));
        assert_eq!((op.step, op.total), (Some(2), Some(3)));
        assert_eq!(op.commit.as_deref(), Some(topic_two.as_str()));
        assert_eq!(op.subject.as_deref(), Some("topic two"));
        assert!(matches!(skip_operation(&client, InProgressKind::Merge).await, Err(GitError::InvalidPath { .. })));
        assert!(rebase_abort(&client).await.unwrap().ok);
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), before);
        assert_eq!(get_in_progress(&client, &git_dir).await, None);

        // Again: Continue is refused while a.txt is unmerged. Taking topic's version of a.txt
        // ("theirs" during a rebase) resolves it; Continue then stops on the third commit's
        // conflict, and Skip leaves that commit out and finishes.
        assert!(rebase(&client, "main").await.unwrap().conflicts);
        let refused = continue_operation(&client, InProgressKind::Rebase).await.unwrap();
        assert!(!refused.ok, "{refused:?}");
        let taken = resolve_conflicts(&client, &strs(&["a.txt"]), ResolveSide::Theirs).await.unwrap();
        assert!(taken.ok, "{taken:?}");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "topic\n");
        let next = continue_operation(&client, InProgressKind::Rebase).await.unwrap();
        assert!(!next.ok && next.conflicts, "{next:?}");
        let op = get_in_progress(&client, &git_dir).await.expect("still rebasing");
        assert_eq!((op.step, op.total, op.subject.as_deref()), (Some(3), Some(3), Some("topic three")));
        let skipped = skip_operation(&client, InProgressKind::Rebase).await.unwrap();
        assert!(skipped.ok, "{skipped:?}");
        assert_eq!(get_in_progress(&client, &git_dir).await, None);
        assert_eq!(out(&dir, &["log", "--format=%s", "-4"]), "topic two\ntopic one\nmain b\nmain a");
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "main b\n");
        assert_eq!(out(&dir, &["status", "--porcelain"]), "");

        // The apply backend keeps its state elsewhere; it reads the same.
        git(&dir, &["checkout", "-q", "-b", "applied", &before]);
        let st = Command::new("git").arg("-C").arg(&dir).args(["rebase", "--apply", "main"]).output().unwrap();
        assert!(!st.status.success());
        let op = get_in_progress(&client, &git_dir).await.expect("an apply rebase in progress");
        assert_eq!(op.kind, InProgressKind::Rebase);
        assert_eq!(op.branch.as_deref(), Some("refs/heads/applied"));
        assert_eq!((op.step, op.total), (Some(2), Some(3)));
        assert_eq!(op.commit.as_deref(), Some(topic_two.as_str()));
        assert_eq!(op.subject.as_deref(), Some("topic two"));
        assert!(rebase_abort(&client).await.unwrap().ok);
        assert!(matches!(rebase(&client, "-x").await, Err(GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn merge_cherry_pick_and_revert_conflicts_resolved_and_continued() {
        use crate::git::queries::diff::{get_diff, ConflictVersion, DiffOptions, DiffTarget};
        use crate::git::queries::in_progress::get_in_progress;
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-conflicts-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git(&dir, &["config", k, v]);
        }
        let git_dir = dir.join(".git").to_string_lossy().into_owned();
        let client = client_for(&dir);
        let write = |file: &str, text: &str| std::fs::write(dir.join(file), text).unwrap();
        for f in ["a.txt", "d.txt", "k.txt"] {
            write(f, &format!("{f} base\n"));
        }
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "base"]);
        // feature changes a.txt, deletes d.txt and changes k.txt; main changes a.txt and d.txt
        // and deletes k.txt: a.txt both modified, d.txt deleted by them, k.txt deleted by us.
        git(&dir, &["checkout", "-q", "-b", "feature"]);
        write("a.txt", "feature\n");
        write("k.txt", "feature k\n");
        git(&dir, &["rm", "-q", "d.txt"]);
        git(&dir, &["commit", "-qam", "feature"]);
        let feature = out(&dir, &["rev-parse", "HEAD"]);
        git(&dir, &["checkout", "-q", "main"]);
        write("a.txt", "main\n");
        write("d.txt", "main d\n");
        git(&dir, &["rm", "-q", "k.txt"]);
        git(&dir, &["commit", "-qam", "main"]);

        let m = merge(&client, "feature", false).await.unwrap();
        assert!(!m.ok && m.conflicts, "{m:?}");
        let op = get_in_progress(&client, &git_dir).await.expect("a merge in progress");
        assert_eq!(op.kind, InProgressKind::Merge);
        assert_eq!(op.commit.as_deref(), Some(feature.as_str()));
        assert_eq!(op.subject.as_deref(), Some("Merge branch 'feature'"));
        // The status every refresh reads carries it.
        let full = crate::git::queries::full_status(&client, &git_dir).await.unwrap();
        assert_eq!((full.in_progress, full.conflicted_count), (Some(op), 3));
        let st = status_of(&client).await;
        assert_eq!(st.len(), 3, "{st:?}");

        // Each comparison of the conflict versions; a version the conflict lacks is empty.
        let diff = |path: &'static str, from: ConflictVersion, to: ConflictVersion| {
            let c = &client;
            async move { get_diff(c, &DiffTarget::Conflict { from, to }, path, None, &DiffOptions::default()).await.unwrap().text }
        };
        let base_ours = diff("a.txt", ConflictVersion::Base, ConflictVersion::Ours).await;
        assert!(base_ours.contains("-a.txt base\n+main\n"), "{base_ours}");
        let base_theirs = diff("a.txt", ConflictVersion::Base, ConflictVersion::Theirs).await;
        assert!(base_theirs.contains("-a.txt base\n+feature\n"), "{base_theirs}");
        let ours_theirs = diff("a.txt", ConflictVersion::Ours, ConflictVersion::Theirs).await;
        assert!(ours_theirs.contains("-main\n+feature\n"), "{ours_theirs}");
        let on_disk = diff("a.txt", ConflictVersion::Ours, ConflictVersion::WorkingTree).await;
        assert!(on_disk.contains("+<<<<<<< HEAD") && !on_disk.contains("Unmerged path"), "{on_disk}");
        let k_added = diff("k.txt", ConflictVersion::Ours, ConflictVersion::Theirs).await;
        assert!(k_added.contains("@@ -0,0 +1,1 @@\n+feature k\n"), "{k_added}");
        let d_removed = diff("d.txt", ConflictVersion::Ours, ConflictVersion::Theirs).await;
        assert!(d_removed.contains("@@ -1,1 +0,0 @@\n-main d\n"), "{d_removed}");

        // Theirs deleted d.txt and ours deleted k.txt, so taking those sides deletes the files;
        // a.txt keeps ours. A file that is no longer conflicted is refused.
        assert!(resolve_conflicts(&client, &strs(&["d.txt"]), ResolveSide::Theirs).await.unwrap().ok);
        assert!(!dir.join("d.txt").exists());
        let r = resolve_conflicts(&client, &strs(&["k.txt", "a.txt"]), ResolveSide::Ours).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert!(!dir.join("k.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "main\n");
        assert!(matches!(resolve_conflicts(&client, &strs(&["a.txt"]), ResolveSide::Theirs).await, Err(GitError::InvalidPath { .. })));
        let done = continue_operation(&client, InProgressKind::Merge).await.unwrap();
        assert!(done.ok, "{done:?}");
        assert_eq!(out(&dir, &["log", "-1", "--format=%s%n%P"]).lines().next(), Some("Merge branch 'feature'"));
        assert_eq!(out(&dir, &["rev-parse", "HEAD^2"]), feature);
        assert_eq!(get_in_progress(&client, &git_dir).await, None);

        // A cherry-pick that conflicts, then is skipped.
        git(&dir, &["checkout", "-q", "-b", "side", "HEAD~2"]);
        write("a.txt", "side\n");
        git(&dir, &["commit", "-qam", "side edit"]);
        let side = out(&dir, &["rev-parse", "HEAD"]);
        git(&dir, &["checkout", "-q", "main"]);
        assert!(cherry_pick(&client, &side, None).await.unwrap().conflicts);
        let op = get_in_progress(&client, &git_dir).await.expect("a cherry-pick in progress");
        assert_eq!((op.kind, op.commit.as_deref(), op.subject.as_deref()), (InProgressKind::CherryPick, Some(side.as_str()), Some("side edit")));
        assert!(skip_operation(&client, InProgressKind::CherryPick).await.unwrap().ok);
        assert_eq!(get_in_progress(&client, &git_dir).await, None);
        assert_eq!(out(&dir, &["status", "--porcelain"]), "");

        // A revert that conflicts, resolved by hand and staged, then continued with Git's message.
        write("a.txt", "one\n");
        git(&dir, &["commit", "-qam", "set one"]);
        write("a.txt", "two\n");
        git(&dir, &["commit", "-qam", "set two"]);
        let target = out(&dir, &["rev-parse", "HEAD~1"]);
        assert!(revert(&client, &target, None).await.unwrap().conflicts);
        let op = get_in_progress(&client, &git_dir).await.expect("a revert in progress");
        assert_eq!(op.kind, InProgressKind::Revert);
        assert_eq!(op.commit.as_deref(), Some(target.as_str()));
        write("a.txt", "resolved by hand\n");
        assert!(stage(&client, &strs(&["a.txt"])).await.unwrap().ok);
        let done = continue_operation(&client, InProgressKind::Revert).await.unwrap();
        assert!(done.ok, "{done:?}");
        assert_eq!(out(&dir, &["log", "-1", "--format=%s"]), "Revert \"set one\"");
        assert_eq!(get_in_progress(&client, &git_dir).await, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn safe_directory_lets_git_open_a_repository_it_refused() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-safe-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        let global = dir.join("global.gitconfig");
        std::fs::write(&global, "").unwrap();
        let client = client_for(&dir);
        // Git's own test switch makes it treat the folder as owned by someone else, and the
        // scratch global config keeps the real one untouched.
        let as_other = |req: GitReq| req.env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1").env("GIT_CONFIG_GLOBAL", &global.to_string_lossy());
        let refused = client.exec(as_other(GitReq::new("rev-parse", ["rev-parse", "--git-dir"]))).await;
        let Err(GitError::UnsafeRepository { path }) = refused else { panic!("{refused:?}") };
        assert!(!path.is_empty());
        let added = client.exec_allow_failure(as_other(safe_directory_req(&path))).await.unwrap();
        assert!(added.success(), "{added:?}");
        assert!(client.exec(as_other(GitReq::new("rev-parse", ["rev-parse", "--git-dir"]))).await.is_ok());
        assert!(std::fs::read_to_string(&global).unwrap().contains("directory ="));
        assert!(matches!(add_safe_directory(&client, "").await, Err(GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stage_unstage_and_discard_single_lines() {
        use super::lines::{apply_lines, ChangeKind, LineAction, SelectedLine};
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-lines-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "core.autocrlf", "false"]);
        let numbered: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("f.txt"), &numbered).unwrap();
        std::fs::write(dir.join("tail.txt"), "a\nb").unwrap();
        std::fs::write(dir.join("crlf.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let client = client_for(&dir);
        let add = |line: u32, text: &str| SelectedLine { kind: ChangeKind::Add, line, text: text.into() };
        let del = |line: u32, text: &str| SelectedLine { kind: ChangeKind::Del, line, text: text.into() };
        let index = |p: &str| out(&dir, &["show", &format!(":{p}")]);

        // Two separate edits; stage only the second, as a removal and an addition.
        let edited = numbered.replace("line 2\n", "line two\n").replace("line 15\n", "line fifteen\n");
        std::fs::write(dir.join("f.txt"), &edited).unwrap();
        let r = apply_lines(&client, LineAction::Stage, "f.txt", false, &[del(15, "line 15"), add(15, "line fifteen")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        let staged = index("f.txt");
        assert!(staged.contains("line fifteen") && staged.contains("line 2\n") && !staged.contains("line two"), "{staged}");
        // Only the addition of the first edit: the old line stays next to the new one.
        let r = apply_lines(&client, LineAction::Stage, "f.txt", false, &[add(2, "line two")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert!(index("f.txt").starts_with("line 1\nline 2\nline two\nline 3\n"));

        // Unstage the second edit's addition only: the index keeps "line 15" and loses "line fifteen".
        let r = apply_lines(&client, LineAction::Unstage, "f.txt", false, &[add(16, "line fifteen")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        let staged = index("f.txt");
        assert!(!staged.contains("line fifteen") && !staged.contains("line 15\n"), "{staged}");
        let r = apply_lines(&client, LineAction::Unstage, "f.txt", false, &[del(15, "line 15")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert!(index("f.txt").contains("line 15\n"));

        // A selection that no longer matches the file is refused, and nothing changes.
        let before = index("f.txt");
        assert!(matches!(apply_lines(&client, LineAction::Stage, "f.txt", false, &[add(15, "something else")]).await, Err(GitError::InvalidPath { .. })));
        assert_eq!(index("f.txt"), before);

        // Discard the unstaged "line fifteen" edit from the working tree; the staged edit stays.
        let r = apply_lines(&client, LineAction::Discard, "f.txt", false, &[del(16, "line 15"), add(15, "line fifteen")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        let disk = std::fs::read_to_string(dir.join("f.txt")).unwrap();
        assert!(disk.contains("line 15\n") && !disk.contains("fifteen") && disk.contains("line two"), "{disk}");

        // No newline at the end: add a line after the last one and stage both changes (the
        // whole file, so the whole-file stage runs); then stage just a middle edit of tail.txt.
        std::fs::write(dir.join("tail.txt"), "a\nb\nc").unwrap();
        let r = apply_lines(&client, LineAction::Stage, "tail.txt", false, &[del(2, "b"), add(2, "b"), add(3, "c")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(index("tail.txt"), "a\nb\nc");
        std::fs::write(dir.join("tail.txt"), "A\nb\nc").unwrap();
        let r = apply_lines(&client, LineAction::Stage, "tail.txt", false, &[del(1, "a"), add(1, "A")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(index("tail.txt"), "A\nb\nc");
        // Adding the newline and a new last line, staging only the new line, cannot work alone.
        std::fs::write(dir.join("tail.txt"), "A\nb\nc\nd").unwrap();
        // `git apply` would take that patch and join "c" and "d" into one line.
        let r = apply_lines(&client, LineAction::Stage, "tail.txt", false, &[add(4, "d")]).await;
        assert!(matches!(&r, Err(GitError::InvalidPath { reason, .. }) if reason.contains("Select the neighbouring lines")), "{r:?}");
        assert_eq!(index("tail.txt"), "A\nb\nc");

        // CRLF lines keep their \r in the index.
        std::fs::write(dir.join("crlf.txt"), "one\r\nTWO\r\nthree\r\nfour\r\n").unwrap();
        let r = apply_lines(&client, LineAction::Stage, "crlf.txt", false, &[del(2, "two"), add(2, "TWO")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(index("crlf.txt"), "one\r\nTWO\r\nthree");

        // An untracked file: stage two of its three lines, then discard the third from disk.
        std::fs::write(dir.join("new.txt"), "x\ny\nz\n").unwrap();
        let r = apply_lines(&client, LineAction::Stage, "new.txt", true, &[add(1, "x"), add(3, "z")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(index("new.txt"), "x\nz");
        let r = apply_lines(&client, LineAction::Discard, "new.txt", false, &[add(2, "y")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(std::fs::read_to_string(dir.join("new.txt")).unwrap(), "x\nz\n");
        // Every line of a new untracked file selected: the whole-file stage runs.
        std::fs::write(dir.join("all.txt"), "1\n2\n").unwrap();
        assert!(apply_lines(&client, LineAction::Stage, "all.txt", true, &[add(1, "1"), add(2, "2")]).await.unwrap().ok);
        assert_eq!(out(&dir, &["status", "--porcelain", "--", "all.txt"]), "A  all.txt");
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

    #[tokio::test]
    async fn fetch_and_push_to_the_upstream_or_a_new_branch() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-push-{}", uuid::Uuid::new_v4().simple()));
        let (origin, local, other) = (root.join("origin.git"), root.join("local"), root.join("other"));
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&std::ffi::OsStr]| assert!(Command::new("git").args(args).status().unwrap().success());
        run(&["init".as_ref(), "-q".as_ref(), "--bare".as_ref(), "-b".as_ref(), "main".as_ref(), origin.as_os_str()]);
        for dir in [&local, &other] {
            run(&["clone".as_ref(), "-q".as_ref(), origin.as_os_str(), dir.as_os_str()]);
            for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
                git(dir, &["config", k, v]);
            }
        }
        let client = client_for(&local);
        let progress = |_: String| {};
        let cancel = tokio_util::sync::CancellationToken::new();
        let rev = |dir: &Path, r: &str| String::from_utf8(Command::new("git").arg("-C").arg(dir).args(["rev-parse", r]).output().unwrap().stdout).unwrap();

        // A first push to a new remote branch sets the upstream. (Cloning an empty repository
        // already makes main track origin/main, so that is undone first.)
        git(&local, &["config", "--remove-section", "branch.main"]);
        std::fs::write(local.join("a.txt"), "1\n").unwrap();
        git(&local, &["add", "-A"]); git(&local, &["commit", "-qm", "one"]);
        assert_eq!(upstream_of(&client, "main").await.unwrap(), None);
        let main = PushTarget { remote: "origin".into(), branch: "main".into() };
        let first = push(&client, "main", &main, true, &progress, &cancel).await.unwrap();
        assert!(first.ok, "{first:?}");
        assert_eq!(upstream_of(&client, "main").await.unwrap(), Some(main.clone()));
        assert_eq!(rev(&origin, "main"), rev(&local, "HEAD"));

        // Another clone pushes; fetch brings it into origin/main, and our push is then rejected.
        git(&other, &["pull", "-q", "origin", "main"]);
        std::fs::write(other.join("b.txt"), "2\n").unwrap();
        git(&other, &["add", "-A"]); git(&other, &["commit", "-qm", "two"]); git(&other, &["push", "-q", "origin", "main"]);
        let fetched = fetch_all(&client, &progress, &cancel).await.unwrap();
        assert!(fetched.ok, "{fetched:?}");
        assert_eq!(rev(&local, "origin/main"), rev(&other, "HEAD"));
        std::fs::write(local.join("c.txt"), "3\n").unwrap();
        git(&local, &["add", "-A"]); git(&local, &["commit", "-qm", "three"]);
        let rejected = push(&client, "main", &main, false, &progress, &cancel).await.unwrap();
        assert!(push_rejected(&rejected), "{rejected:?}");

        // A branch pushed under another name; a tag of the same name is not pushed instead.
        git(&local, &["tag", "topic"]);
        git(&local, &["branch", "topic", "HEAD~1"]);
        let topic = PushTarget { remote: "origin".into(), branch: "remote-topic".into() };
        assert!(push(&client, "topic", &topic, true, &progress, &cancel).await.unwrap().ok);
        assert_eq!(rev(&origin, "refs/heads/remote-topic"), rev(&local, "refs/heads/topic"));
        assert_eq!(upstream_of(&client, "topic").await.unwrap(), Some(topic));
        assert!(Command::new("git").arg("-C").arg(&origin).args(["rev-parse", "--verify", "-q", "refs/tags/topic"]).status().unwrap().code() == Some(1));

        cancel.cancel();
        assert!(matches!(fetch_all(&client, &progress, &cancel).await, Err(GitError::Cancelled)));
        assert!(matches!(push(&client, "main", &PushTarget { remote: "-o".into(), branch: "main".into() }, false, &progress, &cancel).await, Err(GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn set_and_unset_upstream_then_delete_a_remote_branch() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-upstream-{}", uuid::Uuid::new_v4().simple()));
        let (origin, local) = (root.join("origin.git"), root.join("local"));
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&std::ffi::OsStr]| assert!(Command::new("git").args(args).status().unwrap().success());
        run(&["init".as_ref(), "-q".as_ref(), "--bare".as_ref(), "-b".as_ref(), "main".as_ref(), origin.as_os_str()]);
        run(&["clone".as_ref(), "-q".as_ref(), origin.as_os_str(), local.as_os_str()]);
        git(&local, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&local, &["push", "-q", "origin", "main", "main:refs/heads/feature/x"]);
        git(&local, &["fetch", "-q"]);
        git(&local, &["branch", "topic"]);
        // A local branch named like the remote branch, which the full name must not pick.
        git(&local, &["branch", "origin/feature/x"]);
        let client = client_for(&local);
        let upstream = |branch: &str| out(&local, &["rev-parse", "--symbolic-full-name", &format!("{branch}@{{upstream}}")]);

        // Track a remote branch, then another, then none.
        let set = branch_set_upstream(&client, "topic", Some("refs/remotes/origin/feature/x")).await.unwrap();
        assert!(set.ok, "{set:?}");
        assert_eq!(upstream("topic"), "refs/remotes/origin/feature/x");
        assert!(branch_set_upstream(&client, "topic", Some("refs/remotes/origin/main")).await.unwrap().ok);
        assert_eq!(out(&local, &["rev-parse", "--abbrev-ref", "topic@{upstream}"]), "origin/main");
        assert_eq!(upstream_of(&client, "topic").await.unwrap(), Some(PushTarget { remote: "origin".into(), branch: "main".into() }));
        assert!(branch_set_upstream(&client, "topic", None).await.unwrap().ok);
        assert_eq!(upstream_of(&client, "topic").await.unwrap(), None);
        // Unsetting again fails in Git; a missing remote branch, a local branch and option-like
        // names are refused.
        assert!(!branch_set_upstream(&client, "topic", None).await.unwrap().ok);
        let missing = branch_set_upstream(&client, "topic", Some("refs/remotes/origin/nope")).await.unwrap();
        assert!(!missing.ok && missing.stderr.contains("does not exist"), "{missing:?}");
        assert!(matches!(branch_set_upstream(&client, "topic", Some("refs/heads/main")).await, Err(GitError::InvalidPath { .. })));
        assert!(matches!(branch_set_upstream(&client, "-d", Some("refs/remotes/origin/main")).await, Err(GitError::InvalidPath { .. })));

        // Delete feature/x on the remote: it goes there and from the remote-tracking branches here.
        let progress = |_: String| {};
        let cancel = tokio_util::sync::CancellationToken::new();
        let ls_remote = || out(&local, &["ls-remote", "--heads", "origin"]);
        assert!(ls_remote().contains("refs/heads/feature/x"));
        let deleted = delete_remote_branch(&client, "origin", "feature/x", &progress, &cancel).await.unwrap();
        assert!(deleted.ok, "{deleted:?}");
        assert!(!ls_remote().contains("refs/heads/feature/x"), "{}", ls_remote());
        assert!(ls_remote().contains("refs/heads/main"));
        assert!(Command::new("git").arg("-C").arg(&local).args(["rev-parse", "--verify", "-q", "refs/remotes/origin/feature/x"]).status().unwrap().code() == Some(1));
        // The local branch of the same name is left alone.
        assert!(out(&local, &["branch", "--list", "origin/feature/x"]).contains("origin/feature/x"));

        // A branch the remote no longer has: Git succeeds, the remote warns, and the stale
        // remote-tracking branch here is removed.
        git(&local, &["update-ref", "refs/remotes/origin/stale", "HEAD"]);
        let stale = delete_remote_branch(&client, "origin", "stale", &progress, &cancel).await.unwrap();
        assert!(stale.ok && stale.stderr.contains("non-existent ref"), "{stale:?}");
        assert!(Command::new("git").arg("-C").arg(&local).args(["rev-parse", "--verify", "-q", "refs/remotes/origin/stale"]).status().unwrap().code() == Some(1));

        // The remote refuses to delete the branch its HEAD points at.
        let refused = delete_remote_branch(&client, "origin", "main", &progress, &cancel).await.unwrap();
        assert!(!refused.ok && refused.stderr.contains("[remote rejected]"), "{refused:?}");
        assert!(ls_remote().contains("refs/heads/main"));

        assert!(matches!(delete_remote_branch(&client, "-o", "main", &progress, &cancel).await, Err(GitError::InvalidPath { .. })));
        assert!(matches!(delete_remote_branch(&client, "origin", "--all", &progress, &cancel).await, Err(GitError::InvalidPath { .. })));
        cancel.cancel();
        assert!(matches!(delete_remote_branch(&client, "origin", "main", &progress, &cancel).await, Err(GitError::Cancelled)));
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

    #[test]
    fn upstream_comes_from_branch_config() {
        let cfg = b"branch.Dev.remote\norigin\0branch.Dev.merge\nrefs/heads/feature/dev\0";
        assert_eq!(upstream_from_config(cfg, "Dev"), Some(PushTarget { remote: "origin".into(), branch: "feature/dev".into() }));
        assert_eq!(upstream_from_config(b"branch.dev.remote\norigin\0", "dev"), None);
        // Tracking another local branch is not an upstream to push to.
        assert_eq!(upstream_from_config(b"branch.dev.remote\n.\0branch.dev.merge\nrefs/heads/main\0", "dev"), None);
    }
}
