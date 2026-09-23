//! Talk to the running SSH agent: list identities, add a key.

use russh::keys::agent::client::AgentClient;
use russh::keys::agent::Constraint;
use ssh_key::{HashAlg, PrivateKey, PublicKey};

use crate::error::{GitError, Result};

fn agent_err(e: impl std::fmt::Display) -> GitError {
    GitError::Io { detail: format!("ssh agent: {e}") }
}

#[cfg(unix)]
pub async fn list_fingerprints() -> Result<Vec<String>> {
    let mut client = AgentClient::connect_env().await.map_err(agent_err)?;
    let ids = client.request_identities().await.map_err(agent_err)?;
    Ok(ids.into_iter().filter_map(|id| id_fingerprint(&id)).collect())
}

#[cfg(unix)]
pub async fn add_key(key: &PrivateKey, lifetime_secs: Option<u32>) -> Result<()> {
    let mut client = AgentClient::connect_env().await.map_err(agent_err)?;
    let constraints: Vec<Constraint> = lifetime_secs.map(|s| vec![Constraint::KeyLifetime { seconds: s }]).unwrap_or_default();
    client.add_identity(key, &constraints).await.map_err(agent_err)
}

#[cfg(windows)]
const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

#[cfg(windows)]
pub async fn list_fingerprints() -> Result<Vec<String>> {
    if let Ok(mut client) = AgentClient::connect_named_pipe(OPENSSH_PIPE).await {
        let ids = client.request_identities().await.map_err(agent_err)?;
        return Ok(ids.into_iter().filter_map(|id| id_fingerprint(&id)).collect());
    }
    let mut client = AgentClient::connect_pageant().await.map_err(agent_err)?;
    let ids = client.request_identities().await.map_err(agent_err)?;
    Ok(ids.into_iter().filter_map(|id| id_fingerprint(&id)).collect())
}

#[cfg(windows)]
pub async fn add_key(key: &PrivateKey, lifetime_secs: Option<u32>) -> Result<()> {
    let constraints: Vec<Constraint> = lifetime_secs.map(|s| vec![Constraint::KeyLifetime { seconds: s }]).unwrap_or_default();
    if let Ok(mut client) = AgentClient::connect_named_pipe(OPENSSH_PIPE).await {
        return client.add_identity(key, &constraints).await.map_err(agent_err);
    }
    let mut client = AgentClient::connect_pageant().await.map_err(agent_err)?;
    client.add_identity(key, &constraints).await.map_err(agent_err)
}

fn id_fingerprint(id: &russh::keys::agent::AgentIdentity) -> Option<String> {
    use russh::keys::agent::AgentIdentity;
    match id {
        AgentIdentity::PublicKey { key, .. } => Some(key.fingerprint(HashAlg::Sha256).to_string()),
        AgentIdentity::Certificate { certificate, .. } => {
            PublicKey::from(certificate.public_key().clone()).fingerprint(HashAlg::Sha256).to_string().into()
        }
    }
}
