//! Working tree status from `status --porcelain=v2 -z`.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::Result;
use crate::git::parse::lossy;
use crate::git::{GitClient, GitReq};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StatusEntry {
    pub path: String,
    /// Original path for renames and copies.
    pub orig_path: Option<String>,
    /// Index column of the XY pair (`.` = unchanged).
    pub x: String,
    /// Working tree column of the XY pair.
    pub y: String,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub ignored: bool,
    pub conflicted: bool,
    pub rename_score: Option<u32>,
    pub is_submodule: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StatusSnapshot {
    pub head_oid: Option<String>,
    /// Branch name, or None when detached.
    pub branch: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub stash_count: u32,
    pub entries: Vec<StatusEntry>,
    pub staged_count: u32,
    pub unstaged_count: u32,
    pub untracked_count: u32,
    pub conflicted_count: u32,
}

impl StatusSnapshot {
    pub fn is_dirty(&self) -> bool {
        !self.entries.iter().all(|e| e.ignored)
    }
}

fn split_fields(rec: &[u8], n: usize) -> Option<(Vec<String>, String)> {
    // Split the first n space-separated fields; the remainder is the path.
    let mut fields = Vec::with_capacity(n);
    let mut rest = rec;
    for _ in 0..n {
        let i = rest.iter().position(|b| *b == b' ')?;
        fields.push(lossy(&rest[..i]));
        rest = &rest[i + 1..];
    }
    Some((fields, lossy(rest)))
}

pub fn parse_status(bytes: &[u8]) -> StatusSnapshot {
    let mut snap = StatusSnapshot::default();
    let recs: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    let mut i = 0;
    while i < recs.len() {
        let rec = recs[i];
        i += 1;
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix(b"# ") {
            let text = lossy(h);
            if let Some(v) = text.strip_prefix("branch.oid ") {
                snap.head_oid = if v == "(initial)" { None } else { Some(v.to_string()) };
            } else if let Some(v) = text.strip_prefix("branch.head ") {
                if v == "(detached)" {
                    snap.detached = true;
                } else {
                    snap.branch = Some(v.to_string());
                }
            } else if let Some(v) = text.strip_prefix("branch.upstream ") {
                snap.upstream = Some(v.to_string());
            } else if let Some(v) = text.strip_prefix("branch.ab ") {
                for part in v.split(' ') {
                    if let Some(n) = part.strip_prefix('+') {
                        snap.ahead = n.parse().ok();
                    } else if let Some(n) = part.strip_prefix('-') {
                        snap.behind = n.parse().ok();
                    }
                }
            } else if let Some(v) = text.strip_prefix("stash ") {
                snap.stash_count = v.trim().parse().unwrap_or(0);
            }
            continue;
        }
        let entry = match rec[0] {
            b'1' => {
                // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
                let Some((f, path)) = split_fields(rec, 8) else { continue };
                let xy = f[1].clone();
                make_entry(path, None, &xy, &f[2], None)
            }
            b'2' => {
                // 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\0<origPath>
                let Some((f, path)) = split_fields(rec, 9) else { continue };
                let orig = recs.get(i).map(|b| lossy(b));
                i += 1;
                let score = f[8].get(1..).and_then(|s| s.parse().ok());
                make_entry(path, orig, &f[1], &f[2], score)
            }
            b'u' => {
                // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
                let Some((f, path)) = split_fields(rec, 10) else { continue };
                let mut e = make_entry(path, None, &f[1], &f[2], None);
                e.conflicted = true;
                e.staged = false;
                e.unstaged = true;
                e
            }
            b'?' => StatusEntry {
                path: lossy(&rec[2..]),
                orig_path: None,
                x: "?".into(),
                y: "?".into(),
                staged: false,
                unstaged: false,
                untracked: true,
                ignored: false,
                conflicted: false,
                rename_score: None,
                is_submodule: false,
            },
            b'!' => StatusEntry {
                path: lossy(&rec[2..]),
                orig_path: None,
                x: "!".into(),
                y: "!".into(),
                staged: false,
                unstaged: false,
                untracked: false,
                ignored: true,
                conflicted: false,
                rename_score: None,
                is_submodule: false,
            },
            _ => continue,
        };
        snap.entries.push(entry);
    }
    for e in &snap.entries {
        if e.conflicted {
            snap.conflicted_count += 1;
        } else {
            if e.staged {
                snap.staged_count += 1;
            }
            if e.unstaged {
                snap.unstaged_count += 1;
            }
            if e.untracked {
                snap.untracked_count += 1;
            }
        }
    }
    snap
}

fn make_entry(path: String, orig: Option<String>, xy: &str, sub: &str, score: Option<u32>) -> StatusEntry {
    let mut chars = xy.chars();
    let x = chars.next().unwrap_or('.').to_string();
    let y = chars.next().unwrap_or('.').to_string();
    StatusEntry {
        staged: x != ".",
        unstaged: y != ".",
        untracked: false,
        ignored: false,
        conflicted: false,
        rename_score: score,
        is_submodule: sub.starts_with('S'),
        path,
        orig_path: orig,
        x,
        y,
    }
}

pub async fn get_status(client: &GitClient) -> Result<StatusSnapshot> {
    let mut args = vec![
        "status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all", "--ahead-behind",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    if client.caps.status_show_stash {
        args.push("--show-stash".into());
    }
    let out = client.exec(GitReq::new("status", args)).await?;
    Ok(parse_status(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from the fixture repository: rename, added, unmerged, untracked.
    const SAMPLE: &[u8] = b"# branch.oid 2422c79de60063b9087b72d5296086f13ac6294a\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +3 -1\0# stash 2\x002 R. N... 100644 100644 100644 3e757656cf36eca53338e520d134963a44f793f8 3e757656cf36eca53338e520d134963a44f793f8 R100 moved.txt\0added.txt\x001 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 6fd928e49eb0a24cd20972d3b280dd2447f46fb2 staged.txt\0u DU N... 100644 000000 100644 100644 94954abda49de8615a048f8d2e64b5de848e27a1 0000000000000000000000000000000000000000 2299c3797826 a.txt\0? untracked.txt\x001 .M N... 100644 100644 100644 aaaa bbbb sp ace.txt\0";

    #[test]
    fn parses_headers_and_every_entry_kind() {
        let s = parse_status(SAMPLE);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.head_oid.as_deref(), Some("2422c79de60063b9087b72d5296086f13ac6294a"));
        assert_eq!((s.ahead, s.behind), (Some(3), Some(1)));
        assert_eq!(s.stash_count, 2);
        assert_eq!(s.entries.len(), 5);
        let r = &s.entries[0];
        assert_eq!(r.path, "moved.txt");
        assert_eq!(r.orig_path.as_deref(), Some("added.txt"));
        assert_eq!(r.rename_score, Some(100));
        assert!(r.staged && !r.unstaged);
        assert!(s.entries[1].staged);
        let u = &s.entries[2];
        assert!(u.conflicted && u.path == "a.txt");
        assert!(s.entries[3].untracked);
        let sp = &s.entries[4];
        assert_eq!(sp.path, "sp ace.txt");
        assert!(sp.unstaged && !sp.staged);
        assert_eq!((s.staged_count, s.unstaged_count, s.untracked_count, s.conflicted_count), (2, 1, 1, 1));
        assert!(s.is_dirty());
    }

    #[test]
    fn parses_initial_and_detached() {
        let s = parse_status(b"# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(s.head_oid, None);
        assert!(!s.is_dirty());
        let d = parse_status(b"# branch.oid abc\0# branch.head (detached)\0");
        assert!(d.detached && d.branch.is_none());
    }
}
