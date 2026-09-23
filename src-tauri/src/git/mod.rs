//! `GitClient`: the single place a Git request becomes a process.

pub mod console;
pub mod ops;
pub mod parse;
pub mod queries;
pub mod versions;

use std::sync::Arc;
use std::time::Duration;

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
        }
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
}

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
                    argv,
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
}

#[allow(dead_code)]
fn _unused(_: GitError) {}
