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
