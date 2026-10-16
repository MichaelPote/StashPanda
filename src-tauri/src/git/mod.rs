//! `GitClient`: the single place a Git request becomes a process.

pub mod catfile;
pub mod console;
pub mod ops;
pub mod parse;
pub mod queries;
pub mod versions;

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::error::{classify_git_failure, GitError, Result};
use crate::transport::paths::NativePath;
use crate::transport::{Category, Cmd, Output, SharedTransport};
use console::ConsoleRecorder;
use versions::Capabilities;

/// A Git subcommand request. `args` start at the subcommand (`status`, ...).
pub struct GitReq {
    pub label: &'static str,
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    pub category: Category,
    pub timeout: Option<Duration>,
    /// Extra `-c key=value` config for this call only.
    pub config: Vec<(String, String)>,
    /// See `Cmd::own_session`.
    pub own_session: bool,
    /// Environment variables for this call only, after the ones every call carries.
    pub env: Vec<(String, String)>,
}

impl GitReq {
    pub fn new<I, S>(label: &'static str, args: I) -> GitReq
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        GitReq {
            label,
            args: args.into_iter().map(Into::into).collect(),
            stdin: None,
            category: Category::Internal,
            timeout: Some(Duration::from_secs(120)),
            config: Vec::new(),
            own_session: false,
            env: Vec::new(),
        }
    }
    /// Run in a session of its own, so cancelling reaches Git's children and Git has no terminal.
    pub fn own_session(mut self) -> GitReq {
        self.own_session = true;
        self
    }
    pub fn user(mut self) -> GitReq {
        self.category = Category::UserAction;
        self
    }
    pub fn watcher(mut self) -> GitReq {
        self.category = Category::Watcher;
        self
    }
    pub fn stdin(mut self, bytes: Vec<u8>) -> GitReq {
        self.stdin = Some(bytes);
        self
    }
    pub fn timeout(mut self, t: Option<Duration>) -> GitReq {
        self.timeout = t;
        self
    }
    pub fn env(mut self, key: &str, value: &str) -> GitReq {
        self.env.push((key.to_string(), value.to_string()));
        self
    }
}

#[derive(Clone)]
pub struct GitClient {
    pub transport: SharedTransport,
    pub repo: NativePath,
    pub caps: Capabilities,
    pub console: Arc<ConsoleRecorder>,
}

/// Flags every invocation carries. `--no-optional-locks` keeps `status` from
/// rewriting the index, which would trip our own watcher.
fn common_prefix(repo: &NativePath, extra_config: &[(String, String)]) -> Vec<String> {
    let mut v = vec![
        "--no-optional-locks".to_string(),
        "-c".into(),
        "color.ui=never".into(),
        "-c".into(),
        "core.quotepath=false".into(),
        "-c".into(),
        "i18n.logOutputEncoding=utf-8".into(),
    ];
    for (k, val) in extra_config {
        v.push("-c".into());
        v.push(format!("{k}={val}"));
    }
    v.push("-C".into());
    v.push(repo.0.clone());
    v
}

fn common_env() -> Vec<(String, String)> {
    vec![
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ("LC_ALL".into(), "C.UTF-8".into()),
        ("GIT_OPTIONAL_LOCKS".into(), "0".into()),
    ]
}

impl GitClient {
    pub fn build_cmd(&self, req: &GitReq) -> Cmd {
        let mut args = common_prefix(&self.repo, &req.config);
        args.extend(req.args.iter().cloned());
        let mut cmd = Cmd::git(req.label, args)
            .category(req.category)
            .timeout(req.timeout);
        cmd.env = common_env();
        cmd.env.extend(req.env.iter().cloned());
        cmd.own_session = req.own_session;
        cmd.summary = Some(req.args.clone());
        if let Some(b) = &req.stdin {
            cmd = cmd.stdin_bytes(b.clone());
        }
        cmd
    }

    /// Run to completion. A non-zero exit becomes a classified `GitError`.
    pub async fn exec(&self, req: GitReq) -> Result<Output> {
        let cmd = self.build_cmd(&req);
        let argv = self.transport.render(&cmd);
        let ticket = self.console.start(&cmd, argv.clone());
        match self.transport.run(cmd).await {
            Ok(out) if out.success() => {
                self.console.finish(ticket, Some(&out), None);
                Ok(out)
            }
            Ok(out) => {
                let err = classify_git_failure(
                    req.label,
                    argv.iter().map(|a| console::hide_passwords(a)).collect(),
                    out.exit_code,
                    &out.stderr,
                    &self.transport.kind(),
                );
                self.console.finish(ticket, Some(&out), Some(&err));
                Err(err)
            }
            Err(err) => {
                self.console.finish(ticket, None, Some(&err));
                Err(err)
            }
        }
    }

    /// Like `exec` but a non-zero exit is returned as data, for commands where
    /// exit 1 is a normal answer (`diff --quiet`, `rev-parse --verify`).
    pub async fn exec_allow_failure(&self, req: GitReq) -> Result<Output> {
        let cmd = self.build_cmd(&req);
        let argv = self.transport.render(&cmd);
        let ticket = self.console.start(&cmd, argv);
        match self.transport.run(cmd).await {
            Ok(out) => {
                self.console.finish(ticket, Some(&out), None);
                Ok(out)
            }
            Err(err) => {
                self.console.finish(ticket, None, Some(&err));
                Err(err)
            }
        }
    }

    /// Like `exec_allow_failure` for several commands at once, through `Transport::run_batch`
    /// (one process start over WSL). Each command is logged in the console on its own, with the
    /// batch's duration.
    pub async fn exec_batch(&self, reqs: Vec<GitReq>) -> Result<Vec<Output>> {
        let cmds: Vec<Cmd> = reqs.iter().map(|r| self.build_cmd(r)).collect();
        let tickets: Vec<_> = cmds.iter().map(|c| self.console.start(c, self.transport.render(c))).collect();
        match self.transport.run_batch(cmds).await {
            Ok(outs) => {
                for (t, out) in tickets.into_iter().zip(&outs) {
                    self.console.finish(t, Some(out), None);
                }
                Ok(outs)
            }
            Err(err) => {
                for t in tickets {
                    self.console.finish(t, None, Some(&err));
                }
                Err(err)
            }
        }
    }

    /// Like `exec_allow_failure` for a command that reports progress on stderr: each update goes
    /// to `progress` as it arrives, and cancelling `cancel` kills the process (see
    /// `Transport::run_streaming`).
    pub async fn exec_streaming(&self, req: GitReq, progress: &(dyn Fn(String) + Send + Sync), cancel: &CancellationToken) -> Result<Output> {
        let cmd = self.build_cmd(&req);
        let argv = self.transport.render(&cmd);
        let ticket = self.console.start(&cmd, argv);
        match self.transport.run_streaming(cmd, progress, cancel).await {
            Ok(out) => {
                self.console.finish(ticket, Some(&out), None);
                Ok(out)
            }
            Err(err) => {
                self.console.finish(ticket, None, Some(&err));
                Err(err)
            }
        }
    }

    /// Like `exec_allow_failure`, but standard output goes to `on_stdout` piece by piece as it
    /// arrives instead of being collected, so the caller can work on the first records of a long
    /// `git log` while Git is still walking. The returned `Output` holds no standard output.
    /// (`exec_streaming` is the other way round: it streams standard error, for progress.)
    pub async fn exec_reading_stdout(&self, req: GitReq, on_stdout: &mut (dyn FnMut(&[u8]) + Send)) -> Result<Output> {
        let cmd = self.build_cmd(&req);
        let argv = self.transport.render(&cmd);
        let ticket = self.console.start(&cmd, argv);
        let mut stdout_bytes = 0u64;
        let result = self.read_stdout(cmd, req.timeout, &mut stdout_bytes, on_stdout).await;
        match result {
            Ok(out) => {
                self.console.finish_streamed(ticket, Some(&out), stdout_bytes, None);
                Ok(out)
            }
            Err(err) => {
                self.console.finish_streamed(ticket, None, stdout_bytes, Some(&err));
                Err(err)
            }
        }
    }

    async fn read_stdout(
        &self,
        cmd: Cmd,
        timeout: Option<Duration>,
        stdout_bytes: &mut u64,
        on_stdout: &mut (dyn FnMut(&[u8]) + Send),
    ) -> Result<Output> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let stdin_bytes = match &cmd.stdin {
            crate::transport::StdinMode::Bytes(b) => Some(b.clone()),
            _ => None,
        };
        let mut child = self.transport.spawn(cmd).await?;
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
                if let Some(mut r) = stdout.take() {
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        let n = r.read(&mut buf).await?;
                        if n == 0 {
                            break;
                        }
                        *stdout_bytes += n as u64;
                        on_stdout(&buf[..n]);
                    }
                }
                Ok::<_, std::io::Error>(())
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
            o?;
            let stderr = e?;
            let exit_code = child.wait().await?;
            Ok::<Output, GitError>(Output { exit_code, stdout: Vec::new(), stderr })
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

#[allow(dead_code)]
fn _unused(_: GitError) {}
