# StashPanda

A standalone desktop Git GUI that runs the Git you already have. It does not bundle Git; it works
with your installed Git and its configuration, treats repositories inside WSL and on remote hosts
over SSH as first class, paints from a local cache before Git answers, and opens many repositories
at once in tabs. Built with [Tauri 2](https://tauri.app), [Vue 3](https://vuejs.org),
[Tailwind CSS 4](https://tailwindcss.com) and [shadcn-vue](https://www.shadcn-vue.com).

The architecture, the decisions behind it and the implementation status are in
[docs/architecture.md](docs/architecture.md).

## Layout

- `src/` – the Vue frontend, built by Vite.
  - `src/lib/bindings.ts` – **generated** TypeScript bindings for every Rust command and event.
    Regenerate with `cd src-tauri && cargo test export_bindings`; never edit by hand.
  - `src/stores/` – Pinia stores: settings, UI state, repository library, open sessions.
  - `src/components/shell/` – toolbar, repository and branch panels, tab strip, console, dialogs.
  - `src/components/graph/`, `files/`, `diff/` – the commit graph, file lists and diff viewer.
  - `src/components/ui/` – shadcn-vue components, added with `npx shadcn-vue@latest add <name>`.
- `src-tauri/src/` – the Rust side. `transport/` reaches Git locally, through `wsl.exe` or over
  SSH; `git/` runs and parses Git commands; `graph/` orders and lays out the commit graph;
  `storage/` is the SQLite cache; `watch/` detects changes; `keys/` is the SSH key manager;
  `ipc/` holds the Tauri commands.
- `docs/architecture.md` – the plan and the notes on what changed while building it.

## Commands

```bash
npm install            # install frontend dependencies
npm run dev            # frontend only, in the browser at http://localhost:1420
npm run build          # type-check with vue-tsc, then build the frontend into dist/
npm run tauri dev      # run the desktop app with hot reload
npm run tauri build    # build the release binary and installers
cd src-tauri && cargo test   # Rust unit and end-to-end tests (36), also regenerates bindings.ts
```

## Running on Linux and WSL

The Linux build runs inside WSL and appears on the Windows desktop through WSLg:

```bash
npm run tauri dev
```

Settings live in `~/.config/com.bravedigital.stashpanda/` (`settings.json` and `ui-state.json`)
and the commit cache in `~/.local/share/com.bravedigital.stashpanda/stashpanda.sqlite`.

## Building for Windows

The Windows binary is built natively on Windows; cross-compiling from WSL is not set up.

1. Install [rustup](https://rustup.rs) (MSVC toolchain) and the Visual Studio Build Tools with the
   "Desktop development with C++" workload. WebView2 is already present on Windows 11.
2. Install Node.js 20 or newer.
3. Check the repository out on the Windows filesystem (for example `C:\src\stashpanda`), not
   through `\\wsl.localhost`, which is very slow for Cargo.
4. In PowerShell: `npm install`, then `npm run tauri dev` or `npm run tauri build`.

The `russh` SSH client is built with the `ring` backend, so no CMake or NASM is required.

## Prerequisites

- Node.js 20 or newer.
- The Rust toolchain, installed with [rustup](https://rustup.rs).
- The Tauri system libraries for your platform: https://tauri.app/start/prerequisites/
- Git 2.36 or newer on whichever machine or WSL distro holds the repositories.

## Working on the Windows version from WSL

The source lives on the WSL filesystem, but `node_modules/` and `src-tauri/target/` hold
platform-specific binaries, so Windows cannot build from the same folder. `scripts/windows.sh`
mirrors the source (without those folders) to `C:\src\stashpanda` with `rsync`, then runs the
Windows toolchain there through WSL interop. Run these from a WSL shell:

```bash
npm run win:ux       # UX work: Vite runs in WSL, the Windows app loads from it, edits hot reload
npm run win:dev      # mirror, then run the desktop app entirely on Windows (Vite included)
npm run win:build    # mirror, build the Windows release binary and installers, copy them to bundle/ in both folders
npm run win:sync     # mirror the source and run npm install on Windows, nothing else
```

### How `win:ux` works

1. The source is mirrored to `C:\src\stashpanda` and `npm install` runs there.
2. Vite starts in WSL on port 1420. WSL2 forwards Windows `localhost:1420` to it.
3. The Windows `tauri dev` starts with `src-tauri/tauri.wsl-dev.conf.json` merged in, which empties
   `beforeDevCommand` so Windows does not start its own Vite. The app's WebView loads
   `http://localhost:1420`, which is the WSL Vite server.
4. Edits to Vue, TypeScript and CSS files in WSL hot reload in the running Windows app. Rust changes
   need Ctrl-C and a re-run, because the Rust code is compiled from the mirror.
5. Ctrl-C stops Vite and kills the whole Windows process tree (npm, the Tauri CLI, Cargo and the
   app). The Windows side's output is written to `tauri-dev.log` and `tauri-dev.err.log` in the
   mirror folder and streamed into the WSL terminal.

The mirror is one-way. Edit the code in WSL and re-run the script; edits made in the mirror are
overwritten on the next sync.

### Windows prerequisites

All present on Michael's machine as of 2026-09-21: rustup with the `stable-x86_64-pc-windows-msvc`
toolchain, Node.js, Visual Studio with the "Desktop development with C++" workload **and** a
Windows 10/11 SDK component, and the WebView2 runtime. Without the SDK component, Rust cannot find
the MSVC linker and falls back to Git's `link.exe`, which fails with `link: extra operand`.
