#!/usr/bin/env bash
# Create Git repositories on a test server for trying push, pull and fetch in StashPanda.
#
# Each scenario is a bare repository on the server, which plays the part GitHub
# would, plus two clones of it on this machine:
#   $CLONE_DIR/<name>          the clone to open in StashPanda
#   $CLONE_DIR/_other/<name>   a second clone standing in for a colleague, for
#                              pushing commits that the first clone then has to pull
#
#   scripts/testbed-remotes.sh                       # (re)create every scenario
#   scripts/testbed-remotes.sh behind ahead          # (re)create only these scenarios
#   scripts/testbed-remotes.sh list                  # list the scenarios
#   scripts/testbed-remotes.sh windows [scenario...] # the same, for the Windows build
#
# Running a scenario again deletes its bare repository and both clones and
# builds them from scratch, which resets the scenario after a test has pushed
# to the server.
#
# `windows` runs this script in Git Bash, so the clones are made by Windows
# git.exe in ~/stashpanda-testbeds in the Windows home folder, or in
# TESTBED_WIN_CLONE_DIR if that is set. They use their own bare repositories
# (git-testbeds-windows on the server), so resetting one side leaves the other alone.
#
# Needs SSH key login to the server, with no password prompt, through a
# `Host stashpanda-git` entry in ~/.ssh/config (in WSL, and in
# C:\Users\<name>\.ssh\config for Windows). The entry is an alias rather than
# the server's address, so that StashPanda's own SSH connections to the address
# still ask for a password. The settings below can be overridden with
# environment variables of the same name, set directly or in the repository's
# .env file (see .env.example).
set -euo pipefail

if [ "${1:-}" = windows ]; then
  shift
  exec "/mnt/c/Program Files/Git/bin/bash.exe" "$(wslpath -m "$0")" "$@"
fi

. "$(dirname "$0")/load-env.sh"
load_env "$(dirname "$0")/../.env"

case "${OSTYPE:-}" in
  msys* | cygwin*) clone_dir=${TESTBED_WIN_CLONE_DIR:-$HOME/stashpanda-testbeds}; server_dir=git-testbeds-windows ;;
  *)               clone_dir=$HOME/stashpanda-testbeds;                            server_dir=git-testbeds ;;
esac
TESTBED_SERVER="${TESTBED_SERVER:-stashpanda-git}"
TESTBED_SERVER_DIR="${TESTBED_SERVER_DIR:-$server_dir}"   # relative to the server account's home
TESTBED_CLONE_DIR="${TESTBED_CLONE_DIR:-$clone_dir}"

SCENARIOS=(in-sync behind ahead diverged conflict no-upstream other-name empty-remote protected-main branches-tags)

describe() {
  case $1 in
    in-sync)        echo "The clone matches the server. Start here for a first push or pull." ;;
    behind)         echo "The server has 2 commits the clone has not fetched yet. Fetch, then pull." ;;
    ahead)          echo "The clone has 2 commits the server does not. Push." ;;
    diverged)       echo "Both sides have a new commit, in different files. Push is rejected; pull merges or rebases cleanly." ;;
    conflict)       echo "Both sides added a different line to the end of src/app.txt. Pull stops with a conflict." ;;
    no-upstream)    echo "The clone is on feature/login, which has never been pushed and has no upstream." ;;
    other-name)     echo "The clone's branch my-work tracks origin/feature/shared and is 1 commit ahead." ;;
    empty-remote)   echo "The server repository is empty; the clone has 3 commits on main to push first." ;;
    protected-main) echo "A pre-receive hook on the server refuses every push to main. The clone is 1 commit ahead on main." ;;
    branches-tags)  echo "After the clone fetched, the server deleted feature/stale, force-pushed feature/rewritten, and added feature/new and tag v1.1. The clone has an unpushed tag v1.0-local." ;;
    *) return 1 ;;
  esac
}

# Two people with fixed identities, so commits do not depend on the global Git config
as_me() {
  GIT_AUTHOR_NAME='Local Tester' GIT_AUTHOR_EMAIL='local@testbed.invalid' \
  GIT_COMMITTER_NAME='Local Tester' GIT_COMMITTER_EMAIL='local@testbed.invalid' \
    git -c commit.gpgsign=false -c tag.gpgsign=false "$@"
}
as_other() {
  GIT_AUTHOR_NAME='Other Developer' GIT_AUTHOR_EMAIL='other@testbed.invalid' \
  GIT_COMMITTER_NAME='Other Developer' GIT_COMMITTER_EMAIL='other@testbed.invalid' \
    git -c commit.gpgsign=false -c tag.gpgsign=false "$@"
}

# commit <as_me|as_other> <repo> <file> <line> <message>: append a line to a file and commit it
commit() {
  local who=$1 repo=$2 file=$3 line=$4 msg=$5
  mkdir -p "$(dirname "$repo/$file")"
  printf '%s\n' "$line" >> "$repo/$file"
  "$who" -C "$repo" add -- "$file"
  "$who" -C "$repo" commit -q -m "$msg"
}

# new_remote <name>: delete the scenario's clones and replace its bare repository on the server
# with an empty one. Sets MINE, OTHER and URL for the scenario function.
new_remote() {
  local name=$1
  MINE="$TESTBED_CLONE_DIR/$name"
  OTHER="$TESTBED_CLONE_DIR/_other/$name"
  URL="$TESTBED_SERVER:$TESTBED_SERVER_DIR/$name.git"
  rm -rf "$MINE" "$OTHER"
  ssh "$TESTBED_SERVER" "rm -rf '$TESTBED_SERVER_DIR/$name.git' && git init -q --bare -b main '$TESTBED_SERVER_DIR/$name.git'"
}

# fresh <name>: a new bare repository holding three commits on main, pushed from the
# other developer's clone, and a clone of it to open in StashPanda
fresh() {
  new_remote "$1"
  git init -q -b main "$OTHER"
  commit as_other "$OTHER" README.md "# Test bed: $1" "Add a README"
  commit as_other "$OTHER" src/app.txt "first line" "Start the app"
  commit as_other "$OTHER" src/app.txt "second line" "Extend the app"
  git -C "$OTHER" remote add origin "$URL"
  git -C "$OTHER" push -q -u origin main
  git clone -q "$URL" "$MINE"
}

scenario_in_sync() {
  fresh in-sync
}

scenario_behind() {
  fresh behind
  commit as_other "$OTHER" src/app.txt "third line, from the other developer" "Add a third line"
  commit as_other "$OTHER" docs/notes.md "Notes from the other developer" "Add notes"
  git -C "$OTHER" push -q
}

scenario_ahead() {
  fresh ahead
  commit as_me "$MINE" src/app.txt "third line, from me" "Add my third line"
  commit as_me "$MINE" src/extra.txt "an extra file" "Add an extra file"
}

scenario_diverged() {
  fresh diverged
  commit as_other "$OTHER" docs/notes.md "Notes from the other developer" "Add notes"
  git -C "$OTHER" push -q
  commit as_me "$MINE" src/extra.txt "an extra file" "Add an extra file"
}

scenario_conflict() {
  fresh conflict
  commit as_other "$OTHER" src/app.txt "third line, the other developer's version" "Add the other developer's third line"
  git -C "$OTHER" push -q
  commit as_me "$MINE" src/app.txt "third line, my version" "Add my third line"
}

scenario_no_upstream() {
  fresh no-upstream
  git -C "$MINE" switch -q -c feature/login
  commit as_me "$MINE" src/login.txt "a login form" "Add a login form"
}

scenario_other_name() {
  fresh other-name
  git -C "$OTHER" switch -q -c feature/shared
  commit as_other "$OTHER" src/shared.txt "the other developer's part" "Start the shared feature"
  git -C "$OTHER" push -q -u origin feature/shared
  git -C "$MINE" fetch -q
  git -C "$MINE" switch -q -c my-work --track origin/feature/shared
  commit as_me "$MINE" src/shared.txt "my part" "Add my part of the shared feature"
}

scenario_empty_remote() {
  new_remote empty-remote
  git init -q -b main "$MINE"
  commit as_me "$MINE" README.md "# Test bed: empty-remote" "Add a README"
  commit as_me "$MINE" src/app.txt "first line" "Start the app"
  commit as_me "$MINE" src/app.txt "second line" "Extend the app"
  git -C "$MINE" remote add origin "$URL"
}

scenario_protected_main() {
  fresh protected-main
  # Installed after the first push, so that push is let through
  ssh "$TESTBED_SERVER" "cat > '$TESTBED_SERVER_DIR/protected-main.git/hooks/pre-receive' && chmod +x '$TESTBED_SERVER_DIR/protected-main.git/hooks/pre-receive'" <<'EOF'
#!/bin/sh
# Test-bed hook: refuse pushes to main, the way a protected branch does on a hosting service
while read -r old new ref; do
  if [ "$ref" = refs/heads/main ]; then
    echo "main is a protected branch on this server. Push to another branch instead." >&2
    exit 1
  fi
done
EOF
  commit as_me "$MINE" src/app.txt "my change to main" "Change main directly"
}

scenario_branches_tags() {
  fresh branches-tags
  # Branches and a tag that the clone fetches before anything changes
  git -C "$OTHER" branch feature/stale
  git -C "$OTHER" switch -q -c feature/rewritten
  commit as_other "$OTHER" src/rewritten.txt "first version" "Work that will be rewritten"
  as_other -C "$OTHER" tag -a v1.0 -m "Version 1.0" main
  git -C "$OTHER" push -q origin feature/stale feature/rewritten v1.0
  git -C "$MINE" fetch -q
  git -C "$MINE" switch -q feature/rewritten
  as_me -C "$MINE" tag -a v1.0-local -m "A tag only the clone has" main

  # Then the other developer changes the server under the clone
  git -C "$OTHER" push -q origin --delete feature/stale
  as_other -C "$OTHER" commit -q --amend -m "Work that was rewritten"
  git -C "$OTHER" push -q --force origin feature/rewritten
  git -C "$OTHER" switch -q -c feature/new main
  commit as_other "$OTHER" src/new.txt "a new feature" "Start a new feature"
  git -C "$OTHER" push -q -u origin feature/new
  as_other -C "$OTHER" tag -a v1.1 -m "Version 1.1" main
  git -C "$OTHER" push -q origin v1.1
}

if [ "${1:-}" = list ]; then
  for s in "${SCENARIOS[@]}"; do printf '%-15s %s\n' "$s" "$(describe "$s")"; done
  exit 0
fi

selected=("$@")
[ ${#selected[@]} -eq 0 ] && selected=("${SCENARIOS[@]}")
for s in "${selected[@]}"; do
  describe "$s" > /dev/null || { echo "Unknown scenario: $s (run with 'list' to see them)" >&2; exit 1; }
done

if ! ssh -o BatchMode=yes "$TESTBED_SERVER" "mkdir -p '$TESTBED_SERVER_DIR'"; then
  echo "Cannot log in to $TESTBED_SERVER with an SSH key. Set up key login first." >&2
  exit 1
fi
mkdir -p "$TESTBED_CLONE_DIR/_other"

for s in "${selected[@]}"; do
  echo "== $s: $(describe "$s")"
  "scenario_${s//-/_}"
done
echo "Clones to open in StashPanda are in $TESTBED_CLONE_DIR"
