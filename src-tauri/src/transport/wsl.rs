//! Reach a Git inside a WSL distro from the Windows build through `wsl.exe`.
//!
//! Always the `-e` form (no login shell). Every command costs one `wsl.exe`
//! start (about 87 ms measured), so callers batch where they can.

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use super::local::{apply_stdin, base_command, TokioProcess};
use super::paths::{NativePath, PathMapper, WslMapper};
use super::{Cmd, Process, Program, Transport, TransportInfo, TransportKind};
use crate::error::{GitError, Result};
use crate::git::versions::{GitVersion, MIN_GIT};

pub const WSL_EXE: &str = r"C:\Windows\System32\wsl.exe";

pub struct WslTransport {
    pub distro: String,
    /// Git inside the distro, usually just `git`.
    pub git: String,
    pub mapper: WslMapper,
}

/// Decode `wsl.exe`'s own messages, which may be UTF-16LE.
pub fn decode_wsl_text(bytes: &[u8]) -> String {
    let looks_utf16 = bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).take(16).all(|b| *b == 0);
    if looks_utf16 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// `wsl.exe -l -q` output as distro names.
pub async fn list_distros() -> Result<Vec<String>> {
    let mut c = base_command(WSL_EXE);
    c.args(["-l", "-q"]);
    let out = c.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::WslNotAvailable
        } else {
            GitError::Io { detail: e.to_string() }
        }
    })?;
    let text = decode_wsl_text(&out.stdout);
    Ok(text
        .lines()
        .map(|l| l.trim_matches(|c: char| c == '\0' || c == '\r' || c.is_whitespace()).to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

impl WslTransport {
    fn argv(&self, cmd: &Cmd) -> Vec<String> {
        let mut v = vec![WSL_EXE.to_string(), "-d".into(), self.distro.clone(), "-e".into()];
        if !cmd.env.is_empty() {
            v.push("env".into());
            for (k, val) in &cmd.env {
                v.push(format!("{k}={val}"));
            }
        }
        v.push(match &cmd.program {
            Program::Git => self.git.clone(),
            Program::Other(p) => p.clone(),
        });
        v.extend(cmd.args.iter().cloned());
        v
    }
}

#[async_trait]
impl Transport for WslTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Wsl {
            distro: self.distro.clone(),
        }
    }
    fn paths(&self) -> &dyn PathMapper {
        &self.mapper
    }
    fn render(&self, cmd: &Cmd) -> Vec<String> {
        self.argv(cmd)
    }
    async fn spawn(&self, cmd: Cmd) -> Result<Box<dyn Process>> {
        let argv = self.argv(&cmd);
        let mut c = base_command(&argv[0]);
        c.args(&argv[1..]);
        apply_stdin(&mut c, &cmd.stdin);
        let child = c.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::WslNotAvailable
            } else {
                GitError::Io { detail: e.to_string() }
            }
        })?;
        Ok(Box::new(TokioProcess { child }))
    }
    async fn probe(&self) -> Result<TransportInfo> {
        let cmd = Cmd::git("version", vec!["--version".into()]);
        let out = self.run(cmd.clone()).await?;
        if out.exit_code == Some(255) || out.exit_code == Some(-1) {
            let msg = decode_wsl_text(&out.stderr) + &decode_wsl_text(&out.stdout);
            if msg.to_ascii_lowercase().contains("no distribution") || msg.contains("Wsl/Service/") {
                let available = list_distros().await.unwrap_or_default();
                return Err(GitError::WslDistroNotFound {
                    distro: self.distro.clone(),
                    available,
                });
            }
        }
        if !out.success() {
            return Err(GitError::CommandFailed {
                label: "version".into(),
                argv: self.render(&cmd),
                exit_code: out.exit_code,
                stderr: decode_wsl_text(&out.stderr),
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
            git_executable: format!("wsl.exe -d {} -e {}", self.distro, self.git),
            warnings: Vec::new(),
        })
    }
    async fn read_file(&self, path: &NativePath, limit: usize) -> Result<Vec<u8>> {
        // The UNC view is readable from Windows and avoids a wsl.exe spawn.
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
        // Through the UNC view, like read_file.
        let host = self.mapper.to_host(path)?;
        match tokio::fs::remove_file(&host.0).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[allow(dead_code)]
fn _assert_types(_: &dyn AsyncRead, _: &dyn AsyncWrite) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf16_and_utf8() {
        let utf16: Vec<u8> = "Ubuntu\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_wsl_text(&utf16), "Ubuntu\r\n");
        assert_eq!(decode_wsl_text(b"plain"), "plain");
    }
}
