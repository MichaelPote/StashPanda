//! Write operations. Every one runs as `Category::UserAction`, expects the
//! caller to hold the repository's exclusive lock, and reports Git's own
//! output so the UI can show exactly what happened.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{classify_git_failure, GitError, Result};
use crate::git::queries::in_progress::InProgressKind;
use crate::git::queries::diff::join_repo_path;
use crate::git::{GitClient, GitReq};
use crate::transport::paths::NativePath;
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

/// A refusal decided before running Git: `ok: false` with `reason` as stderr.
fn refusal(reason: String) -> OpResult {
    OpResult { ok: false, exit_code: None, stdout: String::new(), stderr: reason, conflicts: false }
}

fn result(out: crate::transport::Output) -> OpResult {
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let conflicts = !out.success() && (stdout.contains("CONFLICT") || stderr.contains("CONFLICT") || stdout.contains("Automatic merge failed"));
    OpResult { ok: out.success(), exit_code: out.exit_code, stdout, stderr, conflicts }
}

/// `IndexLocked` or `ConfigLocked` when Git failed because `index.lock` or `config.lock` is
/// there (another Git process is running, or one died and left the file behind).
pub fn lock_in(res: &OpResult) -> Option<GitError> {
    if res.ok {
        return None;
    }
    match classify_git_failure("op", vec![], res.exit_code, res.stderr.as_bytes(), &TransportKind::Local) {
        e @ (GitError::IndexLocked { .. } | GitError::ConfigLocked { .. }) => Some(e),
        _ => None,
    }
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

/// A remote to fetch from or push to: a remote's name, or a URL or path, which `git push -u <url>`
/// writes into `branch.<name>.remote` and Git then pushes to and pulls from. Only guards against
/// option injection and control characters.
fn valid_remote(remote: &str) -> Result<()> {
    if remote.is_empty() || remote.starts_with('-') || remote.chars().any(|c| c.is_control()) {
        return Err(GitError::InvalidPath { path: remote.to_string(), reason: "not a valid Git remote".into() });
    }
    Ok(())
}

/// Check out a branch name or a commit (detached when `target` is not a branch). The `--` after
/// it makes Git read `target` only as a revision: without it, a name that is no longer a branch
/// but is a file restores that file from the index and loses its changes.
pub async fn checkout(client: &GitClient, target: &str) -> Result<OpResult> {
    valid_revision(target)?;
    let out = client.exec_allow_failure(GitReq::new("checkout", ["checkout", target, "--"]).user()).await?;
    Ok(result(out))
}

/// Create branch `name` at `start_point`, and check it out when `checkout`. A start point of
/// `HEAD` is left out, as `git checkout -b <name>` does it, so that on a branch with no commits
/// yet checking out a new branch renames the unborn branch instead of failing. From a
/// remote-tracking branch (`refs/remotes/origin/dev`) it is created with `--no-track`: Git would
/// otherwise write the upstream into `.git/config` in the same command, and when that file is
/// locked it creates the branch, fails, and leaves it neither checked out nor tracking. Set the
/// upstream afterwards with `branch_set_upstream`, which can be retried on its own.
pub async fn branch_create(client: &GitClient, name: &str, start_point: &str, checkout: bool) -> Result<OpResult> {
    valid_ref_name(name)?;
    valid_revision(start_point)?;
    let start = (start_point != "HEAD").then_some(start_point);
    let no_track = start_point.starts_with("refs/remotes/").then_some("--no-track");
    let args: Vec<&str> = if checkout {
        ["checkout", "-b", name].into_iter().chain(no_track).chain(start).collect()
    } else {
        ["branch"].into_iter().chain(no_track).chain([name]).chain(start).collect()
    };
    let out = client.exec_allow_failure(GitReq::new("branch", args).user()).await?;
    Ok(result(out))
}

/// Merge `target` (a branch name or commit) into the current branch. `autostash` stashes
/// uncommitted changes first and puts them back once the merge is over (`--autostash`): straight
/// away when it completes, and when it stops on conflicts, once it is concluded with a commit or
/// aborted. Git keeps them as `stash@{0}` if putting them back conflicts.
pub async fn merge(client: &GitClient, target: &str, no_ff: bool, autostash: bool) -> Result<OpResult> {
    valid_revision(target)?;
    let mut args = vec!["merge", "--no-edit"];
    if no_ff {
        args.push("--no-ff");
    }
    if autostash {
        args.push("--autostash");
    }
    args.push(target);
    // A merge commit runs the `pre-merge-commit` and `commit-msg` hooks, as a commit does.
    let out = client.exec_allow_failure(GitReq::new("merge", args).user().timeout(Some(COMMIT_TIMEOUT))).await?;
    Ok(result(out))
}

/// Put the branch back where it was before the merge. Rewrites every file the merge changed,
/// through any filter such as Git LFS, so it gets the commit timeout too.
pub async fn merge_abort(client: &GitClient) -> Result<OpResult> {
    let out = client.exec_allow_failure(GitReq::new("merge", ["merge", "--abort"]).user().timeout(Some(COMMIT_TIMEOUT))).await?;
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

/// The first half of a pull: `git fetch --progress <remote> refs/heads/<branch>`, which writes
/// `FETCH_HEAD` and updates `refs/remotes/<remote>/<branch>`. The full name, as `git pull` takes
/// it from `branch.<name>.merge`: a bare `dev` would fetch the remote's tag `dev` instead when it
/// has one, since Git looks for tags before branches. `--progress` makes Git report progress even
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
    valid_remote(remote)?;
    valid_ref_name(branch)?;
    let refname = format!("refs/heads/{branch}");
    let req = GitReq::new("fetch", ["fetch", "--progress", remote, refname.as_str()]).user().timeout(None).own_session();
    let out = client.exec_streaming(req, progress, cancel).await?;
    Ok(result(out))
}

/// The second half of a pull: bring `FETCH_HEAD` into the current branch the way `git pull`
/// would, checked against it with Git 2.43.
///
/// - Merge: `git merge --no-edit FETCH_HEAD`, which writes the same "Merge branch 'dev' of
///   <url>" message, with `--no-ff` when `pull.ff` is false, as `git pull --no-rebase` does.
///   `pull.ff=only` does not override the choice, as it does not override `--no-rebase`.
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
    valid_remote(remote)?;
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
        if rebase_merges_configured(client).await? {
            args.push("--rebase-merges");
        }
        args.extend(autostash_flag);
        args.extend(["--onto", "FETCH_HEAD", upstream]);
        let out = client.exec_allow_failure(GitReq::new("rebase", args).user().timeout(timeout)).await?;
        return Ok(result(out));
    }
    let mut args = vec!["merge", "--no-edit"];
    if mode != PullMode::Merge {
        args.push("--ff-only");
    } else if pull_ff_is_false(client).await? {
        args.push("--no-ff");
    }
    args.extend(autostash_flag);
    args.push("FETCH_HEAD");
    let out = client.exec_allow_failure(GitReq::new("merge", args).user().timeout(timeout)).await?;
    Ok(result(out))
}

/// Fail with `HeadMoved` unless local branch `branch` is checked out. A pull merges or rebases
/// into whatever is checked out, so a pull or sync asked for on `branch` checks this, under the
/// write gate, before it fetches and again before it merges: a checkout in a terminal, or one in
/// StashPanda that ran first, would otherwise send the commits into another branch.
pub async fn require_checked_out(client: &GitClient, branch: &str) -> Result<()> {
    valid_ref_name(branch)?;
    let head = client.exec_allow_failure(GitReq::new("symbolic-ref", ["symbolic-ref", "--quiet", "HEAD"])).await?;
    let current = head.success().then(|| String::from_utf8_lossy(&head.stdout).trim().to_string());
    let current = current.map(|r| r.strip_prefix("refs/heads/").map(str::to_string).unwrap_or(r));
    if current.as_deref() == Some(branch) {
        return Ok(());
    }
    Err(GitError::HeadMoved { expected: branch.to_string(), current })
}

/// `require_checked_out` for an operation that also runs on a detached HEAD (merge, rebase,
/// reset): with `Some(branch)`, that branch must be checked out; with `None`, HEAD must still be
/// detached, or the operation would move a branch the user was not looking at.
pub async fn require_head(client: &GitClient, expected: Option<&str>) -> Result<()> {
    match expected {
        Some(branch) => require_checked_out(client, branch).await,
        None => {
            let head = client.exec_allow_failure(GitReq::new("symbolic-ref", ["symbolic-ref", "--quiet", "--short", "HEAD"])).await?;
            if !head.success() {
                return Ok(());
            }
            let current = String::from_utf8_lossy(&head.stdout).trim().to_string();
            Err(GitError::HeadMoved { expected: "the detached HEAD".into(), current: Some(current) })
        }
    }
}

/// True when the current branch's `branch.<name>.rebase`, or failing that `pull.rebase`, is
/// `merges` (or `m`): `git pull` then rebases with `--rebase-merges`, which keeps the branch's own
/// merge commits instead of flattening them.
async fn rebase_merges_configured(client: &GitClient) -> Result<bool> {
    let head = client.exec_allow_failure(GitReq::new("symbolic-ref", ["symbolic-ref", "--quiet", "--short", "HEAD"])).await?;
    let branch = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let mut keys = Vec::new();
    if head.success() && !branch.is_empty() {
        keys.push(format!("branch.{branch}.rebase"));
    }
    keys.push("pull.rebase".to_string());
    for key in keys {
        let out = client.exec_allow_failure(GitReq::new("config", ["config".to_string(), "--get".into(), key])).await?;
        if out.success() {
            let value = String::from_utf8_lossy(&out.stdout).trim().to_ascii_lowercase();
            return Ok(matches!(value.as_str(), "merges" | "m"));
        }
    }
    Ok(false)
}

/// True when `pull.ff` is false (`false`, `no`, `off` or `0`), which makes `git pull` merge with
/// `--no-ff`: a merge commit even where a fast-forward would do.
async fn pull_ff_is_false(client: &GitClient) -> Result<bool> {
    let out = client.exec_allow_failure(GitReq::new("config", ["config", "--get", "pull.ff"])).await?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_ascii_lowercase();
    Ok(out.success() && matches!(value.as_str(), "false" | "no" | "off" | "0"))
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

/// `git fetch --all --prune --progress`: every remote, with the remotes' own settings. `--prune`
/// removes the remote-tracking branches of branches the remotes no longer have, so a branch
/// deleted on the server goes from the list here too, and a local branch that tracked it shows
/// its upstream as gone. Tags are left alone. Stopping it is safe, as for `pull_fetch`.
pub async fn fetch_all(client: &GitClient, progress: &(dyn Fn(String) + Send + Sync), cancel: &tokio_util::sync::CancellationToken) -> Result<OpResult> {
    let req = GitReq::new("fetch", ["fetch", "--all", "--prune", "--progress"]).user().timeout(None).own_session();
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

/// Where `branch` goes when it is pushed without choosing a target, as plain `git push` decides
/// it, or None when it has no upstream and nothing else says where it goes:
///
/// - The remote is `branch.<branch>.pushRemote`, else `remote.pushDefault`, else the upstream's
///   remote.
/// - When that remote has `remote.<remote>.push` refspecs, they decide the name there. One that
///   sends the branch somewhere other than a branch (`refs/for/main`, for a review server) is
///   refused with `PushTargetRefused` rather than pushed onto the branch itself, and so is a
///   branch none of them includes, which `git push` would not push.
/// - Otherwise, pushing to the remote the branch pulls from follows `push.default` as in
///   `push_target_for` (whose refusal is `InvalidPath`, which the frontend answers by asking where
///   to push). Pushing to another remote (a fork) pushes to the branch's own name for `simple` and
///   `current`, as Git does, and is refused with `PushTargetRefused` for the other values.
pub async fn implicit_push_target(client: &GitClient, branch: &str) -> Result<Option<PushTarget>> {
    valid_ref_name(branch)?;
    let upstream = upstream_of(client, branch).await?;
    let chosen_remote = match config_all(client, &format!("branch.{branch}.pushRemote")).await?.pop() {
        Some(r) => Some(r),
        None => config_all(client, "remote.pushDefault").await?.pop(),
    };
    let Some(remote) = chosen_remote.or_else(|| upstream.as_ref().map(|u| u.remote.clone())) else {
        return Ok(None);
    };
    valid_remote(&remote)?;
    let refspecs = config_all(client, &format!("remote.{remote}.push")).await?;
    if !refspecs.is_empty() {
        let head = client.exec_allow_failure(GitReq::new("symbolic-ref", ["symbolic-ref", "--quiet", "--short", "HEAD"])).await?;
        let checked_out = head.success().then(|| String::from_utf8_lossy(&head.stdout).trim().to_string());
        let refused = |reason: String| GitError::PushTargetRefused { branch: branch.to_string(), reason };
        return match push_ref_for(refspecs.iter().map(String::as_str), branch, checked_out.as_deref()) {
            Some(dst) => match dst.strip_prefix("refs/heads/") {
                Some(name) => Ok(Some(PushTarget { remote, branch: name.to_string() })),
                None => Err(refused(format!(
                    "the repository's remote.{remote}.push setting sends it to {dst}, which is not a branch, and StashPanda only pushes to branches"
                ))),
            },
            None => Err(refused(format!("the repository's remote.{remote}.push setting does not include it, so git push would not push it"))),
        };
    }
    // The remote the branch pulls from; Git takes origin when the branch tracks nothing.
    let pulls_from = upstream.as_ref().map_or("origin", |u| u.remote.as_str());
    if remote == pulls_from {
        return match upstream {
            Some(u) => push_target_for(client, branch, u).await.map(Some),
            None => Ok(None),
        };
    }
    let mode = push_default(client).await?.unwrap_or_default();
    match mode.as_str() {
        "" | "simple" | "current" => Ok(Some(PushTarget { remote, branch: branch.to_string() })),
        _ => Err(GitError::PushTargetRefused {
            branch: branch.to_string(),
            reason: format!("it is pushed to {remote}, which it does not pull from, and Git's push.default ({mode}) does not choose a branch there"),
        }),
    }
}

/// Every value of config `key`, in the order Git reads them (the last one wins for a single-valued
/// key); empty when it is not set.
async fn config_all(client: &GitClient, key: &str) -> Result<Vec<String>> {
    let out = client.exec_allow_failure(GitReq::new("config", ["config".to_string(), "--get-all".into(), key.to_string()])).await?;
    // Exit 1, with nothing printed, when it is not set.
    if !out.success() && out.exit_code != Some(1) {
        return Err(classify_git_failure("config", vec![], out.exit_code, &out.stderr, &client.transport.kind()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

/// The full ref name a push with `refspecs` (a remote's `remote.<name>.push` values) sends local
/// branch `branch` to, or None when none of them includes it. The first refspec that matches wins.
/// Short names (`main:review/main`) are branch names, `HEAD` is `checked_out`, and a refspec with
/// no `:` keeps the name. Force (`+`) is ignored, since StashPanda never forces; negative (`^…`)
/// and deleting (`:dst`) refspecs never match.
fn push_ref_for<'a>(refspecs: impl IntoIterator<Item = &'a str>, branch: &str, checked_out: Option<&str>) -> Option<String> {
    let full = format!("refs/heads/{branch}");
    let qualify = |name: &str| if name.starts_with("refs/") { name.to_string() } else { format!("refs/heads/{name}") };
    refspecs.into_iter().find_map(|spec| {
        let spec = spec.trim().trim_start_matches('+');
        if spec.starts_with('^') {
            return None;
        }
        let (src, dst) = match spec.split_once(':') {
            Some((s, d)) => (s, Some(d)),
            None => (spec, None),
        };
        // `:` alone pushes every branch to the branch of the same name.
        if src.is_empty() {
            return (dst == Some("")).then(|| full.clone());
        }
        let dst = dst.filter(|d| !d.is_empty());
        let src = match src {
            "HEAD" | "@" => qualify(checked_out?),
            s => qualify(s),
        };
        match src.split_once('*') {
            Some((pre, post)) => {
                let middle = full.strip_prefix(pre)?.strip_suffix(post)?;
                Some(dst.map_or_else(|| full.clone(), |d| qualify(&d.replacen('*', middle, 1))))
            }
            None => (src == full).then(|| dst.map_or_else(|| full.clone(), qualify)),
        }
    })
}

/// Where `branch`, which tracks `upstream`, goes when it is pushed without choosing a target. The
/// upstream itself when it has the same name. Otherwise `push.default` decides, as for `git
/// push`: `upstream` (or its old name `tracking`) pushes to the upstream, `current` to the
/// branch's own name on the upstream's remote. `simple`, the default, refuses, and so does
/// `matching`: fails with `InvalidPath`, so that a branch started from `origin/main` is never
/// pushed onto `main` without the user choosing that. `nothing` refuses every push without a
/// target, whatever the names, with `PushTargetRefused`, as `git push` does.
pub async fn push_target_for(client: &GitClient, branch: &str, upstream: PushTarget) -> Result<PushTarget> {
    let mode = push_default(client).await?.unwrap_or_default();
    if mode == "nothing" {
        return Err(GitError::PushTargetRefused {
            branch: branch.to_string(),
            reason: "Git's push.default is nothing, so git push pushes nothing unless it is told where".into(),
        });
    }
    if upstream.branch == branch {
        return Ok(upstream);
    }
    match mode.as_str() {
        "upstream" | "tracking" => Ok(upstream),
        "current" => Ok(PushTarget { remote: upstream.remote, branch: branch.to_string() }),
        _ => Err(GitError::InvalidPath {
            path: branch.to_string(),
            reason: format!(
                "it tracks {}/{}, a branch with another name, and Git's push.default ({}) does not push to a branch with another name; choose where to push it in the Push dialog",
                upstream.remote,
                upstream.branch,
                if mode.is_empty() { "simple" } else { mode.as_str() }
            ),
        }),
    }
}

/// `push.default` as Git reads it for this repository, from whichever config file sets it; None
/// when none does, which Git treats as `simple`.
pub async fn push_default(client: &GitClient) -> Result<Option<String>> {
    // Exit 1, with nothing printed, when it is not set.
    let out = client.exec_allow_failure(GitReq::new("config", ["config", "--get", "push.default"])).await?;
    if !out.success() && out.exit_code != Some(1) {
        return Err(classify_git_failure("config", vec![], out.exit_code, &out.stderr, &client.transport.kind()));
    }
    let mode = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!mode.is_empty()).then_some(mode))
}

/// The `push.default` values that push a branch to a branch with another name: `upstream` to the
/// branch it tracks, `current` to its own name on that branch's remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PushDefault {
    Upstream,
    Current,
}

/// Set `push.default` to `mode` in this repository's own config (`git config --local`): it then
/// applies to every push from this repository, `git push` in a terminal included, and to no other
/// repository.
pub async fn set_push_default(client: &GitClient, mode: PushDefault) -> Result<OpResult> {
    let value = match mode {
        PushDefault::Upstream => "upstream",
        PushDefault::Current => "current",
    };
    let out = client.exec_allow_failure(GitReq::new("config", ["config", "--local", "push.default", value]).user()).await?;
    Ok(result(out))
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

/// `git push --progress <remote> refs/heads/<branch>:refs/heads/<target branch>`. Full ref names
/// on both sides, so a tag with the same name as the branch cannot be pushed by mistake. It never
/// sets the upstream: `git push --set-upstream` writes it into `.git/config` after the push, and
/// when that file is locked it says so and still exits 0, so the branch would silently track
/// nothing. Set it afterwards with `branch_set_upstream`, as its own step. Never forces: a push the remote rejects
/// because it has commits the branch lacks fails with `[rejected]` in stderr (see
/// `push_rejected`). Stopping it is safe for the local repository; if the remote had already
/// taken the update, the next fetch shows it.
pub async fn push(
    client: &GitClient,
    branch: &str,
    target: &PushTarget,
    progress: &(dyn Fn(String) + Send + Sync),
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<OpResult> {
    valid_ref_name(branch)?;
    valid_remote(&target.remote)?;
    valid_ref_name(&target.branch)?;
    let refspec = format!("refs/heads/{branch}:refs/heads/{}", target.branch);
    let args = ["push", "--progress", target.remote.as_str(), refspec.as_str()];
    let out = client.exec_streaming(GitReq::new("push", args).user().timeout(None).own_session(), progress, cancel).await?;
    Ok(result(out))
}

/// The remote-tracking branch here (`refs/remotes/origin/dev`) that holds `target` as last
/// fetched, from the remote's `remote.<remote>.fetch` refspecs; None when none maps it (a remote
/// given as a URL has none) or this repository has never fetched it.
pub async fn tracking_ref_of(client: &GitClient, target: &PushTarget) -> Result<Option<String>> {
    valid_remote(&target.remote)?;
    valid_ref_name(&target.branch)?;
    let refspecs = config_all(client, &format!("remote.{}.fetch", target.remote)).await?;
    let Some(tracking) = tracking_ref_for(refspecs.iter().map(String::as_str), &format!("refs/heads/{}", target.branch)) else {
        return Ok(None);
    };
    let exists = client.exec_allow_failure(GitReq::new("rev-parse", ["rev-parse", "--verify", "-q", tracking.as_str()])).await?;
    Ok(exists.success().then_some(tracking))
}

/// The forward direction of `remote_ref_for`: where a fetch with `refspecs` stores the remote's
/// `remote_ref`, or None when none of them fetches it.
fn tracking_ref_for<'a>(refspecs: impl IntoIterator<Item = &'a str>, remote_ref: &str) -> Option<String> {
    refspecs.into_iter().find_map(|spec| {
        let spec = spec.trim().trim_start_matches('+');
        let (src, dst) = spec.split_once(':').filter(|_| !spec.starts_with('^'))?;
        match src.split_once('*') {
            Some((pre, post)) => {
                let middle = remote_ref.strip_prefix(pre)?.strip_suffix(post)?;
                dst.contains('*').then(|| dst.replacen('*', middle, 1))
            }
            None => (src == remote_ref && !dst.is_empty()).then(|| dst.to_string()),
        }
    })
}

/// True when `ancestor` is `descendant` or one of its ancestors.
async fn is_ancestor(client: &GitClient, ancestor: &str, descendant: &str) -> Result<bool> {
    let out = client.exec_allow_failure(GitReq::new("merge-base", ["merge-base", "--is-ancestor", ancestor, descendant])).await?;
    Ok(out.exit_code == Some(0))
}

/// True when HEAD is already on `target` as last fetched: amending it and pushing the amended
/// commit there would then need a force push.
pub async fn head_on_push_target(client: &GitClient, target: &PushTarget) -> Result<bool> {
    match tracking_ref_of(client, target).await? {
        Some(tracking) => is_ancestor(client, "HEAD", &tracking).await,
        None => Ok(false),
    }
}

/// True when local `branch` lacks commits that `target` has (as last fetched) because they were
/// rewritten here, by an amend or a rebase, rather than because someone else pushed: an earlier
/// position of the branch, from its reflog, contained the target's tip. Pushing would then replace
/// those commits on the target, which needs a force push; pulling the target would bring the old
/// copies back beside the new ones. False when the branch has no reflog, or contains the tip.
pub async fn rewrote_pushed_commits(client: &GitClient, branch: &str, target: &PushTarget) -> Result<bool> {
    valid_ref_name(branch)?;
    let Some(tracking) = tracking_ref_of(client, target).await? else { return Ok(false) };
    let local = format!("refs/heads/{branch}");
    if is_ancestor(client, &tracking, &local).await? {
        return Ok(false);
    }
    let reflog = client
        .exec_allow_failure(GitReq::new("reflog", ["reflog".to_string(), "show".into(), "--format=%H".into(), "-n".into(), "100".into(), local]))
        .await?;
    let earlier: Vec<String> = String::from_utf8_lossy(&reflog.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if !reflog.success() || earlier.is_empty() {
        return Ok(false);
    }
    // Empty when every commit on the target is reachable from some earlier position of the branch.
    let mut args = vec!["rev-list".to_string(), "-n".into(), "1".into(), tracking, "--not".into()];
    args.extend(earlier);
    let missing = client.exec(GitReq::new("rev-list", args)).await?;
    Ok(String::from_utf8_lossy(&missing.stdout).trim().is_empty())
}

/// How many commits HEAD has that `tracking` (`FETCH_HEAD` after a pull, or
/// `refs/remotes/origin/dev`) does not: what a push would send.
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
    /// Where the push goes: `target`, or the branch's own name on that remote when the names
    /// differ and `push.default` is `current` (see `push_target_for`).
    pub push_target: PushTarget,
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
    /// The push was refused because the rebase rewrote commits `push_target` already had, not
    /// because someone pushed (see `rewrote_pushed_commits`): syncing again cannot fix it.
    pub rewrote_pushed: bool,
}

/// True when the remote refused a push because it has commits the pushed branch does not:
/// `! [rejected] dev -> dev (fetch first)` or `(non-fast-forward)`.
pub fn push_rejected(res: &OpResult) -> bool {
    !res.ok && res.stderr.contains("[rejected]")
}

/// The pull mode the repository's Git config asks for, as `git pull` reads it: `pull.ff=only`
/// first, then `branch.<branch>.rebase`, then `pull.rebase`, then any other `pull.ff`. None when
/// none of them is set.
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
    // `merges`, `interactive` and their one-letter forms all rebase. `pull_integrate` keeps merge
    // commits for `merges` (see `rebase_merges_configured`); `interactive` runs a plain rebase.
    let rebases = |v: &str| matches!(v, "true" | "yes" | "on" | "1" | "merges" | "m" | "interactive" | "i");
    // `pull.ff=only` wins over a rebase setting that also comes from config: `git pull` then
    // refuses anything but a fast-forward.
    if pull_ff.as_deref() == Some("only") {
        return Some(PullMode::FastForwardOnly);
    }
    match branch_rebase.or(pull_rebase) {
        Some(v) if rebases(&v) => Some(PullMode::Rebase),
        Some(_) => Some(PullMode::Merge),
        None if pull_ff.is_some() => Some(PullMode::Merge),
        None => None,
    }
}

/// Lightweight tag, or annotated when `message` is given. `--cleanup=whitespace` keeps lines
/// that start with `#` (`#123 fixed`): `git tag` otherwise strips them as comments even from `-m`.
pub async fn tag_create(client: &GitClient, name: &str, target: &str, message: Option<&str>, force: bool) -> Result<OpResult> {
    valid_ref_name(name)?;
    valid_revision(target)?;
    let mut args: Vec<String> = vec!["tag".into()];
    if force {
        args.push("-f".into());
    }
    if let Some(m) = message.filter(|m| !m.trim().is_empty()) {
        args.push("-a".into());
        args.push("--cleanup=whitespace".into());
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
    let listed = client
        .exec(GitReq::new("for-each-ref", ["for-each-ref".to_string(), "--format=%(refname)".into(), format!("refs/heads/{new}")]))
        .await?;
    let new_exists = String::from_utf8_lossy(&listed.stdout).lines().any(|l| l == format!("refs/heads/{new}"));
    let out = client.exec_allow_failure(GitReq::new("branch", ["branch", rename_flag(old, new, new_exists), old, new]).user()).await?;
    Ok(result(out))
}

/// `-M` for a rename that only changes letter case, unless a branch named exactly `new` exists:
/// where branch names are case-insensitive (Windows, macOS) Git finds `old` under `new` and
/// refuses `-m` with "a branch named 'New' already exists". Otherwise `-m`, so that an existing
/// branch is never overwritten.
fn rename_flag(old: &str, new: &str, new_exists: bool) -> &'static str {
    if old != new && old.to_lowercase() == new.to_lowercase() && !new_exists {
        "-M"
    } else {
        "-m"
    }
}

/// True when local branch `branch` is not checked out, is behind HEAD, and its tip lies on the
/// checked-out branch's own lane: the first-parent line from HEAD, the column the graph draws the
/// current branch in. That is a branch the current one has overtaken along its own history (a
/// release branch cut from `main`, say), which `branch_fast_forward` can move up to HEAD. A branch
/// that HEAD only contains through a merge's second parent is behind HEAD too, but not on its
/// lane, so it is not offered.
pub async fn fast_forwards_to_head(client: &GitClient, branch: &str) -> Result<bool> {
    valid_ref_name(branch)?;
    let head = client.exec_allow_failure(GitReq::new("symbolic-ref", ["symbolic-ref", "--quiet", "HEAD"])).await?;
    if !head.success() || String::from_utf8_lossy(&head.stdout).trim() == format!("refs/heads/{branch}") {
        return Ok(false);
    }
    let tip = client.exec_allow_failure(GitReq::new("rev-parse", ["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}^{{commit}}")])).await?;
    let tip = String::from_utf8_lossy(&tip.stdout).trim().to_string();
    if tip.is_empty() || !is_ancestor(client, &tip, "HEAD").await? {
        return Ok(false);
    }
    // The oldest commit on HEAD's first-parent line that the branch lacks has the branch's tip as
    // its first parent exactly when the tip is on that line. None when HEAD is the tip.
    let line = client.exec(GitReq::new("rev-list", ["rev-list", "--first-parent", "--reverse", &format!("{tip}..HEAD")])).await?;
    let Some(oldest) = String::from_utf8_lossy(&line.stdout).lines().next().map(str::to_string) else {
        return Ok(false);
    };
    let parent = client.exec_allow_failure(GitReq::new("rev-parse", ["rev-parse", "--verify", "-q", &format!("{oldest}^1")])).await?;
    Ok(String::from_utf8_lossy(&parent.stdout).trim() == tip)
}

/// A refusal with `ok: false`, sending nothing, when local branch `branch` no longer points at
/// commit `source` (the commit a force push's dropped list was made against); None when it does.
async fn branch_moved(client: &GitClient, branch: &str, source: &str) -> Result<Option<OpResult>> {
    valid_ref_name(branch)?;
    valid_revision(source)?;
    let now = client.exec_allow_failure(GitReq::new("rev-parse", ["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}^{{commit}}")])).await?;
    let now = String::from_utf8_lossy(&now.stdout).trim().to_string();
    let listed = client.exec_allow_failure(GitReq::new("rev-parse", ["rev-parse", "--verify", "-q", &format!("{source}^{{commit}}")])).await?;
    if now == String::from_utf8_lossy(&listed.stdout).trim() && !now.is_empty() {
        return Ok(None);
    }
    Ok(Some(OpResult {
        ok: false,
        exit_code: None,
        stdout: String::new(),
        stderr: format!("{branch} has moved since the list of dropped commits was made (source moved), so nothing was pushed."),
        conflicts: false,
    }))
}

/// Why local branch `branch` must not be moved from outside the worktree using it, or None when
/// nothing is using it. Git's own rule (`git branch -f` refuses the same branches): in this or any
/// linked worktree, HEAD is on it, a rebase is replaying it (`rebase-merge/head-name` or
/// `rebase-apply/head-name`, while HEAD itself is detached) or will move it when it finishes
/// (`rebase-merge/update-refs`, with `rebase.updateRefs`), or a bisect started from it
/// (`BISECT_START`). Moving it under a rebase leaves the rebase unable to finish.
pub async fn branch_in_use(client: &GitClient, branch: &str) -> Result<Option<String>> {
    valid_ref_name(branch)?;
    let refname = format!("refs/heads/{branch}");
    let common = client.exec(GitReq::new("rev-parse", ["rev-parse", "--path-format=absolute", "--git-common-dir"])).await?;
    let common = NativePath(String::from_utf8_lossy(&common.stdout).trim().to_string());
    let mut dirs = vec![common.clone()];
    if let Ok(Some(names)) = client.transport.list_dir(&join_repo_path(&common, "worktrees")).await {
        dirs.extend(names.iter().map(|n| join_repo_path(&common, &format!("worktrees/{n}"))));
    }
    for dir in &dirs {
        if read_trimmed(client, dir, "HEAD").await.as_deref() == Some(format!("ref: {refname}").as_str()) {
            return Ok(Some("it is checked out at a worktree".into()));
        }
        for name in ["rebase-merge/head-name", "rebase-apply/head-name"] {
            if read_trimmed(client, dir, name).await.as_deref() == Some(refname.as_str()) {
                return Ok(Some("a rebase of it is in progress".into()));
            }
        }
        // Three lines per branch: its name, then the commits it moves from and to.
        if let Some(listed) = read_trimmed(client, dir, "rebase-merge/update-refs").await {
            if listed.lines().step_by(3).any(|l| l.trim() == refname) {
                return Ok(Some("a rebase in progress will move it when it finishes".into()));
            }
        }
        if let Some(start) = read_trimmed(client, dir, "BISECT_START").await {
            if start == branch || start == refname {
                return Ok(Some("a bisect started from it is in progress".into()));
            }
        }
    }
    Ok(None)
}

/// A small file in a git directory, trimmed, or None when it is missing or cannot be read.
async fn read_trimmed(client: &GitClient, dir: &NativePath, name: &str) -> Option<String> {
    let bytes = client.transport.read_file(&join_repo_path(dir, name), 4096).await.ok()?;
    Some(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// Move local branch `target` to commit `source`, which local branch `branch` must still point at,
/// whether or not that is a fast-forward: the forced form of `push_to_local_branch`. `git update-ref
/// refs/heads/<target> <source> <expected>`, so the move happens only while `target` is still at
/// `expected`, the commit the dropped list was made from, and Git refuses otherwise. Refused with
/// `ok: false` (`refusing to move …` in stderr) when `branch` has moved, or when `target` is in
/// use (`branch_in_use`): `git update-ref` does not check that itself. Checks nothing out.
pub async fn force_move_local_branch(client: &GitClient, branch: &str, source: &str, target: &str, expected: &str) -> Result<OpResult> {
    valid_ref_name(target)?;
    valid_revision(expected)?;
    if branch == target {
        return Err(GitError::InvalidPath { path: target.to_string(), reason: "a branch cannot be pushed into itself".into() });
    }
    if let Some(moved) = branch_moved(client, branch, source).await? {
        return Ok(moved);
    }
    let refname = format!("refs/heads/{target}");
    if let Some(reason) = branch_in_use(client, target).await? {
        return Ok(refusal(format!("refusing to move {target}: {reason}")));
    }
    let message = format!("push to: forced update from {branch}");
    let out = client
        .exec_allow_failure(GitReq::new("update-ref", ["update-ref", "-m", message.as_str(), refname.as_str(), source, expected]).user())
        .await?;
    Ok(result(out))
}

/// Push local branch `source` into local branch `target` of this same repository: `git fetch .
/// refs/heads/<source>:refs/heads/<target>`, which moves `target` to `source`'s commit, or creates
/// it. Neither branch is checked out for it, so the working tree, the index and HEAD are not
/// touched; uncommitted changes are fine and no checkout hooks run. Git refuses a move that is
/// not a fast-forward (`! [rejected] … (non-fast-forward)`), and a `target` that this or another
/// worktree has checked out (`refusing to fetch into branch … checked out at …`), whose files
/// would then not match it. The forced form is `force_move_local_branch`.
pub async fn push_to_local_branch(client: &GitClient, source: &str, target: &str) -> Result<OpResult> {
    valid_ref_name(source)?;
    move_local_branch_to(client, &format!("refs/heads/{source}"), target).await
}

/// Fast-forward local branch `branch` to its upstream as last fetched (`origin/main`), without
/// fetching. The checked-out branch moves with `git merge --ff-only`, which also updates the files;
/// `autostash` stashes uncommitted changes first and puts them back afterwards (`--autostash`),
/// and when putting them back conflicts Git still exits 0 and keeps them as `stash@{0}`. Any other
/// branch moves with `git fetch . <upstream>:<branch>`, which touches no files. Git refuses either
/// move when it is not a fast-forward. Fails with `InvalidPath` when the branch tracks nothing
/// or its upstream has never been fetched.
pub async fn fast_forward_to_upstream(client: &GitClient, branch: &str, autostash: bool) -> Result<OpResult> {
    valid_ref_name(branch)?;
    let invalid = |reason: &str| GitError::InvalidPath { path: branch.to_string(), reason: reason.into() };
    let upstream = upstream_of(client, branch).await?.ok_or_else(|| invalid("it does not track a remote branch"))?;
    let tracking = tracking_ref_of(client, &upstream).await?.ok_or_else(|| invalid("its remote branch has not been fetched"))?;
    let head = client.exec_allow_failure(GitReq::new("symbolic-ref", ["symbolic-ref", "--quiet", "HEAD"])).await?;
    if head.success() && String::from_utf8_lossy(&head.stdout).trim() == format!("refs/heads/{branch}") {
        let mut args = vec!["merge", "--ff-only"];
        if autostash {
            args.push("--autostash");
        }
        args.push(tracking.as_str());
        // A fast-forward runs the `post-merge` hook.
        let out = client.exec_allow_failure(GitReq::new("merge", args).user().timeout(Some(COMMIT_TIMEOUT))).await?;
        return Ok(result(out));
    }
    move_local_branch_to(client, &tracking, branch).await
}

/// `git fetch --no-write-fetch-head . <source>:refs/heads/<target>` for a full ref name `source`
/// (`refs/heads/dev`, `refs/remotes/origin/dev`): moves local branch `target` there, or creates it,
/// when that is a fast-forward, after `branch_in_use` has said nothing is using it.
async fn move_local_branch_to(client: &GitClient, source: &str, target: &str) -> Result<OpResult> {
    valid_ref_name(source)?;
    valid_ref_name(target)?;
    if source == format!("refs/heads/{target}") {
        return Err(GitError::InvalidPath { path: target.to_string(), reason: "a branch cannot be pushed into itself".into() });
    }
    // Git 2.36, the oldest StashPanda runs, only refuses the branch HEAD is on.
    if let Some(reason) = branch_in_use(client, target).await? {
        return Ok(refusal(format!("refusing to move {target}: {reason}")));
    }
    let refspec = format!("{source}:refs/heads/{target}");
    let args = ["fetch", "--no-write-fetch-head", ".", refspec.as_str()];
    let out = client.exec_allow_failure(GitReq::new("fetch", args).user()).await?;
    Ok(result(out))
}

/// The commits a forced move of a branch from `tip` to local branch `branch` drops from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DroppedCommits {
    /// Newest first, at most the `limit` asked for.
    pub commits: Vec<DroppedCommit>,
    /// How many there are in all.
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DroppedCommit {
    pub oid: String,
    pub subject: String,
    pub author_name: String,
    /// Seconds since the Unix epoch.
    pub author_time: i64,
}

/// What a force push of commit `source` over a branch now at commit `tip` throws away: the commits
/// `tip` has that `source` does not. The Push To dialog lists them before a force push, for the
/// same `source` and `tip` the push then sends and expects (see `force_push` and
/// `force_move_local_branch`), so nothing is dropped that the user was not shown. Empty when it
/// would be a fast-forward.
pub async fn commits_dropped(client: &GitClient, source: &str, tip: &str, limit: u32) -> Result<DroppedCommits> {
    valid_revision(source)?;
    valid_revision(tip)?;
    let local = source.to_string();
    let count = client.exec(GitReq::new("rev-list", ["rev-list", "--count", tip, "--not", local.as_str(), "--"])).await?;
    let text = String::from_utf8_lossy(&count.stdout);
    let total: u32 = text.trim().parse().map_err(|_| GitError::ParseError { what: "rev-list --count".into(), detail: text.trim().to_string() })?;
    let limit = format!("-n{limit}");
    let listed = client
        .exec(GitReq::new("log", ["log", "--format=%H%x1f%s%x1f%an%x1f%at", limit.as_str(), tip, "--not", local.as_str(), "--"]))
        .await?;
    let commits = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            Some(DroppedCommit {
                oid: parts.next()?.to_string(),
                subject: parts.next()?.to_string(),
                author_name: parts.next()?.to_string(),
                author_time: parts.next()?.trim().parse().ok()?,
            })
        })
        .collect();
    Ok(DroppedCommits { commits, total })
}

/// Push commit `source`, which local branch `branch` must still point at, to `target`, forced:
/// `git push <remote> <source>:refs/heads/<target branch>` with
/// `--force-with-lease=refs/heads/<target branch>:<expected>`. Both ends are pinned to the commits
/// the dropped list was made from (`commits_dropped`): the remote's branch is replaced only if it
/// is still at `expected`, the commit the user was shown for it, and what replaces it is `source`,
/// not wherever `branch` has moved since. A branch that has moved is refused with `ok: false`
/// before anything is sent.
/// That is not the remote-tracking branch as it is now: an automatic fetch can move that in the
/// background, after which a lease taken from it would let the push throw away commits the user
/// never saw. When the remote's branch is anywhere else, Git refuses with `[rejected] … (stale
/// info)` (see `push_stale`) and its commits stay. An empty `expected` only lets the push through
/// when the remote has no such branch.
#[allow(clippy::too_many_arguments)]
pub async fn force_push(
    client: &GitClient,
    branch: &str,
    source: &str,
    target: &PushTarget,
    expected: &str,
    progress: &(dyn Fn(String) + Send + Sync),
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<OpResult> {
    valid_ref_name(branch)?;
    valid_remote(&target.remote)?;
    valid_ref_name(&target.branch)?;
    if !expected.is_empty() {
        valid_revision(expected)?;
    }
    if let Some(moved) = branch_moved(client, branch, source).await? {
        return Ok(moved);
    }
    let lease = format!("--force-with-lease=refs/heads/{}:{expected}", target.branch);
    let refspec = format!("{source}:refs/heads/{}", target.branch);
    let args = ["push", "--progress", lease.as_str(), target.remote.as_str(), refspec.as_str()];
    let out = client.exec_streaming(GitReq::new("push", args).user().timeout(None).own_session(), progress, cancel).await?;
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

/// Delete on `remote` the branch whose remote-tracking branch here is `<remote>/<branch>`: `git
/// push --progress <remote> --delete refs/heads/<branch>`. With a fetch refspec other than the
/// usual `refs/heads/*:refs/remotes/<remote>/*`, the branch's name there is found by reading the
/// remote's `remote.<remote>.fetch` refspecs backwards; one that no refspec maps is refused. The
/// full ref name means a tag of the same name on the remote cannot be deleted instead, and it also
/// makes Git remove the remote-tracking branch `<remote>/<branch>` here. When the remote no longer
/// has the branch, Git 2.43 still exits 0 and the remote prints "warning: deleting a non-existent
/// ref" in stderr; the frontend looks for that. A remote can refuse the deletion (a protected
/// branch, the branch its HEAD points at, no permission), which comes back as `[remote rejected]`
/// in stderr. Stopping it is safe for the local repository; if the remote had already deleted the
/// branch, the next fetch shows it.
///
/// The deletion carries `--force-with-lease=<ref>:<oid>`, with the commit `<remote>/<branch>` has
/// here, so a branch someone has pushed to since the last fetch is not deleted: Git refuses with
/// `[rejected] … (stale info)` (see `push_stale`), and commits this repository has never seen stay
/// on the remote. That lease also refuses when the remote has no such branch any more, so a
/// refusal is tried once more with an empty lease, which only lets the deletion through when the
/// branch does not exist there; that keeps the "deleting a non-existent ref" answer above.
pub async fn delete_remote_branch(
    client: &GitClient,
    remote: &str,
    branch: &str,
    progress: &(dyn Fn(String) + Send + Sync),
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<OpResult> {
    valid_ref_name(remote)?;
    valid_ref_name(branch)?;
    let tracking = format!("refs/remotes/{remote}/{branch}");
    let config = client
        .exec_allow_failure(GitReq::new("config", ["config".to_string(), "--get-all".into(), format!("remote.{remote}.fetch")]))
        .await?;
    let refspecs = String::from_utf8_lossy(&config.stdout).into_owned();
    let Some(refname) = remote_ref_for(refspecs.lines(), &tracking) else {
        return Err(GitError::InvalidPath { path: tracking, reason: format!("no fetch refspec of {remote} maps a remote branch to it") });
    };
    let seen = client.exec_allow_failure(GitReq::new("rev-parse", ["rev-parse", "--verify", "-q", &format!("{tracking}^{{commit}}")])).await?;
    let seen = String::from_utf8_lossy(&seen.stdout).trim().to_string();
    if seen.is_empty() {
        return Err(GitError::InvalidPath { path: tracking, reason: "this repository has no such remote branch; fetch first".into() });
    }
    let delete = |expected: &str| {
        let lease = format!("--force-with-lease={refname}:{expected}");
        GitReq::new("push", ["push".to_string(), "--progress".into(), lease, remote.into(), "--delete".into(), refname.clone()])
            .user()
            .timeout(None)
            .own_session()
    };
    let first = result(client.exec_streaming(delete(&seen), progress, cancel).await?);
    if !push_stale(&first) {
        return Ok(first);
    }
    let gone = result(client.exec_streaming(delete(""), progress, cancel).await?);
    Ok(if gone.ok { gone } else { first })
}

/// True when a push with `--force-with-lease` was refused because the remote's branch is not
/// where the lease expected: `! [rejected] dev (stale info)`.
pub fn push_stale(res: &OpResult) -> bool {
    !res.ok && res.stderr.contains("(stale info)")
}

/// The ref on the remote that a fetch with `refspecs` (`+refs/heads/*:refs/remotes/origin/*`)
/// stores as `tracking` here, or None when none of them does. Negative refspecs (`^…`) are skipped.
fn remote_ref_for<'a>(refspecs: impl IntoIterator<Item = &'a str>, tracking: &str) -> Option<String> {
    refspecs.into_iter().find_map(|spec| {
        let spec = spec.trim().trim_start_matches('+');
        let (src, dst) = spec.split_once(':').filter(|_| !spec.starts_with('^'))?;
        match dst.split_once('*') {
            Some((pre, post)) => {
                let middle = tracking.strip_prefix(pre)?.strip_suffix(post)?;
                src.contains('*').then(|| src.replacen('*', middle, 1))
            }
            None => (dst == tracking).then(|| src.to_string()),
        }
    })
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
/// `autostash` stashes uncommitted changes first and puts them back once the rebase is over
/// (`--autostash`): when it finishes, at once or after Continue, or when it is aborted. Git keeps
/// them as `stash@{0}` if putting them back conflicts.
pub async fn rebase(client: &GitClient, onto: &str, autostash: bool) -> Result<OpResult> {
    valid_revision(onto)?;
    let mut args = vec!["rebase"];
    if autostash {
        args.push("--autostash");
    }
    args.push(onto);
    let out = client.exec_allow_failure(GitReq::new("rebase", args).user().timeout(Some(COMMIT_TIMEOUT))).await?;
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
    Ok(parse_unmerged(&unmerged_listing(client).await?))
}

/// `git ls-files -u -z`: every unmerged entry in the index, with its mode and object ID.
async fn unmerged_listing(client: &GitClient) -> Result<Vec<u8>> {
    Ok(client.exec(GitReq::new("ls-files", ["ls-files", "-u", "-z"])).await?.stdout)
}

/// Resolve conflicted `paths` by keeping one side's version whole, and mark them resolved.
/// Where that side has the file, `git checkout --ours|--theirs` writes it and `git add` stages
/// it; where that side deleted it (deleted by us or by them), `git rm` deletes it. A submodule
/// gets that side's commit in the index (`git update-index --index-info`), as `git add` would
/// after checking the commit out in it; its own checkout is left alone. Refuses, and changes
/// nothing, when a path is not conflicted any more.
pub async fn resolve_conflicts(client: &GitClient, paths: &[String], side: ResolveSide) -> Result<OpResult> {
    use crate::git::queries::conflict_file::stage_records;
    valid_paths(paths)?;
    let listing = unmerged_listing(client).await?;
    let stages = parse_unmerged(&listing);
    let wanted = match side {
        ResolveSide::Ours => 2,
        ResolveSide::Theirs => 3,
    };
    let (mut keep, mut delete, mut gitlinks) = (Vec::new(), Vec::new(), Vec::new());
    for p in paths {
        // `git checkout --ours` leaves a submodule as it is, so `git add` would stage whatever
        // commit it happens to have checked out.
        if let Some((_, mode, oid)) = stage_records(&listing, p).find(|(stage, mode, _)| *stage == wanted && mode == "160000") {
            gitlinks.extend_from_slice(format!("{mode} {oid}\t{p}\0").as_bytes());
            continue;
        }
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
        let r = exec_with_paths_slow(client, label, args, list).await?;
        let ok = r.ok;
        res = Some(match res.take() {
            Some(prev) => combine(prev, r),
            None => r,
        });
        if !ok {
            break;
        }
    }
    if !gitlinks.is_empty() && res.as_ref().is_none_or(|r| r.ok) {
        // A stage-0 entry replaces the path's conflict stages; `-z` takes any path as it is.
        let req = GitReq::new("update-index", ["update-index", "-z", "--index-info"]).stdin(gitlinks).user();
        let r = result(client.exec_allow_failure(req).await?);
        res = Some(match res.take() {
            Some(prev) => combine(prev, r),
            None => r,
        });
    }
    Ok(res.expect("at least one path is given"))
}

/// `exec_with_paths` with the commit timeout, for the steps that resolve a conflict: `git
/// checkout` runs the `post-checkout` hook, and a filter such as Git LFS runs for each file.
async fn exec_with_paths_slow(client: &GitClient, label: &'static str, args: &[&str], paths: &[String]) -> Result<OpResult> {
    let mut argv: Vec<String> = vec!["--literal-pathspecs".into()];
    argv.extend(args.iter().map(|a| a.to_string()));
    argv.push("--pathspec-from-file=-".into());
    argv.push("--pathspec-file-nul".into());
    let stdin: Vec<u8> = paths.iter().flat_map(|p| p.bytes().chain([0])).collect();
    let req = GitReq::new(label, argv).stdin(stdin).user().timeout(Some(COMMIT_TIMEOUT));
    Ok(result(client.exec_allow_failure(req).await?))
}

/// Write Git's merge of conflicted `path` to the working tree again, with conflict markers in the
/// `diff3` style, which carry each block's base text (`git checkout --conflict=diff3`). Whatever
/// has been done to the file since is lost, so the frontend asks first. Refuses, and changes
/// nothing, when the path is not conflicted any more: Git would then write the index's version
/// over the file instead. Refuses a symbolic link or a submodule too, which the merge editor does
/// not open: Git would make the link point at the conflict markers.
pub async fn restore_conflict(client: &GitClient, path: &str) -> Result<OpResult> {
    let paths = [path.to_string()];
    valid_paths(&paths)?;
    let listing = unmerged_listing(client).await?;
    if !parse_unmerged(&listing).contains_key(path) {
        return Err(GitError::InvalidPath { path: path.to_string(), reason: "the file is not conflicted any more".into() });
    }
    if crate::git::queries::conflict_file::special_refusal(&listing, path).is_some() {
        return Err(GitError::InvalidPath { path: path.to_string(), reason: "a symbolic link or a submodule has no text to merge".into() });
    }
    exec_with_paths_slow(client, "checkout", &["checkout", "--conflict=diff3"], &paths).await
}

/// Save `content` as working tree file `path` (relative to the repository), byte for byte, and
/// return the hash of what was written. `expected_hash` is the `hash` the file had when the merge
/// editor opened it (see `conflict_file::content_hash`); when the file on disk no longer has it,
/// nothing is written and this fails with `changedOnDisk`, so a change made meanwhile by another
/// program is never overwritten unasked. The file is written in place, keeping its mode. Refuses
/// when the file, or a folder on the way to it, is a symbolic link: writing would change the file
/// the link points to, which can be anywhere.
pub async fn write_worktree_file(client: &GitClient, path: &str, content: &str, expected_hash: &str) -> Result<String> {
    use crate::git::queries::conflict_file::content_hash;
    use crate::git::queries::diff::join_repo_path;
    valid_worktree_path(path)?;
    let parts: Vec<&str> = path.split(['/', '\\']).collect();
    let steps: Vec<_> = (1..=parts.len()).map(|n| join_repo_path(&client.repo, &parts[..n].join("/"))).collect();
    if client.transport.symlinks(&steps).await?.contains(&true) {
        return Err(GitError::InvalidPath { path: path.to_string(), reason: "it is a symbolic link, or inside one, so saving would write somewhere else".into() });
    }
    let full = join_repo_path(&client.repo, path);
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
    paths.iter().try_for_each(|p| crate::git::parse::utf8_name(p))
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
/// pass both the new and the original path. A path whose deletion is already
/// staged and that has no file in the working tree has nothing left to stage
/// and is skipped, since `git add` stops on it with "did not match any files";
/// that is the original path of a rename that is already staged.
///
/// A deleted file with a folder now in its place has only its deletion staged
/// (see `now_folders`). A submodule whose own files changed while the commit it
/// points at did not has nothing `git add` can stage; when nothing else was
/// staged either, this fails with `invalidPath` rather than report success.
pub async fn stage(client: &GitClient, paths: &[String]) -> Result<OpResult> {
    valid_paths(paths)?;
    let removed = already_removed(client, paths).await?;
    let rest: Vec<String> = paths.iter().filter(|p| !removed.contains(p.as_str())).cloned().collect();
    if rest.is_empty() {
        return Ok(OpResult { ok: true, exit_code: Some(0), stdout: String::new(), stderr: String::new(), conflicts: false });
    }
    let folders = now_folders(client, &rest, false).await?;
    let mut steps: Vec<OpResult> = Vec::new();
    if !folders.is_empty() {
        // Exact paths, not pathspecs, so nothing inside the folder is touched.
        let stdin: Vec<u8> = folders.iter().flat_map(|p| p.bytes().chain([0])).collect();
        let req = GitReq::new("update-index", ["update-index", "--force-remove", "-z", "--stdin"]).stdin(stdin).user();
        let removed = result(client.exec_allow_failure(req).await?);
        if !removed.ok {
            return Ok(removed);
        }
        steps.push(removed);
    }
    let rest: Vec<String> = rest.into_iter().filter(|p| !folders.contains(p)).collect();
    if !rest.is_empty() {
        // `-v` names each path staged, so an empty output means nothing was.
        let added = exec_with_paths(client, "add", &["add", "-A", "-v"], &rest).await?;
        if added.ok && added.stdout.is_empty() && steps.is_empty() {
            if let Some(p) = content_only_submodules(client, &rest).await?.into_iter().next() {
                return Err(GitError::InvalidPath {
                    path: p,
                    reason: "only files inside this submodule have changed, and those are committed in the submodule itself; the submodule can be staged once it points at a new commit".into(),
                });
            }
        }
        steps.push(added);
    }
    Ok(combine_all(steps))
}

/// Those of `paths` that are tracked files deleted from the working tree with a
/// folder now in their place that holds files Git does not track, ignored ones
/// included. A pathspec naming the file also names everything in that folder,
/// so `git add` would stage those files and `git restore` would delete them to
/// make room for the file. With `from_head`, files deleted since HEAD count too,
/// not only those deleted since the index.
async fn now_folders(client: &GitClient, paths: &[String], from_head: bool) -> Result<std::collections::HashSet<String>> {
    let chosen: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
    let mut deleted = list_paths(client, "diff", &["diff", "--name-only", "-z", "--no-renames", "--diff-filter=D"]).await?;
    if from_head {
        deleted.extend(list_paths(client, "diff", &["diff", "HEAD", "--name-only", "-z", "--no-renames", "--diff-filter=D", "--"]).await?);
    }
    let deleted: std::collections::HashSet<String> = deleted.into_iter().filter(|p| chosen.contains(p.as_str())).collect();
    let mut folders = std::collections::HashSet::new();
    // Each as `<path>/`: `--directory`, and `git status` without `-uall`, leave out a folder
    // named like a file in the index. `git ls-files` has no stdin form, so in chunks.
    let specs: Vec<String> = deleted.iter().map(|p| format!("{p}/")).collect();
    let mut chunk: Vec<&str> = Vec::new();
    let mut bytes = 0;
    for (i, spec) in specs.iter().enumerate() {
        chunk.push(spec);
        bytes += spec.len() + 1;
        if bytes < CLEAN_CHUNK_BYTES && i + 1 < specs.len() {
            continue;
        }
        let mut argv = vec!["--literal-pathspecs", "ls-files", "-z", "--others", "--"];
        argv.append(&mut chunk);
        bytes = 0;
        for p in list_paths(client, "ls-files", &argv).await? {
            for (i, _) in p.match_indices('/') {
                if let Some(dir) = deleted.get(&p[..i]) {
                    folders.insert(dir.clone());
                }
            }
        }
    }
    Ok(folders)
}

/// The changed submodules `git status` reports, as (path, `XY`, `S<c><m><u>`):
/// `c` is `C` when the submodule's checkout is at a commit other than the one
/// the index records, `m` and `u` mark changed and untracked files inside it.
async fn changed_submodules(client: &GitClient) -> Result<Vec<(String, String, String)>> {
    Ok(list_paths(client, "status", &["status", "--porcelain=v2", "-z"])
        .await?
        .into_iter()
        .filter_map(|record| {
            let f: Vec<&str> = record.splitn(9, ' ').collect();
            (f.len() == 9 && f[0] == "1" && f[2].starts_with('S')).then(|| (f[8].to_string(), f[1].to_string(), f[2].to_string()))
        })
        .collect())
}

/// Those of `paths` that are submodules whose own files changed while the
/// commit they point at did not.
async fn content_only_submodules(client: &GitClient, paths: &[String]) -> Result<Vec<String>> {
    let chosen: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
    Ok(changed_submodules(client)
        .await?
        .into_iter()
        .filter(|(p, _, sub)| chosen.contains(p.as_str()) && sub.as_bytes()[1] == b'.')
        .map(|(p, ..)| p)
        .collect())
}

/// Those of `paths` whose deletion is already staged and that have no file
/// in the working tree either.
async fn already_removed(client: &GitClient, paths: &[String]) -> Result<std::collections::HashSet<String>> {
    let chosen: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
    let mut gone: std::collections::HashSet<String> =
        list_paths(client, "diff", &["diff", "--cached", "--name-only", "-z", "--no-renames", "--diff-filter=D"])
            .await?
            .into_iter()
            .filter(|p| chosen.contains(p.as_str()))
            .collect();
    if !gone.is_empty() {
        // A file created again at a deleted path is untracked, and still needs staging.
        for p in list_paths(client, "ls-files", &["ls-files", "-z", "--others", "--exclude-standard"]).await? {
            gone.remove(&p);
        }
    }
    Ok(gone)
}

/// The NUL-separated paths a read-only Git command prints.
async fn list_paths(client: &GitClient, label: &'static str, args: &[&str]) -> Result<Vec<String>> {
    let out = client.exec_allow_failure(GitReq::new(label, args.to_vec())).await?;
    if !out.success() {
        return Err(classify_git_failure(label, vec![], out.exit_code, &out.stderr, &TransportKind::Local));
    }
    Ok(out
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect())
}

/// Move `paths` out of the index, back to their HEAD state; the working tree
/// is untouched. On an unborn branch there is no HEAD, so the paths are
/// removed from the index instead (`-f`, since Git otherwise refuses a file
/// whose staged content differs from the file on disk). For a staged rename,
/// pass both paths.
pub async fn unstage(client: &GitClient, paths: &[String]) -> Result<OpResult> {
    valid_paths(paths)?;
    if has_head(client).await? {
        exec_with_paths(client, "restore", &["restore", "--staged"], paths).await
    } else {
        exec_with_paths(client, "rm", &["rm", "--cached", "-f", "-q"], paths).await
    }
}

/// False on an unborn branch, which has no commit yet.
async fn has_head(client: &GitClient) -> Result<bool> {
    Ok(client.exec_allow_failure(GitReq::new("rev-parse", ["rev-parse", "--verify", "-q", "HEAD"])).await?.success())
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
    list_paths(client, "diff", &["diff", "--cached", "--name-only", "-z", "--no-renames"]).await
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
///
/// `--only` cannot commit a rename that changes only the letter case where
/// the file system ignores case (Windows, macOS): it starts from HEAD, which
/// still has the old name, and Git refuses the new name as an alias of it. So
/// when `paths` holds two names that differ only in case, the other staged
/// paths are unstaged instead, the index is committed, and they are staged
/// again exactly as they were, partly staged files included.
///
/// When every one of `paths` is already staged (and none is conflicted), they
/// are committed as staged, the same way: a partly staged file keeps its
/// unstaged part out of the commit, which `--only` would take from the disk.
/// That fails with `invalidPath` while a merge, cherry-pick or revert waits to
/// be committed, as `--only` does in Git.
pub async fn commit(client: &GitClient, message: &str, paths: Option<&[String]>, amend: bool, sign_off: bool) -> Result<OpResult> {
    let mut steps: Vec<OpResult> = Vec::new();
    let mut only: Option<&[String]> = None;
    let mut set_aside: Option<(String, Vec<String>)> = None;
    // An amend naming no paths rewords the last commit and leaves whatever is staged alone.
    if let Some([]) = paths {
        if !amend {
            return Err(GitError::InvalidPath { path: String::new(), reason: "no paths given".into() });
        }
        only = Some(&[]);
    } else if let Some(paths) = paths {
        valid_paths(paths)?;
        let settled: std::collections::HashSet<String> =
            list_paths(client, "diff", &["diff", "--cached", "--name-only", "-z", "--no-renames", "--diff-filter=u"]).await?.into_iter().collect();
        let as_staged = paths.iter().all(|p| settled.contains(p));
        if !as_staged {
            let added = stage(client, paths).await?;
            if !added.ok {
                return Ok(added);
            }
            steps.push(added);
        }
        let chosen: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        let others: Vec<String> = staged_paths(client).await?.into_iter().filter(|p| !chosen.contains(p.as_str())).collect();
        if !others.is_empty() && (as_staged || differ_only_in_case(paths)) {
            if let Some(op) = operation_to_commit(client).await? {
                return Err(GitError::InvalidPath {
                    path: String::new(),
                    reason: format!("a {op} is waiting to be committed, so the commit takes everything that is staged"),
                });
            }
            let tree = client.exec_allow_failure(GitReq::new("write-tree", ["write-tree"])).await?;
            if !tree.success() {
                steps.push(result(tree));
                return Ok(combine_all(steps));
            }
            let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();
            let aside = unstage(client, &others).await?;
            let ok = aside.ok;
            steps.push(aside);
            if !ok {
                steps.push(stage_from_tree(client, &tree, &others).await?);
                return Ok(combine_all(steps));
            }
            set_aside = Some((tree, others));
        } else if !others.is_empty() {
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
    steps.push(result(client.exec_allow_failure(req).await?));
    if let Some((tree, others)) = set_aside {
        steps.push(stage_from_tree(client, &tree, &others).await?);
    }
    Ok(combine_all(steps))
}

/// The merge, cherry-pick or revert whose commit the next `git commit` makes,
/// if one is in progress.
async fn operation_to_commit(client: &GitClient) -> Result<Option<&'static str>> {
    for (head, op) in [("MERGE_HEAD", "merge"), ("CHERRY_PICK_HEAD", "cherry-pick"), ("REVERT_HEAD", "revert")] {
        if client.exec_allow_failure(GitReq::new("rev-parse", ["rev-parse", "--verify", "-q", head])).await?.success() {
            return Ok(Some(op));
        }
    }
    Ok(None)
}

/// True when two of `paths` are the same name in different letter case, as
/// both sides of a rename that changes only the case are.
fn differ_only_in_case(paths: &[String]) -> bool {
    let distinct: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
    let folded: std::collections::HashSet<String> = distinct.iter().map(|p| p.to_lowercase()).collect();
    folded.len() < distinct.len()
}

/// Stage `paths` exactly as they are in `tree`, removing those it lacks.
async fn stage_from_tree(client: &GitClient, tree: &str, paths: &[String]) -> Result<OpResult> {
    let source = format!("--source={tree}");
    exec_with_paths(client, "restore", &["restore", &source, "--staged"], paths).await
}

/// Every step's output, succeeding only when all of them did.
fn combine_all(steps: Vec<OpResult>) -> OpResult {
    steps.into_iter().reduce(combine).expect("at least one step")
}

/// Most untracked paths per `git clean` call, which has no stdin form.
const CLEAN_CHUNK_BYTES: usize = 16 * 1024;

/// Throw away changes. `tracked` go back to the index (`to_head` false) or to
/// HEAD in both the index and the working tree (`to_head` true, which also
/// removes files that are staged but not in HEAD). `untracked` are deleted.
/// Nothing here can be undone; the frontend confirms first.
///
/// - On an unborn branch every staged file is new, so going back to HEAD
///   removes it, as it would with a HEAD.
/// - An intent-to-add file (`git add -N`) is removed too: its index entry is an
///   empty placeholder, which `git restore` would write over the file.
/// - A changed submodule is moved back to the commit it should point at
///   (`git submodule update --checkout`, which stops rather than overwrite
///   changes inside it). One whose own files changed while that commit did not
///   is skipped, since those changes belong to the submodule.
/// - Refused with `invalidPath`, before anything changes: a deleted file with a
///   folder of untracked files now in its place (restoring the file would
///   delete them), and an untracked folder, which the status lists only for a
///   repository of its own (`git clean -f` skips one without a word).
pub async fn discard(client: &GitClient, tracked: &[String], untracked: &[String], to_head: bool) -> Result<OpResult> {
    if tracked.is_empty() && untracked.is_empty() {
        return Err(GitError::InvalidPath { path: String::new(), reason: "no paths given".into() });
    }
    if let Some(p) = untracked.iter().find(|p| p.ends_with('/')) {
        return Err(GitError::InvalidPath {
            path: p.clone(),
            reason: "it is a Git repository of its own, which StashPanda does not delete; delete the folder yourself if you mean to".into(),
        });
    }
    let mut res: Option<OpResult> = None;
    if !tracked.is_empty() {
        valid_paths(tracked)?;
        let head = has_head(client).await?;
        if let Some(p) = now_folders(client, tracked, to_head && head).await?.into_iter().next() {
            return Err(GitError::InvalidPath {
                path: p,
                reason: "a folder holding untracked files now has the deleted file's name, and restoring the file would delete them; move the folder away first".into(),
            });
        }
        let chosen: std::collections::HashSet<&str> = tracked.iter().map(String::as_str).collect();
        let submodules: Vec<(String, String, String)> =
            changed_submodules(client).await?.into_iter().filter(|(p, ..)| chosen.contains(p.as_str())).collect();
        let inside: Vec<&String> =
            submodules.iter().filter(|(_, _, sub)| !to_head && sub.as_bytes()[1] == b'.').map(|(p, ..)| p).collect();
        let tracked: Vec<String> = tracked.iter().filter(|p| !inside.contains(p)).cloned().collect();
        if tracked.is_empty() && untracked.is_empty() {
            return Err(GitError::InvalidPath {
                path: inside[0].clone(),
                reason: "only files inside this submodule have changed; discard them in the submodule itself".into(),
            });
        }
        let mut steps: Vec<OpResult> = Vec::new();
        if to_head && !head {
            steps.push(exec_with_paths(client, "rm", &["rm", "-f", "-q"], &tracked).await?);
        } else if to_head {
            steps.push(exec_with_paths(client, "restore", &["restore", "--source=HEAD", "--staged", "--worktree"], &tracked).await?);
        } else if !tracked.is_empty() {
            let placeholders = intent_to_add(client, &tracked).await?;
            let rest: Vec<String> = tracked.iter().filter(|p| !placeholders.contains(*p)).cloned().collect();
            if !placeholders.is_empty() {
                steps.push(exec_with_paths(client, "rm", &["rm", "-f", "-q"], &placeholders).await?);
            }
            if !rest.is_empty() && steps.iter().all(|s| s.ok) {
                steps.push(exec_with_paths(client, "restore", &["restore", "--worktree"], &rest).await?);
            }
        }
        // A submodule added since HEAD is gone from the index now.
        let moved: Vec<&str> = submodules
            .iter()
            .filter(|(p, xy, _)| !inside.contains(&p) && !(to_head && xy.starts_with('A')))
            .map(|(p, ..)| p.as_str())
            .collect();
        if !moved.is_empty() && steps.iter().all(|s| s.ok) {
            let mut argv = vec!["--literal-pathspecs", "submodule", "update", "--checkout", "--no-fetch", "--"];
            argv.extend(moved);
            steps.push(result(client.exec_allow_failure(GitReq::new("submodule", argv).user()).await?));
        }
        if !steps.is_empty() {
            let r = combine_all(steps);
            if !r.ok {
                return Ok(r);
            }
            res = Some(r);
        }
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

/// Those of `paths` that are intent-to-add (`git add -N`): new in the working
/// tree against an index that has only a placeholder for them.
async fn intent_to_add(client: &GitClient, paths: &[String]) -> Result<Vec<String>> {
    let chosen: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
    Ok(list_paths(client, "diff", &["diff", "--name-only", "-z", "--no-renames", "--diff-filter=A"])
        .await?
        .into_iter()
        .filter(|p| chosen.contains(p.as_str()))
        .collect())
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
        let m = merge(&client, "topic", true, false).await.unwrap();
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
        let c = merge(&client, "clash", false, false).await.unwrap();
        assert!(!c.ok && c.conflicts, "{c:?}");
        assert!(merge_abort(&client).await.unwrap().ok);
        assert!(branch_delete(&client, "clash", true).await.unwrap().ok);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn names_that_are_also_files_tags_or_branches_reach_git_as_the_ref_meant() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-names-{}", uuid::Uuid::new_v4().simple()));
        let (origin, dir) = (root.join("origin.git"), root.join("local"));
        std::fs::create_dir_all(&dir).unwrap();
        git(&root, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("notes"), "original\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "one"]);
        git(&dir, &["remote", "add", "origin", &origin.to_string_lossy()]);
        git(&dir, &["push", "-q", "origin", "main", "main:dev"]);
        git(&dir, &["fetch", "-q", "origin"]);
        let client = client_for(&dir);

        // A name that is no longer a branch but is a file: refused, and the file keeps its edit.
        std::fs::write(dir.join("notes"), "edited\n").unwrap();
        let gone = checkout(&client, "notes").await.unwrap();
        assert!(!gone.ok, "{gone:?}");
        assert_eq!(std::fs::read_to_string(dir.join("notes")).unwrap(), "edited\n");
        git(&dir, &["checkout", "-q", "--", "notes"]);

        // A tag and a branch both named v1: the tag's full name detaches HEAD at the tag.
        let one = out(&dir, &["rev-parse", "HEAD"]);
        git(&dir, &["tag", "v1"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "two"]);
        git(&dir, &["branch", "v1"]);
        assert!(checkout(&client, "refs/tags/v1").await.unwrap().ok);
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), one);
        assert_eq!(out(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]), "HEAD");
        assert!(checkout(&client, "main").await.unwrap().ok);

        // A tag named like the remote branch: the full name still starts the branch there, and
        // the upstream, set as its own step, is the remote branch.
        git(&dir, &["tag", "origin/dev"]);
        let tracked = branch_create(&client, "dev", "refs/remotes/origin/dev", true).await.unwrap();
        assert!(tracked.ok, "{tracked:?}");
        assert!(branch_set_upstream(&client, "dev", Some("refs/remotes/origin/dev")).await.unwrap().ok);
        assert_eq!(out(&dir, &["rev-parse", "--symbolic-full-name", "dev@{upstream}"]), "refs/remotes/origin/dev");
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), one);

        // A tag and a branch both named topic: merging the branch's full name merges the branch,
        // and Git names it so in the message.
        git(&dir, &["tag", "topic"]);
        git(&dir, &["checkout", "-q", "-b", "topic"]);
        std::fs::write(dir.join("t.txt"), "t\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "topic work"]);
        git(&dir, &["checkout", "-q", "main"]);
        let merged = merge(&client, "refs/heads/topic", true, false).await.unwrap();
        assert!(merged.ok, "{merged:?}");
        assert_eq!(out(&dir, &["log", "-1", "--format=%s"]), "Merge branch 'refs/heads/topic'");
        assert!(dir.join("t.txt").exists());

        // An annotated tag keeps message lines that start with `#`.
        assert!(tag_create(&client, "v2", "HEAD", Some("#123 release\n#124 fixed"), false).await.unwrap().ok);
        assert_eq!(out(&dir, &["tag", "-l", "--format=%(contents)", "v2"]), "#123 release\n#124 fixed");

        // A rename that only changes letter case; where both names are branches, nothing is
        // overwritten.
        git(&dir, &["branch", "case"]);
        assert!(branch_rename(&client, "case", "Case").await.unwrap().ok);
        assert_eq!(out(&dir, &["branch", "--list", "Case"]), "Case");
        git(&dir, &["branch", "case", "HEAD~1"]);
        let taken = branch_rename(&client, "case", "Case").await.unwrap();
        assert!(!taken.ok && taken.stderr.contains("already exists"), "{taken:?}");
        assert_eq!(out(&dir, &["rev-parse", "Case"]), out(&dir, &["rev-parse", "HEAD"]));

        // A branch with no commits yet: checking out a new branch from HEAD renames it, as
        // `git checkout -b` does; creating one without checking it out fails in Git.
        let empty = root.join("empty");
        git(&root, &["init", "-q", "-b", "main", "empty"]);
        let unborn = client_for(&empty);
        let renamed = branch_create(&unborn, "feature", "HEAD", true).await.unwrap();
        assert!(renamed.ok, "{renamed:?}");
        assert_eq!(out(&empty, &["symbolic-ref", "HEAD"]), "refs/heads/feature");
        assert!(!branch_create(&unborn, "other", "HEAD", false).await.unwrap().ok);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_case_only_rename_uses_capital_m_unless_the_new_name_is_a_branch() {
        assert_eq!(rename_flag("feature", "Feature", false), "-M");
        assert_eq!(rename_flag("feature", "Feature", true), "-m");
        assert_eq!(rename_flag("feature", "topic", false), "-m");
        assert_eq!(rename_flag("Ünï", "ünï", false), "-M");
    }

    #[test]
    fn remote_branch_names_come_from_the_fetch_refspec() {
        let standard = ["+refs/heads/*:refs/remotes/origin/*"];
        assert_eq!(remote_ref_for(standard, "refs/remotes/origin/feature/x").as_deref(), Some("refs/heads/feature/x"));
        let custom = ["^refs/heads/secret", "+refs/heads/*:refs/remotes/origin/team-*", "refs/heads/main:refs/remotes/origin/trunk"];
        assert_eq!(remote_ref_for(custom, "refs/remotes/origin/team-dev").as_deref(), Some("refs/heads/dev"));
        assert_eq!(remote_ref_for(custom, "refs/remotes/origin/trunk").as_deref(), Some("refs/heads/main"));
        assert_eq!(remote_ref_for(custom, "refs/remotes/origin/dev"), None);
    }

    #[tokio::test]
    async fn remote_branches_on_a_custom_fetch_refspec_and_a_remote_named_with_a_slash() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-refspec-{}", uuid::Uuid::new_v4().simple()));
        let (origin, dir) = (root.join("origin.git"), root.join("local"));
        std::fs::create_dir_all(&dir).unwrap();
        git(&root, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let url = origin.to_string_lossy().into_owned();
        git(&dir, &["push", "-q", &url, "main", "main:dev", "main:feature"]);
        git(&dir, &["remote", "add", "origin", &url]);
        git(&dir, &["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/team-*"]);
        git(&dir, &["remote", "add", "team/up", &url]);
        git(&dir, &["fetch", "-q", "--all"]);
        let client = client_for(&dir);
        let progress = |_: String| {};
        let cancel = tokio_util::sync::CancellationToken::new();
        let ls_remote = || out(&dir, &["ls-remote", "--heads", &url]);

        // The remote-tracking branch origin/team-dev is the remote's dev.
        let deleted = delete_remote_branch(&client, "origin", "team-dev", &progress, &cancel).await.unwrap();
        assert!(deleted.ok, "{deleted:?}");
        assert!(!ls_remote().contains("refs/heads/dev"), "{}", ls_remote());
        assert!(matches!(delete_remote_branch(&client, "origin", "dev", &progress, &cancel).await, Err(GitError::InvalidPath { .. })));

        // The remote team/up: its branches are listed under it, and deleting goes to it.
        let refs = get_refs(&client).await.unwrap();
        let feature = refs.refs.iter().find(|r| r.short == "team/up/feature").expect("team/up/feature listed");
        assert_eq!(feature.kind, crate::git::queries::refs::RefKind::RemoteBranch { remote: "team/up".into() });
        let deleted = delete_remote_branch(&client, "team/up", "feature", &progress, &cancel).await.unwrap();
        assert!(deleted.ok, "{deleted:?}");
        assert!(!ls_remote().contains("refs/heads/feature"), "{}", ls_remote());
        let _ = std::fs::remove_dir_all(&root);
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
        let clean = rebase(&client, "main", false).await.unwrap();
        assert!(clean.ok && !clean.conflicts, "{clean:?}");
        assert_eq!(out(&dir, &["rev-parse", "HEAD~1"]), main);
        assert_eq!(get_in_progress(&client, &git_dir).await, None);

        // A conflicting rebase stops at the second of three commits, and says so; abort puts
        // the branch back.
        git(&dir, &["checkout", "-q", "topic"]);
        let before = out(&dir, &["rev-parse", "HEAD"]);
        let stopped = rebase(&client, "main", false).await.unwrap();
        assert!(!stopped.ok && stopped.conflicts, "{stopped:?}");
        let op = get_in_progress(&client, &git_dir).await.expect("a rebase in progress");
        assert_eq!(op.kind, InProgressKind::Rebase);
        assert_eq!(op.branch.as_deref(), Some("refs/heads/topic"));
        assert_eq!(op.onto.as_deref(), Some(main.as_str()));
        assert_eq!((op.step, op.total), (Some(2), Some(3)));
        assert_eq!(op.commit.as_deref(), Some(topic_two.as_str()));
        assert_eq!(op.subject.as_deref(), Some("topic two"));
        assert!(!op.interrupted, "stopped by Git itself, on a conflict");
        assert!(matches!(skip_operation(&client, InProgressKind::Merge).await, Err(GitError::InvalidPath { .. })));
        // What a Git killed part way through applying the commit leaves: the step is listed as
        // done, but nothing marks it as stopped.
        for marker in ["REBASE_HEAD", "rebase-merge/stopped-sha"] {
            std::fs::remove_file(Path::new(&git_dir).join(marker)).unwrap();
        }
        assert!(get_in_progress(&client, &git_dir).await.expect("still rebasing").interrupted);
        assert!(rebase_abort(&client).await.unwrap().ok);
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), before);
        assert_eq!(get_in_progress(&client, &git_dir).await, None);

        // Again: Continue is refused while a.txt is unmerged. Taking topic's version of a.txt
        // ("theirs" during a rebase) resolves it; Continue then stops on the third commit's
        // conflict, and Skip leaves that commit out and finishes.
        assert!(rebase(&client, "main", false).await.unwrap().conflicts);
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
        assert!(matches!(rebase(&client, "-x", false).await, Err(GitError::InvalidPath { .. })));
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

        let m = merge(&client, "feature", false, false).await.unwrap();
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

    /// A submodule and a symbolic link that both sides changed. The merge editor refuses both,
    /// never reads or writes through the link, and taking a side stages that side's commit for
    /// the submodule, not the one the submodule happens to have checked out.
    #[cfg(unix)]
    #[tokio::test]
    async fn submodule_and_symlink_conflicts_are_taken_whole_and_never_written_through() {
        use crate::git::queries::conflict_file::{content_hash, get_conflict_file, ConflictRefusal};
        use std::os::unix::fs::symlink;
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-special-{}", uuid::Uuid::new_v4().simple()));
        let (sub, dir, elsewhere) = (root.join("sub-origin"), root.join("repo"), root.join("elsewhere"));
        for d in [&sub, &dir, &elsewhere] {
            std::fs::create_dir_all(d).unwrap();
            git(d, &["init", "-q", "-b", "main"]);
        }
        // The submodule: a root commit, and one commit on top of it for each of three branches.
        std::fs::write(sub.join("f"), "root\n").unwrap();
        git(&sub, &["add", "-A"]);
        git(&sub, &["commit", "-qm", "root"]);
        let sub_root = out(&sub, &["rev-parse", "HEAD"]);
        let mut tips = Vec::new();
        for name in ["a", "b", "c"] {
            git(&sub, &["checkout", "-qB", name, "main"]);
            std::fs::write(sub.join(name), "x\n").unwrap();
            git(&sub, &["add", "-A"]);
            git(&sub, &["commit", "-qm", name]);
            tips.push(out(&sub, &["rev-parse", "HEAD"]));
        }
        let (sub_main, sub_feature, sub_elsewhere) = (&tips[0], &tips[1], &tips[2]);
        let outside = root.join("outside.txt");
        std::fs::write(&outside, "outside the repository\n").unwrap();
        std::fs::write(elsewhere.join("f.txt"), "elsewhere\n").unwrap();

        // base: the submodule at its root commit, `link` to README. feature: the submodule at b,
        // `link` to other.txt. main: the submodule at a, `link` out of the repository.
        let st = Command::new("git").arg("-C").arg(&dir).args(["-c", "protocol.file.allow=always", "submodule", "add", "-q"]).arg(&sub).arg("sub").status().unwrap();
        assert!(st.success());
        let point = |commit: &str, target: &str| {
            git(&dir.join("sub"), &["checkout", "-q", commit]);
            let _ = std::fs::remove_file(dir.join("link"));
            symlink(target, dir.join("link")).unwrap();
            git(&dir, &["add", "-A"]);
        };
        std::fs::write(dir.join("README"), "readme\n").unwrap();
        point(&sub_root, "README");
        git(&dir, &["commit", "-qm", "base"]);
        git(&dir, &["checkout", "-qb", "feature"]);
        point(sub_feature, "other.txt");
        git(&dir, &["commit", "-qm", "feature"]);
        git(&dir, &["checkout", "-q", "main"]);
        git(&dir, &["-c", "protocol.file.allow=always", "submodule", "update", "-q"]);
        point(sub_main, "../outside.txt");
        git(&dir, &["commit", "-qm", "main"]);
        // The submodule's own checkout moves on to a third commit, not staged.
        git(&dir.join("sub"), &["checkout", "-q", sub_elsewhere]);
        let client = client_for(&dir);
        let m = merge(&client, "feature", false, false).await.unwrap();
        assert!(!m.ok && m.conflicts, "{m:?}");
        assert_eq!(out(&dir, &["status", "--porcelain", "--", "link", "sub"]), "UU link\nUU sub");

        // The merge editor refuses both, with a reason, rather than reading through the link or
        // failing on the submodule's commits.
        assert_eq!(get_conflict_file(&client, None, "link").await.unwrap().refused, Some(ConflictRefusal::Symlink));
        assert_eq!(get_conflict_file(&client, None, "sub").await.unwrap().refused, Some(ConflictRefusal::Submodule));
        // Saving never writes through a link, whether it is the file or a folder on the way.
        let hash = content_hash(b"outside the repository\n");
        assert!(matches!(write_worktree_file(&client, "link", "x\n", &hash).await, Err(GitError::InvalidPath { .. })));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside the repository\n");
        symlink(&elsewhere, dir.join("linked")).unwrap();
        let hash = content_hash(b"elsewhere\n");
        assert!(matches!(write_worktree_file(&client, "linked/f.txt", "x\n", &hash).await, Err(GitError::InvalidPath { .. })));
        assert_eq!(std::fs::read_to_string(elsewhere.join("f.txt")).unwrap(), "elsewhere\n");
        std::fs::remove_file(dir.join("linked")).unwrap();
        // Starting again from Git's merge would point the link at the conflict markers.
        assert!(matches!(restore_conflict(&client, "link").await, Err(GitError::InvalidPath { .. })));
        assert_eq!(std::fs::read_link(dir.join("link")).unwrap(), Path::new("../outside.txt"));

        // Taking feature's side stages feature's link and feature's submodule commit.
        let taken = resolve_conflicts(&client, &strs(&["sub", "link"]), ResolveSide::Theirs).await.unwrap();
        assert!(taken.ok, "{taken:?}");
        assert_eq!(out(&dir, &["ls-files", "-s", "sub"]), format!("160000 {sub_feature} 0\tsub"));
        assert_eq!(std::fs::read_link(dir.join("link")).unwrap(), Path::new("other.txt"));
        assert_eq!(out(&dir, &["ls-files", "-u"]), "");
        assert!(continue_operation(&client, InProgressKind::Merge).await.unwrap().ok);
        assert_eq!(out(&dir, &["rev-parse", "HEAD:sub"]), *sub_feature);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn safe_directory_lets_git_open_a_repository_it_refused() {
        if which::which("git").is_err() { return; }
        // The quote in the name, as in `C:/Users/O'Brien`, is part of the path Git reports.
        let dir = std::env::temp_dir().join(format!("stashpanda-safe-O'Brien-{}", uuid::Uuid::new_v4().simple()));
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
        assert!(path.ends_with(&*dir.file_name().unwrap().to_string_lossy()), "{path}");
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
        git(&dir, &["commit", "-qm", "third"]);

        // A rename that is already staged: the original path is in neither the
        // index nor the working tree, so only the commit can name it.
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        git(&dir, &["mv", "renamed.txt", "sub/moved.txt"]);
        std::fs::write(dir.join("sub/token.txt"), "untracked\n").unwrap();
        std::fs::write(dir.join("b.txt"), "staged, not selected\n").unwrap();
        git(&dir, &["add", "b.txt"]);
        let chosen = strs(&["sub/moved.txt", "renamed.txt", "sub/token.txt"]);
        let r = commit(&client, "moved", Some(&chosen), false, false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(files_in("HEAD"), strs(&["renamed.txt", "sub/moved.txt", "sub/token.txt"]));
        assert_eq!(status_of(&client).await, vec![("b.txt".into(), true, false, false)]);

        // The same with nothing else staged, so the commit names no paths.
        git(&dir, &["commit", "-qm", "b"]);
        git(&dir, &["mv", "sub/moved.txt", "moved-back.txt"]);
        let r = commit(&client, "moved back", Some(&strs(&["moved-back.txt", "sub/moved.txt"])), false, false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(files_in("HEAD"), strs(&["moved-back.txt", "sub/moved.txt"]));
        assert_eq!(status_of(&client).await, vec![]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn commit_a_case_only_rename_while_other_files_stay_staged() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-case-rename-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        for (name, text) in [("Readme.md", "readme\n"), ("b.txt", "b\n"), ("gone.txt", "gone\n")] {
            std::fs::write(dir.join(name), text).unwrap();
        }
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let client = client_for(&dir);

        // b.txt is partly staged and gone.txt is a staged deletion; neither is selected.
        git(&dir, &["mv", "Readme.md", "README.md"]);
        std::fs::write(dir.join("b.txt"), "staged b\n").unwrap();
        git(&dir, &["add", "b.txt"]);
        std::fs::write(dir.join("b.txt"), "unstaged b\n").unwrap();
        git(&dir, &["rm", "-q", "gone.txt"]);
        assert!(differ_only_in_case(&strs(&["README.md", "Readme.md"])));
        assert!(!differ_only_in_case(&strs(&["README.md", "README.md", "b.txt"])));

        let r = commit(&client, "case only", Some(&strs(&["README.md", "Readme.md"])), false, false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(out(&dir, &["diff-tree", "-r", "--name-status", "--no-commit-id", "HEAD"]), "A\tREADME.md\nD\tReadme.md");
        assert_eq!(out(&dir, &["status", "--porcelain"]), "MM b.txt\nD  gone.txt");
        assert_eq!(out(&dir, &["show", ":b.txt"]), "staged b");

        // When the commit fails, the other files are put back all the same.
        git(&dir, &["mv", "README.md", "ReadMe.md"]);
        let r = commit(&client, "", Some(&strs(&["ReadMe.md", "README.md"])), false, false).await.unwrap();
        assert!(!r.ok, "{r:?}");
        assert_eq!(out(&dir, &["status", "--porcelain"]), "R  README.md -> ReadMe.md\nMM b.txt\nD  gone.txt");
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

        // A partly staged file: Git refuses `rm --cached` without `-f`.
        assert!(stage(&client, &strs(&["a.txt"])).await.unwrap().ok);
        std::fs::write(dir.join("a.txt"), "a\nmore\n").unwrap();
        let r = unstage(&client, &strs(&["a.txt"])).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(status_of(&client).await, vec![("a.txt".into(), false, false, true)]);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "a\nmore\n");

        // Discarding a staged file with no commit to go back to removes it, as it removes a
        // staged new file when there is one; the untracked file stays.
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        assert!(stage(&client, &strs(&["b.txt"])).await.unwrap().ok);
        std::fs::write(dir.join("b.txt"), "b\nunstaged\n").unwrap();
        let r = discard(&client, &strs(&["b.txt"]), &[], true).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert!(!dir.join("b.txt").exists());
        assert_eq!(status_of(&client).await, vec![("a.txt".into(), false, false, true)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn discard_an_intent_to_add_file() {
        use super::lines::{apply_lines, ChangeKind, LineAction, SelectedLine};
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-intent-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("base.txt"), "base\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let client = client_for(&dir);

        // `git add -N` leaves an empty placeholder in the index; `git restore` would write it over
        // the file. The file goes, as an untracked one does, and so does the placeholder.
        std::fs::write(dir.join("new.txt"), "precious\n").unwrap();
        std::fs::write(dir.join("changed.txt"), "x\n").unwrap();
        git(&dir, &["add", "changed.txt"]); git(&dir, &["commit", "-qm", "changed"]);
        std::fs::write(dir.join("changed.txt"), "y\n").unwrap();
        git(&dir, &["add", "-N", "new.txt"]);
        let r = discard(&client, &strs(&["new.txt", "changed.txt"]), &[], false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert!(!dir.join("new.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("changed.txt")).unwrap(), "x\n");
        assert_eq!(status_of(&client).await, vec![]);

        // The same when the diff viewer discards every line.
        std::fs::write(dir.join("new.txt"), "precious\n").unwrap();
        git(&dir, &["add", "-N", "new.txt"]);
        let all = [SelectedLine { kind: ChangeKind::Add, line: 1, text: "precious".into() }];
        let r = apply_lines(&client, LineAction::Discard, "new.txt", false, &all).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert!(!dir.join("new.txt").exists());
        assert_eq!(status_of(&client).await, vec![]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_deleted_file_with_a_folder_in_its_place() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-file-to-folder-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a"), "file\n").unwrap();
        std::fs::write(dir.join("keep.txt"), "keep\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let client = client_for(&dir);
        std::fs::remove_file(dir.join("a")).unwrap();
        std::fs::create_dir(dir.join("a")).unwrap();
        std::fs::write(dir.join("a/x.txt"), "precious\n").unwrap();
        std::fs::write(dir.join("keep.txt"), "changed\n").unwrap();

        // Discarding the deletion would delete a/x.txt to make room for the file: refused, and
        // nothing changes, keep.txt included.
        let r = discard(&client, &strs(&["keep.txt", "a"]), &[], false).await;
        assert!(matches!(&r, Err(GitError::InvalidPath { path, .. }) if path == "a"), "{r:?}");
        assert_eq!(std::fs::read_to_string(dir.join("a/x.txt")).unwrap(), "precious\n");
        assert_eq!(std::fs::read_to_string(dir.join("keep.txt")).unwrap(), "changed\n");

        // Staging the deletion stages only that, not the files in the folder.
        assert!(stage(&client, &strs(&["a"])).await.unwrap().ok);
        assert_eq!(out(&dir, &["status", "--porcelain", "--untracked-files=all"]), "D  a\n M keep.txt\n?? a/x.txt");

        // Back to HEAD, from the staged deletion: refused the same way.
        let r = discard(&client, &strs(&["a"]), &[], true).await;
        assert!(matches!(&r, Err(GitError::InvalidPath { path, .. }) if path == "a"), "{r:?}");
        assert_eq!(std::fs::read_to_string(dir.join("a/x.txt")).unwrap(), "precious\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn submodules_and_nested_repositories() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-submodules-{}", uuid::Uuid::new_v4().simple()));
        let lib = dir.join("lib");
        let repo = dir.join("repo");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        git(&lib, &["init", "-q", "-b", "main"]);
        std::fs::write(lib.join("f.txt"), "1\n").unwrap();
        git(&lib, &["add", "-A"]); git(&lib, &["commit", "-qm", "one"]);
        std::fs::write(lib.join("f.txt"), "2\n").unwrap();
        git(&lib, &["commit", "-qam", "two"]);
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.to_str().unwrap(), "sub"]);
        git(&repo, &["commit", "-qm", "sub"]);
        let client = client_for(&repo);
        let sub_head = || out(&repo.join("sub"), &["rev-parse", "HEAD"]);
        let recorded = out(&repo, &["rev-parse", "HEAD:sub"]);

        // Only a file inside the submodule changed: there is nothing to stage or discard here.
        std::fs::write(repo.join("sub/f.txt"), "edited inside\n").unwrap();
        let r = stage(&client, &strs(&["sub"])).await;
        assert!(matches!(&r, Err(GitError::InvalidPath { path, .. }) if path == "sub"), "{r:?}");
        let r = discard(&client, &strs(&["sub"]), &[], false).await;
        assert!(matches!(&r, Err(GitError::InvalidPath { path, .. }) if path == "sub"), "{r:?}");
        assert_eq!(std::fs::read_to_string(repo.join("sub/f.txt")).unwrap(), "edited inside\n");
        git(&repo.join("sub"), &["checkout", "-q", "--", "f.txt"]);

        // The submodule checked out at another commit: discarding moves it back.
        git(&repo.join("sub"), &["checkout", "-q", "HEAD~1"]);
        assert_ne!(sub_head(), recorded);
        let r = discard(&client, &strs(&["sub"]), &[], false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(sub_head(), recorded);
        assert_eq!(status_of(&client).await, vec![]);

        // Staged, then back to HEAD: the index and the checkout both go back.
        git(&repo.join("sub"), &["checkout", "-q", "HEAD~1"]);
        assert!(stage(&client, &strs(&["sub"])).await.unwrap().ok);
        let r = discard(&client, &strs(&["sub"]), &[], true).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(sub_head(), recorded);
        assert_eq!(status_of(&client).await, vec![]);

        // An untracked repository of its own: `git clean -f` would skip it without a word.
        git(&repo, &["init", "-q", "nested"]);
        std::fs::write(repo.join("nested/n.txt"), "n\n").unwrap();
        std::fs::write(repo.join("loose.txt"), "loose\n").unwrap();
        let r = discard(&client, &[], &strs(&["loose.txt", "nested/"]), false).await;
        assert!(matches!(&r, Err(GitError::InvalidPath { path, .. }) if path == "nested/"), "{r:?}");
        assert!(repo.join("nested/n.txt").exists() && repo.join("loose.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn commit_staged_files_as_they_are_staged() {
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-commit-staged-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "user.name", "T"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let client = client_for(&dir);

        // a.txt is partly staged and b.txt staged; committing a.txt alone takes its staged part.
        std::fs::write(dir.join("a.txt"), "a staged\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        std::fs::write(dir.join("a.txt"), "a staged\nnot staged\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b staged\n").unwrap();
        git(&dir, &["add", "b.txt"]);
        let r = commit(&client, "a only", Some(&strs(&["a.txt"])), false, false).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(out(&dir, &["show", "HEAD:a.txt"]), "a staged");
        assert_eq!(out(&dir, &["diff-tree", "-r", "--name-only", "--no-commit-id", "HEAD"]), "a.txt");
        assert_eq!(status_of(&client).await, vec![("a.txt".into(), false, true, false), ("b.txt".into(), true, false, false)]);

        // While a merge waits for its commit, a commit of some staged files only is refused.
        git(&dir, &["commit", "-qm", "b"]);
        git(&dir, &["checkout", "-q", "-b", "topic"]);
        std::fs::write(dir.join("t.txt"), "t\n").unwrap();
        git(&dir, &["add", "t.txt"]); git(&dir, &["commit", "-qm", "topic"]);
        git(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("b.txt"), "b on main\n").unwrap();
        git(&dir, &["add", "b.txt"]); git(&dir, &["commit", "-qm", "main"]);
        git(&dir, &["merge", "-q", "--no-commit", "--no-ff", "topic"]);
        std::fs::write(dir.join("b.txt"), "b staged during the merge\n").unwrap();
        git(&dir, &["add", "b.txt"]);
        let r = commit(&client, "part of a merge", Some(&strs(&["b.txt"])), false, false).await;
        assert!(matches!(&r, Err(GitError::InvalidPath { reason, .. }) if reason.contains("merge")), "{r:?}");
        let staged = |p: &str| (p.to_string(), true, false, false);
        assert_eq!(status_of(&client).await, vec![("a.txt".into(), false, true, false), staged("b.txt"), staged("t.txt")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stage_lines_of_new_files_as_git_add_would() {
        use super::lines::{apply_lines, ChangeKind, LineAction, SelectedLine};
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-new-lines-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "core.autocrlf", "true"]);
        std::fs::write(dir.join("base.txt"), "base\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let client = client_for(&dir);
        let add = |line: u32, text: &str| SelectedLine { kind: ChangeKind::Add, line, text: text.into() };

        // CRLF lines reach the index as LF, as `git add` would store them with core.autocrlf.
        std::fs::write(dir.join("crlf.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
        let r = apply_lines(&client, LineAction::Stage, "crlf.txt", true, &[add(1, "one"), add(2, "two")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(out(&dir, &["cat-file", "-p", ":crlf.txt"]), "one\ntwo");

        // A selection that no longer matches leaves the file untracked, as it was.
        let r = apply_lines(&client, LineAction::Stage, "gone.txt", true, &[add(1, "x")]).await;
        assert!(r.is_err() || !r.unwrap().ok);
        std::fs::write(dir.join("stale.txt"), "a\nb\n").unwrap();
        let r = apply_lines(&client, LineAction::Stage, "stale.txt", true, &[add(1, "not a")]).await;
        assert!(matches!(r, Err(GitError::InvalidPath { .. })), "{r:?}");
        assert_eq!(out(&dir, &["status", "--porcelain", "--", "stale.txt"]), "?? stale.txt");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // An executable keeps its mode.
            std::fs::write(dir.join("run.sh"), "#!/bin/sh\necho 1\necho 2\n").unwrap();
            std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
            let r = apply_lines(&client, LineAction::Stage, "run.sh", true, &[add(1, "#!/bin/sh"), add(2, "echo 1")]).await.unwrap();
            assert!(r.ok, "{r:?}");
            assert!(out(&dir, &["ls-files", "-s", "run.sh"]).starts_with("100755 "));

            // A symbolic link has no lines to pick, whether staging or discarding them; reading it
            // would follow it to the file it points at.
            std::fs::write(dir.join("target.txt"), "t1\nt2\n").unwrap();
            std::os::unix::fs::symlink("target.txt", dir.join("link")).unwrap();
            let r = apply_lines(&client, LineAction::Stage, "link", true, &[add(1, "t1")]).await;
            assert!(matches!(&r, Err(GitError::InvalidPath { reason, .. }) if reason.contains("symbolic link")), "{r:?}");
            assert_eq!(out(&dir, &["status", "--porcelain", "--", "link"]), "?? link");
            let r = apply_lines(&client, LineAction::Discard, "link", true, &[add(1, "t1")]).await;
            assert!(matches!(&r, Err(GitError::InvalidPath { reason, .. }) if reason.contains("symbolic link")), "{r:?}");
            assert!(dir.join("link").symlink_metadata().unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read_to_string(dir.join("target.txt")).unwrap(), "t1\nt2\n");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn single_lines_of_deletions_renames_blank_lines_and_textconv() {
        use super::lines::{apply_lines, ChangeKind, LineAction, SelectedLine};
        if which::which("git").is_err() { return; }
        let dir = std::env::temp_dir().join(format!("stashpanda-more-lines-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "core.autocrlf", "false"]);
        let numbered: String = (1..=8).map(|i| format!("line {i}\n")).collect();
        for name in ["staged-gone.txt", "gone.txt", "old.txt"] {
            std::fs::write(dir.join(name), &numbered).unwrap();
        }
        std::fs::write(dir.join("blank.txt"), "a\nb\n\nc\nd\n").unwrap();
        std::fs::write(dir.join(".gitattributes"), "*.dat diff=upper\n").unwrap();
        std::fs::write(dir.join("x.dat"), "a\nb\nc\nd\ne\nf\ng\nh\n").unwrap();
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let client = client_for(&dir);
        let add = |line: u32, text: &str| SelectedLine { kind: ChangeKind::Add, line, text: text.into() };
        let del = |line: u32, text: &str| SelectedLine { kind: ChangeKind::Del, line, text: text.into() };
        let index = |p: &str| out(&dir, &["show", &format!(":{p}")]);

        // Unstage one line of a staged deletion: the index gets back a file with only that line.
        git(&dir, &["rm", "-q", "staged-gone.txt"]);
        let r = apply_lines(&client, LineAction::Unstage, "staged-gone.txt", false, &[del(3, "line 3")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(index("staged-gone.txt"), "line 3");

        // Discard one line of a file deleted from disk: the file comes back with only that line.
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        let r = apply_lines(&client, LineAction::Discard, "gone.txt", false, &[del(5, "line 5")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(std::fs::read_to_string(dir.join("gone.txt")).unwrap(), "line 5\n");

        // A staged rename with two edits: unstage one, removed line included; the rename stays.
        git(&dir, &["mv", "old.txt", "new.txt"]);
        std::fs::write(dir.join("new.txt"), numbered.replace("line 2\n", "line two\n").replace("line 7\n", "line seven\n")).unwrap();
        git(&dir, &["add", "new.txt"]);
        let r = apply_lines(&client, LineAction::Unstage, "new.txt", false, &[del(2, "line 2"), add(2, "line two")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(index("new.txt"), numbered.replace("line 7\n", "line seven\n").trim_end());
        assert!(out(&dir, &["diff", "--cached", "-M", "--name-status", "--", "new.txt", "old.txt"]).starts_with('R'));
        // The rest of it: both paths are unstaged.
        let r = apply_lines(&client, LineAction::Unstage, "new.txt", false, &[del(7, "line 7"), add(7, "line seven")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(out(&dir, &["diff", "--cached", "--name-status", "--", "new.txt", "old.txt"]), "");

        // With diff.suppressBlankEmpty Git prints a blank context line as an empty line, which
        // must not end the hunk: staging the first change leaves the second unstaged.
        git(&dir, &["config", "diff.suppressBlankEmpty", "true"]);
        std::fs::write(dir.join("blank.txt"), "a\nB\n\nc\nD\n").unwrap();
        let r = apply_lines(&client, LineAction::Stage, "blank.txt", false, &[del(2, "b"), add(2, "B")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(index("blank.txt"), "a\nB\n\nc\nd");
        let r = apply_lines(&client, LineAction::Stage, "blank.txt", false, &[del(5, "d")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(index("blank.txt"), "a\nB\n\nc");

        // A textconv driver changes what `git diff` shows, not the file: the lines are the file's.
        git(&dir, &["config", "diff.upper.textconv", "tr a-z A-Z <"]);
        std::fs::write(dir.join("x.dat"), "a\nbb\nc\nd\ne\nf\ng\nhh\n").unwrap();
        let r = apply_lines(&client, LineAction::Stage, "x.dat", false, &[del(2, "b"), add(2, "bb")]).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(index("x.dat"), "a\nbb\nc\nd\ne\nf\ng\nh");
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

        // A failure that only names a branch with `index.lock` in it is not the lock.
        git(&dir, &["branch", "fix-index.lock-race"]);
        let taken = branch_rename(&client, "other", "fix-index.lock-race").await.unwrap();
        assert!(!taken.ok && index_lock_in(&taken).is_none(), "{taken:?}");
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

        // `pull.ff=false` makes a merge commit even where a fast-forward would do, as `git pull`.
        git(&local, &["reset", "-q", "--hard", "origin/main~1"]);
        git(&local, &["config", "pull.ff", "false"]);
        let no_ff = pull("origin", "main", PullMode::Merge, false).await.unwrap();
        assert!(no_ff.ok, "{no_ff:?}");
        assert_eq!(head(&local, "HEAD^2"), head(&origin, "HEAD"));
        git(&local, &["config", "--unset", "pull.ff"]);

        // The remote has a tag named like the branch, on an older commit: the branch is pulled,
        // and the tag is not rebased onto.
        git(&local, &["reset", "-q", "--hard", "origin/main"]);
        git(&origin, &["tag", "main", "HEAD~2"]);
        std::fs::write(origin.join("h.txt"), "h\n").unwrap();
        git(&origin, &["add", "-A"]); git(&origin, &["commit", "-qm", "after the tag"]);
        std::fs::write(local.join("i.txt"), "i\n").unwrap();
        git(&local, &["add", "-A"]); git(&local, &["commit", "-qm", "mine again"]);
        let tagged = pull("origin", "main", PullMode::Rebase, false).await.unwrap();
        assert!(tagged.ok, "{tagged:?}");
        assert_eq!(head(&local, "HEAD~1"), head(&origin, "refs/heads/main"));
        assert_eq!(head(&local, "refs/remotes/origin/main"), head(&origin, "refs/heads/main"));

        // `pull.rebase=merges` keeps the branch's own merge commit, as `git pull` does; a plain
        // rebase would flatten it.
        git(&local, &["config", "pull.rebase", "merges"]);
        git(&local, &["switch", "-q", "-c", "topic"]);
        std::fs::write(local.join("j.txt"), "j\n").unwrap();
        git(&local, &["add", "-A"]); git(&local, &["commit", "-qm", "topic work"]);
        git(&local, &["switch", "-q", "main"]);
        git(&local, &["merge", "-q", "--no-ff", "--no-edit", "topic"]);
        std::fs::write(origin.join("k.txt"), "k\n").unwrap();
        git(&origin, &["add", "-A"]); git(&origin, &["commit", "-qm", "remote four"]);
        let kept = pull("origin", "main", PullMode::Rebase, false).await.unwrap();
        assert!(kept.ok, "{kept:?}");
        let merges = String::from_utf8(Command::new("git").arg("-C").arg(&local).args(["log", "--merges", "--format=%s", "origin/main..HEAD"]).output().unwrap().stdout).unwrap();
        assert_eq!(merges, "Merge branch 'topic'\n");
        git(&local, &["config", "--unset", "pull.rebase"]);

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

        // A first push to a new remote branch, then the upstream set as its own step. (Cloning an
        // empty repository already makes main track origin/main, so that is undone first.)
        git(&local, &["config", "--remove-section", "branch.main"]);
        std::fs::write(local.join("a.txt"), "1\n").unwrap();
        git(&local, &["add", "-A"]); git(&local, &["commit", "-qm", "one"]);
        assert_eq!(upstream_of(&client, "main").await.unwrap(), None);
        let main = PushTarget { remote: "origin".into(), branch: "main".into() };
        let first = push(&client, "main", &main, &progress, &cancel).await.unwrap();
        assert!(first.ok, "{first:?}");
        assert_eq!(upstream_of(&client, "main").await.unwrap(), None, "the push itself leaves the config alone");
        assert!(branch_set_upstream(&client, "main", Some("refs/remotes/origin/main")).await.unwrap().ok);
        assert_eq!(upstream_of(&client, "main").await.unwrap(), Some(main.clone()));
        assert_eq!(rev(&origin, "main"), rev(&local, "HEAD"));

        // Another clone pushes; fetch brings it into origin/main, and our push is then rejected.
        git(&other, &["pull", "-q", "origin", "main"]);
        std::fs::write(other.join("b.txt"), "2\n").unwrap();
        git(&other, &["add", "-A"]); git(&other, &["commit", "-qm", "two"]); git(&other, &["push", "-q", "origin", "main"]);
        let fetched = fetch_all(&client, &progress, &cancel).await.unwrap();
        assert!(fetched.ok, "{fetched:?}");
        assert_eq!(rev(&local, "origin/main"), rev(&other, "HEAD"));

        // A branch deleted on the server goes from the remote-tracking branches here at the next fetch.
        git(&other, &["push", "-q", "origin", "HEAD:refs/heads/short-lived"]);
        assert!(fetch_all(&client, &progress, &cancel).await.unwrap().ok);
        let has_tracking = || Command::new("git").arg("-C").arg(&local).args(["rev-parse", "--verify", "-q", "refs/remotes/origin/short-lived"]).status().unwrap().success();
        assert!(has_tracking());
        git(&other, &["push", "-q", "origin", "--delete", "short-lived"]);
        assert!(fetch_all(&client, &progress, &cancel).await.unwrap().ok);
        assert!(!has_tracking(), "pruned");
        std::fs::write(local.join("c.txt"), "3\n").unwrap();
        git(&local, &["add", "-A"]); git(&local, &["commit", "-qm", "three"]);
        let rejected = push(&client, "main", &main, &progress, &cancel).await.unwrap();
        assert!(push_rejected(&rejected), "{rejected:?}");

        // A branch pushed under another name; a tag of the same name is not pushed instead.
        git(&local, &["tag", "topic"]);
        git(&local, &["branch", "topic", "HEAD~1"]);
        let topic = PushTarget { remote: "origin".into(), branch: "remote-topic".into() };
        assert!(push(&client, "topic", &topic, &progress, &cancel).await.unwrap().ok);
        assert!(branch_set_upstream(&client, "topic", Some("refs/remotes/origin/remote-topic")).await.unwrap().ok);
        assert_eq!(rev(&origin, "refs/heads/remote-topic"), rev(&local, "refs/heads/topic"));
        assert_eq!(upstream_of(&client, "topic").await.unwrap(), Some(topic.clone()));
        assert!(Command::new("git").arg("-C").arg(&origin).args(["rev-parse", "--verify", "-q", "refs/tags/topic"]).status().unwrap().code() == Some(1));

        // Pushed without choosing a target, as `git push` decides it: the upstream when it has
        // the same name, whatever push.default says. topic's upstream has another name, so
        // `simple` (the default), `matching` and `nothing` refuse, `upstream` pushes there and
        // `current` pushes to topic's own name.
        assert_eq!(implicit_push_target(&client, "main").await.unwrap(), Some(main.clone()));
        for refusing in ["simple", "matching"] {
            git(&local, &["config", "push.default", refusing]);
            assert!(matches!(implicit_push_target(&client, "topic").await, Err(GitError::InvalidPath { .. })), "{refusing}");
        }
        // `nothing` refuses even a branch whose upstream has the same name.
        git(&local, &["config", "push.default", "nothing"]);
        assert!(matches!(implicit_push_target(&client, "topic").await, Err(GitError::PushTargetRefused { .. })));
        assert!(matches!(implicit_push_target(&client, "main").await, Err(GitError::PushTargetRefused { .. })));
        assert_eq!(push_default(&client).await.unwrap().as_deref(), Some("nothing"));
        assert!(set_push_default(&client, PushDefault::Upstream).await.unwrap().ok);
        assert_eq!(out(&local, &["config", "--local", "--get", "push.default"]), "upstream");
        assert_eq!(implicit_push_target(&client, "topic").await.unwrap(), Some(topic));
        assert!(set_push_default(&client, PushDefault::Current).await.unwrap().ok);
        assert_eq!(out(&local, &["config", "--local", "--get", "push.default"]), "current");
        assert_eq!(implicit_push_target(&client, "topic").await.unwrap(), Some(PushTarget { remote: "origin".into(), branch: "topic".into() }));
        git(&local, &["config", "--unset", "push.default"]);
        assert_eq!(push_default(&client).await.unwrap(), None);
        git(&local, &["branch", "untracked"]);
        assert_eq!(implicit_push_target(&client, "untracked").await.unwrap(), None);

        // `remote.pushDefault` and `branch.<name>.pushRemote` send the push to another remote (a
        // fork), under the branch's own name, as `git push` does with push.default=simple.
        git(&local, &["remote", "add", "fork", &origin.to_string_lossy()]);
        git(&local, &["config", "remote.pushDefault", "fork"]);
        let to_fork = |branch: &str| Some(PushTarget { remote: "fork".into(), branch: branch.into() });
        assert_eq!(implicit_push_target(&client, "main").await.unwrap(), to_fork("main"));
        assert_eq!(implicit_push_target(&client, "topic").await.unwrap(), to_fork("topic"));
        assert_eq!(implicit_push_target(&client, "untracked").await.unwrap(), to_fork("untracked"));
        git(&local, &["config", "push.default", "upstream"]);
        assert!(matches!(implicit_push_target(&client, "main").await, Err(GitError::PushTargetRefused { .. })));
        git(&local, &["config", "--unset", "push.default"]);
        git(&local, &["config", "--unset", "remote.pushDefault"]);
        git(&local, &["config", "branch.main.pushRemote", "fork"]);
        assert_eq!(implicit_push_target(&client, "main").await.unwrap(), to_fork("main"));
        // topic has no pushRemote: still its upstream's remote, which push.default=simple refuses.
        assert!(matches!(implicit_push_target(&client, "topic").await, Err(GitError::InvalidPath { .. })));
        git(&local, &["config", "--unset", "branch.main.pushRemote"]);

        // `remote.<name>.push` refspecs choose the name on the remote; one that leads somewhere
        // other than a branch, or that leaves the branch out, is refused rather than pushed onto
        // the upstream.
        git(&local, &["config", "remote.origin.push", "refs/heads/main:refs/heads/for-review/main"]);
        assert_eq!(implicit_push_target(&client, "main").await.unwrap(), Some(PushTarget { remote: "origin".into(), branch: "for-review/main".into() }));
        assert!(matches!(implicit_push_target(&client, "topic").await, Err(GitError::PushTargetRefused { .. })));
        git(&local, &["config", "remote.origin.push", "HEAD:refs/for/main"]);
        assert!(matches!(implicit_push_target(&client, "main").await, Err(GitError::PushTargetRefused { .. })));
        git(&local, &["config", "--unset-all", "remote.origin.push"]);
        assert_eq!(implicit_push_target(&client, "main").await.unwrap(), Some(main.clone()));

        // An upstream whose remote is a URL, as `git push -u <url>` writes it: pushed to and
        // pulled from, as `git push` and `git pull` do.
        let url = format!("file://{}", origin.display());
        git(&local, &["push", "-q", "-u", &url, "untracked"]);
        let by_url = implicit_push_target(&client, "untracked").await.unwrap().unwrap();
        assert_eq!(by_url, PushTarget { remote: url.clone(), branch: "untracked".into() });
        let pushed = push(&client, "untracked", &by_url, &progress, &cancel).await.unwrap();
        assert!(pushed.ok, "{pushed:?}");
        let fetched = pull_fetch(&client, &url, "untracked", &progress, &cancel).await.unwrap();
        assert!(fetched.ok, "{fetched:?}");

        cancel.cancel();
        assert!(matches!(fetch_all(&client, &progress, &cancel).await, Err(GitError::Cancelled)));
        assert!(matches!(push(&client, "main", &PushTarget { remote: "-o".into(), branch: "main".into() }, &progress, &cancel).await, Err(GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn fast_forwards_a_branch_left_behind_on_the_current_lane() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-ff-{}", uuid::Uuid::new_v4().simple()));
        let dir = root.join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&dir, &["config", k, v]);
        }
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&dir, &["branch", "release"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "two"]);
        git(&dir, &["switch", "-q", "-c", "side"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "side work"]);
        git(&dir, &["switch", "-q", "main"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "three"]);
        git(&dir, &["merge", "-q", "--no-ff", "--no-edit", "side"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "four"]);
        git(&dir, &["branch", "at-head"]);
        git(&dir, &["switch", "-q", "-c", "own", "HEAD~1"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "own work"]);
        git(&dir, &["switch", "-q", "main"]);
        let client = client_for(&dir);
        let ff = |b: &'static str| fast_forwards_to_head(&client, b);

        // release is on main's first-parent line; side is behind HEAD only through the merge;
        // own has a commit of its own; at-head and main itself have nothing to move.
        assert!(ff("release").await.unwrap());
        assert!(!ff("side").await.unwrap());
        assert!(!ff("own").await.unwrap());
        assert!(!ff("at-head").await.unwrap());
        assert!(!ff("main").await.unwrap());
        assert!(!ff("missing").await.unwrap());

        // Moved with uncommitted changes, which stay; HEAD stays on main.
        std::fs::write(dir.join("dirty.txt"), "not committed\n").unwrap();
        let (release, head) = (out(&dir, &["rev-parse", "release"]), out(&dir, &["rev-parse", "HEAD"]));
        let moved = push_to_local_branch(&client, "main", "release").await.unwrap();
        assert!(moved.ok, "{moved:?}");
        assert_eq!(out(&dir, &["rev-parse", "release"]), head);
        assert_eq!(out(&dir, &["symbolic-ref", "--short", "HEAD"]), "main");
        assert_eq!(std::fs::read_to_string(dir.join("dirty.txt")).unwrap(), "not committed\n");
        assert!(!dir.join(".git/FETCH_HEAD").exists(), "a pull's FETCH_HEAD is not overwritten");

        // One that would not fast-forward is refused, unless forced.
        let own = out(&dir, &["rev-parse", "own"]);
        let not_ff = push_to_local_branch(&client, "main", "own").await.unwrap();
        assert!(!not_ff.ok && not_ff.stderr.contains("[rejected]"), "{not_ff:?}");
        assert_eq!(out(&dir, &["rev-parse", "own"]), own);
        // What forcing it drops: own's own commit, and nothing for a fast-forward.
        let dropped = commits_dropped(&client, &head, &own, 50).await.unwrap();
        assert_eq!((dropped.total, dropped.commits.len()), (1, 1));
        assert_eq!(dropped.commits[0].subject, "own work");
        assert_eq!(dropped.commits[0].author_name, "T");
        assert_eq!(commits_dropped(&client, &head, &release, 50).await.unwrap().total, 0);
        // Forced, the move is pinned at both ends: not while own has moved on from the listed
        // commit, nor once main has moved on from the commit the list was made against.
        git(&dir, &["branch", "-f", "own-moved", &own]);
        let elsewhere = out(&dir, &["rev-parse", "own-moved~1"]);
        let stale_target = force_move_local_branch(&client, "main", &head, "own", &elsewhere).await.unwrap();
        assert!(!stale_target.ok, "{stale_target:?}");
        assert_eq!(out(&dir, &["rev-parse", "own"]), own);
        let stale_source = force_move_local_branch(&client, "main", &own, "own", &own).await.unwrap();
        assert!(!stale_source.ok && stale_source.stderr.contains("source moved"), "{stale_source:?}");
        let forced = force_move_local_branch(&client, "main", &head, "own", &own).await.unwrap();
        assert!(forced.ok, "{forced:?}");
        assert_eq!(out(&dir, &["rev-parse", "own"]), head);
        // A new branch is created; a branch into itself is refused.
        assert!(push_to_local_branch(&client, "main", "brand-new").await.unwrap().ok);
        assert_eq!(out(&dir, &["rev-parse", "brand-new"]), head);
        assert!(matches!(push_to_local_branch(&client, "main", "main").await, Err(GitError::InvalidPath { .. })));

        // The checked-out branch, and one checked out in another worktree, are refused even forced.
        let into_head = push_to_local_branch(&client, "release", "main").await.unwrap();
        assert!(!into_head.ok && into_head.stderr.contains("checked out"), "{into_head:?}");
        let release_now = out(&dir, &["rev-parse", "release"]);
        let forced_into_head = force_move_local_branch(&client, "release", &release_now, "main", &head).await.unwrap();
        assert!(!forced_into_head.ok && forced_into_head.stderr.contains("checked out"), "{forced_into_head:?}");
        git(&dir, &["branch", "-f", "release", &release]);
        git(&dir, &["worktree", "add", "-q", &root.join("wt").to_string_lossy(), "release"]);
        let in_worktree = force_move_local_branch(&client, "main", &head, "release", &release).await.unwrap();
        assert!(!in_worktree.ok && in_worktree.stderr.contains("checked out"), "{in_worktree:?}");
        assert_eq!(out(&dir, &["rev-parse", "release"]), release);

        // A branch being rebased in a worktree, where HEAD is detached, is refused too, forced or
        // not, and so is one a bisect started from: moving it would leave them unable to finish.
        git(&dir, &["branch", "busy", "HEAD~1"]);
        let wt2 = root.join("wt2");
        git(&dir, &["worktree", "add", "-q", &wt2.to_string_lossy(), "busy"]);
        git(&wt2, &["commit", "-q", "--allow-empty", "-m", "busy work"]);
        let busy = out(&dir, &["rev-parse", "busy"]);
        assert!(!Command::new("git").arg("-C").arg(&wt2).args(["-c", "user.email=t@t", "-c", "user.name=T", "rebase", "-q", "--exec", "false", "main"]).status().unwrap().success());
        assert_eq!(branch_in_use(&client, "busy").await.unwrap().as_deref(), Some("a rebase of it is in progress"));
        let rebasing = force_move_local_branch(&client, "main", &head, "busy", &busy).await.unwrap();
        assert!(!rebasing.ok && rebasing.stderr.contains("rebase"), "{rebasing:?}");
        assert!(!push_to_local_branch(&client, "main", "busy").await.unwrap().ok);
        assert_eq!(out(&dir, &["rev-parse", "busy"]), busy);
        git(&wt2, &["rebase", "--abort"]);
        // A branch the rebase will move at the end, with rebase.updateRefs.
        git(&dir, &["branch", "-f", "brand-new", "busy"]);
        assert!(!Command::new("git").arg("-C").arg(&wt2).args(["-c", "user.email=t@t", "-c", "user.name=T", "-c", "rebase.updateRefs=true", "rebase", "-q", "--exec", "false", "main"]).status().unwrap().success());
        assert_eq!(branch_in_use(&client, "brand-new").await.unwrap().as_deref(), Some("a rebase in progress will move it when it finishes"));
        git(&wt2, &["rebase", "--abort"]);
        git(&dir, &["branch", "-f", "brand-new", &head]);
        git(&wt2, &["bisect", "start"]);
        git(&wt2, &["switch", "-q", "--detach", "HEAD~1"]);
        assert_eq!(branch_in_use(&client, "busy").await.unwrap().as_deref(), Some("a bisect started from it is in progress"));
        git(&wt2, &["bisect", "reset"]);
        assert_eq!(branch_in_use(&client, "busy").await.unwrap().as_deref(), Some("it is checked out at a worktree"));
        assert_eq!(branch_in_use(&client, "brand-new").await.unwrap(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn merges_with_uncommitted_changes_stashed_around_it() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-merge-stash-{}", uuid::Uuid::new_v4().simple()));
        let dir = root.join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&dir, &["config", k, v]);
        }
        for f in ["a.txt", "c.txt", "d.txt"] {
            std::fs::write(dir.join(f), "base\n").unwrap();
        }
        git(&dir, &["add", "-A"]); git(&dir, &["commit", "-qm", "base"]);
        let commit_on = |branch: &str, file: &str, text: &str| {
            git(&dir, &["switch", "-q", branch]);
            std::fs::write(dir.join(file), text).unwrap();
            git(&dir, &["commit", "-qam", &format!("{branch} changes {file}")]);
        };
        git(&dir, &["branch", "topic"]); git(&dir, &["branch", "clash"]); git(&dir, &["branch", "clash2"]);
        commit_on("topic", "a.txt", "topic\n");
        commit_on("clash", "c.txt", "clash\n");
        commit_on("clash2", "c.txt", "clash two\n");
        commit_on("main", "c.txt", "main\n");
        let client = client_for(&dir);
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();

        // A merge that completes puts the uncommitted change straight back.
        std::fs::write(dir.join("d.txt"), "uncommitted\n").unwrap();
        let clean = merge(&client, "topic", false, true).await.unwrap();
        assert!(clean.ok, "{clean:?}");
        assert_eq!((read("a.txt"), read("d.txt")), ("topic\n".into(), "uncommitted\n".into()));

        // One that stops on conflicts keeps it aside until the merge is concluded.
        let stopped = merge(&client, "clash", false, true).await.unwrap();
        assert!(stopped.conflicts, "{stopped:?}");
        assert_eq!(read("d.txt"), "base\n");
        std::fs::write(dir.join("c.txt"), "resolved\n").unwrap();
        git(&dir, &["add", "c.txt"]);
        let continued = continue_operation(&client, InProgressKind::Merge).await.unwrap();
        assert!(continued.ok, "{continued:?}");
        assert_eq!(read("d.txt"), "uncommitted\n");

        // Or aborted.
        assert!(merge(&client, "clash2", false, true).await.unwrap().conflicts);
        assert_eq!(read("d.txt"), "base\n");
        assert!(merge_abort(&client).await.unwrap().ok);
        assert_eq!(read("d.txt"), "uncommitted\n");

        // A rebase that stops on conflicts keeps the change aside too, and puts it back when the
        // rebase is continued to the end, or aborted.
        git(&dir, &["switch", "-q", "clash2"]);
        std::fs::write(dir.join("d.txt"), "uncommitted on clash2\n").unwrap();
        assert!(rebase(&client, "main", true).await.unwrap().conflicts);
        assert_eq!(read("d.txt"), "base\n");
        assert!(rebase_abort(&client).await.unwrap().ok);
        assert_eq!(read("d.txt"), "uncommitted on clash2\n");
        assert!(rebase(&client, "main", true).await.unwrap().conflicts);
        std::fs::write(dir.join("c.txt"), "resolved again\n").unwrap();
        git(&dir, &["add", "c.txt"]);
        let finished = continue_operation(&client, InProgressKind::Rebase).await.unwrap();
        assert!(finished.ok, "{finished:?}");
        assert_eq!(read("d.txt"), "uncommitted on clash2\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn fast_forwards_a_branch_to_its_upstream_as_last_fetched() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-ff-up-{}", uuid::Uuid::new_v4().simple()));
        let (origin, local, other) = (root.join("origin.git"), root.join("local"), root.join("other"));
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&std::ffi::OsStr]| assert!(Command::new("git").args(args).status().unwrap().success());
        run(&["init".as_ref(), "-q".as_ref(), "--bare".as_ref(), "-b".as_ref(), "main".as_ref(), origin.as_os_str()]);
        run(&["clone".as_ref(), "-q".as_ref(), origin.as_os_str(), local.as_os_str()]);
        for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&local, &["config", k, v]);
        }
        std::fs::write(local.join("a.txt"), "1\n").unwrap();
        git(&local, &["add", "-A"]); git(&local, &["commit", "-qm", "one"]);
        git(&local, &["push", "-q", "origin", "main", "main:refs/heads/feature"]);
        git(&local, &["fetch", "-q"]);
        git(&local, &["branch", "feature", "--track", "origin/feature"]);
        run(&["clone".as_ref(), "-q".as_ref(), origin.as_os_str(), other.as_os_str()]);
        std::fs::write(other.join("a.txt"), "1\n2\n").unwrap();
        git(&other, &["-c", "user.email=o@o", "-c", "user.name=O", "commit", "-qam", "two"]);
        git(&other, &["push", "-q", "origin", "HEAD:main", "HEAD:feature"]);
        let client = client_for(&local);
        let rev = |r: &str| out(&local, &["rev-parse", r]);

        // Nothing moves until the upstream has been fetched: no network access here.
        assert!(fast_forward_to_upstream(&client, "main", false).await.unwrap().ok);
        assert_ne!(rev("main"), out(&other, &["rev-parse", "HEAD"]));
        git(&local, &["fetch", "-q"]);

        // A branch that is not checked out moves without touching the files.
        std::fs::write(local.join("b.txt"), "not committed\n").unwrap();
        let moved = fast_forward_to_upstream(&client, "feature", false).await.unwrap();
        assert!(moved.ok, "{moved:?}");
        assert_eq!(rev("feature"), rev("origin/feature"));
        assert_eq!(std::fs::read_to_string(local.join("a.txt")).unwrap(), "1\n");

        // The checked-out branch moves with its files; uncommitted changes go aside and come back.
        let checked_out = fast_forward_to_upstream(&client, "main", true).await.unwrap();
        assert!(checked_out.ok, "{checked_out:?}");
        assert_eq!(rev("main"), rev("origin/main"));
        assert_eq!(std::fs::read_to_string(local.join("a.txt")).unwrap(), "1\n2\n");
        assert_eq!(std::fs::read_to_string(local.join("b.txt")).unwrap(), "not committed\n");

        // Changes that clash with the fast-forward are kept in a stash, and Git still exits 0.
        std::fs::write(other.join("a.txt"), "1\n2\n3\n").unwrap();
        git(&other, &["-c", "user.email=o@o", "-c", "user.name=O", "commit", "-qam", "three"]);
        git(&other, &["push", "-q", "origin", "HEAD:main"]);
        git(&local, &["fetch", "-q"]);
        std::fs::write(local.join("a.txt"), "mine\n").unwrap();
        let clashed = fast_forward_to_upstream(&client, "main", true).await.unwrap();
        assert!(clashed.ok && format!("{}{}", clashed.stdout, clashed.stderr).contains("Applying autostash resulted in conflicts"), "{clashed:?}");
        assert_eq!(rev("main"), rev("origin/main"));
        assert_eq!(crate::git::queries::log::get_stashes(&client).await.unwrap().len(), 1);
        git(&local, &["reset", "-q", "--hard"]);

        // Not a fast-forward any more: refused, nothing moves.
        git(&local, &["switch", "-q", "feature"]);
        git(&local, &["commit", "-q", "--allow-empty", "-m", "mine on feature"]);
        git(&local, &["switch", "-q", "main"]);
        git(&other, &["push", "-q", "origin", "HEAD:feature"]);
        git(&local, &["fetch", "-q"]);
        let before = rev("feature");
        assert!(!fast_forward_to_upstream(&client, "feature", false).await.unwrap().ok);
        assert_eq!(rev("feature"), before);

        // A branch another worktree has checked out is refused; so is one that tracks nothing.
        git(&local, &["branch", "-f", "feature", "origin/feature~1"]);
        git(&local, &["worktree", "add", "-q", &root.join("wt").to_string_lossy(), "feature"]);
        let in_use = fast_forward_to_upstream(&client, "feature", false).await.unwrap();
        assert!(!in_use.ok && in_use.stderr.contains("refusing to move"), "{in_use:?}");
        git(&local, &["branch", "untracked"]);
        assert!(matches!(fast_forward_to_upstream(&client, "untracked", false).await, Err(GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn tells_rewritten_history_from_someone_else_pushing() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-rewrote-{}", uuid::Uuid::new_v4().simple()));
        let (origin, local, other) = (root.join("origin.git"), root.join("local"), root.join("other"));
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&std::ffi::OsStr]| assert!(Command::new("git").args(args).status().unwrap().success());
        run(&["init".as_ref(), "-q".as_ref(), "--bare".as_ref(), "-b".as_ref(), "main".as_ref(), origin.as_os_str()]);
        run(&["clone".as_ref(), "-q".as_ref(), origin.as_os_str(), local.as_os_str()]);
        for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&local, &["config", k, v]);
        }
        git(&local, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&local, &["push", "-q", "origin", "main"]);
        // my-work tracks shared but is pushed to its own name (push.default=current).
        git(&local, &["push", "-q", "origin", "main:refs/heads/shared"]);
        git(&local, &["fetch", "-q"]);
        git(&local, &["switch", "-q", "-c", "my-work", "--track", "origin/shared"]);
        git(&local, &["config", "push.default", "current"]);
        git(&local, &["commit", "-q", "--allow-empty", "-m", "mine"]);
        git(&local, &["push", "-q", "origin", "my-work"]);
        let client = client_for(&local);
        let my_work = PushTarget { remote: "origin".into(), branch: "my-work".into() };
        assert_eq!(implicit_push_target(&client, "my-work").await.unwrap(), Some(my_work.clone()));
        assert_eq!(tracking_ref_of(&client, &my_work).await.unwrap().as_deref(), Some("refs/remotes/origin/my-work"));
        // HEAD is on origin/my-work, though it is ahead of its upstream origin/shared.
        assert!(head_on_push_target(&client, &my_work).await.unwrap());
        assert!(!rewrote_pushed_commits(&client, "my-work", &my_work).await.unwrap());

        // An amend rewrites the pushed commit.
        git(&local, &["commit", "-q", "--amend", "--allow-empty", "-m", "mine, amended"]);
        assert!(!head_on_push_target(&client, &my_work).await.unwrap());
        assert!(rewrote_pushed_commits(&client, "my-work", &my_work).await.unwrap());

        // A rebase onto new commits on shared does too.
        git(&local, &["reset", "-q", "--hard", "origin/my-work"]);
        run(&["clone".as_ref(), "-q".as_ref(), origin.as_os_str(), other.as_os_str()]);
        git(&other, &["-c", "user.email=o@o", "-c", "user.name=O", "commit", "-q", "--allow-empty", "-m", "theirs on shared"]);
        git(&other, &["push", "-q", "origin", "HEAD:refs/heads/shared"]);
        git(&local, &["fetch", "-q"]);
        git(&local, &["rebase", "-q", "origin/shared"]);
        assert!(rewrote_pushed_commits(&client, "my-work", &my_work).await.unwrap());

        // Someone else pushing to my-work is not a rewrite here.
        git(&local, &["reset", "-q", "--hard", "origin/my-work"]);
        git(&other, &["fetch", "-q"]);
        git(&other, &["switch", "-q", "-c", "my-work", "origin/my-work"]);
        git(&other, &["-c", "user.email=o@o", "-c", "user.name=O", "commit", "-q", "--allow-empty", "-m", "theirs on my-work"]);
        git(&other, &["push", "-q", "origin", "my-work"]);
        git(&local, &["fetch", "-q"]);
        assert!(!rewrote_pushed_commits(&client, "my-work", &my_work).await.unwrap());

        // A force push replaces the remote's my-work only while it is where this repository last
        // fetched it: here it is, so the amended commit replaces it.
        let (progress, cancel) = (|_: String| {}, tokio_util::sync::CancellationToken::new());
        git(&local, &["reset", "-q", "--hard", "origin/my-work"]);
        git(&local, &["commit", "-q", "--amend", "--allow-empty", "-m", "replaces theirs"]);
        let shown = out(&local, &["rev-parse", "origin/my-work"]);
        let mine = out(&local, &["rev-parse", "my-work"]);
        let forced = force_push(&client, "my-work", &mine, &my_work, &shown, &progress, &cancel).await.unwrap();
        assert!(forced.ok, "{forced:?}");
        assert_eq!(out(&origin, &["rev-parse", "refs/heads/my-work"]), out(&local, &["rev-parse", "my-work"]));
        // Someone pushes after the last fetch: the lease refuses, and their commit stays.
        git(&other, &["fetch", "-q"]);
        git(&other, &["reset", "-q", "--hard", "origin/my-work"]);
        git(&other, &["-c", "user.email=o@o", "-c", "user.name=O", "commit", "-q", "--allow-empty", "-m", "unseen"]);
        git(&other, &["push", "-q", "origin", "my-work"]);
        let unseen = out(&other, &["rev-parse", "my-work"]);
        git(&local, &["commit", "-q", "--amend", "--allow-empty", "-m", "replaces again"]);
        let shown = out(&local, &["rev-parse", "my-work"]);
        let mine = out(&local, &["rev-parse", "my-work"]);
        let stale = force_push(&client, "my-work", &mine, &my_work, &shown, &progress, &cancel).await.unwrap();
        assert!(push_stale(&stale), "{stale:?}");
        assert_eq!(out(&origin, &["rev-parse", "refs/heads/my-work"]), unseen);
        // An automatic fetch brings the commit into origin/my-work, but the user was shown the
        // older tip: the lease is still on that, so it still refuses.
        git(&local, &["fetch", "-q"]);
        let after_fetch = force_push(&client, "my-work", &mine, &my_work, &shown, &progress, &cancel).await.unwrap();
        assert!(push_stale(&after_fetch), "{after_fetch:?}");
        assert_eq!(out(&origin, &["rev-parse", "refs/heads/my-work"]), unseen);
        // The branch moved after the list was made: nothing is sent.
        let now = out(&origin, &["rev-parse", "refs/heads/my-work"]);
        git(&local, &["reset", "-q", "--hard", "HEAD~1"]);
        let source_moved = force_push(&client, "my-work", &mine, &my_work, &now, &progress, &cancel).await.unwrap();
        assert!(!source_moved.ok && source_moved.stderr.contains("source moved"), "{source_moved:?}");
        assert_eq!(out(&origin, &["rev-parse", "refs/heads/my-work"]), now);

        // A target this repository has no remote-tracking branch for.
        let nowhere = PushTarget { remote: "origin".into(), branch: "never-pushed".into() };
        assert_eq!(tracking_ref_of(&client, &nowhere).await.unwrap(), None);
        assert!(!head_on_push_target(&client, &nowhere).await.unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_locked_config_leaves_push_and_checkout_whole_and_fails_only_the_upstream() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-config-lock-{}", uuid::Uuid::new_v4().simple()));
        let (origin, local) = (root.join("origin.git"), root.join("local"));
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&std::ffi::OsStr]| assert!(Command::new("git").args(args).status().unwrap().success());
        run(&["init".as_ref(), "-q".as_ref(), "--bare".as_ref(), "-b".as_ref(), "main".as_ref(), origin.as_os_str()]);
        run(&["clone".as_ref(), "-q".as_ref(), origin.as_os_str(), local.as_os_str()]);
        git(&local, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&local, &["push", "-q", "origin", "main", "main:refs/heads/feature/x"]);
        git(&local, &["fetch", "-q"]);
        git(&local, &["checkout", "-q", "-b", "hotfix/before-email-confirmation"]);
        let client = client_for(&local);
        let progress = |_: String| {};
        let cancel = tokio_util::sync::CancellationToken::new();
        // Another Git process is writing the config, or one died and left its lock behind.
        let lock = local.join(".git/config.lock");
        std::fs::write(&lock, "").unwrap();

        // The push itself does not need the config, so it goes through whole.
        let target = PushTarget { remote: "origin".into(), branch: "hotfix/before-email-confirmation".into() };
        let pushed = push(&client, "hotfix/before-email-confirmation", &target, &progress, &cancel).await.unwrap();
        assert!(pushed.ok && !pushed.stderr.contains("could not lock config"), "{pushed:?}");
        assert!(out(&local, &["ls-remote", "--heads", "origin"]).contains("refs/heads/hotfix/before-email-confirmation"));
        // Setting the upstream fails, recognisably, and can be run again once the lock is gone.
        let upstream = "refs/remotes/origin/hotfix/before-email-confirmation";
        let blocked = branch_set_upstream(&client, "hotfix/before-email-confirmation", Some(upstream)).await.unwrap();
        assert!(matches!(lock_in(&blocked), Some(GitError::ConfigLocked { lock_path }) if lock_path == ".git/config.lock"), "{blocked:?}");
        assert!(index_lock_in(&blocked).is_none());

        // A local branch for a remote branch is created and checked out without the config.
        let created = branch_create(&client, "x", "refs/remotes/origin/feature/x", true).await.unwrap();
        assert!(created.ok, "{created:?}");
        assert_eq!(out(&local, &["branch", "--show-current"]), "x");

        std::fs::remove_file(&lock).unwrap();
        assert!(branch_set_upstream(&client, "hotfix/before-email-confirmation", Some(upstream)).await.unwrap().ok);
        assert_eq!(out(&local, &["rev-parse", "--abbrev-ref", "hotfix/before-email-confirmation@{upstream}"]), "origin/hotfix/before-email-confirmation");
        assert_eq!(upstream_of(&client, "x").await.unwrap(), None, "created with --no-track");
        assert!(branch_set_upstream(&client, "x", Some("refs/remotes/origin/feature/x")).await.unwrap().ok);
        assert_eq!(out(&local, &["rev-parse", "--abbrev-ref", "x@{upstream}"]), "origin/feature/x");
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

        // Someone pushed to feature/y after this repository last fetched it: the commit this
        // repository has never seen keeps the branch from being deleted.
        git(&local, &["push", "-q", "origin", "main:refs/heads/feature/y"]);
        let seen = out(&local, &["rev-parse", "refs/remotes/origin/feature/y"]);
        git(&local, &["commit", "-q", "--allow-empty", "-m", "unseen"]);
        git(&local, &["push", "-q", "origin", "HEAD:refs/heads/feature/y"]);
        git(&local, &["update-ref", "refs/remotes/origin/feature/y", &seen]);
        let moved = delete_remote_branch(&client, "origin", "feature/y", &progress, &cancel).await.unwrap();
        assert!(push_stale(&moved), "{moved:?}");
        assert!(ls_remote().contains("refs/heads/feature/y"));
        // Once fetched, it can be deleted.
        git(&local, &["fetch", "-q"]);
        assert!(delete_remote_branch(&client, "origin", "feature/y", &progress, &cancel).await.unwrap().ok);
        assert!(!ls_remote().contains("refs/heads/feature/y"));
        // A remote branch this repository has no remote-tracking branch for is refused.
        assert!(matches!(delete_remote_branch(&client, "origin", "never-fetched", &progress, &cancel).await, Err(GitError::InvalidPath { .. })));

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
    fn push_refspecs_map_a_branch_as_git_push_does() {
        let map = |specs: &[&str], branch: &str| push_ref_for(specs.iter().copied(), branch, Some("main"));
        assert_eq!(map(&["refs/heads/main:refs/heads/for-review/main"], "main").as_deref(), Some("refs/heads/for-review/main"));
        assert_eq!(map(&["main:review/main"], "main").as_deref(), Some("refs/heads/review/main"));
        assert_eq!(map(&["+refs/heads/*:refs/heads/mine/*"], "feature/x").as_deref(), Some("refs/heads/mine/feature/x"));
        assert_eq!(map(&["HEAD:refs/for/main"], "main").as_deref(), Some("refs/for/main"));
        assert_eq!(map(&["HEAD:refs/for/main"], "topic"), None, "HEAD is the checked-out branch only");
        assert_eq!(map(&["refs/heads/main"], "main").as_deref(), Some("refs/heads/main"));
        assert_eq!(map(&[":"], "topic").as_deref(), Some("refs/heads/topic"));
        assert_eq!(map(&["^refs/heads/main", ":refs/heads/gone", "refs/heads/dev:refs/heads/dev"], "main"), None);
        // The first refspec that matches wins.
        assert_eq!(map(&["refs/heads/*:refs/heads/a/*", "main:b"], "main").as_deref(), Some("refs/heads/a/main"));
    }

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
        // `pull.ff=only` wins over rebasing set in config, as `git pull` refuses to rebase then.
        assert_eq!(mode(b"pull.rebase\ntrue\0pull.ff\nonly\0", None), Some(PullMode::FastForwardOnly));
        assert_eq!(mode(b"pull.ff\nonly\0branch.dev.rebase\ntrue\0", Some("dev")), Some(PullMode::FastForwardOnly));
        // The branch's own setting comes first; a key with no value means true.
        assert_eq!(mode(b"pull.rebase\ntrue\0branch.Dev.x.rebase\nfalse\0", Some("Dev.x")), Some(PullMode::Merge));
        assert_eq!(mode(b"branch.dev.rebase\0", Some("dev")), Some(PullMode::Rebase));
        // The last value for a key wins, as in Git (system, then global, then local config).
        assert_eq!(mode(b"pull.rebase\ntrue\0pull.rebase\nfalse\0", None), Some(PullMode::Merge));
        assert_eq!(regex_escape("feat.x+1"), r"feat\.x\+1");
    }

    #[test]
    fn refuses_a_path_whose_name_was_not_utf8() {
        // How status reports `caf\xe9.txt`: the name no longer names the file, so no action takes it.
        let name = crate::git::parse::lossy(b"caf\xe9.txt");
        assert!(matches!(valid_paths(&["ok.txt".into(), name]), Err(GitError::InvalidPath { .. })));
        assert!(valid_paths(&["café.txt".into()]).is_ok());
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
