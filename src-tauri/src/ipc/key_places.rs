//! The Key Manager's places (Windows, each WSL distro, this machine, and `~/.ssh` on the server of
//! each repository opened over SSH), and copying a key from one place to another.
//!
//! A copy never overwrites and never deletes a key file. A name that is taken anywhere it would
//! write, private key or `.pub`, fails with `KeyExists` before anything is written, and the
//! writes themselves only create: `create_new` on a local disk, `set -C` (noclobber, an exclusive
//! create) in a WSL distro's or a server's shell.

use std::path::{Path, PathBuf};

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::error::{GitError, Result};
use crate::ipc::ssh_check::parse_key_line;
use crate::keys::{self, KeyInfo, KeyPlace};
use crate::platform::Host;
use crate::settings::ResolvedTransport;
use crate::state::AppState;
use crate::transport::paths::{NativePath, WslMapper};
use crate::transport::wsl::WslTransport;
use crate::transport::{Cmd, Program, SharedTransport, TransportKind};

/// One folder the Key Manager reads keys from, with the tab it belongs in.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct KeyFolder {
    pub path: String,
    pub place: KeyPlace,
    /// Added with Add Folder…, so it can be removed again.
    pub extra: bool,
}

/// The default key folders and `extra_dirs`, each with its place.
#[tauri::command]
#[specta::specta]
pub async fn keys_folders(state: State<'_, AppState>, extra_dirs: Vec<String>) -> Result<Vec<KeyFolder>> {
    let defaults = crate::ipc::keys::default_dirs(&state.host).await;
    let mut out: Vec<KeyFolder> = defaults
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .map(|path| KeyFolder { place: keys::place_of(&path, &state.host), path, extra: false })
        .collect();
    for path in extra_dirs.into_iter().filter(|d| !d.trim().is_empty()) {
        if !out.iter().any(|f| f.path == path) {
            out.push(KeyFolder { place: keys::place_of(&path, &state.host), path, extra: true });
        }
    }
    Ok(out)
}

/// Sets each key's place, and whether the SSH agent of that place holds it: the app's own agent
/// for this machine, `ssh-add -l` inside the distro for a WSL distro seen from Windows.
pub async fn fill_places(host: &Host, infos: &mut [KeyInfo]) {
    for info in infos.iter_mut() {
        info.place = keys::place_of(&info.path, host);
    }
    let own = keys::agent::list_fingerprints().await.unwrap_or_default();
    let mut distro_agents: Vec<(String, Vec<String>)> = Vec::new();
    for info in infos.iter_mut() {
        let Some(fp) = info.fingerprint.clone() else { continue };
        let in_agent = match (&info.place, host) {
            (KeyPlace::Wsl { distro }, Host::Windows) => {
                if !distro_agents.iter().any(|(d, _)| d == distro) {
                    let fps = agent_fingerprints(&wsl_transport(distro)).await;
                    distro_agents.push((distro.clone(), fps));
                }
                distro_agents.iter().find(|(d, _)| d == distro).is_some_and(|(_, fps)| fps.contains(&fp))
            }
            _ => own.contains(&fp),
        };
        info.in_agent = in_agent;
    }
}

fn wsl_transport(distro: &str) -> SharedTransport {
    std::sync::Arc::new(WslTransport {
        distro: distro.to_string(),
        git: "git".into(),
        mapper: WslMapper { distro: distro.to_string(), automount_root: "/mnt".into() },
    })
}

/// Runs `script` with `sh -c` through `transport`, with `args` as `$1`, `$2`, …
async fn shell(transport: &SharedTransport, label: &'static str, script: &str, args: &[&str], stdin: Option<Vec<u8>>) -> Result<crate::transport::Output> {
    let mut all = vec!["-c".to_string(), script.to_string(), "sh".to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    let mut cmd = Cmd::git(label, all);
    cmd.program = Program::Other("sh".into());
    if let Some(bytes) = stdin {
        cmd = cmd.stdin_bytes(bytes);
    }
    transport.run(cmd).await
}

/// The fingerprints `ssh-add -l` lists where `transport` runs; empty when no agent answers.
async fn agent_fingerprints(transport: &SharedTransport) -> Vec<String> {
    match shell(transport, "ssh-add", "ssh-add -l 2>/dev/null", &[], None).await {
        Ok(out) => String::from_utf8_lossy(&out.stdout).lines().filter_map(parse_key_line).map(|k| k.fingerprint).collect(),
        Err(_) => Vec::new(),
    }
}

/// The keys in `~/.ssh` on one server.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RemoteKeys {
    /// The open repository whose connection reached the server.
    pub repo_key: String,
    /// `user@host`, with `:port` when it is not 22.
    pub server: String,
    /// The server's `~/.ssh`, empty when it could not be read.
    pub folder: String,
    pub keys: Vec<KeyInfo>,
    /// Why the folder could not be read.
    pub error: Option<String>,
}

const SKIP_NAMES: &[&str] = &["known_hosts", "known_hosts.old", "config", "authorized_keys", "authorized_keys2", "environment", "rc"];

/// `~/.ssh` on the server of every open repository reached over SSH, one entry per server. Only
/// open repositories are used, so this reads through connections already made and never logs in.
#[tauri::command]
#[specta::specta]
pub async fn keys_scan_remote(state: State<'_, AppState>) -> Result<Vec<RemoteKeys>> {
    let handles: Vec<_> = state.repos.read().await.values().cloned().collect();
    let mut out: Vec<RemoteKeys> = Vec::new();
    for h in handles {
        if !matches!(h.transport.kind(), TransportKind::Ssh { .. }) {
            continue;
        }
        let server = server_label(&h.info.transport);
        if out.iter().any(|r| r.server == server) {
            continue;
        }
        let mut entry = RemoteKeys { repo_key: h.key.clone(), server: server.clone(), folder: String::new(), keys: Vec::new(), error: None };
        match remote_keys(&h.transport, &h.key, &server).await {
            Ok((folder, keys)) => {
                entry.folder = folder;
                entry.keys = keys;
            }
            Err(e) => entry.error = Some(e.to_string()),
        }
        out.push(entry);
    }
    Ok(out)
}

fn server_label(t: &ResolvedTransport) -> String {
    match t {
        ResolvedTransport::Ssh { host, user, port } => {
            let user = user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
            match port {
                Some(p) if *p != 22 => format!("{user}{host}:{p}"),
                _ => format!("{user}{host}"),
            }
        }
        _ => String::new(),
    }
}

async fn remote_home_ssh(transport: &SharedTransport) -> Result<String> {
    let out = shell(transport, "home", r#"printf '%s' "$HOME""#, &[], None).await?;
    let home = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if home.is_empty() {
        return Err(GitError::Io { detail: "the server did not say where its home folder is".into() });
    }
    Ok(format!("{}/.ssh", home.trim_end_matches('/')))
}

async fn remote_keys(transport: &SharedTransport, repo_key: &str, server: &str) -> Result<(String, Vec<KeyInfo>)> {
    let folder = remote_home_ssh(transport).await?;
    let Some(names) = transport.list_dir(&NativePath(folder.clone())).await? else { return Ok((folder, Vec::new())) };
    let fps = agent_fingerprints(transport).await;
    let mut keys = Vec::new();
    let mut sorted = names.clone();
    sorted.sort();
    for name in sorted {
        if SKIP_NAMES.contains(&name.as_str()) || name.ends_with(".pub") || name.starts_with('.') {
            continue;
        }
        let path = format!("{folder}/{name}");
        let Ok(bytes) = transport.read_file(&NativePath(path.clone()), 64 * 1024).await else { continue };
        if !keys::is_private_key_bytes(&bytes) {
            continue;
        }
        let pub_name = format!("{name}.pub");
        let public = if names.contains(&pub_name) {
            let pub_path = format!("{folder}/{pub_name}");
            transport.read_file(&NativePath(pub_path.clone()), 64 * 1024).await.ok().map(|b| (pub_path, String::from_utf8_lossy(&b).into_owned()))
        } else {
            None
        };
        let mut info = keys::inspect_bytes(&path, &bytes, public);
        info.place = KeyPlace::Remote { repo_key: repo_key.to_string(), server: server.to_string() };
        info.in_agent = info.fingerprint.as_ref().is_some_and(|f| fps.contains(f));
        keys.push(info);
    }
    Ok((folder, keys))
}

/// A file name for a key copy: one plain name, which is not a public key or another ssh file.
fn check_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.starts_with('.')
        || name.ends_with(".pub")
        || name.contains(['/', '\\', ':', '\0', '\n'])
        || SKIP_NAMES.contains(&name);
    if bad {
        return Err(GitError::InvalidPath { path: name.to_string(), reason: "a key's file name has no slashes, does not start with a dot or end in .pub, and is not one of ssh's own files".into() });
    }
    Ok(())
}

/// What the copy writes: the private key, and the public key when the source has one.
struct KeyFiles {
    private: Vec<u8>,
    public: Option<Vec<u8>>,
}

async fn read_source(state: &AppState, path: &str, place: &KeyPlace) -> Result<KeyFiles> {
    if let KeyPlace::Remote { repo_key, .. } = place {
        let h = crate::ipc::query::repo(state, repo_key).await?;
        let private = h.transport.read_file(&NativePath(path.to_string()), 64 * 1024).await?;
        let public = h.transport.read_file(&NativePath(format!("{path}.pub")), 64 * 1024).await.ok();
        return Ok(KeyFiles { private, public });
    }
    let path = path.to_string();
    tokio::task::spawn_blocking(move || -> Result<KeyFiles> {
        let private = std::fs::read(&path)?;
        let public = std::fs::read(format!("{path}.pub")).ok();
        Ok(KeyFiles { private, public })
    })
    .await
    .map_err(|e| GitError::internal(e.to_string()))?
}

fn write_local(folder: &Path, name: &str, files: &KeyFiles) -> Result<String> {
    let private = folder.join(name);
    let public = folder.join(format!("{name}.pub"));
    for p in [&private, &public] {
        if p.exists() {
            return Err(GitError::KeyExists { path: p.to_string_lossy().into_owned() });
        }
    }
    std::fs::create_dir_all(folder)?;
    keys::create_new(&private, &files.private, true)?;
    if let Some(bytes) = &files.public {
        keys::create_new(&public, bytes, false)?;
    }
    Ok(private.to_string_lossy().into_owned())
}

/// Writes one file into `~/.ssh` where `transport` runs, creating the folder (owner-only) if it is
/// missing. `set -C` makes the shell refuse to write over a file that is there, exit 3 says so.
const SHELL_CREATE: &str = r#"umask "$2"; mkdir -p "$HOME/.ssh" && chmod 700 "$HOME/.ssh" 2>/dev/null; cd "$HOME/.ssh" || exit 1; if [ -e "$1" ] || [ -L "$1" ]; then exit 3; fi; set -C; cat > "$1" || exit 3; printf '%s/%s' "$HOME/.ssh" "$1""#;
/// Prints which of the names given are taken in `~/.ssh`.
const SHELL_TAKEN: &str = r#"cd "$HOME/.ssh" 2>/dev/null || exit 0; for f; do if [ -e "$f" ] || [ -L "$f" ]; then printf '%s\n' "$f"; fi; done"#;

async fn write_shell(transport: &SharedTransport, where_: &str, name: &str, files: &KeyFiles) -> Result<String> {
    let pub_name = format!("{name}.pub");
    let taken = shell(transport, "key names", SHELL_TAKEN, &[name, &pub_name], None).await?;
    if let Some(first) = String::from_utf8_lossy(&taken.stdout).lines().next() {
        return Err(GitError::KeyExists { path: format!("{first} in ~/.ssh on {where_}") });
    }
    let create = |file: String, umask: &'static str, bytes: Vec<u8>| async move {
        let out = shell(transport, "copy key", SHELL_CREATE, &[&file, umask], Some(bytes)).await?;
        match out.exit_code {
            Some(0) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
            Some(3) => Err(GitError::KeyExists { path: format!("{file} in ~/.ssh on {where_}") }),
            _ => Err(GitError::Io { detail: format!("could not write {file} in ~/.ssh on {where_}: {}", String::from_utf8_lossy(&out.stderr).trim()) }),
        }
    };
    let written = create(name.to_string(), "077", files.private.clone()).await?;
    if let Some(bytes) = &files.public {
        create(pub_name, "022", bytes.clone()).await?;
    }
    Ok(written)
}

/// Copies the key at `source_path` (in `source_place`) to `target`, as `name`, with its `.pub`
/// beside it when it has one. `folder` is the folder to write to for Windows and this machine;
/// a WSL distro reached from Windows and a server get `~/.ssh` of their own user. Never
/// overwrites: a name that is taken fails with `KeyExists`. Returns where the key was written.
#[tauri::command]
#[specta::specta]
pub async fn keys_copy(
    state: State<'_, AppState>,
    source_path: String,
    source_place: KeyPlace,
    target: KeyPlace,
    folder: Option<String>,
    name: String,
) -> Result<String> {
    let name = name.trim().to_string();
    check_name(&name)?;
    if target == source_place {
        return Err(GitError::Internal { detail: "the key is already there; choose another place".into() });
    }
    let files = read_source(&state, &source_path, &source_place).await?;
    if !keys::is_private_key_bytes(&files.private) {
        return Err(GitError::Internal { detail: format!("{source_path} is not a private key") });
    }
    match (&target, &state.host) {
        (KeyPlace::Wsl { distro }, Host::Windows) => write_shell(&wsl_transport(distro), &format!("WSL ({distro})"), &name, &files).await,
        (KeyPlace::Remote { repo_key, server }, _) => {
            let h = crate::ipc::query::repo(&state, repo_key).await?;
            write_shell(&h.transport, server, &name, &files).await
        }
        _ => {
            let folder = folder.ok_or_else(|| GitError::Internal { detail: "no folder was given to copy the key into".into() })?;
            tokio::task::spawn_blocking(move || write_local(&PathBuf::from(folder), &name, &files))
                .await
                .map_err(|e| GitError::internal(e.to_string()))?
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stashpanda-key-copy-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copies_a_key_and_never_overwrites_one() {
        let dir = temp();
        let files = KeyFiles { private: b"PRIVATE".to_vec(), public: Some(b"PUBLIC".to_vec()) };
        let written = write_local(&dir.join(".ssh"), "id_test", &files).unwrap();
        assert_eq!(std::fs::read(&written).unwrap(), b"PRIVATE");
        assert_eq!(std::fs::read(dir.join(".ssh/id_test.pub")).unwrap(), b"PUBLIC");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&written).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // The same name again: refused, and both files keep what they had.
        let other = KeyFiles { private: b"OTHER".to_vec(), public: Some(b"OTHER".to_vec()) };
        assert!(matches!(write_local(&dir.join(".ssh"), "id_test", &other), Err(GitError::KeyExists { .. })));
        assert_eq!(std::fs::read(&written).unwrap(), b"PRIVATE");
        assert_eq!(std::fs::read(dir.join(".ssh/id_test.pub")).unwrap(), b"PUBLIC");

        // A name whose .pub alone is taken is refused before the private key is written.
        std::fs::write(dir.join(".ssh/id_only.pub"), b"KEEP").unwrap();
        assert!(matches!(write_local(&dir.join(".ssh"), "id_only", &other), Err(GitError::KeyExists { .. })));
        assert!(!dir.join(".ssh/id_only").exists());
        assert_eq!(std::fs::read(dir.join(".ssh/id_only.pub")).unwrap(), b"KEEP");

        // create_new itself refuses, whatever was checked before it.
        assert!(matches!(keys::create_new(Path::new(&written), b"X", true), Err(GitError::KeyExists { .. })));
        assert_eq!(std::fs::read(&written).unwrap(), b"PRIVATE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_names_that_are_not_a_plain_key_file() {
        assert!(check_name("id_ed25519_work").is_ok());
        for bad in ["", ".hidden", "id.pub", "../id", r"a\b", "known_hosts", "authorized_keys", "c:id"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    /// A copy into a WSL distro from Windows, through `wsl.exe` as `keys_copy` runs it, into a
    /// throwaway HOME in the distro's /tmp: written once with owner-only permissions, then refused.
    #[cfg(windows)]
    #[tokio::test]
    async fn copies_into_wsl_and_never_overwrites() {
        let Some(distro) = crate::transport::wsl::list_running_distros().await.ok().and_then(|d| d.into_iter().next()) else { return };
        let t = wsl_transport(&distro);
        let home = format!("/tmp/stashpanda-key-copy-{}", uuid::Uuid::new_v4().simple());
        let with_home = |script: &str| format!("HOME='{home}'; export HOME; {script}");
        let files = KeyFiles { private: b"PRIVATE\n".to_vec(), public: None };
        let create = with_home(SHELL_CREATE);
        let first = shell(&t, "t", &create, &["id_x", "077"], Some(files.private.clone())).await.unwrap();
        assert_eq!(first.exit_code, Some(0), "{}", String::from_utf8_lossy(&first.stderr));
        assert_eq!(String::from_utf8_lossy(&first.stdout), format!("{home}/.ssh/id_x"));
        let second = shell(&t, "t", &create, &["id_x", "077"], Some(b"OTHER\n".to_vec())).await.unwrap();
        assert_eq!(second.exit_code, Some(3));
        let check = shell(&t, "t", r#"cat "$1/.ssh/id_x"; stat -c %a "$1/.ssh/id_x""#, &[&home], None).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&check.stdout), "PRIVATE\n600\n");
        let taken = shell(&t, "t", &with_home(SHELL_TAKEN), &["id_x", "id_x.pub"], None).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&taken.stdout), "id_x\n");
        let _ = shell(&t, "t", r#"rm -rf -- "$1""#, &[&home], None).await;
    }

    /// The shell writes as they run inside a WSL distro or on a server: never over a file.
    #[tokio::test]
    async fn the_shell_write_never_overwrites() {
        use crate::transport::local::LocalTransport;
        use crate::transport::paths::IdentityMapper;
        if which::which("sh").is_err() {
            return;
        }
        let home = temp();
        let transport: SharedTransport = std::sync::Arc::new(LocalTransport { git: "git".into(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false });
        let run = |args: Vec<&str>, bytes: &[u8]| {
            let mut all = vec!["-c".to_string(), format!("HOME='{}'; export HOME; {SHELL_CREATE}", home.display()), "sh".to_string()];
            all.extend(args.iter().map(|a| a.to_string()));
            let mut cmd = Cmd::git("t", all).stdin_bytes(bytes.to_vec());
            cmd.program = Program::Other("sh".into());
            let t = transport.clone();
            async move { t.run(cmd).await.unwrap() }
        };
        let first = run(vec!["id_x", "077"], b"ONE").await;
        assert_eq!(first.exit_code, Some(0));
        let second = run(vec!["id_x", "077"], b"TWO").await;
        assert_eq!(second.exit_code, Some(3));
        assert_eq!(std::fs::read(home.join(".ssh/id_x")).unwrap(), b"ONE");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(home.join(".ssh/id_x")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
