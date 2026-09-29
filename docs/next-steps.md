# StashPanda: what is left to build

Status as of 2026-09-18, with the Windows workflow updated on 2026-09-21 and milestone 2 planned
on 2026-09-23. Everything not listed
here is done and tested; see the "Implementation
notes" and "Status against the milestones" sections at the end of [architecture.md](architecture.md)
for what exists and how it deviates from the original plan.

Items are grouped by milestone and ordered within each group by the order they should be built.
Each item names the files it touches so it can be picked up cold.

## Housekeeping (do first)

- [x] **First commit** (2026-09-23), pushed to `git@github.com:MichaelPote/stashpanda.git`.
- [x] **Seeded config removed** (2026-09-23). The Linux build's library in
      `~/.config/com.bravedigital.stashpanda/settings.json` held two hand-written entries with keys
      `seed-stashpanda` and `seed-other-project`, which `repo_add` could not match against the UUID v5
      key it computes, so re-adding either would have created a duplicate. Both entries and their
      open tabs in `ui-state.json` are gone. The Windows build keeps its own settings in
      `%APPDATA%\com.bravedigital.stashpanda\` and was already clean.
- [ ] **Clippy warnings.** Clippy was first installed on 2026-09-23 (`rustup component add clippy`);
      `cargo clippy --all-targets` reports 22 warnings, all in code written before then (octal-looking
      `\0` escapes in parser test fixtures, `&PathBuf` parameters, and similar). One small cleanup
      commit.
- [x] **Windows toolchain and workflow** (done 2026-09-21). Windows has rustup with the
      `stable-x86_64-pc-windows-msvc` toolchain, Node 24, Visual Studio 18 Community with the
      "Desktop development with C++" workload plus the Windows 11 SDK 10.0.26100 (the SDK was the
      missing piece: without it Rust cannot find the MSVC linker and falls back to Git's
      `link.exe`), and the WebView2 runtime. The Windows build does **not** use a separate
      checkout. `scripts/windows.sh` mirrors `/home/me/code/stashpanda` to `C:\src\stashpanda` with
      `rsync` (leaving out `node_modules`, `dist`, `src-tauri/target` and `.git`) and runs the
      Windows toolchain there through WSL interop. Run these from a WSL shell:

      | Command | What it does |
      |---|---|
      | `npm run win:ux` | Vite runs in WSL; the Windows app loads its frontend from it, so Vue, TypeScript and CSS edits hot reload. Rust changes need Ctrl-C and a re-run. Ctrl-C stops both sides. |
      | `npm run win:dev` | Runs `tauri dev` entirely on Windows, including its own Vite. Ctrl-C stops the whole Windows process tree; before starting, it refuses to run if a leftover process holds port 1420 on Windows, and names it. |
      | `npm run win:build` | Runs `tauri build` on Windows, then copies the release binary and the NSIS and MSI installers to `bundle/` in both the WSL checkout (ignored by Git) and `C:\src\stashpanda`. |
      | `npm run win:sync` | Mirrors the source and runs `npm install` on Windows, nothing else. |

      The mirror is one-way: edit in WSL, never in `C:\src\stashpanda`. `win:ux` merges
      `src-tauri/tauri.wsl-dev.conf.json` into the Tauri config to empty `beforeDevCommand`, and
      relies on WSL2 forwarding Windows `localhost:1420` to the Vite server in WSL. Details are in
      the README under "Working on the Windows version from WSL".

## Milestone 1 remainder (read-only browsing)

### Backend

- [ ] **Step 14: test the WSL transport from the Windows build.** `src-tauri/src/transport/wsl.rs`
      is written but has only unit tests for UTF-16 decoding. Start the Windows build with
      `npm run win:ux` from WSL, then in the app open
      `\\wsl.localhost\Ubuntu\home\me\code\stashpanda`, confirm the console shows
      `wsl.exe -d Ubuntu -e env ... git -C /home/me/code/stashpanda ...`, confirm `wsl_list_distros`
      returns `Ubuntu`, and verify the argument self-test the plan calls for (an argument with a
      space and a quote through `wsl.exe -e`) with a fallback to `-e sh -c '<quoted>'` if `wsl.exe`
      re-splits it. The remote-pid `sh -c 'echo $$; exec "$@"'` wrapper for cancellable commands is
      not implemented; `Process::kill` on the WSL transport only kills `wsl.exe`.
- [ ] **Windows git from Linux for `/mnt/<d>/` paths.** `repo/resolve.rs` builds a `LocalTransport`
      with `windows_git_from_linux = true` and a `WindowsFromLinuxMapper`; untested against a real
      `/mnt/c/...` repository. Open one from the Linux build and check the console shows `C:\...`
      paths and that `dubious ownership` is handled (`fix_safe_directory` command does not exist yet).
- [ ] **Step 15: `inotifywait` over SSH and WSL** (`src-tauri/src/watch/remote.rs`, new). Probe
      `command -v inotifywait` on the target; stream
      `inotifywait -m -r -q -e modify,close_write,create,delete,move,attrib --exclude '(/\.git/(objects|lfs|logs)/|\.lock$)' --format '%w%f|%e' <worktree> <gitdir>`
      through the transport with `Program::Other`; map lines with the same `classify_git_dir_path`
      the native watcher uses; downgrade to polling on exit and emit `watcher_state`. Today SSH and
      WSL repositories poll every `polling.status_interval_ms` (3 s) via `watch/polling.rs`.
- [ ] **Watcher activity levels.** `Watcher::set_activity(Focused | Background | Hidden)` from the
      plan is not implemented; polling uses one interval. Add a `set_repo_activity(key, activity)`
      command called from the frontend on window focus/blur and tab visibility, and honour
      `polling.status_background_interval_ms`.
- [ ] **Step 11: pinned `cat-file --batch-command --buffer` helper** (`src-tauri/src/git/catfile.rs`,
      new). `queries/blob.rs::get_blob` spawns `git cat-file blob <spec>` per request; over SSH or
      `wsl.exe` that is one round trip or one process per file. Keep one long-lived process (or SSH
      channel) per repository behind an async mutex: `info <spec>` first to refuse anything over
      `BLOB_CAP`, then `contents <spec>`, then `flush`; respawn lazily if it dies; shut down after
      10 minutes idle. Log spawn and exit as `Internal`; log per-request traffic only when
      `console.verbose_helpers` is on.
- [ ] **Step 17: `run_batch` for `wsl.exe`.** `Transport::run_batch` does not exist. For the WSL
      transport combine the refresh's `for-each-ref`, `status` and `stash list` into one
      `wsl.exe -e sh -c '...'` spawn with NUL separators between outputs. Not needed for SSH
      (channels are cheap) or Local.
- [ ] **Incremental graph refresh** (`graph/loader.rs::refresh_full`). Every refresh runs a full
      `git log`. Implement the plan's incremental path: diff the new `RefsSnapshot` against the
      snapshot's refs, feed new or moved tips as positives and unchanged tips plus the old oids of
      moved refs (checked with `cat-file --batch-check` or the pinned helper) as negatives to
      `queries/log.rs::get_log_between` (already written), insert into a cloned `CommitStore`,
      `retain_reachable_from(new tips)`, fall back to full when more than 20,000 commits arrive or
      the incremental log fails. Persist deletions (the cache `persist` already deletes oids missing
      from the store).
- [ ] **Cache eviction at startup.** `storage/repo_cache.rs::evict` exists and is tested but nothing
      calls it. Call it from `AppState::init` (or the first `graph_open`) with the library keys and
      `cache.max_cached_repos`, and remove cache rows when `repo_remove` runs.
- [ ] **500-row fast first paint.** `load_from_cache` loads the whole cached store and rebuilds the
      snapshot. For very large repositories add the plan's first page (`ORDER BY seq DESC LIMIT 500`,
      partial sweep, `complete: false`) followed by the full load. Measure a 100k-commit clone first;
      only build this if the whole-store load exceeds roughly 200 ms.
- [ ] **Partial snapshots while a live load streams.** `get_log` collects all output before parsing.
      For a repository with no cache the user sees nothing until Git finishes. Stream stdout through
      `Transport::spawn`, parse NUL records as they arrive, emit `graph_progress` every 2,000 records
      and swap a `complete: false` snapshot every 500 ms.
- **`fix_safe_directory` command:** moved to milestone 2, step 2e.
- [ ] **`GraphOrder::Topo`.** `graph/order.rs` treats `Topo` the same as `Date`. Implement the
      grouped tie-break (keep a branch's commits contiguous) or drop the setting.
- [ ] **Text filter on the graph** (`graph_search` command). Planned, not started: return the next
      row index whose subject, author or oid matches, in a direction, for the Ctrl+F box.
- [ ] **`lastSelectedOid` restore.** `ui-state.json` `perRepo[key].lastSelectedOid` is written on
      every selection but never read back. Decide whether to select and scroll to it on the first
      snapshot after open, then do it in `stores/sessions.ts`.
- [ ] **Passphrase storage in the OS keychain.** `keys/secrets.rs::PassphraseCache` is
      session-only. The plan names the `keyring` crate (Windows Credential Manager, Secret Service);
      add an opt-in "store in system keychain" path with a session-only fallback where no backend
      exists (likely inside WSL).
- [ ] **`eol` on `DiffText`.** The diff toolbar has no line-ending badge because `get_file_diff`
      returns no EOL information. Either detect it from the diff text in Rust or fetch it from the
      blob query when a file is selected.
- [ ] **Windows `.ssh` folders from the Windows build.** `keys::default_key_dirs` scans the platform
      home and, from WSL, `/mnt/c/Users/*/.ssh`. From Windows it should also scan
      `\\wsl.localhost\<distro>\home\*\.ssh` for each distro `wsl_list_distros` returns.
- [ ] **PPK export.** `keys::export_private` writes OpenSSH and PKCS#8 only; the `ssh-key` crate
      reads PPK but does not write it. Add a PPK v3 writer only if a tool that still needs PPK
      turns up.
- [ ] **Password authentication prompt flow.** `ssh_set_password` stores a password for
      `user@host:port`; the open then has to be retried by the caller. Consider retrying inside
      `repo_open` once a password arrives.

### Frontend

- [ ] **Settings dialog** (`src/components/settings/SettingsDialog.vue`, new) with pages General
      (theme, date format, graph order, show remote branches, show stashes), Git executables
      (`git.windowsExecutable`, `git.linuxExecutable`, `git.windowsExecutableFromLinux`,
      `git.wslDefaultDistro`, `git.wslAutomountRoot`), Console (`console.showInternal`,
      `console.verboseHelpers`, `console.maxLines`), Polling, Cache. All fields exist in
      `Settings` (see `bindings.ts`) and save through `settings.update(patch)`. Nothing in the UI
      edits settings today.
- [ ] **SSH Keys page** (`src/components/settings/KeyManagerPage.vue`, `KeyRow.vue`,
      `KeyImportDropzone.vue`, `PassphraseDialog.vue`, new). Commands exist and are typed:
      `keysDefaultDirs`, `keysScan(extraDirs)`, `keysInspect`, `keysUnlock(path, passphrase, remember)`,
      `keysForgetPassphrase`, `keysExportPublic`, `keysConvert(path, format, outPath, passphrase, newPassphrase, overwrite)`,
      `keysGenerate(algorithm, comment, outPath, passphrase, overwrite)`, `keysChangePassphrase`,
      `keysImport(text, outPath, passphrase, newPassphrase, overwrite)`, `keysAgentList`,
      `keysAgentAdd(path, passphrase, lifetimeSecs)`. Show name, format badge (PPK 2, PPK 3, PEM,
      OpenSSH, PKCS#8), algorithm and bits, SHA-256 fingerprint, encrypted and in-agent badges,
      matching `.pub`; actions Use for repo, Convert to OpenSSH, Export, Copy public key, Add to
      agent, Change passphrase, Reveal in folder, Delete; drag-and-drop or paste import; Generate.
- [ ] **Identity picker in the Add Repository dialog** (`src/components/shell/AddRepoDialog.vue`).
      The SSH advanced section has a plain text field for the identity file; replace it with a
      picker fed by `keysScan`, and prompt for a passphrase through `keysUnlock` when the open
      returns `sshNeedsPassphrase`.
- [ ] **Host key trust dialog.** When `repoOpen` fails with `sshHostKeyUnknown { host, fingerprint }`
      show the fingerprint with Trust / Cancel; Trust calls `sshTrustHost(host, port)` and retries
      the open. `sshHostKeyChanged` gets a hard error dialog with no override.
- [ ] **Password dialog.** On `sshAuthFailed` offer a password field; call
      `sshSetPassword(host, port, user, password)` and retry the open.
- [ ] **Stash list in the Branches panel** (built in milestone 2, step 3). `getRepoStashes(key)` now exists and returns
      `StashEntry { oid, selector, message, time, index }`; `BranchesPanel.vue` shows only the
      count from `status.stashCount` with a `TODO(backend)` comment.
- [ ] **Watcher hint.** `watcherStateEvent.detail` (for example "filesystem notifications are not
      available for this connection; polling instead") is stored in the session but only the kind is
      shown in the status bar. Surface the detail as a tooltip or an info strip.
- [ ] **Startup warnings and app warnings** are shown as strips; check they also cover
      `RepoInfo.warnings` (Windows git over UNC, Linux git for a Windows folder) after a repo opens.
- [ ] **Page cache eviction.** `session.graph.pages` is never evicted until the next snapshot;
      scrolling a 100k-commit history keeps every fetched page. Drop pages far from the viewport.
- [ ] **Graph text filter box** (Ctrl+F) once `graph_search` exists.
- [ ] **Syntax highlighting in the diff viewer** (Shiki in a Web Worker was the candidate). Planned
      for milestone 3; listed here because the diff viewer is otherwise complete.
- [ ] **Gravatar opt-in** for the author cell (`appearance.avatarSource` exists in settings).

### Verification still owed for milestone 1

- [ ] Smooth scrolling on a 100k-commit clone (for example the `git` or `linux` repository): first
      paint from cache, page fetch latency, SVG repaint cost. Not yet measured on anything larger
      than 370 commits.
- [x] Manual check of the rendered UI (2026-09-23): Michael looked at the running app and was
      happy with it.
- [ ] Cross-side runs from the Windows build (`npm run win:ux`; see step 14 above) and against a real Brave Digital
      host over SSH (only a private `sshd` on localhost has been tested).

## Milestone 2: local write operations

Planned 2026-09-23 as the current phase: finish milestone 2 so the app can stage, commit and stash.
The rest of milestone 1 (WSL, `inotifywait`, performance) is a separate phase.

Every operation goes through `GitClient::exec_allow_failure` with `Category::UserAction`, runs
inside the `write_op!` macro in `ipc/ops.rs` (exclusive lock, `index.lock` retry, then
`refresh_if_changed`), and returns `OpResult` with Git's own stdout and stderr. Each new operation
gets an end-to-end test against a throwaway repository in the `e2e` module of `git/ops/mod.rs`.

- [x] **Operations started** (2026-09-22): `checkout`, `branch_create`, `branch_delete`, `merge`,
      `merge_abort`, `tag_create`, `tag_delete` (commands `op_*` in `ipc/ops.rs`, driven by the
      commit and branch context menus and the Repository menu).
- [x] **Menus** (2026-09-22): native menu bar (File, Edit, View, Repository, Help) forwarding
      `menu_action` ids to the frontend; in-app context menus on commits (checkout, create branch
      here, merge into current, add tag, remove tag, copy SHA and message), on ref pills and on
      branches (checkout, merge, delete).

### Step 1: shared plumbing (done 2026-09-23)

- [x] **One refresh per write.** `graph/loader.rs::refresh_if_changed` reads refs and status and
      rebuilds only what differs from the current snapshot; when both match it does not run
      `git log`, emit `graph_snapshot` or persist. `after_write` in `ipc/ops.rs` and the coalescer
      both use it, and the coalescer only emits `repo_changed` when something changed. The
      watcher's follow-up to the app's own write therefore costs one `for-each-ref` and one
      `status`. `graph_refresh` (the manual refresh) still forces a full rebuild. Not yet watched
      in the running app's console.
- [x] **`IndexLocked` retry.** `write_op!` retries once after 250 ms when `ops::index_lock_in`
      recognises Git's `index.lock` message, then returns `GitError::IndexLocked`. Before this the
      lock only showed as a generic non-zero exit. `op_remove_stale_lock(key)` deletes
      `<git dir>/index.lock` through the new `Transport::remove_file` (`tokio::fs` for Local and
      WSL, `rm -f` over SSH); a missing file counts as success.
- **Put off to milestone 3:** `OperationRegistry` and `start_operation` / `cancel_operation`.
  Nothing in milestone 2 streams progress, and fetch, pull and push will show what the registry
  needs.

### Step 2: backend operations, in the order the UI needs them

- [x] **a. `stage`, `unstage`, `discard`** (2026-09-23; commands `op_stage`, `op_unstage`,
      `op_discard`). Paths go to Git on stdin with `--pathspec-from-file=- --pathspec-file-nul`
      and `--literal-pathspecs`, so a long selection never hits the Windows command-line limit and
      names like `:x`, `*.txt` or `-f` mean exactly that file. `stage` runs `add -A`; `unstage`
      runs `restore --staged`, or `rm --cached` on an unborn branch; `discard(tracked, untracked,
      to_head)` runs `restore --worktree` (back to the index) or
      `restore --source=HEAD --staged --worktree` (back to HEAD, which also deletes a staged file
      that is not in HEAD), then `clean -f` for untracked paths in chunks of about 16 KB, since
      `clean` has no stdin form. For a rename, the frontend passes both the new and the original
      path.
- [x] **b. `commit(message, amend, sign_off)`** (2026-09-23; command `op_commit`). The message goes
      through `-F -` on stdin and arrives intact (tested with a leading dash, `#123` and non-ASCII
      text). Git itself refuses an empty message. `amend` with an empty message uses `--no-edit`;
      to edit the old message, the frontend loads it with `getCommitBody(key, "HEAD")`. There is no
      new oid in the result: the frontend reads the new HEAD from the refs it refetches. Timeout
      10 minutes, for slow commit hooks.
- [ ] **c. `stash_save(message, include_untracked)`, `stash_apply(index, pop)`, `stash_drop(index)`**.
      The `getRepoStashes` query already exists.
- [ ] **d. `branch_rename`, `reset(mode, target)`, `cherry_pick`, `revert`**. Cherry-pick and
      revert reuse `OpResult.conflicts` and get an abort; full conflict resolution stays in
      milestone 3.
- [ ] **e. `fix_safe_directory`** (moved here from milestone 1): on `UnsafeRepository`, run
      `git config --global --add safe.directory <path>` through the repository's own transport, so
      the right Git gets the setting.
- [ ] **f. Hunk and line staging**: `apply --cached` with a patch built in Rust from the diff the
      viewer already has, `--unidiff-zero` for single lines. Largest and riskiest item, so last.
      Test against no newline at end of file, CRLF, and a new file.

### Step 3: frontend

- [ ] **Files panel**: context menu (stage, unstage, discard, open, reveal, copy path) and the
      Stage, Unstage and Discard toolbar groups.
- [ ] **Commit message editor** with amend (loads the previous message). "Commit and push later"
      waits for milestone 3, since push does not exist yet.
- [ ] **Stash list in `BranchesPanel.vue`** replacing the `TODO(backend)` comment, with apply, pop
      and drop, plus the Save Stash and Apply Stash toolbar buttons.
- [ ] **Commit context menu additions**: reset (soft, mixed or hard), cherry-pick, revert.
- [ ] **Stale lock and `safe.directory` prompts** driven by `IndexLocked` and `UnsafeRepository`:
      confirm, then call `opRemoveStaleLock` or the `fix_safe_directory` command and retry.
- [ ] **Hunk and line staging buttons** in `DiffViewer.vue`, once step 2f is done.

### Done when

`cargo test`, `cargo clippy --all-targets` and `npm run build` pass, and in the running app on a
real repository you can stage part of a file, commit, amend, stash and pop, and reset, with one
refresh per action in the console and the result matching the `git` command line.

### Left for the phase after this one

- [ ] **Menu bar follow-ups**: a real Settings dialog behind `file.settings`, the commit search
      behind `edit.find`, and syncing the Theme check marks with the setting (the native
      `CheckMenuItem`s are created checked for Dark and never updated). The Settings dialog comes
      first, since three menu items wait on it.

## Milestone 3: remote and history operations

- [ ] **Fetch, pull, push** with streamed progress: `--progress` on stderr, lines split on `\r`
      and `\n`, emitted as `operation_progress`; `operation_finished` at the end; cancel through
      `Process::kill` (local) or `Sig::TERM` on the SSH channel. Credential prompts: we run with
      `GIT_TERMINAL_PROMPT=0`; HTTPS remotes need a `GIT_ASKPASS` helper or the user's credential
      manager, to be designed.
- [ ] **Merge, rebase, cherry-pick, revert** with conflict detection (`u` entries in status), a
      conflict list using the `:1:`, `:2:`, `:3:` blob stages (`get_blob_content` already accepts
      those specs), and continue / abort actions.
- [ ] **SSH tier 2 fallback** (`transport/ssh_exec.rs`): long-lived `ssh -T host sh` helper with
      sentinel framing, only if a real host needs `ProxyJump`, `ProxyCommand`, GSSAPI, certificates
      or an algorithm `russh` cannot negotiate. `resolve_target` currently returns
      `SshUnsupportedFeature` for ProxyCommand and ProxyJump.
- [ ] **Native `ProxyJump`** by nesting a `russh` connection over a `direct-tcpip` channel, as an
      alternative to tier 2 for the jump-host case.
- [ ] **Console preference `console.hideInternal`** wired to the default state of the console
      filter chips (the setting `console.showInternal` exists; the chips currently persist in
      `localStorage` only).
- [ ] **Blame and file history** views (the toolbar has Blame and Investigate buttons; neither
      is planned in detail yet).
- [ ] **Auto-fetch** on `polling.autoFetchMinutes`.
- [ ] **Reconnect with backoff** for SSH: the pool reconnects lazily when a connection is found
      closed, but there is no `transport_state` event yet (Connecting, Ready, Degraded,
      Disconnected) and no backoff schedule.

## Packaging and CI

- [ ] **Windows installer** through `npm run win:build` from WSL, which runs `npm run tauri build`
      in the Windows mirror (NSIS and MSI targets are enabled in `tauri.conf.json` by
      `"targets": "all"`); an icon set beyond the Tauri defaults.
- [ ] **GitHub Actions** matrix with `tauri-apps/tauri-action` for `windows-latest` and
      `ubuntu-24.04`, running `cargo test`, `npx vue-tsc --noEmit` and `npm run build` first.
- [ ] **Release profile check**: `[profile.release]` in `Cargo.toml` uses `lto = true`,
      `codegen-units = 1`, `panic = "abort"`, `strip = true`; confirm the release binary still
      starts on both platforms and measure start-up time.
- [ ] **Bindings drift guard**: `src/lib/bindings.ts` is regenerated by `cargo test export_bindings`
      and committed. Add a CI step that regenerates it and fails if `git diff` is non-empty.

## Known rough edges to revisit

- `GitError::CommandFailed { argv }` is empty (`vec![]`) in several query modules because the argv
  is only rendered inside `GitClient::exec`; pass it through or drop the field.
- `repo_add` stores `resolvedTransport`, and `repo_open` updates it; `repo_resolve` recomputes it
  every time. A repository whose `Auto` resolution changes between opens (a distro renamed, a
  Windows git installed) silently changes its cache key. Decide whether the key should follow the
  path only.
- The status-only refresh (`refresh_status_only`) does not persist to the cache; only full
  refreshes do. Harmless, since the cache never stores the Working Tree row.
- `graph_rows` caps `count` at 1000 per call; the frontend uses 200. Fine, but undocumented in the
  bindings.
- The `WslTransport::probe` treats exit code 255 or -1 as "distro missing" only when the message
  matches; `wsl.exe` error text varies by version and language.
- `Process::kill` on the WSL transport kills `wsl.exe`, not the Linux child (see step 14).
- `keys_convert` writes `<stem>` for OpenSSH output, which for a file named `foo.ppk` produces `foo`
  next to it; for a file with no extension (already OpenSSH) it refuses unless `overwrite` is set.
  Confirm this is the naming Michael wants.
