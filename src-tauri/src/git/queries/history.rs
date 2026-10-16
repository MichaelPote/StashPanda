//! Every commit that changed one file, following it back through renames:
//! `git log --follow --name-status -z`.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{GitError, Result};
use crate::git::ops::valid_revision;
use crate::git::parse::{lossy, parse_i64, split_us, utf8_name};
use crate::git::queries::commit_files::ChangeStatus;
use crate::git::{GitClient, GitReq};

/// The commit fields, after a record separator (0x1E) so that each commit's record can be told
/// from the file list `--name-status` prints after it.
const HISTORY_FORMAT: &str = "%x1e%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryEntry {
    pub oid: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub subject: String,
    /// The file's path in this commit. It changes across renames.
    pub path: String,
    /// Where the file came from when this commit renamed or copied it.
    pub old_path: Option<String>,
    /// What this commit did to the file.
    pub status: ChangeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileHistory {
    /// Newest first.
    pub entries: Vec<FileHistoryEntry>,
    /// The history goes on past `entries`: ask again with a larger count to see more.
    pub more: bool,
}

/// Parses the output of `log --follow -z --name-status --cc --format=HISTORY_FORMAT`, reading at
/// most `limit` commits. Each record is `0x1E` and the commit fields separated by `0x1F`, a NUL,
/// a newline, then the file's status letter (with a similarity score for a rename or copy), a
/// NUL, and its path or old and new paths, each followed by a NUL. A merge has two NULs and a
/// letter for each parent (`MM`). `path` is the file's path at the newest commit.
///
/// A merge with no file list is left out: it did not change the file. `--follow` turns off the
/// history simplification that `git log -- <path>` uses to drop such merges, so Git lists every
/// merge on the way, but `--cc` gives a file list only to a merge whose file differs from every
/// parent's. One that took the file unchanged from one side has the commit that changed it on
/// that side listed already. `more` counts the commits Git printed, not the entries kept, so
/// asking for more still reaches past a stretch of left-out merges.
pub fn parse_file_history(bytes: &[u8], path: &str, limit: usize) -> FileHistory {
    let mut current = path.to_string();
    let mut out = Vec::new();
    let mut commits = 0;
    let mut more = false;
    // Token by token: a path is taken by its place and never looked into, so a name can hold any
    // byte, and a subject can hold 0x1E.
    let tokens: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        i += 1;
        let Some(head) = token.strip_prefix(b"\n").unwrap_or(token).strip_prefix(b"\x1e") else { continue };
        let f = split_us(head);
        if f.len() < 6 {
            continue;
        }
        if commits == limit {
            more = true;
            break;
        }
        commits += 1;
        // The status letter, after a newline, or after an empty token for a merge.
        while tokens.get(i).is_some_and(|t| t.is_empty()) {
            i += 1;
        }
        let letter = match tokens.get(i).map(|t| t.strip_prefix(b"\n").unwrap_or(t)) {
            Some(t) if t.first().is_some_and(|b| *b != 0x1e) => {
                i += 1;
                t
            }
            // A merge that did not change the file.
            _ => continue,
        };
        let status = ChangeStatus::from_letter(letter[0]);
        // A rename or copy has a score and both names; a merge's letters (`RM`) come with one.
        let (old_path, file_path) = if matches!(status, ChangeStatus::Renamed | ChangeStatus::Copied) && letter[1..].iter().all(u8::is_ascii_digit) {
            let names = (tokens.get(i).map(|t| lossy(t)), tokens.get(i + 1).map(|t| lossy(t)).unwrap_or_else(|| current.clone()));
            i += 2;
            names
        } else {
            let name = tokens.get(i).map(|t| lossy(t)).unwrap_or_else(|| current.clone());
            i += 1;
            (None, name)
        };
        // Older commits know the file by the name it had before this one renamed it.
        current = old_path.clone().unwrap_or_else(|| file_path.clone());
        out.push(FileHistoryEntry {
            oid: lossy(f[0]),
            parents: lossy(f[1]).split_whitespace().map(String::from).collect(),
            author_name: lossy(f[2]),
            author_email: lossy(f[3]),
            author_time: parse_i64(f[4]).unwrap_or(0),
            // The subject is the tail; join in case it contained a stray 0x1F.
            subject: f[5..].iter().map(|b| lossy(b)).collect::<Vec<_>>().join("\u{1f}"),
            path: file_path,
            old_path,
            status,
        });
    }
    FileHistory { entries: out, more }
}

/// The commits that changed `path`, newest first, starting from `rev` (HEAD when None), up to
/// `max_count`. `git log --follow` prints nothing at all when given `--skip`, so a longer history
/// is read by asking again with a larger `max_count`, not by paging.
///
/// Merges that did not change the file are left out (see `parse_file_history`), so the first
/// `max_count` commits Git prints can hold fewer entries than that, or none. Git is then asked
/// again for twice as many commits, until there are enough entries or the history ends.
pub async fn get_file_history(client: &GitClient, path: &str, rev: Option<&str>, max_count: u32) -> Result<FileHistory> {
    if path.is_empty() || path.contains(['\n', '\r', '\0']) {
        return Err(GitError::InvalidPath { path: path.to_string(), reason: "not a valid file path".into() });
    }
    utf8_name(path)?;
    if let Some(r) = rev {
        valid_revision(r)?;
    }
    let max_count = max_count.max(1);
    let mut commits = max_count;
    loop {
        let mut history = read_file_history(client, path, rev, commits).await?;
        if history.entries.len() >= max_count as usize || !history.more || commits == u32::MAX {
            if history.entries.len() > max_count as usize {
                history.entries.truncate(max_count as usize);
                history.more = true;
            }
            return Ok(history);
        }
        commits = commits.saturating_mul(2);
    }
}

/// The entries among the first `commits` commits `git log --follow` prints for `path`.
async fn read_file_history(client: &GitClient, path: &str, rev: Option<&str>, commits: u32) -> Result<FileHistory> {
    let mut args = vec![
        // The path names exactly that file: `:x`, `*` and `a[1]` are not pathspec magic or patterns.
        "--literal-pathspecs".to_string(),
        "log".into(),
        "--follow".into(),
        "-z".into(),
        "--name-status".into(),
        // Without a merge diff, `--follow` shows no merge with a file list, not even one that
        // resolved a conflict in the file; with `--cc`, a merge whose file differs from every
        // parent's gets one. Merges without one are left out by `parse_file_history`.
        "--cc".into(),
        "--no-ext-diff".into(),
        // `log.showSignature` would print each signed commit's check before its record.
        "--no-show-signature".into(),
        // One more than asked for, to learn whether there are more.
        format!("--max-count={}", commits.saturating_add(1)),
        format!("--format={HISTORY_FORMAT}"),
    ];
    args.push(rev.unwrap_or("HEAD").to_string());
    args.push("--".into());
    args.push(path.to_string());
    let out = client.exec_allow_failure(GitReq::new("log", args).timeout(None)).await?;
    if !out.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
        // A branch with no commits has no history yet.
        if rev.is_none() && (err.contains("does not have any commits") || err.contains("bad revision 'head'") || err.contains("unknown revision")) {
            return Ok(FileHistory { entries: Vec::new(), more: false });
        }
        return Err(crate::error::classify_git_failure("log", vec![], out.exit_code, &out.stderr, &client.transport.kind()));
    }
    Ok(parse_file_history(&out.stdout, path, commits as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    // `git log --follow -z --name-status --format=HISTORY_FORMAT -- "new name.txt"` in a
    // repository where `old.txt` was added, changed, then renamed to `new name.txt` by an author
    // with a non-ASCII name, in a commit with a non-ASCII subject.
    const FOLLOW: &[u8] = b"\x1efd8616f254861ec6a3099955abc01a5f6a574f44\x1faee69f1606715e0d8e7d70272954dbca1c25ebeb\x1fJ\xc3\xb6rg \xc3\x9cnicode\x1fj@x\x1f1700000200\x1frename and add f\xc3\xbcnf\x00\n\
R071\x00old.txt\x00new name.txt\x00\x1eaee69f1606715e0d8e7d70272954dbca1c25ebeb\x1fa45f2f6386f76836ae69a036cd0bb920c393bf25\x1fAnn Example\x1fa@x\x1f1700000100\x1fadd four and a tab\x00\n\
M\x00old.txt\x00\x1ea45f2f6386f76836ae69a036cd0bb920c393bf25\x1f\x1fAnn Example\x1fa@x\x1f1700000000\x1ffirst commit\x00\n\
A\x00old.txt\x00";

    #[test]
    fn follows_the_file_back_through_a_rename() {
        let h = parse_file_history(FOLLOW, "new name.txt", usize::MAX).entries;
        assert_eq!(h.len(), 3);
        let rename = &h[0];
        assert_eq!(rename.status, ChangeStatus::Renamed);
        assert_eq!((rename.path.as_str(), rename.old_path.as_deref()), ("new name.txt", Some("old.txt")));
        assert_eq!(rename.author_name, "Jörg Ünicode");
        assert_eq!(rename.subject, "rename and add fünf");
        assert_eq!(rename.author_time, 1700000200);
        assert_eq!(rename.parents, vec!["aee69f1606715e0d8e7d70272954dbca1c25ebeb"]);
        assert_eq!((h[1].status, h[1].path.as_str(), h[1].old_path.as_deref()), (ChangeStatus::Modified, "old.txt", None));
        assert_eq!((h[2].status, h[2].path.as_str()), (ChangeStatus::Added, "old.txt"));
        assert!(h[2].parents.is_empty(), "the root commit has no parents");
    }

    #[test]
    fn keeps_spaces_in_names_and_0x1e_in_subjects() {
        // Captured (oids shortened): ` lead.txt` renamed to `trail.txt `, after a commit whose
        // subject holds 0x1E and a merge that resolved a conflict in the file.
        const ODD: &[u8] = b"\x1ec0\x1fc1\x1fT\x1ft@t\x1f1700000000\x1frename\x00\nR100\x00 lead.txt\x00trail.txt \x00\x1ec1\x1fc2\x1fT\x1ft@t\x1f1700000000\x1fedit after \x1e odd\x00\nM\x00 lead.txt\x00\x1ec2\x1fc3 c4\x1fT\x1ft@t\x1f1700000000\x1fconflict merge\x00\x00MM\x00 lead.txt\x00\x1ec3\x1fc5\x1fT\x1ft@t\x1f1700000000\x1fmain\x00\nM\x00 lead.txt\x00";
        let all = parse_file_history(ODD, "trail.txt ", usize::MAX);
        let got: Vec<(&str, &str, Option<&str>)> = all.entries.iter().map(|e| (e.subject.as_str(), e.path.as_str(), e.old_path.as_deref())).collect();
        assert_eq!(
            got,
            [
                ("rename", "trail.txt ", Some(" lead.txt")),
                ("edit after \u{1e} odd", " lead.txt", None),
                ("conflict merge", " lead.txt", None),
                ("main", " lead.txt", None),
            ]
        );
        assert_eq!(all.entries[2].parents, ["c3", "c4"]);
        let capped = parse_file_history(ODD, "trail.txt ", 2);
        assert_eq!((capped.entries.len(), capped.more), (2, true));
    }

    #[test]
    fn a_merge_without_a_file_change_is_left_out() {
        // A merge as `--cc` lists it (captured from a merge that resolved a conflict): two NULs,
        // then a status letter for each parent.
        let merge = b"\x1e166be4f\x1fp1 p2\x1fAnn Example\x1fa@x\x1f6\x1fconflict merge\x00\x00MM\x00g.txt\x00";
        let h = parse_file_history(merge, "g.txt", usize::MAX).entries;
        assert_eq!((h[0].status, h[0].path.as_str(), h[0].parents.len()), (ChangeStatus::Modified, "g.txt", 2));
        // A merge with no file list did not change the file; the path is still followed past it.
        let bytes = b"\x1em1\x1fp1 p2\x1fA\x1fa@x\x1f5\x1fmerge\x00\x1ec1\x1fp0\x1fA\x1fa@x\x1f4\x1frename\x00\nR100\x00a.txt\x00b.txt\x00\x1ec0\x1f\x1fA\x1fa@x\x1f3\x1fadd\x00\nA\x00a.txt\x00";
        let all = parse_file_history(bytes, "b.txt", usize::MAX);
        let h = &all.entries;
        assert_eq!(h.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["rename", "add"]);
        assert_eq!((h[0].path.as_str(), h[0].old_path.as_deref()), ("b.txt", Some("a.txt")));
        assert_eq!(h[1].path, "a.txt");
        assert!(!all.more);
        // The limit counts the commits Git printed, the left-out merge included.
        let capped = parse_file_history(bytes, "b.txt", 2);
        assert_eq!((capped.entries.len(), capped.more), (1, true));
        assert!(parse_file_history(b"", "x", usize::MAX).entries.is_empty());
    }
}
