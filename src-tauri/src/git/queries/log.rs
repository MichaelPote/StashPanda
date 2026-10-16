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
/// `refs/stash` or notes. `head` is false on a branch with no commits yet, where naming `HEAD`
/// makes `git log` fail.
pub fn graph_rev_args(head: bool) -> Vec<String> {
    let mut args = ["--branches", "--tags", "--remotes"].map(String::from).to_vec();
    if head {
        args.push("HEAD".into());
    }
    args
}

/// Splits a stream of NUL-terminated records that arrives in pieces of any size, so a record cut
/// in two by a read is joined back together before it is parsed.
#[derive(Default)]
pub struct NulRecords {
    carry: Vec<u8>,
}

impl NulRecords {
    /// Calls `each` with every record that `chunk` completes.
    pub fn push(&mut self, chunk: &[u8], mut each: impl FnMut(&[u8])) {
        let mut rest = chunk;
        while let Some(at) = rest.iter().position(|b| *b == 0) {
            if self.carry.is_empty() {
                each(&rest[..at]);
            } else {
                self.carry.extend_from_slice(&rest[..at]);
                each(&self.carry);
                self.carry.clear();
            }
            rest = &rest[at + 1..];
        }
        self.carry.extend_from_slice(rest);
    }

    /// Calls `each` with what is left after the last NUL, if anything: the last record of a
    /// stream that does not end in a NUL.
    pub fn finish(&mut self, mut each: impl FnMut(&[u8])) {
        if !self.carry.is_empty() {
            each(&self.carry);
            self.carry.clear();
        }
    }
}

/// All commits reachable from the graph refs, up to `max_count`, in Git's
/// default (reverse chronological, streaming) order. Ordering for display is
/// done in the graph module.
pub async fn get_log(client: &GitClient, max_count: u32) -> Result<Vec<CommitRecord>> {
    let mut records = Vec::new();
    stream_log(client, max_count, true, &mut |r| records.push(r)).await?;
    Ok(records)
}

/// `get_log`, handing each record to `on_record` as soon as Git has printed it, so a long history
/// can be shown while it loads. Returns the number of records. `head` as for `graph_rev_args`.
pub async fn stream_log(client: &GitClient, max_count: u32, head: bool, on_record: &mut (dyn FnMut(CommitRecord) + Send)) -> Result<u32> {
    let mut args = vec!["log".to_string()];
    args.extend(graph_rev_args(head));
    args.push("-z".into());
    // `log.showSignature` would print each signed commit's check before its record.
    args.push("--no-show-signature".into());
    args.push(format!("--max-count={max_count}"));
    args.push(format!("--format={LOG_FORMAT}"));
    let mut splitter = NulRecords::default();
    let mut count = 0u32;
    let mut each = |rec: &[u8]| {
        if let Some(r) = parse_record(rec) {
            count += 1;
            on_record(r);
        }
    };
    let out = client
        .exec_reading_stdout(GitReq::new("log", args).timeout(None), &mut |chunk| splitter.push(chunk, &mut each))
        .await?;
    splitter.finish(&mut each);
    // An empty repository exits 128 ("does not have any commits yet"); treat as no commits.
    if !out.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
        if err.contains("does not have any commits") || err.contains("bad default revision") {
            return Ok(count);
        }
        return Err(crate::error::GitError::CommandFailed {
            label: "log".into(),
            argv: vec![],
            exit_code: out.exit_code,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(count)
}

/// Which of `oids` (full hex) are no longer in the object database, for instance a commit that
/// a force push replaced and `git gc` has since pruned. One `cat-file --batch-check` for all.
pub async fn missing_objects(client: &GitClient, oids: &[String]) -> Result<Vec<String>> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin = String::new();
    for o in oids {
        stdin.push_str(o);
        stdin.push('\n');
    }
    let out = client
        .exec(GitReq::new("cat-file", ["cat-file", "--batch-check"]).stdin(stdin.into_bytes()))
        .await?;
    Ok(parse_missing(&String::from_utf8_lossy(&out.stdout)))
}

/// The `<oid> missing` lines of `cat-file --batch-check`; an object that exists prints
/// `<oid> <type> <size>` instead.
fn parse_missing(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| l.strip_suffix(" missing")).map(String::from).collect()
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
        "--no-show-signature".into(),
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
        .exec(GitReq::new("show", ["show", "-s", "--no-show-signature", "--format=%B", oid]))
        .await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Each parent of `oid`, in parent order, with whether the commit's tree is that parent's tree, so
/// that comparing the commit with that parent lists no changes. A merge made with `--no-ff` onto a
/// branch that had no commits of its own has the same tree as its second parent.
pub async fn get_unchanged_parents(client: &GitClient, oid: &str) -> Result<Vec<(String, bool)>> {
    let parents = format!("{oid}^@");
    let out = client
        .exec(GitReq::new("show", ["show", "-s", "--no-show-signature", "--format=%H %T %P", oid, parents.as_str()]))
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
    fn records_cut_across_reads_are_joined() {
        let bytes = b"a\x1fone\x00bb\x1ftwo\x00\x00ccc\x1fthree";
        for size in [1, 2, 3, 5, bytes.len()] {
            let mut splitter = NulRecords::default();
            let mut seen: Vec<Vec<u8>> = Vec::new();
            for chunk in bytes.chunks(size) {
                splitter.push(chunk, |r| seen.push(r.to_vec()));
            }
            splitter.finish(|r| seen.push(r.to_vec()));
            let expected: Vec<Vec<u8>> = vec![b"a\x1fone".to_vec(), b"bb\x1ftwo".to_vec(), Vec::new(), b"ccc\x1fthree".to_vec()];
            assert_eq!(seen, expected, "read size {size}");
        }
        assert_eq!(parse_missing("abc missing\ndef commit 281\n"), vec!["abc".to_string()]);
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
        .exec_allow_failure(GitReq::new("stash", ["stash", "list", "-z", "--no-show-signature", "--format=%H%x1f%gd%x1f%gs%x1f%ct"]))
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
