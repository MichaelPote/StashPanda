//! Files changed by a commit: `diff-tree --raw --numstat -z`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::Result;
use crate::git::parse::lossy;
use crate::git::{GitClient, GitReq};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Unknown,
}

impl ChangeStatus {
    fn from_letter(c: u8) -> ChangeStatus {
        match c {
            b'A' => ChangeStatus::Added,
            b'M' => ChangeStatus::Modified,
            b'D' => ChangeStatus::Deleted,
            b'R' => ChangeStatus::Renamed,
            b'C' => ChangeStatus::Copied,
            b'T' => ChangeStatus::TypeChanged,
            b'U' => ChangeStatus::Unmerged,
            _ => ChangeStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub score: Option<u32>,
    pub old_mode: String,
    pub new_mode: String,
    pub old_oid: String,
    pub new_oid: String,
    pub added: Option<u32>,
    pub deleted: Option<u32>,
    pub binary: bool,
}

/// Parse the combined `--raw --numstat -z` stream. Raw records come first
/// (`:oldmode newmode oldoid newoid X[score]\0path\0[newpath\0]`), then the
/// numstat records (`added\tdeleted\tpath\0` or, for renames and copies,
/// `added\tdeleted\t\0old\0new\0`).
pub fn parse_raw_numstat(bytes: &[u8]) -> Vec<FileChange> {
    let toks: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    let mut out: Vec<FileChange> = Vec::new();
    let mut by_key: HashMap<(String, Option<String>), usize> = HashMap::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        i += 1;
        if t.is_empty() {
            continue;
        }
        if t[0] == b':' {
            let head = lossy(&t[1..]);
            let f: Vec<&str> = head.split(' ').collect();
            if f.len() < 5 {
                continue;
            }
            let status_field = f[4];
            let letter = status_field.as_bytes().first().copied().unwrap_or(b'?');
            let score = status_field.get(1..).and_then(|s| s.parse::<u32>().ok());
            let status = ChangeStatus::from_letter(letter);
            let (old_path, path) = if matches!(status, ChangeStatus::Renamed | ChangeStatus::Copied) {
                let old = toks.get(i).map(|b| lossy(b));
                let new = toks.get(i + 1).map(|b| lossy(b)).unwrap_or_default();
                i += 2;
                (old, new)
            } else {
                let p = toks.get(i).map(|b| lossy(b)).unwrap_or_default();
                i += 1;
                (None, p)
            };
            by_key.insert((path.clone(), old_path.clone()), out.len());
            out.push(FileChange {
                path,
                old_path,
                status,
                score,
                old_mode: f[0].to_string(),
                new_mode: f[1].to_string(),
                old_oid: f[2].to_string(),
                new_oid: f[3].to_string(),
                added: None,
                deleted: None,
                binary: false,
            });
        } else {
            // numstat: added \t deleted \t path   (path empty => rename follows)
            let text = lossy(t);
            let mut parts = text.splitn(3, '\t');
            let a = parts.next().unwrap_or("-");
            let d = parts.next().unwrap_or("-");
            let p = parts.next().unwrap_or("");
            let (old_path, path) = if p.is_empty() {
                let old = toks.get(i).map(|b| lossy(b));
                let new = toks.get(i + 1).map(|b| lossy(b)).unwrap_or_default();
                i += 2;
                (old, new)
            } else {
                (None, p.to_string())
            };
            let binary = a == "-" || d == "-";
            let added = a.parse().ok();
            let deleted = d.parse().ok();
            let idx = by_key
                .get(&(path.clone(), old_path.clone()))
                .copied()
                .or_else(|| out.iter().position(|c| c.path == path));
            if let Some(idx) = idx {
                out[idx].added = added;
                out[idx].deleted = deleted;
                out[idx].binary = binary;
            }
        }
    }
    out
}

/// Files changed in `commit` relative to `parent` (default: first parent; the
/// root commit diffs against the empty tree).
pub async fn get_commit_files(
    client: &GitClient,
    commit: &str,
    parent: Option<&str>,
) -> Result<Vec<FileChange>> {
    let mut args: Vec<String> = ["diff-tree", "-r", "-z", "-M", "-C", "--root", "--no-commit-id", "--raw", "--numstat"]
        .map(String::from)
        .to_vec();
    if let Some(p) = parent {
        args.push(p.to_string());
    }
    args.push(commit.to_string());
    let out = client.exec(GitReq::new("diff-tree", args)).await?;
    Ok(parse_raw_numstat(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from the fixture repository (added, binary modified, rename, deleted).
    const SAMPLE: &[u8] = b":000000 100644 0000000000000000000000000000000000000000 3e757656cf36eca53338e520d134963a44f793f8 A\0added.txt\0:100644 100644 e7f77f2726c8f9e3155d684e7776c3fe73bb75e9 37d5d96dc136fe828261df52d8db51c18a153325 M\0bin.dat\0:100644 100644 94954abda49de8615a048f8d2e64b5de848e27a1 8b14c4f055267310b71fb4a50e5a8736644bfc28 R070\0a.txt\0renamed.txt\0:100644 000000 587be6b4c3f93f93c489c0111bba5596147a26cb 0000000000000000000000000000000000000000 D\0sp ace.txt\x001\t0\tadded.txt\0-\t-\tbin.dat\x001\t0\t\0a.txt\0renamed.txt\x000\t1\tsp ace.txt\0";

    #[test]
    fn parses_raw_and_numstat_including_renames_and_binary() {
        let files = parse_raw_numstat(SAMPLE);
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].status, ChangeStatus::Added);
        assert_eq!((files[0].added, files[0].deleted), (Some(1), Some(0)));
        assert_eq!(files[1].status, ChangeStatus::Modified);
        assert!(files[1].binary);
        let r = &files[2];
        assert_eq!(r.status, ChangeStatus::Renamed);
        assert_eq!(r.score, Some(70));
        assert_eq!(r.old_path.as_deref(), Some("a.txt"));
        assert_eq!(r.path, "renamed.txt");
        assert_eq!((r.added, r.deleted), (Some(1), Some(0)));
        assert_eq!(files[3].status, ChangeStatus::Deleted);
        assert_eq!(files[3].path, "sp ace.txt");
    }
}
