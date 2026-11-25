//! End to end: `git fetch` over `ssh` with an encrypted key, where `ssh` starts the real
//! StashPanda executable as its askpass helper and the helper asks an askpass server in this
//! test. Against a private `sshd` on a free port with throwaway keys, so nothing on the machine
//! changes. Skips when sshd, ssh, ssh-keygen or git is missing.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use stashpanda_lib::askpass::{AskMode, Askpass, AskpassKind, UiEvent};
use stashpanda_lib::keys::secrets::PassphraseCache;
use stashpanda_lib::transport::local::LocalTransport;
use stashpanda_lib::transport::paths::IdentityMapper;
use stashpanda_lib::transport::{Cmd, Output, SharedTransport};

struct Sshd(Child);
impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run(program: &str, args: &[&str]) {
    let st = Command::new(program).args(args).stdout(Stdio::null()).status().unwrap();
    assert!(st.success(), "{program} {args:?}");
}

fn start_sshd(dir: &Path, port: u16) -> Sshd {
    let cfg = format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nUsePAM no\nStrictModes no\nPidFile none\nLogLevel ERROR\n",
        dir.join("host_key").display(),
        dir.join("id_test.pub").display(),
    );
    std::fs::write(dir.join("sshd_config"), cfg).unwrap();
    let log = std::fs::File::create(dir.join("sshd.log")).unwrap();
    let child = Command::new("/usr/sbin/sshd").arg("-f").arg(dir.join("sshd_config")).args(["-D", "-e"]).stdout(Stdio::null()).stderr(log).spawn().expect("sshd starts");
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(std::time::Instant::now() < deadline, "sshd did not listen: {}", std::fs::read_to_string(dir.join("sshd.log")).unwrap_or_default());
        std::thread::sleep(Duration::from_millis(100));
    }
    Sshd(child)
}

#[tokio::test(flavor = "multi_thread")]
async fn git_fetch_asks_the_app_for_the_passphrase_of_an_encrypted_key() {
    if !Path::new("/usr/sbin/sshd").exists() || ["ssh", "ssh-keygen", "git"].iter().any(|p| which::which(p).is_err()) {
        eprintln!("sshd, ssh, ssh-keygen or git missing; skipping");
        return;
    }
    let Ok(user) = std::env::var("USER") else { return };
    let dir = std::env::temp_dir().join(format!("stashpanda-askpass-e2e-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();

    // The server's key, and the user's key, encrypted with the passphrase `secret`.
    run("ssh-keygen", &["-q", "-N", "", "-t", "ed25519", "-f", &path("host_key")]);
    run("ssh-keygen", &["-q", "-N", "secret", "-t", "ed25519", "-f", &path("id_test")]);
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let _sshd = start_sshd(&dir, port);

    // A repository on the "server" with one commit, and one here whose remote reaches it over ssh.
    let git = ["-c", "user.email=t@t", "-c", "user.name=T", "-c", "commit.gpgsign=false"];
    run("git", &["init", "-q", "-b", "main", &path("seed")]);
    std::fs::write(dir.join("seed/a.txt"), "1\n").unwrap();
    run("git", &[&["-C", &path("seed")], &git[..], &["add", "-A"]].concat());
    run("git", &[&["-C", &path("seed")], &git[..], &["commit", "-qm", "first"]].concat());
    run("git", &["clone", "-q", "--bare", &path("seed"), &path("remote.git")]);
    run("git", &["init", "-q", "-b", "main", &path("local")]);
    run("git", &["-C", &path("local"), "remote", "add", "origin", &format!("ssh://{user}@127.0.0.1:{port}{}", path("remote.git"))]);

    // The dialogs: yes to the unknown host key, and whatever `typed` holds for the passphrase.
    let typed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let seen: Arc<Mutex<Vec<AskpassKind>>> = Arc::new(Mutex::new(Vec::new()));
    let slot: Arc<OnceLock<Arc<Askpass>>> = Arc::new(OnceLock::new());
    let (typed2, seen2, slot2) = (typed.clone(), seen.clone(), slot.clone());
    let ui = Box::new(move |event: UiEvent| {
        let UiEvent::Request(r) = event else { return };
        seen2.lock().unwrap().push(r.kind.clone());
        let answer = match &r.kind {
            AskpassKind::Confirm { .. } => Some("yes".to_string()),
            AskpassKind::Passphrase { .. } => typed2.lock().unwrap().clone(),
            _ => None,
        };
        let server = slot2.get().unwrap().clone();
        tokio::spawn(async move { server.answer(&r.id, answer) });
    });
    let cache = Arc::new(PassphraseCache::default());
    let app = PathBuf::from(env!("CARGO_BIN_EXE_stashpanda"));
    let server = Askpass::start_with_helper(app, cache.clone(), ui).await.unwrap();
    let _ = slot.set(server.clone());

    let transport: SharedTransport = Arc::new(LocalTransport { git: "git".into(), mapper: Box::new(IdentityMapper), windows_git_from_linux: false });
    let ssh = format!(
        "ssh -F /dev/null -i {} -o IdentitiesOnly=yes -o IdentityAgent=none -o PreferredAuthentications=publickey -o UserKnownHostsFile={}",
        path("id_test"),
        path("known_hosts"),
    );
    let fetch = |mode: AskMode| {
        let (server, transport, ssh, local) = (server.clone(), transport.clone(), ssh.clone(), path("local"));
        async move {
            let mut cmd = Cmd::git("fetch", vec!["-C".into(), local, "fetch".into(), "origin".into()]);
            cmd.env = vec![("GIT_TERMINAL_PROMPT".into(), "0".into()), ("GIT_SSH_COMMAND".into(), ssh)];
            cmd.own_session = true;
            cmd.timeout = Some(Duration::from_secs(60));
            let _session = server.attach(&transport, mode, "fetch", &mut cmd);
            let out: Output = transport.run(cmd).await.unwrap();
            (out.success(), String::from_utf8_lossy(&out.stderr).into_owned())
        }
    };

    // A fetch that may not ask: the unknown host key is not accepted, and no dialog opens.
    let (ok, stderr) = fetch(AskMode::StoredOnly).await;
    assert!(!ok && stderr.contains("Host key verification failed"), "{stderr}");
    assert!(seen.lock().unwrap().is_empty());

    // The user cancels the passphrase dialog: ssh has no key to offer.
    let (ok, stderr) = fetch(AskMode::Ask).await;
    assert!(!ok && stderr.contains("Permission denied"), "{stderr}");

    // The user types it: the fetch works, and the dialog named the key file.
    *typed.lock().unwrap() = Some("secret".into());
    let (ok, stderr) = fetch(AskMode::Ask).await;
    assert!(ok, "{stderr}");
    {
        let seen = seen.lock().unwrap();
        assert!(matches!(&seen[0], AskpassKind::Confirm { prompt } if prompt.contains("continue connecting")), "{seen:?}");
        let key = path("id_test");
        assert!(matches!(seen.last().unwrap(), AskpassKind::Passphrase { path, key_name, wrong: false } if *path == key && key_name == "id_test"), "{seen:?}");
    }
    let asked = seen.lock().unwrap().len();

    // Remembered: a fetch that may not ask now works too, and nothing is asked.
    *typed.lock().unwrap() = None;
    cache.remember(&path("id_test"), "secret");
    let (ok, stderr) = fetch(AskMode::StoredOnly).await;
    assert!(ok, "{stderr}");
    assert_eq!(seen.lock().unwrap().len(), asked);

    // A remembered passphrase that no longer fits is not handed to ssh: the user is asked.
    cache.remember(&path("id_test"), "not it");
    *typed.lock().unwrap() = Some("secret".into());
    let (ok, stderr) = fetch(AskMode::Ask).await;
    assert!(ok, "{stderr}");
    assert!(matches!(seen.lock().unwrap().last().unwrap(), AskpassKind::Passphrase { wrong: true, .. }));
    assert!(cache.get(&path("id_test")).is_none());

    let _ = std::fs::remove_dir_all(&dir);
}
