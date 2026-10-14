//! Staging, unstaging and discarding single lines and whole hunks of one file.
//!
//! The frontend names the lines it shows as selected: an added line by its number in the new
//! file, a removed line by its number in the old file, each with its text. Those numbers do not
//! depend on how much context the viewer asked for, so the diff is recomputed here with fixed
//! options rather than taken from the frontend, and every selected line's text is checked
//! against it: a file that changed since its diff was shown is refused instead of patched in the
//! wrong place. A patch that touches only the selected lines then goes to `git apply`, on the
//! index (`--cached`) to stage or unstage, or on the working tree to discard.
//!
//! The patch always applies forwards to the version it changes:
//! - Stage, from the unstaged diff (index → working tree), patches the index. A selected line
//!   keeps its sign; an unselected removal becomes context, since it stays in the index; an
//!   unselected addition is left out.
//! - Unstage, from the staged diff (HEAD → index), patches the index, and discard, from the
//!   unstaged diff, patches the working tree. Both undo the selected lines, so a selected
//!   addition becomes a removal and a selected removal an addition; an unselected addition
//!   becomes context, since it stays, and an unselected removal is left out.
//!
//! When the selection covers every changed line, the whole-file operation runs instead, which
//! also handles new and deleted files and file modes.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use specta::Type;

use super::{discard, result, stage, unstage, OpResult};
use crate::error::{GitError, Result};
use crate::git::parse::looks_binary;
use crate::git::queries::diff::join_repo_path;
use crate::git::{GitClient, GitReq};

/// What to do with the selected lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum LineAction {
    /// Add the selected working tree changes to the index (from the unstaged diff).
    Stage,
    /// Take the selected changes out of the index, back to HEAD (from the staged diff).
    Unstage,
    /// Throw away the selected working tree changes, back to the index (from the unstaged diff).
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Add,
    Del,
}

/// One changed line as the diff viewer showed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SelectedLine {
    pub kind: ChangeKind,
    /// The line's number in the new file for an addition, in the old file for a removal.
    pub line: u32,
    /// The line's text without the leading `+` or `-` and without a trailing `\r`.
    pub text: String,
}

/// Largest untracked file whose lines can be staged one by one.
const FILE_CAP: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    /// `b' '`, `b'-'` or `b'+'`.
    sign: u8,
    /// The line's bytes after the sign, including its `\n`.
    body: Vec<u8>,
    /// Followed by `\ No newline at end of file`: the file's last line, with no newline.
    no_newline: bool,
    old_no: u32,
    new_no: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hunk {
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
    lines: Vec<Line>,
}

fn stale() -> GitError {
    GitError::InvalidPath {
        path: String::new(),
        reason: "the file has changed since its diff was shown; look at it again and reselect the lines".into(),
    }
}

/// `@@ -a[,b] +c[,d] @@` → (a, b, c, d); a missing count means 1.
fn parse_header(line: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let text = std::str::from_utf8(line).ok()?;
    let inner = text.strip_prefix("@@ -")?;
    let end = inner.find(" @@")?;
    let (old, new) = inner[..end].split_once(" +")?;
    let range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((s, c)) => Some((s.parse().ok()?, c.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (a, b) = range(old)?;
    let (c, d) = range(new)?;
    Some((a, b, c, d))
}

/// The hunks of a one-file `git diff`. Fails on a binary diff, which has no lines to pick.
fn parse_hunks(diff: &[u8]) -> Result<Vec<Hunk>> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut in_hunk = false;
    let (mut old_no, mut new_no) = (0, 0);
    for raw in diff.split_inclusive(|b| *b == b'\n') {
        if raw.starts_with(b"Binary files ") || raw.starts_with(b"GIT binary patch") {
            return Err(GitError::InvalidPath { path: String::new(), reason: "a binary file has no lines to pick; stage the whole file".into() });
        }
        if raw.starts_with(b"@@ ") {
            let (a, b, c, d) = parse_header(raw.trim_ascii_end()).ok_or_else(stale)?;
            hunks.push(Hunk { old_start: a, old_count: b, new_start: c, new_count: d, lines: Vec::new() });
            in_hunk = true;
            // A count of 0 puts the start on the line before; numbering starts after it.
            old_no = if b == 0 { a + 1 } else { a };
            new_no = if d == 0 { c + 1 } else { c };
            continue;
        }
        if !in_hunk {
            continue;
        }
        let hunk = hunks.last_mut().expect("in a hunk");
        match raw.first() {
            Some(b'\\') => {
                if let Some(last) = hunk.lines.last_mut() {
                    last.no_newline = true;
                }
            }
            Some(&sign @ (b' ' | b'-' | b'+')) => {
                hunk.lines.push(Line { sign, body: raw[1..].to_vec(), no_newline: false, old_no, new_no });
                if sign != b'+' {
                    old_no += 1;
                }
                if sign != b'-' {
                    new_no += 1;
                }
            }
            _ => in_hunk = false,
        }
    }
    Ok(hunks)
}

/// The diff of a file Git does not track yet, as one hunk of additions.
fn untracked_hunks(content: &[u8]) -> Result<Vec<Hunk>> {
    if looks_binary(content) {
        return Err(GitError::InvalidPath { path: String::new(), reason: "a binary file has no lines to pick; stage the whole file".into() });
    }
    let mut lines: Vec<Line> = content
        .split_inclusive(|b| *b == b'\n')
        .enumerate()
        .map(|(i, l)| {
            let mut body = l.to_vec();
            if !body.ends_with(b"\n") {
                body.push(b'\n');
            }
            Line { sign: b'+', body, no_newline: false, old_no: 0, new_no: i as u32 + 1 }
        })
        .collect();
    if !content.is_empty() && !content.ends_with(b"\n") {
        if let Some(last) = lines.last_mut() {
            last.no_newline = true;
        }
    }
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    let n = lines.len() as u32;
    Ok(vec![Hunk { old_start: 0, old_count: 0, new_start: 1, new_count: n, lines: std::mem::take(&mut lines) }])
}

fn key_of(line: &Line) -> Option<(ChangeKind, u32)> {
    match line.sign {
        b'+' => Some((ChangeKind::Add, line.new_no)),
        b'-' => Some((ChangeKind::Del, line.old_no)),
        _ => None,
    }
}

fn text_of(line: &Line) -> String {
    let body = line.body.strip_suffix(b"\n").unwrap_or(&line.body);
    let body = body.strip_suffix(b"\r").unwrap_or(body);
    String::from_utf8_lossy(body).into_owned()
}

/// Checks every selected line against the recomputed diff. Returns the selected keys and
/// whether they cover every changed line.
fn match_selection(hunks: &[Hunk], selected: &[SelectedLine]) -> Result<(HashSet<(ChangeKind, u32)>, bool)> {
    if selected.is_empty() {
        return Err(GitError::InvalidPath { path: String::new(), reason: "no lines selected".into() });
    }
    let mut found = HashSet::new();
    let mut changed = 0;
    for line in hunks.iter().flat_map(|h| &h.lines) {
        let Some(key) = key_of(line) else { continue };
        changed += 1;
        if let Some(s) = selected.iter().find(|s| (s.kind, s.line) == key) {
            if s.text != text_of(line) {
                return Err(stale());
            }
            found.insert(key);
        }
    }
    let wanted: HashSet<(ChangeKind, u32)> = selected.iter().map(|s| (s.kind, s.line)).collect();
    if found.len() != wanted.len() {
        return Err(stale());
    }
    Ok((found, found_all(changed, &wanted)))
}

fn found_all(changed: usize, wanted: &HashSet<(ChangeKind, u32)>) -> bool {
    changed > 0 && wanted.len() == changed
}

/// C-quotes a path for a patch header when Git would, so `git apply` reads it back exactly.
fn header_path(prefix: &str, path: &str) -> String {
    if !path.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return format!("{prefix}{path}");
    }
    let mut out = String::from("\"");
    out.push_str(prefix);
    for c in path.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\{:03o}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const NEIGHBOURS: &str = "These lines cannot be changed on their own: a line next to them depends on them (often the last line of a file that has no newline at its end). Select the neighbouring lines too.";

/// The patch for `action` with only the `selected` lines, against the version it changes.
/// `new_file` marks a patch that creates the file in the index (staging part of an untracked
/// file). Fails when the result would put a line after one that has no newline: `git apply`
/// accepts such a patch and joins the two lines into one.
fn build_patch(hunks: &[Hunk], selected: &HashSet<(ChangeKind, u32)>, action: LineAction, path: &str, new_file: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let a = header_path("a/", path);
    let b = header_path("b/", path);
    out.extend_from_slice(format!("diff --git {a} {b}\n").as_bytes());
    if new_file {
        out.extend_from_slice(format!("new file mode 100644\n--- /dev/null\n+++ {b}\n").as_bytes());
    } else {
        out.extend_from_slice(format!("--- {a}\n+++ {b}\n").as_bytes());
    }
    let forward = action == LineAction::Stage;
    let mut delta: i64 = 0;
    for hunk in hunks {
        if !hunk.lines.iter().any(|l| key_of(l).is_some_and(|k| selected.contains(&k))) {
            continue;
        }
        let mut body: Vec<u8> = Vec::new();
        let (mut pre, mut post) = (0u32, 0u32);
        let mut post_ended = false;
        for line in &hunk.lines {
            let chosen = key_of(line).is_some_and(|k| selected.contains(&k));
            let sign = match (line.sign, chosen, forward) {
                (b' ', _, _) => b' ',
                (s, true, true) => s,
                (b'-', false, true) => b' ',
                (b'+', false, true) => continue,
                (b'+', true, false) => b'-',
                (b'-', true, false) => b'+',
                (b'+', false, false) => b' ',
                (b'-', false, false) => continue,
                _ => continue,
            };
            if sign != b'+' {
                pre += 1;
            }
            if sign != b'-' {
                if post_ended {
                    return Err(GitError::InvalidPath { path: path.to_string(), reason: NEIGHBOURS.into() });
                }
                post_ended = line.no_newline;
                post += 1;
            }
            body.push(sign);
            body.extend_from_slice(&line.body);
            if line.no_newline {
                body.extend_from_slice(b"\\ No newline at end of file\n");
            }
        }
        // The version being patched is the diff's old side when staging, its new side otherwise.
        let pre_start = if forward { hunk.old_start } else { hunk.new_start };
        let first = if pre == 0 { pre_start as i64 + 1 } else { pre_start as i64 };
        let post_first = first + delta;
        let post_start = if post == 0 { post_first - 1 } else { post_first };
        out.extend_from_slice(format!("@@ -{pre_start},{pre} +{post_start},{post} @@\n").as_bytes());
        out.extend_from_slice(&body);
        delta += post as i64 - pre as i64;
    }
    Ok(out)
}

/// `git diff [--cached]` for `path`, with the options the line numbers are taken from.
async fn raw_diff(client: &GitClient, cached: bool, path: &str) -> Result<Vec<u8>> {
    let mut args: Vec<&str> = vec!["--literal-pathspecs", "diff"];
    if cached {
        args.push("--cached");
    }
    args.extend(["--no-ext-diff", "--no-color", "--no-renames", "-U3", "--src-prefix=a/", "--dst-prefix=b/", "--", path]);
    let out = client.exec_allow_failure(GitReq::new("diff", args)).await?;
    if !out.success() {
        return Err(GitError::CommandFailed { label: "diff".into(), argv: vec![], exit_code: out.exit_code, stderr: String::from_utf8_lossy(&out.stderr).into_owned() });
    }
    Ok(out.stdout)
}

/// Stage, unstage or discard the `selected` lines of `path`. `untracked` says the file is not
/// in the index yet (its diff is the whole file as additions); only stage and discard apply to
/// one. Fails with `invalidPath` when a selected line is no longer in the file's diff.
pub async fn apply_lines(client: &GitClient, action: LineAction, path: &str, untracked: bool, selected: &[SelectedLine]) -> Result<OpResult> {
    super::valid_paths(&[path.to_string()])?;
    if untracked && action == LineAction::Unstage {
        return Err(GitError::InvalidPath { path: path.into(), reason: "an untracked file has nothing staged".into() });
    }
    let hunks = if untracked {
        untracked_hunks(&client.transport.read_file(&join_repo_path(&client.repo, path), FILE_CAP).await?)?
    } else {
        parse_hunks(&raw_diff(client, action == LineAction::Unstage, path).await?)?
    };
    let (keys, everything) = match_selection(&hunks, selected)?;
    let paths = [path.to_string()];
    if everything {
        return match (action, untracked) {
            (LineAction::Stage, _) => stage(client, &paths).await,
            (LineAction::Unstage, _) => unstage(client, &paths).await,
            (LineAction::Discard, false) => discard(client, &paths, &[], false).await,
            (LineAction::Discard, true) => discard(client, &[], &paths, false).await,
        };
    }
    let patch = build_patch(&hunks, &keys, action, path, untracked && action == LineAction::Stage)?;
    let mut args = vec!["apply", "--whitespace=nowarn"];
    if action != LineAction::Discard {
        args.push("--cached");
    }
    args.push("-");
    let out = client.exec_allow_failure(GitReq::new("apply", args).stdin(patch).user()).await?;
    let mut res = result(out);
    if !res.ok && res.stderr.contains("patch does not apply") {
        res.stderr.push('\n');
        res.stderr.push_str(NEIGHBOURS);
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(kind: ChangeKind, line: u32, text: &str) -> SelectedLine {
        SelectedLine { kind, line, text: text.into() }
    }

    #[test]
    fn parses_hunks_and_numbers_lines() {
        let diff = b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -10,0 +11,2 @@\n+x\n+y\n\\ No newline at end of file\n";
        let h = parse_hunks(diff).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].lines[1].sign, h[0].lines[1].old_no), (b'-', 2));
        assert_eq!((h[0].lines[2].sign, h[0].lines[2].new_no), (b'+', 2));
        assert_eq!((h[1].lines[0].new_no, h[1].lines[1].new_no), (11, 12));
        assert!(h[1].lines[1].no_newline && !h[1].lines[0].no_newline);
        assert!(parse_hunks(b"diff --git a/x b/x\nBinary files a/x and b/x differ\n").is_err());
    }

    #[test]
    fn stage_patch_keeps_only_the_chosen_lines() {
        let diff = b"@@ -1,4 +1,4 @@\n a\n-b\n-c\n+B\n+C\n d\n";
        let h = parse_hunks(diff).unwrap();
        let (keys, all) = match_selection(&h, &[sel(ChangeKind::Del, 2, "b"), sel(ChangeKind::Add, 2, "B")]).unwrap();
        assert!(!all);
        let p = String::from_utf8(build_patch(&h, &keys, LineAction::Stage, "f", false).unwrap()).unwrap();
        assert!(p.ends_with("@@ -1,4 +1,4 @@\n a\n-b\n c\n+B\n d\n"), "{p}");
        // Unstaging the same lines turns them round and keeps the other addition as context.
        let p = String::from_utf8(build_patch(&h, &keys, LineAction::Unstage, "f", false).unwrap()).unwrap();
        assert!(p.ends_with("@@ -1,4 +1,4 @@\n a\n+b\n-B\n C\n d\n"), "{p}");
    }

    #[test]
    fn later_hunks_shift_by_what_earlier_ones_add() {
        let diff = b"@@ -1,2 +1,4 @@\n a\n+x\n+y\n b\n@@ -20,2 +22,2 @@\n c\n-d\n+D\n";
        let h = parse_hunks(diff).unwrap();
        let keys: HashSet<_> = [(ChangeKind::Add, 2), (ChangeKind::Del, 21), (ChangeKind::Add, 23)].into_iter().collect();
        let p = String::from_utf8(build_patch(&h, &keys, LineAction::Stage, "f", false).unwrap()).unwrap();
        assert!(p.contains("@@ -1,2 +1,3 @@\n a\n+x\n b\n@@ -20,2 +21,2 @@\n"), "{p}");
    }

    #[test]
    fn a_changed_file_is_refused() {
        let h = parse_hunks(b"@@ -1 +1 @@\n-a\n+b\n").unwrap();
        assert!(match_selection(&h, &[sel(ChangeKind::Add, 1, "other")]).is_err());
        assert!(match_selection(&h, &[sel(ChangeKind::Add, 2, "b")]).is_err());
        assert!(match_selection(&h, &[]).is_err());
        assert!(match_selection(&h, &[sel(ChangeKind::Add, 1, "b"), sel(ChangeKind::Del, 1, "a")]).unwrap().1);
    }

    #[test]
    fn quotes_awkward_paths() {
        assert_eq!(header_path("a/", "sp ace.txt"), "a/sp ace.txt");
        assert_eq!(header_path("a/", "q\"uote\\x\ty"), "\"a/q\\\"uote\\\\x\\ty\"");
    }
}
