//! Branches, tags, remotes and HEAD.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{GitError, Result};
use crate::git::parse::{lossy, parse_i64};
use crate::git::{GitClient, GitReq};
use crate::transport::Output;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RefKind {
    LocalBranch,
    RemoteBranch { remote: String },
    Tag { annotated: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefEntry {
    /// `refs/heads/dev`
    pub full_name: String,
    /// `dev`, `origin/dev`, `v1.2`
    pub short: String,
    /// Peeled commit oid (hex).
    pub target: String,
    pub kind: RefKind,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub upstream_gone: bool,
    pub is_head: bool,
    pub committer_time: i64,
    pub subject: String,
    pub worktree_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub url: Option<String>,
    pub push_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefsSnapshot {
    pub refs: Vec<RefEntry>,
    /// HEAD commit (None in an empty repository).
    pub head: Option<String>,
    /// `refs/heads/dev` when on a branch, None when detached or unborn-on-branch is reported in `head_branch_name`.
    pub head_ref: Option<String>,
    pub detached: bool,
    pub remotes: Vec<Remote>,
}

pub const FOR_EACH_REF_FORMAT: &str = "%(refname)%00%(objectname)%00%(objecttype)%00%(*objectname)%00%(upstream)%00%(upstream:track)%00%(HEAD)%00%(committerdate:unix)%00%(subject)%00%(worktreepath)";

/// The short name and kind of `full`. A remote branch's remote is the longest of `remotes` its
/// name starts with, since a remote's name may itself contain `/` (`team/origin`); the first part
/// of the name when none matches.
fn short_name(full: &str, remotes: &[Remote]) -> (String, Option<RefKind>) {
    if let Some(s) = full.strip_prefix("refs/heads/") {
        return (s.to_string(), Some(RefKind::LocalBranch));
    }
    if let Some(s) = full.strip_prefix("refs/remotes/") {
        let configured = remotes
            .iter()
            .map(|r| r.name.as_str())
            .filter(|name| s.strip_prefix(name).is_some_and(|rest| rest.starts_with('/')))
            .max_by_key(|name| name.len());
        let remote = configured.unwrap_or_else(|| s.split('/').next().unwrap_or("")).to_string();
        return (s.to_string(), Some(RefKind::RemoteBranch { remote }));
    }
    if let Some(s) = full.strip_prefix("refs/tags/") {
        return (s.to_string(), Some(RefKind::Tag { annotated: false }));
    }
    (full.to_string(), None)
}

/// `[ahead 1, behind 2]`, `[gone]`, `` -> (ahead, behind, gone)
fn parse_track(s: &str) -> (Option<u32>, Option<u32>, bool) {
    let inner = s.trim().trim_start_matches('[').trim_end_matches(']');
    if inner.is_empty() {
        return (None, None, false);
    }
    if inner == "gone" {
        return (None, None, true);
    }
    let mut ahead = Some(0);
    let mut behind = Some(0);
    for part in inner.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.trim().parse().ok();
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.trim().parse().ok();
        }
    }
    (ahead, behind, false)
}

pub fn parse_for_each_ref(bytes: &[u8], remotes: &[Remote]) -> Vec<RefEntry> {
    let mut out = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let f: Vec<&[u8]> = line.split(|b| *b == 0).collect();
        if f.len() < 9 {
            continue;
        }
        let full_name = lossy(f[0]);
        let objectname = lossy(f[1]);
        let objecttype = lossy(f[2]);
        let peeled = lossy(f[3]);
        let upstream = lossy(f[4]);
        let track = lossy(f[5]);
        let head_mark = lossy(f[6]);
        let committer_time = parse_i64(f[7]).unwrap_or(0);
        let subject = lossy(f[8]);
        let worktree = f.get(9).map(|b| lossy(b)).filter(|s| !s.is_empty());

        let (short, kind) = short_name(&full_name, remotes);
        let Some(mut kind) = kind else { continue };
        // Skip the symbolic `refs/remotes/origin/HEAD` pointer: it duplicates a branch. A branch
        // named `release/HEAD` on the remote is a branch like any other.
        if matches!(&kind, RefKind::RemoteBranch { remote } if short == format!("{remote}/HEAD")) {
            continue;
        }
        let target = if objecttype == "tag" {
            if let RefKind::Tag { annotated } = &mut kind {
                *annotated = true;
            }
            if peeled.is_empty() { objectname.clone() } else { peeled }
        } else {
            objectname.clone()
        };
        let (ahead, behind, upstream_gone) = parse_track(&track);
        out.push(RefEntry {
            full_name,
            short,
            target,
            kind,
            upstream: if upstream.is_empty() { None } else { Some(upstream) },
            ahead,
            behind,
            upstream_gone,
            is_head: head_mark.trim() == "*",
            committer_time,
            subject,
            worktree_path: worktree,
        });
    }
    out
}

/// `config -z --get-regexp` output: `key\nvalue\0`.
pub fn parse_remotes(bytes: &[u8]) -> Vec<Remote> {
    let mut remotes: Vec<Remote> = Vec::new();
    for rec in bytes.split(|b| *b == 0) {
        if rec.is_empty() {
            continue;
        }
        let text = lossy(rec);
        let (key, value) = match text.split_once('\n') {
            Some(kv) => kv,
            None => continue,
        };
        let Some(rest) = key.strip_prefix("remote.") else { continue };
        let Some((name, field)) = rest.rsplit_once('.') else { continue };
        let entry = match remotes.iter_mut().find(|r| r.name == name) {
            Some(r) => r,
            None => {
                remotes.push(Remote { name: name.to_string(), url: None, push_url: None });
                remotes.last_mut().expect("just pushed")
            }
        };
        match field {
            "url" => entry.url = Some(value.to_string()),
            "pushurl" => entry.push_url = Some(value.to_string()),
            _ => {}
        }
    }
    remotes
}

/// The commands `get_refs` runs: the refs, HEAD's oid and name, the branch name on an unborn
/// branch, and the remotes. Separate from reading their output so they can go in one batch.
pub fn refs_requests(client: &GitClient) -> Vec<GitReq> {
    let mut args = vec!["for-each-ref".to_string(), format!("--format={FOR_EACH_REF_FORMAT}")];
    if !client.caps.worktreepath {
        args[1] = args[1].replace("%00%(worktreepath)", "%00");
    }
    args.extend(["refs/heads", "refs/remotes", "refs/tags"].map(String::from));
    vec![
        GitReq::new("for-each-ref", args),
        // Fails in an empty repository; that is fine.
        GitReq::new("rev-parse", ["rev-parse", "HEAD", "--symbolic-full-name", "HEAD"]),
        // On an unborn branch `symbolic-ref` still names it.
        GitReq::new("symbolic-ref", ["symbolic-ref", "-q", "HEAD"]),
        GitReq::new("config", ["config", "-z", "--get-regexp", r"^remote\..*\.(url|pushurl)$"]),
    ]
}

/// Reads the output of `refs_requests`, in its order.
pub fn refs_from(outs: &[Output]) -> Result<RefsSnapshot> {
    let [refs_out, head_out, sym_out, remotes_out] = outs else {
        return Err(GitError::Internal { detail: format!("expected 4 outputs for the refs, got {}", outs.len()) });
    };
    if !refs_out.success() {
        return Err(GitError::CommandFailed {
            label: "for-each-ref".into(),
            argv: vec![],
            exit_code: refs_out.exit_code,
            stderr: String::from_utf8_lossy(&refs_out.stderr).into_owned(),
        });
    }
    let remotes = parse_remotes(&remotes_out.stdout);
    let refs = parse_for_each_ref(&refs_out.stdout, &remotes);
    let (head, head_ref, detached) = if head_out.success() {
        let text = String::from_utf8_lossy(&head_out.stdout);
        let mut lines = text.lines().map(str::trim);
        let oid = lines.next().unwrap_or("").to_string();
        let sym = lines.next().unwrap_or("HEAD").to_string();
        let detached = sym == "HEAD";
        (Some(oid), if detached { None } else { Some(sym) }, detached)
    } else {
        let name = String::from_utf8_lossy(&sym_out.stdout).trim().to_string();
        (None, if name.is_empty() { None } else { Some(name) }, false)
    };
    // exit 1 = no matches; anything else is a real failure
    if !remotes_out.success() && remotes_out.exit_code != Some(1) {
        return Err(GitError::CommandFailed {
            label: "config".into(),
            argv: vec![],
            exit_code: remotes_out.exit_code,
            stderr: String::from_utf8_lossy(&remotes_out.stderr).into_owned(),
        });
    }
    Ok(RefsSnapshot {
        refs,
        head,
        head_ref,
        detached,
        remotes,
    })
}

pub async fn get_refs(client: &GitClient) -> Result<RefsSnapshot> {
    let mut outs = Vec::with_capacity(4);
    for (i, req) in refs_requests(client).into_iter().enumerate() {
        // `symbolic-ref` only matters when `rev-parse HEAD` failed.
        if i == 2 && outs.get(1).is_some_and(|o: &Output| o.success()) {
            outs.push(Output { exit_code: Some(0), stdout: Vec::new(), stderr: Vec::new() });
            continue;
        }
        outs.push(client.exec_allow_failure(req).await?);
    }
    refs_from(&outs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"refs/heads/dev\x008465d1f879aa72af09409a7fd77ee7af8c056aa1\x00commit\x00\x00refs/remotes/origin/dev\x00[ahead 2, behind 1]\x00 \x001789715708\x00Merge pull request #68\x00\nrefs/heads/feature/search\x002ba812a8d1161ee13b64d2b21f3efb5cfcfb9bef\x00commit\x00\x00refs/remotes/origin/feature/search\x00\x00*\x001789662117\x00Heading change\x00/home/me/code/other-project\nrefs/heads/old\x00aaaa\x00commit\x00\x00refs/remotes/origin/old\x00[gone]\x00 \x001\x00x\x00\nrefs/remotes/origin/HEAD\x000dd1\x00commit\x00\x00\x00\x00 \x001789049590\x00Merge\x00\nrefs/remotes/origin/master\x000dd1\x00commit\x00\x00\x00\x00 \x001789049590\x00Merge\x00\nrefs/tags/v1.0\x00tagobj\x00tag\x00peeledcommit\x00\x00\x00 \x0042\x00Release\x00\n";

    #[test]
    fn parses_refs_with_tracking_head_and_tags() {
        let refs = parse_for_each_ref(SAMPLE, &[]);
        assert_eq!(refs.len(), 5, "origin/HEAD is skipped");
        let dev = &refs[0];
        assert_eq!(dev.short, "dev");
        assert_eq!(dev.kind, RefKind::LocalBranch);
        assert_eq!((dev.ahead, dev.behind), (Some(2), Some(1)));
        assert!(!dev.is_head);
        let feat = &refs[1];
        assert!(feat.is_head);
        assert_eq!(feat.worktree_path.as_deref(), Some("/home/me/code/other-project"));
        assert_eq!((feat.ahead, feat.behind), (None, None));
        assert!(refs[2].upstream_gone);
        let master = &refs[3];
        assert_eq!(master.short, "origin/master");
        assert_eq!(master.kind, RefKind::RemoteBranch { remote: "origin".into() });
        let tag = &refs[4];
        assert_eq!(tag.kind, RefKind::Tag { annotated: true });
        assert_eq!(tag.target, "peeledcommit");
    }

    #[test]
    fn remote_names_with_a_slash_and_branches_named_head() {
        let line = |name: &str| format!("{name}\x00aaaa\x00commit\x00\x00\x00\x00 \x001\x00x\x00\n");
        let bytes: String = ["refs/remotes/team/origin/HEAD", "refs/remotes/team/origin/dev", "refs/remotes/team/other", "refs/remotes/origin/HEAD", "refs/remotes/origin/release/HEAD"]
            .map(line)
            .concat();
        let remote = |name: &str| Remote { name: name.into(), url: Some("u".into()), push_url: None };
        let refs = parse_for_each_ref(bytes.as_bytes(), &[remote("origin"), remote("team"), remote("team/origin")]);
        let found: Vec<(&str, RefKind)> = refs.iter().map(|r| (r.short.as_str(), r.kind.clone())).collect();
        let on = |remote: &str| RefKind::RemoteBranch { remote: remote.into() };
        assert_eq!(found, [("team/origin/dev", on("team/origin")), ("team/other", on("team")), ("origin/release/HEAD", on("origin"))]);
    }

    #[test]
    fn parses_remotes() {
        let r = parse_remotes(b"remote.origin.url\ngit@github.com:x/y.git\0remote.origin.pushurl\nhttps://x/y\0remote.up.url\nhttps://u\0");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].name, "origin");
        assert_eq!(r[0].push_url.as_deref(), Some("https://x/y"));
        assert_eq!(r[1].url.as_deref(), Some("https://u"));
    }
}
