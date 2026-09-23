//! One authenticated SSH connection per (host, port, user), shared by every
//! repository on that host, with keepalive and reconnect. Also the host-key
//! policy and the authentication chain.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, AuthResult, Handle, Msg};
use russh::keys::agent::client::AgentClient;
use russh::keys::agent::AgentIdentity;
use russh::keys::known_hosts::learn_known_hosts_path;
use russh::keys::{check_known_hosts_path, PrivateKeyWithHashAlg};
use russh::Channel;
use ssh_key::{HashAlg, PublicKey};
use tokio::sync::Semaphore;

use crate::error::{GitError, Result};
use crate::keys::secrets::PassphraseCache;
use crate::settings::Connection;
use crate::transport::paths::SshSpec;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_files: Vec<PathBuf>,
    pub identities_only: bool,
}

/// Shared by all SSH transports: the pool, passphrases, host-key policy.
pub struct SshContext {
    pub pool: SshPool,
    pub passphrases: Arc<PassphraseCache>,
    pub known_hosts: PathBuf,
    /// Accept and record unknown host keys without asking (per-repo opt-in
    /// handled by the caller; tests set it directly).
    pub accept_new: std::sync::atomic::AtomicBool,
    /// Host keys offered by servers we refused, keyed by `host:port`, so the
    /// UI can show the fingerprint and "Trust this host" can record it.
    pub offered_keys: Mutex<HashMap<String, PublicKey>>,
    /// Passwords supplied through the UI, keyed by `user@host:port`.
    pub passwords: PassphraseCache,
}

impl SshContext {
    pub fn new(passphrases: Arc<PassphraseCache>, known_hosts: PathBuf) -> SshContext {
        SshContext {
            pool: SshPool::default(),
            passphrases,
            known_hosts,
            accept_new: std::sync::atomic::AtomicBool::new(false),
            offered_keys: Mutex::new(HashMap::new()),
            passwords: PassphraseCache::default(),
        }
    }

    pub fn default_known_hosts() -> PathBuf {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
        home.join(".ssh").join("known_hosts")
    }

    /// Record the key a server offered earlier, so the next connect succeeds.
    pub fn trust_offered(&self, host: &str, port: u16) -> Result<()> {
        let key = self
            .offered_keys
            .lock()
            .ok()
            .and_then(|m| m.get(&format!("{host}:{port}")).cloned())
            .ok_or_else(|| GitError::internal("no host key was offered for this host yet"))?;
        if let Some(dir) = self.known_hosts.parent() {
            std::fs::create_dir_all(dir)?;
        }
        learn_known_hosts_path(host, port, &key, &self.known_hosts).map_err(|e| GitError::Io { detail: e.to_string() })
    }
}

/// Resolve the user's `~/.ssh/config` and the repo's explicit settings into a target.
pub fn resolve_target(spec: &SshSpec, connection: &Connection) -> Result<SshTarget> {
    let (c_host, c_user, c_port, c_identity) = match connection {
        Connection::Ssh { host, user, port, identity_file, .. } => (
            Some(host.clone()).filter(|h| !h.trim().is_empty()),
            user.clone(),
            *port,
            identity_file.clone(),
        ),
        _ => (None, None, None, None),
    };
    let alias = c_host.unwrap_or_else(|| spec.host.clone());
    let cfg = russh_config::parse_home(&alias).ok();
    if let Some(c) = &cfg {
        if c.host_config.proxy_command.is_some() || c.host_config.proxy_jump.is_some() {
            return Err(GitError::SshUnsupportedFeature { feature: format!("ProxyCommand/ProxyJump for host {alias}") });
        }
    }
    let host = cfg.as_ref().map(|c| c.host_name.clone()).filter(|h| !h.is_empty()).unwrap_or_else(|| alias.clone());
    let user = c_user
        .or_else(|| spec.user.clone())
        .or_else(|| cfg.as_ref().and_then(|c| c.user.clone()))
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("USERNAME").ok())
        .unwrap_or_else(|| "root".to_string());
    let port = c_port.or(spec.port).or_else(|| cfg.as_ref().and_then(|c| c.port)).unwrap_or(22);
    let mut identity_files: Vec<PathBuf> = Vec::new();
    if let Some(f) = c_identity.filter(|f| !f.trim().is_empty()) {
        identity_files.push(PathBuf::from(f));
    }
    if let Some(files) = cfg.as_ref().and_then(|c| c.host_config.identity_file.clone()) {
        identity_files.extend(files);
    }
    // russh-config does not expose IdentitiesOnly; an explicit identity file implies it.
    let identities_only = !identity_files.is_empty();
    Ok(SshTarget { host, port, user, identity_files, identities_only })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostKeyVerdict {
    Known,
    Unknown,
    Changed,
}

struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    accept_new: bool,
    verdict: Arc<Mutex<Option<(HostKeyVerdict, PublicKey)>>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &russh::keys::PublicKeyOrCertificate) -> std::result::Result<bool, Self::Error> {
        let pk: PublicKey = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            russh::keys::PublicKeyOrCertificate::Certificate(certificate) => {
                PublicKey::from(certificate.public_key().clone())
            }
        };
        let verdict = match check_known_hosts_path(&self.host, self.port, &pk, &self.known_hosts) {
            Ok(true) => HostKeyVerdict::Known,
            Ok(false) => {
                if self.accept_new {
                    if let Some(dir) = self.known_hosts.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    match learn_known_hosts_path(&self.host, self.port, &pk, &self.known_hosts) {
                        Ok(()) => HostKeyVerdict::Known,
                        Err(_) => HostKeyVerdict::Unknown,
                    }
                } else {
                    HostKeyVerdict::Unknown
                }
            }
            Err(russh::keys::Error::KeyChanged { .. }) => HostKeyVerdict::Changed,
            // Missing known_hosts file counts as unknown.
            Err(_) => {
                if self.accept_new {
                    let _ = self.known_hosts.parent().map(std::fs::create_dir_all);
                    match learn_known_hosts_path(&self.host, self.port, &pk, &self.known_hosts) {
                        Ok(()) => HostKeyVerdict::Known,
                        Err(_) => HostKeyVerdict::Unknown,
                    }
                } else {
                    HostKeyVerdict::Unknown
                }
            }
        };
        if let Ok(mut v) = self.verdict.lock() {
            *v = Some((verdict, pk));
        }
        Ok(verdict == HostKeyVerdict::Known)
    }
}

pub struct SshConn {
    handle: tokio::sync::Mutex<Handle<ClientHandler>>,
    /// OpenSSH servers default `MaxSessions` to 10; stay under it.
    pub slots: Arc<Semaphore>,
}

impl SshConn {
    pub async fn open_session(&self) -> Result<Channel<Msg>> {
        let h = self.handle.lock().await;
        h.channel_open_session().await.map_err(|e| GitError::TransportDisconnected { detail: e.to_string() })
    }
    pub async fn is_closed(&self) -> bool {
        self.handle.lock().await.is_closed()
    }
}

#[derive(Default)]
pub struct SshPool {
    conns: tokio::sync::Mutex<HashMap<(String, u16, String), Arc<SshConn>>>,
}

fn fingerprint(pk: &PublicKey) -> String {
    pk.fingerprint(HashAlg::Sha256).to_string()
}

impl SshPool {
    pub async fn get_or_connect(&self, target: &SshTarget, ctx: &SshContext) -> Result<Arc<SshConn>> {
        let key = (target.host.clone(), target.port, target.user.clone());
        let mut conns = self.conns.lock().await;
        if let Some(c) = conns.get(&key) {
            if !c.is_closed().await {
                return Ok(c.clone());
            }
            conns.remove(&key);
        }
        let conn = Arc::new(connect(target, ctx).await?);
        conns.insert(key, conn.clone());
        Ok(conn)
    }

    pub async fn disconnect_all(&self) {
        let mut conns = self.conns.lock().await;
        for (_, c) in conns.drain() {
            let h = c.handle.lock().await;
            let _ = h.disconnect(russh::Disconnect::ByApplication, "StashPanda closing", "").await;
        }
    }
}

async fn connect(target: &SshTarget, ctx: &SshContext) -> Result<SshConn> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        inactivity_timeout: None,
        ..Default::default()
    });
    let verdict = Arc::new(Mutex::new(None));
    let handler = ClientHandler {
        host: target.host.clone(),
        port: target.port,
        known_hosts: ctx.known_hosts.clone(),
        accept_new: ctx.accept_new.load(std::sync::atomic::Ordering::Relaxed),
        verdict: verdict.clone(),
    };
    let addr = (target.host.as_str(), target.port);
    let connect_fut = client::connect(config, addr, handler);
    let mut handle = match tokio::time::timeout(Duration::from_secs(20), connect_fut).await {
        Err(_) => {
            return Err(GitError::SshConnectFailed { host: target.host.clone(), detail: "timed out after 20s".into() })
        }
        Ok(Err(e)) => {
            let v = verdict.lock().ok().and_then(|g| g.clone());
            return Err(match v {
                Some((HostKeyVerdict::Unknown, pk)) => {
                    if let Ok(mut m) = ctx.offered_keys.lock() {
                        m.insert(format!("{}:{}", target.host, target.port), pk.clone());
                    }
                    GitError::SshHostKeyUnknown { host: target.host.clone(), fingerprint: fingerprint(&pk) }
                }
                Some((HostKeyVerdict::Changed, pk)) => {
                    GitError::SshHostKeyChanged { host: target.host.clone(), fingerprint: fingerprint(&pk) }
                }
                _ => GitError::SshConnectFailed { host: target.host.clone(), detail: e.to_string() },
            });
        }
        Ok(Ok(h)) => h,
    };

    let mut tried: Vec<String> = Vec::new();
    let rsa_hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();

    // 1. Agent (unless IdentitiesOnly).
    if !target.identities_only {
        if let Some(ok) = try_agent(&mut handle, &target.user, rsa_hash, &mut tried).await {
            if ok {
                return Ok(SshConn { handle: tokio::sync::Mutex::new(handle), slots: Arc::new(Semaphore::new(8)) });
            }
        }
    }
    // 2. Identity files (explicit ones, then the usual defaults).
    let mut files = target.identity_files.clone();
    if !target.identities_only {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
        for n in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            files.push(home.join(".ssh").join(n));
        }
    }
    for f in files {
        let f = expand_tilde(&f);
        if !f.is_file() {
            continue;
        }
        let path_s = f.to_string_lossy().into_owned();
        let bytes = match std::fs::read(&f) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let pass = ctx.passphrases.get(&path_s);
        let key = match crate::keys::load_private_key(&path_s, &bytes, pass.as_deref().map(|s| s.as_str())) {
            Ok(k) => k,
            Err(GitError::SshNeedsPassphrase { .. }) => {
                tried.push(format!("key {} (needs passphrase)", f.display()));
                // Explicit identity: the UI must ask for the passphrase.
                if target.identity_files.iter().any(|x| expand_tilde(x) == f) {
                    return Err(GitError::SshNeedsPassphrase { path: path_s });
                }
                continue;
            }
            Err(e) => {
                tried.push(format!("key {} ({e})", f.display()));
                continue;
            }
        };
        tried.push(format!("key {}", f.display()));
        let k = PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash);
        match handle.authenticate_publickey(target.user.clone(), k).await {
            Ok(AuthResult::Success) => {
                return Ok(SshConn { handle: tokio::sync::Mutex::new(handle), slots: Arc::new(Semaphore::new(8)) });
            }
            Ok(AuthResult::Failure { .. }) => {}
            Err(e) => {
                return Err(GitError::SshConnectFailed { host: target.host.clone(), detail: e.to_string() });
            }
        }
    }
    // 3. Password supplied through the UI.
    if let Some(pw) = ctx.passwords.get(&format!("{}@{}:{}", target.user, target.host, target.port)) {
        tried.push("password".into());
        if let Ok(AuthResult::Success) = handle.authenticate_password(target.user.clone(), pw.to_string()).await {
            return Ok(SshConn { handle: tokio::sync::Mutex::new(handle), slots: Arc::new(Semaphore::new(8)) });
        }
    }
    let _ = handle.disconnect(russh::Disconnect::ByApplication, "authentication failed", "").await;
    Err(GitError::SshAuthFailed { host: target.host.clone(), methods_tried: tried })
}

fn expand_tilde(p: &PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return PathBuf::from(home).join(rest);
        }
    }
    p.clone()
}

/// Returns Some(true) on success, Some(false) when the agent had keys but none
/// worked, None when no agent was reachable.
async fn try_agent(handle: &mut Handle<ClientHandler>, user: &str, rsa_hash: Option<HashAlg>, tried: &mut Vec<String>) -> Option<bool> {
    #[cfg(unix)]
    let mut agent = AgentClient::connect_env().await.ok()?;
    #[cfg(windows)]
    let mut agent = match AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
        Ok(a) => a,
        Err(_) => return None,
    };
    let ids = agent.request_identities().await.ok()?;
    if ids.is_empty() {
        return None;
    }
    for id in ids {
        let key = match id {
            AgentIdentity::PublicKey { key, .. } => key,
            AgentIdentity::Certificate { .. } => continue,
        };
        tried.push(format!("agent key {}", fingerprint(&key)));
        let hash = if key.algorithm().is_rsa() { rsa_hash } else { None };
        match handle.authenticate_publickey_with(user.to_string(), key, hash, &mut agent).await {
            Ok(AuthResult::Success) => return Some(true),
            Ok(AuthResult::Failure { .. }) => continue,
            Err(_) => return Some(false),
        }
    }
    Some(false)
}
