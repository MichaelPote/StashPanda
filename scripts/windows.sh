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
#   scripts/windows.sh build   # sync, then build the release binary and installers
#   scripts/windows.sh sync    # mirror the source and run npm install on Windows
#
# The mirror is one-way: edit the code in WSL, then re-run this script.
# In `ux` mode only Rust changes need a re-run; Vue, TypeScript and CSS changes
# hot reload in the running app.
set -euo pipefail

WIN_DIR='C:\src\stashpanda'
WSL_DIR="$(wslpath -u "$WIN_DIR")"
SRC_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ACTION="${1:-ux}"
VITE_PORT=1420

mkdir -p "$WSL_DIR"

echo "Syncing $SRC_DIR -> $WIN_DIR"
rsync -a --delete \
  --exclude '.git' \
  --exclude '.claude' \
  --exclude 'node_modules' \
  --exclude 'dist' \
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
start_win_detached() {
  local command="$1"
  (cd /mnt/c && powershell.exe -NoProfile -NonInteractive -Command \
    "(Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = 'cmd.exe /c \"cd /d $WIN_DIR && $command\"'; CurrentDirectory = '$WIN_DIR' }).ProcessId") | tr -d '\r'
}

ux_mode() {
  run_win "npm install"

  if [[ "$(ss -ltn)" == *":$VITE_PORT "* ]]; then
    echo "Port $VITE_PORT is already in use in WSL. Stop the other Vite first." >&2
    exit 1
  fi

  local vite_pid="" win_pid="" tail_pid=""
  win_alive() {
    local procs
    procs="$(cd /mnt/c && cmd.exe /c "tasklist /FO CSV /NH" 2>/dev/null || true)"
    # An empty listing is an interop hiccup, not proof the process is gone.
    # Plain bash matching: tasklist output contains non-UTF-8 bytes that make
    # some grep wrappers treat it as binary and print nothing.
    [ -z "$procs" ] || [[ "$procs" == *"\"$1\""* ]]
  }
  cleanup() {
    trap - EXIT INT TERM
    echo
    echo "Stopping..."
    if [ -n "$win_pid" ]; then
      # Kill the Windows tree (cmd -> npm -> tauri CLI -> cargo -> app). Interop
      # calls occasionally fail silently, so retry until the root is gone.
      local attempt
      for attempt in 1 2 3 4 5; do
        local killed=0 line
        while IFS= read -r line; do [[ "$line" == SUCCESS* ]] && killed=$((killed + 1)); done \
          < <(cd /mnt/c && cmd.exe /c "taskkill /PID $win_pid /T /F" 2>&1 | tr -d '\r' || true)
        echo "  killed Windows processes: $killed"
        sleep 1
        if ! win_alive "$win_pid"; then break; fi
      done
      if win_alive "$win_pid"; then
        echo "Warning: Windows process $win_pid is still running; kill it with: taskkill /PID $win_pid /T /F" >&2
      fi
    fi
    [ -n "$tail_pid" ] && kill "$tail_pid" 2>/dev/null || true
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
  : > "$WSL_DIR/tauri-dev.log"; : > "$WSL_DIR/tauri-dev.err.log"
  win_pid="$(start_win_detached "npm run tauri dev -- --config src-tauri/tauri.wsl-dev.conf.json > tauri-dev.log 2> tauri-dev.err.log")"
  if ! [[ "$win_pid" =~ ^[0-9]+$ ]]; then
    echo "Could not start the Windows app: $win_pid" >&2
    exit 1
  fi
  echo "Windows npm process id: $win_pid"
  echo "Press Ctrl-C to stop both Vite and the Windows app."

  # inotify does not fire on the /mnt/c mount, so poll the log files instead.
  tail -n +1 -f ---disable-inotify --sleep-interval=1 "$WSL_DIR/tauri-dev.log" "$WSL_DIR/tauri-dev.err.log" | tr -d '\r' &
  tail_pid=$!

  # Exit when Vite dies (the app is useless without it) or the Windows app exits.
  # tasklist called through interop occasionally returns nothing at all, so only
  # trust a non-empty listing, and require three misses in a row.
  local misses=0
  while kill -0 "$vite_pid" 2>/dev/null; do
    if win_alive "$win_pid"; then misses=0; else misses=$((misses + 1)); fi
    if [ "$misses" -ge 3 ]; then
      echo "The Windows app has exited."
      break
    fi
    sleep 3
  done
}

case "$ACTION" in
  ux)    ux_mode ;;
  sync)  run_win "npm install" ;;
  dev)   run_win "npm install && npm run tauri dev" ;;
  build) run_win "npm install && npm run tauri build" ;;
  *)     echo "Unknown action: $ACTION (expected ux, sync, dev or build)" >&2; exit 1 ;;
esac
