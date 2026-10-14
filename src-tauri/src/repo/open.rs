//! Open a repository from its library entry: resolve, probe, rev-parse, register.

use std::sync::Arc;

use tauri::AppHandle;
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

use super::resolve::resolve;
use super::{RepoHandle, RepoInfo};
use crate::error::{GitError, Result};
use crate::git::console::ConsoleRecorder;
use crate::git::versions::Capabilities;
use crate::git::{GitClient, GitReq};
use crate::platform::Host;
use crate::settings::{RepoEntry, Settings};
use crate::transport::paths::NativePath;
use crate::transport::ssh_pool::SshContext;

/// `rev-parse --git-common-dir` prints a path relative to the folder Git ran in (`.git` for an
/// ordinary repository) unless the common directory is somewhere else; this makes it absolute,
/// so the watchers can watch it. `ran_in` is the folder given to `git -C`.
fn absolute_common_dir(common: &str, ran_in: &str, git_dir: &str) -> String {
    if common.is_empty() || common == "." {
        return git_dir.to_string();
    }
    let absolute = common.starts_with(['/', '\\']) || common.as_bytes().get(1) == Some(&b':');
    if absolute {
        return common.to_string();
    }
    let sep = if ran_in.contains('\\') && !ran_in.starts_with('/') { "\\" } else { "/" };
    let mut parts: Vec<&str> = ran_in.trim_end_matches(['/', '\\']).split(['/', '\\']).collect();
    for part in common.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    parts.join(sep)
}

fn display_name_for(entry: &RepoEntry, native: &NativePath) -> String {
    if let Some(n) = &entry.display_name {
        if !n.trim().is_empty() {
            return n.clone();
        }
    }
    native
        .0
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(&native.0)
        .to_string()
}

pub async fn open_repo(
    app: Option<&AppHandle>,
    entry: &RepoEntry,
    settings: &Settings,
    host: &Host,
    ssh: Option<&Arc<SshContext>>,
) -> Result<Arc<RepoHandle>> {
    let res = resolve(entry, settings, host, ssh)?;
    let console = Arc::new(ConsoleRecorder::new(
        app.cloned(),
        entry.key.clone(),
        settings.console.output_cap_bytes as usize,
    ));

    // Probe: git --version through the transport (logged like any command).
    let probe_client = GitClient {
        transport: res.transport.clone(),
        repo: res.native_path.clone(),
        caps: Capabilities::default(),
        console: console.clone(),
    };
    let info = res.transport.probe().await?;
    let caps = Capabilities::for_version(info.git_version);

    // One rev-parse call answers five questions.
    let out = probe_client
        .exec(GitReq::new(
            "rev-parse",
            [
                "rev-parse",
                "--is-inside-work-tree",
                "--show-toplevel",
                "--absolute-git-dir",
                "--git-common-dir",
                "--is-bare-repository",
            ],
        ))
        .await
        .map_err(|e| match e {
            GitError::CommandFailed { stderr, .. }
                if stderr.to_ascii_lowercase().contains("not a git repository") =>
            {
                GitError::NotARepository {
                    path: res.native_path.0.clone(),
                }
            }
            other => other,
        })?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines().map(str::trim);
    let inside = lines.next().unwrap_or("false") == "true";
    let toplevel = lines.next().unwrap_or("").to_string();
    let git_dir = lines.next().unwrap_or("").to_string();
    let common_dir = lines.next().unwrap_or("").to_string();
    let is_bare = lines.next().unwrap_or("false") == "true";
    if !inside && !is_bare {
        return Err(GitError::NotARepository {
            path: res.native_path.0.clone(),
        });
    }

    let common_abs = absolute_common_dir(&common_dir, &res.native_path.0, &git_dir);
    // A linked worktree has a Git directory of its own inside the main repository's. Compared with
    // one kind of slash: Git for Windows prints `C:/…`, and `ran_in` can be `C:\…`.
    let same = |a: &str, b: &str| a.replace('\\', "/").trim_end_matches('/') == b.replace('\\', "/").trim_end_matches('/');
    let is_linked_worktree = !same(&common_abs, &git_dir);
    let mut warnings = res.warnings.clone();
    warnings.extend(info.warnings.clone());

    let host_path = res.transport.paths().to_host(&res.native_path).ok().map(|h| h.0);
    let repo_info = RepoInfo {
        key: entry.key.clone(),
        display_name: display_name_for(entry, &res.native_path),
        host_path,
        native_path: res.native_path.0.clone(),
        transport: res.resolved.clone(),
        git_version: info.git_version.to_string(),
        toplevel,
        git_dir,
        common_dir: common_abs,
        is_bare,
        is_linked_worktree,
        warnings,
    };

    let client = GitClient {
        transport: res.transport.clone(),
        repo: res.native_path.clone(),
        caps,
        console,
    };

    Ok(Arc::new(RepoHandle {
        key: entry.key.clone(),
        info: repo_info,
        transport: res.transport,
        client,
        host_path: res.host_path,
        native_path: res.native_path,
        lock: RwLock::new(()),
        write_gate: tokio::sync::Mutex::new(()),
        operations: std::sync::Mutex::new(Default::default()),
        read_slots: Semaphore::new(4),
        cancel: CancellationToken::new(),
        graph: arc_swap::ArcSwapOption::from(None),
        graph_version: std::sync::atomic::AtomicU64::new(0),
        last_status: RwLock::new(None),
        watcher: std::sync::Mutex::new(None),
        activity: tokio::sync::watch::Sender::new(crate::watch::Activity::Focused),
        catfile: Arc::new(crate::git::catfile::CatFile::new(settings.console.verbose_helpers)),
        watcher_state: std::sync::Mutex::new(None),
        hidden: std::sync::RwLock::new(Default::default()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_the_common_dir_absolute() {
        assert_eq!(absolute_common_dir(".git", "/srv/repo", "/srv/repo/.git"), "/srv/repo/.git");
        assert_eq!(absolute_common_dir("../.git", "/srv/repo/sub/", "/srv/repo/.git"), "/srv/repo/.git");
        assert_eq!(absolute_common_dir(".", "/srv/bare.git", "/srv/bare.git"), "/srv/bare.git");
        assert_eq!(absolute_common_dir("/srv/main/.git", "/srv/wt", "/srv/main/.git/worktrees/wt"), "/srv/main/.git");
        assert_eq!(absolute_common_dir(".git", r"C:\src\x", "C:/src/x/.git"), r"C:\src\x\.git");
        assert_eq!(absolute_common_dir(".git", r"\\wsl.localhost\Ubuntu\srv\x", "/srv/x/.git"), r"\\wsl.localhost\Ubuntu\srv\x\.git");
        assert_eq!(absolute_common_dir("C:/src/x/.git", r"C:\src\wt", "C:/src/x/.git/worktrees/wt"), "C:/src/x/.git");
    }

    /// The common directory comes out absolute for an ordinary repository opened from a
    /// subfolder, and for a linked worktree, where it is the main repository's `.git`.
    #[tokio::test]
    async fn opens_with_an_absolute_common_dir() {
        use crate::platform::Host;
        use crate::settings::{Connection, Settings};
        if which::which("git").is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("stashpanda-common-{}", uuid::Uuid::new_v4().simple()));
        let main = root.join("main");
        std::fs::create_dir_all(main.join("sub")).unwrap();
        let git = |dir: &std::path::Path, args: &[&str]| {
            let st = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.email=t@t", "-c", "user.name=T", "-c", "commit.gpgsign=false"]).args(args).status().unwrap();
            assert!(st.success(), "git {args:?}");
        };
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("sub/a.txt"), "1\n").unwrap();
        git(&main, &["add", "-A"]);
        git(&main, &["commit", "-qm", "first"]);
        git(&main, &["worktree", "add", "-q", root.join("wt").to_str().unwrap(), "-b", "side"]);
        let open = |path: std::path::PathBuf| async move {
            let entry = RepoEntry {
                key: "common".into(),
                path: path.to_string_lossy().into_owned(),
                connection: Connection::Local,
                resolved_transport: None,
                display_name: None,
                git_executable: None,
                group_id: None,
                last_opened_at: None,
            };
            open_repo(None, &entry, &Settings::default(), &Host::detect(), None).await.unwrap()
        };
        // Git for Windows prints `C:/…`; the folder it ran in is `C:\…`.
        let slashes = |p: &str| p.replace('\\', "/");
        let dot_git = slashes(&main.join(".git").to_string_lossy());
        let h = open(main.join("sub")).await;
        assert_eq!((slashes(&h.info.common_dir), slashes(&h.info.git_dir), h.info.is_linked_worktree), (dot_git.clone(), dot_git.clone(), false));
        let h = open(root.join("wt")).await;
        assert_eq!(slashes(&h.info.common_dir), dot_git);
        assert!(h.info.is_linked_worktree && h.info.git_dir.ends_with("worktrees/wt"), "{:?}", h.info.git_dir);
        let _ = std::fs::remove_dir_all(&root);
    }
}
