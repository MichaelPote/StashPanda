# Notes for coding agents

- **Every commit that changes what StashPanda does adds a line under `## Unreleased` in
  `CHANGELOG.md`.** Never rename that heading or write a version heading by hand: `npm version`
  does it, and the GitHub release's notes are taken from the result. The rules are in
  [docs/changelog.md](docs/changelog.md).
- How the app works, and the pitfalls already found, are in
  [docs/architecture.md](docs/architecture.md). What is still to build is in
  [docs/next-steps.md](docs/next-steps.md).
