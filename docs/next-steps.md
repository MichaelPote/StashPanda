# StashPanda: what is left to build

Status as of 2026-09-30. Written on 2026-09-18, with the Windows workflow updated on 2026-09-21,
milestone 2 planned on 2026-09-23, and the commit dialog and a round of interface and graph work
added on 2026-09-30 (see "Interface and graph work" below). Everything not listed here is done and
tested; see the "Implementation notes" and "Status against the milestones" sections at the end of
[architecture.md](architecture.md) for what exists and how it deviates from the original plan.

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
- [x] **Clippy warnings** (fixed 2026-09-30). `cargo clippy --all-targets` reports none. The
      24 warnings were style points: `\0` followed by a digit in parser test fixtures (now
      `\x00`), `&PathBuf` parameters (now `&Path`), and small rewrites clippy suggested. The one
      `#[allow]` is on `Oid::len`, which is the hash length and never zero, so an `is_empty`
      would mean nothing.
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
- [x] **`win:dev` and `win:ux` exiting silently** (fixed 2026-09-30). Both stopped right after
      `npm install`. The port check `win_port_owner` in `scripts/windows.sh` asks PowerShell's
      `Get-NetTCPConnection` who holds port 1420; when nobody does, `powershell.exe` exits with 1
      even with `-ErrorAction SilentlyContinue`, and `set -euo pipefail` ended the script. The
      function now ignores PowerShell's exit code and judges by its output.
- [x] **`architecture.md` brought up to date** (2026-09-30): new implementation notes for this
      round, the status against the milestones, the test list (45 tests), the 14 px root and the
      Windows mirror in place of a separate checkout.

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
- [ ] **`lastSelectedOid` restore.** The UI state's `perRepo[key].lastSelectedOid` is written on
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

- [x] **Settings dialog** (2026-09-30, `src/components/settings/SettingsDialog.vue`, opened by
      File > Settings or Ctrl+,). A list of pages on the left (General, Commit, Diff, Git
      executables, Console, Polling, Cache) and the selected page's form on the right, with Save
      Changes and Cancel. The pages and their fields are data in `src/components/settings/sections.ts`;
      `SettingsField.vue` draws one field as a list, a checkbox, a number box with its unit, or a
      text box. Save Changes sends a merge patch holding only the changed fields, so a setting
      changed elsewhere while the dialog is open is kept, and an emptied optional box is sent as
      `null` (not set). Saving the graph order, lane colours or commits per repository refreshes
      every open tab, since the backend reads those only when it builds a graph. A page with a
      value that cannot be saved shows a warning icon in the list; a page with unsaved changes
      shows a dot. Fields that nothing reads yet are labelled "Not used yet":
      `appearance.avatarSource`, `console.verboseHelpers`, `polling.statusBackgroundIntervalMs`,
      `polling.refsIntervalMs`, `polling.autoFetchMinutes` and `cache.maxCachedRepos`. Settings
      that only show or hide something the window already has a control for are left out of the
      dialog: `appearance.showRemoteBranches` (each branch's show/hide tag in the Branches
      panel), `console.showInternal` (the Console header's Internal button, whose state is kept in
      the UI state) and `appearance.showStashesInGraph` (stashes are not drawn in the
      graph yet; its control belongs on the Branches panel's Stashes heading). Nothing reads any
      of the three, so they could be deleted from `settings.rs`. The Git
      executables page, `console.outputCapBytes` and `polling.statusIntervalMs` apply when a
      repository is opened. Still to add: the SSH Keys page below, and a way to reach the dialog
      from the toolbar.
- [x] **Console Log mode and layout** (2026-10-01, `src/components/shell/ConsolePanel.vue`).
      Commands are listed newest first, and each has one pill with its duration, green for exit
      code 0 and red with the code after a dash otherwise. A new first header button, Log (the
      default mode), switches User, Internal and Watcher off and lists only the user's own commands: each row starts with
      the git command alone, with no clock time (`ConsoleEvent.command`, from `Cmd.summary`, which `GitClient::build_cmd`
      sets to the caller's arguments before the common flags and the transport wrapper are
      added), the full command line follows in grey, there are no category tags, and each row
      starts expanded to show only its standard output. A category button pressed in Log mode
      leaves Log mode with just that category on; switching Log off turns all three back on. The
      buttons are remembered as `console.*` in the UI state (`console.log` is new). A successful
      internal or watcher command's console event carries no output text, only its byte counts
      and `outputOmitted` (`kept_output` in `src-tauri/src/git/console.rs`); nothing but the
      Console read that copy, and it cost up to 128 KiB per entry across 2,000 entries per
      repository.
- [x] **Ref pill styles** (2026-09-30, `src/components/graph/RefPill.vue`). Local branches are
      solid and remote branches outlined; `appearance.solidRemotePills` (Settings, General) swaps
      them. Tags and a detached HEAD are always solid. A local branch at parity with its upstream
      is two pills that interlock at a chevron, the remote half (`pill-head`) styled as a remote
      branch and the branch half (`pill-tail`) as a local one, overlapping by 1px. Since the
      checked-out branch no longer has its own pill style, its commit is marked by the ringed
      dot in the lane (`GraphLanesSvg.vue`) and a lighter row tint (`--row-head`) when the row
      is not selected.
- [x] **Resizable dialogs** (2026-09-30, `src/components/ui/dialog/DialogContent.vue`). Every
      dialog can be dragged bigger by its edges and corners and stays centred (the size changes by
      twice the pointer's movement); it never goes below its natural size, because the size is
      applied as `min-width` and `min-height`. Double-clicking an edge resets it. Each dialog
      passes `size-key`, and the size is remembered in the UI state as `dialogSizes.<key>`.
      While a dialog is bigger than natural (`data-resized`), the rules in `index.css` give the
      extra height to the parts marked `data-dialog-grow` (the Commit dialog's file list and
      message, the Apply Stash list, the Create Tag message, the Settings body). The Settings
      dialog opens at 80% of the window each way. Settings, Commit and Add Repository pass
      `:close-on-outside-click="false"`, so a click on the background does not close them;
      Escape and the close button still do.
- [x] **UI state in the SQLite database** (2026-09-30). `ui-state.json` and the `localStorage`
      values (Console buttons, Files panel grouping and sorting, diff header choices and divider)
      moved to the `ui_prefs` and `repo_ui_prefs` tables, one row per value
      (`src-tauri/src/storage/ui_prefs.rs`, schema version 2). The old file is imported once and
      renamed to `ui-state.json.imported`; the `localStorage` values are imported once by
      `uiState.load()` and deleted. Components read and write them through `uiState.pref(...)`.
      Removing a repository from the library now also deletes its hidden refs and last selected
      commit, which `ui-state.json` used to keep forever.
- [x] **Diff settings** (2026-09-30, the Settings dialog's Diff page, `diff` section of
      `settings.json`): font, font size (default 12px; it was fixed at 10.5px, `text-xs`, before), line height
      (moved from `appearance.diffLineHeight`; `settings::migrate_settings` moves an existing
      value when the file loads), tab width (default 8, the old browser default), ligatures (on)
      and changed-word highlighting (on). The font list offers the bundled JetBrains Mono and IBM
      Plex Mono (`@fontsource/ibm-plex-mono`) and every monospaced font installed on the machine,
      from the `fonts_monospace` command (`src-tauri/src/ipc/fonts.rs`, the `fontdb` crate, 74 ms
      for 7 fonts on this Linux machine). A saved font that is not installed stays in the list,
      marked "(not installed)". The page shows a live preview (`DiffPreview.vue`) of the unsaved
      values. `src/lib/diff/font.ts` turns the settings into the styles both the viewer and the
      preview use; the line-number gutters are now `5.5ch` and the marker `1.7ch`, so 5-digit line
      numbers fit at any font size. The font applies to the diff rows only.
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
      to edit the old message, the frontend loads it with `getCommitMessage(key, "HEAD")`. There is no
      new oid in the result: the frontend reads the new HEAD from the refs it refetches. Timeout
      10 minutes, for slow commit hooks.
      Extended on 2026-09-30 with `paths`: the paths are staged with `add -A`, and when other
      files are staged too, the commit adds `--only` (paths on the command line, at most 24 KB,
      because the message already uses stdin) so those stay staged and out of the commit. An
      empty `paths` with `amend` runs `--amend --only` with no paths, which changes only the
      message.
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

- [x] **Multi-select in the Files panel** (2026-09-30). Click, Ctrl-click, Shift-click,
      Ctrl+Shift-click, arrow keys with and without Shift and Ctrl, Home, End and Ctrl+A, from
      `src/composables/useListSelection.ts`, which the Commit dialog's list uses too. reka-ui's
      `Listbox` was checked first and not used: its mouse clicks ignore Shift and Ctrl. The diff
      shows the last file clicked, not the whole selection. Ctrl+A outside a text field never
      selects page text (`App.vue`) and selects every file in the active tab.
- [ ] **Files panel**: context menu (stage, unstage, discard, open, reveal, copy path) and the
      Stage, Unstage and Discard toolbar groups.
- [x] **Commit dialog** (2026-09-30, `src/components/dialogs/CommitDialog.vue`, opened by the
      toolbar's Commit button). Lists every outstanding file with a checkbox. The files selected
      in the Files panel start ticked; with no selection, the `commit.preselect` setting decides
      (`all`, the default, or `staged`), set on the Settings dialog's Commit page. Space
      flips the ticks of the selected rows. Ticking exactly the staged files commits the index as
      it is; any other set is committed as it is on disk, with a warning for partly staged files.
      Amend loads the previous message, and the buttons change to "Amend Message" (nothing
      ticked) or "Amend Commit" (files ticked). The message draft survives Cancel. "Commit & Push" is
      shown disabled until push exists in milestone 3. The dialog is 48rem wide with large buttons,
      and the overlay behind every dialog (`ui/dialog/DialogOverlay.vue`) is black at 50%.
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

- [ ] **Menu bar follow-ups**: the commit search behind `edit.find`. (The Settings dialog behind
      `file.settings` was added 2026-09-30.)
      (The Theme check marks now follow the setting: fixed 2026-09-30 in `menu.rs`,
      `sync_theme`.)

## Interface and graph work (done 2026-09-30)

Requested while using the Windows build, alongside milestone 2. `cargo test` (45 tests),
`cargo clippy --all-targets` and `npm run build` pass for all of it; the frontend selection,
grouping and sorting rules were checked with scratch Node scripts, since the project has no
frontend test runner.

### Commit graph

- [x] **Show and hide branches.** Each local branch, remote branch and tag in `BranchesPanel.vue`
      has an arrow-shaped tag (`shell/RefToggleTag.vue`) in its lane colour; clicking it switches
      the ref off. `GraphSnapshot::build` takes the hidden set and lays out only commits reachable
      from the refs still shown, leaving hidden refs' labels off; HEAD counts as a tip only when
      detached. `graph_set_hidden_refs` rebuilds from the commits already loaded, with no Git
      command. The list is saved in the UI state as `perRepo[key].hiddenRefs` and read by
      `repo_open`. The cache still saves every commit, from the snapshot's `full_order`.
      `GraphMeta.refColours` gives each shown ref's lane colour. The checked-out branch is marked
      with a `ChevronRight` instead of a tick. Test: `hidden_refs_drop_their_commits_and_labels`.
- [x] **Column dividers** in `GraphHeader.vue`: visible, draggable dividers between every column,
      double-click to reset. Header and rows both reserve the scrollbar's space
      (`scrollbar-gutter: stable`), so the columns line up.
- [x] **ID column** with the short commit ID, at the right-hand end.
- [x] **Columns sized to their contents.** Commit message takes the free space; Author, Date and
      ID default to their widest possible value, measured with the real fonts in
      `lib/graph/columnWidths.ts` (Author from the 24 longest names in `GraphMeta.widestAuthors`).
      A dragged width stays; `GraphColumns` fields are optional, and the old always-saved defaults
      (140, 150, 76) are read as automatic.
- [x] **Branch pills never truncate**; the subject truncates, and the cell clips at its edge.
- [x] **Dots centred on their rows.** Rows were 1.6rem (22.4px) but the graph SVG spaces rows at
      the rounded 22px, so the text drifted 0.4px a row. `main.ts` now sets `--row-h` to the
      rounded `ROW_HEIGHT`.
- [x] **Working Tree row in view on open.** When the list is scrolled to the top it stays there
      across snapshot swaps (`onGraphSnapshot` in `stores/sessions.ts`), instead of following the
      old first row down.

### Files panel

- [x] **Group by** Status, Folder or nothing, and **sort by** name, Git status or date modified,
      in the panel header (`lib/files.ts::arrangeSections`). Date modified is for the working tree
      only and uses the new `get_modified_times` command, which reads times through each
      transport's new `Transport::modified_times` (filesystem for Local and WSL, a remote `stat`
      loop for SSH). Tests cover Local and SSH.
- [x] **File name and folder as columns** when not grouped by folder: one shared name width, and a
      folder column that truncates from its start. Folder headings keep their case.
- [x] **Commit header**: regular-weight subject across the width, with a "More" toggle at the
      right, under the date, for the rest of the message.

### Diff viewer

- [x] **Side by side** splits the panel 50/50 with a draggable divider (`stashpanda.diff.split` in
      local storage). Each side scrolls sideways on its own scrollbar, and moving one moves the
      other to the same fraction of its range.
- [x] **Line height setting** `appearance.diffLineHeight`, since moved to `diff.lineHeight` (default 1.2 times the font size). The
      virtual list and the rows now use one whole-pixel height; before, the list spaced rows at
      1.4rem while they were drawn at 1.25rem, so the text jumped while scrolling.
      `useVirtualRows` accepts the row height as a getter.

### Shell, fonts and icons

- [x] **Fonts bundled**: IBM Plex Sans, Outfit and JetBrains Mono from their `@fontsource`
      packages, imported in `main.ts`, instead of a Google Fonts `@import`, so the app looks the
      same offline.
- [x] **Theme menu ticks** follow the setting (`menu.rs::sync_theme`).
- [x] **Active tab** keeps the same box as the others, so it no longer moves, and is lit in the
      accent colour.
- [x] **Keyboard focus** is a 1px accent outline instead of the browser's white ring
      (`index.css`).
- [x] **Lucide icons** draw 1px lines at any size, and the toolbar icons are 0.5rem larger.
- [x] **Console** scrolls sideways for long command lines; expanded output stays panel-wide.

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
- [x] **Console filter buttons remembered** (2026-09-30). The User, Internal and Watcher buttons
      are kept in the UI state (`console.*` rows in the database), not in a setting; the unused
      `console.showInternal` setting is left out of the Settings dialog.
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
- The `@fontsource` stylesheets list a `.woff` copy of every `.woff2` file, so about 0.9 MB of the
  1.7 MB of bundled fonts is never used (WebView2 and WebKitGTK both read `.woff2`). Drop the
  `.woff` files in a build step if the installer size matters.
- `perRepo[key].hiddenRefs` keeps the names of branches that were hidden and later deleted.
  Harmless, since a missing ref hides nothing, but the list is never pruned.
- The side-by-side diff sizes each horizontal scroll range from a character count (tabs counted as
  8 columns), so lines with unusually wide or narrow characters can leave it slightly long or
  short.
- `get_modified_times` over SSH reports whole seconds; Local and WSL report milliseconds. Files
  changed within the same second can sort in either order.
