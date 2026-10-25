#!/usr/bin/env bash
# Build or run the Windows version of StashPanda from inside WSL.
#
# The project lives on the WSL filesystem, but node_modules and the Rust
# target directory contain platform-specific binaries, so Windows cannot build
# from the same folder. This script mirrors the source to a folder on the
# Windows filesystem and runs the Windows toolchain (Node, Cargo, MSVC) there.
#
#   scripts/windows.sh ux      # frontend hot reload: Vite runs here in WSL, the
#                              # Windows app loads from it. Edit in WSL, see it live.
#   scripts/windows.sh dev     # sync, then run the app fully on Windows (Vite too)
#   scripts/windows.sh build   # sync, build the release binary and installers,
#                              # copy them to bundle/ here and in the Windows mirror,
#                              # then remove the release build's intermediate files
#   scripts/windows.sh sync    # mirror the source and run npm install on Windows
#
# The mirror is one-way: edit the code in WSL, then re-run this script.
# In `ux` mode nothing needs a re-run: Vue, TypeScript and CSS changes hot
# reload in the running app, and src-tauri/ keeps being copied to the mirror,
# so the Windows `tauri dev` rebuilds and restarts the app after a Rust change.
#
# The Windows folder comes from STASHPANDA_WIN_DIR, set in the environment or in
# the repository's .env file (see .env.example).
set -euo pipefail

SRC_DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$SRC_DIR/scripts/load-env.sh"
load_env "$SRC_DIR/.env"
if [ -z "${STASHPANDA_WIN_DIR:-}" ]; then
  echo "STASHPANDA_WIN_DIR is not set. Copy .env.example to .env and set it to the Windows" >&2
  echo "folder to mirror the source to and build in." >&2
  exit 1
fi
WIN_DIR="$STASHPANDA_WIN_DIR"
WSL_DIR="$(wslpath -u "$WIN_DIR")"
ACTION="${1:-ux}"
VITE_PORT=1420

mkdir -p "$WSL_DIR"

echo "Syncing $SRC_DIR -> $WIN_DIR"
rsync -a --delete \
  --exclude '.git' \
  --exclude '.claude' \
  --exclude 'node_modules' \
  --exclude 'dist' \
  --exclude '/bundle' \
  --exclude 'src-tauri/target' \
  --exclude 'src-tauri/gen' \
  --exclude 'tauri-dev*.log' \
  "$SRC_DIR/" "$WSL_DIR/"

# cmd.exe cannot start in a WSL (UNC) directory, so start it from C:\ instead.
run_win() {
  (cd /mnt/c && cmd.exe /c "cd /d $WIN_DIR && $1") | tr -d '\r'
}

# Start a Windows command fully detached from this shell and print its Windows
# PID, so we can kill its whole process tree later with taskkill. WMI's
# Win32_Process.Create shares no handles with us, so the call returns at once;
# PowerShell's Start-Process does not return until the child exits when it is
# launched through WSL interop.
# ShowWindow = 0 hides the console window Windows would otherwise open for the
# new cmd.exe. Nothing is written to it, since all output goes to the log
# files, so it would only be an empty window titled after npm's last command.
# The app's own window still shows: npm starts its children with the default
# window mode, so the hidden setting stops at the first node process.
start_win_detached() {
  local command="$1"
  (cd /mnt/c && powershell.exe -NoProfile -NonInteractive -Command \
    "\$startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ ShowWindow = [uint16]0 }; (Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = 'cmd.exe /c \"cd /d $WIN_DIR && $command\"'; CurrentDirectory = '$WIN_DIR'; ProcessStartupInformation = \$startup }).ProcessId") | tr -d '\r'
}

# True while a Windows process is alive. tasklist called through interop
# occasionally returns nothing at all; an empty listing is an interop hiccup,
# not proof the process is gone. Plain bash matching: tasklist output contains
# non-UTF-8 bytes that make some grep wrappers treat it as binary.
win_alive() {
  local procs
  procs="$(cd /mnt/c && cmd.exe /c "tasklist /FO CSV /NH" 2>/dev/null || true)"
  [ -z "$procs" ] || [[ "$procs" == *"\"$1\""* ]]
}

# Kill a Windows process and every process under it. Killing the WSL side
# never does this: Windows children outlive the cmd.exe that WSL started.
# Interop calls occasionally fail silently, so retry until the root is gone.
kill_win_tree() {
  local pid="$1" attempt
  for attempt in 1 2 3 4 5; do
    local killed=0 denied=0 line
    while IFS= read -r line; do
      [[ "$line" == SUCCESS* ]] && killed=$((killed + 1))
      [[ "$line" == *"Access is denied"* ]] && denied=$((denied + 1))
    done < <(cd /mnt/c && cmd.exe /c "taskkill /PID $pid /T /F" 2>&1 | tr -d '\r' || true)
    echo "  killed Windows processes: $killed"
    if [ "$denied" -gt 0 ]; then
      echo "Warning: $denied Windows processes run as Administrator and could not be stopped from here." >&2
      echo "From an elevated prompt run: taskkill /PID $pid /T /F" >&2
      return 0
    fi
    sleep 1
    if ! win_alive "$pid"; then return 0; fi
  done
  echo "Warning: Windows process $pid is still running; kill it with: taskkill /PID $pid /T /F" >&2
}

# "<pid> <name>" for whatever listens on a Windows TCP port, or nothing.
# `wslrelay` means the listener is a process inside WSL.
# When nothing listens, Get-NetTCPConnection reports an error: SilentlyContinue
# hides the message, but powershell.exe still exits with 1, which `set -e`
# would turn into a silent exit of the whole script. No listener is the normal
# answer, so the exit code is ignored and only the output counts.
win_port_owner() {
  (cd /mnt/c && powershell.exe -NoProfile -NonInteractive -Command \
    "Get-NetTCPConnection -LocalPort $1 -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1 | ForEach-Object { '{0} {1}' -f \$_.OwningProcess, (Get-Process -Id \$_.OwningProcess).ProcessName }") | tr -d '\r' || true
}

# Stop before starting when the Windows side of the Vite port is taken,
# usually by a Vite left over from an earlier run.
require_free_win_port() {
  local owner pid name
  owner="$(win_port_owner "$VITE_PORT")"
  [ -z "$owner" ] && return 0
  read -r pid name <<< "$owner"
  echo "Port $VITE_PORT is already in use on Windows by $name (process $pid)." >&2
  if [ "$name" = "wslrelay" ]; then
    echo "The listener is inside WSL. Stop the other Vite there first." >&2
  else
    echo "It is usually a Vite left over from an earlier run. Stop it with:" >&2
    echo "  cmd.exe /c \"taskkill /PID $pid /T /F\"" >&2
    echo "If that says Access is denied, the run was started as Administrator: use an elevated prompt." >&2
  fi
  exit 1
}

# With WEBVIEW_DEBUG_PORT set (`WEBVIEW_DEBUG_PORT=9222 npm run win:ux`), the app's WebView2
# serves the DevTools protocol on that port, on Windows' 127.0.0.1 only, so a script running on
# Windows can inspect the real page (its layer tree, for example). No space before `&&`: cmd
# would keep it in the value.
webview_debug_prefix() {
  if [[ "${WEBVIEW_DEBUG_PORT:-}" =~ ^[0-9]+$ ]]; then
    printf 'set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=%s&& ' "$WEBVIEW_DEBUG_PORT"
  fi
}

# Set by start_win_logged, read by stop_win_logged.
win_pid=""
tail_pid=""
# Set by ux_mode, read by its cleanup. Not local to ux_mode: the cleanup also runs from the EXIT
# trap, after ux_mode has returned, and would then find nothing to stop and leave Vite on the
# port and the sync loop running.
vite_pid=""
sync_pid=""

# Start a Windows command detached, with its output going to tauri-dev.log and
# tauri-dev.err.log in the mirror, and follow both logs here.
start_win_logged() {
  : > "$WSL_DIR/tauri-dev.log"; : > "$WSL_DIR/tauri-dev.err.log"
  win_pid="$(start_win_detached "$1 > tauri-dev.log 2> tauri-dev.err.log")"
  if ! [[ "$win_pid" =~ ^[0-9]+$ ]]; then
    echo "Could not start the Windows process: $win_pid" >&2
    win_pid=""
    exit 1
  fi
  echo "Windows process id: $win_pid"
  # inotify does not fire on the /mnt/c mount, so poll the log files instead.
  # --pid makes tail exit once this script has, however the script ends: a run
  # whose cleanup never happened otherwise leaves its tail echoing every later
  # run's log into the same terminal, so each line shows up once per old run.
  # The `kill` in stop_win_logged only stops tr: $! is the last command in the
  # pipeline.
  tail -n +1 -f ---disable-inotify --sleep-interval=1 --pid=$$ "$WSL_DIR/tauri-dev.log" "$WSL_DIR/tauri-dev.err.log" | tr -d '\r' &
  tail_pid=$!
}

stop_win_logged() {
  if [ -n "$win_pid" ]; then kill_win_tree "$win_pid"; fi
  if [ -n "$tail_pid" ]; then kill "$tail_pid" 2>/dev/null || true; fi
}

# Return once the Windows process has exited, or, when a WSL pid is given,
# once that process has. Requires three misses in a row (see win_alive).
wait_win_exit() {
  local guard_pid="${1:-}" misses=0
  while [ -z "$guard_pid" ] || kill -0 "$guard_pid" 2>/dev/null; do
    if win_alive "$win_pid"; then misses=0; else misses=$((misses + 1)); fi
    if [ "$misses" -ge 3 ]; then
      echo "The Windows app has exited."
      return
    fi
    sleep 3
  done
}

# Copy src-tauri/ to the mirror every few seconds and report what changed. The
# mirror's target/ and gen/ are excluded, which also keeps --delete off them.
# Without this the Windows backend stays at the build from when the script
# started, while Vite hot reloads frontend code that expects the newer IPC
# types; a missing field then reaches the components as undefined.
sync_rust_loop() {
  local changed
  while sleep 2; do
    changed="$(rsync -a --delete --itemize-changes \
      --exclude '/target' --exclude '/gen' \
      "$SRC_DIR/src-tauri/" "$WSL_DIR/src-tauri/" 2>&1 | grep -E '^(>f|\*deleting)' || true)"
    if [ -n "$changed" ]; then
      echo "Copied Rust changes to the Windows mirror; tauri dev will rebuild:"
      echo "$changed" | sed 's/^/  /'
    fi
  done
}

dev_mode() {
  run_win "npm install"
  require_free_win_port

  cleanup() {
    trap - EXIT INT TERM
    echo
    echo "Stopping..."
    stop_win_logged
    echo "Stopped."
  }
  trap cleanup EXIT INT TERM

  echo "Starting the app on Windows (with its own Vite)"
  start_win_logged "npm run tauri dev"
  echo "Press Ctrl-C to stop the Windows app and its Vite."
  wait_win_exit
}

ux_mode() {
  run_win "npm install"

  if [[ "$(ss -ltn)" == *":$VITE_PORT "* ]]; then
    echo "Port $VITE_PORT is already in use in WSL. Stop the other Vite first." >&2
    exit 1
  fi
  # A Windows process on the port would shadow WSL's Vite for the app.
  require_free_win_port

  # Also runs from the EXIT trap, after ux_mode has returned; vite_pid and sync_pid are set at the
  # top of the script for that reason.
  cleanup() {
    trap - EXIT INT TERM
    echo
    echo "Stopping..."
    if [ -n "$sync_pid" ]; then kill "$sync_pid" 2>/dev/null || true; fi
    stop_win_logged
    if [ -n "$vite_pid" ]; then
      kill "$vite_pid" 2>/dev/null || true
      sleep 1
      kill -9 "$vite_pid" 2>/dev/null || true
    fi
    echo "Stopped."
  }
  trap cleanup EXIT INT TERM

  echo "Starting Vite in WSL on port $VITE_PORT"
  # exec so that $! is Vite's own PID rather than a wrapper shell's.
  (cd "$SRC_DIR" && exec ./node_modules/.bin/vite) &
  vite_pid=$!

  for _ in $(seq 1 60); do
    if curl -s -o /dev/null "http://localhost:$VITE_PORT/"; then break; fi
    sleep 1
  done
  if ! curl -s -o /dev/null "http://localhost:$VITE_PORT/"; then
    echo "Vite did not come up on port $VITE_PORT." >&2
    exit 1
  fi

  echo "Starting the Windows app (it loads the frontend from WSL's Vite)"
  start_win_logged "$(webview_debug_prefix)npm run tauri dev -- --config src-tauri/tauri.wsl-dev.conf.json"
  sync_rust_loop &
  sync_pid=$!
  echo "Rust changes under src-tauri/ are copied to the mirror as you save them."
  echo "Press Ctrl-C to stop both Vite and the Windows app."
  # Exit when Vite dies (the app is useless without it) or the Windows app exits.
  wait_win_exit "$vite_pid"
}

# Copy the release binary and installers to bundle/ in both the WSL checkout
# and the Windows mirror, replacing any from an earlier build so an old
# version does not linger. The mirror's bundle/ survives syncs because rsync
# excludes it.
collect_bundle() {
  local release="$WSL_DIR/src-tauri/target/release"
  local out
  echo
  for out in "$SRC_DIR/bundle" "$WSL_DIR/bundle"; do
    mkdir -p "$out"
    # The *.msi removal clears installers left from before the build stopped making them.
    rm -f "$out"/*.exe "$out"/*.msi
    cp "$release/stashpanda.exe" "$out/"
    cp "$release"/bundle/nsis/*-setup.exe "$out/"
    echo "Windows build copied to $out:"
    ls -1 "$out"/*.exe | sed 's|^|  |'
  done
  echo "On Windows: $WIN_DIR\\bundle"
}

# Tauri adds each build's installer to bundle/nsis and never removes the ones
# from earlier versions, so they are cleared first: collect_bundle then copies
# only the installer this build made.
clear_old_installers() {
  rm -f "$WSL_DIR/src-tauri/target/release/bundle/nsis/"*-setup.exe
}

# Once the binary and installer are in bundle/, the release build's own files
# (about 2 GB, mostly compiled crates) are only a cache for the next build, so
# they are removed to save disk space. The next build compiles everything again.
# Cargo removes them on the Windows side, which is far quicker than deleting
# that many files through /mnt/c.
clean_release_build() {
  echo
  echo "Removing the release build's intermediate files (cargo clean --release)..."
  run_win "cd src-tauri && cargo clean --release"
}

case "$ACTION" in
  ux)    ux_mode ;;
  sync)  run_win "npm install" ;;
  dev)   dev_mode ;;
  build) clear_old_installers; run_win "npm install && npm run tauri build"; collect_bundle; clean_release_build ;;
  *)     echo "Unknown action: $ACTION (expected ux, sync, dev or build)" >&2; exit 1 ;;
esac
