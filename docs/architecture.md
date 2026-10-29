
# StashPanda: architecture

How StashPanda works as of 2026-10-25 (version 0.6.0). [next-steps.md](next-steps.md) lists what is
still to build. The "Pitfalls" section near the end collects the things that cost time to find out,
so they are not found out twice.

## What StashPanda is

StashPanda is a standalone desktop Git GUI that differs from other Git GUIs in four ways:

1. **It does not ship Git.** It runs whichever Git executable the operating system already has,
   with the user's own Git configuration, credential helpers and SSH agent.
2. **WSL Git and remote Git over SSH are first class.** A repository can live on Windows, inside
   a WSL distro, or on another machine reached over SSH, and every command for that repository
   runs through the right wrapper (`wsl.exe`, or an SSH channel) without the user noticing.
3. **It is fast.** Cached history paints before Git answers, Git calls are batched or served by a
   long-lived helper process, and the commit graph is a virtualised SVG.
4. **Many repositories at once:** one tab per repository, with a library of repositories in the
   left sidebar.

Decisions taken with Michael on 2026-09-18 that still shape the code:

| Decision | Choice |
|---|---|
| Platforms | A Windows `.exe` and a Linux binary (run in WSL through WSLg) from one codebase. macOS is prepared but not built yet |
| SSH | SSH is the transport for any repository that is neither a local folder nor a WSL folder. Every command runs over SSH, and file contents come through Git or the SSH channel, never the local filesystem. The transport is chosen automatically from the path, and a per-repository setting can override it |
| SSH implementation | The in-process Rust library `russh`, with one connection per host and one channel per command. A fallback through OpenSSH is planned only for hosts `russh` cannot serve |
| SSH keys | Keys must be easy: StashPanda reads PEM, PPK (PuTTY v2 and v3), OpenSSH and PKCS#8 private keys directly, encrypted or not, and converts between them, so PuTTYgen is never needed |

## Platforms and builds

- **Linux:** `npm run tauri dev` inside WSL; the window appears through WSLg.
- **Windows:** built natively on Windows from a one-way mirror, not a separate checkout.
  `scripts/windows.sh` copies `/home/me/code/stashpanda` to the Windows folder named by
  `STASHPANDA_WIN_DIR` in `.env` (see `.env.example`) with `rsync`,
  leaving out `node_modules`, `dist`, `src-tauri/target` and `.git`, and runs the Windows
  toolchain there through WSL interop. Edit only in WSL, never in the mirror. Run these from a
  WSL shell:

  | Command | What it does |
  |---|---|
  | `npm run win:ux` | Vite runs in WSL and the Windows app loads its frontend from it, so Vue, TypeScript and CSS changes hot reload. Every 2 seconds it copies `src-tauri/` to the mirror, so the Windows `tauri dev` rebuilds on Rust changes. `src-tauri/tauri.wsl-dev.conf.json` empties `beforeDevCommand`, and WSL2 forwards Windows `localhost:1420` to Vite. Ctrl-C stops both sides |
  | `npm run win:dev` | Runs `tauri dev` entirely on Windows, with its own Vite. It refuses to start if a leftover process holds port 1420 on Windows, and names that process |
  | `npm run win:build` | Runs `tauri build` on Windows, then copies the release binary and the NSIS installer to `bundle/` in both the WSL checkout (ignored by Git) and the mirror |
  | `npm run win:sync` | Mirrors the source and runs `npm install` on Windows, nothing else |

  The Windows side has rustup with the `stable-x86_64-pc-windows-msvc` toolchain, Node 24,
  Visual Studio with the "Desktop development with C++" workload and the Windows 11 SDK, and the
  WebView2 runtime. Windows builds only the NSIS installer: `src-tauri/tauri.windows.conf.json`
  sets `"targets": ["nsis"]` over the `"all"` in `tauri.conf.json`.
- **macOS (code ready, not built):** it needs a Mac or a macOS CI runner, plus an Apple Developer
  ID certificate and notarization, without which Gatekeeper refuses a downloaded app.
  `--target universal-apple-darwin` makes one app for Apple Silicon and Intel. The macOS-only
  code:
  - `platform::adopt_login_shell_path` gives the app the login shell's `PATH` at start-up, since
    an app started from Finder gets only `/usr/bin:/bin:/usr/sbin:/sbin`, which is too little for
    Git hooks and credential helpers.
  - `transport/discover.rs` tries Homebrew's Git (`/opt/homebrew/bin`, `/usr/local/bin`) before
    `PATH`, and takes Apple's `/usr/bin/git` only when `xcode-select -p` says the Command Line
    Tools are installed: without them, that `git` opens an install dialog instead of running.
  - `menu.rs` builds a Mac menu bar: a StashPanda application menu (About, Settings, Services,
    Hide, Quit with Cmd+Q) and a Window menu, with real Cmd key equivalents. Quit is
    StashPanda's own `file.exit`, which closes the window through the close guard in
    `shutdown.rs`.
  - Passphrases go to the macOS Keychain through `keyring`'s default features.
  - It compiles for `aarch64-apple-darwin` (checked from Linux with a stand-in C compiler for
    `objc2-exception-helper`), but it has never run on a Mac. `tauri-driver` has no macOS
    support, so a Mac build has to be tested by hand.
- **CI:** `.github/workflows/build.yml` runs the TypeScript check, the frontend tests and
  `cargo test --lib` on Ubuntu 22.04 for every push to `main` and every pull request. Only for a
  version tag, or a run started by hand from the Actions tab, does it then build `.deb`, `.rpm`
  and AppImage there (22.04 is the oldest supported release, for glibc) and the NSIS installer on
  `windows-latest`, and put them in a draft release for `v<package.json's version>`. A run by hand
  from a branch makes the draft for the commit it built, and GitHub makes the tag when the draft
  is published; it refuses a version that is already published or whose tag is on another
  commit. macOS is in its matrix, commented out.
- **Updates:** installed copies update themselves from the GitHub releases with
  `tauri-plugin-updater`. They read `latest.json` from the newest *published* release, so a draft
  reaches nobody until it is published. CI builds with `src-tauri/tauri.release.conf.json`, which
  turns on the updater's `.sig` files, signed with the private key in the repository secrets
  `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; the public half is
  `plugins.updater.pubkey` in `tauri.conf.json`. Local builds leave that file out and need no
  key. The bundler signs every installer, `.deb` and `.rpm` included: the updater installs those
  too, with `pkexec dpkg -i` or `rpm -U`, which ask for the password. A build whose key does not
  match `pubkey` warns about it in the log. `scripts/updater-manifest.mjs` writes `latest.json`
  from the signed files. Losing the private key means installed copies can never update again: a new
  key needs a new public key, which only a manually installed version carries. The app checks
  when it starts (`updates.checkAutomatically`) and from Help › Check for Updates;
  `update_prepare` refuses while Git is writing and saves settings first, because the Windows
  installer ends the app without going through the close guard.
- **Versions:** `npm version <x.y.z>` bumps the version everywhere. Its `version` script runs
  `scripts/sync-version.mjs`, which copies the version into `src-tauri/tauri.conf.json`,
  `src-tauri/Cargo.toml` and `Cargo.lock`, then `scripts/changelog-version.mjs`, which turns
  `## Unreleased` in `CHANGELOG.md` into the version's section. The release job takes the
  release notes from that section. Every commit adds its line under `## Unreleased`; see
  [changelog.md](changelog.md).
- **App icon:** `npm run icons` runs `tauri icon assets/app_icon.png`, which writes every icon
  size into `src-tauri/icons/`. The Photoshop source in `assets/` is ignored by Git.

Platform-specific code is confined to `platform.rs`, `transport/local.rs` (`CREATE_NO_WINDOW`,
process groups, discovery paths), `transport/wsl.rs` (used only by the Windows build at runtime,
but compiled everywhere), `transport/paths.rs`, `keys/agent.rs` (agent socket or named pipe),
`menu.rs` and `webview.rs`. Everything else is shared, including the whole SSH client, because
`russh` is pure Rust.

## Overview

```
┌──────────────────────────── Vue 3 frontend (WebView) ─────────────────────────────┐
│ Pinia stores, one session per open tab  ←→  typed IPC bindings (tauri-specta)      │
└───────────────────────────────────────┬────────────────────────────────────────────┘
                                        │ Tauri commands and events
┌───────────────────────────────────────┴────────────────────────────────────────────┐
│ Rust core (src-tauri/src)                                                          │
│  AppState: settings.json, SQLite database (cache and UI state), open repositories │
│  RepoHandle per open repository: transport, GitClient, locks, cat-file helper,     │
│  watcher, graph snapshot                                                           │
│  transport/{local,wsl,ssh} ── git/{queries,ops} ── graph/ ── watch/ ── keys/       │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Everything to do with Git happens in Rust. The frontend never starts a process and never touches
the filesystem: it only calls commands and listens to events. That keeps the WebView fast, keeps
the three transports invisible to the interface, and lets the same interface work on every
platform.

## Measured facts that shaped the Git layer

Measured on 2026-09-18 on Michael's machine, unless a date says otherwise.

| Fact | Consequence |
|---|---|
| Starting a process from WSL: native `git` about 1 ms, `git.exe` through interop about 46 ms, `wsl.exe -e true` about 87 ms | Anything through `wsl.exe` or SSH is batched into one process or served by a long-lived helper |
| `wsl.exe -d Ubuntu -e git -C /path ...` passes stdout bytes and the exit code through unchanged | The WSL transport uses the `-e` form (no login shell). `wsl.exe`'s own error messages can be UTF-16LE, so the transport checks before decoding |
| The OpenSSH 9.5 client built into Windows dies at once with `ControlMaster=auto`: `getsockname failed: Not a socket` | Connection sharing through the OpenSSH client is unusable on Windows. This is the main reason the SSH transport uses `russh` in-process rather than driving `ssh.exe` |
| `russh` defaults to the `aws-lc-rs` crypto backend, which needs CMake and NASM to build on Windows | StashPanda builds `russh` with the `ring` backend |
| Real `.ssh` folders mix PEM, PPK v2 and v3, OpenSSH and PKCS#8 keys, encrypted or not, and a `.pem` file can hold an OpenSSH-format key | Keys are recognised by their content, never by their extension |
| Both Gits (2.43 in WSL, 2.47 on Windows) support `status --porcelain=v2 -z`, `for-each-ref` with `%00`, `log -z --format`, `diff-tree --raw -z` and `cat-file --batch-command` | The minimum supported Git is 2.36. Anything newer sits behind a capability flag (`git/versions.rs`) |
| On git.git (85,563 commits), 2026-10-14, release build: loading the whole cache 260 ms, the first 500 rows 2.6 ms, a refresh after one new commit 299 ms, a 200-row page 0.3 to 2.3 ms | The first paint reads 500 rows when the cache holds more than 10,000 commits. Debug builds build snapshots about nine times slower, so timings in `tauri dev` look worse than the shipped app |

## Rust backend (`src-tauri/src/`)

### Modules

```
lib.rs                 Tauri builder, tauri-specta command and event lists, TypeScript export, tracing
main.rs                entry point
error.rs               GitError and classify(): Git's stderr and exit code to an error kind
platform.rs            which OS the app runs on, whether that Linux is WSL, the macOS login-shell PATH
settings.rs            settings.json model, defaults, migrations of old field names
ui_state.rs            the remembered UI state model (stored in the database)
state.rs               AppState: paths, settings, UI state, database, open repositories, clones in progress
persist.rs             debounced writer for settings.json and the UI state rows; flushed on exit
shutdown.rs            closing the window waits while Git is changing a repository
menu.rs                native menu bar; clicks go to the frontend as menu_action events
webview.rs             turns off WebView2's browser shortcuts on Windows
storage/               SQLite connection, schema.sql (cache), schema_v2.sql (UI state), repo_cache.rs, ui_prefs.rs
transport/mod.rs       Transport and Process traits, Cmd, Output, run, run_batch, run_streaming
transport/local.rs     run an executable directly (git, git.exe, or /mnt/c/.../git.exe through interop)
transport/wsl.rs       wsl.exe -d <distro> -e ..., distro list, batching several commands into one wsl.exe
transport/ssh.rs       SSH transport: one russh channel per command
transport/ssh_pool.rs  one russh connection per (host, port, user): authentication, host keys, keepalive, reconnect
transport/paths.rs     HostPath and NativePath, and translating between them; pure string logic
transport/discover.rs  find Git executables: settings override, then PATH, then well-known places
repo/mod.rs            RepoHandle: one open repository, its locks and its operation registry
repo/resolve.rs        library entry to transport and native path; RepoEntry.key
repo/open.rs           open: resolve, probe, rev-parse, start the helpers and the watcher
repo/watch_start.rs    pick and start a repository's watcher and coalescer
repo/clone.rs          clone, including into a folder that already has files in it
repo/cross_e2e.rs      tests across the Windows and WSL boundary
git/mod.rs             GitClient::exec: the one place a Git request becomes a process
git/console.rs         console events for every command
git/catfile.rs         the long-lived cat-file helper
git/parse.rs           byte-level splitting and lossy decoding of Git output
git/versions.rs        Git version and capability flags
git/queries/           one module per read query, each with a parser tested on captured output
git/ops/mod.rs         every write operation
git/ops/lines.rs       staging, unstaging and discarding single lines and hunks
graph/                 oid, commit store, display order, lane sweep, snapshot, loader
watch/                 native (notify), remote (inotifywait), polling, and the coalescer
keys/                  key detection, legacy PEM decryption, agent, keychain, passphrase cache
ipc/                   thin #[tauri::command] wrappers: settings, repos, query, graph, ops, keys, fonts, events
```

### Main crates

| Crate | Used for |
|---|---|
| `tokio`, `tokio-util` | starting processes, cancellation tokens, framing |
| `tauri-specta`, `specta`, `specta-typescript` | the generated TypeScript bindings. Pinned exactly (`=2.0.0-rc.25`, `=0.0.12`), because release candidates break each other |
| `russh` (`ring` backend), `russh-config` | the SSH client and `~/.ssh/config` |
| `ssh-key` (`ppk` feature), `rsa`, `pkcs1`, `aes`, `cbc`, `des`, `md-5` | reading every key format, including PPK v2 and v3; the legacy PEM decryption |
| `keyring` | passphrases in Windows Credential Manager, the Secret Service or the macOS Keychain |
| `rusqlite` (`bundled`) | the cache and UI state database |
| `notify-debouncer-full` | native filesystem watching |
| `arc-swap`, `rustc-hash`, `smallvec`, `bstr` | swapping graph snapshots, fast hash maps, parent lists, byte strings |
| `uuid` (`v4`, `v5`) | operation ids and `RepoEntry.key` |
| `which`, `dunce`, `shlex` | finding executables, plain Windows paths, quoting arguments for SSH |
| `fontdb` | the list of monospaced fonts for the diff font setting |
| `tauri-plugin-dialog`, `tauri-plugin-window-state`, `tauri-plugin-opener` | folder picker, window size and position, opening files |
| `tauri-plugin-updater`, `tauri-plugin-process` | installing new versions from the GitHub releases, and restarting afterwards. Held at `~2.12` and `~2.3`, the last versions that work with Tauri 2.11: newer ones need Tauri 2.12, whose wry uses `webview2-com` 0.39 rather than the 0.38 pinned for `webview.rs` |
| `webview2-com`, `windows-core` (Windows only) | WebView2 settings Tauri does not expose, at the versions `wry` uses |

### Running Git

`GitClient::exec` (`git/mod.rs`) is the single place that turns a request into a process. Every
command gets:

```
git --no-optional-locks -c color.ui=never -c core.quotepath=false -c i18n.logOutputEncoding=utf-8 -C <native repo path> ...
```

with `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C.UTF-8` and `GIT_OPTIONAL_LOCKS=0` in its environment
(written as `env K=V git ...` through `wsl.exe` and SSH). `--no-optional-locks` stops `git status`
rewriting `.git/index`, which would otherwise wake the app's own watcher.

Each command has a `Category`: `UserAction` for what the user asked for, and `Internal` for the
app's own reads and the watcher's checks. Internal commands are logged to the Console only while
`console.logInternal` is on. A successful internal or watcher command's console event keeps only
its byte counts, not its output (`kept_output` in `git/console.rs`).

Each `ConsoleEvent` carries the command line as executed (wrapper included), a quoted display
line, `command` (just the Git arguments the caller asked for, without the common flags or the
wrapper), `title` (what the command does, such as "Fetch" or "Abort rebase", from
`command_title`), timing, exit code, and stdout and stderr truncated at `console.outputCapBytes`.

Rules for Git output: everything is parsed as bytes; text is decoded lossily at the last moment,
so a stray Latin-1 byte never aborts a parse. A file is binary when its first 8000 bytes hold a
NUL. CRLF is never rewritten in transit; the diff reports each side's line endings, and the
viewer strips `\r` for display.

### Transports

```rust
pub enum TransportKind { Local, Wsl { distro: String }, Ssh { host: String } }

#[async_trait] pub trait Transport: Send + Sync {
    fn kind(&self) -> TransportKind;
    fn paths(&self) -> &dyn PathMapper;
    fn render(&self, cmd: &Cmd) -> Vec<String>;              // the full host command line, for the console
    async fn spawn(&self, cmd: Cmd) -> Result<Box<dyn Process>>;
    async fn probe(&self) -> Result<TransportInfo>;
    async fn read_file(&self, path: &NativePath, limit: usize) -> Result<Vec<u8>>;
    async fn write_file(&self, path: &NativePath, bytes: &[u8]) -> Result<()>;  // in place, so the file's mode is kept
    async fn remove_file(&self, path: &NativePath) -> Result<()>;
    async fn modified_times(&self, paths: &[NativePath]) -> Result<Vec<Option<i64>>>;
    async fn symlinks(&self, paths: &[NativePath]) -> Result<Vec<bool>>;
    async fn list_dir(&self, path: &NativePath) -> Result<Option<Vec<String>>>;
    async fn remove_dir_all(&self, path: &NativePath) -> Result<()>;
    fn batches(&self) -> bool;                               // true for WSL
    async fn run_batch(&self, cmds: Vec<Cmd>) -> Result<Vec<Output>>;
    async fn run(&self, cmd: Cmd) -> Result<Output>;
    async fn run_streaming(&self, cmd: Cmd, progress: ..., cancel: &CancellationToken) -> Result<Output>;
    // plus shutdown() and connected()
}

#[async_trait] pub trait Process: Send {
    fn stdin(&mut self) / stdout(&mut self) / stderr(&mut self);
    async fn wait(&mut self) -> Result<Option<i32>>;
    async fn kill(&mut self) -> Result<()>;
    async fn terminate(&mut self) -> Result<()>;             // a polite stop that lets Git remove its lock files
    fn remote_pid(&self) -> Option<u32>;
}
```

`run_streaming` reads stderr as it arrives, splits it on `\r` and `\n`, strips escape codes and
passes each update to the caller, which sends it to the frontend as an `operationProgress` event.
The `stderr` it returns keeps only the last update of each line.

**Local** (`transport/local.rs`): `tokio::process::Command` with `kill_on_drop`. On Windows every
process gets `CREATE_NO_WINDOW` and never inherits the app's standard input and output.

**WSL** (`transport/wsl.rs`): `wsl.exe -d <distro> -e env ... git ... -C /linux/path ...`. The
distro list comes from `wsl.exe -l -q`, which answers in UTF-16LE. An argument with a space and
both kinds of quote arrives unsplit through `-e`, so no `sh -c` fallback is needed (checked on
2026-10-14). `run_batch` runs several commands in one `wsl.exe -e sh -c` script that prints each
command's exit code and output lengths and then the bytes; lengths, because `status -z` output
contains NUL bytes. `queries::refs_and_status` uses it, so a refresh starts one `wsl.exe` instead
of five.

**SSH** (`transport/ssh.rs`, `transport/ssh_pool.rs`): no `ssh.exe` is involved, so Windows and
Linux behave the same.

- **Connection pool:** one authenticated `russh` connection per `(host, port, user)`, shared by
  every repository on that host, with a keepalive every 15 s. At most 8 channels run at once on
  one connection, because OpenSSH servers default `MaxSessions` to 10. After a network failure the
  pool waits 1, 2, 5, 10, 30 and then 60 seconds between attempts, failing at once in between
  rather than blocking, and reports `Connecting`, `Ready`, `Degraded` or `Disconnected` in a
  `transportState` event, which the status bar shows with a countdown to the next attempt.
- **One command, one channel:** the command line is shell-quoted with `shlex`, because `exec`
  requests go through the remote login shell. Stdout is the channel's data, stderr its extended
  data, the exit code its exit status. No pseudo-terminal is ever requested, so nothing
  translates line endings.
- **Authentication**, in this order: `~/.ssh/config` is read with `russh-config` for the host
  alias, and the repository's own settings override it; then the SSH agent (`SSH_AUTH_SOCK`, or
  the `\\.\pipe\openssh-ssh-agent` named pipe on Windows); then identity files in any format the
  key manager reads, decrypted in memory after a passphrase prompt if needed; then a password,
  only if the server offers it. `russh-config` does not expose `IdentitiesOnly`: an explicit
  identity file implies it. A passphrase-protected default key whose passphrase is not known
  does not block a server that takes passwords: that server gets the password prompt, as OpenSSH
  falls back to one.
- **Host keys** are checked against `~/.ssh/known_hosts`. An unknown host fails the open with
  `sshHostKeyUnknown` and the SHA-256 fingerprint; the Trust dialog then calls `ssh_trust_host`,
  which appends the key, and the open is tried again. A changed key is `sshHostKeyChanged`, with
  no way to override it in the interface.
- **Not supported:** `ProxyCommand` and `ProxyJump` (refused with `SshUnsupportedFeature`),
  GSSAPI, certificates and PKCS#11 tokens. PuTTY's `plink` is not used for anything.

**Stopping a command** (`Cmd::own_session`): a command started this way can be stopped together
with every process it starts, and `run_streaming` returns `Cancelled` only once it has exited, or
after 5 seconds and a `kill`. So a repository's write lock is never released while Git still
runs.

- Linux and macOS: `setsid` before `exec`, so Git leads a new session with no controlling
  terminal, and SIGTERM goes to its whole process group.
- Windows: `taskkill /T /F` on the process tree. Git for Windows' `cmd\git.exe` only launches
  `mingw64\bin\git.exe`, so killing the first alone left the real Git and its
  `git-remote-https.exe` or `ssh.exe` running.
- WSL: `wsl.exe -e setsid -w sh -c 'echo $$; exec "$@"' sh <command>`. The first stdout line is
  the process group's id, and stopping runs `kill -TERM -- -<pid>` through a second `wsl.exe`.
- SSH: SIGTERM is sent on the channel and the transport waits for the exit status; `kill` closes
  the channel, which OpenSSH's `sshd` answers by ending the remote process and its children.

### Paths (`transport/paths.rs`)

| The app sees | Transport | `git -C` receives |
|---|---|---|
| `C:\foo\bar` | Local (Windows Git) | `C:\foo\bar` |
| `C:\foo\bar` | WSL | `/mnt/c/foo/bar` (the automount root is a setting) |
| `\\wsl.localhost\Ubuntu\home\me\code\x` or `\\wsl$\Ubuntu\...` | WSL, Ubuntu (the distro in the path must match) | `/home/me/code/x` |
| `/home/me/code/x` (Linux build) | Local (Linux Git) | `/home/me/code/x` |
| `/mnt/c/foo/bar` (Linux build) | Local, with `/mnt/c/Program Files/Git/bin/git.exe` | `C:\foo\bar` |
| `user@host:/srv/repo` | SSH | `/srv/repo` |

Git's own output paths are relative to the repository, use `/`, and are never translated. An
ignored test checks the translation against `wslpath -w` and `wslpath -u` when `wslpath` exists.

### Choosing the transport

`RepoEntry.connection` is `Auto` by default, resolved from the path when the repository is added
and again on every open:

| Path as entered | App running on | Transport |
|---|---|---|
| `C:\...` or any drive letter | Windows | Local, Windows `git.exe` |
| `\\wsl.localhost\<distro>\...` or `\\wsl$\<distro>\...` | Windows | WSL, through `wsl.exe` |
| `/mnt/<d>/...` | Linux (WSL) | Local, with Windows `git.exe` from `/mnt/<d>/Program Files/Git/bin/`, because the files are on NTFS. Falls back to Linux `git` with a warning if no Windows Git is found |
| any other `/...` | Linux | Local, Linux `git` |
| `user@host:/path`, `ssh://user@host[:port]/path`, or a `Host` alias from `~/.ssh/config` followed by `:/path` | either | SSH |

The Add Repository dialog shows the resolved transport beside the path, and its Advanced section
has a Connection list: Auto, Local, WSL (distro) and SSH (host, user, port, identity file, remote
Git). Useful overrides:

- **A WSL folder over SSH instead of `wsl.exe`.** With `sshd` running inside the distro, choosing
  SSH to `localhost` gives one lasting connection and a few milliseconds per command, instead of
  87 ms per `wsl.exe`. The library keeps the `\\wsl.localhost\...` path.
- **A Windows folder from the Linux build with Linux Git**, when there is no Windows Git. Line
  endings and file modes may then be reported differently.
- **Windows Git on a `\\wsl.localhost` path** is allowed but slow, and almost always hits
  "dubious ownership".

`RepoEntry.key` is a UUID v5 of the resolved transport and the native path (`repo::resolve::repo_key`),
so the same folder reached two ways is two library entries with two caches.

### SSH keys (`keys/`)

Any private key file can be used as it is, inspected, converted and exported from inside
StashPanda.

**Formats read**, recognised by content (`keys/detect.rs`):

| Header | Format | Read by |
|---|---|---|
| `PuTTY-User-Key-File-2:` / `-3:` | PPK v2 and v3, encrypted or not | `ssh-key` (`PrivateKey::from_ppk`) |
| `-----BEGIN OPENSSH PRIVATE KEY-----` | OpenSSH, optionally bcrypt-encrypted | `ssh-key` |
| `-----BEGIN RSA PRIVATE KEY-----`, `EC`, `DSA` | traditional PEM; `Proc-Type: 4,ENCRYPTED` means OpenSSL's legacy encryption | `russh::keys::decode_secret_key`, and `keys/pem_legacy.rs` for encrypted PEM with a cipher other than AES-128-CBC (MD5 `EVP_BytesToKey`, then the named cipher in CBC mode; RSA only) |
| `-----BEGIN PRIVATE KEY-----`, `ENCRYPTED PRIVATE KEY` | PKCS#8 | `russh::keys` |
| `ssh-rsa AAAA...`, `---- BEGIN SSH2 PUBLIC KEY ----` | public only | listed and matched to a private key |

PPK keys read by `ssh-key` give the same SHA-256 fingerprint as the same key converted to PEM or
OpenSSH, which is why no PPK parser of our own was needed.

**Formats written:** OpenSSH (with a `.pub` beside it), PKCS#8, and a public key as an
`authorized_keys` line or RFC 4716. Writing PPK is not supported.

**Settings › SSH Keys** (`KeyManagerPage.vue`) lists every key in the `.ssh` folders and
`keys.extraDirs` with its format, algorithm, fingerprint and badges. From the Windows build that
includes `\\wsl.localhost\<distro>\home\*\.ssh` for each running distro; a stopped distro is left
out, because reading its files would start it. Actions: Copy Public Key, Use for Repository,
Convert to OpenSSH (writes a new file, never overwrites), Export, Add to Agent, Change or Forget
Passphrase, Reveal in Folder, Delete; plus Generate (ed25519 by default), Import (drop, choose or
paste) and Add Folder. The Add Repository dialog's identity picker uses the same list.

**Passphrases** are asked for on first use. `keys_unlock` takes `PassphraseMemory`: `none`,
`session` (in memory, wiped on exit) or `keychain` (`keys/keychain.rs`). A passphrase the keychain
refuses is still kept for the session. Inside WSL there is usually no Secret Service, so the
dialog offers the session only and says why.

**Safety:** decrypted keys and passphrases are wiped from memory with `zeroize`; nothing is written
to disk without an explicit Convert, Export or Generate; written private keys get mode 600 on Unix;
the console logs key operations with fingerprints only.

### Read queries (`git/queries/`)

| Need | Git command (after the common flags) |
|---|---|
| Probe on open | `--version`, then one `rev-parse --is-inside-work-tree --show-toplevel --absolute-git-dir --git-common-dir --is-bare-repository`. "dubious ownership" becomes `UnsafeRepository` |
| Branches, tags, HEAD, upstreams | `for-each-ref --format=%(refname)%00%(objectname)%00%(objecttype)%00%(*objectname)%00%(upstream)%00%(upstream:track)%00%(HEAD)%00%(committerdate:unix)%00%(subject)%00%(worktreepath)`, then `rev-parse HEAD --symbolic-full-name HEAD`, `symbolic-ref -q HEAD` (for an unborn branch) and `config -z --get-regexp ^remote\..*\.(url\|pushurl)$` |
| Commits for the graph | `log --branches --tags --remotes HEAD -z --format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%s`, streamed. No `--topo-order` and no `--skip` (see "Commit graph") |
| Stashes | `stash list -z --no-show-signature --format=%H%x1f%gd%x1f%gs%x1f%ct` |
| Files changed in a commit | `diff-tree -r -z -M -C --root --no-commit-id --raw --numstat <sha>`; for a merge, against the parent the user picks |
| Working tree status | `status --porcelain=v2 -z --branch --untracked-files=all --ahead-behind` |
| Diff of one file | `diff --no-ext-diff` against the parent, the index or HEAD, with `-w` and `--ignore-cr-at-eol` as the user's toggles. An untracked file is read and shown as all added |
| File contents | the long-lived `cat-file` (below); the working tree copy through `Transport::read_file`. Over 16 MiB (`BLOB_CAP`) is refused with `tooLarge` |
| A merge, rebase, cherry-pick or revert in progress | no Git process: `git/queries/in_progress.rs` reads `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `REBASE_HEAD`, `MERGE_MSG` and the `rebase-merge` or `rebase-apply` folder through the transport |
| File history | `log --follow --cc --name-status -z`, read 500 commits at a time by asking again with a larger `--max-count` |
| Blame | `blame --porcelain`, for the working tree version or a commit's |
| One conflicted file | the three stages by their object ids from `ls-files -u`, and the working tree file with its conflict markers parsed in Rust (`conflict_file.rs`) |

**The long-lived `cat-file`** (`git/catfile.rs`): one `git cat-file --batch-command --buffer` per
repository, so reading a blob does not start a process (or open an SSH channel, or start
`wsl.exe`) each time. It asks `info` first, so a blob over the cap is refused without being read,
then `contents`. It starts on the first read, restarts once if it has died, and stops after ten
minutes unused or when the tab closes. The console shows it as one entry for its whole life, and
each request as well with `console.verboseHelpers`. It keeps the index it first read, so the
stages of a conflicted file are read by object id, not as `:1:<path>`.

### Write operations (`git/ops/`, `ipc/ops.rs`)

Every write runs as `Category::UserAction` and returns `OpResult` with Git's own stdout and
stderr, then refreshes refs, status and the graph through `refresh_if_changed`, so the interface
updates before the watcher notices. The watcher's refresh then finds nothing new and emits
nothing. Each operation has an end-to-end test against a throwaway repository.

**Locks on `RepoHandle`:**

- `lock`, a write-preferring `RwLock<()>`: reads (log, status, diff, blob, refs, polls) share it,
  and local writes take it exclusively. `read_slots`, a `Semaphore(4)`, bounds how many reads run
  at once.
- `write_gate`, a mutex every write holds for its whole run, so writes never overlap. Network
  writes (pull, push, sync) hold the gates but not the exclusive lock, so reads carry on while they
  transfer; a pull takes the exclusive lock only for its local step.
- `network_gate`, held by every network operation and by the writes that delete or rename refs,
  which lock `packed-refs` as a fetch's `--prune` can. A fetch holds only `network_gate`, so
  staging, committing and checking out go ahead during it. `network_gate` is always taken before
  `write_gate`.
- `operations`, a registry of running operations by `opId` with a cancellation token each, for
  `op_cancel`. An operation can mark itself as no longer cancellable once it reaches a step that
  must not be stopped. Tokens are children of the repository's own, so closing a tab cancels too.

A held `index.lock` is recognised as `IndexLocked` and a held `config.lock` as `ConfigLocked`; the
write waits 250 ms and tries once more, then the interface offers to remove the stale lock
(`op_remove_stale_lock`).

**How the operations run:**

- **Stage, unstage, discard:** paths go to Git on stdin (`--pathspec-from-file=- --pathspec-file-nul`)
  with `--literal-pathspecs`, so a long selection never hits the Windows command-line limit and
  names like `:x`, `*.txt` or `-f` mean exactly that file. Discard runs `restore`, then `clean -f`
  for untracked files in chunks of about 16 KB, since `clean` has no stdin form.
- **Lines and hunks** (`git/ops/lines.rs`): the frontend names the picked lines by number and
  text; the backend recomputes the diff with fixed options, refuses a selection whose text no
  longer matches, and builds a patch for `git apply` that always applies forwards to the version
  it changes.
- **Commit:** the message goes through `-F -` on stdin. With paths, they are staged and, when
  other files are staged too, the commit adds `--only` so those stay staged and out of the commit.
  Ticking exactly the staged files commits the index as it is. During a merge, cherry-pick or
  revert, the whole index is committed with Git's prepared message.
- **Pull** is two commands, so Stop only ever lands on the network transfer: `fetch --progress
  <remote> <branch>`, which can be stopped, then a local step that cannot: `merge --no-edit
  FETCH_HEAD`, `merge --ff-only FETCH_HEAD`, or `rebase --onto FETCH_HEAD <fork point>` with the
  fork point from `merge-base --fork-point`, as `git pull --rebase` does. `get_pull_mode` reads
  `branch.<name>.rebase`, `pull.rebase` and `pull.ff` in Git's order.
- **Push** names full refs on both sides (`refs/heads/<branch>:refs/heads/<remote branch>`), so a
  tag with the branch's name is never pushed instead. Without an explicit target it pushes where
  `git push` would, following `push.default`, `branch.<name>.pushRemote` and `remote.pushDefault`;
  when that would be refused, `PushDefaultDialog.vue` explains why and offers to set
  `push.default`. Push To can force, after a check.
- **Tags** are pushed only on request (`op_push_tags`): one tag from its menu, every tag from the
  Tags heading's menu, the new tag from Create Tag's "Push to origin", or, from Push To's "Also
  push its tags" (on by default), the tags on the pushed commit's history, as a step of its own
  after the branch push so a refused tag is never reported as a refused branch. Full ref names
  (`refs/tags/<name>`), never forced: a tag the remote has on another commit is refused and left
  as it is there. The one exception is Move Tag (below). Tag annotations are loaded one tag at a
  time when a pill is first hovered (`get_tag_annotation`), so the refs snapshot carries no tag
  messages.
- **Removing, renaming and moving tags** go through `RemoveTagDialog.vue`, `RenameTagDialog.vue`
  and `MoveTagDialog.vue`. Each has an unticked checkbox per remote (`RemoteTagOptions.vue`) and,
  when it opens, asks every remote in the background whether it has the tag
  (`query_remote_tag`, `git ls-remote --tags <remote> refs/tags/<name> refs/tags/<name>^{}`: the
  second pattern is needed, because a pattern matches the end of a name, so without it the
  annotated tag's commit line is left out). Until a remote answers, the dialog acts as if it has
  no such tag; the check registers an operation id so closing the dialog cancels it, and it takes
  no gate. Remove Tag deletes from the ticked remotes first (`op_delete_remote_tag`), and only
  when all of them worked deletes the tag here, so a failure leaves it here to try again.
  `op_tag_rename` creates the new name and then deletes the old one; an annotated tag is rebuilt
  from `git cat-file tag` with `--cleanup=verbatim`, `tag.gpgSign=false` and the original tagger
  and date in `GIT_COMMITTER_NAME`, `GIT_COMMITTER_EMAIL` and `GIT_COMMITTER_DATE`, so the name
  stored inside the tag object matches. On a remote, a rename pushes the new name (never forced),
  then deletes the old one. `op_tag_move` keeps an annotated tag's message with the user as
  tagger. A signature cannot survive either, so both dialogs warn about a signed tag. Moving on a
  remote is `op_push_tag_force`, with `--force-with-lease=refs/tags/<name>:<oid>` on the object
  the dialog found there ("" for none), or plain `--force` when the check had not answered. Move
  Tag's commit list (`CommitPicker.vue`) is the graph: the newest 200 rows, or `graph_search`'s
  matches fetched with `graph_rows_at`, so commits only on hidden branches are not offered.
- **Remotes** are added, edited (URL and an optional separate push URL), renamed and removed
  through `git remote` (`op_remote_add`, `op_remote_set_url`, `op_remote_rename`,
  `op_remote_remove`), from `RemoteDialog.vue`. It opens from a remote's heading in the Branches
  panel, the Remotes heading (Add Remote…), and Repository › Remotes, where the remote is picked
  in the dialog so the native menu never needs rebuilding. Renaming and removing rewrite
  remote-tracking branches and upstreams, so they wait for a running fetch. Removing confirms by
  naming the remote-tracking branches deleted and the local branches left tracking nothing.
- **Git settings** (`RepoSettingsDialog.vue`): one tab for the repository's own file and one for
  the global file of the machine whose Git runs it (that WSL distro or SSH server, not this
  computer, since that is the file that applies). `get_git_config` lists every file Git reads with
  `git config --list --show-scope --show-origin -z`, leaving out the `-c` options every StashPanda
  command carries; `op_config_change` applies Set, Replace (one value of several, with
  `--fixed-value`), Add and Unset with `--` before the key, so a value such as `--oneline` is
  kept. The common settings, their choices and explanations are data in `lib/gitSettings.ts`;
  everything else in the file is listed under Other settings, editable. Opens from Repository ›
  Repository Settings…, a repository's right-click menu in the Repositories list, and an optional
  toolbar item.
- **Upstreams are set as a separate step.** Pushing and creating a branch from a remote branch
  never touch `.git/config`; the upstream is set afterwards (`trackAfter` in `repoActions.ts`),
  because with a `config.lock` in place `git push --set-upstream` pushes, fails to record the
  upstream, and still exits 0.
- **Sync** is pull, then push, as one operation under the write gate. Nothing is pushed when the
  pull fails or stops on conflicts.
- **Clone** (`repo/clone.rs`): a missing or empty folder gets a plain `git clone --progress`. A
  folder with files in it is never moved or copied: `git init`, `git remote add origin`,
  `git fetch`, then the default branch is made current with a mixed `git reset`, which leaves
  every file alone, and `git checkout-index --stdin` writes only the files the folder lacks. On
  failure or Stop, the `.git` folder it created is deleted.
- **Conflicts:** `OperationBanner.vue` names the merge, rebase, cherry-pick or revert in progress
  and offers Continue (run with `GIT_EDITOR=true`), Skip and Abort. A conflicted file can take one
  side (`op_resolve_conflicts`), be marked resolved, or be opened in the merge editor.
  `merge.autoResolve` decides whether conflicts whose two sides changed touching but separate
  lines are resolved automatically, after a prompt, or left alone.
- **Saving a file from the merge editor:** `write_worktree_file` saves under the write gate, and
  refuses with `ChangedOnDisk` when the file's hash no longer matches the one read when the editor
  opened. `op_restore_conflict` runs `git checkout --conflict=diff3 -- <path>` to start again from
  Git's merge.
- **`safe.directory`:** `repo_fix_safe_directory` runs `git config --global --add safe.directory`
  through the library entry's own connection, with the path exactly as Git's message gave it.

### Watching for changes (`watch/`)

- **Local repositories:** `notify` with a debouncer, watching the working tree, `.git` itself
  (HEAD, index, packed-refs and the merge and rebase files) and `.git/refs`. Never
  `.git/objects`; lock files are dropped. `Access` events are dropped, and so are paths Git
  ignores, from `git ls-files --others --ignored --exclude-standard --directory`, refreshed every
  minute.
- **WSL from Windows, and SSH:** `inotifywait -m -r` runs on the system that holds the repository,
  through the repository's own transport and in a session of its own. Its `--exclude` leaves out
  Git's object store, logs, lock files and every folder Git ignores, so a `node_modules` neither
  floods it nor uses up the limit on watched folders. A repository starts by polling and switches
  to `inotifywait` once it is found. Over SSH it holds one of the connection's 8 channels.
- **Polling**, where neither works: status every `polling.statusIntervalMs` for the focused tab,
  `polling.statusBackgroundIntervalMs` for the others, four times that for a hidden window, and
  refs every `polling.refsIntervalMs`. The frontend reports `Focused`, `Background` or `Hidden`
  through `set_repo_activity`. Outputs are hashed, and only a change is reported.
- **The coalescer** turns a burst into one refresh: 300 ms of quiet, or at most 2 s of continuous
  activity. A working tree or index change refreshes status; a HEAD, ref or merge-state change
  refreshes refs and history. `repoChanged` is emitted only when something did change.
- `watcherState` reports the kind (`Native`, `RemoteInotify`, `Polling`, `Off`) and why, for
  example "Install inotify-tools there…". The status bar shows it.
- **Automatic fetch:** every half minute the frontend fetches each open repository whose last
  fetch is older than `polling.autoFetchMinutes` (`autoFetchDue` in `lib/repoActions.ts`).

### Events

| Event | Carries |
|---|---|
| `consoleEvent` | one per command start and finish (see "Running Git") |
| `repoChanged` | whether status or refs changed, and the watcher kind |
| `watcherState` | watcher kind and detail |
| `transportState` | SSH connection state, detail, time to the next attempt |
| `graphSnapshot` | a new graph version and its `GraphMeta` (head, working tree summary, ref colours, widest authors, open lanes) |
| `graphProgress`, `graphLoadFailed` | commits loaded so far; a load that failed |
| `operationProgress`, `cloneProgress` | one progress line of a running operation or clone, and whether it can still be stopped |
| `settingsChanged`, `uiStateChanged` | the whole new document and its revision |
| `appWarning` | a warning to show |
| `menuAction` | the id of the clicked native menu item, such as `repo.create_branch` |

### Errors (`error.rs`)

`GitError` is serialised as a tagged union, so TypeScript can switch on `kind`: `GitNotFound`,
`GitVersionTooOld`, `WslNotAvailable`, `WslDistroNotFound`, `SshConnectFailed`, `SshAuthFailed`
(with the methods tried, and whether a password is allowed or was refused), `SshHostKeyUnknown`,
`SshHostKeyChanged`, `SshKeyUnsupported`, `SshNeedsPassphrase`, `SshWrongPassphrase`,
`KeyParseError`, `SshUnsupportedFeature`, `TransportDisconnected`, `NotARepository`,
`RepoPathNotFound`, `UnsafeRepository`, `IndexLocked`, `ConfigLocked`, `InvalidPath`,
`ObjectNotFound`, `TooLarge`, `ChangedOnDisk`, `HeadMoved`, `PushTargetRefused`, `CommandFailed`,
`ParseError`, `Timeout`, `Cancelled`, `RepoNotOpen`, `StaleVersion`, `Io` and `Internal`.
`classify()` maps Git's stderr and exit code to a kind, by substrings such as
`not a git repository`, `dubious ownership` and `index.lock': File exists`. SSH failures do not
go through it, because they come from `russh` directly.

### Typed IPC

`tauri-specta` exports every command, event and payload type to `src/lib/bindings.ts` in debug
builds and in `cargo test export_bindings`; the file is committed. The frontend calls
`commands.getRepoStatus(key)` and `events.repoChanged.listen(...)`. Merge-patch commands
(`settings_update`, `ui_state_update`) take the patch as a JSON string, because the specta exporter
overflows its stack on the recursive `serde_json::Value`. The repository library is stored flat
(groups with `parentId`, repositories with `groupId`) for the same kind of reason: serde and the
exporter both reject an internally tagged recursive enum.

## Commit graph and cache (`graph/`)

### Order and layout are computed in Rust and never stored

- `git log` is streamed in its default order. `--topo-order` and `--skip` both make Git walk the
  whole history before printing its first line.
- Rust sorts the commits itself (`graph/order.rs`). Date order is Kahn's topological sort with a
  max-heap on commit time, ties broken by the order Git printed them, which matches
  `git log --date-order` and keeps HEAD above a sibling made in the same second. Topological order
  (`GraphOrder::Topo`) matches `git log --topo-order`: a stack instead of the heap, so a merged
  branch's commits sit together under the merge. Because Rust sorts, the cached and the live load
  give the same order, and the graph never reshuffles after the first paint.
- The lane sweep (`graph/lanes.rs`) runs down the sorted rows. Each row's lanes depend on every
  row above, so one new commit at the top can renumber every lane below; the layout is therefore
  recomputed for every snapshot (about 25 ms for 100k commits) and never written to disk.
- The whole history loads in one streamed process, capped by `cache.maxCommitsPerRepo` (200,000).

### Lane sweep rules

- An edge runs from the centre of row i to the centre of row i+1 and is stored on row i. A row is
  finished when the next one is pushed, so a lane that joins another can curve into the next dot.
- Lane slots are never compacted; a freed slot is reused. Two lanes can wait for the same commit
  and meet at its row.
- The first parent continues in the commit's own lane. Each further parent joins a lane already
  waiting for it (`MergeInto`) or starts a new lane (`Parent`). Octopus merges need no special
  case.
- Lane 0 is reserved for HEAD's first-parent line until HEAD is placed, and for the Working Tree
  row, which is always shown (with "No changes" on a clean tree) directly above HEAD.
- Lanes still waiting at the end are drawn as short fading stubs (`Dangling`).
- The sweep records, for each row, the nearest branch tip above it on its lane
  (`RowLayout::branch`).

### Colours

The palette has twelve distinct hues in both themes (violet, cyan, green, yellow, orange, magenta,
amber, blue, lime, orchid, mint, rose), from the StashPanda Design System. Each hue has the hue itself, a strong tint,
the chip fill, a subtle tint, and an ink for text on a solid pill (`index.css`, `lib/hues.ts`).

- The first lane takes the repository's own colour (`RepoEntry.colour`), and later lanes go round
  the palette from it; without one they start from cyan (`LaneSweep::with_first_colour`).
- Several branch tips on one lane: the top-most keeps the lane's colour and each tip below gets
  its own, picked to differ from the lane's and, while one is free, from every open lane's.
  `appearance.commitLaneColours` decides whether only the pills change (`keepLane`, the default)
  or the lane changes colour from that tip down (`changeAtEachTip`). Tags do not count as tips,
  and nor does a remote branch that a shown local branch tracks: it wears that branch's colour.
- Branches and tags can be given a colour from their right-click menu. It is saved per repository
  in the UI state (`perRepo[key].refColours`), never in the repository, and sent to the backend
  with `graph_set_ref_colours`. A chosen colour on the top-most tip colours the whole lane,
  including the stretch above it, so the Working Tree row takes `main`'s colour.
- While a repository's tab is active, its colour is the interface's accent: `App.vue` puts the
  hue in `data-accent` on `<html>`, and `index.css` redefines `--primary`, `--ring`, the filled
  buttons and `--row-selected` from it.

### Hidden refs

Each ref in the Branches panel has a show/hide tag. `GraphSnapshot::build` lays out only commits
reachable from the refs still shown, and HEAD counts as a tip only when detached.
`graph_set_hidden_refs` rebuilds from the commits already in memory, with no Git command. The
hidden set is saved as `perRepo[key].hiddenRefs`. The store and the cache keep every commit.

### Loading and refreshing

```
open tab   -> graph_open: more than 10,000 cached commits? paint the newest 500 rows first (2.6 ms on git.git)
           -> the whole cache -> sort -> sweep -> snapshot (reason: cache)
           -> refresh                         -> snapshot (reason: live)
graph_snapshot event -> the frontend keeps the selected and first visible commits in view
                        (or stays at the top when it was at the top), and refetches its pages
graph_rows(version, start, count) -> a page of 200 rows; StaleVersion for an old version
```

For a repository with no cache, `log::stream_log` parses `git log` as it arrives and swaps in a
`complete: false` snapshot at the first 2,000 commits and then every 500 ms, so the graph visibly
grows; on git.git the first rows show 221 ms into a 1.7 s load. With a cache, only the final
snapshot is swapped in, so nothing flickers.

**Refresh** (`graph/loader.rs`): `refresh_if_changed` reads refs and status, and when neither
changed does nothing at all. When only status changed, the snapshot is rebuilt with the new
Working Tree row and nothing is persisted. Otherwise the incremental path runs
(`incremental_store`): new tips are walked from with `git log --stdin`, tips the store already
holds and old tips that `cat-file` still finds stop the walk, and `retain_reachable_from` drops
commits that no tip reaches. It falls back to a full load for more than 20,000 new commits, any
Git failure, an incomplete snapshot, or a result over the commit cap. The toolbar's Refresh
always reads everything (`graph_refresh`, `force_full`).

**Search:** `graph_search` finds the next or previous row whose subject, author or email contains
the text, or whose id starts with it, wrapping at the ends; 3.5 to 4.7 ms on git.git. The search
box is in the Commits panel's header (`GraphSearchField.vue`).

## Storage

| File | Location | Holds |
|---|---|---|
| `settings.json` | `app_config_dir()` | preferences and the repository library: things a person might edit by hand or copy to another machine |
| `stashpanda.sqlite` | `app_data_dir()` | the commit and ref cache, and the remembered UI state. `rusqlite` with `bundled`, WAL mode, one connection behind a mutex, used only on blocking threads (`Db::run`) |

**Settings** (`settings.rs`): `git` (executables per platform, default WSL distro, automount
root), `appearance` (theme, zoom, date format, graph order, lane colours, pill styles, which
commit list columns and headings show, the graph drawn inside the message column, Markdown commit
descriptions, avatar source), `console`, `polling`, `cache`, `commit` (`preselect`), `merge`
(`autoResolve`), `diff` (font, size, line height, tab width, ligatures, word and syntax
highlighting, syntax themes for each theme, font smoothing, and the added, removed and modified
colours for each theme), `keys` (`extraDirs`), `toolbar` (the button layout, `null` while it is
the default) and `library` (groups and `RepoEntry` items). Rust owns the document:
`settings_update` applies an RFC 7396 merge patch, checks it by deserialising, bumps a revision and
emits `settingsChanged`, and the frontend replaces its copy from the event. `migrate_settings`
moves old field names (such as `appearance.diffLineHeight` to `diff.lineHeight`). A corrupt file
is renamed to `*.broken-<timestamp>` and defaults are used.

**UI state** (`ui_state.rs`): panel sizes, commit list column widths (each `None` until dragged,
meaning sized to its contents), open tabs and the active tab, the Console's buttons, the Files
panel's grouping and sorting, the diff layout, context, whitespace choices and split position,
each dialog's size and whether it is maximised (`dialogSizes`), and per repository
(`perRepo[key]`) the last selected commit, hidden refs, ref colours and pull mode. It is stored one
row per value: `ui_prefs(key, value)` with dotted keys such as `panels.leftWidth`, and
`repo_ui_prefs(repo_key, key, value)` per repository (`storage/schema_v2.sql`). Values are JSON.
`repo_ui_prefs` does not reference the cache's `repos` table, whose rows are evicted. Window
position and size are kept by `tauri-plugin-window-state`. Removing a repository from the library
deletes its rows.

The persist task writes `settings.json` atomically (a `*.tmp` file renamed over it) 500 ms after
the last change, and the UI state rows in one transaction 1 s after the last change; both are
written at once on exit.

**Cache schema** (`storage/schema.sql`, versioned with `PRAGMA user_version`):

```sql
CREATE TABLE repos   (id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, oid_len INTEGER NOT NULL DEFAULT 20,
                      commit_count INTEGER NOT NULL DEFAULT 0, last_opened_at INTEGER NOT NULL,
                      last_full_load_at INTEGER, last_refresh_at INTEGER, truncated INTEGER NOT NULL DEFAULT 0,
                      schema_version INTEGER NOT NULL);
CREATE TABLE commits (repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
                      oid BLOB NOT NULL, parents BLOB NOT NULL,          -- raw oids concatenated, n * oid_len bytes
                      author_name TEXT NOT NULL, author_email TEXT NOT NULL,
                      author_time INTEGER NOT NULL, commit_time INTEGER NOT NULL, subject TEXT NOT NULL,
                      seq INTEGER,                                        -- display position counted from the root
                      PRIMARY KEY (repo_id, oid)) WITHOUT ROWID;
CREATE INDEX commits_repo_seq ON commits(repo_id, seq DESC);              -- serves the 500-row first paint
CREATE TABLE refs    (repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
                      name TEXT NOT NULL, target BLOB NOT NULL, tag_oid BLOB, kind INTEGER NOT NULL,
                      upstream TEXT, symbolic_target TEXT, PRIMARY KEY (repo_id, name)) WITHOUT ROWID;
CREATE INDEX refs_repo_target ON refs(repo_id, target);
```

Parents are a raw byte blob: half the size of hex, no parsing beyond chunking, and no join when
loading. Refs are replaced in the same transaction as the commits, so the two always agree. `seq`
counts from the root, so the usual refresh (new commits on top) changes no existing row. The cache
never stores the Working Tree row. At start-up `repo_cache::evict` removes repositories no longer
in the library and keeps at most `cache.maxCachedRepos`, by `last_opened_at`.

## Frontend (`src/`)

Vue 3.5, Vite 8, TypeScript 6, Tailwind 4, shadcn-vue components on reka-ui (`src/components/ui/`),
Lucide icons, Pinia 4, CodeMirror 6 (merge editor only), Shiki (syntax colours), markdown-it
(commit descriptions) and Vitest. The fonts (IBM Plex Sans, Outfit, JetBrains Mono, IBM Plex Mono)
are bundled through `@fontsource`, so the app looks the same offline.

### Window layout

```
┌ AppToolbar (customisable) ─────────────────────────────────────────────────────────────────────┐
├───────────────┬─ RepoTabStrip ──────────────────────────────────────────────────────────────────┤
│ Repositories  │ OperationBanner (while a merge, rebase, cherry-pick or revert is open)          │
│ Panel         │ ┌ CommitGraphView ──────────────────────┬ FilesPanel ──────────────────────────┐ │
│ (library,     │ │ graph | message | author | date | ID  │ a commit's files, or the working tree │ │
│  groups)      │ ├───────────────────────────────────────┴──────────────────────────────────────┤ │
├───────────────┤ │ DiffViewer: unified or side by side                                          │ │
│ Branches      │ └──────────────────────────────────────────────────────────────────────────────┘ │
│ Panel         ├─ ConsolePanel (the active repository's commands) ───────────────────────────────┤
├───────────────┴─────────────────────────────────────────────────────────────────────────────────┤
│ StatusBar: watcher, connection, running operation with Stop, success messages                  │
└─────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Every divider is a reka-ui `Resizable` splitter, with its size kept in the UI state. Each open tab
has its own `RepoWorkspace.vue`, kept alive with `v-show` so scroll positions and selections
survive switching tabs; only the tab on screen loads at start-up. The native menu bar (File, Edit,
View, Repository, Help, and Developer in debug builds) is built in `menu.rs`; right-click menus
are in-app context menus, so they match the theme, show branch names and can grey an item out
with the reason.

### Stores (`src/stores/`)

- `settings.ts` and `uiState.ts` mirror the Rust documents and are replaced from their events.
  Components read and write UI state through `uiState.pref(...)`.
- `library.ts`: the Repositories tree.
- `sessions.ts`: a `shallowReactive` map of `RepoSession` objects, one per open tab, plus the
  active id. A session holds the repository's info, status, refs, graph page cache (`markRaw`,
  with a version counter, so 100k rows are never deeply reactive), selection, console buffer,
  running operation and saved scroll position. The store installs each event listener once and
  dispatches by repository, so background tabs keep receiving updates with no component mounted.
  Pinia stores are not created per repository, because dynamic stores leak unless disposed.
- `dialogs.ts`: which dialog is open. A confirmation asked for while another dialog is open shows
  on top of it, and Add Repository does not open over another dialog.
- `host.ts`: what the backend reports about the machine.

Actions that need several steps or prompts live in `lib/repoActions.ts`: one checkout path
(`requestCheckout`) for menus and double-clicks, `runOp` (with the stale lock and
`safe.directory` prompts), and fetch, pull, push and sync through `runOperation`, which keeps the
running operation on the session for the status bar and refuses a second one in the same
repository. Errors go to a red strip under the toolbar (`notify`); success messages go to the
status bar, and each clears the other.

### Commit graph (`components/graph/`)

- **Virtualisation is hand-written** (`composables/useVirtualRows.ts`): rows are a fixed 22 px
  (`--row-h`, `ROW_HEIGHT` in `lib/graph/paths.ts`), so the visible range is plain arithmetic.
  Virtual list libraries exist for rows of varying height, and they own the translated container
  the SVG overlay must share. Pages more than five from the visible rows are dropped.
- **One SVG for the visible rows** (`GraphLanesSvg.vue`), inside the same translated block as the
  row elements, so the dot for row i sits at exactly the height of its text. Straight runs of one
  lane collapse into one `<path>`. The SVG ignores the pointer; the rows beneath handle clicks.
- Ref pills (`RefPill.vue`): local branches solid and remote branches outlined, or the other way
  round with `appearance.solidRemotePills`. A local branch level with its upstream is two pills
  joined at a chevron; when they are on different commits, the local pill has a notch on its left
  and the remote pill a matching point. The checked-out branch's commit has a ringed dot.
  Double-clicking a pill checks the branch out.
- Columns: graph, commit message, author, date and ID, each of which can be hidden except the
  message. Author, date and ID size themselves to their widest possible value, measured with the
  real fonts (`lib/graph/columnWidths.ts`), until a divider is dragged. The graph column follows
  the number of lanes until dragged, and again whenever the lane count changes. With
  `appearance.graphInMessageColumn` the lanes are drawn at the start of the message column instead.
- Selection is by commit id, so it survives new snapshots, with Ctrl and Shift for several.

### Files panel and diff viewer

- **Files panel** (`components/files/FilesPanel.vue`): a commit's files or the working tree's,
  grouped by status, folder or nothing, and sorted by name, status or date modified
  (`lib/files.ts`). Multi-select with the mouse and keyboard (`composables/useListSelection.ts`,
  shared with the Commit dialog). The first file is selected when a commit is. A merge that kept
  one parent's files shows "Merged X into Y" and compares with the other parent; branch names are
  read from merge messages in the forms of the common Git hosts (`lib/mergeParents.ts`).
- **Diff viewer** (`components/diff/DiffViewer.vue`): the unified diff text is parsed in
  TypeScript (`lib/diff/parseUnified.ts`) and drawn with the same virtualisation as the graph.
  Changed words are highlighted from a small LCS on paired lines. Side by side shows each side's
  own lines only, joined across a gap by curved SVG bands (`lib/diff/sideLayout.ts`); an added or
  deleted file is always unified. Stage, Unstage and Discard act on a hunk from its header, or on
  lines picked by clicking their numbers.
- **Syntax colours:** Shiki in a Web Worker (`lib/diff/syntax.worker.ts`) with its JavaScript
  regex engine, so no WebAssembly. 62 chosen languages, each grammar loaded the first time it is
  needed. Each hunk's old and new side go to the worker as whole runs, so a comment spanning lines
  colours correctly. Diffs over 20,000 lines stay plain.
- **Find** (`composables/useFind.ts`, `FindBox.vue`, `lib/diff/find.ts`): Ctrl+F in a diff, the
  File History dialog or Blame. Past 10,000 matches the rest are neither drawn nor counted.

### Merge editor (`components/merge/MergeEditorDialog.vue`, `lib/merge/`)

Three CodeMirror 6 panes, loaded with a dynamic `import()` so the main bundle does not grow: the
branch being built on (read-only), the result (editable, saved to the working tree), and the
incoming version (read-only). CodeMirror's own two-pane merge view is not used; only `Chunk` from
`@codemirror/merge` is. Each side is named after its branch or commit (`operationNames` in
`lib/conflicts.ts`); the words "ours" and "theirs" never appear.

- The editor starts from the working tree file with its conflict markers parsed into blocks, not
  from a fresh three-way merge, so it keeps changes Git already merged, a merge driver's output,
  `rerere`'s resolutions and hand edits. All three marker styles (`merge`, `diff3`, `zdiff3`) and
  the file's `conflict-marker-size` are read.
- Curved SVG bands join each difference across the gaps, and arrows take one side's lines for a
  block; "×" dismisses a side's difference. Every action is one undoable transaction.
- The panes scroll together, kept level at the middle of the pane being scrolled
  (`lib/merge/` and `mapLine`). Unchanged runs fold into one row in all three panes. Each pane has
  marks beside its scroll bar.
- Saving keeps the file's line endings, byte order mark and final newline. A block still
  unresolved is written back with conflict markers of the file's own length, so a conflict is
  never dropped silently.
- Keys: F7 and Shift+F7 move between conflicts; Alt+Shift+Left and Right take a side; Ctrl+S
  saves.

### Other interface

- **Dialogs** (`components/ui/dialog/`): every dialog can be resized from its edges and corners and
  stays centred; the size is remembered per dialog (`size-key`), and the extra height goes to parts
  marked `data-dialog-grow`. A maximise button fills the window. Never override the shared dialog
  padding, header, footer or button sizes in a single dialog.
- **Settings dialog** (`components/settings/`): pages on the left, defined as data in
  `sections.ts`. Save Changes sends a merge patch of the changed fields only, so a setting changed
  elsewhere meanwhile is kept. The Toolbar page arranges the toolbar (`lib/toolbarItems.ts`).
- **Console** (`ConsolePanel.vue`): newest first. Log mode (the default) shows only the user's own
  commands, each expanded with its output; the User, Internal and Watcher buttons show those
  categories instead.
- **Browser behaviour off** (`lib/browserDefaults.ts`, `src-tauri/src/webview.rs`): no web view
  right-click menu outside text fields, and on Windows WebView2's own shortcuts (Ctrl+F, F5,
  Ctrl+R, Ctrl+P, F12) are switched off at start-up. Text cannot be selected except commit ids,
  the commit message, the repository path and diff code.
- **Type scale:** the root font size is 14 px, and everything is sized in `rem`, except row
  heights, which are whole pixels because the virtual lists and the graph SVG place rows by pixel
  counts.

## Pitfalls

Things that were found the hard way, each with where it matters.

### Git

- `git log --follow` prints nothing at all when given `--skip`, so file history is paged by asking
  again with a larger `--max-count`.
- `--follow` without `--cc` leaves out every merge, even one that resolved a conflict in the file;
  and `--follow` turns off the simplification that drops irrelevant merges, so merges with no
  file list must be filtered out (`parse_file_history`).
- `git rev-parse --verify` accepts one revision only.
- `git push --set-upstream` with a `config.lock` present pushes, fails to record the upstream, and
  exits 0; `git checkout -b x --track origin/x` creates `x` but neither checks it out nor tracks.
  Hence the separate upstream step.
- `git apply` accepts a patch that puts a line after one with no final newline, and joins the two
  lines into one. Line staging refuses such a selection.
- The long-lived `cat-file` keeps the index it first read; read conflict stages by object id.
- `--no-optional-locks` is needed, or `git status` rewrites `.git/index` and wakes the watcher.
- On Linux `notify` reports reads as events: `git status` and `for-each-ref` reading the
  repository caused 347 full refreshes in two minutes with nothing changing on disk, until
  `Access` events and ignored paths were dropped.

### Windows and WSL

- `powershell.exe` exits with 1 when `Get-NetTCPConnection` finds no connection, even with
  `-ErrorAction SilentlyContinue`, which `set -euo pipefail` in `scripts/windows.sh` turned into a
  silent exit. The port check judges by the output instead.
- The Rust test binaries on Windows exited with `STATUS_ENTRYPOINT_NOT_FOUND` before any test ran:
  they import `TaskDialogIndirect`, which only version 6 of `comctl32.dll` exports, and Windows
  loads version 6 only for a program whose manifest asks for it. `src-tauri/build.rs` passes
  `src-tauri/windows-app-manifest.xml` to the linker for every binary, test binaries included.
  Run the Windows tests from WSL with
  `cmd.exe /c "cd /d <STASHPANDA_WIN_DIR>\src-tauri && cargo test"` after `npm run win:sync`.
- Windows does not allow `:` or `*` in file names, so tests that use such names need another name
  there.
- Killing `wsl.exe` ends the Linux processes it started directly, but abruptly, and not ones in a
  session of their own.
- Without the Windows 11 SDK, Rust cannot find the MSVC linker and falls back to Git's `link.exe`.
- WebView2 shares its runtime between apps but gives each app its own profile folder,
  `%LOCALAPPDATA%\<identifier>\EBWebView`. Development grew it to 480 MB: 7,203 cached entries
  from the Vite dev server and their compiled code, and none from the installed app, whose files
  come through Tauri's own protocol. Debug builds therefore clear it at start-up
  (`webview::clear_dev_browsing_data`). `additionalBrowserArgs` in `tauri.conf.json` adds
  `--disable-component-update` to stop the Edge components (Widevine and others, 44 MB)
  downloading; whether WebView2 honours it is unconfirmed. Setting `additionalBrowserArgs`
  replaces wry's default `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`, so
  that switch is repeated there.

### Text rendering on Windows (WebView2)

Chromium draws ClearType text only on an opaque compositor layer. Anything that gives text a
layer of its own turns it grayscale and soft:

- `will-change: transform` on a virtual list's row container (removed from the diff and the
  graph; the diff's Grayscale font smoothing setting uses it on purpose).
- `backdrop-filter` blur on the dialog backdrop, `position: fixed` on the backdrop, the dialog and
  menu wrappers, and a box-shadow `ring` on the dialog. Dialogs and their backdrop are `absolute`
  and centred with `inset-0 m-auto h-fit` (a -50% translate put odd-sized dialogs on half
  pixels); the menu, popover and tooltip wrappers default `positionStrategy` to `absolute`; and
  `index.css` overrides the `will-change: transform` Floating UI adds at 150% scaling or more.
- A canvas: the curves in the diff and merge editor are inline SVG for this reason.
- `-webkit-font-smoothing` is ignored outside macOS. The diff's None setting uses the
  `#diff-aliased-text` SVG filter in `index.html` on the text-only `diff-glyphs` spans instead.
- Still unsolved: Add Repository's Cancel and Add buttons, which Chromium puts in a separate
  squashing layer. To inspect the real window, start it with
  `WEBVIEW_DEBUG_PORT=9222 npm run win:ux` and use DevTools protocol scripts, which are kept
  outside the repository.

### Linux (WebKitGTK)

- WebKit ignores `h-fit` on an absolutely placed `display: grid` box and stretches it to full
  height; `DialogContent.vue` is a flex column for this reason.
- WebKitGTK draws `<select>` boxes as white GTK controls unless the page declares a colour scheme;
  `index.css` sets `color-scheme` on `:root` and `.dark`.

### Build

- Importing Shiki's full language list put all 346 grammars into the build, about 10 MB more in
  the program, because Tauri builds every asset into the binary. Only 62 are imported.
- The window theme is pinned to `Dark` in `tauri.conf.json`, which is what makes `muda` draw the
  Windows menu bar dark.
- The dev server must not watch `.claude/` (`vite.config.ts`), or an agent's Git worktree there
  reloads the app.

## Tests

- **Rust:** `cd src-tauri && cargo test` runs 182 tests on Linux and Windows: path translation,
  every parser on captured Git output, ordering and lane sweep fixtures, snapshot and cache round
  trips, the incremental refresh against a full load, end-to-end runs of every query and write
  operation against throwaway repositories and remotes, stopping commands on each transport, the
  WSL batch and `cat-file` through `wsl.exe`, `inotifywait`, opening across the Windows and WSL
  boundary, the keychain against an in-memory store, the bindings export, and the SSH transport
  against a private `sshd` (host keys, passphrases, refused passwords, reconnecting). Tests that
  need WSL interop or `inotifywait` skip themselves elsewhere. Three ignored tests need this
  machine or a large repository: `cargo test -- --ignored agrees_with_wslpath`,
  `cargo test -- --ignored probe_real_keys --nocapture`, and
  `STASHPANDA_TIMING_REPO=<path> cargo test --lib time_large_repository -- --ignored --nocapture`.
- **Frontend:** `npm test` runs Vitest (pinned at 5.0.3) on the pure functions in `src/lib/`: 150
  tests covering the diff, merge, find, column width, branch tree, file grouping, toolbar and
  repository action logic. There is no DOM testing library.
- **Checks:** `npm run build` (`vue-tsc --noEmit`, then Vite) and `cargo clippy --all-targets`
  must be clean.
- **In the running app:** the Linux build can be driven invisibly through WebDriver
  (`tauri-driver` with WebKitWebDriver on an Xvfb display, with its own config folder); the
  Windows window through WebView2's DevTools protocol, as above.
