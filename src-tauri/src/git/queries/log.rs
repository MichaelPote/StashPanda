//! Commit records for the graph, streamed from `git log -z`.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::Result;
use crate::git::parse::{lossy, parse_i64, split_nul, split_us};
use crate::git::{GitClient, GitReq};

pub const LOG_FORMAT: &str = "%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%s";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CommitRecord {
    pub oid: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer_name: String,
    pub committer_email: String,
    pub commit_time: i64,
    pub subject: String,
}

pub fn parse_record(rec: &[u8]) -> Option<CommitRecord> {
    let f = split_us(rec);
    if f.len() < 9 {
        return None;
    }
    let parents_text = lossy(f[1]);
    Some(CommitRecord {
        oid: lossy(f[0]),
        parents: parents_text.split_whitespace().map(String::from).collect(),
        author_name: lossy(f[2]),
        author_email: lossy(f[3]),
        author_time: parse_i64(f[4]).unwrap_or(0),
        committer_name: lossy(f[5]),
        committer_email: lossy(f[6]),
        commit_time: parse_i64(f[7]).unwrap_or(0),
        // The subject is the tail; join in case it contained a stray 0x1F.
        subject: f[8..].iter().map(|b| lossy(b)).collect::<Vec<_>>().join("\u{1f}"),
    })
}

pub fn parse_log(bytes: &[u8]) -> Vec<CommitRecord> {
    split_nul(bytes).into_iter().filter_map(parse_record).collect()
}

/// The refs the graph walks: branches, tags, remotes and HEAD, never
/// `refs/stash` or notes.
pub fn graph_rev_args() -> Vec<String> {
    ["--branches", "--tags", "--remotes", "HEAD"].map(String::from).to_vec()
}

/// All commits reachable from the graph refs, up to `max_count`, in Git's
/// default (reverse chronological, streaming) order. Ordering for display is
/// done in the graph module.
pub async fn get_log(client: &GitClient, max_count: u32) -> Result<Vec<CommitRecord>> {
    let mut args = vec!["log".to_string()];
    args.extend(graph_rev_args());
    args.push("-z".into());
    args.push(format!("--max-count={max_count}"));
    args.push(format!("--format={LOG_FORMAT}"));
    let out = client
        .exec_allow_failure(GitReq::new("log", args).timeout(None))
        .await?;
    // An empty repository exits 128 ("does not have any commits yet"); treat as no commits.
    if !out.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
        if err.contains("does not have any commits")
            || err.contains("bad default revision")
            || (err.contains("ambiguous argument 'head'") && err.contains("unknown revision"))
        {
            return Ok(Vec::new());
        }
        return Err(crate::error::GitError::CommandFailed {
            label: "log".into(),
            argv: vec![],
            exit_code: out.exit_code,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(parse_log(&out.stdout))
}

/// Commits reachable from `positives` but not from `negatives`, for the
/// incremental refresh. Revisions are fed on stdin, so there is no argv limit.
pub async fn get_log_between(
    client: &GitClient,
    positives: &[String],
    negatives: &[String],
    max_count: u32,
) -> Result<Vec<CommitRecord>> {
    if positives.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin = String::new();
    for p in positives {
        stdin.push_str(p);
        stdin.push('\n');
    }
    if !negatives.is_empty() {
        stdin.push_str("--not\n");
        for n in negatives {
            stdin.push_str(n);
            stdin.push('\n');
        }
    }
    let args = vec![
        "log".to_string(),
        "--stdin".into(),
        "-z".into(),
        format!("--max-count={max_count}"),
        format!("--format={LOG_FORMAT}"),
    ];
    let out = client
        .exec(GitReq::new("log", args).stdin(stdin.into_bytes()).timeout(None))
        .await?;
    Ok(parse_log(&out.stdout))
}

/// Full commit message body for the details pane.
pub async fn get_commit_body(client: &GitClient, oid: &str) -> Result<String> {
    let out = client
        .exec(GitReq::new("show", ["show", "-s", "--format=%B", oid]))
        .await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Each parent of `oid`, in parent order, with whether the commit's tree is that parent's tree, so
/// that comparing the commit with that parent lists no changes. A merge made with `--no-ff` onto a
/// branch that had no commits of its own has the same tree as its second parent.
pub async fn get_unchanged_parents(client: &GitClient, oid: &str) -> Result<Vec<(String, bool)>> {
    let parents = format!("{oid}^@");
    let out = client
        .exec(GitReq::new("show", ["show", "-s", "--format=%H %T %P", oid, parents.as_str()]))
        .await?;
    Ok(parse_unchanged_parents(&String::from_utf8_lossy(&out.stdout)))
}

/// One `hash tree parents...` line for the commit, then one for each of its parents.
fn parse_unchanged_parents(text: &str) -> Vec<(String, bool)> {
    let mut lines = text.lines();
    let Some(first) = lines.next() else { return Vec::new() };
    let mut fields = first.split(' ');
    let (_, tree) = (fields.next(), fields.next().unwrap_or_default());
    let parents: Vec<&str> = fields.collect();
    let trees: Vec<(&str, &str)> = lines
        .filter_map(|l| {
            let mut f = l.split(' ');
            Some((f.next()?, f.next()?))
        })
        .collect();
    parents
        .iter()
        .map(|p| (p.to_string(), trees.iter().any(|(h, t)| h == p && *t == tree)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_parent_whose_tree_the_merge_kept() {
        // Each parent's line lists its own parents too.
        let text = "m1 t2 p1 p2\np1 t1 g1\np2 t2 g1 g2\n";
        let flags = |text: &str| parse_unchanged_parents(text).into_iter().map(|(_, u)| u).collect::<Vec<_>>();
        assert_eq!(parse_unchanged_parents(text), vec![("p1".to_string(), false), ("p2".to_string(), true)]);
        assert_eq!(flags("m1 t3 p1 p2\np1 t1 g1\np2 t2 g1\n"), vec![false, false]);
        assert_eq!(flags("m1 t1 p1\np1 t1\n"), vec![true], "a root parent's line has no parents");
        assert!(parse_unchanged_parents("").is_empty());
    }

    #[test]
    fn parses_merge_and_plain_records() {
        let bytes = b"dcefad88\x1fd79c58c1 8465d1f8\x1fJane Doe\x1fjane@example.com\x1f1789716747\x1fGitHub\x1fnoreply@github.com\x1f1789716747\x1fMerge pull request #69 from octo-org/dev\x002ba812a8\x1fd5215fc4\x1fMichael\x1fme@example.com\x1f1789662117\x1fMichael\x1fme@example.com\x1f1789662117\x1fHeading change\x00root\x1f\x1fA\x1fa@b\x1f1\x1fA\x1fa@b\x1f1\x1fInitial\x00";
        let recs = parse_log(bytes);
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0].parents, vec!["d79c58c1", "8465d1f8"]);
        assert_eq!(recs[0].committer_name, "GitHub");
        assert_eq!(recs[1].subject, "Heading change");
        assert!(recs[2].parents.is_empty());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    pub oid: String,
    /// `stash@{0}`
    pub selector: String,
    /// `WIP on main: abc123 subject`
    pub message: String,
    pub time: i64,
    pub index: u32,
}

pub async fn get_stashes(client: &GitClient) -> Result<Vec<StashEntry>> {
    let out = client
        .exec_allow_failure(GitReq::new("stash", ["stash", "list", "-z", "--format=%H%x1f%gd%x1f%gs%x1f%ct"]))
        .await?;
    if !out.success() {
        return Ok(Vec::new());
    }
    Ok(split_nul(&out.stdout)
        .into_iter()
        .enumerate()
        .filter_map(|(i, rec)| {
            let f = split_us(rec);
            if f.len() < 4 {
                return None;
            }
            Some(StashEntry {
                oid: lossy(f[0]),
                selector: lossy(f[1]),
                message: lossy(f[2]),
                time: parse_i64(f[3]).unwrap_or(0),
                index: i as u32,
            })
        })
        .collect())
}
