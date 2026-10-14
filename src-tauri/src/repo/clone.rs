//! Clone a remote repository into a folder, including a folder that already has files in it.
//!
//! `git clone` refuses a folder that is not empty. For one with files in it, the clone is done by
//! hand in that folder: `git init`, `git remote add origin`, `git fetch`, then the remote's
//! default branch is made the current branch with `git reset`, which sets the index and leaves
//! every file in the folder as it is. Only the repository files the folder does not have are then
//! written out. The folder's own files are never moved or overwritten, so a failed or stopped
//! clone cannot lose them: the `.git` folder it made is deleted and the folder is as it was. A
//! file the folder already had shows as modified wherever it differs from the cloned one, and as
//! untracked where the repository has no such file.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use specta::Type;
use tokio_util::sync::CancellationToken;

use crate::error::{classify_git_failure, GitError, Result};
use crate::git::{GitClient, GitReq};
use crate::transport::paths::NativePath;
use crate::transport::{Output, SharedTransport};

/// What is at the folder a clone would go into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CloneFolder {
    /// Nothing there yet: `git clone` creates the folder.
    Missing,
    Empty,
    /// The folder has `count` entries at its top level. They are kept, over the cloned files.
    HasFiles { count: u32 },
    /// The folder has a `.git` entry, so it is a repository already and cannot be cloned into.
    Repository,
}

/// What `clone_into` did, beyond creating the repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CloneOutcome {
    /// The folder had files in it, and they were kept over the cloned files.
    pub kept_existing: bool,
    /// The branch checked out: the remote's default branch. None when the remote has no branches.
    pub branch: Option<String>,
    /// Repository files that were not written because one of the folder's own files is where a
    /// folder of theirs would go (a file `docs` where the repository has `docs/index.md`). They
    /// show as deleted.
    pub blocked: Vec<String>,
    /// Problems after the repository was created, which leave it usable.
    pub warnings: Vec<String>,
}

/// `name` inside folder `dir`, with the separator `dir` already uses.
pub fn join_native(dir: &NativePath, name: &str) -> NativePath {
    let base = dir.0.trim_end_matches(['/', '\\']);
    let sep = if base.contains('\\') && !base.contains('/') { '\\' } else { '/' };
    NativePath(format!("{base}{sep}{name}"))
}

/// The folder that contains `path`, or None for a root such as `/` or `C:\`.
pub fn parent_native(path: &NativePath) -> Option<NativePath> {
    let trimmed = path.0.trim_end_matches(['/', '\\']);
    let i = trimmed.rfind(['/', '\\'])?;
    let parent = &trimmed[..i];
    let sep = &trimmed[i..=i];
    // `/foo` and `C:\foo` keep the separator that makes their parent a root rather than a
    // relative path or a bare drive.
    if parent.is_empty() || parent.ends_with(':') {
        Some(NativePath(format!("{parent}{sep}")))
    } else {
        Some(NativePath(parent.to_string()))
    }
}

/// The name a repository's folder usually gets: the last part of its URL without `.git`.
/// `git@github.com:django/django.git` gives `django`.
pub fn name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(['/', '\\']);
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed).trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next()?;
    (!last.is_empty()).then(|| last.to_string())
}

/// Refuse a URL Git could read as an option, and one with control characters.
pub fn check_url(url: &str) -> Result<()> {
    let bad = |reason: &str| Err(GitError::InvalidPath { path: url.to_string(), reason: reason.into() });
    if url.is_empty() {
        return bad("enter the URL of the repository to clone");
    }
    if url.starts_with('-') {
        return bad("a repository URL cannot start with '-'");
    }
    if url.chars().any(char::is_control) {
        return bad("the URL contains control characters");
    }
    Ok(())
}

/// What is at `path`. Fails when `path` is a file, or when the folder that would contain a
/// missing `path` does not exist either.
pub async fn inspect(transport: &SharedTransport, path: &NativePath) -> Result<CloneFolder> {
    match transport.list_dir(path).await? {
        Some(names) if names.iter().any(|n| n == ".git") => Ok(CloneFolder::Repository),
        Some(names) if names.is_empty() => Ok(CloneFolder::Empty),
        Some(names) => Ok(CloneFolder::HasFiles { count: u32::try_from(names.len()).unwrap_or(u32::MAX) }),
        None => {
            if let Some(parent) = parent_native(path) {
                if transport.list_dir(&parent).await?.is_none() {
                    return Err(GitError::InvalidPath {
                        path: path.0.clone(),
                        reason: format!("the folder that would contain it, {}, does not exist", parent.0),
                    });
                }
            }
            Ok(CloneFolder::Missing)
        }
    }
}

/// Lets a step say from where on Stop no longer applies. The step that writes the working tree
/// out runs to the end, so a stop cannot leave half of it written.
pub trait StopControl: Sync {
    fn set_cancellable(&self, cancellable: bool);
}

impl StopControl for crate::repo::OperationGuard<'_> {
    fn set_cancellable(&self, cancellable: bool) {
        crate::repo::OperationGuard::set_cancellable(self, cancellable);
    }
}

/// Clone `url` into `client.repo`, which `inspect` found to be `folder`. `client` runs in the
/// target folder; `outer` runs in the folder that contains it, for `git clone`, which creates the
/// target. On failure or after Stop, what the clone created is deleted again: the whole target
/// when it did not exist, else its `.git` folder.
pub async fn clone_into(
    client: &GitClient,
    outer: &GitClient,
    folder: &CloneFolder,
    url: &str,
    progress: &(dyn Fn(String) + Send + Sync),
    stop: &dyn StopControl,
    cancel: &CancellationToken,
) -> Result<CloneOutcome> {
    check_url(url)?;
    let target = &client.repo;
    let result = match folder {
        CloneFolder::Repository => {
            return Err(GitError::InvalidPath {
                path: target.0.clone(),
                reason: "it is a Git repository already; add it with Add Existing instead".into(),
            })
        }
        CloneFolder::Missing | CloneFolder::Empty => plain_clone(client, outer, url, progress, stop, cancel).await,
        CloneFolder::HasFiles { .. } => overlay_clone(client, url, progress, stop, cancel).await,
    };
    if result.is_err() {
        let leftover = match folder {
            CloneFolder::Missing => target.clone(),
            _ => join_native(target, ".git"),
        };
        if let Err(e) = client.transport.remove_dir_all(&leftover).await {
            tracing::warn!("could not remove {} after a failed clone: {e}", leftover.0);
        }
    }
    result
}

/// A failed Git command as an error, with what Git said.
fn failure(client: &GitClient, label: &str, out: &Output) -> GitError {
    classify_git_failure(label, Vec::new(), out.exit_code, &out.stderr, &client.transport.kind())
}

async fn plain_clone(
    client: &GitClient,
    outer: &GitClient,
    url: &str,
    progress: &(dyn Fn(String) + Send + Sync),
    stop: &dyn StopControl,
    cancel: &CancellationToken,
) -> Result<CloneOutcome> {
    // Stop applies until Git starts writing the files out.
    let report = |line: String| {
        if line.starts_with("Updating files") {
            stop.set_cancellable(false);
        }
        progress(line);
    };
    let req = GitReq::new("clone", ["clone".to_string(), "--progress".into(), "--".into(), url.to_string(), client.repo.0.clone()])
        .user()
        .timeout(None)
        .own_session();
    let out = outer.exec_streaming(req, &report, cancel).await?;
    stop.set_cancellable(false);
    if !out.success() {
        return Err(failure(outer, "clone", &out));
    }
    let branch = client.exec_allow_failure(GitReq::new("symbolic-ref", ["symbolic-ref", "-q", "--short", "HEAD"])).await?;
    Ok(CloneOutcome {
        branch: branch.success().then(|| String::from_utf8_lossy(&branch.stdout).trim().to_string()).filter(|b| !b.is_empty()),
        ..Default::default()
    })
}

async fn overlay_clone(
    client: &GitClient,
    url: &str,
    progress: &(dyn Fn(String) + Send + Sync),
    stop: &dyn StopControl,
    cancel: &CancellationToken,
) -> Result<CloneOutcome> {
    progress("The folder has files in it: cloning around them…".into());
    client.exec(GitReq::new("init", ["init", "-q"]).user()).await?;
    client.exec(GitReq::new("remote", ["remote".to_string(), "add".into(), "origin".into(), "--".into(), url.to_string()]).user()).await?;
    let fetch = GitReq::new("fetch", ["fetch", "--progress", "origin"]).user().timeout(None).own_session();
    let out = client.exec_streaming(fetch, progress, cancel).await?;
    if !out.success() {
        return Err(failure(client, "fetch", &out));
    }
    let branch = default_branch(client, progress, cancel).await?;
    // From here on the steps are local and quick, and the last one writes files out.
    stop.set_cancellable(false);
    if cancel.is_cancelled() {
        return Err(GitError::Cancelled);
    }
    let mut outcome = CloneOutcome { kept_existing: true, ..Default::default() };
    let Some(branch) = branch else {
        // An empty remote: the repository stays on its unborn branch, as `git clone` leaves it.
        outcome.warnings.push("The remote repository has no branches yet, so nothing was checked out.".into());
        return Ok(outcome);
    };
    progress(format!("Checking out {branch} around the existing files…"));
    let tracking = format!("refs/remotes/origin/{branch}");
    client.exec(GitReq::new("branch", ["branch".to_string(), "--track".into(), branch.clone(), tracking]).user()).await?;
    client.exec(GitReq::new("symbolic-ref", ["symbolic-ref".to_string(), "HEAD".into(), format!("refs/heads/{branch}")]).user()).await?;
    // A mixed reset: the index becomes the branch's tree and no file in the folder changes.
    client.exec(GitReq::new("reset", ["reset", "-q"]).user().timeout(None)).await?;
    outcome.branch = Some(branch);

    let (write, blocked) = files_to_write(client).await?;
    outcome.blocked = blocked;
    if !write.is_empty() {
        let mut stdin = Vec::new();
        for path in &write {
            stdin.extend_from_slice(path.as_bytes());
            stdin.push(0);
        }
        // Without `--force`, so it never overwrites a file, even one that appeared meanwhile.
        let req = GitReq::new("checkout-index", ["checkout-index", "-z", "--stdin"]).user().stdin(stdin).timeout(None);
        let out = client.exec_allow_failure(req).await?;
        if !out.success() {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            outcome.warnings.push(format!("Some of the repository's files could not be written: {detail}"));
        }
    }
    Ok(outcome)
}

/// The remote's default branch after `git fetch origin`: what `refs/remotes/origin/HEAD` points
/// at (Git 2.48 and later create it while fetching; older ones need `git remote set-head`). When
/// the remote does not say, its only branch, else `main`, else `master`, else the first. None when
/// the remote has no branches.
async fn default_branch(client: &GitClient, progress: &(dyn Fn(String) + Send + Sync), cancel: &CancellationToken) -> Result<Option<String>> {
    async fn origin_head(client: &GitClient) -> Result<Option<String>> {
        let out = client.exec_allow_failure(GitReq::new("symbolic-ref", ["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"])).await?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(out.success().then(|| text.strip_prefix("origin/").map(String::from)).flatten().filter(|b| !b.is_empty()))
    }
    if let Some(b) = origin_head(client).await? {
        return Ok(Some(b));
    }
    let out = client.exec_allow_failure(GitReq::new("for-each-ref", ["for-each-ref", "--format=%(refname:strip=3)", "refs/remotes/origin"])).await?;
    let branches: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|b| !b.is_empty() && *b != "HEAD").map(String::from).collect();
    if branches.len() <= 1 {
        return Ok(branches.into_iter().next());
    }
    // Ask the remote which branch its HEAD is. A failure here only means guessing below.
    let req = GitReq::new("remote", ["remote", "set-head", "origin", "--auto"]).user().timeout(None).own_session();
    let out = client.exec_streaming(req, progress, cancel).await?;
    if out.success() {
        if let Some(b) = origin_head(client).await? {
            return Ok(Some(b));
        }
    }
    Ok(["main", "master"]
        .iter()
        .find(|b| branches.iter().any(|x| x == *b))
        .map(|b| b.to_string())
        .or_else(|| branches.into_iter().next()))
}

/// The branch's files the folder does not have, split into those that can be written and those
/// that cannot, because one of the folder's own files is where a folder of theirs would go.
/// Writing one of those would stop `git checkout-index` with "cannot create directory".
async fn files_to_write(client: &GitClient) -> Result<(Vec<String>, Vec<String>)> {
    let missing = client.exec(GitReq::new("ls-files", ["ls-files", "-z", "--deleted"]).timeout(None)).await?;
    // Untracked folders come back as one `name/` entry each, so this lists files, symbolic links
    // and folders, without walking into folders the repository knows nothing about.
    let others = client.exec(GitReq::new("ls-files", ["ls-files", "-z", "--others", "--directory"]).timeout(None)).await?;
    let not_folders: HashSet<String> = split_z(&others.stdout).into_iter().filter(|p| !p.ends_with('/')).collect();
    Ok(split_z(&missing.stdout).into_iter().partition(|path| !blocked_by(path, &not_folders)))
}

/// Whether a proper prefix of `path`, ending at a `/`, is in `not_folders`.
fn blocked_by(path: &str, not_folders: &HashSet<String>) -> bool {
    path.match_indices('/').any(|(i, _)| not_folders.contains(&path[..i]))
}

fn split_z(bytes: &[u8]) -> Vec<String> {
    bytes.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;

    use super::*;
    use crate::git::console::ConsoleRecorder;
    use crate::git::versions::Capabilities;
    use crate::transport::local::LocalTransport;
    use crate::transport::paths::IdentityMapper;

    #[test]
    fn paths_and_names() {
        assert_eq!(parent_native(&NativePath("/home/me/code/django".into())), Some(NativePath("/home/me/code".into())));
        assert_eq!(parent_native(&NativePath("/django/".into())), Some(NativePath("/".into())));
        assert_eq!(parent_native(&NativePath(r"C:\code\django".into())), Some(NativePath(r"C:\code".into())));
        assert_eq!(parent_native(&NativePath(r"C:\django".into())), Some(NativePath(r"C:\".into())));
        assert_eq!(parent_native(&NativePath("/".into())), None);
        assert_eq!(join_native(&NativePath(r"C:\code\".into()), ".git"), NativePath(r"C:\code\.git".into()));
        assert_eq!(join_native(&NativePath("/srv/app".into()), ".git"), NativePath("/srv/app/.git".into()));
        assert_eq!(name_from_url("git@github.com:django/django.git").as_deref(), Some("django"));
        assert_eq!(name_from_url("https://example.com/a/b.git/").as_deref(), Some("b"));
        assert_eq!(name_from_url("me@host:repo").as_deref(), Some("repo"));
        assert!(check_url("--upload-pack=x").is_err());
        let set: HashSet<String> = ["docs".to_string(), "a/b".to_string()].into();
        assert!(blocked_by("docs/index.md", &set));
        assert!(blocked_by("a/b/c/d", &set));
        assert!(!blocked_by("docs", &set));
        assert!(!blocked_by("docsx/y", &set));
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn client(dir: &Path) -> GitClient {
        GitClient {
            transport: Arc::new(LocalTransport { git: "git".into(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false }),
            repo: NativePath(dir.to_string_lossy().into_owned()),
            caps: Capabilities::default(),
            console: Arc::new(ConsoleRecorder::new(None, "clone-test".into(), 1 << 20)),
        }
    }

    struct NoStop;
    impl StopControl for NoStop {
        fn set_cancellable(&self, _: bool) {}
    }

    /// Clones `origin` into `target` the way `repo_clone` does: inspect, then clone.
    async fn clone(origin: &Path, target: &Path, cancel: &CancellationToken) -> (CloneFolder, Result<CloneOutcome>) {
        let (inner, outer) = (client(target), client(target.parent().unwrap()));
        let folder = inspect(&inner.transport, &inner.repo).await.unwrap();
        let progress = |_: String| {};
        let result = clone_into(&inner, &outer, &folder, origin.to_str().unwrap(), &progress, &NoStop, cancel).await;
        (folder, result)
    }

    #[tokio::test]
    async fn clones_into_missing_empty_and_non_empty_folders() {
        if which::which("git").is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("stashpanda-clone-{}", uuid::Uuid::new_v4().simple()));
        let src = root.join("src");
        std::fs::create_dir_all(src.join("docs")).unwrap();
        git(&root, &["init", "-q", "-b", "trunk", src.to_str().unwrap()]);
        for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&src, &["config", k, v]);
        }
        for (file, text) in [("README", "upstream\n"), ("a.txt", "a\n"), ("docs/d.md", "d\n"), ("z.txt", "z\n")] {
            std::fs::write(src.join(file), text).unwrap();
        }
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-qm", "one"]);
        git(&src, &["branch", "aaa-other"]);
        let origin = root.join("origin.git");
        git(&root, &["clone", "-q", "--bare", src.to_str().unwrap(), origin.to_str().unwrap()]);
        let never = CancellationToken::new();

        // Missing: a plain `git clone`, which creates the folder.
        let missing = root.join("missing");
        let (folder, out) = clone(&origin, &missing, &never).await;
        assert_eq!(folder, CloneFolder::Missing);
        let out = out.unwrap();
        assert_eq!((out.kept_existing, out.branch.as_deref()), (false, Some("trunk")));
        assert_eq!(std::fs::read_to_string(missing.join("docs/d.md")).unwrap(), "d\n");

        // Empty: also a plain `git clone`.
        let empty = root.join("empty");
        std::fs::create_dir(&empty).unwrap();
        let (folder, out) = clone(&origin, &empty, &never).await;
        assert_eq!(folder, CloneFolder::Empty);
        assert!(out.unwrap().branch.is_some());
        assert!(empty.join("z.txt").exists());

        // Files in it: kept, laid over the clone. README differs, mine.txt is new, and the file
        // `docs` is where the repository has a folder, so docs/d.md cannot be written.
        let full = root.join("full");
        std::fs::create_dir(&full).unwrap();
        std::fs::write(full.join("README"), "mine\n").unwrap();
        std::fs::write(full.join("mine.txt"), "keep\n").unwrap();
        std::fs::write(full.join("docs"), "a file\n").unwrap();
        let (folder, out) = clone(&origin, &full, &never).await;
        assert_eq!(folder, CloneFolder::HasFiles { count: 3 });
        let out = out.unwrap();
        assert!(out.kept_existing);
        // origin/HEAD names trunk even though aaa-other sorts first.
        assert_eq!(out.branch.as_deref(), Some("trunk"));
        assert_eq!(out.blocked, vec!["docs/d.md".to_string()]);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_eq!(std::fs::read_to_string(full.join("README")).unwrap(), "mine\n");
        assert_eq!(std::fs::read_to_string(full.join("docs")).unwrap(), "a file\n");
        assert_eq!(std::fs::read_to_string(full.join("a.txt")).unwrap(), "a\n");
        assert_eq!(std::fs::read_to_string(full.join("z.txt")).unwrap(), "z\n");
        let status = git(&full, &["status", "--porcelain"]);
        let mut lines: Vec<&str> = status.lines().collect();
        lines.sort();
        assert_eq!(lines, vec![" D docs/d.md", " M README", "?? docs", "?? mine.txt"]);
        assert_eq!(git(&full, &["rev-parse", "--abbrev-ref", "trunk@{upstream}"]).trim(), "origin/trunk");
        assert_eq!(git(&full, &["rev-parse", "HEAD"]), git(&src, &["rev-parse", "HEAD"]));

        // Already a repository: refused, and nothing is deleted.
        let (folder, out) = clone(&origin, &full, &never).await;
        assert_eq!(folder, CloneFolder::Repository);
        assert!(matches!(out, Err(GitError::InvalidPath { .. })));
        assert!(full.join(".git").exists());

        // A failed clone into a folder with files: the folder is back as it was.
        let bad = root.join("bad");
        std::fs::create_dir(&bad).unwrap();
        std::fs::write(bad.join("keep.txt"), "keep\n").unwrap();
        let (inner, outer) = (client(&bad), client(&root));
        let progress = |_: String| {};
        let folder = inspect(&inner.transport, &inner.repo).await.unwrap();
        let nowhere = root.join("no-such-remote.git");
        assert!(clone_into(&inner, &outer, &folder, nowhere.to_str().unwrap(), &progress, &NoStop, &never).await.is_err());
        let left: Vec<String> = std::fs::read_dir(&bad).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["keep.txt".to_string()]);

        // A failed clone into a missing folder leaves no folder behind.
        let gone = root.join("gone");
        let (inner, outer) = (client(&gone), client(&root));
        let folder = inspect(&inner.transport, &inner.repo).await.unwrap();
        assert!(clone_into(&inner, &outer, &folder, nowhere.to_str().unwrap(), &progress, &NoStop, &never).await.is_err());
        assert!(!gone.exists());

        // The folder that would contain the target has to exist.
        let deep = root.join("x").join("y");
        assert!(matches!(inspect(&client(&deep).transport, &NativePath(deep.to_string_lossy().into_owned())).await, Err(GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&root);
    }
}
