#!/usr/bin/env bash
# Build a demo repository for StashPanda's README screenshots: a small made-up notes app called
# Lantern, with four made-up authors, feature branches, merges, tags and a remote, so the
# commit graph, the Branches panel and the Files panel all have something to show.
#
#   scripts/demo-repo.sh              # (re)create the demo in ~/stashpanda-demo
#   scripts/demo-repo.sh --conflict   # the same, but leave a merge stopped on a conflict
#   DEMO_DIR=/some/folder scripts/demo-repo.sh
#
# It makes, inside DEMO_DIR:
#   lantern/              the clone to open in StashPanda
#   remote/lantern.git    a bare repository standing in for GitHub (origin)
#   _colleague/lantern    a second clone, which pushed a commit lantern/ has not pulled yet
#
# The clone ends up like this:
#   - main is 1 commit ahead of origin/main and 1 behind, so Pull, Push and Sync all have work.
#   - Tags v0.1.0 and v0.2.0, and merged branches, so the graph has lanes joining.
#   - feature/dark-mode is pushed and unmerged; feature/export is pushed and conflicts with
#     main in src/notes.ts if merged; fix/empty-title is local only, with no upstream.
#   - One stash, and a working tree with a staged change, an unstaged change and an
#     untracked file. With --conflict, the working tree instead holds `git merge
#     feature/export` stopped on its conflict, for the merge editor.
#
# Dates are counted back from now, so the Date column reads "3 weeks ago" to "2 hours ago"
# whenever the script runs. The authors' emails are @example.com, so no avatar service has a
# picture for them and the commit list shows their initials.
#
# Running it again deletes DEMO_DIR and builds it from scratch, but only when DEMO_DIR holds the
# .stashpanda-demo marker this script leaves, so it never deletes a folder it did not make.
set -euo pipefail

CONFLICT=false
case "${1:-}" in
  --conflict) CONFLICT=true ;;
  "") ;;
  *) echo "Unknown option: $1 (expected --conflict or nothing)" >&2; exit 1 ;;
esac

DEMO_DIR="${DEMO_DIR:-$HOME/stashpanda-demo}"
MARKER="$DEMO_DIR/.stashpanda-demo"

if [ -e "$DEMO_DIR" ]; then
  if [ ! -e "$MARKER" ]; then
    echo "$DEMO_DIR exists and was not made by this script; choose another DEMO_DIR." >&2
    exit 1
  fi
  rm -rf "$DEMO_DIR"
fi
mkdir -p "$DEMO_DIR"
touch "$MARKER"

REMOTE="$DEMO_DIR/remote/lantern.git"
WORK="$DEMO_DIR/lantern"
OTHER="$DEMO_DIR/_colleague/lantern"

NOW=$(date +%s)
ZONE=$(date +%z)

declare -A NAME=(
  [ada]="Ada Okafor"
  [tomas]="Tomás Lindqvist"
  [mei]="Mei Tanaka"
  [priya]="Priya Raman"
)

# Run a Git command as one of the authors, dated the given number of hours ago.
as() {
  local who=$1 hours=$2
  shift 2
  local when="$((NOW - hours * 3600)) $ZONE"
  GIT_AUTHOR_NAME="${NAME[$who]}" GIT_AUTHOR_EMAIL="$who@example.com" GIT_AUTHOR_DATE="$when" \
  GIT_COMMITTER_NAME="${NAME[$who]}" GIT_COMMITTER_EMAIL="$who@example.com" GIT_COMMITTER_DATE="$when" \
    "$@"
}

# Stage everything and commit it as an author, hours ago, with git commit's own -m arguments.
commit() {
  local who=$1 hours=$2
  shift 2
  git add -A
  as "$who" "$hours" git commit -q "$@"
}

merge() {
  local who=$1 hours=$2 branch=$3
  as "$who" "$hours" git merge -q --no-ff "$branch" -m "Merge branch '$branch'"
}

# Write standard input to a file, making its folder first.
put() {
  mkdir -p "$(dirname "$1")"
  cat > "$1"
}

# Repository settings that keep the user's own Git config from changing the result.
quiet_config() {
  git config commit.gpgsign false
  git config tag.gpgsign false
  git config core.autocrlf false
  git config core.hooksPath /dev/null
}

git init -q --bare --initial-branch=main "$REMOTE"
git init -q --initial-branch=main "$WORK"
cd "$WORK"
quiet_config
git remote add origin "$REMOTE"

# ---- The first release ---------------------------------------------------------------------

put README.md <<'EOF'
# Lantern

A small, fast notes app that keeps everything in your browser.

## Getting started

```sh
npm install
npm run dev
```
EOF
put package.json <<'EOF'
{
  "name": "lantern",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build"
  },
  "devDependencies": {
    "typescript": "^5.4.0",
    "vite": "^5.2.0"
  }
}
EOF
put .gitignore <<'EOF'
node_modules
dist
EOF
commit ada 500 -m "Initial commit"

put src/notes.ts <<'EOF'
export interface Note {
  id: string;
  title: string;
  body: string;
  updatedAt: number;
}

const notes = new Map<string, Note>();

export function addNote(title: string, body = ""): Note {
  const note = { id: crypto.randomUUID(), title, body, updatedAt: Date.now() };
  notes.set(note.id, note);
  return note;
}

export function allNotes(): Note[] {
  return [...notes.values()].sort((a, b) => b.updatedAt - a.updatedAt);
}
EOF
commit ada 480 -m "Add the note store"

put src/ui/list.ts <<'EOF'
import { allNotes, type Note } from "../notes";

export function renderList(root: HTMLElement): void {
  root.replaceChildren(...allNotes().map(renderItem));
}

function renderItem(note: Note): HTMLElement {
  const item = document.createElement("li");
  item.className = "note";
  item.dataset.id = note.id;
  item.textContent = note.title || "Untitled";
  return item;
}
EOF
commit tomas 460 -m "Render the note list"

put src/notes.ts <<'EOF'
export interface Note {
  id: string;
  title: string;
  body: string;
  updatedAt: number;
}

const notes = new Map<string, Note>();
const STORAGE_KEY = "lantern.notes";

export function addNote(title: string, body = ""): Note {
  const note = { id: crypto.randomUUID(), title, body, updatedAt: Date.now() };
  notes.set(note.id, note);
  return note;
}

export function allNotes(): Note[] {
  return [...notes.values()].sort((a, b) => b.updatedAt - a.updatedAt);
}

export function saveNotes(): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(allNotes()));
}

export function loadNotes(): void {
  const saved = localStorage.getItem(STORAGE_KEY);
  for (const note of saved ? (JSON.parse(saved) as Note[]) : []) notes.set(note.id, note);
}
EOF
commit ada 430 -m "Save notes to local storage" -m "Notes now survive a page reload. Everything is stored under one key, \`lantern.notes\`, as a JSON array.

- Saved after every change
- Loaded once at start-up"
as ada 428 git tag -a v0.1.0 -m "Lantern 0.1.0"

# ---- Search and tags, built side by side ---------------------------------------------------

git switch -q -c feature/search
put src/search.ts <<'EOF'
import { allNotes, type Note } from "./notes";

export function search(query: string): Note[] {
  const words = query.toLowerCase().split(/ +/).filter(Boolean);
  return allNotes().filter((note) => {
    const text = `${note.title} ${note.body}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
}
EOF
commit mei 400 -m "Add full-text search over titles and bodies"

git switch -q main
put src/ui/list.ts <<'EOF'
import { allNotes, type Note } from "../notes";

export function renderList(root: HTMLElement): void {
  const scrollTop = root.scrollTop;
  root.replaceChildren(...allNotes().map(renderItem));
  root.scrollTop = scrollTop;
}

function renderItem(note: Note): HTMLElement {
  const item = document.createElement("li");
  item.className = "note";
  item.dataset.id = note.id;
  item.textContent = note.title || "Untitled";
  return item;
}
EOF
commit tomas 390 -m "Keep the list's scroll position after an edit"

git switch -q feature/search
put src/ui/highlight.ts <<'EOF'
export function highlight(text: string, query: string): DocumentFragment {
  const fragment = document.createDocumentFragment();
  const lower = text.toLowerCase();
  const needle = query.toLowerCase();
  let from = 0;
  for (let at = lower.indexOf(needle); needle && at !== -1; at = lower.indexOf(needle, from)) {
    fragment.append(text.slice(from, at));
    const mark = document.createElement("mark");
    mark.textContent = text.slice(at, at + needle.length);
    fragment.append(mark);
    from = at + needle.length;
  }
  fragment.append(text.slice(from));
  return fragment;
}
EOF
commit mei 370 -m "Highlight matches in the note list"

put src/search.ts <<'EOF'
import { allNotes, type Note } from "./notes";

export function search(query: string): Note[] {
  const words = query.toLowerCase().split(/ +/).filter(Boolean);
  const score = (note: Note) => (words.some((word) => note.title.toLowerCase().includes(word)) ? 1 : 0);
  return allNotes()
    .filter((note) => {
      const text = `${note.title} ${note.body}`.toLowerCase();
      return words.every((word) => text.includes(word));
    })
    .sort((a, b) => score(b) - score(a));
}
EOF
commit mei 350 -m "Rank title matches above body matches"

git switch -q main
git switch -q -c feature/tags
put src/notes.ts <<'EOF'
export interface Note {
  id: string;
  title: string;
  body: string;
  tags: string[];
  updatedAt: number;
}

const notes = new Map<string, Note>();
const STORAGE_KEY = "lantern.notes";

export function addNote(title: string, body = "", tags: string[] = []): Note {
  const note = { id: crypto.randomUUID(), title, body, tags, updatedAt: Date.now() };
  notes.set(note.id, note);
  return note;
}

export function allNotes(): Note[] {
  return [...notes.values()].sort((a, b) => b.updatedAt - a.updatedAt);
}

export function saveNotes(): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(allNotes()));
}

export function loadNotes(): void {
  const saved = localStorage.getItem(STORAGE_KEY);
  for (const note of saved ? (JSON.parse(saved) as Note[]) : []) notes.set(note.id, { tags: [], ...note });
}
EOF
commit priya 340 -m "Add tags to notes" -m "Notes saved before this change load with no tags."

git switch -q main
merge ada 320 feature/search

git switch -q feature/tags
put src/tags.ts <<'EOF'
import { allNotes, type Note } from "./notes";

export function allTags(): string[] {
  return [...new Set(allNotes().flatMap((note) => note.tags))].sort();
}

export function notesTagged(tag: string): Note[] {
  return allNotes().filter((note) => note.tags.includes(tag));
}
EOF
commit priya 300 -m "Filter the list by tag"

git switch -q main
put package.json <<'EOF'
{
  "name": "lantern",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build"
  },
  "devDependencies": {
    "typescript": "^5.6.0",
    "vite": "^5.4.0"
  }
}
EOF
commit tomas 280 -m "Update TypeScript and Vite"

merge priya 260 feature/tags
git branch -q -d feature/search feature/tags

put CHANGELOG.md <<'EOF'
# Changelog

## 0.2.0

- Search titles and bodies, with matches highlighted
- Tags, and filtering the list by tag
- The list keeps its scroll position after an edit

## 0.1.0

- Notes saved in the browser
EOF
sed -i 's/"version": "0.1.0"/"version": "0.2.0"/' package.json
commit ada 250 -m "Release 0.2.0"
as ada 249 git tag -a v0.2.0 -m "Lantern 0.2.0"

# ---- Work since the last release -----------------------------------------------------------

git switch -q -c feature/dark-mode
put src/ui/theme.css <<'EOF'
:root {
  --page: #fbfaf7;
  --ink: #23262b;
  --accent: #2b8db8;
}

:root.dark {
  --page: #1c1e22;
  --ink: #e6e4df;
  --accent: #5fb4dc;
}

body {
  background: var(--page);
  color: var(--ink);
}
EOF
commit tomas 200 -m "Add a dark theme"

git switch -q main
put src/ui/editor.ts <<'EOF'
import { saveNotes } from "../notes";

let timer: ReturnType<typeof setTimeout> | undefined;

export function onInput(): void {
  clearTimeout(timer);
  timer = setTimeout(saveNotes, 400);
}
EOF
commit mei 190 -m "Wait for a pause in typing before saving"

git switch -q feature/dark-mode
put src/ui/theme.ts <<'EOF'
const query = window.matchMedia("(prefers-color-scheme: dark)");

export function followSystemTheme(): void {
  const apply = () => document.documentElement.classList.toggle("dark", query.matches);
  query.addEventListener("change", apply);
  apply();
}
EOF
commit tomas 180 -m "Follow the system's colour scheme"

git switch -q main
git switch -q -c fix/date-format
put src/ui/date.ts <<'EOF'
const format = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

export function formatDate(time: number): string {
  return format.format(time);
}
EOF
commit priya 150 -m "Show dates in the reader's own format"

git switch -q main
merge ada 120 fix/date-format
git branch -q -d fix/date-format

git switch -q -c feature/export
put src/notes.ts <<'EOF'
export interface Note {
  id: string;
  title: string;
  body: string;
  tags: string[];
  updatedAt: number;
}

const notes = new Map<string, Note>();
const STORAGE_KEY = "lantern.notes";

export function addNote(title: string, body = "", tags: string[] = []): Note {
  const note = { id: crypto.randomUUID(), title, body, tags, updatedAt: Date.now() };
  notes.set(note.id, note);
  return note;
}

export function allNotes(): Note[] {
  return [...notes.values()].sort((a, b) => b.updatedAt - a.updatedAt);
}

export function saveNotes(): void {
  const all = allNotes();
  localStorage.setItem(STORAGE_KEY, JSON.stringify(all));
  localStorage.setItem(`${STORAGE_KEY}.savedAt`, String(Date.now()));
}

export function loadNotes(): void {
  const saved = localStorage.getItem(STORAGE_KEY);
  for (const note of saved ? (JSON.parse(saved) as Note[]) : []) notes.set(note.id, { tags: [], ...note });
}
EOF
put src/export.ts <<'EOF'
import { allNotes } from "./notes";

export function exportMarkdown(): string {
  return allNotes()
    .map((note) => `# ${note.title}\n\n${note.body}\n`)
    .join("\n");
}
EOF
commit mei 100 -m "Export notes as Markdown" -m "Adds **File > Export** and records when the notes were last saved, so the export can say how fresh it is."

git switch -q main
put src/notes.ts <<'EOF'
export interface Note {
  id: string;
  title: string;
  body: string;
  tags: string[];
  updatedAt: number;
}

const notes = new Map<string, Note>();
const STORAGE_KEY = "lantern.notes";
let pending: number | undefined;

export function addNote(title: string, body = "", tags: string[] = []): Note {
  const note = { id: crypto.randomUUID(), title, body, tags, updatedAt: Date.now() };
  notes.set(note.id, note);
  return note;
}

export function allNotes(): Note[] {
  return [...notes.values()].sort((a, b) => b.updatedAt - a.updatedAt);
}

export function saveNotes(): void {
  if (pending !== undefined) return;
  pending = requestIdleCallback(() => {
    pending = undefined;
    localStorage.setItem(STORAGE_KEY, JSON.stringify(allNotes()));
  });
}

export function loadNotes(): void {
  const saved = localStorage.getItem(STORAGE_KEY);
  for (const note of saved ? (JSON.parse(saved) as Note[]) : []) notes.set(note.id, { tags: [], ...note });
}
EOF
commit ada 90 -m "Save notes when the browser is idle"

put src/ui/keys.ts <<'EOF'
const shortcuts = new Map<string, () => void>();

export function bind(key: string, action: () => void): void {
  shortcuts.set(key, action);
}

window.addEventListener("keydown", (event) => {
  const key = `${event.ctrlKey || event.metaKey ? "Mod+" : ""}${event.key}`;
  const action = shortcuts.get(key);
  if (!action) return;
  event.preventDefault();
  action();
});
EOF
commit priya 60 -m "Add keyboard shortcuts"

git push -q origin main feature/dark-mode feature/export --tags
git branch -q --set-upstream-to=origin/main main
git branch -q --set-upstream-to=origin/feature/dark-mode feature/dark-mode
git branch -q --set-upstream-to=origin/feature/export feature/export

# ---- A colleague pushes a commit this clone has not pulled ---------------------------------

git clone -q "$REMOTE" "$OTHER"
(
  cd "$OTHER"
  quiet_config
  put src/ui/count.ts <<'EOF'
export function wordCount(text: string): number {
  return text.split(/\s+/).filter(Boolean).length;
}
EOF
  commit tomas 20 -m "Show a word count under each note"
  git push -q origin main
)

# ---- Local work: a commit to push, a branch with no upstream, a stash, a dirty tree ---------

git switch -q -c fix/empty-title
put src/ui/list.ts <<'EOF'
import { allNotes, type Note } from "../notes";

export function renderList(root: HTMLElement): void {
  const scrollTop = root.scrollTop;
  root.replaceChildren(...allNotes().map(renderItem));
  root.scrollTop = scrollTop;
}

function renderItem(note: Note): HTMLElement {
  const item = document.createElement("li");
  item.className = "note";
  item.dataset.id = note.id;
  item.textContent = note.title.trim() || note.body.split("\n")[0] || "Untitled";
  return item;
}
EOF
commit priya 30 -m "Use a note's first line when it has no title"

git switch -q main
put README.md <<'EOF'
# Lantern

A small, fast notes app that keeps everything in your browser.

## Getting started

```sh
npm install
npm run dev
```

## Keyboard shortcuts

| Keys | Action |
| --- | --- |
| Ctrl+N | New note |
| Ctrl+F | Search |
EOF
commit ada 10 -m "Document the keyboard shortcuts"

git fetch -q origin

put src/ui/list.ts <<'EOF'
import { allNotes, type Note } from "../notes";

export function renderList(root: HTMLElement): void {
  const scrollTop = root.scrollTop;
  root.classList.add("compact");
  root.replaceChildren(...allNotes().map(renderItem));
  root.scrollTop = scrollTop;
}

function renderItem(note: Note): HTMLElement {
  const item = document.createElement("li");
  item.className = "note";
  item.dataset.id = note.id;
  item.textContent = note.title || "Untitled";
  return item;
}
EOF
as ada 5 git stash push -q -m "Try a compact list layout"

if $CONFLICT; then
  as ada 1 git merge --no-ff feature/export -m "Merge branch 'feature/export'" >/dev/null 2>&1 || true
else
  sed -i 's/^## Getting started$/## Getting started locally/' README.md
  git add README.md
  put src/notes.ts.new < src/notes.ts
  {
    cat src/notes.ts.new
    cat <<'EOF'

export function removeNote(id: string): void {
  notes.delete(id);
  saveNotes();
}
EOF
  } > src/notes.ts
  rm src/notes.ts.new
  put src/ui/empty-state.ts <<'EOF'
export function renderEmptyState(root: HTMLElement): void {
  root.textContent = "No notes yet. Press Ctrl+N to write one.";
}
EOF
fi

echo "Demo repository ready: $WORK"
git status -sb | head -1
git log --oneline --graph --all -12
