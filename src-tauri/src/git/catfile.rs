//! One long-lived `git cat-file --batch-command --buffer` per repository, so reading a blob does
//! not start a process each time: over SSH that is a round trip to open a channel, and from Windows
//! into WSL a `wsl.exe` start, for every file. Requests go through one at a time. The helper starts
//! on the first read, starts again if it dies, and stops after ten minutes without a request.
//!
//! The console shows the helper as one entry that runs for as long as it lives. With
//! `console.verboseHelpers` on, each request gets an entry of its own as well.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::error::{GitError, Result};
use crate::git::console::ConsoleTicket;
use crate::git::{GitClient, GitReq};
use crate::transport::{Output, Process, StdinMode};

/// How long the helper may sit unused before it is stopped.
pub const IDLE: Duration = Duration::from_secs(600);

struct Helper {
    process: Box<dyn Process>,
    stdin: Box<dyn AsyncWrite + Send + Unpin>,
    stdout: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
    ticket: Option<ConsoleTicket>,
}

pub struct CatFile {
    helper: tokio::sync::Mutex<Option<Helper>>,
    last_used: std::sync::Mutex<Instant>,
    /// `console.verboseHelpers`: log every request, not only the helper's start and exit.
    pub verbose: AtomicBool,
}

impl CatFile {
    /// Nothing runs until the first read. `verbose` is `console.verboseHelpers`.
    pub fn new(verbose: bool) -> CatFile {
        CatFile { helper: tokio::sync::Mutex::new(None), last_used: std::sync::Mutex::new(Instant::now()), verbose: AtomicBool::new(verbose) }
    }
}

/// What `info` said about an object.
#[derive(Debug, PartialEq, Eq)]
enum Info {
    Found { oid: String, size: u64 },
    Missing,
}

fn parse_info(line: &str) -> Result<Info> {
    let line = line.trim_end();
    if line.ends_with(" missing") || line.ends_with(" ambiguous") {
        return Ok(Info::Missing);
    }
    let mut parts = line.split(' ');
    match (parts.next(), parts.next(), parts.next().and_then(|s| s.parse::<u64>().ok())) {
        (Some(oid), Some(_kind), Some(size)) => Ok(Info::Found { oid: oid.to_string(), size }),
        _ => Err(GitError::ParseError { what: "cat-file --batch-command".into(), detail: line.to_string() }),
    }
}

impl CatFile {
    /// The bytes of the object `spec` names (`<commit>:<path>`, `:0:<path>`, an oid), or None
    /// when Git has no such object. Fails with `tooLarge` when it is bigger than `limit`, without
    /// reading it. A helper that has died is started again once.
    pub async fn read(self: &Arc<Self>, client: &GitClient, cancel: &CancellationToken, spec: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        if spec.is_empty() || spec.contains(['\n', '\r', '\0']) {
            return Err(GitError::InvalidPath { path: spec.to_string(), reason: "not a valid object name".into() });
        }
        let mut slot = self.helper.lock().await;
        *self.last_used.lock().expect("last used lock") = Instant::now();
        for attempt in 0..2 {
            if slot.is_none() {
                *slot = Some(self.spawn(client, cancel).await?);
            }
            let helper = slot.as_mut().expect("just spawned");
            let started = self.verbose.load(Ordering::Relaxed).then(|| log_request(client, spec));
            match request(helper, spec, limit).await {
                Ok(r) => {
                    if let Some(t) = started {
                        let size = r.as_ref().map_or(0, Vec::len);
                        client.console.finish(t, Some(&Output { exit_code: Some(0), stdout: format!("{size} bytes").into_bytes(), stderr: Vec::new() }), None);
                    }
                    return Ok(r);
                }
                Err(e @ GitError::TooLarge { .. }) => return Err(e),
                Err(e) => {
                    if let Some(t) = started {
                        client.console.finish(t, None, Some(&e));
                    }
                    let dead = slot.take().expect("helper present");
                    stop(client, dead).await;
                    if attempt == 1 {
                        return Err(e);
                    }
                }
            }
        }
        unreachable!("the loop returns on its second attempt")
    }

    /// Which of `oids` (full hex) Git does not have, with one `info` request for each and one
    /// flush for all of them. A helper that has died is started again once.
    pub async fn missing(self: &Arc<Self>, client: &GitClient, cancel: &CancellationToken, oids: &[String]) -> Result<Vec<String>> {
        if oids.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(bad) = oids.iter().find(|o| o.is_empty() || !o.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(GitError::InvalidPath { path: bad.clone(), reason: "not an object id".into() });
        }
        let mut slot = self.helper.lock().await;
        *self.last_used.lock().expect("last used lock") = Instant::now();
        for attempt in 0..2 {
            if slot.is_none() {
                *slot = Some(self.spawn(client, cancel).await?);
            }
            let helper = slot.as_mut().expect("just spawned");
            match request_missing(helper, oids).await {
                Ok(m) => return Ok(m),
                Err(e) => {
                    let dead = slot.take().expect("helper present");
                    stop(client, dead).await;
                    if attempt == 1 {
                        return Err(e);
                    }
                }
            }
        }
        unreachable!("the loop returns on its second attempt")
    }

    async fn spawn(self: &Arc<Self>, client: &GitClient, cancel: &CancellationToken) -> Result<Helper> {
        let mut cmd = client.build_cmd(&GitReq::new("cat-file", ["cat-file", "--batch-command", "--buffer"]));
        cmd.stdin = StdinMode::Piped;
        cmd.timeout = None;
        let argv = client.transport.render(&cmd);
        let ticket = client.console.start(&cmd, argv);
        let mut process = match client.transport.spawn(cmd).await {
            Ok(p) => p,
            Err(e) => {
                client.console.finish(ticket, None, Some(&e));
                return Err(e);
            }
        };
        let (Some(stdin), Some(stdout)) = (process.stdin(), process.stdout()) else {
            let e = GitError::Internal { detail: "cat-file started without its pipes".into() };
            client.console.finish(ticket, None, Some(&e));
            return Err(e);
        };
        tracing::info!("cat-file helper started for {}", client.repo.0);
        self.watch_idle(client.clone(), cancel.clone());
        Ok(Helper { process, stdin, stdout: BufReader::new(stdout), ticket: Some(ticket) })
    }

    /// Stops the helper after `IDLE` without a request, or when the repository closes.
    fn watch_idle(self: &Arc<Self>, client: GitClient, cancel: CancellationToken) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let idle_for = me.last_used.lock().expect("last used lock").elapsed();
                let wait = IDLE.saturating_sub(idle_for).max(Duration::from_secs(1));
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(wait) => {}
                }
                if me.last_used.lock().expect("last used lock").elapsed() >= IDLE {
                    break;
                }
            }
            if let Some(h) = me.helper.lock().await.take() {
                stop(&client, h).await;
            }
        });
    }

    /// Stops the helper if it is running.
    pub async fn shutdown(&self, client: &GitClient) {
        if let Some(h) = self.helper.lock().await.take() {
            stop(client, h).await;
        }
    }
}

/// A console entry for one request, shown only with `console.verboseHelpers` on.
fn log_request(client: &GitClient, spec: &str) -> ConsoleTicket {
    let mut cmd = client.build_cmd(&GitReq::new("cat-file", ["cat-file", "--batch-command"]));
    cmd.summary = Some(vec!["cat-file".into(), "contents".into(), spec.to_string()]);
    let argv = vec!["cat-file".into(), "contents".into(), spec.to_string()];
    client.console.start(&cmd, argv)
}

/// Closes the helper's stdin, which makes `cat-file` exit, and finishes its console entry.
async fn stop(client: &GitClient, mut h: Helper) {
    let _ = h.stdin.shutdown().await;
    drop(h.stdin);
    let code = match tokio::time::timeout(Duration::from_secs(2), h.process.wait()).await {
        Ok(Ok(code)) => code,
        _ => {
            let _ = h.process.kill().await;
            None
        }
    };
    if let Some(t) = h.ticket.take() {
        client.console.finish(t, Some(&Output { exit_code: code, stdout: Vec::new(), stderr: Vec::new() }), None);
    }
    tracing::info!("cat-file helper stopped for {}", client.repo.0);
}

/// One `info` line for each oid, answered in order, one line each.
async fn request_missing(h: &mut Helper, oids: &[String]) -> Result<Vec<String>> {
    let io = |e: std::io::Error| GitError::Io { detail: format!("cat-file helper: {e}") };
    let mut lines = String::new();
    for oid in oids {
        lines.push_str("info ");
        lines.push_str(oid);
        lines.push('\n');
    }
    lines.push_str("flush\n");
    h.stdin.write_all(lines.as_bytes()).await.map_err(io)?;
    h.stdin.flush().await.map_err(io)?;
    let mut missing = Vec::new();
    let mut line = String::new();
    for oid in oids {
        line.clear();
        if h.stdout.read_line(&mut line).await.map_err(io)? == 0 {
            return Err(GitError::Io { detail: "cat-file helper exited".into() });
        }
        if parse_info(&line)? == Info::Missing {
            missing.push(oid.clone());
        }
    }
    Ok(missing)
}

/// `info` first, so an object over `limit` is refused without being read, then `contents`.
async fn request(h: &mut Helper, spec: &str, limit: usize) -> Result<Option<Vec<u8>>> {
    let io = |e: std::io::Error| GitError::Io { detail: format!("cat-file helper: {e}") };
    h.stdin.write_all(format!("info {spec}\nflush\n").as_bytes()).await.map_err(io)?;
    h.stdin.flush().await.map_err(io)?;
    let mut line = String::new();
    if h.stdout.read_line(&mut line).await.map_err(io)? == 0 {
        return Err(GitError::Io { detail: "cat-file helper exited".into() });
    }
    let (oid, size) = match parse_info(&line)? {
        Info::Missing => return Ok(None),
        Info::Found { oid, size } => (oid, size),
    };
    if size > limit as u64 {
        return Err(GitError::TooLarge { bytes: size, limit: limit as u64 });
    }
    h.stdin.write_all(format!("contents {oid}\nflush\n").as_bytes()).await.map_err(io)?;
    h.stdin.flush().await.map_err(io)?;
    line.clear();
    if h.stdout.read_line(&mut line).await.map_err(io)? == 0 {
        return Err(GitError::Io { detail: "cat-file helper exited".into() });
    }
    let Info::Found { size, .. } = parse_info(&line)? else {
        return Ok(None);
    };
    let mut body = vec![0u8; size as usize + 1];
    h.stdout.read_exact(&mut body).await.map_err(io)?;
    // Each object ends with a newline that is not part of it.
    body.pop();
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn round_trip(transport: crate::transport::SharedTransport, repo: &std::path::Path) {
        use crate::git::console::ConsoleRecorder;
        use crate::git::versions::{Capabilities, GitVersion};
        use crate::transport::paths::NativePath;
        let client = GitClient {
            transport,
            repo: NativePath(repo.to_string_lossy().into_owned()),
            caps: Capabilities::for_version(GitVersion { major: 2, minor: 43, patch: 0 }),
            console: Arc::new(ConsoleRecorder::new(None, "catfile".into(), 4096)),
        };
        let cat = Arc::new(CatFile::new(true));
        let cancel = CancellationToken::new();
        assert_eq!(cat.read(&client, &cancel, "HEAD:a.txt", 1024).await.unwrap().as_deref(), Some(&b"one\ntwo\n"[..]));
        assert_eq!(cat.read(&client, &cancel, "HEAD:b.bin", 1024).await.unwrap().as_deref(), Some(&b"\0\x01\n\n"[..]));
        assert_eq!(cat.read(&client, &cancel, "HEAD:nope.txt", 1024).await.unwrap(), None);
        assert!(matches!(cat.read(&client, &cancel, "HEAD:a.txt", 4).await, Err(GitError::TooLarge { bytes: 8, limit: 4 })));
        assert!(matches!(cat.read(&client, &cancel, "a\nb", 4).await, Err(GitError::InvalidPath { .. })));
        // Several oids checked at once: only the one Git lacks comes back.
        let head = std::process::Command::new("git").arg("-C").arg(repo).args(["rev-parse", "HEAD"]).output().unwrap();
        let head = String::from_utf8(head.stdout).unwrap().trim().to_string();
        let absent = "0123456789012345678901234567890123456789".to_string();
        let asked = [head.clone(), absent.clone(), head];
        assert_eq!(cat.missing(&client, &cancel, &asked).await.unwrap(), vec![absent]);
        assert!(matches!(cat.missing(&client, &cancel, &["HEAD:a.txt".into()]).await, Err(GitError::InvalidPath { .. })));
        // A helper that died is started again on the next read.
        cat.helper.lock().await.as_mut().unwrap().process.kill().await.unwrap();
        assert_eq!(cat.read(&client, &cancel, "HEAD:a.txt", 1024).await.unwrap().as_deref(), Some(&b"one\ntwo\n"[..]));
        cat.shutdown(&client).await;
        assert!(cat.helper.lock().await.is_none());
        cancel.cancel();
    }

    #[tokio::test]
    async fn reads_blobs_through_one_process() {
        use crate::transport::local::LocalTransport;
        use crate::transport::paths::{IdentityMapper, WslMapper};
        use crate::transport::wsl::WslTransport;
        if which::which("git").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("stashpanda-catfile-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let st = std::process::Command::new("git").arg("-C").arg(&dir).args(["-c", "user.email=t@t", "-c", "user.name=T", "-c", "commit.gpgsign=false"]).args(args).status().unwrap();
            assert!(st.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        // Ends in a blank line, which must not be confused with the newline after each object.
        std::fs::write(dir.join("b.bin"), b"\0\x01\n\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        let local: crate::transport::SharedTransport = Arc::new(LocalTransport { git: which::which("git").unwrap(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false });
        round_trip(local, &dir).await;
        // The same through wsl.exe, as the Windows build runs it; only inside WSL with interop.
        if let Ok(distro) = std::env::var("WSL_DISTRO_NAME") {
            if std::path::Path::new("/mnt/c/Windows/System32/wsl.exe").exists() {
                let wsl: crate::transport::SharedTransport =
                    Arc::new(WslTransport { distro: distro.clone(), git: "git".into(), mapper: WslMapper { distro, automount_root: "/mnt/".into() } });
                round_trip(wsl, &dir).await;
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_info_lines() {
        assert_eq!(parse_info("abc123 blob 42\n").unwrap(), Info::Found { oid: "abc123".into(), size: 42 });
        assert_eq!(parse_info("HEAD:nope missing\n").unwrap(), Info::Missing);
        assert_eq!(parse_info("ab ambiguous\n").unwrap(), Info::Missing);
        assert!(parse_info("garbage").is_err());
    }
}
