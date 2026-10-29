# Changelog

Every change to StashPanda, newest first. Each commit adds its line under Unreleased, and a
release moves those lines under its version number.

## Unreleased

- Give the error and warning bars a full-size Close key in their own colour, keys the size of the merge banner's, and a larger icon at the top
- Toggle the commit list's columns and column headings from a right-click on the Commits pane's header, which also counts the rows selected when there is more than one
- Enable the Push button again as soon as a push or sync finishes, instead of only after something else redraws the toolbar

## 0.6.5 (2026-10-29)

- Rename and move tags from their right-click menu, choosing the commit from a searchable list, and remove, rename or move them on remotes too, with each remote checked in the background for whether it has the tag
- Offer to move a tag in Create Tag only when the name is already taken, saying which commit it moves from

## 0.6.4 (2026-10-28)

- Make a draft release from a run of the build started by hand, for the version in package.json
- Build the installers on GitHub only for a version tag or a run started by hand; other pushes run just the tests
- Take each GitHub release's notes from CHANGELOG.md, whose Unreleased section npm version now turns into the version's
- Give the SSH Keys page a tab for Windows, each WSL distro and each server, and a Copy to… button that never replaces a key
- Never overwrite a key or its .pub file when exporting, generating or importing one
- Stop a long menu scrolling the whole window up when the pointer moves over its items
- Keep the chosen file when clicking the commit that is already selected
- Use medium-weight text on every button
- Add Refresh Window to the Developer menu of debug builds
- Push every branch that tracks a remote branch at once, from the Repository menu or an optional toolbar button
- Ink branch names in the right-click menus in their lane colour
- Share the commit's menu items with the branch and tag menus, which gain Add Tag, Cherry-pick, Revert and Reset
- Start the toolbar with Repository Settings and Settings on the right, Fetch before Refresh, and no Sync
- Add an SSH Diagnosis dialog, opened from the notice when a remote refuses Git's SSH key
- Make the conflict buttons stand out as large keys: yellow, with a red Abort, a purple Merge Editor and an accent-coloured Open, and Take buttons named after each branch's last part and the side it is shown on
- Give the window the same App User Model ID as the Start menu shortcut
- Treat a drive letter mapped to WSL as the WSL folder it points to

## 0.6.3 (2026-10-26)

- Remove the banner image from the top of the README
- Update the app automatically from the GitHub releases
- Add a Repository Settings dialog for Git's own settings

## 0.6.2 (2026-10-25)

- Let the remotes test allow the remote HEAD ref newer Git creates at each fetch
- Add, edit, rename and remove remotes from the app
- Attach every installer to the release, and let a rerun finish a half-made one
- Emboldened the early development warning.

## 0.6.1 (2026-10-25)

- Documentation update
- Add an AI disclosure
- Update russh to 0.64.1
- Drop the shadcn-vue package and update Vite, clearing the npm security alerts
- Added repo banner
- Create a draft GitHub Release with the installers for each version tag
- Push tags from the app, sort annotated tags by date, and show a tag's annotation on hover
- Recognise newer Git's autostash conflict message, and fix the tests on GitHub's runner
- Prepare for public release: MIT licence, new identifier, rewritten docs and screenshots
- Clear the web view's caches in debug builds, and stop WebView2 downloading components
- Give the Add Repository tabs borders and the accent colour, and reword some messages
- Read the Windows mirror folder from a .env file, and clear old builds
- Use the new app icon, and rebuild the .exe's icon when it changes

## 0.6.0 (2026-10-25)

- Rename branches on the remote, push to new branches, and choose syntax colours

## 0.5.0 (2026-10-22)

- Let the commit message use the Files panel's whole width when expanded
- Replace the native title tooltips with the app's own, and drop the pedantic ones
- Make the Console's Clear a text button and its close button an X
- Give every toolbar button a proper tooltip: its name, then what it does
- Fit the diff's line-number gutter to the lines it numbers, down to the bottom
- Search the commits from a box in the Commits panel's header
- Add a Zoom setting, on the General page now called Appearance
- Draw the 12px icons on whole pixels, so their lines come out solid
- Give the window the theme's background colour instead of white
- Load only the tab on screen when the app starts
- Show each repository's checked-out branch in the Repositories list
- Show a running fetch, pull, push or sync on its toolbar button
- Keep every panel header on one line
- Save toolbar changes with the rest of Settings, and drop them on Cancel
- Let the commit list show only the columns and author details wanted
- Tidy the Branches panel's indentation, tracking labels and badges
- Put the side-by-side diff's divider on the new side's border
- Make stopping npm run win:ux stop Vite and the sync loop
- Make the whole gap between the sides of the side-by-side diff drag the divider
- Fade a removed line's text in the unified diff
- Leave a gap before the lanes when the graph is drawn inside the commit message column
- Add a setting to hide the commit list's column headings, and put the commit message first
- Indent branch folders a level deeper, and shrink long branch names from the left
- Let Merge and Rebase stash uncommitted changes instead of refusing to start
- Add "Fast-forward to origin/…" to a local branch's menu

## 0.4.0 (2026-10-21)

- Show the app's real version in the About dialog
- Add a GitHub Actions workflow that tests and builds the Linux and Windows installers
- Build only the NSIS installer on Windows, not the MSI
- Prepare StashPanda for macOS: finding Git, PATH, the menu bar and quitting
- Fit the commit table's columns into the room there is, so the messages always show
- Ignore a second click on Sync or Push while the first is getting ready
- Report a rebase as done when only an extra branch could not be moved with it
- Lead failure messages with Git's reason instead of its progress output
- Remove remote branches that were deleted on the server when fetching
- Push where git push would, explain refused pushes, and add Push To with a checked force push
- Warn when the new remote branch named in the Push dialog already exists
- Refuse to change a branch other than the one the dialog named
- Refuse to delete a remote branch that has commits this repository has not fetched
- Keep merge commits when pull.rebase is set to merges
- Wait for Git before closing the window, and recognise a rebase that was cut off
- Add a setting that draws the graph inside the commit message column
- Show what each Git command does at the start of its console row
- Add a script that builds test repositories on a server for trying push and pull

## 0.3.0 (2026-10-16)

- Set a branch's upstream as its own step, so a locked config cannot leave it half done
- Record the Git command review and the push.default dialog in the docs
- Fix the Git command bugs found in review, and ask before pushing to another branch name
- Bump the version everywhere with npm version
- Record the branch, toolbar and colour work in the docs
- Size the lane graph column to the lanes again when their number changes
- Turn off the browser's right-click menu and keyboard shortcuts
- Let the toolbar be customised
- Start a graph's automatic lane colours from the repository's colour
- Stop the dev server reloading the app when an agent's worktree changes
- Check out a branch by double-clicking its pill in the graph
- Take the lane colours from the Stashpanda Design System
- Choose a branch's upstream and delete remote branches from the branch menu
- Run only the tests in src
- Branch folders and heading menus in the Branches panel
- Mark the checked-out branch's pill with a HEAD ring and a sheen

## 0.2.0 (2026-10-14)

- Let dialogs fill the window
- Keep a diff's syntax colours to its own language while the next file loads
- Show an added or deleted file's diff unified, whatever layout is chosen
- Give the toolbar's Commit button its fill's text colour
- Choose colours for branches, tags and repositories

## 0.1.0 (2026-10-14)

- Record the merge editor's check in the Windows app
- Give the merge editor's panes the app's thin scroll bars
- Forget dismissed differences and opened folds when the merge editor moves to another file
- Record the merge editor's additions in the docs
- Find in the merge editor, and start it at the first conflict
- Add Vitest and unit tests for the merge editor and diff helpers
- Show empty conflicts, mark the scroll bars, and name the stash in the merge editor
- Report a conflicted file's marker length and which line ending to save it with
- Dismiss a difference and fold unchanged lines in the merge editor
- Fall back to a fresh cat-file when the long-lived one cannot find a conflict's version
- Record the merge editor in the docs
- Resolve a conflicted file in a three-pane merge editor
- Read a conflict's versions by object ID, not by stage name
- Read, save and restore a conflicted file for the merge editor
- Write a file in place on every transport
- Make the diff's added and deleted backgrounds a touch stronger
- End a diff band's gradient in its lines' own background
- Close the gaps between neighbouring diff bands
- Outline only whole insertions and deletions in the side-by-side curves
- Syntax colours and Find in the Blame dialog
- Draw the current Find match in the same dimmer purple as the others
- Find in a diff view with Ctrl+F
- Make the fill of a diff band with lines on both sides a touch stronger
- Leave merges that did not change a file out of its history
- Match each removed line with the added line most like it
- Let changed-word highlights reach 1px past their text on every side
- Draw each run of changed words as one highlight
- Fade a diff band's edges towards its block of lines
- Keep the diff curves inside the rows, and bring each band's point onto its line
- Join the two sides of the side-by-side diff with curves
- Plan the three-pane merge editor in milestone 3
- Record milestone 3 in the docs
- Colour the code in the diff by its language
- Finish a merge from the Commit dialog, select the next conflict, and clean the Console
- Merge conflict resolution, Merge and Rebase
- Resolve merge, rebase, cherry-pick and revert conflicts, with Continue, Skip and Abort
- Merge the file history and blame views
- Fetch automatically, reconnect SSH with backoff, and recover a graph whose load failed
- Add file history and blame views
- Select the first file when a commit or the working tree is selected
- Show standard error in the Console's Log mode, where push and fetch report
- Keep Add Repository from opening under another dialog, and add a WebView2 debug port
- Ask for a password, not a default key's passphrase, when the server takes passwords
- Record the rest of milestone 1 in next-steps.md and architecture.md
- Make the Git common directory absolute, and explain greyed-out key actions
- Merge the incremental graph refresh, early paints and Find
- Merge the SSH keys page, SSH prompts and keychain passphrases
- Refresh the graph incrementally, paint large caches and live loads early, and add Find
- Add the SSH Keys page, SSH prompts on open, and passphrases in the system keychain
- Show the watcher in the status bar from the start, and after a reload
- Stop polling when the repository's activity sender is gone, instead of spinning
- Watch remote repositories with inotifywait, batch WSL refreshes, and pin cat-file
- Finish milestone 2: reset, cherry-pick, revert, rename, line staging and file actions
- Run the tests on Windows, and fix dialogs and selects in the Linux build
- Add Sync and Clone, success messages in the status bar, and ClearType fixes
- Add Fetch and Push, Commit & Push, and progress in the status bar
- Add Pull with streamed progress and Stop, and stop commands cleanly
- Ignore TypeScript build output, and bring next-steps.md up to date
- Redesign ref pills and lane colours, name merge parents, restyle the toolbar
- Hide the Windows console window and copy Rust changes in ux mode
- Make dialogs resizable with one shared layout, and edit library entries
- Add a Log mode to the Console, and keep less command output
- Keep remembered UI state in the SQLite database
- Add a Settings dialog, with diff font, size and tab width settings
- Add stashes: save, apply, pop and drop
- Show the file name and its folder as columns in the Files panel
- Keep the case of folder names in the Files panel headings
- Size the Author, Date and ID columns to their contents
- Keep the commit graph's dots centred on their rows
- Add a diff line height setting and tighten the diff rows
- Move the commit header's More toggle under the date
- Keep the View > Theme ticks in step with the theme setting
- Bundle the fonts with the app instead of loading Google Fonts
- Branch show and hide in the graph, file grouping and sorting, and UI fixes
- Make the Commit dialog larger and darken the dialog background
- Stop the Windows scripts exiting when port 1420 is free
- Add the Commit dialog and multi-select in the file lists
- Fix every clippy warning
- New design system. Windows build improvements
- Add stage, unstage, discard and commit operations
- initial commit
