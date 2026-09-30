//! Run an executable directly on the machine StashPanda runs on. Also used for
//! Windows `git.exe` invoked from inside WSL through interop.

use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::{Child, Command};

use super::paths::{NativePath, PathMapper};
use super::{Cmd, Process, Program, StdinMode, Transport, TransportInfo, TransportKind};
use crate::error::{GitError, Result};
use crate::git::versions::{GitVersion, MIN_GIT};

pub struct LocalTransport {
    pub git: PathBuf,
    pub mapper: Box<dyn PathMapper>,
    /// True when `git` is a Windows executable reached from Linux; the
    /// resolver uses it to phrase warnings, the transport treats it the same.
    pub windows_git_from_linux: bool,
}

pub struct TokioProcess {
    pub child: Child,
}

#[async_trait]
impl Process for TokioProcess {
    fn stdin(&mut self) -> Option<Box<dyn AsyncWrite + Send + Unpin>> {
        self.child.stdin.take().map(|s| Box::new(s) as _)
    }
    fn stdout(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        self.child.stdout.take().map(|s| Box::new(s) as _)
    }
    fn stderr(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        self.child.stderr.take().map(|s| Box::new(s) as _)
    }
    async fn wait(&mut self) -> Result<Option<i32>> {
        let status = self.child.wait().await?;
        Ok(status.code())
    }
    async fn kill(&mut self) -> Result<()> {
        self.child.kill().await?;
        Ok(())
    }
}

/// Build a `tokio::process::Command` that never shows a console window on
/// Windows and never inherits stdio (a GUI process has no console handles).
pub fn base_command(program: &str) -> Command {
    let mut c = Command::new(program);
    c.kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

pub fn apply_stdin(c: &mut Command, mode: &StdinMode) {
    match mode {
        StdinMode::Null => c.stdin(Stdio::null()),
        StdinMode::Piped | StdinMode::Bytes(_) => c.stdin(Stdio::piped()),
    };
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
}

impl LocalTransport {
    fn program_path(&self, p: &Program) -> String {
        match p {
            Program::Git => self.git.to_string_lossy().into_owned(),
            Program::Other(s) => s.clone(),
        }
    }
}

#[async_trait]
impl Transport for LocalTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Local
    }
    fn paths(&self) -> &dyn PathMapper {
        self.mapper.as_ref()
    }
    fn render(&self, cmd: &Cmd) -> Vec<String> {
        let mut v = Vec::with_capacity(cmd.args.len() + 1);
        v.push(self.program_path(&cmd.program));
        v.extend(cmd.args.iter().cloned());
        v
    }
    async fn spawn(&self, cmd: Cmd) -> Result<Box<dyn Process>> {
        let program = self.program_path(&cmd.program);
        let mut c = base_command(&program);
        c.args(&cmd.args);
        for (k, v) in &cmd.env {
            c.env(k, v);
        }
        apply_stdin(&mut c, &cmd.stdin);
        let child = c.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::GitNotFound {
                    searched: vec![program.clone()],
                }
            } else {
                GitError::Io {
                    detail: format!("could not start {program}: {e}"),
                }
            }
        })?;
        Ok(Box::new(TokioProcess { child }))
    }
    async fn probe(&self) -> Result<TransportInfo> {
        let out = self
            .run(Cmd::git("version", vec!["--version".into()]))
            .await?;
        if !out.success() {
            return Err(GitError::CommandFailed {
                label: "version".into(),
                argv: self.render(&Cmd::git("version", vec!["--version".into()])),
                exit_code: out.exit_code,
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        let git_version = GitVersion::parse(&out.stdout)?;
        if git_version < MIN_GIT {
            return Err(GitError::GitVersionTooOld {
                found: git_version.to_string(),
                required: MIN_GIT.to_string(),
            });
        }
        Ok(TransportInfo {
            git_version,
            git_executable: self.git.to_string_lossy().into_owned(),
            warnings: Vec::new(),
        })
    }
    async fn read_file(&self, path: &NativePath, limit: usize) -> Result<Vec<u8>> {
        // The native path is also a host path for Local, except for Windows
        // git driven from Linux, where the host view is the /mnt path.
        let host = self.mapper.to_host(path)?;
        let meta = tokio::fs::metadata(&host.0).await?;
        if meta.len() as usize > limit {
            return Err(GitError::TooLarge {
                bytes: meta.len(),
                limit: limit as u64,
            });
        }
        Ok(tokio::fs::read(&host.0).await?)
    }
    async fn remove_file(&self, path: &NativePath) -> Result<()> {
        let host = self.mapper.to_host(path)?;
        match tokio::fs::remove_file(&host.0).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
    async fn modified_times(&self, paths: &[NativePath]) -> Result<Vec<Option<i64>>> {
        super::host_modified_times(self.mapper.as_ref(), paths).await
    }
}
