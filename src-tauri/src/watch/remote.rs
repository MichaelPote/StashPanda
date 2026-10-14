//! Change detection where the app cannot watch the files itself (over SSH, and from Windows into
//! WSL): `inotifywait` runs on the system that holds the repository, through the repository's own
//! transport, and prints one line per change, which maps to the same events the native watcher
//! sends. Where `inotifywait` is not installed, or stops, the repository falls back to polling.

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{classify_git_dir_path, WatchEvent, WatcherHandle, WatcherKind};
use crate::error::Result;
use crate::transport::{Category, Cmd, Program, SharedTransport};

pub struct RemoteWatcher {
    cancel: CancellationToken,
}

impl WatcherHandle for RemoteWatcher {
    fn kind(&self) -> WatcherKind {
        WatcherKind::RemoteInotify
    }
    fn stop(self: Box<Self>) {
        self.cancel.cancel();
    }
}

/// Most ignored folders named in the `--exclude` pattern; past that the pattern gets long enough
/// to slow every event down, and the rest are filtered by the refresh finding nothing.
const MAX_EXCLUDED_DIRS: usize = 500;

/// The events that change what Git reports. `access`, `open` and `close_nowrite` never do.
const EVENTS: &str = "modify,close_write,create,delete,move,attrib";

/// True when `inotifywait` is on the remote system's `PATH`.
pub async fn available(transport: &SharedTransport) -> bool {
    let mut cmd = Cmd::git("probe", vec!["-c".into(), "command -v inotifywait".into()]);
    cmd.program = Program::Other("sh".into());
    cmd.category = Category::Watcher;
    matches!(transport.run(cmd).await, Ok(out) if out.success() && !out.stdout.trim_ascii().is_empty())
}

/// Escapes a path for a POSIX extended regular expression.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.[]()*+?{}|^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The `--exclude` pattern: Git's object store, logs and lock files, which change on every
/// command and never on their own, and every folder Git ignores (`node_modules`, build output),
/// which would otherwise use up the system's limit on watched folders and flood the watcher.
/// `ignored_dirs` are absolute paths on the remote system.
pub fn exclude_pattern(ignored_dirs: &[String]) -> String {
    let mut parts = vec![r"/\.git/(objects|lfs|logs)/".to_string(), r"\.lock$".to_string()];
    for dir in ignored_dirs.iter().take(MAX_EXCLUDED_DIRS) {
        parts.push(format!("^{}(/|$)", regex_escape(dir.trim_end_matches('/'))));
    }
    format!("({})", parts.join("|"))
}

/// What one line of `inotifywait --format '%w%f|%e'` means, or None for noise. The paths are
/// absolute paths on the remote system without a trailing slash; `worktree` is None for a bare
/// repository.
pub fn map_line(line: &str, worktree: Option<&str>, git_dir: &str, common_dir: &str) -> Option<WatchEvent> {
    let (path, events) = line.rsplit_once('|')?;
    if events.contains("Q_OVERFLOW") {
        return Some(WatchEvent::Overflow);
    }
    for dir in [git_dir, common_dir] {
        if let Some(rel) = path.strip_prefix(dir).and_then(|r| r.strip_prefix('/')) {
            return classify_git_dir_path(rel).map(WatchEvent::GitDir);
        }
    }
    let wt = worktree?;
    path.strip_prefix(wt).filter(|r| r.is_empty() || r.starts_with('/'))?;
    if path.ends_with(".lock") {
        return None;
    }
    Some(WatchEvent::Worktree)
}

/// Starts `inotifywait` on the remote system and turns its output into watch events. When it
/// stops of its own accord (the connection dropped, or it hit the limit on watched folders),
/// `on_exit` gets what it printed on stderr; stopping the watcher does not call it.
#[allow(clippy::too_many_arguments)]
pub async fn start(
    transport: SharedTransport,
    worktree: Option<String>,
    git_dir: String,
    common_dir: String,
    ignored_dirs: &[String],
    parent_cancel: &CancellationToken,
    tx: mpsc::UnboundedSender<WatchEvent>,
    on_exit: impl FnOnce(String) + Send + 'static,
) -> Result<RemoteWatcher> {
    let mut args: Vec<String> = vec!["-m".into(), "-r".into(), "-q".into(), "-e".into(), EVENTS.into()];
    args.push("--exclude".into());
    args.push(exclude_pattern(ignored_dirs));
    args.push("--format".into());
    args.push("%w%f|%e".into());
    if let Some(wt) = &worktree {
        args.push(wt.clone());
    }
    args.push(git_dir.clone());
    if common_dir != git_dir {
        args.push(common_dir.clone());
    }
    let mut cmd = Cmd::git("inotifywait", args);
    cmd.program = Program::Other("inotifywait".into());
    cmd.category = Category::Watcher;
    cmd.timeout = None;
    // In a session of its own, so stopping the watcher ends it on the remote system too.
    cmd.own_session = true;
    let mut process = transport.spawn(cmd).await?;
    let stdout = process.stdout();
    let mut stderr = process.stderr();

    let cancel = parent_cancel.child_token();
    let stop = cancel.clone();
    tauri::async_runtime::spawn(async move {
        let Some(stdout) = stdout else { return };
        let mut lines = BufReader::new(stdout).lines();
        let ended = loop {
            tokio::select! {
                _ = stop.cancelled() => break false,
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if let Some(ev) = map_line(&line, worktree.as_deref(), &git_dir, &common_dir) {
                            let _ = tx.send(ev);
                        }
                    }
                    _ => break true,
                },
            }
        };
        if !ended {
            let _ = process.terminate().await;
            let _ = process.wait().await;
            return;
        }
        let _ = process.wait().await;
        let mut text = String::new();
        if let Some(err) = stderr.as_mut() {
            let mut buf = Vec::new();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), err.read_to_end(&mut buf)).await;
            text = String::from_utf8_lossy(&buf).trim().to_string();
        }
        on_exit(text);
    });
    Ok(RemoteWatcher { cancel })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::GitDirChange;

    #[test]
    fn maps_lines_to_events() {
        let m = |l: &str| map_line(l, Some("/srv/repo"), "/srv/repo/.git", "/srv/repo/.git");
        assert_eq!(m("/srv/repo/src/a.rs|MODIFY"), Some(WatchEvent::Worktree));
        assert_eq!(m("/srv/repo/a b|c.txt|CLOSE_WRITE,CLOSE"), Some(WatchEvent::Worktree));
        assert_eq!(m("/srv/repo/.git/HEAD|MOVED_TO"), Some(WatchEvent::GitDir(GitDirChange::Head)));
        assert_eq!(m("/srv/repo/.git/refs/heads/main|CREATE"), Some(WatchEvent::GitDir(GitDirChange::Refs)));
        assert_eq!(m("/srv/repo/.git/index|MODIFY"), Some(WatchEvent::GitDir(GitDirChange::Index)));
        assert_eq!(m("/srv/repo/.git/objects/ab/cd|CREATE"), None);
        assert_eq!(m("/srv/repo/x.lock|CREATE"), None);
        assert_eq!(m("/srv/repository-other/a|CREATE"), None);
        assert_eq!(m("|Q_OVERFLOW"), Some(WatchEvent::Overflow));
        assert_eq!(m("no separator"), None);
        // A bare repository has no working tree.
        assert_eq!(map_line("/srv/repo/x|CREATE", None, "/srv/repo/.git", "/srv/repo/.git"), None);
    }

    async fn next_event(rx: &mut mpsc::UnboundedReceiver<WatchEvent>) -> Option<WatchEvent> {
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.ok().flatten()
    }

    /// A write inside an ignored folder, an edit, a commit, then stop, through `transport`, against
    /// a new repository at `dir`.
    async fn watch_a_repository(transport: crate::transport::SharedTransport, dir: &std::path::Path) {
        use std::process::Command;
        use std::time::Duration;
        let git = |args: &[&str]| {
            let st = Command::new("git").arg("-C").arg(dir).args(["-c", "user.email=t@t", "-c", "user.name=T", "-c", "commit.gpgsign=false"]).args(args).status().unwrap();
            assert!(st.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join(".gitignore"), "build/\n").unwrap();
        std::fs::create_dir_all(dir.join("build/deep")).unwrap();
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        let root = dir.to_string_lossy().into_owned();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let exited = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let exited2 = exited.clone();
        let watcher = start(transport, Some(root.clone()), format!("{root}/.git"), format!("{root}/.git"), &[format!("{root}/build")], &cancel, tx, move |e| {
            *exited2.lock().unwrap() = Some(e);
        })
        .await
        .unwrap();
        // inotifywait sets its watches up before it reports anything; give it a moment.
        tokio::time::sleep(Duration::from_millis(700)).await;
        std::fs::write(dir.join("build/deep/out.o"), "ignored").unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(800), rx.recv()).await.is_err(), "a write in an ignored folder is not reported");
        std::fs::write(dir.join("a.txt"), "2\n").unwrap();
        assert_eq!(next_event(&mut rx).await, Some(WatchEvent::Worktree));
        while tokio::time::timeout(Duration::from_millis(300), rx.recv()).await.is_ok() {}
        git(&["commit", "-qam", "second"]);
        let mut seen = Vec::new();
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(800), rx.recv()).await {
            seen.push(ev);
        }
        assert!(seen.contains(&WatchEvent::GitDir(GitDirChange::Refs)), "{seen:?}");
        Box::new(watcher).stop();
        tokio::time::sleep(Duration::from_millis(800)).await;
        let running = Command::new("pgrep").args(["-f", &format!("inotifywait.*{root}")]).output().unwrap();
        assert!(running.stdout.is_empty(), "inotifywait still running: {}", String::from_utf8_lossy(&running.stdout));
        assert!(exited.lock().unwrap().is_none(), "stopping is not an unexpected exit");
    }

    #[tokio::test]
    async fn watches_through_the_local_and_wsl_transports() {
        use crate::transport::local::LocalTransport;
        use crate::transport::paths::{IdentityMapper, WslMapper};
        use crate::transport::wsl::WslTransport;
        if which::which("inotifywait").is_err() || which::which("pgrep").is_err() {
            return;
        }
        let tmp = |name: &str| {
            let d = std::env::temp_dir().join(format!("stashpanda-{name}-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let local = tmp("inotify-local");
        let t: crate::transport::SharedTransport = std::sync::Arc::new(LocalTransport { git: which::which("git").unwrap(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false });
        watch_a_repository(t, &local).await;
        let _ = std::fs::remove_dir_all(&local);

        // The same through wsl.exe, as the Windows build runs it; only inside WSL with interop.
        let Ok(distro) = std::env::var("WSL_DISTRO_NAME") else { return };
        if !std::path::Path::new("/mnt/c/Windows/System32/wsl.exe").exists() {
            return;
        }
        let wsl = tmp("inotify-wsl");
        let t: crate::transport::SharedTransport =
            std::sync::Arc::new(WslTransport { distro: distro.clone(), git: "git".into(), mapper: WslMapper { distro, automount_root: "/mnt/".into() } });
        assert!(available(&t).await);
        watch_a_repository(t, &wsl).await;
        let _ = std::fs::remove_dir_all(&wsl);
    }

    #[test]
    fn excludes_git_internals_and_ignored_folders() {
        let p = exclude_pattern(&["/srv/repo/node_modules/".into(), "/srv/repo/a+b (1)".into()]);
        assert_eq!(p, r"(/\.git/(objects|lfs|logs)/|\.lock$|^/srv/repo/node_modules(/|$)|^/srv/repo/a\+b \(1\)(/|$))");
    }
}
