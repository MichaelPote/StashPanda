# StashPanda: what is left to build

Status as of 2026-10-14. Written on 2026-09-18, with the Windows workflow updated on 2026-09-21,
milestone 2 planned on 2026-09-23, the commit dialog and a round of interface and graph work
added on 2026-09-30, and stashes, a second round of interface work and pull added on 2026-10-01
(see "Interface, merge and checkout work" and "Milestone 3" below), and the first run in the real
app plus the Windows test fix on 2026-10-14 (see "Checked in the running app"), the rest of
milestone 2 the same day, the rest of milestone 1 after it, and most of milestone 3 after that
(conflicts, merge and rebase, file history and blame, automatic fetch, SSH reconnect, syntax
colours, Find in diffs and Blame, and the three-pane merge editor). Left in milestone 3: a few
parts of the merge editor, and the SSH fallback and `ProxyJump`. Everything not listed here is
done and tested; see
the "Implementation notes" and "Status against the milestones" sections at the end of
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
      | `npm run win:ux` | Vite runs in WSL; the Windows app loads its frontend from it, so Vue, TypeScript and CSS edits hot reload. Every 2 seconds it copies `src-tauri/` to the Windows mirror (leaving out `target` and `gen`), so the Windows `tauri dev` rebuilds on Rust changes. Ctrl-C stops both sides. No empty console window opens on Windows. |
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

- [x] **Step 14: the WSL transport from the Windows build** (2026-10-14). The test
      `windows_opens_a_wsl_repository_through_its_unc_path` (`repo/cross_e2e.rs`, Windows only)
      makes a repository inside WSL, opens it through `\\wsl.localhost\<distro>\tmp\…` with
      `Connection::Auto` the way the Windows build does, and checks that the WSL transport was
      chosen, then runs status, refs, a commit of a file with a space in its name, a file read, and
      `rev-parse --sq-quote` on an argument with a space and both quotes. That argument arrives
      unsplit through `wsl.exe -e`, so no `sh -c` fallback is needed. Run on Windows from the mirror.
- [x] **Windows Git for `/mnt/<d>/` paths** (2026-10-14). The test
      `wsl_opens_a_windows_folder_with_windows_git` makes a repository in the Windows `%TEMP%`
      folder and opens it from the Linux build: it runs
      `/mnt/c/Program Files/Git/bin/git.exe -C C:\Users\…`, and status, refs and a commit work.
      No "dubious ownership" came up there; where it does, the "Trust this folder…" notice from
      milestone 2 handles it.
- [x] **Step 15: `inotifywait` over SSH and WSL** (2026-10-14, `watch/remote.rs`). A remote
      repository starts by polling; meanwhile the app checks for `inotifywait` on the other system
      and, when it is there, runs `inotifywait -m -r` through the repository's own transport (in a
      session of its own, so stopping reaches it) and swaps it in. Its `--exclude` leaves out Git's
      object store, logs and lock files and every folder Git ignores, so a `node_modules` neither
      floods it nor uses up the limit on watched folders. Lines map to the native watcher's events.
      Without `inotifywait`, or when it stops, polling takes over and `watcher_state.detail` says
      why ("Install inotify-tools there…"). Over SSH it holds one of the connection's eight
      channels for as long as the repository is open. Tests: `watches_through_the_local_and_wsl_transports`
      (an ignored write is not reported, an edit and a commit are, stopping ends the process).
      Tried in the running Linux build over SSH to a private `sshd`: a change on the server showed
      in 0.4 s, against 2.3 s when polling.
- [x] **Watcher activity levels** (2026-10-14). `set_repo_activity(key, activity)` with
      `Focused`, `Background` or `Hidden`, sent by `stores/sessions.ts` when the active tab, window
      focus or page visibility changes. Polling checks status every `polling.statusIntervalMs`,
      `statusBackgroundIntervalMs`, or four times that for a hidden tab, and checks at once when the
      tab becomes the focused one again; it checks the refs at `polling.refsIntervalMs`. Watchers
      that are told about changes ignore the activity.
- [x] **Step 11: one long-lived `cat-file`** (2026-10-14, `git/catfile.rs`, `RepoHandle::catfile`).
      Blob reads (`get_blob_content`) go through one `git cat-file --batch-command --buffer` per
      repository when Git is 2.36 or newer: `info` first, so a blob over `BLOB_CAP` is refused
      with `tooLarge` without being read, then `contents`. It starts on the first read, starts again
      once if it has died, and stops after ten minutes unused or when the tab closes. The console
      shows it as one entry for its lifetime, and each request too with `console.verboseHelpers`.
      The incremental graph refresh also uses it (`CatFile::missing`). Test:
      `reads_blobs_through_one_process`, through the local transport and `wsl.exe`.
- [x] **Step 17: `run_batch` for WSL** (2026-10-14). `Transport::run_batch` runs several
      commands; the WSL transport runs them in one `wsl.exe -e sh -c` script that prints each
      command's exit code and output lengths, then the bytes (lengths, because `status -z` output
      has NUL bytes). `queries::refs_and_status` uses it for a refresh's `for-each-ref`,
      `rev-parse HEAD`, `symbolic-ref`, remotes and `status`: one `wsl.exe` start instead of five.
      Other transports keep running `get_refs` and `get_status` side by side. Tests:
      `batch_matches_separate_runs`, `refs_and_status_agree_between_batch_and_separate_runs`.
- [x] **Incremental graph refresh** (2026-10-14, `incremental_store` in `graph/loader.rs`).
      New tips the store lacks are walked from; tips it already holds, and old tips that
      `cat-file` still finds, stop the walk; `retain_reachable_from` then drops what no tip reaches,
      and the cache deletes those rows. Unchanged tips reuse the store without calling Git. It falls
      back to a full load for more than 20,000 new commits, any Git failure, an incomplete snapshot,
      or a result over `cache.maxCommitsPerRepo`. The toolbar's Refresh still reads everything
      (`force_full`). On git.git (85,563 commits) a refresh after one new commit takes 299 ms in a
      release build, against about 1.7 s before. Test: `incremental_refresh_matches_a_full_load`
      (new commits, a moved branch, a force-push with pruned commits, a deleted branch, the
      fallbacks), comparing rows and cache with a fresh full load.
- [x] **Cache eviction** (2026-10-14). `AppState::init` calls `repo_cache::evict` with the library
      keys and `cache.maxCachedRepos`; `repo_remove` deletes the repository's rows
      (`repo_cache::forget`). Rows under a key that `repo_update` replaced stay until the next start.
- [x] **500-row first paint** (2026-10-14). Measured on git.git: loading the whole cache took 260 ms
      in a release build (1,231 ms in debug), over the 200 ms threshold, so
      `repo_cache::load_first_page` paints the newest 500 rows first (2.6 ms) when the cache holds
      more than 10,000 commits, then swaps in the whole store.
- [x] **Partial snapshots while a load streams** (2026-10-14). `log::stream_log` parses `git log`
      output as it arrives (`GitClient::exec_reading_stdout`), emits `graph_progress` every 2,000
      commits, and, while nothing complete is on screen, swaps in a `complete: false` snapshot at
      the first 2,000 commits and then every 500 ms. On git.git the first rows show 221 ms into a
      1.7 s load.
- **`fix_safe_directory` command:** moved to milestone 2, step 2e.
- [x] **`GraphOrder::Topo`** (2026-10-14). Matches `git log --topo-order`: tips newest first, then a
      stack instead of the date heap, so a merged branch's commits sit together under the merge.
      Tests: `topo_order_keeps_a_branch_together_when_dates_interleave`, `topo_order_matches_git`.
- [x] **`graph_search`** (2026-10-14). The next or previous row whose subject, author name or email
      contains the text, or whose oid starts with it, wrapping at the ends, with the number of
      matches; 3.5 to 4.7 ms on git.git.
- [x] **Restoring the last selected commit** (2026-10-14, `restoreLastSelected` in
      `stores/sessions.ts`). The first snapshot holding it selects it and scrolls to it, unless the
      user has selected something already. Tried in the running app: close and reopen the tab.
- [x] **Passphrases in the system keychain** (2026-10-14, `keys/keychain.rs`, the `keyring`
      crate). `keysUnlock` takes `PassphraseMemory` (`none`, `session` or `keychain`); a passphrase
      the keychain refuses is still kept for the session, and `KeyInfo.passphraseRemembered` says
      where it went. Inside WSL there is usually no Secret Service, so the dialog offers the session
      only and says why. Not yet tried with Windows Credential Manager in the running app.
- [x] **Line endings on the diff** (2026-10-14). `DiffText.oldEol` and `newEol` come from the diff's
      own lines, and the diff header shows "LF", "CRLF" or "Mixed", or "LF → CRLF" highlighted when
      they change. Tried in the running app.
- [x] **WSL `.ssh` folders from the Windows build** (2026-10-14). `default_key_dirs` adds
      `\\wsl.localhost\<distro>\home\*\.ssh` for each running distribution
      (`wsl::list_running_distros`); a stopped one is left out, because reading its files would
      start it each time the key list is scanned. Not yet run on Windows.
- [ ] **PPK export.** `keys::export_private` writes OpenSSH and PKCS#8 only; the `ssh-key` crate
      reads PPK but does not write it. Left for when a tool that still needs PPK turns up.
- [x] **Password prompt and retry** (2026-10-14). `sshAuthFailed` now carries `port`, `user`,
      `passwordAllowed` (from the server's own list of methods) and `passwordRejected`; a refused
      password is forgotten. The frontend asks and retries the open through `reportOpenFailure`.
      Tried on 2026-10-14 from the Windows build against Michael's Ubuntu home server
      (192.168.2.200, password logins): the Trust dialog showed the fingerprint `ssh-keyscan`
      reads, the password dialog came up and the password worked, and the repository opened,
      polling with the "inotifywait is not installed" hint until inotify-tools was installed, then
      "watching: inotify (remote)". **Fixed on the way:** a passphrase-protected default key
      (`~/.ssh/id_rsa`) whose passphrase is not known used to win over a server that takes
      passwords, so the passphrase dialog came up and a password could never be given. Now such a
      server gets the password prompt, as OpenSSH falls back to one; a key chosen in the connection
      settings still asks for its passphrase.

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
      starts expanded to show what the command printed: its standard output, then its standard
      error, red when it failed (since 2026-10-14; before, only standard output, so a push, fetch,
      pull or checkout, which report on standard error, showed nothing). A category button pressed in Log mode
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
- [x] **SSH Keys page** (2026-10-14, `KeyManagerPage.vue`, `KeyRow.vue`, `KeyImportDropzone.vue`,
      `PassphraseDialog.vue`). Every key in the `.ssh` folders and `keys.extraDirs`, with its
      format, algorithm, fingerprint and badges, and Copy public key, Use for repository, Convert to
      OpenSSH, Export, Add to agent, Change or Forget passphrase, Reveal in folder and Delete; Generate,
      Import (drop, choose or paste) and Add Folder. New commands `keysDelete`, `keysReadFile`,
      `keysDefaultComment`, `keysKeychainStatus`. Passphrase, host key and password dialogs open on
      top of Settings. Tried in the running app (list, actions menu); dropping a file from the
      desktop has not been tried.
- [x] **Key picker in Add Repository** (2026-10-14). The SSH key field is a list fed by
      `keysScan`, with Automatic first and the chosen key's fingerprint under it. Tried in the
      running app. Switching the connection to SSH does not yet fill Host and User from a
      `user@host:path` already typed.
- [x] **Host key dialog** (2026-10-14, `HostKeyDialog.vue`). An unknown key shows its fingerprint
      with Trust and Connect; a changed key is a red warning that only closes and says how to remove
      the old line. Tried in the running app against a private `sshd`, then the passphrase dialog
      (a wrong passphrase is said to be wrong) and the open.
- [x] **Password dialog** (2026-10-14, `SshPasswordDialog.vue`): calls `sshSetPassword`, then the
      open is retried; a refused password shows "The server refused the last password."
- [x] **Stash list in the Branches panel** (2026-10-01; see milestone 2, step 3). The Stashes
      heading lists each stash with its message and a relative time, and right-clicking one offers
      Apply, Pop and Drop.
- [x] **Watcher hint** (2026-10-14). `watcherDetail` is the status bar watcher label's tooltip, with
      a mark beside the label when there is one. The first watcher state, which arrives before the
      tab's session exists, is now held until it does (before, the label never showed), and
      `repo_open` announces it again to a frontend that reloaded.
- [x] **Repository warnings** (2026-10-14). The active repository's `RepoInfo.warnings` show as
      strips under the startup warnings, each dismissible for that tab.
- [x] **Dropping far pages** (2026-10-14, `useGraphPages.dropFarPages`, `lib/graph/pageWindow.ts`):
      pages more than five from the visible rows are dropped when the view moves.
- [x] **Find box** (2026-10-14, `GraphFindBar.vue`): Ctrl+F or Edit > Find Commit; Enter and
      Shift+Enter move through matches, Escape closes it. Tried in the running app.
- [ ] **Syntax highlighting in the diff viewer** (Shiki in a Web Worker was the candidate). Planned
      for milestone 3; listed here because the diff viewer is otherwise complete.
- [x] **Gravatar** (2026-10-14, `lib/avatars.ts`). With `appearance.avatarSource` set to Gravatar,
      the author cell shows the picture for the SHA-256 hash of the email address, and keeps the
      initials when there is none.

### Verification still owed for milestone 1

- [x] Large history (2026-10-14, git.git, 85,563 commits, release build): first paint from cache
      2.6 ms, a 200-row page 0.3 to 2.3 ms, a refresh after one commit 299 ms. **Found:** git.git's
      graph is 282 lanes wide, and a page in the middle carries 2.4 MB of JSON (43,520 edges) to
      draw lanes the column clips anyway; that, not the backend, is the likely cost of scrolling.
      Leaving out edges beyond the column's width would fix it. Debug builds build snapshots about
      nine times slower, so timings in `tauri dev` look worse than the shipped app.
- [x] Manual check of the rendered UI (2026-09-23): Michael looked at the running app and was
      happy with it.
- [x] Cross-side runs (2026-10-14): from the Windows build against a real Ubuntu server over SSH with
      a password (see "Password prompt and retry"), and the WSL and Windows-folder tests in
      `repo/cross_e2e.rs`. Still to check on screen: whether dialog and menu text is ClearType.

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
- **Put off to milestone 3, done 2026-10-01 with pull:** the operation registry
  (`RepoHandle::start_operation` and `cancel_operation`, and the `op_cancel` command).

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
- [x] **c. Stashes** (2026-10-01; commands `op_stash_push`, `op_stash_apply`, `op_stash_pop`,
      `op_stash_drop`). `op_stash_push` takes an optional message and an include-untracked flag.
      Before apply, pop or drop, `stash_selector` checks that `stash@{index}` still points at the
      commit the list showed, and refuses the command when a stash was made elsewhere in the
      meantime. Test: `stash_push_apply_pop_drop_and_stale_index`.
- [x] **d. Rename, reset, cherry-pick, revert** (2026-10-14; commands `op_branch_rename`,
      `op_reset` with `ResetMode` soft, mixed or hard, `op_cherry_pick` and `op_revert` with an
      optional `mainline` for a merge commit, `op_cherry_pick_abort`, `op_revert_abort`).
      Conflicts come back as `OpResult.conflicts`; full conflict resolution stays in milestone 3.
      `reset` ends its arguments with `--`, so a target that is also a file name means the
      revision. Test: `rename_reset_cherry_pick_and_revert`.
- [x] **e. `safe.directory`** (2026-10-14; command `repo_fix_safe_directory(key, path)` in
      `ipc/repos.rs`). Runs `git config --global --add safe.directory <path>` through the library
      entry's own connection, with the path exactly as Git's "dubious ownership" message gave it,
      before the repository is open. Git runs that command even inside the folder it refuses.
      Test: `safe_directory_lets_git_open_a_repository_it_refused`, which uses Git's
      `GIT_TEST_ASSUME_DIFFERENT_OWNER` and a scratch global config through the new
      `GitReq::env`.
- [x] **f. Hunk and line staging** (2026-10-14; `git/ops/lines.rs`, command `op_apply_lines`).
      The frontend sends the picked lines by number (new file for an addition, old file for a
      removal) and text; the backend recomputes the diff with fixed options, refuses a selection
      whose text no longer matches, and builds a patch for `git apply` (`--cached` to stage or
      unstage, the working tree to discard) that always applies forwards to the version it
      changes. A selection that covers every changed line runs the whole-file command instead,
      which handles new and deleted files and modes. A patch that would put a line after one with
      no final newline is refused: `git apply` accepts it and joins the two lines into one.
      Partly staging an untracked file creates it with mode 100644. Tests:
      `stage_unstage_and_discard_single_lines` (CRLF, no final newline, new file, stale
      selection, whole-file fallback) and the patch unit tests in `lines.rs`.

### Step 3: frontend

- [x] **Multi-select in the Files panel** (2026-09-30). Click, Ctrl-click, Shift-click,
      Ctrl+Shift-click, arrow keys with and without Shift and Ctrl, Home, End and Ctrl+A, from
      `src/composables/useListSelection.ts`, which the Commit dialog's list uses too. reka-ui's
      `Listbox` was checked first and not used: its mouse clicks ignore Shift and Ctrl. The diff
      shows the last file clicked, not the whole selection. Ctrl+A outside a text field never
      selects page text (`App.vue`) and selects every file in the active tab.
- [x] **Files panel** (2026-10-14): a right-click menu with Stage, Unstage, Discard Changes…,
      Open, Show in Folder, Copy Path and Copy Full Path, and the toolbar's Stage, Unstage and
      Discard buttons, all acting on the selected working tree rows (`worktreeActionPaths` in
      `lib/files.ts`). Right-clicking a row outside the selection selects just that row. Discard
      sends unstaged rows back to the index, staged rows back to HEAD and deletes untracked
      files, after one confirmation that lists them. A toolbar button that cannot act is greyed
      out with the reason as its tooltip. Open and Show in Folder use the new `get_host_path`
      command and the `opener:allow-open-path` permission; neither works over SSH.
- [x] **Commit dialog** (2026-09-30, `src/components/dialogs/CommitDialog.vue`, opened by the
      toolbar's Commit button). Lists every outstanding file with a checkbox. The files selected
      in the Files panel start ticked; with no selection, the `commit.preselect` setting decides
      (`all`, the default, or `staged`), set on the Settings dialog's Commit page. Space
      flips the ticks of the selected rows. Ticking exactly the staged files commits the index as
      it is; any other set is committed as it is on disk, with a warning for partly staged files.
      Amend loads the previous message, and the buttons change to "Amend Message" (nothing
      ticked) or "Amend Commit" (files ticked). The message draft survives Cancel. "Commit & Push"
      works since 2026-10-01 (see Fetch and push under milestone 3). The dialog is 48rem wide
      with large buttons, and the overlay behind every dialog (`ui/dialog/DialogOverlay.vue`) is
      black at 50%.
- [x] **Stashes in the interface** (2026-10-01). The Branches panel lists the stashes, with
      Apply, Pop and Drop on right-click (Drop asks first). A "Stash changes" dialog
      (`StashDialog.vue`) takes an optional message and an "Include untracked files" checkbox; an
      "Apply stash" dialog (`ApplyStashDialog.vue`) picks one stash and can drop it after applying.
      They open from the Repository menu, the toolbar's Stash button (saving only: applying means
      choosing a stash, so it has no toolbar button), and a new right-click menu on the Working
      Tree row (`WorkingTreeContextMenu.vue`), which greys out an item and gives the reason when it
      cannot run. A stash that applied with conflicts is reported as conflicts to resolve, not as a
      failure.
- [x] **Apply Stash dialog lost the chosen stash** (fixed 2026-10-14). When the picked stash
      disappears while the dialog is open, nothing is picked, Apply is disabled, and the dialog
      says so; it no longer falls back to the newest stash.
- [x] **Commit menu additions** (2026-10-14): Cherry-pick onto the current branch, Revert, and a
      Reset submenu (soft, mixed, hard), each asking first; a merge commit is measured against
      its first parent. A conflict leaves a notice with Abort; a cherry-pick whose change is
      already on the branch is aborted at once and said so. Branch menus gained Rename Branch…
      (`RenameBranchDialog.vue`).
- [x] **Stale lock and `safe.directory` prompts** (2026-10-14). `runOp` now takes a function
      that starts the command: on `indexLocked` it asks whether to remove the lock, removes it
      and runs the command once more. A confirmation asked for while another dialog is open
      shows on top of it (`overlay` in `stores/dialogs.ts`), so the Commit dialog stays. An
      `unsafeRepository` failure to open becomes a notice with "Trust this folder…"
      (`openRepository` and `reportOpenFailure` in `lib/repoActions.ts`).
- [x] **Hunk and line buttons in `DiffViewer.vue`** (2026-10-14). In an unstaged or untracked
      file's diff each hunk header has Stage hunk and Discard…, in a staged file's diff Unstage
      hunk. Clicking a changed line's number picks it (Shift-click picks a range), and a strip
      under the header offers Stage, Discard… or Unstage for the picked lines.

### Done when

`cargo test`, `cargo clippy --all-targets` and `npm run build` pass, and in the running app on a
real repository you can stage part of a file, commit, amend, stash and pop, and reset, with one
refresh per action in the console and the result matching the `git` command line.

Met on 2026-10-14 in the Linux build, driven through WebDriver against throwaway repositories
(see "Checked in the running app"): a hunk, a single removed line and two of an untracked file's
three lines staged, a hunk unstaged, a line discarded, the Files panel's menu and the toolbar
buttons, a stale `index.lock` during a commit, revert, cherry-pick (twice), a soft reset, a
rename, the Apply Stash fix and the `safe.directory` prompt, each checked with `git`.

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
      `GraphMeta.refColours` gives each shown ref's colour. The checked-out branch is marked
      with the graph's HEAD dot, in its colour (since 2026-10-01; a `ChevronRight` before that). Test: `hidden_refs_drop_their_commits_and_labels`.
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
      the rounded 22px, so the text drifted 0.4px a row. Since 2026-10-01 the row, diff row and
      pill heights are whole pixels in the stylesheet (`--row-h: 22px`), `ROW_HEIGHT` in
      `lib/graph/paths.ts` is the same 22, and `main.ts` no longer overrides `--row-h`.
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
- [x] **Commit header**: regular-weight subject across the width, with a "More" toggle for the
      rest of the message, since 2026-10-01 in the header's bottom right corner.

### Diff viewer

- [x] **Side by side** splits the panel 50/50 with a draggable divider (`stashpanda.diff.split` in
      local storage). Each side scrolls sideways on its own scrollbar, and moving one moves the
      other to the same fraction of its range.
- [x] **Line height setting** `appearance.diffLineHeight`, since moved to `diff.lineHeight` (default 1.2 times the font size). The
      virtual list and the rows now use one whole-pixel height; before, the list spaced rows at
      1.4rem while they were drawn at 1.25rem, so the text jumped while scrolling.
      `useVirtualRows` accepts the row height as a getter.
- [x] **Font smoothing setting** `diff.fontSmoothing` (2026-10-01): ClearType (the default),
      Grayscale or None, for the diff text only. Until now the diff was always grayscale and
      looked soft next to the rest of the window: the virtual list's row container had
      `will-change-transform`, which puts the rows on a compositor layer of their own, and
      Chromium never draws ClearType there. That class is gone. WebView2 ignores
      `-webkit-font-smoothing` (macOS only), so Grayscale sets `will-change: transform` on the
      element holding the rows, and None uses the `#diff-aliased-text` SVG filter in
      `index.html`, which rounds each pixel's coverage to on or off. That filter sits on the
      `diff-glyphs` spans, which hold only text, so translucent row colours and changed-word
      highlights keep their colour. All three measured in headless Edge 154 on Windows against
      the real `DiffViewer` and `DiffPreview`. Not yet tried in the running app, and scrolling
      speed with None is unmeasured. The commit graph's `VirtualRowList.vue` had the same
      `will-change-transform` and lost it too, so the graph's text is ClearType now as well
      (measured in headless Edge with the real component: no colour-fringed pixels before, 12593
      after). The graph has no smoothing setting.
- [x] **ClearType in dialogs, menus, popovers and tooltips** (2026-10-01). All of them drew
      grayscale, soft text, because Chromium gave them a compositor layer of their own, and it
      never draws ClearType on a layer that is not opaque (their rounded corners are
      transparent). Chromium's DevTools layer tree gave the reasons: the dialog backdrop's
      `backdrop-blur-xs` (`BackdropFilter`), `position: fixed` on the backdrop, the dialog and
      reka-ui's menu wrappers (`Viewport`), and the dialog drawn over the backdrop (`Overlap`).
      Fixes: the backdrop has no blur, and it and `DialogContent` are `absolute` (the window
      never scrolls); `DialogContent` is centred with `inset-0 m-auto h-fit` instead of a -50%
      translate, which put a dialog of odd size on a half pixel; the dropdown menu, context
      menu, popover and tooltip wrappers in `src/components/ui` default `positionStrategy` to
      `absolute`; and `index.css` overrides the `will-change: transform` Floating UI adds on
      screens scaled 150% or more. Measured in headless Edge 154 with the real components:
      Add Repository's text went from no colour-fringed pixels to ClearType, as did an open
      dropdown menu, and the layer tree now has only the root layers. The open and close
      animations still take a layer while they run (100ms). The only `will-change` left in
      `src` is the diff's Grayscale font smoothing, which uses it on purpose. Not yet tried in
      the running app. The blur cannot come back on its own: with the backdrop `absolute`,
      `backdrop-blur` still gives it a `BackdropFilter` layer and the dialog an `Overlap` one,
      and the dialog text measured grayscale again.
      **Correction, same day:** in the running app the dialog text was still soft. The test page
      had nothing composited behind the dialog; the app has the panels' composited scrollers,
      so the backdrop and the dialog always get a layer (`Overlap`). With a composited element
      behind the test dialog, the dialog's `ring-1` (a box-shadow) turned all its text grayscale;
      without it the title, labels and fields measured ClearType, rounded corners and all. A
      translucent border did the same as the ring; an `outline` kept ClearType. The ring is now
      gone, with no replacement, and the backdrop is 60% black instead of 50%. **Still open:**
      in that test the Advanced row and the footer (Cancel, Clone) stayed grayscale. Chromium
      puts them in a separate 588×101 layer, which goes away if either the footer or the
      Advanced section is hidden; removing the footer's inset shadow, the buttons' shadow,
      transition, background clip or corners, the resize handles, or giving the dialog its own
      layer with `translateZ(0)` did not help. Menus need both their `shadow-md` removed and
      `overflow-y-auto` off before they measure ClearType over a composited layer; not changed.
      **Measured in the running Windows app on 2026-10-14** (through WebView2's DevTools protocol;
      start the app with `WEBVIEW_DEBUG_PORT=9222 npm run win:ux`): the Settings dialog's footer
      has ClearType, while Add Repository's Cancel and Add buttons do not (0 coloured pixels, the
      title 337). Chromium draws Add Repository from the Advanced row down to the bottom edge in a
      separate squashing layer (794×252, reason Overlap). These made no difference: hiding the
      resize handles, hiding the panel dividers' `::after` hit areas, turning the opening
      animations off, the disabled Add button at full opacity, and an opaque background behind
      the "Advanced" label; giving the whole dialog a layer of its own turns its title grayscale
      too. Michael finds the menus fine and the dialog text sharp apart from those two buttons.
- [x] **The first file is selected** (2026-10-14, `FilesPanel.vue`). Selecting a commit or the
      Working Tree row selects the first file of its list once it has loaded, which shows its
      diff. Once per row: a status refresh never moves a selection, and a file already clicked is
      left alone.
- [x] **Add Repository opened under another dialog** (fixed 2026-10-14). Opening it (File >
      Open Repository still works from the native menu) while Settings was open put it underneath,
      because it comes first in the page, while as the newest modal dialog it took every click, so
      the dialog in view could not be used. `dialogs.showAddRepo()` now does nothing while another
      dialog is open; every opener goes through it.
- [x] **Dialog resizing applied the size again** (2026-10-01). `DialogContent` wrote the
      dragged size onto `unrefElement(contentRef)`, the element reka-ui forwards from its
      DialogContent. reka-ui works that element out once and caches it, so it could be the
      placeholder rendered while the dialog was closed, and dragging an edge then did nothing.
      The size now goes onto the resize handles' parent element. Checked with real mouse
      input through the DevTools protocol: the Add Repository dialog failed in 4 of 4 drags
      before, grew from 588×612 to 708×732 in 4 of 4 after, and kept its size after release.
- [x] **Diff row classes** (2026-10-01): the rows, gutters, markers and changed-word highlights
      use named classes from `src/assets/diff.css` (`diff-line`, `diff-num`, `diff-sign`,
      `diff-code`, `diff-word-add` and the rest), built with `@apply`, instead of long utility
      lists on every row; `DiffPreview.vue` uses the same classes. The rows no longer go
      through `cn()` (tailwind-merge) on every render. Checked in headless Edge: the unified
      viewer and the preview render pixel for pixel as before.

### Shell, fonts and icons

- [x] **Fonts bundled**: IBM Plex Sans, Outfit and JetBrains Mono from their `@fontsource`
      packages, imported in `main.ts`, instead of a Google Fonts `@import`, so the app looks the
      same offline.
- [x] **Theme menu ticks** follow the setting (`menu.rs::sync_theme`).
- [x] **Active tab** keeps the same box as the others, so it no longer moves, and is lit in the
      accent colour.
- [x] **Keyboard focus** is a 1px accent outline instead of the browser's white ring
      (`index.css`).
- [x] **Lucide icons** draw 1px lines at any size. (The toolbar's own icons are 20px with a
      1.67px line since 2026-10-01; see below.)
- [x] **Console** scrolls sideways for long command lines; expanded output stays panel-wide.

## Interface, merge and checkout work (done 2026-10-01)

Requested while using the Windows build. `cargo test` (60 tests), `cargo clippy --all-targets`,
`vue-tsc` and `vite build` pass for all of it. Layout was checked by rendering the built stylesheet
in headless Chromium and measuring pixels, and the merge message parsing with scratch Node scripts;
none of it has been checked in the running app yet.

### Ref pills and lane colours

- [x] **Pill text and size.** Pills are 18px tall, so they centre in the 22px row, with regular
      400-weight `text-sm` text. The `pill-text` utility trims each text span to its lowercase
      letters (`text-box: trim-both ex alphabetic`), since IBM Plex Sans's line box put them 0.164em
      low on Windows and 0.117em low elsewhere; an `@supports` fallback lifts them by 0.117em.
      Solid pills use the lane's chip fill (`--lane-bg`) for their text.
- [x] **Notched pills.** A local branch whose upstream is on another commit has a chevron notch on
      its left (`pill-tail`), and its remote a matching point on its right (`pill-head`), drawn
      with `clip-path` over a 1px edge layer. A remote pill reads "origin › dev" with a
      `ChevronRight` icon.
- [x] **A colour per branch tip.** Several branch tips on one lane used to share its colour. Now
      the top-most tip keeps the lane's colour and each tip below it gets its own, picked so its
      hue (`index % 6`, since the dark theme repeats lanes 0 to 5 as 6 to 11) differs from the
      lane's. The new `appearance.commitLaneColours` setting chooses between `keepLane` (the
      default: only the pills and Branches panel tags change) and `changeAtEachTip` (the lane's
      colour switches at each tip too). A remote branch that a shown local branch tracks is not a
      tip of its own and wears that branch's colour. `GraphOptions` carries the setting and the
      graph order from the IPC layer to `GraphSnapshot::build`. Each `RefLabel` has its own
      `colour`.
- [x] **The branch each row belongs to.** The lane sweep records, for each row, the nearest branch
      tip above it on its lane (`RowLayout::branch`), following which lane carries on where two
      join. Test: `each_row_belongs_to_the_nearest_branch_tip_on_its_lane`.

### Commit table

- [x] **Working Tree row always shown**, clean or not (`worktree_summary` returns all-zero counts,
      and `None` only for a bare repository). On a clean tree it reads "No changes", and the Files
      panel says "The working tree is clean. Nothing to commit." centred.
- [x] **Author column collapses** to just the avatars: its minimum is 25px, and the name hides
      below about 51px through a container query. Avatar initials are centred on their capitals
      (`text-cap-box`).

### Merges in the Files panel

- [x] **Straight merges.** The new `get_merge_info` command returns the merge commit's own branch
      and, for each parent, its branch and whether the merge kept its files unchanged (one
      `git show -s --format="%H %T %P" <merge> <merge>^@`). A merge that kept one parent's files
      shows "Merged X into Y" with a `GitBranch` icon and compares with the other parent only.
      Other merges show "Compare with" toggles named after each parent's branch, with a `GitFork`
      icon. Test: `finds_the_parent_whose_tree_the_merge_kept`.
- [x] **Branch names from merge messages** (`src/lib/mergeParents.ts`): Git, GitLab, GitHub,
      Bitbucket, Bitbucket Server, Gitea, Forgejo and Gerrit forms. `git pull`'s "of <url>" is
      matched to a configured remote, so it reads as `origin/master`. A Git message with no
      "into" means main or master, since Git leaves the target out only for those. Azure DevOps's
      "Merged PR 123: Title" names neither branch, so those rely on the graph.
- [x] **More or Less** sits in the commit header's bottom right corner (`MessageToggle.vue`), inside
      whichever part comes last: the subject, the expanded body or the Compare with row.

### Toolbar, Branches panel and checkout

- [x] **Flat toolbar buttons** (`AppToolbar.vue`), from the design canvas's "Changes — Blender
      pass" artboard: 63 × 42px, no gap between them, 20px icons with a 1.67px line, hover and
      pressed at 6% and 10% ink, disabled at 38%. Dividers use `--key` and are centred in the bar.
      Commit is a solid primary button with a badge counting the files the Commit dialog would
      list (`commitFileCount` in `lib/files.ts`).
- [x] **Branches panel**: the checked-out branch has the graph's HEAD dot, each local branch shows
      the branch it tracks after its name ("› origin/master") in muted text, and double-clicking a
      branch checks it out.
- [x] **One checkout path**, `requestCheckout` in `lib/repoActions.ts`, for the menus and the
      double-click. A dirty working tree asks first; a remote branch with no local branch of its
      name asks before `git checkout -b <local> <remote>` creates one that tracks it; a clean local
      checkout runs straight away. The Checkout menu items are no longer disabled on a dirty tree.
- [x] **Success notices** (`notifySuccess`) show plainly with a tick and `role="status"`; only
      failures use the red error style.
- [x] **Status bar messages** (2026-10-01). Success messages ("Committed …", "Stashed …") show
      at the right of the status bar with a tick and a dismiss button. They are kept in their own
      `successMessage` state in the sessions store and never go through `notify()`, which now
      posts errors only, to the red strip under the toolbar. A new error clears the success
      message and a new success message clears the error. The "Loading N commits…" message
      shows in the status bar too, with a spinner, in place of the strip that used to sit above
      the Commits table header (`GraphLoadingBar.vue`, now deleted). Tried in the running
      Linux build on 2026-10-14.
- [x] **Text selection off** everywhere except commit IDs, the Files panel's commit message, the
      repository path and diff code. Shift-click in the Commits table does not extend a text
      selection. The pointer cursor is gone from list rows.
- [x] **Diff highlight**: the dark theme's changed-word highlight in added lines is
      `rgba(94, 196, 98, 0.36)`, from the design system.

## Checked in the running app (2026-10-14)

The Linux build was driven through WebDriver (`tauri-driver` with WebKitWebDriver, on an
invisible Xvfb display, with its own config folder) against throwaway repositories: a bare
remote, a clone one commit behind and one ahead, and two clone targets. Each result was compared
with `git` on the command line. Open repository, Fetch, Sync (a merge, then the push), Push, Pull
through its dialog, Commit & Push, Stash, and Clone into a missing folder and into a folder with
files of its own all did what the sections above describe. Fixed on the way:

- [x] **Dialogs filled the window height on Linux.** WebKit ignores `h-fit` on an absolutely
      placed `display: grid` box and stretches it to the full height, so `m-auto` had nothing
      to centre; Chromium does not do this, which is why the Windows build looked right.
      `DialogContent.vue` is now a flex column (a resized dialog already was one), and the
      Settings dialog's body takes `flex-1` in place of its `grid-rows` row.
- [x] **Unreadable `<select>` boxes on Linux.** WebKitGTK drew them as white GTK controls under
      the field's light text, because the page never declared a colour scheme. `index.css` now
      sets `color-scheme` to `light` on `:root` and `dark` on `.dark`.
- [x] **A plain clone posted no message**, so the status bar kept the last message from another
      repository's tab. It now says "Cloned <url> into <folder>.", and the success message is
      cleared when another tab is shown (`stores/sessions.ts`).

Not covered: the Windows build's look (ClearType, font smoothing), dialog resizing by mouse,
and anything over WSL or SSH.

## Milestone 3: remote and history operations

- [x] **Pull** (2026-10-01). `op_pull(key, opId, remote, branch, mode, autostash)` runs two
      commands rather than `git pull`, so that Stop only ever lands on the network transfer:
      `ops::pull_fetch` runs `git fetch --progress <remote> <branch>`, which can be stopped, and
      `ops::pull_integrate` then brings `FETCH_HEAD` in, which cannot. Merge is
      `merge --no-edit FETCH_HEAD` (the same "Merge branch 'dev' of <url>" message as `git
      pull`); Fast-forward only is `merge --ff-only FETCH_HEAD`; Rebase fast-forwards when the
      branch has no commits of its own, and otherwise runs `rebase --onto FETCH_HEAD <fork
      point>` with the fork point from `merge-base --fork-point refs/remotes/<remote>/<branch>
      HEAD`, so commits the remote has rewritten are dropped as `git pull --rebase` drops them.
      `--autostash` goes on the second command. All checked against `git pull` with Git 2.43.
      `Transport::run_streaming` reads stderr as it arrives, splits it on `\r` and `\n`, strips
      escape codes, and sends each update as an `operationProgress` event carrying the caller's
      `opId`; the `stderr` it returns keeps only the last update of each line. When the fetch is
      done, an event with `cancellable: false` names the local step, and the dialog disables
      Stop. There is no `operation_finished` event: the command's own result is the end.
      `op_cancel(key, opId)` cancels through the registry on `RepoHandle` (`start_operation`,
      `cancel_operation`; tokens are children of the repository's, so closing the tab cancels
      too). `get_pull_mode` reads `branch.<name>.rebase`, `pull.rebase` and `pull.ff` in Git's
      order. `op_rebase_abort` is new, for a rebasing pull that conflicts.
      Test: `pull_modes_conflicts_autostash_and_cancel`.
      **Locking changed:** every write now takes `RepoHandle::write_gate` for its whole run, so
      writes never overlap; short writes also take the exclusive lock as before. A pull holds the
      gate throughout but the exclusive lock only for its local step, so status polls, graph
      pages and diffs keep working while it fetches.
      Frontend: the toolbar's Pull button and Repository › Pull open `PullDialog.vue` (remote
      branch, defaulting to the upstream; Merge, Rebase or Fast-forward only, remembered in
      `perRepo[key].pullMode`, else the Git config, else Merge). With uncommitted changes,
      `openPullDialog` asks first and offers "Pull anyway (autostash)". The dialog shows the
      latest progress line with a bar from its percentage, and Stop. Closing the dialog leaves
      the pull running; the outcome then shows as a notice. Conflicts offer Abort rebase
      or Abort merge; an autostash that conflicts on the way back (Git exits 0) is reported as
      needing attention. Tried in the running Linux build on 2026-10-14.
- [x] **Stopping a command properly** (2026-10-01, for the pull's fetch). `Cmd::own_session`
      (`GitReq::own_session`) runs a command so that `Process::terminate` reaches it and every
      process it starts, and `run_streaming` returns `Cancelled` only once it has exited, or
      after `TERMINATE_GRACE` (5 s) and a `kill`. The write gate is therefore never released
      while Git still runs.
      - Linux and macOS (`transport/local.rs`): `setsid` before `exec`, so Git leads a new
        session with no controlling terminal (`ssh` cannot stop to ask for a passphrase on the
        terminal StashPanda was started from) and terminate sends SIGTERM to its process group,
        which lets Git remove its lock files.
      - Windows: `taskkill /T /F` on the process tree. Git for Windows' `cmd\git.exe` is a
        launcher for `mingw64\bin\git.exe`, so killing only the first left the real Git and its
        `git-remote-https.exe` or `ssh.exe` running. The tests run on Windows since 2026-10-14
        (see Packaging and CI), and `own_session_on_the_local_transport` passes there.
      - WSL (`transport/wsl.rs`): `wsl.exe -d <distro> -e setsid -w sh -c 'echo $$; exec "$@"'
        sh <command>`. `PidLine` takes the pid off stdout; terminate runs `kill -TERM -- -<pid>`
        through a second `wsl.exe` (87 ms). Measured 2026-10-01: killing `wsl.exe` alone does end
        the Linux processes it started, but not ones in a session of their own, so a
        `WslProcess` dropped before it exits kills its group too, and so does a `TokioProcess`
        with `own_session` on Linux.
      - SSH (`transport/ssh.rs`): terminate sends SIGTERM on the channel and keeps reading until
        the exit status arrives; `kill` still closes the channel, which OpenSSH's `sshd` answers
        by ending the remote process and its children (checked with OpenSSH 9.6).
      Tests: `own_session_on_the_local_transport`, `own_session_on_the_wsl_transport` (drives
      the real `WslTransport` through `wsl.exe` interop from WSL; skips elsewhere),
      `pid_line_takes_the_first_line_and_passes_the_rest`, and the terminate step in
      `ssh_transport_against_private_sshd`. With `own_session` off, or the `Drop` impls disabled,
      the local and WSL tests fail.
      Left: Windows `git.exe` driven from the Linux build through interop gets `setsid` on the
      Linux side, which does not reach the Windows process tree. A fetch stopped on Windows at
      the moment it writes refs can leave a `.lock` file under `.git/refs`, since `taskkill /F`
      gives Git no chance to clean up.
- [x] **Fetch and push** (2026-10-01), on the same plumbing as pull. `op_fetch(key, opId)` runs
      `git fetch --all --progress`, which follows `fetch.prune`. `op_push(key, opId, branch,
      target, setUpstream)` runs `git push --progress [--set-upstream] <remote>
      refs/heads/<branch>:refs/heads/<remote branch>`; full ref names on both sides, so a tag
      with the branch's name is never pushed instead. With no `target` it pushes to the upstream
      from `branch.<name>.remote` and `.merge`, which `get_upstream` also returns (an upstream
      on remote `.`, another local branch, counts as none). It never forces. Both are stoppable
      from start to end and hold only the write gate, through the new `network_op!` macro in
      `ipc/ops.rs`. Test: `fetch_and_push_to_the_upstream_or_a_new_branch`.
      Frontend (`lib/repoActions.ts`):
      - A new Fetch toolbar button and Repository › Fetch run `fetchAll`, in one click.
      - The toolbar's Push button and Repository › Push run `pushCurrentBranch`, in one click to
        the upstream. A branch with no upstream opens `PushDialog.vue` (remote, branch name on
        the remote, "Track it from now on", ticked by default). A rejected push says the remote
        has commits the branch lacks and offers "Pull…".
      - The Commit dialog's "Commit & Push" commits, then runs `pushCurrentBranch`. "Amend &
        Push" is disabled when the last commit is already on the upstream (the branch is not
        ahead of it), since that would need a force push.
      - `runOperation` keeps the running fetch, pull or push on the session
        (`RepoSession.operation`), fed by `operationProgress` events in `stores/sessions.ts`.
        The status bar shows its title, Git's latest progress line and Stop; the Pull dialog
        reads the same state. A second fetch, pull or push in the same repository is refused
        with a notice while one runs.
      - Credential failures (`GIT_TERMINAL_PROMPT=0`) get one explanation for all three
        (`networkFailureText`); a `GIT_ASKPASS` helper is still to be designed.
      Tried in the running Linux build on 2026-10-14.
- [x] **Sync** (2026-10-01): the toolbar's Sync button and the new Repository › Sync (Pull,
      then Push) run `syncCurrentBranch`, in one click. It pulls the upstream with the mode last
      chosen in the Pull dialog (else the Git config, else Merge), then pushes back. With
      uncommitted changes it asks first, as Pull does, and goes ahead with `--autostash`. A
      branch with no upstream has nothing to pull from, so it goes to `pushCurrentBranch`, which
      opens the Push dialog. `op_sync(key, opId, branch, mode, autostash)` runs the whole thing
      as one operation under the write gate (`sync_steps` in `ipc/ops.rs`, sharing
      `pull_steps` with `op_pull`), returning a `SyncResult` with the pull, the push, and
      `nothingToPush` or `pushStopped`. Nothing is pushed when the pull fails or stops on
      conflicts, and the push is skipped when `rev-list --count` finds no commits to send. Stop
      works while it fetches and while it pushes, not while it merges or rebases: the registry
      entry now carries a cancellable flag (`OperationGuard::set_cancellable`), which
      `op_cancel` respects. A push refused because the remote moved on meanwhile offers "Sync
      again". Test: `sync_pulls_then_pushes_and_stops_where_it_should`, which goes through a
      real `RepoHandle` from `open_repo` and fails if the merge step can be stopped. Tried in
      the running Linux build on 2026-10-14.
- [x] **Clone** (2026-10-01): File › Clone Repository… opens the Add Repository dialog on its
      new Clone From tab; the other tab, Add Existing, is the dialog as it was. Editing a library
      entry shows no tabs. The two tabs share the path field and the Advanced connection
      settings, so Auto still picks Local, WSL or SSH from the folder chosen. As the path
      changes, `clone_inspect(path, connection)` resolves it and reports what is in the folder:
      missing (the clone creates it), empty, has files, or a repository already, which is
      refused with a link to Add Existing. When the folder has files or is a repository and its
      name is not the repository's (`django` from `git@github.com:django/django.git`), the
      dialog offers "Clone into …/django instead". `repo_clone(opId, url, path, connection,
      groupId, displayName)` clones and then adds the result to the library, as `repo_add`
      does, and the dialog opens it in a tab. A missing or empty folder gets a plain
      `git clone --progress`, run in the folder above it. **A folder with files in it** is never
      moved or copied: `repo/clone.rs` runs `git init`, `git remote add origin`, `git fetch
      --progress origin`, takes the default branch from `refs/remotes/origin/HEAD` (running `git
      remote set-head origin --auto` when Git did not create it and there is more than one
      branch), then `git branch --track`, `git symbolic-ref HEAD` and a mixed `git reset`, which
      sets the index and leaves every file alone. `git checkout-index --stdin` (without
      `--force`) then writes only the files the folder lacks. Before that, any repository file
      whose folder is taken by a file of the user's is left out: `checkout-index` would otherwise
      stop with "cannot create directory", and those files come back in `CloneOutcome.blocked`.
      The result is the clone with the user's files over it: changed where they differ, untracked
      where the repository has no such file, and a notice says so. On failure or Stop the
      `.git` folder it created is deleted (the whole folder, when it did not exist before), so
      the folder is as it was. Progress arrives as `cloneProgress` events; `clone_cancel(opId)`
      stops it through `AppState::clones`, a registry of the same kind as `RepoHandle`'s
      (`repo::start_operation` and `repo::cancel_operation` are now free functions that both
      use). Stop is refused once files are being written: from `Updating files` in a plain
      clone, and after the fetch in the other case. While a clone runs the dialog cannot be
      closed, only stopped. `Transport` gained `list_dir` and `remove_dir_all`. Test:
      `clones_into_missing_empty_and_non_empty_folders`. There is no toolbar button yet. Tried
      in the running Linux build on 2026-10-14, into a missing folder and a folder with files;
      not yet over WSL or SSH.
- [x] **Merge, rebase, cherry-pick and revert, with conflicts** (2026-10-14).
      - Status now carries `inProgress` (`git/queries/in_progress.rs`): the merge, rebase,
        cherry-pick or revert under way, read from the git directory through the transport's
        file functions (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `REBASE_HEAD`,
        `MERGE_MSG` and the `rebase-merge` or `rebase-apply` folder), with the rebase's branch,
        its `onto` commit and its step out of the total.
      - `OperationBanner.vue`, a bar above the commits, names the operation, counts the files
        still conflicted and offers Continue, Skip and Abort (`op_continue`, `op_skip`, and the
        existing aborts). Continue runs with `GIT_EDITOR=true`, so Git keeps its prepared message
        without opening an editor.
      - A conflicted file can take one side's version (`op_resolve_conflicts`, which deletes the
        file when that side deleted it) or be marked resolved. Both are in the Files panel's
        right-click menu and in the diff panel, and the sides are named by branch or commit
        (`lib/conflicts.ts`). Resolving one selects the next conflicted file.
      - The diff panel compares a conflicted file's base, either side and the working tree file
        (`DiffTarget` `conflict`, reading the `:1:`, `:2:` and `:3:` stages).
      - The toolbar's Merge and Rebase buttons, and Repository › Merge and Rebase, open
        `MergeDialog.vue` and `RebaseDialog.vue` (`op_merge`, `op_rebase`, `get_merge_info`).
      - During a merge, cherry-pick or revert, the Commit dialog commits the whole index with
        Git's prepared message (`get_pending_message` reads `MERGE_MSG` without its comment
        lines), because Git refuses a partial commit then. During a rebase it points at
        Continue instead, and with files still conflicted it says to resolve them first.
      - The conflict notices from merge, pull, sync, cherry-pick and revert point at the bar.
      Tests: `merge_cherry_pick_and_revert_conflicts_resolved_and_continued` and
      `rebase_continue_skip_abort_and_the_operation_in_progress`. Tried in the running Linux
      build on 2026-10-14.
- [x] **Three-pane merge editor** (planned and built 2026-10-14). Before it, a conflicted file
      could only take one side's whole version, or be edited in another editor and marked
      resolved. The merge editor resolves it in the app, block by block.

      **Built** (`src/components/merge/MergeEditorDialog.vue`, `src/lib/merge/`, and on the
      backend `git/queries/conflict_file.rs` and `write_worktree_file`): steps 1 to 6 below,
      tried in the running Linux build on 2026-10-14 against a real merge stopped on two
      conflicts (open by double-click, names, arrows, take for all, F7, Alt+Shift+Left, undo,
      scrolling together, Save with the markers left in, Save and Mark Resolved). Where it
      differs from the plan below:
      - The curves are inline SVG, not a canvas, for the ClearType reason given under "Curves
        in the side-by-side diff".
      - Unresolved conflicts are drawn in the conflict yellow (`--state-conflict`), and any
        other difference from the result in the diff green.
      - The panes are kept level at the middle of the pane being scrolled.
      - The stages are read by their object IDs from `git ls-files -u`, not as `:1:<path>`,
        since the long-lived `cat-file` keeps the first index it read (fixed in b5f207b).
      - Unresolved blocks are written back with seven-character markers, whatever the file's
        `conflict-marker-size`; a file with mixed line endings is saved with LF.

      **Added the same day:**
      - The "×" beside each arrow dismisses that side's difference. Its arrows go, its band
        fades and its lines lose their background. A conflict with both sides dismissed counts
        as resolved, keeping the result as it is. Dismissing goes into the undo history, so
        Ctrl+Z brings it back (`setChunkDismissed` and `setDismissed` in `lib/merge/blocks.ts`,
        inverted with `invertedEffects`).
      - Runs of four or more lines that are the same in all three panes, beyond three lines of
        context around each difference, fold into one "N unchanged lines hidden · Show" row in
        every pane (`lib/merge/folds.ts`, block widgets one line high). Show unfolds the run in
        all three panes. "Hide unchanged lines" in the toolbar turns folding off. The curves,
        arrows and scrolling work in fold-aware positions (`lineTop`, `lineAtHeight`), so the
        panes stay level through folds.
      - When the long-lived `cat-file` cannot find a version, the query asks a fresh one, which
        sees the repository as it is now.

      - An unresolved conflict with no lines in the result, which is every conflict in Git's
        default `merge` marker style, shows as a one-row placeholder that the curves reach and
        where typing resolves it (`placeholders` in `lib/merge/blocks.ts`).
      - Each pane has a strip of marks beside its scroll bar, green for differences and yellow
        for unresolved conflicts; clicking one goes there.
      - Applying or popping a stash that conflicts records it on the session
        (`RepoSession.stashConflict`), so the sides are named `main` and `stash@{0}`; a stash
        put back by `--autostash` is named from Git's "Stashed changes" label.
      - Saving writes unresolved blocks with the file's own marker length, and a file with
        mixed line endings with the one most of its lines use (`markerSize`, `crlf`).
      - Ctrl+F opens the shared Find box over all three panes, opening a fold that hides a
        match; the editor opens with the cursor on the first conflict.
      - `npm test` runs Vitest (pinned at 5.0.3) on `lib/merge/chunks`, `lib/merge/blocks`,
        `lib/diff/find`, `lib/diff/pairLines` and `lib/diff/sideLayout`.

      **Checked in the running Windows app on 2026-10-14**, through WebView2's DevTools protocol
      (scripts `merge-open.mjs`, `regions.mjs` and `merge-save.mjs` in `C:\src\stashpanda-debug`),
      on a merge-style conflict in a WSL repository:
      - ClearType holds everywhere. Counting colour-fringed pixels in grey or white text: the
        dialog title 295, the toolbar 265, each pane's line numbers 856 to 992, a fold row 335.
        CodeMirror's sticky gutter costs nothing; its only animation, the blinking cursor,
        belongs to `drawSelection`, which the panes do not use. The layer tree adds only the
        panes' scroll bars and the caret.
      - Taking a side with an arrow, typing into a placeholder, and Save and Mark Resolved
        worked over WSL: the file came out as expected, LF only, no markers, and staged.
      - The panes had Windows' wide scroll bars; they now have the app's thin ones.

      **Layout.** Three panes side by side, all in the diff font
      (`resolveDiffFont`, with ligatures, tab width and font smoothing) and with syntax colours
      from the existing Shiki worker:
      - Left: the version on the branch being built on, read-only.
      - Middle: the result, which is editable and is what gets saved to the working tree.
      - Right: the incoming version, read-only.

      **Names.** Each side pane's header names the branch in bold, followed by the short commit
      ID and subject. The words "ours", "theirs", "HEAD", "local", "remote" and "upstream" never
      appear. The names come from `operationNames` in `lib/conflicts.ts`, which the conflict bar
      already uses:

      | Operation | Left pane | Right pane |
      | --- | --- | --- |
      | Merge | the checked-out branch (`dev`) | the branch merged in (`feature/something`) |
      | Pull | the checked-out branch (`dev`) | the remote branch (`origin/dev`) |
      | Rebase | the branch being rebased onto (`main`) | the commit being replayed (`a1b2c3d from feature/something`) |
      | Cherry-pick | the checked-out branch | the picked commit |
      | Revert | the checked-out branch | `before a1b2c3d` |
      | Stash pop | the checked-out branch | the stash (`stash@{0}`) |

      A commit that no branch points at is named by its short ID and subject. Git leaves no
      marker file in the git directory after a stash conflicts, so the app keeps the stash it
      applied on the session to name it (done 2026-10-14).

      **Where it opens.** Each of these opens `MergeEditorDialog.vue`:
      - Files panel › right-click › "Resolve in Merge Editor…", the first item for a conflicted
        file;
      - double-clicking a conflicted file;
      - a "Merge Editor" button next to the conflict comparisons in the diff panel's header.

      The dialog uses the shared dialog sizing (a `size-key` and `data-dialog-grow`), opens at
      nearly the whole window, and remembers its size. It only offers files where both sides
      have the file (`UU`, `AA`). A file one side deleted (`DU`, `UD`) keeps the take-a-side
      actions. Binary files, files that are not UTF-8, and files over 5 MB are refused with the
      reason, and with "Open in your editor" instead.

      **Step 1: backend.**
      - `get_conflict_file(key, path)` in a new `git/queries/conflict_file.rs`. It returns the
        base, left and right versions (stages 1, 2 and 3, read through the long-lived `cat-file`)
        and the working tree file. For the working tree file it also returns its line ending
        (LF, CRLF or mixed), whether it starts with a byte order mark, whether it ends with a
        newline, and a hash of its bytes.
      - The conflict markers are parsed in Rust, so they can be tested against real Git output.
        Git writes three marker styles, and all three are read:
        - `merge`: `<<<<<<<`, `=======` and `>>>>>>>`;
        - `diff3`: adds a `|||||||` section with the base text;
        - `zdiff3`: like `diff3`, with the lines both sides share moved out of the block.
      - The marker length follows the file's `conflict-marker-size` attribute (`git check-attr`),
        which defaults to 7. A line that only looks like a marker, outside a complete
        `<<<<<<<`, `=======`, `>>>>>>>` sequence, stays as ordinary text.
      - The query returns the result text, which is the working tree file with each marker block
        replaced by its base text (or by nothing when the markers carry no base). It also
        returns the blocks, each with its line range in that text and its left, right and base
        text.
      - Starting from the working tree file, rather than merging the three versions again in
        TypeScript, keeps everything Git and the user have already done to it: changes that
        merged cleanly, a merge driver's output, `rerere`'s earlier resolutions and hand edits.
        A file with no markers left opens with no conflict blocks, and the arrows still work.
      - `Transport::write_file(path, bytes)` is new on all three transports:
        - Local: a plain write.
        - WSL and SSH: `cat > "$1"`, with the bytes on standard input.
        It writes in place rather than to a new file renamed over the old one, so the file's
        mode is kept.
      - `write_worktree_file(key, path, bytes, expectedHash)` saves the result under the write
        gate. It refuses, with a "changed on disk" error, when the file's hash no longer matches
        the one from opening. The dialog then offers to reload the file or overwrite it.
      - `op_restore_conflict(key, path)` runs `git checkout --conflict=diff3 -- <path>`, which
        writes Git's merge of that file again, with base sections. This is for "Start again from
        Git's merge", after asking first.

      **Step 2: editor panes** (`src/components/merge/`, loaded with a dynamic `import()` so the
      main bundle does not grow).
      - The panes use CodeMirror 6: `@codemirror/state`, `view`, `commands` and `search`, plus
        `@codemirror/merge` for `Chunk.build`, `Chunk.updateA` and `Chunk.updateB`.
      - CodeMirror's own two-pane `MergeView` is not used; the three panes, the curves and the
        arrows are drawn by StashPanda.
      - Why CodeMirror: it is modular (about 250 kB for these packages, to be measured), it
        draws only the lines on screen, and it gives undo, search and multiple cursors for free.
        Monaco is several megabytes and needs its own workers. A hand-made editable pane would
        have to build cursor movement, selection and input-method support from nothing.
      - Syntax colours are applied as CodeMirror mark decorations, built from the Shiki tokens.
        The result pane asks the worker again 300 ms after the last edit.
      - Each conflict block is tracked as a range in the result document, in a CodeMirror
        `StateField`, so it moves correctly as the user types.
      - An unresolved block with no base text shows a one-line placeholder: "Unresolved: take
        dev's or feature/something's version, or type here".
      - A block counts as resolved once an arrow fills it or the user types in it.

      **Step 3: connecting curves.**
      - A canvas sits in each gap between two panes, about 40 px wide.
      - The differences between the left pane and the result, and between the result and the
        right pane, come from `Chunk.build`. After each edit to the result they are updated with
        `Chunk.updateA` or `updateB`, rather than compared again from the start.
      - Each difference is drawn as a filled shape joining its lines in one pane to its lines in
        the next. The top and bottom edges are cubic Bézier curves with horizontal ends (control
        points at half the gap's width), as in the reference screenshot.
      - A difference that is empty on one side narrows to a line at the point where the text
        would go.
      - Colours come from the existing diff tokens: added, removed, changed, and
        `--state-conflict` for conflict blocks. The same colours are drawn behind those lines in
        the panes, as CodeMirror line decorations.
      - The canvases are redrawn in a `requestAnimationFrame` after any scroll, edit or resize.
        Only the differences that are on screen are drawn, at the screen's device pixel ratio.

      **Step 4: arrows in the gaps.**
      - Next to each difference, the left gap has "»", which copies the left pane's lines into
        the result over that block. The right gap has "«", which does the same from the right.
      - For a conflict block, this replaces the block with that side's version, which is the
        quick overwrite. Ctrl-click, or the arrow's right-click menu, offers "Add below the
        current text" instead, for keeping both sides.
      - "×" dismisses one side's difference: its arrow goes and its curve is drawn faded.
      - Every arrow is one undoable CodeMirror transaction, so Ctrl+Z puts the block back.
      - A toolbar above the panes has:
        - Previous and Next conflict;
        - "Take dev for all conflicts" and "Take feature/something for all conflicts", using the
          real names;
        - a "3 of 5 conflicts left" count;
        - Start again from Git's merge;
        - a Sync scrolling toggle.

      **Step 5: synchronised scrolling.**
      - All three panes have the same line height, so their positions can be compared in lines.
      - A function `mapLine(chunks, line)` in `src/lib/merge/scroll.ts` maps a line in one pane
        to the matching line in the next:
        - between differences it moves line for line;
        - inside a difference it moves in proportion, so a 4-line block on one side and a
          13-line block on the other line up at their tops and at their bottoms.
      - The pane the user scrolls (by wheel, scroll bar, keyboard or the cursor moving) leads,
        and the other two follow through `mapLine`. The line at the middle of the leading pane is
        used as the anchor, so the differences line up where the eye is.
      - A flag set during the following panes' own scroll events stops them leading in turn.
      - Horizontal scrolling moves all three panes together.
      - The toggle is kept in the UI state (`mergeEditor.syncScroll`), on by default.

      **Step 6: saving.**
      - The result is written back with the file's own line endings (CodeMirror holds `\n`
        internally), its byte order mark and its final newline as they were. A file with mixed
        line endings is saved with its most common one, and the editor says so before saving.
      - A block still unresolved when saving is written back with Git's markers around both
        sides (and the base, where it was known), so a conflict is never dropped silently. The
        existing "Mark as resolved" check then still catches it.
      - The buttons:
        - Save;
        - "Save and Mark Resolved", which saves, then runs `op_stage`, and is disabled while any
          block is unresolved, with the reason in its tooltip;
        - "Save, Resolve and Open Next", which does the same and then opens the next conflicted
          file;
        - Cancel, which asks before discarding edits.
      - Keyboard:
        - F7 and Shift+F7: next and previous conflict. (Alt+Up and Alt+Down are not used,
          because CodeMirror's default keymap already uses them to move a line up and down.)
        - Alt+Shift+Left and Alt+Shift+Right: take the left or right version for the block at
          the cursor. (Plain Alt+Left can act as the browser's Back key in WebView2; check
          these keys in the Windows window before settling on them.)
        - Ctrl+S: save.

      **Step 7, after the rest works.**
      - Folding unchanged runs of more than 8 lines into a "153 lines hidden · show" row in all
        three panes at once, as in the reference screenshot.
      - Marks on the scroll bars for each difference and conflict.

      **Tests.**
      - Rust:
        - marker parsing on files produced by real merges in all three marker styles, with CRLF
          lines, a byte order mark, no final newline, a `conflict-marker-size` attribute, and a
          lone marker-like line in the code;
        - `write_file` on each transport, keeping the file's mode;
        - the changed-on-disk refusal;
        - `op_restore_conflict`.
      - Frontend: the project has no frontend unit tests yet. This would add Vitest, for the
        pure functions in `src/lib/merge/`: `mapLine`, writing unresolved blocks back with
        markers, and keeping line endings.
      - In the Linux app through WebDriver, on the `conflict` test repository:
        - check the pane headers show the branch names;
        - take the left side for one block and the right side for another;
        - type into a third block;
        - save, and compare the file with what was expected;
        - scroll the middle pane and check the other two panes' positions;
        - take a screenshot of the curves.
      - On Windows: look at the curves and the scrolling in the real window.
- [ ] **SSH tier 2 fallback** (`transport/ssh_exec.rs`): long-lived `ssh -T host sh` helper with
      sentinel framing, only if a real host needs `ProxyJump`, `ProxyCommand`, GSSAPI, certificates
      or an algorithm `russh` cannot negotiate. `resolve_target` currently returns
      `SshUnsupportedFeature` for ProxyCommand and ProxyJump.
- [ ] **Native `ProxyJump`** by nesting a `russh` connection over a `direct-tcpip` channel, as an
      alternative to tier 2 for the jump-host case.
- [x] **Console filter buttons remembered** (2026-09-30). The User, Internal and Watcher buttons
      are kept in the UI state (`console.*` rows in the database), not in a setting; the unused
      `console.showInternal` setting is left out of the Settings dialog.
- [x] **File history and blame** (2026-10-14). Both open from the Files panel's right-click menu
      (Show History, Blame) and from History and Blame buttons in the diff panel's header, and
      work over every transport.
      - `FileHistoryDialog.vue` (`get_file_history`, `git/queries/history.rs`): every commit
        that changed the file, newest first, following renames, with the file's path in each
        commit. The selected commit's diff of the file shows below the list; Show in Commit Graph
        and Blame at This Commit act on it. It runs `git log --follow --cc --name-status -z`.
        **Found:** `git log --follow` prints nothing at all when given `--skip`, so a long history
        is read 500 commits at a time by asking again with a larger `--max-count`. Without
        `--cc`, `--follow` left out every merge, even one that resolved a conflict in the file.
      - `BlameDialog.vue` (`get_file_blame`, `git/queries/blame.rs`, which parses `git blame
        --porcelain`): the file's lines with the commit that last changed each block, shaded by
        how recent it is, for the working tree version or a commit's. Blame Before This Change
        blames the version the lines came from, and Back returns. The version is read first
        through the long-lived `cat-file`, so a binary or too large file is never blamed.
      Tests: the parsers on captured output (renames, tabs, non-ASCII text, a boundary commit,
      uncommitted lines, quoted names, CRLF lines, a merge) and
      `file_history_and_blame_against_a_real_repository`.
      **Fixed 2026-10-14:** `--follow` turns off the history simplification that drops merges
      from a plain `git log -- <path>`, so the history listed every merge on the way, with an
      empty file list and "No differences to show" (a test repository's `assets/scripts/rellax.js`
      had 11 entries, 9 of them such merges). `parse_file_history` now leaves out a merge with no
      file list, and `get_file_history` asks again for twice as many commits when that leaves
      fewer entries than asked for.
- [x] **Automatic fetch** (2026-10-14) on `polling.autoFetchMinutes`. Every half minute, each open
      repository whose last fetch is that old fetches quietly (`autoFetchDue` in
      `lib/repoActions.ts`). A failure is reported once, and not again until a fetch works.
- [x] **SSH reconnect with backoff** (2026-10-14, `transport/ssh_pool.rs`). After a network
      failure the pool waits 1, 2, 5, 10, 30 and then 60 s between attempts, and fails at once
      in between rather than blocking. It reports Connecting, Ready, Degraded or Disconnected as
      a `transport_state` event, which the status bar shows with a countdown to the next
      attempt. When the connection is back, the remote `inotifywait` watcher starts again, and
      a graph whose first load failed loads by itself (`GraphLoadFailedEvent`, where it used to
      show "Loading" for ever). Test: `reconnects_with_backoff_after_the_server_goes_away`.
- [x] **Syntax colours in the diff** (2026-10-14). Shiki colours the diff in a Web Worker
      (`lib/diff/syntax.worker.ts`), using its JavaScript regular expression engine, so no
      WebAssembly is loaded. The language comes from the file's name or extension, from a list
      of 62 common languages; any other file stays plain. Each hunk's old side and new side go
      to the worker as whole runs, so a comment or string spanning several lines is coloured
      correctly, and `mergeSpans` in `lib/diff/syntax.ts` cuts the colours and the
      changed-word marks into one set of pieces. Diffs over 20,000 lines stay plain. Each
      grammar loads the first time a file of its language is shown. **Found:** importing
      Shiki's full language list put all 346 grammars into the build as separate files, about
      10 MB more in the program, because Tauri builds every asset into it; the 62 chosen ones
      add about 0.5 MB after Tauri's compression. Settings › Diff › "Syntax Highlighting" turns it
      off, and the Settings preview follows the box. Tried in the running Linux build on
      2026-10-14 with TypeScript, Rust and an unknown extension.
- [x] **Curves in the side-by-side diff** (2026-10-14). Side by side no longer pads the shorter
      side of a change with blank rows. Each side shows only its own lines, and a 36 px gap
      between them draws each change as a band from its old lines to its new lines, with curved
      top and bottom edges (`src/lib/diff/sideLayout.ts`).
      - A band has the changed lines' own background, outlined in the stronger changed-word
        colour. A deletion is red, an insertion green, and a change with lines on both sides
        fades from red to green. Hunk headers are joined by a band in the hunk colour.
      - Where a change has no lines on one side, the band narrows to a point, and a line across
        that side marks where the lines would go.
      - Scrolling: both sides follow one scrollbar, measured in rows, where a change takes as
        many rows as its longer side. Inside a change the shorter side moves more slowly
        (`sideLine`). The two sides are lined up at a point that moves from the top of the view
        to its bottom as the view scrolls from start to end (`sideOffsets`), so both sides start
        at the top together and finish at the bottom together.
      - The curves are inline SVG over the rows, not a canvas. Chromium gives a canvas a
        compositor layer of its own, and text near such a layer can lose ClearType (see
        "Font smoothing setting" and "ClearType in dialogs" above).
      - The divider's line now shows only on hover and while dragging, so it does not cut
        through the curves. A "No newline at end of file" note goes on the side it is about.
      Tried in the running Linux build on 2026-10-14: an insertion, a deletion, three lines
      changed into one, picking a line by its number, Stage hunk, hunk navigation and switching
      to Unified. Not yet looked at in the Windows window.
- [x] **Find in a diff view and in Blame** (2026-10-14). Ctrl+F with the keyboard focus in a
      diff view (the main panel's, or the File History dialog's), or anywhere in the Blame
      dialog, opens a Find box in its top right corner. The two share `useFind`
      (`src/composables/useFind.ts`) and `FindBox.vue`; each view only collects its matches and
      scrolls to one.
      - Every match is drawn in purple, a stronger magenta than the hunk headers' blue-violet,
        and the current one outlined in the same purple, which keeps its text readable. Case is
        ignored.
      - The count reads "3 of 12". Enter and the > button go to the next match, and Shift+Enter
        and < to the previous one, wrapping around at the ends. Escape closes the box and gives
        the focus back to the diff.
      - Typing picks the first match from the top of the view down. A match off screen is
        scrolled to the middle of the view, and one past the edge of its text area is scrolled
        to sideways (side by side, on its own side).
      - The diff view's `<section>` takes the focus when clicked (`tabindex="-1"`) and marks
        Ctrl+F handled, so the Commits panel's window-wide Ctrl+F leaves it alone. That
        listener now also ignores Ctrl+F while a dialog is open.
      - The matching is in `src/lib/diff/find.ts`. `markFound` cuts a line's coloured pieces at
        the edges of the matches, so a match inside a changed word stays inside its highlight.
        Past 10,000 matches the rest are neither drawn nor counted, and the count ends in "+".
      - The Blame dialog's lines now have the diff viewer's syntax colours too, the whole file
        coloured as one run, and follow the "Syntax Highlighting" setting.
      Tried in the running Linux build on 2026-10-14, in both diff layouts and in Blame: wrapping
      both ways, a match on one side only, and a match at the end of a 300-character line.

## Packaging and CI

- [x] **Tests on Windows** (fixed 2026-10-14). The Windows test binary exited with
      `STATUS_ENTRYPOINT_NOT_FOUND` before any test ran: it imports `TaskDialogIndirect`, which
      only version 6 of `comctl32.dll` exports, and Windows loads version 6 only for a program
      whose manifest asks for it. tauri-build embedded the manifest in the app binary only
      (`rustc-link-arg-bins`). `src-tauri/build.rs` now passes
      `src-tauri/windows-app-manifest.xml` to the MSVC linker for every binary, test binaries
      included, and tells tauri-build not to embed its own copy. One test,
      `stage_unstage_and_discard`, used `:colon.txt` and `*.txt`, which Windows does not allow;
      there it uses `a.txt` and `[ab].txt`. All 73 tests pass on Windows and Linux. Run them
      from WSL with `cmd.exe /c "cd /d C:\src\stashpanda\src-tauri && cargo test"` after
      `npm run win:sync`.

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

- `CatFile::read` (`git/catfile.rs`) breaks when its future is dropped part way through a
  request: the answer stays in the helper's output and the next request reads it as its own.
  `get_conflict_file` avoids it by never cancelling a read (`tokio::join!`, not `try_join!`);
  any other caller that cancels a read could hit it. Found 2026-10-14.

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
- On the WSL transport, a command without `Cmd::own_session` that times out is stopped by killing
  `wsl.exe`. That does end the Linux processes it started (measured 2026-10-01), but abruptly, so
  Git cannot remove its lock files. Only the pull's fetch uses `own_session` so far.
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
