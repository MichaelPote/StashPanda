//! Reach a Git inside a WSL distro from the Windows build through `wsl.exe`.
//!
//! Always the `-e` form (no login shell). Every command costs one `wsl.exe`
//! start (about 87 ms measured), so callers batch where they can.

use std::process::Stdio;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use super::local::{apply_stdin, base_command, TokioProcess};
use super::paths::{NativePath, PathMapper, WslMapper};
use super::{Cmd, PidLine, Process, Program, Transport, TransportInfo, TransportKind};
use crate::error::{GitError, Result};
use crate::git::versions::{GitVersion, MIN_GIT};

pub const WSL_EXE: &str = r"C:\Windows\System32\wsl.exe";

/// `wsl.exe` as this build starts it. Tests on Linux reach it through WSL interop, so the
/// transport's own code runs against a real distro there too.
fn wsl_exe() -> &'static str {
    if cfg!(all(test, not(windows))) {
        "/mnt/c/Windows/System32/wsl.exe"
    } else {
        WSL_EXE
    }
}

/// How a command with `Cmd::own_session` starts inside the distro: `setsid -w` makes it lead a
/// new session with no terminal and waits for it, so `wsl.exe` exits when it does; the shell
/// prints its pid, which `exec` hands to the command, and that pid is also the process group
/// every process it starts joins.
const OWN_SESSION_WRAPPER: [&str; 5] = ["setsid", "-w", "sh", "-c", r#"echo $$; exec "$@""#];

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
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// `wsl.exe -l -q` output as distro names.
pub async fn list_distros() -> Result<Vec<String>> {
    list_with(&["-l", "-q"]).await
}

/// The distros running now. Reading a stopped distro's files through `\\wsl.localhost` starts
/// it, so the SSH key scan only looks in these. With none running, `wsl.exe` prints a sentence
/// and exits non-zero, which gives an empty list.
pub async fn list_running_distros() -> Result<Vec<String>> {
    let mut c = base_command(wsl_exe());
    c.args(["-l", "--running", "-q"]);
    let out = c.output().await.map_err(|e| GitError::Io { detail: e.to_string() })?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(distro_lines(&decode_wsl_text(&out.stdout)))
}

async fn list_with(args: &[&str]) -> Result<Vec<String>> {
    let mut c = base_command(wsl_exe());
    c.args(args);
    let out = c.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::WslNotAvailable
        } else {
            GitError::Io { detail: e.to_string() }
        }
    })?;
    Ok(distro_lines(&decode_wsl_text(&out.stdout)))
}

fn distro_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_matches(|c: char| c == '\0' || c == '\r' || c.is_whitespace()).to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

impl WslTransport {
    fn argv(&self, cmd: &Cmd) -> Vec<String> {
        let mut v = vec![wsl_exe().to_string(), "-d".into(), self.distro.clone(), "-e".into()];
        if cmd.own_session {
            v.extend(OWN_SESSION_WRAPPER.map(String::from));
            // `$0` for the script.
            v.push("sh".into());
        }
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
        let process = TokioProcess::new(child);
        if !cmd.own_session {
            return Ok(Box::new(process));
        }
        Ok(Box::new(WslProcess { wsl: process, distro: self.distro.clone(), pid: Arc::new(OnceLock::new()), exited: false }))
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
    async fn modified_times(&self, paths: &[NativePath]) -> Result<Vec<Option<i64>>> {
        // Through the UNC view, like read_file: no wsl.exe spawn.
        super::host_modified_times(&self.mapper, paths).await
    }
    async fn list_dir(&self, path: &NativePath) -> Result<Option<Vec<String>>> {
        // Through the UNC view, like read_file.
        super::host_list_dir(&self.mapper, path).await
    }
    async fn remove_dir_all(&self, path: &NativePath) -> Result<()> {
        super::host_remove_dir_all(&self.mapper, path).await
    }
}

#[allow(dead_code)]
/// A command started with `Cmd::own_session`. Killing `wsl.exe` would leave the Linux process
/// running, so terminate and kill signal its process group inside the distro instead, through a
/// second `wsl.exe`, using the pid the wrapper printed first on stdout.
///
/// Killing `wsl.exe` alone ends the processes it started directly (measured 2026-10-01), but not
/// one in a session of its own, which is what the wrapper creates. So dropping a `WslProcess`
/// that has not exited kills its group as well.
struct WslProcess {
    wsl: TokioProcess,
    distro: String,
    /// Known once stdout has been read past the wrapper's first line.
    pid: Arc<OnceLock<u32>>,
    exited: bool,
}

/// How long `terminate` waits for the wrapper's pid line. It is the first thing the wrapper
/// prints, so only a stop in the first moments of the command waits at all.
const PID_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// `kill -<signal> -- -<pid>` inside the distro.
async fn signal_wsl_group(distro: &str, pid: u32, signal: &str) -> Result<()> {
    let mut c = base_command(wsl_exe());
    c.args(["-d", distro, "-e", "kill", &format!("-{signal}"), "--", &format!("-{pid}")]);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    c.status().await?;
    Ok(())
}

impl Drop for WslProcess {
    fn drop(&mut self) {
        if self.exited {
            return;
        }
        if let Some(pid) = self.pid.get().copied() {
            let distro = self.distro.clone();
            tauri::async_runtime::spawn(async move {
                let _ = signal_wsl_group(&distro, pid, "KILL").await;
            });
        }
    }
}

#[async_trait]
impl Process for WslProcess {
    fn stdin(&mut self) -> Option<Box<dyn AsyncWrite + Send + Unpin>> {
        self.wsl.stdin()
    }
    fn stdout(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        let pid = self.pid.clone();
        self.wsl.stdout().map(|r| Box::new(PidLine::new(r, pid)) as _)
    }
    fn stderr(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        self.wsl.stderr()
    }
    async fn wait(&mut self) -> Result<Option<i32>> {
        let code = self.wsl.wait().await?;
        self.exited = true;
        Ok(code)
    }
    async fn kill(&mut self) -> Result<()> {
        if let Some(pid) = self.pid.get().copied() {
            let _ = signal_wsl_group(&self.distro, pid, "KILL").await;
        }
        self.wsl.kill().await
    }
    async fn terminate(&mut self) -> Result<()> {
        // The caller keeps reading stdout meanwhile, which is where the pid arrives.
        let deadline = std::time::Instant::now() + PID_WAIT;
        while self.pid.get().is_none() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        match self.pid.get().copied() {
            Some(pid) => signal_wsl_group(&self.distro, pid, "TERM").await,
            // The wrapper never printed (no `setsid` in the distro, say), so nothing runs in a
            // session of its own and killing `wsl.exe` ends it.
            None => self.wsl.kill().await,
        }
    }
    fn remote_pid(&self) -> Option<u32> {
        self.pid.get().copied()
    }
}

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
