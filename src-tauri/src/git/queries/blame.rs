//! Who last changed each line of a file: `git blame --porcelain`, for the working tree version
//! or the version in a commit.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tokio_util::sync::CancellationToken;

use crate::error::{GitError, Result};
use crate::git::catfile::CatFile;
use crate::git::ops::valid_revision;
use crate::git::parse::{lossy, parse_i64, unquote_path, utf8_name};
use crate::git::queries::blob::{get_blob, get_worktree_file, Eol};
use crate::git::queries::diff::join_repo_path;
use crate::git::{GitClient, GitReq};

/// A commit that last changed at least one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BlameCommit {
    pub oid: String,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub summary: String,
    /// The walk stopped at this commit: the first commit of the history, so its lines may be
    /// older than it.
    pub boundary: bool,
    /// Lines changed in the working tree or the index and not committed yet. Git names them
    /// `0000000` with the author "Not Committed Yet".
    pub uncommitted: bool,
}

/// The file in the commit before, where its lines last came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BlamePrevious {
    pub oid: String,
    /// The file's path in that commit, which differs from `BlameOrigin::path` across a rename.
    pub path: String,
}

/// A commit together with the path the file had in it. A commit usually has one; a file
/// assembled from two files in one commit has two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BlameOrigin {
    /// Index into `FileBlame::commits`.
    pub commit: u32,
    pub path: String,
    /// The same file in the commit before this one, for blaming the lines as they were before
    /// it changed them. None when the commit added the file.
    pub previous: Option<BlamePrevious>,
}

/// A run of lines that came from one origin, in the order Git reports them. Neighbouring groups
/// can share a commit, when its lines were not next to each other in that commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BlameGroup {
    /// Index into `FileBlame::origins`.
    pub origin: u32,
    /// Index of the group's first line in `FileBlame::lines`.
    pub start: u32,
    pub count: u32,
    /// Line number of the first line in the origin's version of the file, counting from 1.
    pub orig_start: u32,
}

/// What `get_blame` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum BlameContent {
    /// The file's lines, and the commit that last changed each.
    Lines,
    /// A binary file, which has no lines to blame.
    Binary,
    /// The working tree file is in no commit yet: Git does not track it, or the branch has no
    /// commits.
    NotCommitted,
    /// The file is not in that commit, or has been deleted from the working tree.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileBlame {
    pub content: BlameContent,
    pub commits: Vec<BlameCommit>,
    pub origins: Vec<BlameOrigin>,
    pub groups: Vec<BlameGroup>,
    /// The file's lines, with any `\r` before the line break removed.
    pub lines: Vec<String>,
    pub eol: Eol,
}

impl FileBlame {
    fn empty(content: BlameContent) -> FileBlame {
        FileBlame { content, commits: Vec::new(), origins: Vec::new(), groups: Vec::new(), lines: Vec::new(), eol: Eol::None }
    }
}

/// The oid Git gives lines that are not committed yet.
fn is_zero_oid(oid: &str) -> bool {
    !oid.is_empty() && oid.bytes().all(|b| b == b'0')
}

/// `<oid> <line in the origin> <line in the file> [<lines in the group>]`, which starts each line's
/// record. The count is only on the first line of a group.
fn parse_header(line: &[u8]) -> Option<(String, u32, u32, Option<u32>)> {
    let text = std::str::from_utf8(line).ok()?;
    let mut parts = text.split(' ');
    let oid = parts.next()?;
    if !(oid.len() == 40 || oid.len() == 64) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let orig = parts.next()?.parse().ok()?;
    let fin = parts.next()?.parse().ok()?;
    let count = match parts.next() {
        Some(c) => Some(c.parse().ok()?),
        None => None,
    };
    Some((oid.to_string(), orig, fin, count))
}

/// Parses `git blame --porcelain`. Each line of the file is a header line, then, the first time
/// its commit appears, the commit's details (`author`, `summary`, `boundary`, `previous`,
/// `filename`, ...), then the line itself after a tab. `previous` and `filename` come again only
/// when a commit has more than one path, so each commit's latest origin is remembered.
pub fn parse_blame(bytes: &[u8]) -> Result<FileBlame> {
    let mut blame = FileBlame::empty(BlameContent::Lines);
    let mut by_oid: HashMap<String, u32> = HashMap::new();
    let mut origin_of_commit: HashMap<u32, u32> = HashMap::new();
    let mut by_origin: HashMap<(u32, String, Option<BlamePrevious>), u32> = HashMap::new();
    // The commit of the line being read, its line number in the file, and its group when the
    // line starts one.
    let mut commit: Option<u32> = None;
    let mut line_no: u32 = 0;
    let mut group: Option<(u32, u32, u32)> = None;
    let mut previous: Option<BlamePrevious> = None;
    let (mut crlf, mut lf) = (0usize, 0usize);
    let mut binary = false;

    let mut rest = bytes;
    while !rest.is_empty() {
        let (line, after) = match rest.iter().position(|b| *b == b'\n') {
            Some(at) => (&rest[..at], &rest[at + 1..]),
            None => (rest, &rest[rest.len()..]),
        };
        rest = after;

        if let Some(text) = line.strip_prefix(b"\t") {
            let Some(c) = commit else {
                return Err(GitError::ParseError { what: "git blame --porcelain".into(), detail: "a line before its header".into() });
            };
            if let Some((count, orig_start, start)) = group.take() {
                let origin = match origin_of_commit.get(&c) {
                    Some(o) => *o,
                    // A commit always names its file the first time; this is only a fallback.
                    None => intern_origin(&mut blame, &mut by_origin, c, String::new(), None),
                };
                blame.groups.push(BlameGroup { origin, start, count, orig_start });
            }
            let (text, cr) = match text.strip_suffix(b"\r") {
                Some(t) => (t, true),
                None => (text, false),
            };
            if cr { crlf += 1 } else { lf += 1 }
            binary |= text.contains(&0);
            let index = line_no.saturating_sub(1) as usize;
            if blame.lines.len() <= index {
                blame.lines.resize(index + 1, String::new());
            }
            blame.lines[index] = lossy(text);
            continue;
        }

        if let Some((oid, orig, fin, count)) = parse_header(line) {
            let next = blame.commits.len() as u32;
            let c = *by_oid.entry(oid.clone()).or_insert(next);
            if c == next {
                blame.commits.push(BlameCommit {
                    uncommitted: is_zero_oid(&oid),
                    oid,
                    author_name: String::new(),
                    author_email: String::new(),
                    author_time: 0,
                    summary: String::new(),
                    boundary: false,
                });
            }
            commit = Some(c);
            line_no = fin;
            previous = None;
            group = count.map(|n| (n, orig, fin.saturating_sub(1)));
            continue;
        }

        let Some(c) = commit else { continue };
        let (key, value) = match line.iter().position(|b| *b == b' ') {
            Some(at) => (&line[..at], &line[at + 1..]),
            None => (line, &line[line.len()..]),
        };
        let entry = &mut blame.commits[c as usize];
        match key {
            b"author" => entry.author_name = lossy(value),
            b"author-mail" => entry.author_email = lossy(value.strip_prefix(b"<").and_then(|v| v.strip_suffix(b">")).unwrap_or(value)),
            b"author-time" => entry.author_time = parse_i64(value).unwrap_or(0),
            b"summary" => entry.summary = lossy(value),
            b"boundary" => entry.boundary = true,
            b"previous" => {
                let at = value.iter().position(|b| *b == b' ').unwrap_or(value.len());
                let path = value.get(at + 1..).unwrap_or_default();
                previous = Some(BlamePrevious { oid: lossy(&value[..at]), path: unquote_path(path) });
            }
            b"filename" => {
                let origin = intern_origin(&mut blame, &mut by_origin, c, unquote_path(value), previous.take());
                origin_of_commit.insert(c, origin);
            }
            _ => {}
        }
    }

    if binary {
        return Ok(FileBlame::empty(BlameContent::Binary));
    }
    blame.eol = match (lf, crlf) {
        (0, 0) => Eol::None,
        (_, 0) => Eol::Lf,
        (0, _) => Eol::CrLf,
        _ => Eol::Mixed,
    };
    Ok(blame)
}

fn intern_origin(
    blame: &mut FileBlame,
    by_origin: &mut HashMap<(u32, String, Option<BlamePrevious>), u32>,
    commit: u32,
    path: String,
    previous: Option<BlamePrevious>,
) -> u32 {
    let key = (commit, path, previous);
    if let Some(i) = by_origin.get(&key) {
        return *i;
    }
    let i = blame.origins.len() as u32;
    blame.origins.push(BlameOrigin { commit, path: key.1.clone(), previous: key.2.clone() });
    by_origin.insert(key, i);
    i
}

/// Blame can walk a long history; give it longer than an ordinary query.
const BLAME_TIMEOUT: Duration = Duration::from_secs(300);

/// Blames `path` as it is in commit `rev`, or in the working tree when `rev` is None (lines
/// changed and not committed yet then belong to an `uncommitted` commit). The version is read
/// first, through the long-lived `cat-file` when `helper` is given: a binary file is reported as
/// such without running blame, and one over `BLOB_CAP` is refused with `tooLarge` before blame
/// reads it.
pub async fn get_blame(
    client: &GitClient,
    helper: Option<(&Arc<CatFile>, &CancellationToken)>,
    path: &str,
    rev: Option<&str>,
) -> Result<FileBlame> {
    if path.is_empty() || path.contains(['\n', '\r', '\0']) {
        return Err(GitError::InvalidPath { path: path.to_string(), reason: "not a valid file path".into() });
    }
    utf8_name(path)?;
    if let Some(r) = rev {
        valid_revision(r)?;
    }
    let source = match rev {
        Some(r) => match get_blob(client, helper, &format!("{r}:{path}")).await {
            Err(GitError::ObjectNotFound { .. }) => return Ok(FileBlame::empty(BlameContent::Missing)),
            // Without the long-lived helper, `cat-file blob` fails with a plain error instead.
            Err(GitError::CommandFailed { stderr, .. }) if stderr.contains("does not exist") || stderr.contains("Not a valid object name") => {
                return Ok(FileBlame::empty(BlameContent::Missing));
            }
            other => other?,
        },
        None => match get_worktree_file(client, path).await {
            Ok(content) => content,
            Err(e) => {
                // The read fails differently on each transport; a file that is not there is
                // told apart by asking for its modified time, which is None for a missing file.
                let full = join_repo_path(&client.repo, path);
                let exists = client.transport.modified_times(&[full]).await.ok().and_then(|t| t.first().copied().flatten()).is_some();
                if exists {
                    return Err(e);
                }
                return Ok(FileBlame::empty(BlameContent::Missing));
            }
        },
    };
    if source.is_binary {
        return Ok(FileBlame::empty(BlameContent::Binary));
    }

    let mut args = vec!["blame".to_string(), "--porcelain".into()];
    if let Some(r) = rev {
        args.push(r.to_string());
    }
    args.push("--".into());
    args.push(path.to_string());
    let out = client.exec_allow_failure(GitReq::new("blame", args).timeout(Some(BLAME_TIMEOUT))).await?;
    if !out.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        // `no such path 'x' in HEAD` for an untracked file; an unborn branch has no HEAD at all.
        let lower = stderr.to_ascii_lowercase();
        if rev.is_none() && (lower.contains("no such path") || lower.contains("no such ref") || lower.contains("bad revision 'head'")) {
            return Ok(FileBlame::empty(BlameContent::NotCommitted));
        }
        // `blame.ignoreRevsFile` names a file the repository does not have. Git refuses to blame
        // then, and no option on the command line takes the setting back.
        if lower.contains("could not open object name list") {
            return Err(GitError::CommandFailed {
                label: "blame".into(),
                argv: vec![],
                exit_code: out.exit_code,
                stderr: format!(
                    "{}\nThe Git setting blame.ignoreRevsFile names that file, and this repository does not have it. Add the file, or remove the setting from the Git config that sets it.",
                    stderr.trim_end()
                ),
            });
        }
        return Err(crate::error::classify_git_failure("blame", vec![], out.exit_code, &out.stderr, &client.transport.kind()));
    }
    parse_blame(&out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `git blame --porcelain -- "new name.txt"` in a repository where `old.txt` was committed
    // ("first commit", the root, so a boundary), given a tab-holding line ("add four and a tab"),
    // then renamed to `new name.txt` with a non-ASCII line by a non-ASCII author; the third and
    // sixth lines are changed in the working tree and not committed.
    const WORKTREE: &[u8] = b"a45f2f6386f76836ae69a036cd0bb920c393bf25 1 1 1\n\
author Ann Example\n\
author-mail <a@x>\n\
author-time 1700000000\n\
author-tz +0200\n\
committer Ann Example\n\
committer-mail <a@x>\n\
committer-time 1700000000\n\
committer-tz +0200\n\
summary first commit\n\
boundary\n\
filename old.txt\n\
\tone\n\
aee69f1606715e0d8e7d70272954dbca1c25ebeb 2 2 1\n\
author Ann Example\n\
author-mail <a@x>\n\
author-time 1700000100\n\
author-tz +0200\n\
committer Ann Example\n\
committer-mail <a@x>\n\
committer-time 1700000100\n\
committer-tz +0200\n\
summary add four and a tab\n\
previous a45f2f6386f76836ae69a036cd0bb920c393bf25 old.txt\n\
filename old.txt\n\
\ttwo\tTAB\n\
0000000000000000000000000000000000000000 3 3 1\n\
author Not Committed Yet\n\
author-mail <not.committed.yet>\n\
author-time 1791981024\n\
author-tz +0200\n\
committer Not Committed Yet\n\
committer-mail <not.committed.yet>\n\
committer-time 1791981024\n\
committer-tz +0200\n\
summary Version of new name.txt from new name.txt\n\
previous fd8616f254861ec6a3099955abc01a5f6a574f44 new name.txt\n\
filename new name.txt\n\
\tTHREE\n\
aee69f1606715e0d8e7d70272954dbca1c25ebeb 4 4 1\n\
\tfour\n\
fd8616f254861ec6a3099955abc01a5f6a574f44 5 5 1\n\
author J\xc3\xb6rg \xc3\x9cnicode\n\
author-mail <j@x>\n\
author-time 1700000200\n\
author-tz +0200\n\
committer J\xc3\xb6rg \xc3\x9cnicode\n\
committer-mail <j@x>\n\
committer-time 1700000200\n\
committer-tz +0200\n\
summary rename and add f\xc3\xbcnf\n\
previous aee69f1606715e0d8e7d70272954dbca1c25ebeb old.txt\n\
filename new name.txt\n\
\tf\xc3\xbcnf \xc3\xbc\n\
0000000000000000000000000000000000000000 6 6 1\n\
\twip\n";

    // `git blame --porcelain HEAD -- 'say "hi".txt'`: a name Git quotes, CRLF line endings, and
    // groups of two lines whose second line has no count.
    const QUOTED_CRLF: &[u8] = b"30870d705f8c8b57d6e25bfce8adab4fa907f920 1 1 2\n\
author Ann Example\n\
author-mail <a@x>\n\
author-time 1700000300\n\
author-tz +0200\n\
committer Ann Example\n\
committer-mail <a@x>\n\
committer-time 1700000300\n\
committer-tz +0200\n\
summary quoted name\n\
filename \"say \\\"hi\\\".txt\"\n\
\ta\x0d\n\
30870d705f8c8b57d6e25bfce8adab4fa907f920 2 2\n\
\tb\x0d\n\
e5ca33bb407d2ae32d91631b30c154007850d452 3 3 1\n\
author Ann Example\n\
author-mail <a@x>\n\
author-time 1700000400\n\
author-tz +0200\n\
committer Ann Example\n\
committer-mail <a@x>\n\
committer-time 1700000400\n\
committer-tz +0200\n\
summary insert a line\n\
previous 30870d705f8c8b57d6e25bfce8adab4fa907f920 \"say \\\"hi\\\".txt\"\n\
filename \"say \\\"hi\\\".txt\"\n\
\tnew\x0d\n\
30870d705f8c8b57d6e25bfce8adab4fa907f920 3 4 2\n\
\tc\x0d\n\
30870d705f8c8b57d6e25bfce8adab4fa907f920 4 5\n\
\td\x0d\n";

    #[test]
    fn parses_renames_tabs_non_ascii_boundaries_and_uncommitted_lines() {
        let b = parse_blame(WORKTREE).unwrap();
        assert_eq!(b.content, BlameContent::Lines);
        assert_eq!(b.lines, vec!["one", "two\tTAB", "THREE", "four", "fünf ü", "wip"]);
        assert_eq!(b.eol, Eol::Lf);
        assert_eq!(b.commits.len(), 4);
        let commit_of = |line: usize| {
            let g = b.groups.iter().find(|g| (g.start as usize..(g.start + g.count) as usize).contains(&line)).unwrap();
            let o = &b.origins[g.origin as usize];
            (&b.commits[o.commit as usize], o)
        };

        let (root, origin) = commit_of(0);
        assert!(root.boundary && !root.uncommitted);
        assert_eq!((root.summary.as_str(), root.author_email.as_str(), root.author_time), ("first commit", "a@x", 1700000000));
        assert_eq!(origin.path, "old.txt");
        assert_eq!(origin.previous, None, "the root commit added the file");

        let (tab, origin) = commit_of(1);
        assert_eq!(tab.summary, "add four and a tab");
        assert!(!tab.boundary);
        assert_eq!(origin.previous, Some(BlamePrevious { oid: root.oid.clone(), path: "old.txt".into() }));
        // The fourth line comes from the same commit with no details repeated, so the same origin.
        let (again, origin_again) = commit_of(3);
        assert_eq!(again.oid, tab.oid);
        assert_eq!(origin_again, origin);

        let (wip, origin) = commit_of(2);
        assert!(wip.uncommitted);
        assert_eq!(wip.author_name, "Not Committed Yet");
        assert_eq!(origin.path, "new name.txt");
        assert_eq!(commit_of(5).0.oid, wip.oid);

        let (renamed, origin) = commit_of(4);
        assert_eq!(renamed.author_name, "Jörg Ünicode");
        assert_eq!(renamed.summary, "rename and add fünf");
        assert_eq!(origin.path, "new name.txt");
        assert_eq!(origin.previous.as_ref().map(|p| p.path.as_str()), Some("old.txt"), "the line was in old.txt before the rename");

        assert_eq!(b.groups.len(), 6);
        assert_eq!(b.groups.iter().map(|g| (g.start, g.count, g.orig_start)).collect::<Vec<_>>(), [(0, 1, 1), (1, 1, 2), (2, 1, 3), (3, 1, 4), (4, 1, 5), (5, 1, 6)]);
    }

    #[test]
    fn parses_quoted_names_crlf_and_groups_of_several_lines() {
        let b = parse_blame(QUOTED_CRLF).unwrap();
        assert_eq!(b.lines, vec!["a", "b", "new", "c", "d"]);
        assert_eq!(b.eol, Eol::CrLf);
        assert_eq!(b.commits.len(), 2);
        assert_eq!(b.origins.len(), 2);
        assert_eq!(b.origins[0].path, "say \"hi\".txt");
        assert_eq!(b.origins[1].previous.as_ref().map(|p| p.path.as_str()), Some("say \"hi\".txt"));
        let groups: Vec<_> = b.groups.iter().map(|g| (g.origin, g.start, g.count, g.orig_start)).collect();
        assert_eq!(groups, [(0, 0, 2, 1), (1, 2, 1, 3), (0, 3, 2, 3)]);
    }

    #[test]
    fn reports_binary_lines_and_rejects_a_line_without_a_header() {
        let binary = b"a45f2f6386f76836ae69a036cd0bb920c393bf25 1 1 1\nauthor A\nfilename x\n\tab\x00cd\n";
        assert_eq!(parse_blame(binary).unwrap().content, BlameContent::Binary);
        assert!(parse_blame(b"\torphan\n").is_err());
        let empty = parse_blame(b"").unwrap();
        assert!(empty.lines.is_empty() && empty.groups.is_empty());
        assert_eq!(empty.eol, Eol::None);
    }
}
