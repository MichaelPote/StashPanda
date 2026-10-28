# The changelog

`CHANGELOG.md` at the root of the repository lists every change to StashPanda, newest first. It is
also where each GitHub release's notes come from, so it has to be kept up to date as work is
committed, not written at release time.

## The rule: every commit adds a line under `## Unreleased`

The top section of `CHANGELOG.md` is always `## Unreleased`. Every commit that changes what the
app does, or how it is built or released, adds one line there, in the same commit:

```markdown
## Unreleased

- Give the SSH Keys page a tab for each place, and a Copy to… button that never replaces a key
- Treat a drive letter mapped to WSL as the WSL folder it points to

## 0.6.4 (2026-10-28)
```

- **Newest first:** put the new line at the top of the `## Unreleased` list.
- **One line per change,** written the way a commit's subject line is: what now happens, in plain
  words, starting with a verb ("Add", "Fix", "Stop", "Keep", "Make"). Several commits for one
  feature can share a line: update it rather than adding a second one.
- **Never rename or remove `## Unreleased` by hand,** and never write a version heading yourself.
  `npm version` does both (see below). If `## Unreleased` is missing, `npm version` stops.
- **Changes nobody would notice** (a refactor with no change in behaviour, a test on its own, a
  typo in a comment) need no line.
- **No private details:** like everything committed here, the lines must not name client or test
  repositories, folders on a developer's own machine, people, or other Git GUIs.

## What happens at a release

1. `npm version <x.y.z>` bumps the version. Its `version` script in `package.json` runs
   `scripts/sync-version.mjs` (the version in `tauri.conf.json`, `Cargo.toml` and `Cargo.lock`)
   and then `scripts/changelog-version.mjs`, which renames `## Unreleased` to
   `## <x.y.z> (<today's date>)` and puts a new, empty `## Unreleased` above it. `CHANGELOG.md`
   goes into the version's commit and tag with the other version files.
   `scripts/changelog-version.mjs` stops with an error when the changelog has no
   `## Unreleased`, or already has a section for that version.
2. Pushing the tag (`git push --follow-tags`) runs `.github/workflows/build.yml`. So does Run
   workflow on the Actions tab, which needs no tag: its draft is for the commit it built, and
   publishing it makes the tag. Its release job takes the release notes from that version's
   section with `scripts/release-notes.mjs <x.y.z>`, and creates the draft release with them. A
   version whose `CHANGELOG.md` has no lines for it gets GitHub's notes made from the commits
   instead, with a warning in the job's log.
3. The draft is read over, and its notes edited if need be, on the Releases page, then
   published. A rerun of the release job for a tag that already has a release only uploads the
   files again, so it never replaces notes edited there.

To see the notes a version would get: `node scripts/release-notes.mjs 0.6.4`.
