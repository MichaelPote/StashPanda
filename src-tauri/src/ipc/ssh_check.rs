//! The SSH Diagnosis dialog, for a remote that refused Git's SSH key: which Git and which `ssh`
//! the repository uses, the keys that `ssh` would offer and the ones its agent holds, and a login
//! test. Everything runs through the repository's own transport, as a Git alias (see
//! `ssh_check.sh`), so it sees what Git sees: Git for Windows' own ssh.exe, the WSL distro's
//! `~/.ssh` and agent, or the server's for a repository opened over SSH.

use std::time::Duration;

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::error::{GitError, Result};
use crate::git::{GitClient, GitReq};
use crate::ipc::query::repo;
use crate::settings::ResolvedTransport;
use crate::state::AppState;

const SCRIPT: &str = include_str!("ssh_check.sh");
const ALIAS: &str = "stashpanda-ssh-check";

/// How Git reaches `user@host`, read without connecting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SshSetup {
    /// The Git executable, as the app runs it.
    pub git_executable: String,
    pub git_version: String,
    /// What chose the SSH command: `GIT_SSH_COMMAND`, `GIT_SSH`, `core.sshCommand` or `default`.
    pub ssh_source: String,
    /// The SSH command Git runs, such as `ssh` or a path set in `core.sshCommand`.
    pub ssh_command: String,
    /// Where that command's program is.
    pub ssh_program: String,
    /// `ssh -V`.
    pub ssh_version: String,
    /// From `ssh -G`. Empty when the SSH program is not OpenSSH and could not say.
    pub user: String,
    pub hostname: String,
    pub port: String,
    /// `IdentitiesOnly yes`: only the identity files are offered, not other keys in the agent.
    pub identities_only: bool,
    pub identity_agent: Option<String>,
    /// The key files `ssh` tries, in its order.
    pub identity_files: Vec<IdentityFile>,
    pub agent: SshAgent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IdentityFile {
    pub path: String,
    pub state: IdentityFileState,
    /// From `ssh-keygen -l` when the file could be read.
    pub key: Option<KeyLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum IdentityFileState {
    Present,
    Missing,
    /// An old PEM key with no `.pub` beside it: its fingerprint, and any use of it, needs the
    /// passphrase, which nothing can type when the app runs Git.
    Encrypted,
    Unreadable,
}

/// One key as `ssh-add -l` and `ssh-keygen -l` print it: `256 SHA256:… me@laptop (ED25519)`.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct KeyLine {
    pub bits: Option<u32>,
    pub fingerprint: String,
    pub comment: String,
    pub key_type: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Type)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SshAgent {
    Keys { keys: Vec<KeyLine> },
    /// An agent answered and holds no keys.
    Empty,
    /// No agent could be reached; `detail` is what `ssh-add` said.
    #[default]
    Unavailable,
    Failed { detail: String },
}

/// The outcome of `ssh -T -v user@host`.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SshTest {
    pub outcome: SshTestOutcome,
    /// The command to run by hand, without the options that keep the test from waiting.
    pub command: String,
    pub exit_code: Option<i32>,
    /// Every key `ssh` considered, in its order, marked with whether it was offered and accepted.
    pub keys: Vec<TriedKey>,
    /// The whole of `ssh`'s output.
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SshTestOutcome {
    Authenticated,
    Denied,
    /// `Host key verification failed`: the host is not in known_hosts, or its key changed.
    HostKeyUnverified,
    CouldNotConnect,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TriedKey {
    /// The key file's path, or an agent key's comment.
    pub label: String,
    pub key_type: Option<String>,
    pub fingerprint: Option<String>,
    /// Where `ssh` found it: `explicit` (an IdentityFile), `agent`, and so on.
    pub source: String,
    pub offered: bool,
    pub accepted: bool,
}

/// What `ssh_check.sh` prints for `setup`, read into an `SshSetup`. `git_executable` is filled in
/// by the caller.
pub fn parse_setup(text: &str) -> SshSetup {
    let mut s = SshSetup::default();
    let mut section = "";
    let mut agent_lines: Vec<&str> = Vec::new();
    let mut agent_exit = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("@@") {
            let (tag, value) = rest.split_once(' ').unwrap_or((rest, ""));
            let value = value.trim().to_string();
            section = "";
            match tag {
                "git" => s.git_executable = value,
                "gitversion" => s.git_version = value,
                "source" => s.ssh_source = value,
                "sshcommand" => s.ssh_command = value,
                "program" => s.ssh_program = value,
                "version" => s.ssh_version = value,
                "config" => section = "config",
                "agent" => section = "agent",
                "agentexit" => agent_exit = value.parse::<i32>().ok(),
                _ => {}
            }
            continue;
        }
        match section {
            "config" => {
                let (k, v) = line.split_once(' ').unwrap_or((line, ""));
                match k {
                    "user" => s.user = v.to_string(),
                    "hostname" => s.hostname = v.to_string(),
                    "port" => s.port = v.to_string(),
                    "identitiesonly" => s.identities_only = v == "yes",
                    "identityagent" => s.identity_agent = Some(v.to_string()).filter(|a| a != "none" && !a.is_empty()),
                    "identityfile" => {
                        let (state, rest) = v.split_once(' ').unwrap_or((v, ""));
                        let (path, key) = rest.split_once('\t').unwrap_or((rest, ""));
                        let state = match state {
                            "present" => IdentityFileState::Present,
                            "missing" => IdentityFileState::Missing,
                            "encrypted" => IdentityFileState::Encrypted,
                            _ => IdentityFileState::Unreadable,
                        };
                        s.identity_files.push(IdentityFile { path: path.to_string(), state, key: parse_key_line(key) });
                    }
                    _ => {}
                }
            }
            "agent" => agent_lines.push(line),
            _ => {}
        }
    }
    let keys: Vec<KeyLine> = agent_lines.iter().filter_map(|l| parse_key_line(l)).collect();
    // ssh-add -l: 0 with keys, 1 when the agent has none, 2 when there is no agent.
    s.agent = match agent_exit {
        Some(0) if !keys.is_empty() => SshAgent::Keys { keys },
        Some(1) if agent_lines.iter().any(|l| l.contains("no identities")) => SshAgent::Empty,
        Some(2) => SshAgent::Unavailable,
        _ => SshAgent::Failed { detail: agent_lines.join("\n").trim().to_string() },
    };
    s
}

/// `2048 SHA256:… me@laptop (RSA)` into its parts. None for anything else.
pub fn parse_key_line(line: &str) -> Option<KeyLine> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let fp_at = words.iter().position(|w| w.starts_with("SHA256:") || w.starts_with("MD5:"))?;
    let bits = if fp_at > 0 { words[fp_at - 1].parse::<u32>().ok() } else { None };
    let rest = &words[fp_at + 1..];
    let (key_type, comment) = match rest.split_last() {
        Some((last, before)) if last.starts_with('(') && last.ends_with(')') => (last.trim_matches(['(', ')']).to_string(), before.join(" ")),
        _ => (String::new(), rest.join(" ")),
    };
    Some(KeyLine { bits, fingerprint: words[fp_at].to_string(), comment, key_type })
}

/// What `ssh_check.sh` prints for `test`, read into an `SshTest`.
pub fn parse_test(text: &str) -> SshTest {
    let mut command = String::new();
    let mut exit_code = None;
    let mut output = Vec::new();
    let mut keys: Vec<TriedKey> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(c) = line.strip_prefix("@@command ") {
            command = c.split_whitespace().collect::<Vec<_>>().join(" ");
            continue;
        }
        if let Some(c) = line.strip_prefix("@@exit ") {
            exit_code = c.trim().parse::<i32>().ok();
            continue;
        }
        // `testkey` says where the command came from; that is not part of ssh's output.
        if line.starts_with("@@source ") {
            continue;
        }
        output.push(line);
        let body = line.strip_prefix("debug1: ").unwrap_or(line);
        if let Some(rest) = body.strip_prefix("Will attempt key: ") {
            keys.push(tried_key(rest));
        } else if let Some(rest) = body.strip_prefix("Offering public key: ") {
            mark(&mut keys, rest, |k| k.offered = true);
        } else if let Some(rest) = body.strip_prefix("Server accepts key: ") {
            mark(&mut keys, rest, |k| {
                k.offered = true;
                k.accepted = true;
            });
        }
    }
    let all = output.join("\n");
    let outcome = if all.contains("Authenticated to ") {
        SshTestOutcome::Authenticated
    } else if all.contains("Host key verification failed") {
        SshTestOutcome::HostKeyUnverified
    } else if all.contains("Permission denied (") || all.contains("No more authentication methods") || all.contains("Too many authentication failures") {
        SshTestOutcome::Denied
    } else if ["Could not resolve hostname", "Connection timed out", "Connection refused", "Network is unreachable", "Operation timed out", "No route to host"]
        .iter()
        .any(|p| all.contains(p))
    {
        SshTestOutcome::CouldNotConnect
    } else {
        SshTestOutcome::Unknown
    };
    SshTest { outcome, command, exit_code, keys, output: all.trim().to_string() }
}

/// `/home/me/.ssh/id_rsa RSA SHA256:… explicit`, or just a path when the file is missing.
fn tried_key(rest: &str) -> TriedKey {
    let words: Vec<&str> = rest.split_whitespace().collect();
    match words.iter().position(|w| w.starts_with("SHA256:") || w.starts_with("MD5:")) {
        Some(fp_at) if fp_at >= 1 => TriedKey {
            label: words[..fp_at - 1].join(" "),
            key_type: Some(words[fp_at - 1].to_string()),
            fingerprint: Some(words[fp_at].to_string()),
            source: words[fp_at + 1..].join(" "),
            offered: false,
            accepted: false,
        },
        _ => TriedKey { label: rest.trim().to_string(), key_type: None, fingerprint: None, source: String::new(), offered: false, accepted: false },
    }
}

/// Applies `f` to the key an `Offering` or `accepts` line names, by fingerprint, adding it when
/// `ssh` did not list it first.
fn mark(keys: &mut Vec<TriedKey>, rest: &str, f: impl Fn(&mut TriedKey)) {
    let seen = tried_key(rest);
    let found = keys.iter_mut().find(|k| k.fingerprint.is_some() && k.fingerprint == seen.fingerprint);
    match found {
        Some(k) => f(k),
        None => {
            let mut k = seen;
            f(&mut k);
            keys.push(k);
        }
    }
}

/// `user@host`, refused when it could be read as an option or holds anything but a login.
fn check_destination(dest: &str) -> Result<()> {
    let ok = !dest.is_empty() && !dest.starts_with('-') && dest.chars().all(|c| c.is_ascii_alphanumeric() || "@._-:[]%".contains(c));
    if ok {
        Ok(())
    } else {
        Err(GitError::Internal { detail: format!("{dest} is not a user@host to test") })
    }
}

/// The Git command that runs `ssh_check.sh` in `mode` for `dest`.
fn check_request(mode: &str, dest: &str, port: Option<u16>, timeout: Duration) -> Result<GitReq> {
    check_destination(dest)?;
    let port = port.map(|p| p.to_string()).unwrap_or_default();
    // The check logs in with BatchMode, which never asks, so it gets no askpass helper either.
    let mut req = GitReq::new("ssh check", [ALIAS, mode, dest, port.as_str()]).user().own_session().askpass(crate::askpass::AskMode::Never).timeout(Some(timeout));
    // Git appends ` "$@"` to a shell alias, so the script must end on the call to `f`.
    req.config.push((format!("alias.{ALIAS}"), format!("!{}", SCRIPT.trim_end())));
    Ok(req)
}

/// Runs `ssh_check.sh` in `mode` (`setup`, `test` or `keys`) through `client`, and returns what it printed.
async fn run_check(client: &GitClient, mode: &str, dest: &str, port: Option<u16>, timeout: Duration) -> Result<String> {
    let out = client.exec_allow_failure(check_request(mode, dest, port, timeout)?).await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The request for `testkey`: a login with `command` as Git's SSH command for this one run. It
/// may ask for the key's passphrase, so it gets the askpass helper and two minutes.
fn test_key_request(dest: &str, port: Option<u16>, command: String) -> Result<GitReq> {
    let mut req = check_request("testkey", dest, port, Duration::from_secs(120))?.askpass(crate::askpass::AskMode::Ask);
    req.config.push(("core.sshCommand".into(), command));
    Ok(req)
}

async fn run_script(state: &AppState, key: &str, mode: &str, dest: &str, port: Option<u16>, timeout: Duration) -> Result<(String, crate::repo::RepoInfo)> {
    let h = repo(state, key).await?;
    let text = run_check(&h.client, mode, dest, port, timeout).await?;
    Ok((text, h.info.clone()))
}

/// Which Git and `ssh` the repository uses for `dest` (`git@github.com`), with the keys `ssh`
/// would offer and the ones its agent holds. `port` is the remote URL's, when it names one.
#[tauri::command]
#[specta::specta]
pub async fn remote_ssh_setup(state: State<'_, AppState>, key: String, dest: String, port: Option<u16>) -> Result<SshSetup> {
    let (text, info) = run_script(&state, &key, "setup", &dest, port, Duration::from_secs(30)).await?;
    let mut setup = parse_setup(&text);
    // Inside the alias `git` is the copy in git-core; the executable the app starts is the one to show.
    if let ResolvedTransport::Local { git_executable } = &info.transport {
        setup.git_executable = git_executable.clone();
    }
    Ok(setup)
}

/// Logs in to `dest` with the SSH command Git uses, without a shell and without waiting for a
/// password or passphrase, and reports which keys were offered and whether one was accepted.
#[tauri::command]
#[specta::specta]
pub async fn remote_ssh_test(state: State<'_, AppState>, key: String, dest: String, port: Option<u16>) -> Result<SshTest> {
    let (text, _) = run_script(&state, &key, "test", &dest, port, Duration::from_secs(40)).await?;
    Ok(parse_test(&text))
}

// ---- Trying another key, and using it for the repository -----------------------------------
//
// When the server refuses the keys `ssh` offers, the fix is often to name the right key for the
// host. StashPanda does that for one repository, in its own config: `core.sshCommand` becomes
// the SSH command Git already used, followed by `-i "<key>" -o IdentitiesOnly=yes`, so `ssh`
// offers that key and no other. A key is tried with exactly that command, given to Git for the
// one test with `-c`, so what was tested is what gets saved.

/// What ends a `core.sshCommand` that StashPanda wrote, after the key's path.
const KEY_TAIL: &str = "\" -o IdentitiesOnly=yes";
/// What comes before the key's path in it.
const KEY_FLAG: &str = " -i \"";

/// `command` without the key StashPanda added to it earlier, if it added one: choosing another
/// key replaces the first one rather than offering both.
pub fn base_ssh_command(command: &str) -> &str {
    let Some(without_tail) = command.strip_suffix(KEY_TAIL) else { return command };
    match without_tail.rfind(KEY_FLAG) {
        // The path itself holds no quote (`check_identity`), so this is the flag StashPanda wrote.
        Some(at) if !without_tail[at + KEY_FLAG.len()..].contains('"') => &command[..at],
        _ => command,
    }
}

/// The SSH command that offers only the key file `identity`, built on `command`.
pub fn ssh_command_with_key(command: &str, identity: &str) -> String {
    format!("{}{KEY_FLAG}{identity}{KEY_TAIL}", base_ssh_command(command))
}

/// A key file's path, refused when it could be read as an option or would not survive the
/// double quotes it is put in: Git runs `core.sshCommand` through a shell, where `"`, `$`, a
/// backtick and a backslash all mean something inside them.
fn check_identity(identity: &str) -> Result<()> {
    let ok = !identity.is_empty() && !identity.starts_with('-') && !identity.chars().any(|c| c.is_control() || "\"$`\\".contains(c));
    if ok {
        Ok(())
    } else {
        Err(GitError::Internal { detail: format!("{identity} cannot be named in an SSH command: its path has a quote, a dollar sign, a backtick or a backslash in it") })
    }
}

/// `@@source` and `@@sshcommand` from what `ssh_check.sh` printed: where Git's SSH command comes
/// from, and the command.
fn parse_command(text: &str) -> (String, String) {
    let line = |tag: &str| text.lines().find_map(|l| l.trim_end_matches('\r').strip_prefix(tag)).unwrap_or("").trim().to_string();
    (line("@@source "), line("@@sshcommand "))
}

/// What `ssh_check.sh` prints for `keys`, read into the key files.
pub fn parse_keys(text: &str) -> Vec<IdentityFile> {
    text.lines()
        .filter_map(|l| l.trim_end_matches('\r').strip_prefix("key "))
        .map(|v| {
            let (state, rest) = v.split_once(' ').unwrap_or((v, ""));
            let (path, key) = rest.split_once('\t').unwrap_or((rest, ""));
            let state = match state {
                "present" => IdentityFileState::Present,
                "encrypted" => IdentityFileState::Encrypted,
                _ => IdentityFileState::Unreadable,
            };
            IdentityFile { path: path.to_string(), state, key: parse_key_line(key) }
        })
        .collect()
}

/// Why a key cannot be set for the repository when Git's SSH command comes from `source`, or
/// None when it can. An environment variable is read before `core.sshCommand`, so the setting
/// would never be used.
fn cannot_set_reason(source: &str) -> Option<String> {
    match source {
        "default" | "core.sshCommand" => None,
        other => Some(format!(
            "Git takes its SSH command from the {other} environment variable here, which comes before the repository's core.sshCommand setting, so a key set there would not be used. Change or remove {other}, or name the key in your SSH config."
        )),
    }
}

/// The private key files in `~/.ssh` where Git runs for this repository (Windows, a WSL distro,
/// or the server of a repository opened over SSH), for choosing one to try.
#[tauri::command]
#[specta::specta]
pub async fn remote_ssh_keys(state: State<'_, AppState>, key: String, dest: String, port: Option<u16>) -> Result<Vec<IdentityFile>> {
    let (text, _) = run_script(&state, &key, "keys", &dest, port, Duration::from_secs(30)).await?;
    Ok(parse_keys(&text))
}

/// The keys to choose from for a repository's `core.sshCommand`, and where Git's SSH command
/// comes from there.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyList {
    /// What chose the SSH command: `GIT_SSH_COMMAND`, `GIT_SSH`, `core.sshCommand` or `default`.
    /// With one of the two environment variables, Git never reads `core.sshCommand`.
    pub source: String,
    /// The private key files in `~/.ssh` where Git runs.
    pub keys: Vec<IdentityFile>,
}

/// The `keys` mode of the script needs no login to look at; this stands in for one.
const NO_DESTINATION: &str = "localhost";

/// The private key files in `~/.ssh` where Git runs for this repository, and where Git's SSH
/// command comes from there, for the key list in Repository Settings. Unlike `remoteSshKeys`
/// it needs no remote that refused a key.
#[tauri::command]
#[specta::specta]
pub async fn repo_ssh_keys(state: State<'_, AppState>, key: String) -> Result<SshKeyList> {
    let h = repo(&state, &key).await?;
    let mut req = check_request("keys", NO_DESTINATION, None, Duration::from_secs(30))?;
    // Opening Repository Settings reads this, as it reads the settings: not a command the user
    // ran, so it stays out of the console's own-commands view. The Diagnosis dialog's checks are
    // asked for, and show there.
    req.category = crate::transport::Category::Internal;
    let text = String::from_utf8_lossy(&h.client.exec_allow_failure(req).await?.stdout).into_owned();
    let (source, _) = parse_command(&text);
    Ok(SshKeyList { source, keys: parse_keys(&text) })
}

/// Logs in to `dest` offering only the key file `identity`, with the command `remoteSshUseKey`
/// would save. Unlike `remoteSshTest` it may ask for the key's passphrase, through StashPanda
/// as a fetch does; it never offers a password. Fails when an environment variable sets Git's
/// SSH command here, since the key could then not be saved for the repository either.
#[tauri::command]
#[specta::specta]
pub async fn remote_ssh_test_key(state: State<'_, AppState>, key: String, dest: String, port: Option<u16>, identity: String) -> Result<SshTest> {
    check_identity(&identity)?;
    let h = repo(&state, &key).await?;
    let (source, command) = parse_command(&run_check(&h.client, "keys", &dest, port, Duration::from_secs(30)).await?);
    if let Some(reason) = cannot_set_reason(&source) {
        return Err(GitError::Internal { detail: reason });
    }
    let command = ssh_command_with_key(if command.is_empty() { "ssh" } else { &command }, &identity);
    let out = h.client.exec_allow_failure(test_key_request(&dest, port, command)?).await?;
    Ok(parse_test(&String::from_utf8_lossy(&out.stdout)))
}

/// Makes Git use the key file `identity`, and only that key, for this repository's SSH remotes:
/// sets `core.sshCommand` in the repository's own config to the SSH command Git used so far
/// followed by `-i "<identity>" -o IdentitiesOnly=yes`. A key set this way before is replaced.
/// Returns the command it saved. Fails, and changes nothing, when an environment variable sets
/// Git's SSH command here.
#[tauri::command]
#[specta::specta]
pub async fn remote_ssh_use_key(state: State<'_, AppState>, key: String, dest: String, port: Option<u16>, identity: String) -> Result<String> {
    check_identity(&identity)?;
    let h = repo(&state, &key).await?;
    let (source, command) = parse_command(&run_check(&h.client, "keys", &dest, port, Duration::from_secs(30)).await?);
    if let Some(reason) = cannot_set_reason(&source) {
        return Err(GitError::Internal { detail: reason });
    }
    let command = ssh_command_with_key(if command.is_empty() { "ssh" } else { &command }, &identity);
    let _gate = h.write_gate.lock().await;
    let change = crate::git::ops::ConfigChange::Set { key: "core.sshCommand".into(), value: command.clone() };
    let result = crate::git::ops::config_change(&h.client, crate::git::ops::ConfigScope::Local, &change).await?;
    if !result.ok {
        return Err(GitError::CommandFailed { label: "config".into(), argv: vec![], exit_code: result.exit_code, stderr: result.stderr });
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS_SETUP: &str = "@@git /mingw64/libexec/git-core/git\n@@gitversion git version 2.47.1.windows.1\n@@source default\n@@sshcommand ssh\n@@program C:\\Program Files\\Git\\usr\\bin\\ssh.exe\n@@version OpenSSH_9.9p1, OpenSSL 3.2.3 3 Sep 2024\n@@config\nuser git\nhostname github.com\nport 22\nidentitiesonly no\nidentityfile encrypted C:\\Users\\me\\.ssh\\id_rsa\t\nidentityfile missing C:\\Users\\me\\.ssh\\id_ed25519\t\n@@agent\nCould not open a connection to your authentication agent.\n@@agentexit 2\n";

    #[test]
    fn reads_the_setup() {
        let s = parse_setup(WINDOWS_SETUP);
        assert_eq!(s.git_version, "git version 2.47.1.windows.1");
        assert_eq!(s.ssh_source, "default");
        assert_eq!(s.ssh_program, r"C:\Program Files\Git\usr\bin\ssh.exe");
        assert_eq!((s.user.as_str(), s.hostname.as_str(), s.port.as_str(), s.identities_only), ("git", "github.com", "22", false));
        assert_eq!(s.identity_files.len(), 2);
        assert_eq!(s.identity_files[0].state, IdentityFileState::Encrypted);
        assert_eq!(s.identity_files[0].path, r"C:\Users\me\.ssh\id_rsa");
        assert_eq!(s.identity_files[1].state, IdentityFileState::Missing);
        assert_eq!(s.agent, SshAgent::Unavailable);
    }

    #[test]
    fn reads_present_keys_and_the_agent() {
        let text = "@@config\nidentitiesonly yes\nidentityfile present /home/me/.ssh/id_rsa\t2048 SHA256:abc no comment (RSA)\n@@agent\n256 SHA256:def me@laptop (ED25519)\n@@agentexit 0\n";
        let s = parse_setup(text);
        assert!(s.identities_only);
        let key = s.identity_files[0].key.clone().unwrap();
        assert_eq!((key.bits, key.fingerprint.as_str(), key.comment.as_str(), key.key_type.as_str()), (Some(2048), "SHA256:abc", "no comment", "RSA"));
        match s.agent {
            SshAgent::Keys { keys } => assert_eq!((keys[0].fingerprint.as_str(), keys[0].comment.as_str()), ("SHA256:def", "me@laptop")),
            other => panic!("{other:?}"),
        }
        assert_eq!(parse_setup("@@agent\nThe agent has no identities.\n@@agentexit 1\n").agent, SshAgent::Empty);
    }

    #[test]
    fn reads_an_accepted_login() {
        let text = "@@command ssh -T -v  git@github.com\ndebug1: Will attempt key: /home/me/.ssh/id_rsa RSA SHA256:abc explicit\ndebug1: Offering public key: /home/me/.ssh/id_rsa RSA SHA256:abc explicit\ndebug1: Server accepts key: /home/me/.ssh/id_rsa RSA SHA256:abc explicit\nAuthenticated to github.com ([192.0.2.1]:22) using \"publickey\".\nHi me! You've successfully authenticated, but GitHub does not provide shell access.\n@@exit 1\n";
        let t = parse_test(text);
        assert_eq!(t.outcome, SshTestOutcome::Authenticated);
        assert_eq!(t.command, "ssh -T -v git@github.com");
        assert_eq!(t.exit_code, Some(1));
        assert_eq!(t.keys.len(), 1);
        let k = &t.keys[0];
        assert_eq!((k.label.as_str(), k.key_type.as_deref(), k.source.as_str(), k.offered, k.accepted), ("/home/me/.ssh/id_rsa", Some("RSA"), "explicit", true, true));
    }

    #[test]
    fn reads_a_refusal_with_nothing_offered() {
        let text = "@@command ssh -T -v  git@github.com\ndebug1: Will attempt key: /c/Users/me/.ssh/id_rsa \ndebug1: Will attempt key: /c/Users/me/.ssh/id_ed25519 \ngit@github.com: Permission denied (publickey).\n@@exit 255\n";
        let t = parse_test(text);
        assert_eq!(t.outcome, SshTestOutcome::Denied);
        assert_eq!(t.keys.len(), 2);
        assert!(t.keys.iter().all(|k| !k.offered && k.fingerprint.is_none()));
        assert_eq!(t.keys[0].label, "/c/Users/me/.ssh/id_rsa");
    }

    #[test]
    fn reads_connection_and_host_key_failures() {
        assert_eq!(parse_test("Host key verification failed.\n@@exit 255").outcome, SshTestOutcome::HostKeyUnverified);
        assert_eq!(parse_test("ssh: Could not resolve hostname nope: Name or service not known\n@@exit 255").outcome, SshTestOutcome::CouldNotConnect);
    }

    /// The script through a real `GitClient`, as the commands run it: the setup from a repository
    /// whose `core.sshCommand` names a key of its own, and a test login to a host that cannot exist.
    #[test]
    fn builds_the_command_for_a_key_and_replaces_a_key_set_before() {
        assert_eq!(ssh_command_with_key("ssh", "C:/Users/me/.ssh/work"), "ssh -i \"C:/Users/me/.ssh/work\" -o IdentitiesOnly=yes");
        // A path with a space, on a command that names its own program.
        let first = ssh_command_with_key("C:/Windows/System32/OpenSSH/ssh.exe", "/home/me/.ssh/a key");
        assert_eq!(first, "C:/Windows/System32/OpenSSH/ssh.exe -i \"/home/me/.ssh/a key\" -o IdentitiesOnly=yes");
        assert_eq!(base_ssh_command(&first), "C:/Windows/System32/OpenSSH/ssh.exe");
        assert_eq!(ssh_command_with_key(&first, "/home/me/.ssh/b"), "C:/Windows/System32/OpenSSH/ssh.exe -i \"/home/me/.ssh/b\" -o IdentitiesOnly=yes");
        // A command somebody wrote by hand keeps its own -i: only StashPanda's own form is replaced.
        let own = "ssh -i ~/.ssh/mine -o IdentitiesOnly=yes";
        assert_eq!(base_ssh_command(own), own);
        assert_eq!(ssh_command_with_key(own, "/k"), "ssh -i ~/.ssh/mine -o IdentitiesOnly=yes -i \"/k\" -o IdentitiesOnly=yes");
    }

    #[test]
    fn refuses_a_key_path_that_would_break_the_command() {
        assert!(check_identity("C:/Users/me/.ssh/id (work)").is_ok());
        assert!(check_identity("/home/me/.ssh/id_ed25519").is_ok());
        for bad in ["", "-oProxyCommand=x", "/k\"; rm -rf ~; \"", "/k$(id)", "/k`id`", "C:\\Users\\me\\.ssh\\id", "/k\nx"] {
            assert!(check_identity(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn says_when_an_environment_variable_would_overrule_the_setting() {
        assert_eq!(cannot_set_reason("default"), None);
        assert_eq!(cannot_set_reason("core.sshCommand"), None);
        assert!(cannot_set_reason("GIT_SSH_COMMAND").is_some_and(|r| r.contains("GIT_SSH_COMMAND environment variable")));
        assert!(cannot_set_reason("GIT_SSH").is_some());
    }

    #[test]
    fn reads_the_key_list() {
        let text = "@@source default\n@@sshcommand ssh\n@@keys\nkey present C:/Users/me/.ssh/id_ed25519\t256 SHA256:abc me@laptop (ED25519)\r\nkey encrypted C:/Users/me/.ssh/old_rsa\t\nkey unreadable /home/me/.ssh/odd\t\n";
        let keys = parse_keys(text);
        assert_eq!(keys.iter().map(|k| (k.path.as_str(), k.state)).collect::<Vec<_>>(), [
            ("C:/Users/me/.ssh/id_ed25519", IdentityFileState::Present),
            ("C:/Users/me/.ssh/old_rsa", IdentityFileState::Encrypted),
            ("/home/me/.ssh/odd", IdentityFileState::Unreadable),
        ]);
        assert_eq!(keys[0].key.as_ref().map(|k| k.fingerprint.as_str()), Some("SHA256:abc"));
        assert_eq!(parse_command(text), ("default".to_string(), "ssh".to_string()));
    }

    #[tokio::test]
    async fn runs_through_git_as_an_alias() {
        use crate::git::console::ConsoleRecorder;
        use crate::git::versions::Capabilities;
        use crate::transport::local::LocalTransport;
        use crate::transport::paths::{IdentityMapper, NativePath};
        use std::process::Command;
        use std::sync::Arc;
        let (Ok(git), Ok(_), Ok(_)) = (which::which("git"), which::which("ssh"), which::which("ssh-keygen")) else { return };
        let dir = std::env::temp_dir().join(format!("stashpanda-ssh-check-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| assert!(Command::new(args[0]).args(&args[1..]).current_dir(&dir).output().unwrap().status.success(), "{args:?}");
        run(&["git", "init", "-q"]);
        let key = dir.join("id_test");
        run(&["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "me@example.com", "-f", key.to_str().unwrap()]);
        // Forward slashes: Git runs core.sshCommand through a shell, which would eat a backslash.
        let ssh = format!("ssh -F /dev/null -o IdentitiesOnly=yes -i {}", key.to_string_lossy().replace('\\', "/"));
        run(&["git", "config", "core.sshCommand", &ssh]);

        let version = crate::git::versions::GitVersion::parse(&Command::new(&git).arg("version").output().unwrap().stdout).unwrap();
        let client = GitClient {
            transport: Arc::new(LocalTransport { git, mapper: Box::new(IdentityMapper), windows_git_from_linux: false }),
            repo: NativePath(dir.to_string_lossy().into_owned()),
            caps: Capabilities::for_version(version),
            console: Arc::new(ConsoleRecorder::new(None, "ssh-check".into(), 4096)),
        };

        let setup = parse_setup(&run_check(&client, "setup", "git@example.invalid", Some(2222), Duration::from_secs(30)).await.unwrap());
        assert_eq!(setup.ssh_source, "core.sshCommand");
        assert_eq!(setup.ssh_command, ssh);
        assert!(setup.ssh_version.contains("OpenSSH"), "{setup:?}");
        assert_eq!((setup.user.as_str(), setup.hostname.as_str(), setup.port.as_str()), ("git", "example.invalid", "2222"));
        let ours = setup.identity_files.iter().find(|f| f.path == key.to_string_lossy()).expect("the -i key is listed");
        assert_eq!(ours.state, IdentityFileState::Present);
        assert_eq!(ours.key.as_ref().map(|k| (k.comment.as_str(), k.key_type.as_str())), Some(("me@example.com", "ED25519")));

        let test = parse_test(&run_check(&client, "test", "git@example.invalid", None, Duration::from_secs(30)).await.unwrap());
        assert_eq!(test.outcome, SshTestOutcome::CouldNotConnect, "{}", test.output);
        assert_eq!(test.command, format!("{ssh} -T -v git@example.invalid"));

        // The key files in ~/.ssh where Git runs: private keys only, with a path free of backslashes.
        let home = dir.join("home");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        let other = home.join(".ssh").join("id_other");
        run(&["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "other@example.com", "-f", other.to_str().unwrap()]);
        std::fs::write(home.join(".ssh").join("known_hosts"), "example.invalid ssh-ed25519 AAAA\n").unwrap();
        std::fs::write(home.join(".ssh").join("config"), "Host *\n").unwrap();
        std::fs::write(home.join(".ssh").join("notes.txt"), "not a key\n").unwrap();
        let listing = check_request("keys", "git@example.invalid", None, Duration::from_secs(30)).unwrap().env("HOME", home.to_str().unwrap());
        let listed = String::from_utf8_lossy(&client.exec_allow_failure(listing).await.unwrap().stdout).into_owned();
        let keys = parse_keys(&listed);
        assert_eq!(keys.len(), 1, "{listed}");
        assert_eq!(keys[0].state, IdentityFileState::Present);
        assert_eq!(keys[0].key.as_ref().map(|k| k.comment.as_str()), Some("other@example.com"));
        assert!(keys[0].path.ends_with("/.ssh/id_other") && !keys[0].path.contains('\\'), "{}", keys[0].path);
        assert_eq!(parse_command(&listed), ("core.sshCommand".to_string(), ssh.clone()));

        // Trying that key: Git's SSH command for the one run is the repository's own with the
        // key added, and the script says it came from core.sshCommand.
        let command = ssh_command_with_key(&ssh, &keys[0].path);
        let tried = client.exec_allow_failure(test_key_request("git@example.invalid", None, command.clone()).unwrap()).await.unwrap();
        let tried = String::from_utf8_lossy(&tried.stdout).into_owned();
        assert!(tried.contains("@@source core.sshCommand"), "{tried}");
        let tried = parse_test(&tried);
        assert_eq!(tried.outcome, SshTestOutcome::CouldNotConnect, "{}", tried.output);
        assert_eq!(tried.command, format!("{command} -T -v git@example.invalid"));
        assert!(!tried.output.contains("@@source"), "{}", tried.output);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_a_destination_that_could_be_an_option() {
        assert!(check_destination("git@github.com").is_ok());
        assert!(check_destination("me@[2001:db8::1]").is_ok());
        assert!(check_destination("-oProxyCommand=sh").is_err());
        assert!(check_destination("git@host; rm").is_err());
        assert!(check_destination("").is_err());
    }
}
