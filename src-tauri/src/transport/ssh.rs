//! SSH transport over the in-process `russh` client: one authenticated
//! connection per host, one channel per command. No `ssh.exe`, so Windows and
//! Linux behave identically and the Windows ControlMaster gap does not exist.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russh::client::Msg;
use russh::{Channel, ChannelMsg, Sig};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{oneshot, Notify};

use super::paths::{NativePath, PathMapper, SshMapper};
use super::ssh_pool::{SshContext, SshTarget};
use super::{Cmd, Process, Program, Transport, TransportInfo, TransportKind};
use crate::error::{GitError, Result};
use crate::git::versions::{GitVersion, MIN_GIT};

pub struct SshTransport {
    pub target: SshTarget,
    pub ctx: Arc<SshContext>,
    /// `git` on the remote unless the user configured a path.
    pub remote_git: String,
    mapper: SshMapper,
}

impl SshTransport {
    pub fn new(target: SshTarget, remote_git: Option<String>, ctx: Arc<SshContext>) -> SshTransport {
        SshTransport {
            target,
            ctx,
            remote_git: remote_git.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "git".to_string()),
            mapper: SshMapper,
        }
    }

    /// The remote command line: shlex-quoted because `exec` runs through the
    /// login shell. `$$` echo lets us kill the real process later.
    fn remote_command(&self, cmd: &Cmd) -> String {
        let mut words: Vec<String> = Vec::new();
        if !cmd.env.is_empty() {
            words.push("env".into());
            for (k, v) in &cmd.env {
                words.push(format!("{k}={v}"));
            }
        }
        words.push(match &cmd.program {
            Program::Git => self.remote_git.clone(),
            Program::Other(p) => p.clone(),
        });
        words.extend(cmd.args.iter().cloned());
        shlex::try_join(words.iter().map(String::as_str)).unwrap_or_else(|_| words.join(" "))
    }
}

/// A remote process on one channel. A pump task turns channel messages into
/// stdout and stderr pipes and an exit code.
pub struct SshChannelProcess {
    stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    stdout: Option<Box<dyn AsyncRead + Send + Unpin>>,
    stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
    exit: Option<oneshot::Receiver<Option<i32>>>,
    kill: Arc<Notify>,
    term: Arc<Notify>,
}

/// `kill` sends SIGTERM and closes the channel at once, which OpenSSH's sshd answers by ending
/// the remote process and its children. `term` sends SIGTERM and keeps reading, so the exit is
/// reported only once the remote process has really gone; a server that ignores signal requests
/// leaves it running until the caller falls back to `kill`.
async fn pump(mut channel: Channel<Msg>, mut out_w: tokio::io::WriteHalf<tokio::io::SimplexStream>, mut err_w: tokio::io::WriteHalf<tokio::io::SimplexStream>, exit_tx: oneshot::Sender<Option<i32>>, kill: Arc<Notify>, term: Arc<Notify>) {
    let mut code: Option<i32> = None;
    loop {
        tokio::select! {
            _ = term.notified() => {
                let _ = channel.signal(Sig::TERM).await;
            }
            _ = kill.notified() => {
                let _ = channel.signal(Sig::TERM).await;
                let _ = channel.close().await;
                break;
            }
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        if out_w.write_all(&data).await.is_err() { break; }
                    }
                    Some(ChannelMsg::ExtendedData { data, ext }) => {
                        if ext == 1 && err_w.write_all(&data).await.is_err() { break; }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => code = Some(exit_status as i32),
                    Some(ChannelMsg::ExitSignal { .. }) => code = None,
                    Some(ChannelMsg::Eof) => {}
                    Some(ChannelMsg::Close) | None => break,
                    Some(_) => {}
                }
            }
        }
    }
    let _ = out_w.shutdown().await;
    let _ = err_w.shutdown().await;
    let _ = exit_tx.send(code);
}

#[async_trait]
impl Process for SshChannelProcess {
    fn stdin(&mut self) -> Option<Box<dyn AsyncWrite + Send + Unpin>> {
        self.stdin.take()
    }
    fn stdout(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        self.stdout.take()
    }
    fn stderr(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        self.stderr.take()
    }
    async fn wait(&mut self) -> Result<Option<i32>> {
        match self.exit.take() {
            Some(rx) => rx.await.map_err(|_| GitError::TransportDisconnected { detail: "channel closed before exit status".into() }),
            None => Ok(None),
        }
    }
    async fn kill(&mut self) -> Result<()> {
        self.kill.notify_one();
        Ok(())
    }
    async fn terminate(&mut self) -> Result<()> {
        self.term.notify_one();
        Ok(())
    }
}

#[async_trait]
impl Transport for SshTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Ssh { host: self.target.host.clone() }
    }
    fn paths(&self) -> &dyn PathMapper {
        &self.mapper
    }
    fn render(&self, cmd: &Cmd) -> Vec<String> {
        let mut v = vec!["ssh".to_string()];
        if self.target.port != 22 {
            v.push("-p".into());
            v.push(self.target.port.to_string());
        }
        v.push(format!("{}@{}", self.target.user, self.target.host));
        v.push("--".into());
        v.push(self.remote_command(cmd));
        v
    }
    async fn spawn(&self, cmd: Cmd) -> Result<Box<dyn Process>> {
        let conn = self.ctx.pool.get_or_connect(&self.target, &self.ctx).await?;
        let permit = conn.slots.clone().acquire_owned().await.map_err(|_| GitError::Cancelled)?;
        let channel = conn.open_session().await?;
        let command = self.remote_command(&cmd);
        channel.exec(true, command.as_bytes().to_vec()).await.map_err(|e| GitError::TransportDisconnected { detail: e.to_string() })?;

        let writer = channel.make_writer();
        let (out_r, out_w) = tokio::io::simplex(256 * 1024);
        let (err_r, err_w) = tokio::io::simplex(64 * 1024);
        let (exit_tx, exit_rx) = oneshot::channel();
        let kill = Arc::new(Notify::new());
        let kill2 = kill.clone();
        let term = Arc::new(Notify::new());
        let term2 = term.clone();
        tauri::async_runtime::spawn(async move {
            pump(channel, out_w, err_w, exit_tx, kill2, term2).await;
            drop(permit);
        });
        Ok(Box::new(SshChannelProcess {
            stdin: Some(Box::new(writer)),
            stdout: Some(Box::new(out_r)),
            stderr: Some(Box::new(err_r)),
            exit: Some(exit_rx),
            kill,
            term,
        }))
    }
    async fn probe(&self) -> Result<TransportInfo> {
        let cmd = Cmd::git("version", vec!["--version".into()]).timeout(Some(Duration::from_secs(30)));
        let out = self.run(cmd.clone()).await?;
        if out.exit_code == Some(127) {
            return Err(GitError::GitNotFound { searched: vec![format!("{} on {}", self.remote_git, self.target.host)] });
        }
        if !out.success() {
            return Err(GitError::CommandFailed {
                label: "version".into(),
                argv: self.render(&cmd),
                exit_code: out.exit_code,
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        let git_version = GitVersion::parse(&out.stdout)?;
        if git_version < MIN_GIT {
            return Err(GitError::GitVersionTooOld { found: git_version.to_string(), required: MIN_GIT.to_string() });
        }
        Ok(TransportInfo {
            git_version,
            git_executable: format!("{}@{}:{}", self.target.user, self.target.host, self.remote_git),
            warnings: Vec::new(),
        })
    }
    async fn read_file(&self, path: &NativePath, limit: usize) -> Result<Vec<u8>> {
        let mut cmd = Cmd::git("cat", vec!["--".into(), path.0.clone()]);
        cmd.program = Program::Other("cat".into());
        let out = self.run(cmd).await?;
        if !out.success() {
            return Err(GitError::Io { detail: String::from_utf8_lossy(&out.stderr).trim().to_string() });
        }
        if out.stdout.len() > limit {
            return Err(GitError::TooLarge { bytes: out.stdout.len() as u64, limit: limit as u64 });
        }
        Ok(out.stdout)
    }
    async fn remove_file(&self, path: &NativePath) -> Result<()> {
        let mut cmd = Cmd::git("rm", vec!["-f".into(), "--".into(), path.0.clone()]);
        cmd.program = Program::Other("rm".into());
        let out = self.run(cmd).await?;
        if !out.success() {
            return Err(GitError::Io { detail: String::from_utf8_lossy(&out.stderr).trim().to_string() });
        }
        Ok(())
    }
    async fn modified_times(&self, paths: &[NativePath]) -> Result<Vec<Option<i64>>> {
        // One line per path whatever happens: GNU stat, then BSD stat, then "-" for a file that
        // is gone. The paths are absolute, so none can be mistaken for an option.
        const SCRIPT: &str = r#"for f; do stat -c %Y -- "$f" 2>/dev/null || stat -f %m "$f" 2>/dev/null || echo -; done"#;
        let mut times = Vec::with_capacity(paths.len());
        for chunk in paths.chunks(MODIFIED_TIMES_CHUNK) {
            let mut args: Vec<String> = vec!["-c".into(), SCRIPT.into(), "sh".into()];
            args.extend(chunk.iter().map(|p| p.0.clone()));
            let mut cmd = Cmd::git("stat", args);
            cmd.program = Program::Other("sh".into());
            let out = self.run(cmd).await?;
            let text = String::from_utf8_lossy(&out.stdout);
            let mut lines = text.lines();
            for _ in chunk {
                times.push(lines.next().and_then(|l| l.trim().parse::<i64>().ok()).map(|secs| secs * 1000));
            }
        }
        Ok(times)
    }
    async fn list_dir(&self, path: &NativePath) -> Result<Option<Vec<String>>> {
        // The first line says what is at the path; for a folder, one name per line follows.
        const SCRIPT: &str = r#"if [ -d "$1" ]; then echo dir; cd -- "$1" && ls -A; elif [ -e "$1" ] || [ -L "$1" ]; then echo file; else echo missing; fi"#;
        let mut cmd = Cmd::git("ls", vec!["-c".into(), SCRIPT.into(), "sh".into(), path.0.clone()]);
        cmd.program = Program::Other("sh".into());
        let out = self.run(cmd).await?;
        if !out.success() {
            return Err(GitError::Io { detail: String::from_utf8_lossy(&out.stderr).trim().to_string() });
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        match lines.next().map(str::trim) {
            Some("dir") => Ok(Some(lines.filter(|l| !l.is_empty()).map(String::from).collect())),
            Some("file") => Err(GitError::InvalidPath { path: path.0.clone(), reason: "it is a file, not a folder".into() }),
            _ => Ok(None),
        }
    }
    async fn remove_dir_all(&self, path: &NativePath) -> Result<()> {
        let mut cmd = Cmd::git("rm", vec!["-rf".into(), "--".into(), path.0.clone()]);
        cmd.program = Program::Other("rm".into());
        let out = self.run(cmd).await?;
        if !out.success() {
            return Err(GitError::Io { detail: String::from_utf8_lossy(&out.stderr).trim().to_string() });
        }
        Ok(())
    }
    async fn shutdown(&self) {
        // Connections are shared per host and closed by the pool on exit.
    }
}

/// Paths per remote `stat` loop, which keeps each command line far below the remote limit.
const MODIFIED_TIMES_CHUNK: usize = 500;

/// End-to-end against a private `sshd` started on a free port with throwaway
/// keys, so nothing on the machine changes. Skips when sshd or ssh-keygen is
/// missing.
#[cfg(test)]
mod e2e {
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;

    use super::*;
    use crate::git::console::ConsoleRecorder;
    use crate::git::versions::Capabilities;
    use crate::git::GitClient;
    use crate::keys::secrets::PassphraseCache;
    use crate::transport::ssh_pool::{SshContext, SshTarget};
    use crate::transport::{Cmd, SharedTransport, Transport};

    struct Sshd(Child);
    impl Drop for Sshd {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn keygen(path: &PathBuf) {
        let st = Command::new("ssh-keygen").args(["-q", "-N", "", "-t", "ed25519", "-f"]).arg(path).status().unwrap();
        assert!(st.success());
    }

    fn start_sshd(dir: &Path, port: u16) -> Sshd {
        let host_key = dir.join("host_key");
        keygen(&host_key);
        let cfg = format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nUsePAM no\nStrictModes no\nPidFile none\nLogLevel ERROR\n",
            host_key.display(),
            dir.join("authorized_keys").display()
        );
        std::fs::write(dir.join("sshd_config"), cfg).unwrap();
        let log = std::fs::File::create(dir.join("sshd.log")).unwrap();
        let child = Command::new("/usr/sbin/sshd")
            .args(["-f"]).arg(dir.join("sshd_config")).args(["-D", "-e"])
            .stdout(Stdio::null()).stderr(log)
            .spawn().expect("sshd starts");
        // wait for the port
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(std::time::Instant::now() < deadline, "sshd did not listen: {}", std::fs::read_to_string(dir.join("sshd.log")).unwrap_or_default());
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Sshd(child)
    }

    fn git(dir: &PathBuf, args: &[&str]) {
        let st = Command::new("git").arg("-C").arg(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=T", "-c", "commit.gpgsign=false"]).args(args).status().unwrap();
        assert!(st.success());
    }

    #[tokio::test]
    async fn ssh_transport_against_private_sshd() {
        if !std::path::Path::new("/usr/sbin/sshd").exists() || which::which("ssh-keygen").is_err() || which::which("git").is_err() {
            eprintln!("sshd, ssh-keygen or git missing; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("stashpanda-ssh-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let client_key = dir.join("client_key");
        keygen(&client_key);
        std::fs::copy(dir.join("client_key.pub"), dir.join("authorized_keys")).unwrap();
        let port = free_port();
        let _sshd = start_sshd(&dir, port);

        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "first"]);
        std::fs::write(repo.join("a.txt"), "hello\nworld\n").unwrap();

        let user = std::env::var("USER").unwrap();
        let target = SshTarget { host: "127.0.0.1".into(), port, user, identity_files: vec![client_key.clone()], identities_only: true };
        let ctx = Arc::new(SshContext::new(Arc::new(PassphraseCache::default()), dir.join("known_hosts")));
        let transport = SshTransport::new(target.clone(), None, ctx.clone());

        // Unknown host key is refused and remembered for "Trust this host".
        let err = transport.probe().await.expect_err("unknown host must be refused");
        assert!(matches!(err, GitError::SshHostKeyUnknown { .. }), "{err:?}");
        ctx.trust_offered("127.0.0.1", port).unwrap();
        assert!(std::fs::read_to_string(dir.join("known_hosts")).unwrap().contains("ssh-ed25519"));

        let info = transport.probe().await.expect("probe over ssh");
        assert!(info.git_version.major >= 2);

        let transport: SharedTransport = Arc::new(transport);
        let client = GitClient {
            transport: transport.clone(),
            repo: NativePath(repo.to_string_lossy().into_owned()),
            caps: Capabilities::for_version(info.git_version),
            console: Arc::new(ConsoleRecorder::new(None, "ssh-test".into(), 4096)),
        };
        let status = crate::git::queries::status::get_status(&client).await.unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.unstaged_count, 1);
        let log = crate::git::queries::log::get_log(&client, 100).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].subject, "first");
        let bytes = transport.read_file(&NativePath(repo.join("a.txt").to_string_lossy().into_owned()), 1024).await.unwrap();
        assert_eq!(bytes, b"hello\nworld\n");
        assert!(matches!(transport.read_file(&NativePath(repo.join("a.txt").to_string_lossy().into_owned()), 3).await, Err(GitError::TooLarge { .. })));
        let times = transport
            .modified_times(&[
                NativePath(repo.join("a.txt").to_string_lossy().into_owned()),
                NativePath(repo.join("missing.txt").to_string_lossy().into_owned()),
            ])
            .await
            .unwrap();
        let expected = std::fs::metadata(repo.join("a.txt")).unwrap().modified().unwrap()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 * 1000;
        assert_eq!(times, vec![Some(expected), None], "remote stat reports whole seconds");

        // Many concurrent channels share one connection.
        let mut tasks = Vec::new();
        for _ in 0..12 {
            let c = client.transport.clone();
            tasks.push(tokio::spawn(async move {
                let mut cmd = Cmd::git("echo", vec!["hi".into()]);
                cmd.program = Program::Other("echo".into());
                c.run(cmd).await.unwrap()
            }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap().stdout, b"hi\n");
        }

        // Kill a long-running remote process.
        let mut cmd = Cmd::git("sleep", vec!["30".into()]);
        cmd.program = Program::Other("sleep".into());
        let mut proc = transport.spawn(cmd).await.unwrap();
        proc.kill().await.unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), proc.wait()).await.expect("killed process exits");

        // Terminate: the exit is reported only once the remote process has gone, and the child
        // it started has gone with it. The `^` anchors keep pgrep from matching its own caller.
        let alive = || String::from_utf8_lossy(&Command::new("pgrep").args(["-f", "^sleep 31[78]"]).output().unwrap().stdout).into_owned();
        let mut cmd = Cmd::git("sh", vec!["-c".into(), "sleep 317 & sleep 318".into()]);
        cmd.program = Program::Other("sh".into());
        let mut proc = transport.spawn(cmd).await.unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while alive().lines().count() < 2 {
            assert!(std::time::Instant::now() < deadline, "remote sleeps did not start");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        proc.terminate().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), proc.wait()).await.expect("terminated process exits").unwrap();
        // The background child may take a moment longer than the shell.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !alive().is_empty() {
            assert!(std::time::Instant::now() < deadline, "left running: {}", alive());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        ctx.pool.disconnect_all().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
