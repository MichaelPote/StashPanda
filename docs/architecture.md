# StashPanda: architecture and implementation plan

## Context

StashPanda is a standalone desktop Git GUI that differs from every other
Git GUI in four ways:

1. **It does not ship Git.** It runs whichever Git executable the operating system already has, with
   the user's existing Git configuration, credential helpers and SSH agent.
2. **WSL Git and remote Git over SSH are first class.** A repository can live on Windows, inside a
   WSL distro, or on another machine reached over SSH, and every operation for that repository is
   run through the right wrapper (`wsl.exe`, `ssh`) transparently.
3. **It is fast.** Cached history paints before Git answers, every Git call is batched or served by
   a long-lived helper process, and the commit graph is virtualised SVG.
4. **Many repositories at once**, one tab per repository, with a global repository tree on the left.

The project lives at `/home/me/code/stashpanda` (WSL path), a fresh Tauri 2 + Vue 3 scaffold with one
uncommitted "greet" demo. Decisions taken with Michael on 2026-09-18:

| Decision | Choice |
|---|---|
| Runtime platform | Both a Windows `.exe` and a Linux binary (run in WSL through WSLg), one codebase, from day one |
| "SSHed Git" | SSH is the transport for any repository that is neither a local folder nor a WSL folder: every operation runs over SSH and file contents come through Git, never the local filesystem. The transport is chosen automatically from the path, and an advanced per-repo setting can override it, for example SSH into a WSL distro's own `sshd` when `wsl.exe` interop is too slow |
| First usable milestone (M1) | Read-only browsing: repos in tabs, branches, commit graph, commit files, working tree files, diffs, console. No write operations |
| SSH implementation, in order of preference | 1. A Rust SSH library with multiplexing and keepalive (`russh`). 2. Long-lived OpenSSH helper processes. 3. Accept the spawn cost and batch commands |
| SSH key management | A tenet of the app: keys must be easy. StashPanda reads PEM, PPK (PuTTY v2 and v3), OpenSSH and PKCS#8 private keys directly, encrypted or not, and converts between them. Michael never opens PuTTYgen again |

## What exists today (do not rebuild)

| Piece | Where | State |
|---|---|---|
| Tauri 2.11 shell, Rust 1.98, edition 2021 | `src-tauri/` (`lib.rs`, `main.rs`, `Cargo.toml`, `tauri.conf.json`) | Compiles for `x86_64-unknown-linux-gnu`; only the `greet` command |
| Vue 3.5, Vite 8, TypeScript 6, Tailwind 4, shadcn-vue 2.8 (reka-ui 2.10), lucide icons, @vueuse/core | `src/` | `App.vue` is the demo card; ui components present: `button`, `card`, `input`, `label` |
| `@` alias to `src/`, `cn()` helper, dark theme tokens | `vite.config.ts`, `tsconfig.json`, `src/lib/utils.ts`, `src/assets/index.css` | Reuse as is. Dark mode is the `.dark` class on `<html>` |
| Tauri capability file | `src-tauri/capabilities/default.json` | Only `core:default` and `opener:default` |

Environment facts (checked 2026-09-18):

- WSL: Ubuntu 24.04, Git 2.43, OpenSSH 9.6, Node 24.16, Rust 1.98 with only the Linux target. No `inotifywait` installed.
- Windows: Git for Windows 2.47.1 at `C:\Program Files\Git\bin\git.exe`, Windows OpenSSH at `C:\Windows\System32\OpenSSH\ssh.exe`, `wsl.exe` present with one distro (`Ubuntu`, WSL 2). Node installed. **No Rust toolchain on Windows.**
- PhpStorm currently has an unrelated project open; it is not used for this work.

## Architecture overview

```
┌──────────────────────────── Vue 3 frontend (WebView) ─────────────────────────────┐
│ Pinia stores keyed by repoId  ←→  typed IPC bindings (tauri-specta)  ←→  events    │
└───────────────────────────────────────┬────────────────────────────────────────────┘
                                        │ Tauri commands + events
┌───────────────────────────────────────┴────────────────────────────────────────────┐
│ Rust core (src-tauri/src)                                                          │
│  app_state ── settings (JSON) ── cache (SQLite) ── repo manager (one RepoSession   │
│  per open repo: transport + command queue + watcher + cat-file helper)             │
│  git/ ── transport/{local,wsl,ssh} ── commands (log, status, diff, refs …)         │
│  ── parsers ── graph layout ── console event emitter                               │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Everything Git-related happens in Rust. The frontend never spawns processes and never touches the
filesystem; it only calls commands and listens to events. That keeps the WebView fast, keeps the
three transports invisible to the UI, and lets the same UI work on both platforms.

## Measured facts that drive the Git layer (2026-09-18, this machine)

| Fact | Consequence |
|---|---|
| Spawn cost from WSL: native `git` about 1 ms, `git.exe` through interop about 46 ms, `wsl.exe -e true` about 87 ms | Anything over `wsl.exe` or SSH must be batched into one spawn or served by a long-lived helper process |
| `wsl.exe -d Ubuntu -e git -C /path ...` passes stdout bytes and the exit code through unmodified | Use the `-e` form (no login shell). `wsl.exe`'s own error messages can be UTF-16LE, so sniff before decoding |
| In-box Windows OpenSSH 9.5 with `ControlMaster=auto` dies at once: `getsockname failed: Not a socket` | Connection multiplexing through the OpenSSH client is unusable on Windows. This is the main reason the SSH transport uses the in-process `russh` library (one connection, many channels) rather than driving `ssh.exe` |
| `russh` 0.63 (pure Rust, tokio), `russh-config` 0.58 (`~/.ssh/config` parser) are current on crates.io. `russh` builds on the RustCrypto `ssh-key` crate and defaults to the `aws-lc-rs` backend | Native multiplexing and keepalive without OpenSSH. Select the `ring` backend feature instead of `aws-lc-rs`, which needs CMake and NASM on Windows |
| Real `.ssh` folders mix PEM, PPK v2 and v3, OpenSSH and PKCS#8 keys, encrypted or not, and a `.pem` file can hold an OpenSSH-format key. No Rust crate reads PPK | Key detection must be by content, not extension. StashPanda ships its own PPK v2 and v3 parser and converts everything to `ssh_key::PrivateKey` in memory, so any key works without a conversion step |
| Both Gits (2.43 WSL, 2.47 Windows) support `status --porcelain=v2 -z`, `for-each-ref` with `%00`, `log -z --format`, `diff-tree --raw -z`, `cat-file --batch-command` | Minimum supported Git is 2.36. Anything newer is behind a capability flag |
| `inotifywait` is not installed in this WSL distro | Remote watching probes for it and falls back to polling, with a visible hint in the UI |

## Part 1: Git interface (Rust, `src-tauri/src/`)

### Module tree

```
lib.rs                      Tauri builder, tauri-specta builder (collect_commands!/collect_events!), TS export in debug builds, AppState, tracing
error.rs                    GitError enum + classify(stderr, exit code, transport kind)
settings.rs                 GlobalSettings, RepoConfig (transport choice, git path, ssh params, poll intervals, console caps); load/save
state.rs                    AppState { repos: RwLock<HashMap<RepoId, Arc<RepoHandle>>>, settings, cache }
repo/mod.rs                 RepoHandle: location, transport, GitClient, CatFileHelper, watcher, coalescer, per-repo lock
repo/queue.rs               per-repo RwLock<()> (reads shared, writes exclusive) + OperationRegistry { op_id -> CancellationToken, remote pid }
repo/open.rs                open flow: resolve location -> pick transport -> probe -> rev-parse -> start helpers and watcher
transport/mod.rs            Transport + Process traits, Cmd, Program, StdinMode, TransportKind, spawn helpers (CREATE_NO_WINDOW, kill_on_drop)
transport/local.rs          direct executable (git, git.exe, /mnt/c/.../git.exe)
transport/wsl.rs            `wsl.exe -d <distro> -e ...`, distro enumeration, UTF-16LE sniff for wsl.exe's own errors
transport/ssh.rs            SshTransport over russh: one channel per command; SshChannelProcess implements Process (real stderr, exit status, signal)
transport/ssh_pool.rs       SshConnectionPool keyed by (host, port, user): connect, authenticate, keepalive, reconnect, channel concurrency bound
transport/ssh_auth.rs       ~/.ssh/config resolution (russh-config), agent client, key file loading, known_hosts check, UI prompts for host key and passphrase
transport/ssh_exec.rs       FALLBACK (M3, only if a real host cannot be served by russh): long-lived `ssh -T host sh` helper with sentinel framing
keys/mod.rs                 KeyManager: scan folders, inspect, load any format to ssh_key::PrivateKey, convert, export, generate
keys/detect.rs              format detection by content: PPK v2/v3, OpenSSH, PEM PKCS#1 RSA / SEC1 EC / DSA (legacy, encrypted or not), PKCS#8, public-only (.pub, RFC 4716)
keys/ppk.rs                 PuTTY PPK parser and writer: v2 (SHA-1 KDF, AES-256-CBC, HMAC-SHA-1) and v3 (Argon2id, AES-256-CBC, HMAC-SHA-256)
keys/pem_legacy.rs          traditional OpenSSL encrypted PEM (`Proc-Type: 4,ENCRYPTED`, `DEK-Info`, EVP_BytesToKey with MD5)
keys/agent.rs               agent client: list identities, add identity (with lifetime), Windows named pipe and Unix socket
keys/secrets.rs             passphrase cache: in-memory for the session, optional OS keychain via `keyring`
ipc/keys.rs                 keys_scan, keys_inspect, keys_convert, keys_export_public, keys_add_to_agent, keys_generate, keys_change_passphrase, keys_import
transport/discover.rs       find executables (settings override -> `which` -> well-known paths); parse `git --version`; Capabilities
transport/paths.rs          HostPath / NativePath newtypes, PathMapper trait; pure Rust, never spawns `wslpath`
git/mod.rs                  GitClient::exec: THE single execution path. Injects common flags, times, runs, records console, classifies errors
git/console.rs              ConsoleRecorder: display quoting, truncation, sequence numbers, emits ConsoleEvent
git/catfile.rs              pinned `cat-file --batch-command --buffer` per repo, serialised requests, size cap, auto-respawn
git/parse.rs                NUL and 0x1F record splitting on bytes, lossy UTF-8 decode, oid/mode parsing
git/versions.rs             GitVersion, Capabilities
git/queries/{refs,log,status,commit_files,diff,blob}.rs   one file per query, each with a parser unit test on captured bytes
git/ops/mod.rs              (M2+) stage/unstage/discard/commit/checkout; streaming fetch/push/pull
watch/{mod,native,remote,polling,coalesce}.rs             Watcher trait; notify-based, inotifywait-over-transport, polling; RefreshCoalescer
ipc/{mod,repos,query,ops,events}.rs                       thin #[tauri::command] wrappers and tauri_specta::Event payload structs
```

### Crates to add to `src-tauri/Cargo.toml`

| Crate | Version | Purpose |
|---|---|---|
| tokio | 1 (`rt-multi-thread, process, io-util, sync, time, macros, fs`) | async process spawning; already in the lockfile through Tauri |
| tokio-util | 0.7 | CancellationToken, line and NUL framing codecs |
| async-trait | 0.1 | `dyn Transport` / `dyn Process` / `dyn Watcher` |
| thiserror | 2 | `GitError` |
| tracing, tracing-subscriber (`env-filter, fmt`) | 0.1 / 0.3 | internal logging, separate from the user-facing console |
| tauri-specta, specta, specta-typescript | `=2.0.0-rc.25`, `=2.0.0-rc.25`, `=0.0.12` (pin exactly; RC bumps break each other) | generated TypeScript bindings for commands, events and `GitError` |
| notify, notify-debouncer-full | 8 (stable; 9 is still RC), matching debouncer | native filesystem watching |
| which | 8 | locate `git`, `wsl.exe`, `ssh.exe`, honouring PATHEXT |
| dunce | 1 | strip `\\?\` from canonicalised Windows paths |
| bstr | 1 | byte-string splitting and lossy decode; all Git parsing is on `&[u8]` |
| shlex | 1.3 | POSIX quoting of the remote argv for SSH (`exec` requests are parsed by the remote login shell) |
| russh | 0.63 | in-process SSH client: one connection, many channels, keepalive, agent and key auth. `russh::keys` replaced the separate `russh-keys` crate; verify at add time |
| russh-config | 0.58 | parse `~/.ssh/config` (Host aliases, User, Port, IdentityFile, ProxyCommand) |
| ssh-key | the exact version `russh` 0.63 depends on (check `cargo tree`), features `ed25519, rsa, ecdsa, dsa, encryption, std` | read and write OpenSSH, PKCS#8 and PEM RSA keys; fingerprints; key generation |
| aes, cbc, sha1, sha2, hmac, argon2, md-5 | current RustCrypto releases | PPK v2 and v3 decryption and MAC, legacy PEM decryption |
| zeroize | 1 | wipe decrypted key material and passphrases from memory |
| keyring | 4 | optional passphrase storage in Windows Credential Manager or Secret Service; session-only fallback when unavailable (likely in WSL) |
| uuid | 1 (`v4`) | RepoId, OpId, and the sentinel nonce for the tier 2 SSH fallback |
| rusqlite | 0.40 (`bundled`) | commit and ref cache (Part 2) |
| tauri-plugin-dialog, tauri-plugin-window-state | 2 (stable line for Tauri 2, not the 3.0 alphas) | folder picker; remember window size and position |

### Core traits

```rust
pub enum TransportKind { Local, Wsl { distro: String }, Ssh { host: String } }
pub enum Category { UserAction, Internal, Watcher }   // console filter preference later hides Internal and Watcher

pub struct Cmd { program: Program, args: Vec<String>, env: Vec<(String,String)>, stdin: StdinMode,
                 category: Category, timeout: Option<Duration>, label: &'static str }

#[async_trait] pub trait Transport: Send + Sync {
    fn kind(&self) -> TransportKind;
    fn paths(&self) -> &dyn PathMapper;
    fn render(&self, cmd: &Cmd) -> Vec<String>;          // full host command line incl. wsl.exe/ssh wrapper, for the console
    async fn spawn(&self, cmd: Cmd) -> Result<Box<dyn Process>, GitError>;
    async fn run(&self, cmd: Cmd) -> Result<Output, GitError>;             // default impl over spawn
    async fn run_batch(&self, cmds: Vec<Cmd>) -> Result<Vec<Output>, GitError>; // one spawn for several commands on wsl/ssh
    async fn probe(&self) -> Result<TransportInfo, GitError>;
    async fn read_file(&self, path: &NativePath, limit: usize) -> Result<Vec<u8>, GitError>;
    async fn shutdown(&self);
}
#[async_trait] pub trait Process: Send {
    fn stdin(&mut self) -> Option<Box<dyn AsyncWrite + Send + Unpin>>;
    fn stdout(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>>;
    fn stderr(&mut self) -> Option<Box<dyn AsyncRead + Send + Unpin>>;   // russh: ExtendedData(1) channel messages; None only for the tier 2 helper (stderr arrives in the trailer)
    async fn wait(&mut self) -> Result<ExitStatus, GitError>;
    async fn kill(&mut self) -> Result<(), GitError>;                    // wsl/ssh: `kill -TERM <remote pid>` via a fresh spawn
    fn remote_pid(&self) -> Option<u32>;
}
pub trait PathMapper: Send + Sync {
    fn to_native(&self, host: &HostPath) -> Result<NativePath, GitError>;   // what `git -C` receives
    fn to_host(&self, native: &NativePath) -> Result<HostPath, GitError>;   // Err for SSH (no host view)
}
#[async_trait] pub trait Watcher: Send {
    fn kind(&self) -> WatcherKind;                       // Native | RemoteInotify | Polling | Off
    async fn start(&mut self, tx: mpsc::Sender<WatchEvent>) -> Result<(), GitError>;
    async fn set_activity(&mut self, a: Activity);       // Focused | Background | Hidden
    async fn stop(&mut self);
}
```

`GitClient::exec` is the one place that turns a request into a process. It always injects:
`git --no-optional-locks -c color.ui=never -c core.quotepath=false -c i18n.logOutputEncoding=utf-8 -C <native repo path> ...`
with env `GIT_TERMINAL_PROMPT=0` and `LC_ALL=C.UTF-8` (rendered as `env K=V git` on wsl/ssh). `--no-optional-locks` stops `git status` rewriting `.git/index`, which would otherwise trip our own watcher.

### Command table (M1 queries)

| Need | Git command (after the common flags) |
|---|---|
| Probe on open | `--version`; then `rev-parse --is-inside-work-tree --show-toplevel --absolute-git-dir --git-common-dir --is-bare-repository` (one call). `dubious ownership` in stderr becomes `UnsafeRepository` |
| Branches, tags, HEAD, upstream | `for-each-ref --format=%(refname)%00%(objectname)%00%(objecttype)%00%(*objectname)%00%(upstream)%00%(upstream:track)%00%(HEAD)%00%(committerdate:unix)%00%(subject)%00%(worktreepath) refs/heads refs/remotes refs/tags` |
| Remotes | `config -z --get-regexp ^remote\..*\.(url\|pushurl\|fetch)$` |
| Commit rows for the graph | `log --branches --tags --remotes HEAD --topo-order -z --max-count=5000 [--skip=K] --format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%s`. Not `--all` (that drags in stash and notes refs). Decorations are joined in Rust from the refs snapshot, so there is one source of truth |
| Stashes | `stash list -z --format=%H%x1f%gd%x1f%gs%x1f%ct` |
| Files changed in a commit | `diff-tree -r -z -M -C --root --no-commit-id --raw --numstat <sha>` (merge: explicit `<parent> <sha>`, UI picks the parent). Verify the combined `--raw --numstat` ordering once on real output |
| Working tree status | `status --porcelain=v2 -z --branch --untracked-files=all --show-stash --ahead-behind` |
| Diff of one file | commit vs parent: `diff --no-ext-diff -U3 -M <parent> <sha> -- <path>`; staged: `diff --cached ... -- <path>`; unstaged: `diff ... -- <path>`; worktree vs HEAD: `diff HEAD ... -- <path>`; untracked: read the file and render as all-added |
| Blob content (side-by-side) | pinned `cat-file --batch-command --buffer`: `info <sha>:<path>` first (refuse over 16 MiB), then `contents <sha>:<path>`, then `flush`. Index side is `:0:<path>`; conflict stages `:1:`, `:2:`, `:3:` |
| Working tree file content | `Transport::read_file` (local: `tokio::fs`; WSL from Windows: `wsl.exe -e cat`; SSH: `cat --`) |

Bytes rules: never `from_utf8().unwrap()` on Git output; binary means a NUL in the first 8000 bytes, checked on raw bytes; CRLF is never rewritten on the wire, the blob result carries `eol: Lf | CrLf | Mixed | None` and the renderer strips `\r` for display; `--ignore-cr-at-eol` and `-w` are user toggles.

### Path translation (`transport/paths.rs`, table-driven tests)

| The app sees | Transport | `git -C` receives |
|---|---|---|
| `C:\foo\bar` | Local (Windows git) | `C:\foo\bar` |
| `C:\foo\bar` | WSL {Ubuntu} | `/mnt/c/foo/bar` (automount root configurable) |
| `\\wsl.localhost\Ubuntu\home\me\code\x` or `\\wsl$\Ubuntu\...` | WSL {Ubuntu} (distro parsed from the UNC must match) | `/home/me/code/x` |
| `/home/me/code/x` (app on Linux) | Local (Linux git) | `/home/me/code/x` |
| `/mnt/c/foo/bar` (app on Linux) | Local using `/mnt/c/Program Files/Git/bin/git.exe` | `C:\foo\bar` |
| `user@host:/srv/repo` | SSH | `/srv/repo` (`~` rendered as `"$HOME"` because every argument is shlex-quoted) |

Git's own output paths are repo-relative with `/` and are never translated. An ignored integration test cross-checks the mapper against `wslpath -w` and `wslpath -u` when `wslpath` exists.

### Choosing the transport for a repository

`RepoConfig.connection` is `Auto` by default. `Auto` resolves from the path the user typed or picked, at add time and again on every open (so a moved distro or a changed default is picked up):

| Path as entered | App running on | Resolved transport |
|---|---|---|
| `C:\...` or any drive letter | Windows | Local, Windows `git.exe` |
| `\\wsl.localhost\<distro>\...` or `\\wsl$\<distro>\...` | Windows | WSL {distro}, via `wsl.exe` |
| `/mnt/<d>/...` | Linux (WSL) | Local, Windows `git.exe` found under `/mnt/<d>/Program Files/Git/bin/`, because the files are NTFS and Windows Git handles them correctly; falls back to Linux `git` with a warning if no Windows Git is found |
| any other `/...` | Linux (WSL) | Local, Linux `git` |
| `user@host:/path`, `ssh://user@host[:port]/path`, or a `Host` alias from `~/.ssh/config` followed by `:/path` | either | SSH (tier 1, `russh`) |

The "Add repository" dialog shows the resolved transport next to the path and an **Advanced** disclosure with a `Connection` select: `Auto`, `Local`, `WSL (distro…)`, `SSH (host, user, port, identity…)`. Overrides that matter in practice:

- **WSL folder over SSH instead of `wsl.exe`.** Every `wsl.exe -e` spawn costs about 87 ms on this machine, so a working tree with hundreds of status refreshes a day pays for it. If `sshd` is running inside the distro (`sudo apt install openssh-server`, listening on localhost, port 22 or a custom one), choosing `SSH` with host `localhost` gives one persistent connection and a few milliseconds per command, while the path stays `\\wsl.localhost\Ubuntu\home\me\code\x` in the library and is mapped to `/home/me/code/x` on the wire. The probe reports both timings (`wsl.exe -e true` and an SSH `true` channel) in the console so the choice is informed rather than guessed.
- **Windows folder from the Linux build with Linux Git.** Allowed when there is no Windows Git; the UI warns that line endings and file modes may be reported differently.
- **Local override for a UNC WSL path** is allowed but warned against: Windows Git over `\\wsl.localhost` is slow and almost always hits `dubious ownership`.

Whatever the override, the `RepoEntry.key` is computed from the canonical native path plus the resolved transport, so the same folder reached two ways is two library entries with two caches, and the console always shows which wrapper is in use.

### Transports

- **Local**: `tokio::process::Command`, `kill_on_drop(true)`, on Windows `creation_flags(CREATE_NO_WINDOW)` and never inherit stdio. Discovery: settings override, then `which`, then well-known paths (`%ProgramFiles%\Git\bin\git.exe`, `/usr/bin/git`, `/mnt/<d>/Program Files/Git/bin/git.exe`).
- **WSL**: `wsl.exe -d <distro> -e env GIT_TERMINAL_PROMPT=0 LC_ALL=C.UTF-8 git ... -C /linux/path ...`. Distro list from `wsl.exe -l -q` (UTF-16LE). Cancellable commands are wrapped as `setsid -w sh -c 'echo $$; exec "$@"' sh <argv>` and the first stdout line is the pid of the new session's process group, which terminate signals through a second `wsl.exe`. (As implemented 2026-10-01: killing `wsl.exe` does end the Linux processes it started directly, contrary to the original plan, but abruptly, and not ones in a session of their own.) Startup self-test passes an argument with a space and a quote, and falls back to `-e sh -c '<quoted>'` if `wsl.exe` re-splits it.
- **SSH, tier 1 (M1, the default): in-process `russh` client.** No `ssh.exe` involved, so Windows and Linux behave identically and the Windows ControlMaster problem disappears.
  - **Connection pool** (`ssh_pool.rs`): one authenticated `russh::client::Handle` per `(host, port, user)`, shared by every repo on that host. `Config { keepalive_interval: 15 s, keepalive_max: 3, inactivity_timeout: None }`. A dropped connection emits `transport_state: Disconnected` and reconnects with backoff (1, 2, 5, 10, 30, then 60 s between attempts, failing at once in between), then re-opens the pinned channels. Channel concurrency per connection is bounded by a `Semaphore(8)` because OpenSSH servers default `MaxSessions` to 10; a second connection is opened when the bound is saturated.
  - **One command = one channel** (`ssh.rs`): `channel_open_session()` then `exec(true, "env GIT_TERMINAL_PROMPT=0 LC_ALL=C.UTF-8 git -C /srv/repo ...")` with the remote argv shlex-quoted, because `exec` goes through the remote login shell. `SshChannelProcess` implements the `Process` trait: stdin is `channel.data()` plus `eof()`, stdout is the `Data` messages, stderr is `ExtendedData(1)`, `wait()` resolves on `ExitStatus` (or `ExitSignal`), `kill()` sends `signal(Sig::TERM)` and falls back to `kill -TERM <pid>` on another channel using the echoed-pid wrapper. No sentinel framing, no lost stderr separation, no CRLF translation (no pty is ever requested).
  - **Pinned helpers are long-lived channels**: `git cat-file --batch-command --buffer` and `inotifywait -m` each hold one channel for the life of the repo session. `run_batch` becomes `sh -c` of several commands on one channel with `printf '\0'` separators only when the round trip matters; with channels this cheap it is an optimisation, not a necessity.
  - **Authentication** (`ssh_auth.rs`), tried in this order and shown step by step in the console as Internal entries: (1) `~/.ssh/config` is resolved with `russh-config` for the alias, so `Host`, `HostName`, `User`, `Port`, `IdentityFile` and `IdentitiesOnly` are honoured; the per-repo settings override any of them. (2) SSH agent: `SSH_AUTH_SOCK` on Linux, the `\\.\pipe\openssh-ssh-agent` named pipe on Windows (verify that `russh::keys::agent` supports the Windows named pipe; if not, `keys/agent.rs` implements the small agent protocol over `tokio::net::windows::named_pipe`). (3) Identity files in **any format the key manager reads** (PPK, PEM, OpenSSH, PKCS#8): the file is loaded through `KeyManager::load`, decrypted with a passphrase dialog if needed, and handed to `russh` as an `ssh_key::PrivateKey` in memory. No file is ever converted just to log in. (4) Password, via a UI dialog, only if the server offers it.
  - **Host keys**: checked against `~/.ssh/known_hosts` (platform home directory) with `russh::keys::known_hosts`. Unknown host raises a dialog with the SHA-256 fingerprint and Accept / Reject, and Accept appends to `known_hosts` (the OpenSSH `StrictHostKeyChecking=ask` behaviour). A changed key is a hard `SshHostKeyChanged` error with no override in the UI.
  - **Not supported by tier 1, routed to tier 2**: `ProxyCommand`, `ProxyJump`, GSSAPI, certificates, PKCS#11 tokens, and any host whose key-exchange or cipher list `russh` cannot negotiate. `ProxyJump` may later be implemented natively by nesting a connection over a `direct-tcpip` channel.
- **SSH, tier 2 (M3, fallback): long-lived OpenSSH helper processes** (`ssh_exec.rs`). Built only if a real host fails tier 1. Same `Transport` interface. Per repo a small pool of `ssh -T -o BatchMode=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o LogLevel=ERROR ... -- sh` sessions; each command is one line whose stdout streams to a nonce sentinel and whose trailer carries stderr text and `@@RC=<n>`; one command in flight per session; a protocol desync drops the session and reconnects. Pinned helpers run `git cat-file --batch-command` and `inotifywait -m` directly on their own `ssh` processes. This is the design that works with the in-box Windows OpenSSH, which has no ControlMaster.
- **SSH, tier 3 (implicit): one-shot `ssh` per batch.** Falls out of tier 2's `run_batch` when a session cannot be kept alive. Never the primary path.
- PuTTY's `plink` is not used in any tier: no multiplexing, different options, interactive host key prompts.

### SSH key management (`keys/`, a tenet of the app)

Goal: any private key file Michael has can be used as-is, inspected, converted and exported from inside StashPanda. No PuTTYgen, no `ssh-keygen -p` on the command line.

**Formats read** (detected by content, never by extension, because several `.pem` files here are OpenSSH format inside):

| Header | Format | How it is loaded |
|---|---|---|
| `PuTTY-User-Key-File-2: <alg>` | PPK v2 | own parser: public and private blobs base64, key from SHA-1 KDF, AES-256-CBC, HMAC-SHA-1 over the MAC input; `Encryption: none` or `aes256-cbc` |
| `PuTTY-User-Key-File-3: <alg>` | PPK v3 | own parser: Argon2id (or Argon2i/d as declared) with the declared memory, passes, parallelism and salt; AES-256-CBC; HMAC-SHA-256 |
| `-----BEGIN OPENSSH PRIVATE KEY-----` | OpenSSH new format, optionally bcrypt-pbkdf encrypted | `ssh_key::PrivateKey::from_openssh` and `decrypt` |
| `-----BEGIN RSA PRIVATE KEY-----`, `-----BEGIN EC PRIVATE KEY-----`, `-----BEGIN DSA PRIVATE KEY-----` | traditional PEM (PKCS#1, SEC1, DSA); `Proc-Type: 4,ENCRYPTED` means OpenSSL legacy encryption | `pem_legacy.rs` decrypts (DEK-Info cipher, EVP_BytesToKey with MD5), then `rsa`, `sec1` or `ssh-key` DSA parsing into `ssh_key::PrivateKey` |
| `-----BEGIN PRIVATE KEY-----`, `-----BEGIN ENCRYPTED PRIVATE KEY-----` | PKCS#8 | `ssh_key::PrivateKey::from_pkcs8_pem`, PBES2 decryption |
| `ssh-rsa AAAA...`, `---- BEGIN SSH2 PUBLIC KEY ----`, `.pub` | public only | listed, fingerprinted, matched to a private key by public-key equality |

Algorithms: RSA, ed25519, ECDSA P-256/384/521, DSA (read and convert only; shown as deprecated because OpenSSH 9 servers reject it).

**Formats written**: OpenSSH new format (default, optionally encrypted with a new passphrase, `.pub` alongside), PEM PKCS#8, PPK v3 (for the rare tool that still wants PuTTY), public key as an `authorized_keys` line or RFC 4716.

**UI**: a **Settings › SSH Keys** screen and the same list inside the Add Repository dialog's identity picker.
- Scans `~/.ssh` in the platform home plus, when both sides are visible, the other side's home (`/mnt/c/Users/<name>/.ssh` from Linux, `\\wsl.localhost\<distro>\home\<user>\.ssh` from Windows) plus any folders the user adds. Shows each key's name, algorithm and bits, SHA-256 fingerprint, comment, format badge (PPK 2, PPK 3, PEM, OpenSSH, PKCS#8), encrypted badge, whether a matching `.pub` exists, and whether it is loaded in the agent.
- Actions per key: **Use for repo**, **Convert to OpenSSH** (writes next to the original, never overwrites, original untouched), **Export** (choose format and passphrase), **Copy public key**, **Add to agent** (optionally for this session only via a lifetime constraint), **Change passphrase**, **Reveal in folder**, **Delete** (confirm, private key only).
- **Import**: drag a file or paste key text into the window; pasted text is detected the same way.
- **Generate**: ed25519 by default, RSA 4096 as an option, comment defaults to `michael@<machine>`, saved in OpenSSH format with optional passphrase.
- **Passphrases**: prompted on first use with "remember for this session" (in memory, zeroized on exit) and "store in system keychain" when `keyring` reports a backend. Wrong passphrase is reported as such, not as a generic auth failure.
- Errors are specific: `SshKeyUnsupported` is now reserved for genuinely unknown formats (for example PGP or PKCS#12 files), with the detected header quoted.

**Safety rules**: decrypted material lives in `zeroize`-wrapped buffers; nothing is written to disk without an explicit Convert or Export click; converted files get mode 600 on Unix; the console logs key operations as Internal entries with fingerprints only, never key bytes.

**Tests**: fixtures for every format and algorithm, encrypted and not, generated during implementation with `ssh-keygen`, `openssl` and PuTTY's own `puttygen.exe` run from WSL if PuTTY is installed on Windows (so the fixtures come from an independent implementation, not from our own writer); throwaway keys only, never real ones. An ignored local test additionally reads every real key in the Windows and WSL `.ssh` folders and checks that each one parses and fingerprints, skipping encrypted keys unless a passphrase is supplied through an environment variable; a round-trip test converts each fixture to OpenSSH and back and compares public keys and signatures; a smoke test authenticates to the local `sshd` with a key loaded from a PPK file.

### Watching and refresh

- `choose_watcher`: Local repos use `notify` 8 with a 250 ms debouncer, watching the worktree recursively, `.git` non-recursively (HEAD, index, packed-refs, ORIG_HEAD, MERGE_HEAD, rebase dirs) and `.git/refs` recursively, plus the common dir for linked worktrees. Never `.git/objects`; drop `*.lock`. Overflow or `ENOSPC` downgrades to polling and emits `WatcherStateEvent`.
- WSL-from-Windows and SSH repos use `inotifywait -m -r -q -e modify,close_write,create,delete,move,attrib --exclude '(/\.git/(objects|lfs|logs)/|\.lock$)' --format '%w%f|%e' <worktree> <gitdir>` streamed through the transport if `command -v inotifywait` succeeds; otherwise polling.
- Polling: every 3 s when focused, 15 s in the background, paused when hidden. Runs status and for-each-ref as `Category::Watcher` (one spawn via `run_batch` on wsl/ssh), hashes the outputs, emits only on change.
- `RefreshCoalescer` per repo: worktree or index events arm a 300 ms trailing debounce (2 s maximum hold under bursts like `npm install`) and set `status = true`; HEAD, refs, packed-refs and merge-state events set `refs = true`. One `RepoChangedEvent { status, refs }` per burst; the frontend then calls `get_status`, or `get_refs` and `get_log`.

### Events (all carry `repo_id`)

| Event | Payload |
|---|---|
| `console_event` | seq, phase (Started or Finished), category, label, argv as executed (wrapper included), display string, started_at, duration, exit code, stdout and stderr (lossy, truncated at 64 KiB with a flag), classified error |
| `repo_changed` | status: bool, refs: bool, cause: WatcherKind |
| `operation_progress` / `operation_finished` (M2+) | op_id, streamed line, stream; exit code, error |
| `watcher_state` | kind, detail (for example "inotifywait not found; polling every 3 s") |
| `transport_state` | Connecting, Ready, Degraded, Disconnected, detail |

Pinned helpers log their spawn and exit as Internal; their per-request traffic is logged only when the `console.verbose_helpers` preference is on.

### Concurrency

Tauri commands are `async fn` on Tauri's tokio runtime. Each `RepoHandle` is an `Arc`; different repos share no lock. Per repo, a write-preferring `tokio::sync::RwLock<()>`: reads (log, status, diff, blob, refs, polls) take the shared lock, short writes (stage, commit, checkout) take the exclusive lock, and a `Semaphore(4)` bounds concurrent read spawns. Every request carries a `CancellationToken` that is a child of the repo token; `close_repo` cancels it, kills children and remote pids, stops the watcher. Every write also holds a `write_gate` mutex for its whole run, so writes never overlap; network writes (pull, and later fetch and push) hold only the gate, so reads carry on during the transfer (as implemented 2026-10-01). `index.lock` present is classified `IndexLocked`, retried once after 250 ms for writes, then surfaced with a "remove stale lock" action.

### Error enum (`error.rs`, serialised as a tagged union so TypeScript can switch on `kind`)

`GitNotFound { searched }`, `GitVersionTooOld { found, required }`, `WslNotAvailable`, `WslDistroNotFound { distro, available }`, `SshConnectFailed { host, detail }`, `SshAuthFailed { host, methods_tried }`, `SshHostKeyUnknown { host, fingerprint }`, `SshHostKeyChanged { host, fingerprint }`, `SshKeyUnsupported { path, detected_header }` (only for formats the key manager does not read), `SshNeedsPassphrase { path }`, `SshWrongPassphrase { path }`, `KeyParseError { path, format, detail }`, `SshUnsupportedFeature { feature }` (routes to tier 2), `TransportDisconnected`, `NotARepository`, `RepoPathNotFound`, `UnsafeRepository`, `IndexLocked { lock_path }`, `InvalidPath`, `ObjectNotFound`, `TooLarge { bytes, limit }`, `CommandFailed { label, argv, exit_code, stderr }`, `ParseError`, `Timeout`, `Cancelled`, `Io`. `classify()` maps by stderr substring (`not a git repository`, `dubious ownership`, `index.lock': File exists`, `Permission denied (publickey`, `Host key verification failed`, wsl exit 255 with `no distribution`, ssh exit 255 otherwise).

### Type-safe IPC

Use tauri-specta. Export under `#[cfg(debug_assertions)]` to `src/lib/bindings.ts` with `bigint(BigIntExportBehavior::Number)`, and commit the generated file. The frontend calls `commands.getStatus(repoId)` and `events.repoChanged.listen(...)`. Run `vue-tsc --noEmit` once after the first export to confirm TypeScript 6 accepts the output; if not, fall back to a hand-written `types.ts` and keep the `ipc/*.rs` boundary unchanged.

## Part 2: Commit graph and repository cache

### Ordering and layout run in Rust; layout is never persisted

- `git log` is streamed in its default order (no `--topo-order`, no `--skip`: both make Git walk the whole history before the first line, so paging would re-walk 100k commits per page). Rust parses the stream into a `CommitStore` (oid, parents, interned author, times, subject).
- Rust computes the display order itself with the same algorithm Git uses for `--date-order`: Kahn's topological sort with a max-heap on committer date, deterministic tie-break on oid. About 10 to 20 ms for 100k commits. A "topo (grouped)" order is a different tie-break behind a setting, not a different Git call.
- Consequence: the cached and the live load produce identical orderings, so the graph never reshuffles after the first paint. Incremental refresh is "add commits to the store, re-sort, re-sweep".
- Lane layout is a serial sweep whose state at row N depends on every row above, so one new commit at the top can renumber every lane below. Recomputing 100k rows costs about 5 to 15 ms, so layout is recomputed on every snapshot and never written to disk.
- Whole history is loaded in one streamed process, capped by `cache.maxCommitsPerRepo` (default 200 000). Past the cap the bottom rows show dangling stubs and a "Load N more" action raises the cap.

### Core data (Rust, `src-tauri/src/graph/`)

```
oid.rs        Oid { len: u8, bytes: [u8; 32] } (SHA-1 and SHA-256), FxHashMap everywhere
store.rs      CommitRecord { oid, parents: SmallVec<[Oid;2]>, author: u32, author_time, commit_time, subject }
              CommitStore { commits, by_oid, authors } with insert() and retain_reachable_from(tips)
refs.rs       RefEntry { full_name, short, target (peeled), kind: LocalBranch | RemoteBranch{remote} | Tag{annotated} | Stash, upstream }
              RefSnapshot { refs, head, head_branch (None = detached), by_target }
order.rs      order(store, DateOrder | TopoOrder) -> Vec<u32>
lanes.rs      LaneSweep: push(oid, parents) -> RowLayout; finish() -> open lanes
snapshot.rs   GraphSnapshot { version, order, layout, edges (flat arena), row_by_oid, store, refs, worktree, complete, truncated, max_lanes, open_lanes }
commands.rs   graph_open, graph_rows, graph_find, graph_search, graph_refresh
```

Lane sweep rules:
- Edges span from the centre of row i to the centre of row i+1 and are stored on row i; the sweep finalises row i when row i+1 is pushed (one-row lookahead) so a converging lane can curve into the next dot.
- Lane slots are never compacted; a freed slot is reused. Two lanes may wait for the same oid and converge at its row.
- First parent continues in the commit's own lane. Each further parent either merges into a lane already waiting for it (`MergeInto` edge) or forks a new lane (`Parent` edge). Octopus merges fall out of that loop with no special case.
- Lane 0 is reserved for HEAD's first-parent line until HEAD's commit is placed, which gives the "HEAD on the left" look. Smarter priority is a later refinement.
- A synthetic **Working Tree** row is pushed first, with HEAD as its parent, when status is dirty; it lands in lane 0 directly above HEAD with a straight edge.
- Lanes still waiting at the end become `Dangling` edges drawn as a short fading stub.
- Edge kinds: `Pass`, `Parent`, `MergeInto`, `Dangling`. Colour is a palette index 0 to 11; an edge wears the colour of the slot at its `to_lane`.
- Branch tips stacked on one lane (a branch started from the tip of another) are told apart by the `appearance.commitLaneColours` setting. The top-most local or remote branch tip on a lane shares the lane's colour; each tip below it gets a new one, picked so that its hue (`index % 6`, since the dark theme repeats lanes 0 to 5 as 6 to 11) differs from the lane's and, while a hue is free, from every open lane's. With `keepLane` (the default) only that row's pills and Branches panel tags take the new colour (`RowLayout::ref_colour`) and the lane is untouched. With `changeAtEachTip` the lane itself switches to the new colour from that tip's dot down. Tags do not count as tips, and neither does a remote branch that a shown local branch tracks: its pill and tag take the local branch's colour (`RefLabel::colour`), in both modes.

IPC payload per row (`GraphRow`, camelCase via serde, generated into TypeScript by specta): index, kind (`commit` | `workingTree`), oid, parents, lane, colour, refColour (the colour of the row's pills), maxLane, edges, author name and email, author and commit time, subject, `refs: RefLabel[]` (full name, short, kind, isCurrent, colour, upstreamAtSameCommit), isHead, optional worktree summary (staged, unstaged, untracked, conflicted counts).

### Loading and reconciliation

```
open tab  -> graph_open(repoId): first 500 rows from SQLite (ORDER BY seq DESC LIMIT 500), swept, returned in < 50 ms
          -> background: rest of cache -> order -> sweep -> snapshot v1 (reason: cache)
          -> background: refresh()   -> snapshot v2 (reason: live)
graph_snapshot event -> frontend remembers the oid of the first visible row and the selection,
          calls graph_find(version, oids) to map them to new indices, resets scrollTop so the same row stays put,
          clears its page cache and refetches the visible pages with the new version
graph_rows(repoId, version, start, count) -> page of 200 rows; Err(StaleVersion) for an old version
```
For a repo with no cache, partial snapshots (`complete: false`) are swapped in every 500 ms so the graph visibly grows behind a progress bar. With a cache, only the final snapshot is swapped, so nothing flickers.

### Rendering (Vue, `src/components/graph/`)

- **Virtualisation is hand-rolled**: fixed 22 px rows, so `start = floor(scrollTop / H) - 10`, `end = ceil((scrollTop + viewportH) / H) + 10`, a spacer of `total * H`, and one block translated by `start * H`. Composable `useVirtualRows` on top of `useScroll` and `useResizeObserver` from @vueuse/core, updated in requestAnimationFrame. No virtual-list library: they exist for dynamic row heights and they own the translated container that the SVG overlay needs to share.
- **One SVG for the visible window**, absolutely positioned inside the same translated block as the row divs, so it scrolls on the compositor with them and the dot for row i is at exactly the same `y` as its text. Edges cross row boundaries, so one SVG draws each curve once. Vertical pass-through runs of the same lane and colour collapse into one `<path d="M x y V y2 ...">`, giving at most about 12 run paths, 12 curve paths and N circles per window. The SVG has `pointer-events: none`; the row `<div>` beneath handles click, double click, context menu and hover.
- Curves are cubic Beziers between row centres; straight lines otherwise; stroke 2 px, round caps. Lane width 14 px; graph column width defaults to `clamp(maxLanes + 1, 3, 12) * 14px` and is user-resizable; lanes beyond the column are clipped.
- Palette: 12 CSS variables `--graph-lane-0` to `--graph-lane-11` (starting values `#5b9cff #45d483 #ff9a4d #b48cff #ff7eb6 #f5d34a #3fd3e8 #ff6b6b #a6e34d #2fc4a8 #7f8cff #e0a94a`), checked for contrast against the real dark and light backgrounds during M1.
- Row grid shared by header and rows: `var(--col-graph) minmax(0,1fr) var(--col-author) var(--col-date)`. Ref pills before the subject: local branch (bold and filled when current), remote as `origin`, a Lucide ChevronRight icon, then `dev`, tags with the lucide Tag icon, detached HEAD pill. A local branch and its upstream on the same commit collapse into one pill `origin | dev`, with a chevron seam after the remote so the two halves look interlocked; when they are on different commits the local pill has a chevron notch on its left and the remote pill a matching point on its right. More than three pills show three plus a `+N` popover. Author cell shows initials coloured by a hash of the email; Gravatar is an opt-in later. Date is relative or absolute by setting.
- Selection is by oid (stable across versions), with Ctrl and Shift multi-select; keyboard Up, Down, PageUp, PageDown, Home, End, Enter, Ctrl+C copies the oid; `ensureVisible` scrolls minimally.

Components: `CommitGraphView.vue` (owner: repo binding, selection, keyboard, context menu, column widths), `GraphHeader.vue` with `ColumnResizer.vue`, `VirtualRowList.vue`, `GraphLanesSvg.vue`, `CommitRow.vue` (v-memo on row, selected, widths), `WorkingTreeRow.vue`, `RowSkeleton.vue`, `RefLabels.vue`, `RefPill.vue`, `AuthorCell.vue`, `DateCell.vue`. Composables: `useVirtualRows.ts`, `useGraphPages.ts`, `useRowSelection.ts`. Pure helpers: `src/lib/graph/paths.ts`, `palette.ts`.

### Persistent storage: a JSON file for settings, SQLite for the cache and the UI state

| File | Location | Holds |
|---|---|---|
| `settings.json` | `app_config_dir()` | preferences and the repository library: things a person might hand-edit or copy to another machine |
| `stashpanda.sqlite` | `app_data_dir()` | commit and ref cache, and the remembered UI state. `rusqlite` with `bundled`, WAL mode, `synchronous=NORMAL`, one connection behind a Mutex used through `spawn_blocking` |

The remembered UI state (`ui_state::UiState`) is what the window returns to, as opposed to settings:
splitter sizes, graph column widths, open tabs and the active tab, the Console's User, Internal and
Watcher buttons, the Files panel's grouping and sorting, the diff header's layout, context and
whitespace choices and its side-by-side divider, the size of each dialog dragged bigger
(`dialogSizes`), and for each repository the refs hidden from the
graph and the last selected commit. It is stored one row per value (schema version 2,
`storage/schema_v2.sql`): `ui_prefs(key, value)` for the app, with dotted keys such as
`panels.leftWidth`, and `repo_ui_prefs(repo_key, key, value)` for each repository, keyed by
`RepoEntry.key`. Values are JSON. `repo_ui_prefs` does not reference the `repos` cache table, whose
rows are evicted. Window geometry stays with `tauri-plugin-window-state`. Until 2026-09-30 the UI
state was `ui-state.json` in `app_config_dir()`, plus `localStorage` for the Console, Files panel
and diff header; on first start `AppState::init` imports the file and renames it to
`ui-state.json.imported`, and the frontend's `uiState.load()` moves the `localStorage` values across
and deletes them. Removing a repository from the library deletes its rows.

One persist task writes both: `settings.json` atomically (write `*.tmp`, rename) debounced 500 ms, the UI state rows in one transaction debounced 1 s, both flushed synchronously on `RunEvent::ExitRequested`. Rust is the single source of truth: `settings_update(patch)` applies an RFC 7396 merge patch, validates by deserialising, bumps a revision and emits `settings_changed` with the full object; the frontend store replaces its copy from the event. A corrupt file is renamed to `*.broken-<timestamp>` and defaults are used.

Schema (`storage/schema.sql`, versioned with `PRAGMA user_version`):

```sql
CREATE TABLE repos   (id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, oid_len INTEGER NOT NULL DEFAULT 20,
                      commit_count INTEGER NOT NULL DEFAULT 0, last_opened_at INTEGER NOT NULL,
                      last_full_load_at INTEGER, last_refresh_at INTEGER, truncated INTEGER NOT NULL DEFAULT 0,
                      schema_version INTEGER NOT NULL);
CREATE TABLE commits (repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
                      oid BLOB NOT NULL, parents BLOB NOT NULL,          -- raw oids concatenated, n * oid_len bytes
                      author_name TEXT NOT NULL, author_email TEXT NOT NULL,
                      author_time INTEGER NOT NULL, commit_time INTEGER NOT NULL, subject TEXT NOT NULL,
                      seq INTEGER,                                        -- display position counted from the bottom; NULL = not laid out
                      PRIMARY KEY (repo_id, oid)) WITHOUT ROWID;
CREATE INDEX commits_repo_seq ON commits(repo_id, seq DESC);              -- serves the first 500-row paint
CREATE TABLE refs    (repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
                      name TEXT NOT NULL, target BLOB NOT NULL, tag_oid BLOB, kind INTEGER NOT NULL,
                      upstream TEXT, symbolic_target TEXT, PRIMARY KEY (repo_id, name)) WITHOUT ROWID;
CREATE INDEX refs_repo_target ON refs(repo_id, target);
```
Parents are a raw byte blob rather than JSON or an edges table: half the bytes of hex, no parsing beyond chunking, and no join on the hot load path. The `refs` snapshot is the "cached tips" set and is replaced in the same transaction as the commits reachable from it. `seq` counts from the root so a normal "new commits on top" refresh changes zero existing rows.

`RepoEntry.key` is `sha1(transport | transport key | canonical native path)` and joins `settings.json` to `repos.key`.

### Refresh, invalidation, bounds

```
refresh(repo, force_full):
  new_refs = for-each-ref + HEAD
  if refs and HEAD unchanged and not forced: status-only; rebuild the snapshot with or without the Working Tree row
  if forced, no cache, or cache schema_version mismatch: full streamed `git log --branches --tags --remotes HEAD`
  else incremental:
      positives = new or moved refs (+ HEAD if moved)
      negatives = unchanged refs + old oids of moved refs that still exist (checked with the pinned cat-file `info`)
      `git log --stdin -z --format=...` fed positives, `--not`, negatives   (no argv length limit)
      store.insert each; store.retain_reachable_from(new tips) prunes rebased-away commits
      more than 20 000 new commits -> fall back to full
  order -> sweep -> snapshot vN+1 -> swap (ArcSwap) -> emit graph_snapshot
  persist in one transaction: INSERT OR IGNORE commits, DELETE pruned, replace refs, UPDATE seq where changed, UPDATE repos
```
Triggers: tab opened, window regains focus, `repo_changed` with `refs = true` from Part 1, after any Git operation the app itself runs. Any failure of the incremental path (for example `bad revision` after a gc) falls back to a full load.

Bounds: `cache.maxCommitsPerRepo` (200 000) enforced with `--max-count` and by truncating the sorted order from the bottom; `cache.maxCachedRepos` (20) evicts by `last_opened_at`, plus repos no longer in the library, at startup; `PRAGMA incremental_vacuum` after large deletes.

### Settings model (Rust, mirrored in TypeScript by specta)

`Settings { version, git: { executable per platform, wsl_default_distro }, appearance: { theme, date_format, graph_order, show_remote_branches, show_stashes_in_graph, avatar_source }, console: { verbose, verbose_helpers, hide_internal (later preference), max_lines }, polling: { status_interval_ms 3000, refs_interval_ms 10000, auto_fetch_minutes }, cache: { max_commits_per_repo, max_cached_repos }, library: { roots: LibraryNode[] } }` where `LibraryNode` is `Group { id, name, children, collapsed }` or `Repo(RepoEntry { key, path as entered, connection: Auto | Local | Wsl { distro } | Ssh { host, user, port, identity_file, remote_git }, resolved_transport (cached result of Auto), display_name, git_executable override, last_opened_at })`.

`UiState { window, panels: { left_width, repos_height, bottom_height, files_width, console_height }, graph_columns: { graph, author, date }, session: { open_tabs, active_tab }, per_repo: { last_selected_oid } }`.

### Frontend state: Pinia, one session per open repo

Add `pinia` (4.x). Stores in `src/stores/`: `settings.ts` and `uiState.ts` (mirrors of the Rust structs, updated from events), `library.ts` (the Repositories tree, recents, add and group operations), `sessions.ts` holding `shallowReactive(new Map<RepoId, RepoSession>())` plus `activeId`. One session object per tab, created by a factory, holding `info`, `status`, `refs`, the graph page cache (`markRaw`, with a `pagesVersion` counter so 100k rows are never deeply reactive), the selection, the selected file and diff, the console ring buffer (capped at `console.max_lines`), and the saved scrollTop. `sessions.ts` installs each global event listener once and dispatches by `repoId`, so background tabs keep receiving status, console and snapshot updates with no component mounted. Not one Pinia store per repo: dynamic stores leak unless disposed and are awkward in devtools.

## Part 3: Application shell

```
┌ Toolbar ──────────────────────────────────────────────────────────────────────────────────────┐
│ Open repo · Refresh · [Pull · Sync · Push · Commit] · [Stage · Unstage · Discard] · [Stash…]   │  (bracketed groups render disabled in M1 with a "coming in M2" tooltip)
├───────────────┬─ Tab strip: one tab per open repo ───────────────────────────────────────────┤
│ Repositories  │ ┌ CommitGraphView ─────────────────────┬ FilesPanel ─────────────────────────┐ │
│ (global tree, │ │ graph | refs + subject | author | date│ commit files OR working tree changes│ │
│  groups,      │ │                                       │ (staged / unstaged / untracked)      │ │
│  recents)     │ ├───────────────────────────────────────┴──────────────────────────────────────┤ │
├───────────────┤ │ DiffViewer (spans both columns): unified or side by side, hunk navigation     │ │
│ Branches      │ └──────────────────────────────────────────────────────────────────────────────┘ │
│ (repo:        ├─ ConsolePanel (repo-specific): every git command as executed, exit code, timing ─┤
│  local,       │   expandable stdout / stderr; filter chips for UserAction / Internal / Watcher   │
│  remotes,     │                                                                                  │
│  tags,        │                                                                                  │
│  stashes)     │                                                                                  │
└───────────────┴──────────────────────────────────────────────────────────────────────────────────┘
```

- `SettingsDialog.vue` with pages General, Git executables, Appearance, Console, **SSH Keys** (`KeyManagerPage.vue`, `KeyRow.vue`, `KeyImportDropzone.vue`, `PassphraseDialog.vue`, `HostKeyDialog.vue`), Cache.
- `src/App.vue` becomes `AppShell.vue`: toolbar, left sidebar with a vertical splitter between `RepositoriesPanel.vue` (global) and `BranchesPanel.vue` (active repo), main area with `RepoTabStrip.vue` and one `RepoWorkspace.vue` per open tab kept alive with `v-show` so scroll positions and selections survive tab switches, and `ConsolePanel.vue` under the main area bound to the active repo.
- `RepoWorkspace.vue`: top row is a horizontal splitter between `CommitGraphView.vue` and `FilesPanel.vue`; a vertical splitter separates that row from `DiffViewer.vue`. Selecting the Working Tree row switches the files panel to working tree mode; selecting a commit switches it to that commit's files. Selecting a file loads its diff.
- shadcn-vue components to add with `npx shadcn-vue@latest add`: `resizable` (reka-ui Splitter, for every panel divider), `tabs`, `scroll-area`, `context-menu`, `dropdown-menu`, `tooltip`, `dialog`, `command`, `badge`, `separator`, `toggle`, `popover`, `collapsible`, `tree` if available (otherwise a small recursive `LibraryTree.vue`).
- `DiffViewer.vue`: hunks parsed in TypeScript from the unified diff text returned by Part 1 (`src/lib/diff/parseUnified.ts`), rendered with the same hand-rolled virtualisation as the graph (fixed row height, `overscan`), unified and side-by-side modes (side by side shows each side's own lines, joined across a gap by curved SVG bands; `src/lib/diff/sideLayout.ts`), per-word intra-line highlighting with a small LCS on changed line pairs, whitespace and CRLF toggles mapped to `-w` and `--ignore-cr-at-eol`, binary and "too large" placeholders, `eol` badge. Syntax colours come from Shiki in a Web Worker (`src/lib/diff/syntax.worker.ts`, 62 chosen languages, each grammar loaded on first use).
- `FilesPanel.vue`: flat list with path, status letter and colour, rename arrows, added and deleted counts from numstat; tree view toggle later.
- `BranchesPanel.vue`: local branches (current in bold, ahead and behind badges), remotes grouped by remote, tags, stashes; clicking a ref selects its commit in the graph.
- `ConsolePanel.vue`: rows from the session's console buffer, newest at the bottom, each with the display command line, duration and exit code, expandable output, a copy button, and filter chips wired to the `Category` enum. The later preference `console.hide_internal` defaults the Internal and Watcher chips off.
- Theme: dark by default, `.dark` on `<html>` driven by the `appearance.theme` setting through `@vueuse/core` `usePreferredDark`.
- Window state through `tauri-plugin-window-state`; folder picker through `tauri-plugin-dialog`; capabilities added to `src-tauri/capabilities/default.json`.

## Build story for both platforms

- **Linux (today)**: `npm run tauri dev` inside WSL, window appears through WSLg. Debug build already exists, so the Tauri Linux prerequisites are installed.
- **Windows**: there is no Rust on the Windows side yet, and no cross-compilation tooling (no `cargo-xwin`, `lld-link` or `makensis`) in WSL, so the Windows binary is built natively on Windows. One-time setup on Windows: `rustup` (MSVC toolchain), Visual Studio Build Tools with the C++ workload, WebView2 (already present on Windows 11). Use a separate checkout on the Windows filesystem (for example `C:\src\stashpanda`) rather than building through `\\wsl.localhost`, which is very slow for Cargo. Then `npm install` and `npm run tauri dev` from PowerShell.
- Platform-specific code is confined to `transport/local.rs` (CREATE_NO_WINDOW, discovery paths), `transport/wsl.rs` (Windows only at runtime, compiled everywhere), `transport/paths.rs` and `transport/ssh_auth.rs` (agent socket versus named pipe, home directory for `.ssh`). Everything else, including the whole SSH client, is shared because `russh` is pure Rust.
- CI later: GitHub Actions matrix with `tauri-apps/tauri-action` for `windows-latest` and `ubuntu-24.04`.

## Milestones and implementation order

**M0 Foundations** (no Git yet beyond opening a repo)
1. Cargo and npm dependencies (Part 1 crate table, `pinia`, shadcn components). Remove the `greet` demo.
2. tauri-specta builder in `lib.rs`, one dummy command, confirm `src/lib/bindings.ts` exports and `vue-tsc --noEmit` passes under TypeScript 6.
3. `error.rs`, `settings.rs`, `ui_state`, persist task, `storage/` (connection, WAL, schema, migrations), `AppState` managed in Tauri.
4. `transport/paths.rs` with table-driven tests. `transport/mod.rs`, `local.rs`, `discover.rs`, `git/versions.rs`.
5. `git/console.rs` and `GitClient::exec`: the single execution path with timing, truncation, classification and `console_event`.
6. `repo/*`, `open_repo` and `close_repo` (local transport only), per-repo RwLock.
7. App shell: toolbar, sidebar with the two panels, tab strip, three-pane workspace with splitters, console panel; Pinia stores; the "Add repository" dialog (folder picker or typed path, resolved transport shown, Advanced connection override); open a repo and see its probe commands in the console.

**M1 Read-only browse** (the first usable release)
8. Queries with parser tests on captured bytes: `status`, `refs`, `log` (streaming), `commit_files`, `diff`. Branches panel and files panel wired.
9. `graph/` crate-internal modules with unit tests (linear, fork and merge, octopus, two roots, worktree row, missing parent, HEAD pinning), `snapshot.rs`, `graph_open` / `graph_rows` / `graph_find`.
10. Graph frontend: virtual list, rows, `GraphLanesSvg`, ref pills, working tree row, selection, keyboard, resizable columns. Validate scrolling on a 100k-commit clone.
11. `git/catfile.rs` pinned helper, `blob.rs`, `Transport::read_file`; `DiffViewer` unified then side by side.
12. Cache write-through and cache-first `graph_open`; incremental refresh; LRU eviction.
13. `watch/native.rs` and `coalesce.rs`; confirm exactly one `repo_changed` per `touch`, `git commit`, `git checkout`.
14. `transport/wsl.rs` (distro list, `-e` form, UTF-16 sniff, argument self-test, remote-pid kill), tested from the Windows build against `\\wsl.localhost\Ubuntu\home\me\code\...` and from the Linux build against `/mnt/c/...` with Windows `git.exe`.
15. `watch/polling.rs`, `watch/remote.rs` (inotifywait), downgrade paths, `watcher_state` UI hint.
15b. `keys/` module: detection, PPK v2 and v3 parser, legacy PEM decryption, conversion to `ssh_key::PrivateKey`, OpenSSH and PPK v3 writers, agent client, fixtures and round-trip tests; the Settings › SSH Keys screen and the identity picker. Done before the SSH transport so tier 1 is usable with every key Michael already has.
16. `transport/ssh_pool.rs`, `ssh_auth.rs`, `ssh.rs` over `russh`: connect and keepalive, agent then key file then password auth, `known_hosts` check with the accept dialog, `SshChannelProcess`, pinned cat-file and inotifywait channels, reconnect with backoff. Tested against `sshd` on localhost from WSL first, then from the Windows build against the same WSL `sshd` and against a real Brave Digital host.
17. `run_batch` for wsl (refs, status and stash list in one `wsl.exe` spawn per refresh); for SSH only if channel round trips prove to matter.

**M2 Local writes**: stage, unstage, discard, commit (with message editor and amend), stash save and apply, checkout, branch create and delete, `IndexLocked` retry and stale-lock removal, `fix_safe_directory`, enable the toolbar groups.

**M3 Remote and history operations**: fetch, pull, push with streamed progress via `operation_progress`, merge, rebase, conflict listing with `:1:`/`:2:`/`:3:` stages, SSH tier 2 (`ssh_exec.rs`) if any real host needs `ProxyJump`, GSSAPI or an algorithm `russh` lacks, console `hide_internal` preference, syntax highlighting in the diff, Gravatar opt-in, graph text filter.

## Verification

- **Rust unit tests** (`cd src-tauri && cargo test`): path mapper table (plus the ignored `wslpath` cross-check when `wslpath` exists), every parser on captured Git output, ordering and lane sweep fixtures, error classification.
- **Type check and build**: `npm run build` (runs `vue-tsc --noEmit` then Vite) and `cargo clippy --all-targets` must be clean. Confirm the specta-generated `bindings.ts` compiles under TypeScript 6 at step 2, before anything depends on it.
- **Linux manual run** (`npm run tauri dev` in WSL): open `/home/me/code/stashpanda` and a large repo; confirm first paint from cache under 50 ms on reopen, smooth scrolling on 100k rows, the console shows every command with its full argv, and one `repo_changed` per burst when running `touch`, `git commit` and `git checkout` in a terminal.
- **Cross-side manual runs**: from Linux open a `/mnt/c/...` repo with `/mnt/c/Program Files/Git/bin/git.exe` and check the console shows `C:\...` paths; from the Windows build open `\\wsl.localhost\Ubuntu\home\me\code\stashpanda` and check the console shows `wsl.exe -d Ubuntu -e ... git -C /home/me/code/stashpanda`.
- **SSH**: with `sshd` running in WSL and `ssh localhost` working, add `brave@localhost:/home/me/code/stashpanda` as an SSH repo; confirm one connection is authenticated (agent first, then key file), each query is one channel with its own exit code and stderr in the console, blob reads come through the pinned cat-file channel, restarting `sshd` produces `transport_state: Disconnected` then a reconnect, an unknown host raises the fingerprint dialog, and pointing a repo at a `.ppk` identity, an encrypted PEM and an OpenSSH key each authenticates after at most one passphrase prompt.
- **Key manager**: the SSH Keys screen lists every file in the Windows and WSL `.ssh` folders with the right format badge; converting a PPK v2, a PPK v3 and an encrypted PEM to OpenSSH produces files that `ssh-keygen -y -f <file>` accepts and whose public key matches the original; Add to agent makes the key appear in `ssh-add -l`; Generate produces an ed25519 key that `sshd` on localhost accepts.
- **Watcher fallbacks**: with `inotifywait` absent the console and `watcher_state` should say polling is in use; installing `inotify-tools` and reopening should switch to `RemoteInotify`.
- **Persistence**: close and reopen the app; tabs, active tab, panel sizes, column widths and window geometry return; `settings.json` stays diff-friendly while the UI state rows in the database churn.

## Implementation notes (2026-09-18, after the first build)

What changed from the plan above while building it, and why. The plan text is left as written; this
section is the delta.

- **Merge-patch commands take a JSON string, not a JSON value.** `settings_update` and
  `ui_state_update` accept `patch: String`. The specta TypeScript exporter (2.0.0-rc.25) overflows
  its stack on `serde_json::Value` because that type is recursive; the frontend calls
  `JSON.stringify(patch)`.
- **The repository library is stored flat.** `RepoLibrary { groups: LibraryGroup[], repos: RepoEntry[] }`
  with `parentId` on groups and `groupId` on repositories, instead of a recursive `LibraryNode` enum.
  Easier to edit from the UI, and an internally tagged recursive enum with a newtype variant is
  something both serde and the exporter reject.
- **Display order ties are broken by insertion order, not object id.** Commits made within the
  same second share a commit time; Git breaks that tie by the order it emitted them, so the sort
  key is `(commit_time, store index)`. This is what keeps HEAD's tip above a sibling tip created in
  the same second.
- **Lane 0 is granted to the Working Tree row as well as to HEAD.** The row's only parent is HEAD,
  so it must sit directly above it; the first version reserved lane 0 for HEAD alone and pushed the
  working tree row into lane 1.
- **`git rev-parse --verify` accepts one revision only.** The HEAD query is
  `rev-parse HEAD --symbolic-full-name HEAD` (no `--verify`), with `symbolic-ref -q HEAD` as the
  fallback for an unborn branch.
- **No hand-written PPK parser was needed.** The `ssh-key` crate that `russh` depends on parses PPK
  version 2 and version 3, encrypted or not (`PrivateKey::from_ppk`), and its `DotSsh` type scans a
  `.ssh` folder. Verified against real keys: each PPK produced the
  same SHA-256 fingerprint as its PEM or OpenSSH twin. `russh::keys::decode_secret_key` covers
  OpenSSH, PKCS#1 RSA, SEC1 and PKCS#8. The only code StashPanda adds is `keys/pem_legacy.rs` for
  OpenSSL-encrypted PEM with a DEK-Info cipher other than AES-128-CBC (for example
  DES-EDE3-CBC). PPK export (writing PPK) is not implemented.
- **`russh` is built with the `ring` backend**, not the default `aws-lc-rs`, so the Windows build
  does not need CMake or NASM.
- **`russh-config` does not expose `IdentitiesOnly`.** An explicit identity file, from the repo
  settings or from `~/.ssh/config`, implies it; otherwise the agent is tried first.
- **Host key prompts are not modal.** An unknown host fails the open with `sshHostKeyUnknown` plus
  the fingerprint, the offered key is kept in memory, and the `ssh_trust_host` command records it
  in `known_hosts`; the next open succeeds. A changed key is `sshHostKeyChanged` with no override.
- **Passwords and passphrases** are supplied through `ssh_set_password` and `keys_unlock` and held
  in memory for the session only. No keychain integration yet.
- **`GraphSnapshotEvent` carries the whole `GraphMeta`** (head, head ref, working tree summary,
  open lanes) so the frontend needs no follow-up `graph_meta` call.
- **The status-only refresh does not persist**, only full refreshes write the cache; the cache
  never stores the Working Tree row.
- **The native watcher drops `Access` events and Git-ignored paths.** On Linux `notify` reports
  reads: `git status` opening every directory and `for-each-ref` reading the refs folders showed
  up as changes, so each refresh triggered the next one (347 full refreshes in two minutes with
  nothing changing on disk). Access events are now skipped, and paths listed by
  `git ls-files --others --ignored --exclude-standard --directory` (refreshed every minute) are
  filtered out so build output and caches inside the worktree do not count either. After the fix
  a two-minute run with one touched file and one `git add` produced exactly those two events.
- **Type scale is rem-based** (2026-09-22; the root became 14 px with the design system in
  `27897ec`). `html { font-size: 14px }` in `index.css`; UI text uses `text-base`, headers
  `text-xs`, pills `text-[0.7rem]`; `--pill-h` is 1.2rem. Row heights are whole pixels, because the
  virtual lists and the graph SVG position rows by pixel counts: `ROW_HEIGHT` in
  `src/lib/graph/paths.ts` is 1.6rem rounded (22 px), and `main.ts` sets `--row-h` to it at startup
  (2026-09-30; with the unrounded 22.4 px the rows drifted 0.4 px a row away from the graph dots).
  Diff rows are sized from a setting, see the 2026-09-30 notes below. Changing the root size still
  rescales everything consistently. Branch and tag pills use the flat `pill-*`
  utilities (solid fill, hairline border, no shadow); the selected row is the single solid colour
  `--row-selected`, a step lighter than the well and darker than the header chrome.
- **The graph column sizes itself to the snapshot's lane count.** `GraphColumns.graph` is
  `Option<u32>`: None (the default) means auto, `autoGraphColumnWidth(maxLanes)` with no upper
  bound; dragging the header handle pins a width and double-clicking it returns to auto. The lane
  sweep itself has never had a cap.
- **Menus** (2026-09-22). The menu bar is native, built in Rust in `src-tauri/src/menu.rs` with
  Tauri's `MenuBuilder` (File, Edit, View, Repository, Help). Every click is forwarded to the
  frontend as a `menu_action` event carrying a dotted id such as `repo.create_branch`, and the
  frontend owns the behaviour so it can reuse its dialogs and stores. The window theme is pinned to
  `Dark` in `tauri.conf.json`, which is what makes `muda` draw the Windows menu bar dark. Right-click
  menus on commits, ref pills and branches are in-app shadcn `ContextMenu`s, not native popups: they
  match the theme, show branch and tag names inline and can grey items out with a reason.
- **First write operations** live in `src-tauri/src/git/ops/mod.rs` (checkout, branch create and
  delete, merge and merge abort, tag create and delete) and `ipc/ops.rs`. Each command takes the
  repository's exclusive lock, runs with `Category::UserAction`, returns Git's own stdout and stderr
  in an `OpResult` with a `conflicts` flag for merges that stop on conflicts, and then runs a full
  graph refresh so the UI updates before the watcher fires. Names being created are validated as
  Git reference names; revisions the user selected only reject a leading `-` and control
  characters, because `HEAD~1` and `origin/dev` are legitimate targets.
- **Cache-first paint loads the whole cached store**, not a 500-row first page. For 100k commits
  that is a few hundred milliseconds from SQLite and one sort and sweep; the 500-row fast path in
  the plan is deferred until a repository shows it is needed.

## Implementation notes (2026-09-30, the commit dialog and a round of interface work)

What changed from the plan in this round. [next-steps.md](next-steps.md) lists each change with its
files; this is what a reader of the plan above needs to know.

- **The Windows build runs from a mirror, not a separate checkout** (2026-09-21).
  `scripts/windows.sh` copies `/home/me/code/stashpanda` to `C:\src\stashpanda` with `rsync` and runs
  the Windows toolchain there through WSL interop (`npm run win:ux`, `win:dev`, `win:build`,
  `win:sync`), instead of the separate `C:\src\stashpanda` checkout in "Build story". Its check
  for a leftover process on port 1420 must ignore `powershell.exe`'s exit code: `Get-NetTCPConnection`
  on a free port exits 1 even with `-ErrorAction SilentlyContinue`, which `set -euo pipefail`
  turned into a silent exit of the whole script.
- **Committing takes paths.** `op_commit(message, paths, amend, sign_off)`: with `paths`, they are
  staged (`add -A`) and, when other files are staged too, the commit adds `--only` so those stay
  staged and out of it. The paths go on the command line (at most 24 KB), because the message
  already uses stdin. An empty `paths` with `amend` runs `--amend --only`, which changes only the
  message.
- **The plan's commit message editor is a Commit dialog** (`dialogs/CommitDialog.vue`, from the
  toolbar's Commit button). It lists every outstanding file with a checkbox; the Files panel
  selection starts ticked, otherwise the new `commit.preselect` setting (`all` or `staged`).
  Ticking exactly the staged files commits the index as it is (no paths), so a partly staged file
  keeps its unstaged part out; any other set is committed as it is on disk.
- **List selection is hand-written** (`composables/useListSelection.ts`, shared by the Files panel
  and the Commit dialog), after reka-ui's `Listbox` turned out to ignore Shift and Ctrl on mouse
  clicks. Ctrl+A outside a text field never selects page text (`App.vue`); it selects every file.
- **Refs can be hidden in the graph**, which the plan does not have. `GraphSnapshot::build` takes
  the hidden set (`RepoHandle::hidden`, loaded by `repo_open` from `perRepo[key].hiddenRefs` in
  the UI state) and lays out only commits reachable from the refs still shown; HEAD is a tip of
  its own only when detached. `graph_set_hidden_refs` rebuilds from the store already in memory,
  with no `git log`. The store stays whole, and the snapshot keeps the unfiltered order in
  `full_order` so the cache's `seq` still covers every commit.
- **`GraphMeta` carries two more fields:** `refColours` (each shown ref's pill colour, the same as `RefLabel::colour`, for
  the Branches panel's show and hide tags) and `widestAuthors` (the 24 longest distinct author
  names, for sizing the Author column).
- **The row grid has an ID column:** `var(--col-graph) minmax(0, 1fr) var(--col-author)
  var(--col-date) var(--col-id)`. Author, Date and ID size themselves to their widest possible
  value (`lib/graph/columnWidths.ts`, measured with the real fonts) until a divider is dragged;
  `GraphColumns.author`, `date` and `id` are optional, and the old always-saved defaults 140, 150
  and 76 are read as automatic. Header and rows reserve the same scrollbar space.
- **A snapshot swap keeps the list at the top** when it is at the top, instead of keeping the old
  first row in place, so rows that appear above it (the Working Tree row after the cached paint,
  new commits) are in view.
- **The side-by-side diff is two panes in one vertical scroll.** The split starts at 50% and has a
  draggable divider; each side clips its text and scrolls sideways on its own scrollbar, and
  moving one moves the other to the same fraction of its range. Diff rows are `diff.lineHeight`
  (default 1.2) times `diff.fontSize`, in whole pixels, and the virtual list uses the same number;
  `useVirtualRows` accepts the row height as a getter. The font, size, tab width and ligatures
  come from the `diff` section of settings through `src/lib/diff/font.ts`.
- **The Transport trait gains `modified_times`** (filesystem for Local and WSL, one remote `stat`
  loop that works with GNU and BSD `stat` for SSH), behind the `get_modified_times` command, for
  sorting the Files panel by date modified. The Files panel also groups by status, folder or
  nothing, and sorts by name, Git status or date.
- **Fonts are bundled.** IBM Plex Sans, Outfit and JetBrains Mono come from their `@fontsource`
  packages, imported in `main.ts`, instead of a Google Fonts `@import`, so the app looks the same
  offline.
- **The native Theme menu follows the setting:** `menu.rs` keeps the three check items and
  `sync_theme` ticks one, at startup, after a click and in `settings_update`.
- **Settings and UI state gained** `commit.preselect`, `appearance.diffLineHeight` (now
  `diff.lineHeight`), `GraphColumns.id` and `perRepo[key].hiddenRefs`. Settings are edited in the
  Settings dialog (File > Settings) or by hand in `settings.json`.

### Status against the milestones

Done: M0 and M1 entirely, since 2026-10-14, apart from PPK export (left until a tool needs it) and
the checks that need a real SSH host and the Windows window; M2 entirely, since 2026-10-14. M3 so
far: pull, fetch, push, sync, clone, merge, rebase, cherry-pick and revert with conflict
resolution, file history and blame, automatic fetch, SSH reconnect with backoff and a
`transport_state` event, and syntax colours in the diff viewer. See `next-steps.md` for each item.

Not done yet: in M3, the three-pane merge editor (planned in `next-steps.md`), and the SSH tier 2
fallback and `ProxyJump`, both only if a real host needs them.

### Tests

`cd src-tauri && cargo test` runs 116 tests, on Linux and Windows: path mapper table, every parser on
captured bytes, ordering (date and topological) and lane sweep fixtures, snapshot and cache round
trips, the incremental refresh against a full load, the graph search, end-to-end runs of every query
and every write operation against throwaway repositories (including line staging, reset,
cherry-pick, revert, conflicts with Continue, Skip and Abort, and `safe.directory`), file history
and blame, pull, fetch, push, sync and clone against throwaway
remotes, stopping commands on each transport, the WSL batch and the long-lived `cat-file` through
`wsl.exe`, `inotifywait` through the local and WSL transports, opening a WSL repository from the
Windows build and a Windows folder with Windows Git from WSL, the keychain against an in-memory
store, the TypeScript bindings export, and the SSH transport against a private `sshd` (host keys,
passphrases, refused passwords, reconnecting with backoff). Tests that need WSL interop or `inotifywait` skip themselves
elsewhere. Three ignored tests need this machine or a large repository:
`cargo test -- --ignored agrees_with_wslpath`, `cargo test -- --ignored probe_real_keys --nocapture`,
and `STASHPANDA_TIMING_REPO=<path> cargo test --lib time_large_repository -- --ignored --nocapture`.
