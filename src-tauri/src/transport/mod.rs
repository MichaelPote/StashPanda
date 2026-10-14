//! The transport abstraction: how a command line reaches a Git executable.
//!
//! `Local` runs the executable directly, `Wsl` wraps it in `wsl.exe`, `Ssh`
//! runs it on a remote host over a multiplexed connection. Everything above
//! this layer works in terms of [`Cmd`] and [`Output`] and never spawns.

pub mod discover;
pub mod local;
pub mod paths;
pub mod ssh;
pub mod ssh_pool;
pub mod wsl;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::error::{GitError, Result};
use paths::{NativePath, PathMapper};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TransportKind {
    Local,
    Wsl { distro: String },
    Ssh { host: String },
}

/// Who asked for a command. The console can hide `Internal` and `Watcher`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    UserAction,
    Internal,
    Watcher,
}

#[derive(Debug, Clone)]
pub enum Program {
    /// The transport's configured git executable.
    Git,
    Other(String),
}

#[derive(Debug, Clone)]
pub enum StdinMode {
    Null,
    Piped,
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct Cmd {
    pub program: Program,
    pub args: Vec<String>,
    /// The arguments the caller asked for, before the flags every git command carries were added
    /// (`checkout master`). The console lists this first; None means `args` is already that.
    pub summary: Option<Vec<String>>,
    pub env: Vec<(String, String)>,
    pub stdin: StdinMode,
    pub category: Category,
    pub timeout: Option<Duration>,
    pub label: &'static str,
    /// Run the command so that `Process::terminate` reaches it and every process it starts
    /// (`ssh`, `git-remote-https`), not just the one the transport spawned. On Linux, macOS and
    /// inside WSL it leads a new session (`setsid`), which also leaves it without a controlling
    /// terminal, so `ssh` cannot stop to ask for a passphrase on the terminal StashPanda was
    /// started from. On Windows its whole process tree is killed. Set for cancellable commands.
    pub own_session: bool,
}

impl Cmd {
    pub fn git(label: &'static str, args: Vec<String>) -> Cmd {
        Cmd {
            program: Program::Git,
            args,
            summary: None,
            env: Vec::new(),
            stdin: StdinMode::Null,
            category: Category::Internal,
            timeout: Some(Duration::from_secs(120)),
            label,
            own_session: false,
        }
    }
    pub fn category(mut self, c: Category) -> Cmd {
        self.category = c;
        self
    }
    pub fn stdin_bytes(mut self, b: Vec<u8>) -> Cmd {
        self.stdin = StdinMode::Bytes(b);
        self
    }
    pub fn timeout(mut self, t: Option<Duration>) -> Cmd {
        self.timeout = t;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct Output {
    /// None when the process was killed by a signal.
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Facts learned by probing a transport before the first real command.
#[derive(Debug, Clone)]
pub struct TransportInfo {
    pub git_version: crate::git::versions::GitVersion,
    pub git_executable: String,
    pub warnings: Vec<String>,
}

#[async_trait]
pub trait Process: Send {
    fn stdin(&mut self) -> Option<Box<dyn AsyncWrite + Send + Unpin>>;
    fn stdout(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>>;
    fn stderr(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>>;
    async fn wait(&mut self) -> Result<Option<i32>>;
    async fn kill(&mut self) -> Result<()>;
    /// Ask the process to stop, together with the processes it started when it was spawned with
    /// `Cmd::own_session`: SIGTERM on Unix, so Git removes its lock files on the way out, and a
    /// tree kill on Windows, where a console process cannot be asked. Returns once the request is
    /// sent; `wait` reports the exit. Transports with nothing gentler than `kill` use that.
    async fn terminate(&mut self) -> Result<()> {
        self.kill().await
    }
    fn remote_pid(&self) -> Option<u32> {
        None
    }
}

/// `modified_times` for transports whose files the app can reach on its own filesystem
/// (Local, and WSL through the `\\wsl.localhost` view). Symbolic links report their own time,
/// since Git tracks the link rather than its target.
pub(crate) async fn host_modified_times(mapper: &dyn PathMapper, paths: &[NativePath]) -> Result<Vec<Option<i64>>> {
    let hosts: Vec<Option<String>> = paths.iter().map(|p| mapper.to_host(p).ok().map(|h| h.0)).collect();
    let times = tokio::task::spawn_blocking(move || {
        hosts
            .into_iter()
            .map(|host| {
                let meta = std::fs::symlink_metadata(host?).ok()?;
                let since = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
                i64::try_from(since.as_millis()).ok()
            })
            .collect()
    })
    .await
    .map_err(|e| GitError::internal(e.to_string()))?;
    Ok(times)
}

/// `list_dir` for transports whose files the app can reach on its own filesystem, as for
/// `host_modified_times`. A symbolic link to a folder counts as the folder.
pub(crate) async fn host_list_dir(mapper: &dyn PathMapper, path: &NativePath) -> Result<Option<Vec<String>>> {
    let host = mapper.to_host(path)?;
    match tokio::fs::metadata(&host.0).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(meta) if !meta.is_dir() => {
            return Err(GitError::InvalidPath { path: path.0.clone(), reason: "it is a file, not a folder".into() })
        }
        Ok(_) => {}
    }
    let mut names = Vec::new();
    let mut entries = tokio::fs::read_dir(&host.0).await?;
    while let Some(entry) = entries.next_entry().await? {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(Some(names))
}

/// `remove_dir_all` through the host filesystem, as for `host_list_dir`.
pub(crate) async fn host_remove_dir_all(mapper: &dyn PathMapper, path: &NativePath) -> Result<()> {
    let host = mapper.to_host(path)?;
    match tokio::fs::remove_dir_all(&host.0).await {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> TransportKind;
    fn paths(&self) -> &dyn PathMapper;
    /// The full host-side command line as it will execute, wrapper included.
    fn render(&self, cmd: &Cmd) -> Vec<String>;
    async fn spawn(&self, cmd: Cmd) -> Result<Box<dyn Process>>;
    async fn probe(&self) -> Result<TransportInfo>;
    /// Bytes of a working-tree file, capped at `limit`.
    async fn read_file(&self, path: &NativePath, limit: usize) -> Result<Vec<u8>>;
    /// Delete one file. A file that does not exist is not an error.
    async fn remove_file(&self, path: &NativePath) -> Result<()>;
    /// Last-modified time of each working-tree file, in milliseconds since 1970, one result per
    /// path in order. None for a path that does not exist (a deleted file) or cannot be read.
    async fn modified_times(&self, paths: &[NativePath]) -> Result<Vec<Option<i64>>>;
    /// The names of the entries in folder `path`, or None when nothing exists at `path`. Fails
    /// with `InvalidPath` when `path` is a file.
    async fn list_dir(&self, path: &NativePath) -> Result<Option<Vec<String>>>;
    /// Delete folder `path` and everything inside it. A folder that does not exist is not an
    /// error.
    async fn remove_dir_all(&self, path: &NativePath) -> Result<()>;
    async fn shutdown(&self) {}

    /// Spawn, feed stdin, drain both pipes, wait. Honors `cmd.timeout`.
    async fn run(&self, cmd: Cmd) -> Result<Output> {
        let timeout = cmd.timeout;
        let stdin_bytes = match &cmd.stdin {
            StdinMode::Bytes(b) => Some(b.clone()),
            _ => None,
        };
        let mut child = self.spawn(cmd).await?;
        let fut = async {
            let mut stdin = child.stdin();
            let mut stdout = child.stdout();
            let mut stderr = child.stderr();
            let write = async {
                if let (Some(mut w), Some(bytes)) = (stdin.take(), stdin_bytes) {
                    w.write_all(&bytes).await?;
                    w.shutdown().await?;
                }
                Ok::<_, std::io::Error>(())
            };
            let read_out = async {
                let mut buf = Vec::new();
                if let Some(mut r) = stdout.take() {
                    r.read_to_end(&mut buf).await?;
                }
                Ok::<_, std::io::Error>(buf)
            };
            let read_err = async {
                let mut buf = Vec::new();
                if let Some(mut r) = stderr.take() {
                    r.read_to_end(&mut buf).await?;
                }
                Ok::<_, std::io::Error>(buf)
            };
            let (w, o, e) = tokio::join!(write, read_out, read_err);
            w?;
            let stdout = o?;
            let stderr = e?;
            let exit_code = child.wait().await?;
            Ok::<Output, GitError>(Output {
                exit_code,
                stdout,
                stderr,
            })
        };
        match timeout {
            Some(t) => match tokio::time::timeout(t, fut).await {
                Ok(r) => r,
                Err(_) => Err(GitError::Timeout { secs: t.as_secs() }),
            },
            None => fut.await,
        }
    }

    /// Like `run`, for commands that report progress on stderr (`fetch` with `--progress`). Each
    /// update reaches `progress` as it arrives: Git separates them with `\r` and ends a phase with
    /// `\n`. The returned stderr is what a terminal would be left showing, one line per phase (see
    /// `collapse_progress`). Takes no stdin.
    ///
    /// Cancelling `cancel` terminates the process (see `Process::terminate`), keeps reading its
    /// output, and returns `Cancelled` only once it has exited, or after `TERMINATE_GRACE` and a
    /// `kill` when it does not stop. So when this returns, nothing is still writing to the
    /// repository and the caller can release its locks.
    async fn run_streaming(&self, cmd: Cmd, progress: &(dyn Fn(String) + Send + Sync), cancel: &CancellationToken) -> Result<Output> {
        let timeout = cmd.timeout;
        let mut child = self.spawn(cmd).await?;
        let mut stdout = child.stdout();
        let mut stderr = child.stderr();
        let read_out = async move {
            let mut buf = Vec::new();
            if let Some(r) = stdout.as_mut() {
                r.read_to_end(&mut buf).await?;
            }
            Ok::<_, std::io::Error>(buf)
        };
        let read_err = async move {
            let mut all = Vec::new();
            let mut start = 0;
            if let Some(r) = stderr.as_mut() {
                let mut chunk = [0u8; 4096];
                loop {
                    let n = r.read(&mut chunk).await?;
                    if n == 0 {
                        break;
                    }
                    all.extend_from_slice(&chunk[..n]);
                    while let Some(i) = all[start..].iter().position(|b| *b == b'\r' || *b == b'\n') {
                        send_progress(&all[start..start + i], progress);
                        start += i + 1;
                    }
                }
            }
            send_progress(&all[start..], progress);
            Ok::<_, std::io::Error>(all)
        };
        // The readers own the pipes and do not borrow `child`, so the process can be terminated
        // while they keep draining its output.
        let readers = async { tokio::join!(read_out, read_err) };
        tokio::pin!(readers);
        let deadline = async {
            match timeout {
                Some(t) => tokio::time::sleep(t).await,
                None => std::future::pending().await,
            }
        };
        let read = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            // Dropping `child` on the way out kills it.
            _ = deadline => return Err(GitError::Timeout { secs: timeout.map(|t| t.as_secs()).unwrap_or(0) }),
            r = &mut readers => Some(r),
        };
        let Some((o, e)) = read else {
            let exited = tokio::time::timeout(TERMINATE_GRACE, async {
                // Keep reading while terminating: the WSL transport learns the pid it signals from
                // stdout, and a full pipe would keep Git from exiting.
                let (sent, _) = tokio::join!(child.terminate(), &mut readers);
                if let Err(err) = sent {
                    tracing::warn!("terminating a cancelled command failed: {err}");
                }
                child.wait().await
            })
            .await;
            if exited.is_err() {
                tracing::warn!("a cancelled command did not exit within {}s; killing it", TERMINATE_GRACE.as_secs());
                child.kill().await?;
                let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
            }
            return Err(GitError::Cancelled);
        };
        let stdout = o?;
        let stderr = collapse_progress(&e?);
        let exit_code = child.wait().await?;
        Ok(Output { exit_code, stdout, stderr })
    }
}

/// How long a terminated process gets to exit on its own before it is killed.
pub const TERMINATE_GRACE: Duration = Duration::from_secs(5);
/// How long to wait for a killed process to be reported as exited.
const KILL_GRACE: Duration = Duration::from_secs(2);

/// The process's stdout when the transport wrapped it as `echo $$; exec <command>` (WSL, SSH):
/// the first line, the pid of the process that `exec` became, goes to `pid`, and the rest passes
/// through unchanged. When the first line is not a number it passes through too.
pub(crate) struct PidLine<R> {
    inner: R,
    pid: Arc<std::sync::OnceLock<u32>>,
    head: Vec<u8>,
    /// Bytes read with the first line that are output, still to be handed on.
    pending: Vec<u8>,
    done: bool,
}

impl<R> PidLine<R> {
    pub(crate) fn new(inner: R, pid: Arc<std::sync::OnceLock<u32>>) -> PidLine<R> {
        PidLine { inner, pid, head: Vec::new(), pending: Vec::new(), done: false }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for PidLine<R> {
    fn poll_read(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        let this = self.get_mut();
        loop {
            if this.done {
                if !this.pending.is_empty() {
                    let n = this.pending.len().min(buf.remaining());
                    buf.put_slice(&this.pending[..n]);
                    this.pending.drain(..n);
                    return Poll::Ready(Ok(()));
                }
                return std::pin::Pin::new(&mut this.inner).poll_read(cx, buf);
            }
            let mut chunk = [0u8; 256];
            let mut rb = tokio::io::ReadBuf::new(&mut chunk);
            match std::pin::Pin::new(&mut this.inner).poll_read(cx, &mut rb) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {}
            }
            let got = rb.filled();
            if got.is_empty() {
                // End of output before a whole first line.
                this.done = true;
                this.pending = std::mem::take(&mut this.head);
                if this.pending.is_empty() {
                    return Poll::Ready(Ok(()));
                }
                continue;
            }
            this.head.extend_from_slice(got);
            if let Some(i) = this.head.iter().position(|b| *b == b'\n') {
                match String::from_utf8_lossy(&this.head[..i]).trim().parse::<u32>() {
                    Ok(pid) => {
                        let _ = this.pid.set(pid);
                        this.pending = this.head.split_off(i + 1);
                    }
                    Err(_) => this.pending = std::mem::take(&mut this.head),
                }
                this.head.clear();
                this.done = true;
            } else if this.head.len() > 32 {
                // Far longer than a pid: not the wrapper's line.
                this.done = true;
                this.pending = std::mem::take(&mut this.head);
            }
        }
    }
}

/// One progress update without terminal escape codes or the padding Git adds to overwrite a
/// longer previous update; empty updates are dropped.
fn send_progress(segment: &[u8], progress: &(dyn Fn(String) + Send + Sync)) {
    let text = strip_ansi(&String::from_utf8_lossy(segment));
    let text = text.trim_end();
    if !text.is_empty() {
        progress(text.to_string());
    }
}

/// Remove ANSI escape sequences such as the `\x1b[K` (clear to end of line) that `rebase`
/// writes before "Successfully rebased".
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            // Parameters and intermediates up to the final byte, `@` to `~`.
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// Progress output as a terminal would be left showing it: each `\r` returns to the start of the
/// line, so only the last non-empty update before each `\n` is kept, without escape codes or
/// trailing padding. `Receiving objects:   1% ...\r ... 100% (302/302), done.\n` becomes
/// `Receiving objects: 100% (302/302), done.`.
pub fn collapse_progress(stderr: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| line.split('\r').map(|s| strip_ansi(s).trim_end().to_string()).rfind(|s| !s.is_empty()).unwrap_or_default())
        .collect();
    lines.join("\n").into_bytes()
}

pub type SharedTransport = Arc<dyn Transport>;

/// Shell-quote for display in the console (POSIX style, good enough on Windows
/// for reading). Not used to execute anything.
pub fn display_command(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty() {
                "''".to_string()
            } else if a
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,\\".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_collapses_to_the_last_update_of_each_line() {
        let raw = b"remote: Counting objects:   0% (1/302)        \rremote: Counting objects: 100% (302/302), done.        \nUnpacking objects:  50% (1/2)\rUnpacking objects: 100% (2/2), done.\n\x1b[KSuccessfully rebased.\n";
        assert_eq!(
            String::from_utf8(collapse_progress(raw)).unwrap(),
            "remote: Counting objects: 100% (302/302), done.\nUnpacking objects: 100% (2/2), done.\nSuccessfully rebased.\n"
        );
        // A trailing `\r` leaves the update before it on screen.
        assert_eq!(collapse_progress(b"Rebasing (1/2)\r"), b"Rebasing (1/2)");
        assert_eq!(strip_ansi("a\x1b[2Kb\x1b[0mc"), "abc");
    }

    /// Processes whose command line starts with `pattern`; the `^` keeps pgrep from matching the
    /// shell that runs the tests.
    fn running(pattern: &str) -> String {
        let out = std::process::Command::new("pgrep").args(["-f", pattern]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Start `sh -c 'echo started >&2; sleep <a> & sleep <b>'` in its own session, cancel it once
    /// it has started, and check that `run_streaming` returns `Cancelled` only after the shell and
    /// its background child have both gone, well inside the grace period.
    async fn cancel_reaches_the_whole_group(transport: &dyn Transport, a: u32) {
        let mut cmd = Cmd::git("sh", vec!["-c".into(), format!("echo started >&2; sleep {a} & sleep {}", a + 1)]);
        cmd.program = Program::Other("sh".into());
        cmd.own_session = true;
        cmd.timeout = None;
        let cancel = CancellationToken::new();
        let progress = |line: String| {
            if line == "started" {
                cancel.cancel();
            }
        };
        let started = std::time::Instant::now();
        let out = transport.run_streaming(cmd, &progress, &cancel).await;
        assert!(matches!(out, Err(GitError::Cancelled)), "{out:?}");
        assert!(started.elapsed() < TERMINATE_GRACE, "took {:?}: terminate did not reach it", started.elapsed());
        let left = running(&format!("^sleep {a}|^sleep {}", a + 1));
        assert!(left.is_empty(), "left running: {left}");
    }

    /// Spawn `sh -c 'echo ready; sleep <a> & sleep <b>'` in its own session, read up to `ready`,
    /// drop the process without waiting for it, and check that both sleeps go.
    async fn drop_reaches_the_whole_group(transport: &dyn Transport, a: u32) {
        let mut cmd = Cmd::git("sh", vec!["-c".into(), format!("echo ready; sleep {a} & sleep {}", a + 1)]);
        cmd.program = Program::Other("sh".into());
        cmd.own_session = true;
        let mut process = transport.spawn(cmd).await.unwrap();
        let mut stdout = process.stdout().unwrap();
        let mut buf = [0u8; 16];
        let n = stdout.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"ready\n");
        let pattern = format!("^sleep {a}|^sleep {}", a + 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while running(&pattern).lines().count() < 2 {
            assert!(std::time::Instant::now() < deadline, "sleeps did not start");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        drop(stdout);
        drop(process);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !running(&pattern).is_empty() {
            assert!(std::time::Instant::now() < deadline, "left running after drop: {}", running(&pattern));
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn own_session_on_the_local_transport() {
        use crate::transport::local::LocalTransport;
        use crate::transport::paths::IdentityMapper;
        if !cfg!(unix) || which::which("pgrep").is_err() {
            return;
        }
        let local = LocalTransport { git: "git".into(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false };
        // A session leader's session id is its own pid, and a new session has no terminal.
        let mut cmd = Cmd::git("sh", vec!["-c".into(), "echo $$ $(ps -o sid= -p $$)".into()]);
        cmd.program = Program::Other("sh".into());
        cmd.own_session = true;
        let out = local.run(cmd).await.unwrap();
        let ids: Vec<String> = String::from_utf8_lossy(&out.stdout).split_whitespace().map(String::from).collect();
        assert!(ids.len() == 2 && ids[0] == ids[1], "{ids:?}");
        cancel_reaches_the_whole_group(&local, 3201).await;
        drop_reaches_the_whole_group(&local, 3203).await;
    }

    /// Drives `WslTransport` through `wsl.exe` from inside WSL, against the distro the tests run
    /// in, so the processes it starts are visible to pgrep here. Skips anywhere else.
    #[tokio::test]
    async fn own_session_on_the_wsl_transport() {
        use crate::transport::paths::WslMapper;
        use crate::transport::wsl::WslTransport;
        let Ok(distro) = std::env::var("WSL_DISTRO_NAME") else { return };
        if !std::path::Path::new("/mnt/c/Windows/System32/wsl.exe").exists() || which::which("pgrep").is_err() {
            return;
        }
        let wsl = WslTransport { distro: distro.clone(), git: "git".into(), mapper: WslMapper { distro, automount_root: "/mnt/".into() } };
        // The wrapper's pid line is taken off stdout, and arguments arrive unsplit.
        let mut cmd = Cmd::git("sh", vec!["-c".into(), r#"printf '%s\n' "$1""#.into(), "sh".into(), "a b'c".into()]);
        cmd.program = Program::Other("sh".into());
        cmd.own_session = true;
        let progress = |_: String| {};
        let out = wsl.run_streaming(cmd, &progress, &CancellationToken::new()).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "a b'c\n");
        cancel_reaches_the_whole_group(&wsl, 3211).await;
        drop_reaches_the_whole_group(&wsl, 3213).await;
    }

    #[tokio::test]
    async fn pid_line_takes_the_first_line_and_passes_the_rest() {
        async fn read(input: &'static [u8]) -> (Option<u32>, Vec<u8>) {
            let pid = Arc::new(std::sync::OnceLock::new());
            let mut r = PidLine::new(input, pid.clone());
            let mut out = Vec::new();
            r.read_to_end(&mut out).await.unwrap();
            (pid.get().copied(), out)
        }
        assert_eq!(read(b"4242\nhello\nworld").await, (Some(4242), b"hello\nworld".to_vec()));
        assert_eq!(read(b"4242\n").await, (Some(4242), Vec::new()));
        assert_eq!(read(b"").await, (None, Vec::new()));
        // Not the wrapper's line (the wrapper failed to start): everything passes through.
        assert_eq!(read(b"fatal: no\nmore").await, (None, b"fatal: no\nmore".to_vec()));
        assert_eq!(read(b"12").await, (None, b"12".to_vec()));
    }
}
