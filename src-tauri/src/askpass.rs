//! Lets Git and `ssh` ask StashPanda for a passphrase, a password or a yes.
//!
//! Git runs with no terminal, so the `ssh` it starts cannot ask for the passphrase of an encrypted
//! key. `SSH_ASKPASS` and `GIT_ASKPASS` name a program to ask instead, and that program is
//! StashPanda's own executable: started with [`ENDPOINT_ENV`] set and the prompt as its one
//! argument, it does nothing but pass the prompt to the running app and print the answer
//! ([`run_as_helper`]). The app answers a passphrase prompt from the passphrases it remembers
//! (memory, then the system keychain), and otherwise asks the user in a dialog.
//!
//! The two talk over a TCP connection on 127.0.0.1, because that is the one channel that is the
//! same on Linux and Windows and that a Windows program started from inside WSL can reach. Each
//! Git command gets a token of its own, which stops being valid when the command ends.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

use crate::error::GitError;
use crate::keys::secrets::PassphraseCache;
use crate::transport::paths::NativePath;
use crate::transport::{Cmd, SharedTransport, TransportKind};

/// Where the helper finds the app: `127.0.0.1:<port>/<token>`.
pub const ENDPOINT_ENV: &str = "STASHPANDA_ASKPASS";

/// The longest prompt or answer either side reads.
const MAX_LINE: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
struct Question {
    token: String,
    prompt: String,
}

#[derive(Serialize, Deserialize)]
struct Answer {
    /// None: there is no answer, so `ssh` or Git fails as it does when nobody can be asked.
    answer: Option<String>,
}

// ---- The helper: what the executable does when Git or ssh start it as their askpass program ----

/// The exit code to end with when this process was started as the askpass helper, or None when
/// it is the app. Called before anything else in `main`.
pub fn run_as_helper() -> Option<i32> {
    let endpoint = std::env::var(ENDPOINT_ENV).ok()?;
    // Git and ssh start it with the prompt as the only argument.
    let mut args = std::env::args().skip(1);
    let prompt = args.next()?;
    if args.next().is_some() {
        return None;
    }
    Some(match ask_app(&endpoint, &prompt) {
        Ok(Some(answer)) => {
            let mut out = std::io::stdout();
            let _ = writeln!(out, "{answer}");
            let _ = out.flush();
            0
        }
        _ => 1,
    })
}

/// Sends `prompt` to the app at `endpoint` and waits for its answer, however long the user takes.
fn ask_app(endpoint: &str, prompt: &str) -> std::io::Result<Option<String>> {
    let bad = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, what.to_string());
    let (addr, token) = endpoint.rsplit_once('/').ok_or_else(|| bad("no token in the endpoint"))?;
    let mut stream = std::net::TcpStream::connect(addr)?;
    let mut line = serde_json::to_string(&Question { token: token.to_string(), prompt: prompt.to_string() })?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut reply = String::new();
    BufReader::new(std::io::Read::take(stream, MAX_LINE)).read_line(&mut reply)?;
    let answer: Answer = serde_json::from_str(&reply)?;
    Ok(answer.answer)
}

// ---- The app's side ----

/// Whether a Git command may ask through the helper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AskMode {
    /// The helper is not offered to the command at all.
    #[default]
    Never,
    /// A passphrase StashPanda remembers is given; nothing is asked of the user. For commands
    /// the user did not start: an automatic fetch, and comparing tags with the remotes.
    StoredOnly,
    /// A remembered passphrase is given, and anything else is asked in a dialog.
    Ask,
}

/// What a transport adds to a command's environment so it can reach the helper. A command
/// carries it as `Cmd::askpass`.
#[derive(Debug, Clone)]
pub struct AskpassEnv {
    /// This executable, as the machine StashPanda runs on names it.
    pub exe: PathBuf,
    /// The value of [`ENDPOINT_ENV`].
    pub endpoint: String,
}

/// What the helper asked for, as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AskpassKind {
    /// The passphrase of the key file at `path`, a path this app can read. `wrong`: a remembered
    /// passphrase did not fit the key and has been forgotten.
    #[serde(rename_all = "camelCase")]
    Passphrase { path: String, key_name: String, wrong: bool },
    /// Anything else that must not be shown while typed: a password. `prompt` is ssh's or Git's own text.
    Secret { prompt: String },
    /// A user name, shown while typed.
    Text { prompt: String },
    /// A question answered with yes: whether to trust a server's host key.
    Confirm { prompt: String },
}

/// The helper is waiting for the user. Answer with `askpass_answer`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct AskpassRequest {
    pub id: String,
    /// The Git command that is asking: `fetch`, `push`.
    pub command: String,
    pub kind: AskpassKind,
}

/// A request no longer needs an answer: its Git command ended or was stopped.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct AskpassClosed {
    pub id: String,
}

/// What the server tells the window.
pub enum UiEvent {
    Request(AskpassRequest),
    Closed(AskpassClosed),
}

struct SessionInfo {
    transport: SharedTransport,
    mode: AskMode,
    label: &'static str,
}

struct Pending {
    token: String,
    answer: oneshot::Sender<Option<String>>,
}

pub struct Askpass {
    addr: SocketAddr,
    exe: PathBuf,
    passphrases: Arc<PassphraseCache>,
    /// One per running Git command, by its token.
    sessions: Mutex<HashMap<String, SessionInfo>>,
    /// The requests a dialog is open for, by request id.
    pending: Mutex<HashMap<String, Pending>>,
    /// One per key file a passphrase was asked for, by the file's path. Commands that run at the
    /// same time each start an `ssh` of their own, and each `ssh` asks; the gate lets one of them
    /// ask the user while the others wait for its answer.
    key_gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<KeyGate>>>>,
    ui: Box<dyn Fn(UiEvent) + Send + Sync>,
}

/// What the user last answered for one key, kept only while other commands wait for it.
#[derive(Default)]
struct KeyGate {
    /// The passphrase typed, or None when the dialog was cancelled, and when.
    last: Option<(Option<zeroize::Zeroizing<String>>, std::time::Instant)>,
}

/// One Git command's permission to ask. Dropping it, when the command has ended, makes its token
/// invalid and closes any dialog still open for it.
pub struct Session {
    server: Arc<Askpass>,
    token: String,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut sessions) = self.server.sessions.lock() {
            sessions.remove(&self.token);
        }
        let closed: Vec<(String, Pending)> = match self.server.pending.lock() {
            Ok(mut pending) => {
                let ids: Vec<String> = pending.iter().filter(|(_, p)| p.token == self.token).map(|(id, _)| id.clone()).collect();
                ids.into_iter().filter_map(|id| pending.remove(&id).map(|p| (id, p))).collect()
            }
            Err(_) => Vec::new(),
        };
        for (id, p) in closed {
            let _ = p.answer.send(None);
            (self.server.ui)(UiEvent::Closed(AskpassClosed { id }));
        }
    }
}

static GLOBAL: OnceLock<Arc<Askpass>> = OnceLock::new();

/// Makes `server` the one every Git command is offered. Done once, when the app starts.
pub fn install(server: Arc<Askpass>) {
    let _ = GLOBAL.set(server);
}

/// Gives `cmd` the helper, when the app has one and the command may ask. The returned session
/// must live until the command has ended.
pub fn attach(transport: &SharedTransport, mode: AskMode, label: &'static str, cmd: &mut Cmd) -> Option<Session> {
    GLOBAL.get()?.clone().attach(transport, mode, label, cmd)
}

impl Askpass {
    /// Starts listening on a free port of 127.0.0.1. `ui` is called when a dialog must open or close.
    pub async fn start(passphrases: Arc<PassphraseCache>, ui: Box<dyn Fn(UiEvent) + Send + Sync>) -> std::io::Result<Arc<Askpass>> {
        Askpass::start_with_helper(std::env::current_exe()?, passphrases, ui).await
    }

    /// `start`, with another executable as the helper than the one running: the end-to-end test
    /// runs in a test program and names the app's.
    pub async fn start_with_helper(exe: PathBuf, passphrases: Arc<PassphraseCache>, ui: Box<dyn Fn(UiEvent) + Send + Sync>) -> std::io::Result<Arc<Askpass>> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let server = Arc::new(Askpass {
            addr: listener.local_addr()?,
            exe,
            passphrases,
            sessions: Mutex::default(),
            pending: Mutex::default(),
            key_gates: Mutex::default(),
            ui,
        });
        let accepting = server.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let server = accepting.clone();
                        tokio::spawn(async move { server.serve(stream).await });
                    }
                    Err(e) => {
                        tracing::warn!("askpass: accept failed: {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
            }
        });
        Ok(server)
    }

    /// See [`attach`].
    pub fn attach(self: Arc<Self>, transport: &SharedTransport, mode: AskMode, label: &'static str, cmd: &mut Cmd) -> Option<Session> {
        // Over SSH, Git runs on the server, where this helper does not exist.
        if mode == AskMode::Never || matches!(transport.kind(), TransportKind::Ssh { .. }) {
            return None;
        }
        // 244 random bits: whoever can connect to the port still has to know this.
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        self.sessions.lock().ok()?.insert(token.clone(), SessionInfo { transport: transport.clone(), mode, label });
        cmd.askpass = Some(AskpassEnv { exe: self.exe.clone(), endpoint: format!("{}/{token}", self.addr) });
        Some(Session { server: self, token })
    }

    /// The answer the window gave for request `id`. False when nothing waits for it any more.
    pub fn answer(&self, id: &str, answer: Option<String>) -> bool {
        let pending = self.pending.lock().ok().and_then(|mut p| p.remove(id));
        match pending {
            Some(p) => p.answer.send(answer).is_ok(),
            None => false,
        }
    }

    /// One helper: reads its question, answers it, closes.
    async fn serve(self: Arc<Self>, stream: tokio::net::TcpStream) {
        let (read, mut write) = stream.into_split();
        let mut read = tokio::io::BufReader::new(read.take(MAX_LINE));
        let mut line = String::new();
        if read.read_line(&mut line).await.is_err() {
            return;
        }
        let Ok(question) = serde_json::from_str::<Question>(&line) else { return };
        let session = self.sessions.lock().ok().and_then(|s| s.get(&question.token).map(|i| (i.transport.clone(), i.mode, i.label)));
        let answer = match session {
            Some((transport, mode, label)) => self.answer_prompt(&question.token, transport, mode, label, &question.prompt, &mut read).await,
            None => None,
        };
        if let Ok(mut reply) = serde_json::to_string(&Answer { answer }) {
            reply.push('\n');
            let _ = write.write_all(reply.as_bytes()).await;
            let _ = write.shutdown().await;
        }
    }

    async fn answer_prompt(
        &self,
        token: &str,
        transport: SharedTransport,
        mode: AskMode,
        label: &'static str,
        prompt: &str,
        helper: &mut (impl tokio::io::AsyncRead + Unpin),
    ) -> Option<String> {
        let kind = match classify(prompt) {
            Prompt::Passphrase { path } => {
                let host_path = key_file_on_host(&transport, &path);
                match tokio::fs::read(&host_path).await {
                    Ok(bytes) => return self.passphrase(token, mode, label, host_path, bytes, helper).await,
                    // A key file this app cannot read: the passphrase cannot be checked or
                    // remembered, only passed on.
                    Err(_) => AskpassKind::Secret { prompt: prompt.trim().to_string() },
                }
            }
            Prompt::Confirm => AskpassKind::Confirm { prompt: prompt.trim().to_string() },
            Prompt::Text => AskpassKind::Text { prompt: prompt.trim().to_string() },
            Prompt::Secret => AskpassKind::Secret { prompt: prompt.trim().to_string() },
        };
        if mode != AskMode::Ask {
            return None;
        }
        self.ask_user(token, label, kind, helper).await
    }

    /// The passphrase of the key file at `path`, whose contents are `bytes`: the one remembered,
    /// or the one the user types. Commands that ask for the same key at the same time (a tag
    /// dialog asks each remote at once whether it has the tag) share one dialog:
    /// the first asks, and the others wait and take its answer, also when the user chose not to
    /// have it remembered or cancelled.
    async fn passphrase(
        &self,
        token: &str,
        mode: AskMode,
        label: &'static str,
        path: String,
        bytes: Vec<u8>,
        helper: &mut (impl tokio::io::AsyncRead + Unpin),
    ) -> Option<String> {
        let arrived = std::time::Instant::now();
        let gate = self.key_gates.lock().ok()?.entry(path.clone()).or_default().clone();
        let mut state = gate.lock().await;
        // The map holds the gate once and each command at it once: more than that, and others wait.
        let others_wait = || Arc::strong_count(&gate) > 2;
        if let Some((answer, answered)) = &state.last {
            if *answered >= arrived {
                // Answered while this command waited: the answer is for it too.
                let answer = answer.as_ref().map(|p| p.to_string());
                if !others_wait() {
                    state.last = None;
                }
                return answer;
            }
            state.last = None;
        }
        let wrong = match self.remembered(&path, bytes).await {
            Remembered::Fits(passphrase) => return Some(passphrase),
            Remembered::Wrong => true,
            Remembered::Nothing => false,
        };
        if mode != AskMode::Ask {
            return None;
        }
        let kind = AskpassKind::Passphrase { key_name: file_name(&path).to_string(), path, wrong };
        let answer = self.ask_user(token, label, kind, helper).await;
        if others_wait() {
            state.last = Some((answer.clone().map(zeroize::Zeroizing::new), std::time::Instant::now()));
        }
        answer
    }

    /// The passphrase remembered for the key at `path`, checked against the key itself, so that
    /// `ssh` is never handed one that does not fit. One that does not fit is forgotten, as the
    /// app's own SSH connection does.
    async fn remembered(&self, path: &str, bytes: Vec<u8>) -> Remembered {
        let cache = self.passphrases.clone();
        let path = path.to_string();
        // The keychain and the key derivation both block.
        tokio::task::spawn_blocking(move || {
            let Some(passphrase) = cache.get(&path) else { return Remembered::Nothing };
            match crate::keys::load_private_key(&path, &bytes, Some(passphrase.as_str())) {
                Err(GitError::SshWrongPassphrase { .. }) => {
                    let _ = cache.forget(&path);
                    Remembered::Wrong
                }
                // A key this app cannot parse may still be one ssh can use: let ssh judge it.
                _ => Remembered::Fits(passphrase.to_string()),
            }
        })
        .await
        .unwrap_or(Remembered::Nothing)
    }

    /// Opens a dialog and waits for its answer, or for the helper to go away (its Git command
    /// was stopped), which closes the dialog.
    async fn ask_user(&self, token: &str, label: &'static str, kind: AskpassKind, helper: &mut (impl tokio::io::AsyncRead + Unpin)) -> Option<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().ok()?.insert(id.clone(), Pending { token: token.to_string(), answer: tx });
        (self.ui)(UiEvent::Request(AskpassRequest { id: id.clone(), command: label.to_string(), kind }));
        let mut byte = [0u8; 1];
        tokio::select! {
            answer = rx => answer.ok().flatten(),
            // The helper sends nothing after its question, so this returns when it has gone.
            _ = helper.read(&mut byte) => {
                let was_open = self.pending.lock().ok().and_then(|mut p| p.remove(&id)).is_some();
                if was_open {
                    (self.ui)(UiEvent::Closed(AskpassClosed { id }));
                }
                None
            }
        }
    }
}

enum Remembered {
    Fits(String),
    Wrong,
    Nothing,
}

/// What a prompt from `ssh` or Git is asking for.
#[derive(Debug, PartialEq, Eq)]
enum Prompt {
    /// `Enter passphrase for key '/home/me/.ssh/id_ed25519': `
    Passphrase { path: String },
    /// `Are you sure you want to continue connecting (yes/no/[fingerprint])? `
    Confirm,
    /// `Username for 'https://example.com': `
    Text,
    /// `me@example.com's password: `, `Password for 'https://me@example.com': ` and anything else.
    Secret,
}

fn classify(prompt: &str) -> Prompt {
    let p = prompt.trim();
    if let Some(rest) = p.strip_prefix("Enter passphrase for key '") {
        if let Some(end) = rest.rfind('\'') {
            return Prompt::Passphrase { path: rest[..end].to_string() };
        }
    }
    if p.contains("(yes/no") {
        return Prompt::Confirm;
    }
    if p.starts_with("Username for ") {
        return Prompt::Text;
    }
    Prompt::Secret
}

/// The key file `ssh` named in its prompt, as a path this app can read and as the key manager
/// names the same file, so a passphrase remembered there is found here. Inside a WSL distro
/// reached from Windows that is the `\\wsl.localhost\<distro>` view; for Git for Windows, whose
/// `ssh` writes `/c/Users/me/.ssh/id_ed25519`, the Windows form.
fn key_file_on_host(transport: &SharedTransport, prompt_path: &str) -> String {
    match transport.kind() {
        TransportKind::Wsl { .. } => match transport.paths().to_host(&NativePath(prompt_path.to_string())) {
            Ok(host) => host.0,
            Err(_) => prompt_path.to_string(),
        },
        _ if cfg!(windows) => msys_to_windows(prompt_path),
        _ => prompt_path.to_string(),
    }
}

/// `/c/Users/me/.ssh/id_rsa` and `C:/Users/me/.ssh/id_rsa` as `C:\Users\me\.ssh\id_rsa`.
fn msys_to_windows(path: &str) -> String {
    let bytes = path.as_bytes();
    let drive_path = bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b'/';
    let with_drive = if drive_path { format!("{}:{}", (bytes[1] as char).to_ascii_uppercase(), &path[2..]) } else { path.to_string() };
    with_drive.replace('/', "\\")
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The helper's path for a program that is not started by this operating system's own rules:
/// forward slashes, which Git for Windows' `ssh` takes and a shell does not eat.
pub fn exe_for_env(exe: &std::path::Path) -> String {
    let text = exe.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::keychain::MemoryStore;
    use crate::transport::local::LocalTransport;
    use crate::transport::paths::IdentityMapper;

    #[test]
    fn prompts_are_told_apart() {
        assert_eq!(classify("Enter passphrase for key '/home/me/.ssh/id_ed25519': "), Prompt::Passphrase { path: "/home/me/.ssh/id_ed25519".into() });
        assert_eq!(classify("Enter passphrase for key '/home/me/it's mine/id_rsa': "), Prompt::Passphrase { path: "/home/me/it's mine/id_rsa".into() });
        assert_eq!(
            classify("The authenticity of host 'example.com (192.0.2.1)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? "),
            Prompt::Confirm
        );
        assert_eq!(classify("Username for 'https://example.com': "), Prompt::Text);
        assert_eq!(classify("Password for 'https://me@example.com': "), Prompt::Secret);
        assert_eq!(classify("me@example.com's password: "), Prompt::Secret);
    }

    #[test]
    fn git_for_windows_paths_become_windows_paths() {
        assert_eq!(msys_to_windows("/c/Users/me/.ssh/id_rsa"), r"C:\Users\me\.ssh\id_rsa");
        assert_eq!(msys_to_windows("C:/Users/me/.ssh/id_rsa"), r"C:\Users\me\.ssh\id_rsa");
        assert_eq!(msys_to_windows(r"C:\Users\me\.ssh\id_rsa"), r"C:\Users\me\.ssh\id_rsa");
        assert_eq!(file_name(r"\\wsl.localhost\Ubuntu\home\me\.ssh\id_ed25519"), "id_ed25519");
        assert_eq!(file_name("/home/me/.ssh/id_rsa"), "id_rsa");
    }

    fn local_transport() -> SharedTransport {
        Arc::new(LocalTransport { git: "git".into(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false })
    }

    /// A server whose dialogs are answered by `on_request`, and the requests and closings it saw.
    async fn server(
        on_request: impl Fn(&AskpassRequest) -> Option<Option<String>> + Send + Sync + 'static,
    ) -> (Arc<Askpass>, Arc<PassphraseCache>, Arc<Mutex<Vec<String>>>) {
        let cache = Arc::new(PassphraseCache::with_keychain(Arc::new(MemoryStore::default())));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let slot: Arc<OnceLock<Arc<Askpass>>> = Arc::new(OnceLock::new());
        let (seen2, slot2) = (seen.clone(), slot.clone());
        let ui = Box::new(move |event: UiEvent| match event {
            UiEvent::Request(r) => {
                seen2.lock().unwrap().push(format!("{:?}", r.kind));
                if let Some(answer) = on_request(&r) {
                    let server = slot2.get().unwrap().clone();
                    tokio::spawn(async move { server.answer(&r.id, answer) });
                }
            }
            UiEvent::Closed(c) => seen2.lock().unwrap().push(format!("closed {}", c.id.len())),
        });
        let server = Askpass::start(cache.clone(), ui).await.unwrap();
        let _ = slot.set(server.clone());
        (server, cache, seen)
    }

    async fn helper(endpoint: String, prompt: &str) -> Option<String> {
        let prompt = prompt.to_string();
        tokio::task::spawn_blocking(move || ask_app(&endpoint, &prompt)).await.unwrap().unwrap()
    }

    /// An encrypted key in a new folder, with the passphrase `secret`.
    fn encrypted_key() -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("stashpanda-askpass-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = crate::keys::generate(crate::keys::GenerateAlgorithm::Ed25519, "test").unwrap();
        let text = crate::keys::export_private(&key, crate::keys::ExportFormat::OpenSsh, Some("secret")).unwrap();
        let path = dir.join("id_ed25519");
        std::fs::write(&path, text.as_bytes()).unwrap();
        (dir, path.to_string_lossy().into_owned())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_remembered_passphrase_is_given_without_asking() {
        let (server, cache, seen) = server(|_| Some(None)).await;
        let (dir, key) = encrypted_key();
        let prompt = format!("Enter passphrase for key '{key}': ");
        let mut cmd = Cmd::git("fetch", vec![]);
        let session = server.clone().attach(&local_transport(), AskMode::StoredOnly, "fetch", &mut cmd).unwrap();
        let endpoint = cmd.askpass.clone().unwrap().endpoint;

        // Nothing remembered, and this command may not ask the user.
        assert_eq!(helper(endpoint.clone(), &prompt).await, None);
        assert!(seen.lock().unwrap().is_empty());

        cache.remember(&key, "secret");
        assert_eq!(helper(endpoint.clone(), &prompt).await.as_deref(), Some("secret"));

        // One that does not fit the key is never handed to ssh, and is forgotten.
        cache.remember(&key, "not it");
        assert_eq!(helper(endpoint.clone(), &prompt).await, None);
        assert!(cache.get(&key).is_none());
        assert!(seen.lock().unwrap().is_empty(), "a command that may not ask opens no dialog");

        // The token dies with the command.
        cache.remember(&key, "secret");
        drop(session);
        assert_eq!(helper(endpoint, &prompt).await, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_user_is_asked_for_what_is_not_remembered() {
        let (server, cache, seen) = server(|r| {
            Some(match &r.kind {
                AskpassKind::Passphrase { .. } => Some("typed".to_string()),
                AskpassKind::Confirm { .. } => Some("yes".to_string()),
                AskpassKind::Text { .. } => Some("me".to_string()),
                AskpassKind::Secret { .. } => None,
            })
        })
        .await;
        let (dir, key) = encrypted_key();
        let mut cmd = Cmd::git("push", vec![]);
        let _session = server.clone().attach(&local_transport(), AskMode::Ask, "push", &mut cmd).unwrap();
        let endpoint = cmd.askpass.clone().unwrap().endpoint;

        cache.remember(&key, "not it");
        assert_eq!(helper(endpoint.clone(), &format!("Enter passphrase for key '{key}': ")).await.as_deref(), Some("typed"));
        assert_eq!(helper(endpoint.clone(), "Are you sure you want to continue connecting (yes/no/[fingerprint])? ").await.as_deref(), Some("yes"));
        assert_eq!(helper(endpoint.clone(), "Username for 'https://example.com': ").await.as_deref(), Some("me"));
        // Cancelled in the dialog.
        assert_eq!(helper(endpoint.clone(), "Password for 'https://me@example.com': ").await, None);
        // A key file that is not there is asked for as a plain secret.
        assert_eq!(helper(endpoint.clone(), "Enter passphrase for key '/no/such/key': ").await, None);
        // A token nobody was given.
        assert_eq!(helper(format!("{}/{}", server.addr, "0".repeat(64)), "Username for 'https://example.com': ").await, None);

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 5, "{seen:?}");
        assert!(seen[0].contains("Passphrase") && seen[0].contains("wrong: true") && seen[0].contains("id_ed25519"), "{seen:?}");
        assert!(seen[1].starts_with("Confirm") && seen[2].starts_with("Text") && seen[3].starts_with("Secret") && seen[4].starts_with("Secret"), "{seen:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A server whose passphrase dialog is answered with `typed` after a moment, long enough for
    /// every helper of a test to have asked, and the number of dialogs it opened.
    async fn slow_server(typed: Option<&'static str>) -> (Arc<Askpass>, Arc<Mutex<usize>>) {
        let cache = Arc::new(PassphraseCache::with_keychain(Arc::new(MemoryStore::default())));
        let opened = Arc::new(Mutex::new(0usize));
        let slot: Arc<OnceLock<Arc<Askpass>>> = Arc::new(OnceLock::new());
        let (opened2, slot2) = (opened.clone(), slot.clone());
        let ui = Box::new(move |event: UiEvent| {
            let UiEvent::Request(r) = event else { return };
            *opened2.lock().unwrap() += 1;
            let server = slot2.get().unwrap().clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                server.answer(&r.id, typed.map(str::to_string))
            });
        });
        let server = Askpass::start(cache, ui).await.unwrap();
        let _ = slot.set(server.clone());
        (server, opened)
    }

    /// Three commands at once, as when a tag dialog asks three remotes about a tag: one dialog.
    #[tokio::test(flavor = "multi_thread")]
    async fn commands_that_ask_for_one_key_at_once_share_one_dialog() {
        let (dir, key) = encrypted_key();
        let prompt = format!("Enter passphrase for key '{key}': ");
        for typed in [Some("typed"), None] {
            let (server, opened) = slow_server(typed).await;
            let mut sessions = Vec::new();
            let mut helpers = Vec::new();
            for _ in 0..3 {
                let mut cmd = Cmd::git("ls-remote", vec![]);
                sessions.push(server.clone().attach(&local_transport(), AskMode::Ask, "ls-remote", &mut cmd).unwrap());
                let (endpoint, prompt) = (cmd.askpass.clone().unwrap().endpoint, prompt.clone());
                helpers.push(tokio::spawn(async move { helper(endpoint, &prompt).await }));
            }
            for h in helpers {
                assert_eq!(h.await.unwrap().as_deref(), typed, "every command gets the one answer");
            }
            assert_eq!(*opened.lock().unwrap(), 1, "answer {typed:?}");

            // A command that asks afterwards is asked again: nothing was remembered, and the
            // answer was kept only for the commands that were waiting.
            let mut cmd = Cmd::git("fetch", vec![]);
            let _later = server.clone().attach(&local_transport(), AskMode::Ask, "fetch", &mut cmd).unwrap();
            assert_eq!(helper(cmd.askpass.clone().unwrap().endpoint, &prompt).await.as_deref(), typed);
            assert_eq!(*opened.lock().unwrap(), 2);
            let gates = server.key_gates.lock().unwrap();
            assert!(gates.values().all(|g| g.try_lock().unwrap().last.is_none()), "no typed passphrase is kept once nobody waits");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dialog_closes_when_its_command_ends() {
        // Nobody answers: the dialog stays open until the command is over.
        let (server, _cache, seen) = server(|_| None).await;
        let mut cmd = Cmd::git("fetch", vec![]);
        let session = server.clone().attach(&local_transport(), AskMode::Ask, "fetch", &mut cmd).unwrap();
        let endpoint = cmd.askpass.clone().unwrap().endpoint;
        let waiting = tokio::spawn(helper(endpoint, "me@example.com's password: "));
        for _ in 0..200 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        drop(session);
        assert_eq!(waiting.await.unwrap(), None);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen[1].starts_with("closed"), "{seen:?}");
    }
}
