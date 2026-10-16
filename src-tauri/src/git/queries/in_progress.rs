//! A merge, rebase, cherry-pick or revert that has stopped part way, from the files Git keeps in
//! the git directory while one is open. Read through the transport's file functions, so it works
//! the same over every transport and starts no Git process: one folder listing per status
//! refresh, and a few small reads only while an operation is open.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::git::queries::diff::join_repo_path;
use crate::git::GitClient;
use crate::transport::paths::NativePath;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum InProgressKind {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct InProgress {
    pub kind: InProgressKind,
    /// The commit being merged (the first line of `MERGE_HEAD`), cherry-picked
    /// (`CHERRY_PICK_HEAD`) or reverted (`REVERT_HEAD`), or the commit a rebase stopped at
    /// (`REBASE_HEAD`). None when Git has not written it, as for a rebase stopped by `exec`.
    pub commit: Option<String>,
    /// The first line of the message the commit will get (`MERGE_MSG`), or of the message of the
    /// commit a rebase is applying.
    pub subject: Option<String>,
    /// Rebase only: the branch being rebased, such as `refs/heads/main`. None when the rebase
    /// started from a detached HEAD.
    pub branch: Option<String>,
    /// Rebase only: the commit the branch is being rebased onto.
    pub onto: Option<String>,
    /// Rebase only: which of the commits being replayed is being applied now, counting from 1.
    pub step: Option<u32>,
    /// Rebase only: how many commits the rebase replays.
    pub total: Option<u32>,
    /// Rebase only: Git was stopped part way through applying a commit (it was killed, or the
    /// program running it closed), rather than stopping by itself on a conflict or an `edit`. Its
    /// list of done steps already names the commit, but nothing marks it as stopped: no
    /// `REBASE_HEAD` and no `stopped-sha`. `git rebase --continue` would then go on without that
    /// commit, so only Abort is safe.
    pub interrupted: bool,
}

impl InProgress {
    fn new(kind: InProgressKind) -> InProgress {
        InProgress { kind, commit: None, subject: None, branch: None, onto: None, step: None, total: None, interrupted: false }
    }
}

/// The operation open in the repository whose git directory is `git_dir` (as `rev-parse
/// --absolute-git-dir` gives it, so a linked worktree's own), or None. A folder that cannot be
/// read counts as none, with a warning in the log: the status it goes with is still worth showing.
pub async fn get_in_progress(client: &GitClient, git_dir: &str) -> Option<InProgress> {
    let dir = NativePath(git_dir.to_string());
    let names = match client.transport.list_dir(&dir).await {
        Ok(Some(names)) => names,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("could not list {git_dir} to look for a merge or rebase in progress: {e}");
            return None;
        }
    };
    let has = |name: &str| names.iter().any(|n| n == name);
    let read = |name: &'static str| read_text(client, &dir, name);

    if has("rebase-merge") {
        let (head_name, onto, step, total, message, rebase_head, stopped, done) = tokio::join!(
            read("rebase-merge/head-name"),
            read("rebase-merge/onto"),
            read("rebase-merge/msgnum"),
            read("rebase-merge/end"),
            read("rebase-merge/message"),
            read("REBASE_HEAD"),
            read("rebase-merge/stopped-sha"),
            read("rebase-merge/done"),
        );
        let interrupted = rebase_head.is_none() && stopped.is_none() && done.as_deref().is_some_and(last_step_applies_a_commit);
        let mut op = rebase(head_name, onto, step, total, message, rebase_head.or(stopped));
        op.interrupted = interrupted;
        return Some(op);
    }
    if has("rebase-apply") {
        let inner = client.transport.list_dir(&join_repo_path(&dir, "rebase-apply")).await.ok().flatten().unwrap_or_default();
        // `git am` keeps its state in the same folder, marked by `applying` instead of
        // `rebasing`; it is not a rebase, and StashPanda does not run it.
        if inner.iter().any(|n| n == "rebasing") {
            let (head_name, onto, step, total, message, rebase_head, original) = tokio::join!(
                read("rebase-apply/head-name"),
                read("rebase-apply/onto"),
                read("rebase-apply/next"),
                read("rebase-apply/last"),
                read("rebase-apply/final-commit"),
                read("REBASE_HEAD"),
                read("rebase-apply/original-commit"),
            );
            return Some(rebase(head_name, onto, step, total, message, rebase_head.or(original)));
        }
    }
    let (kind, head) = if has("MERGE_HEAD") {
        (InProgressKind::Merge, "MERGE_HEAD")
    } else if has("CHERRY_PICK_HEAD") {
        (InProgressKind::CherryPick, "CHERRY_PICK_HEAD")
    } else if has("REVERT_HEAD") {
        (InProgressKind::Revert, "REVERT_HEAD")
    } else {
        return None;
    };
    let (commit, message) = tokio::join!(read(head), read("MERGE_MSG"));
    let mut op = InProgress::new(kind);
    op.commit = commit.and_then(|c| first_line(&c));
    op.subject = message.and_then(|m| first_line(&m));
    Some(op)
}

fn rebase(
    head_name: Option<String>,
    onto: Option<String>,
    step: Option<String>,
    total: Option<String>,
    message: Option<String>,
    commit: Option<String>,
) -> InProgress {
    let mut op = InProgress::new(InProgressKind::Rebase);
    // `detached HEAD` when the rebase started from one.
    op.branch = head_name.and_then(|h| first_line(&h)).filter(|h| h.starts_with("refs/"));
    op.onto = onto.and_then(|o| first_line(&o));
    op.step = step.and_then(|s| s.trim().parse().ok());
    op.total = total.and_then(|s| s.trim().parse().ok());
    op.subject = message.and_then(|m| first_line(&m));
    op.commit = commit.and_then(|c| first_line(&c));
    op
}

/// True when the last line of a rebase's `done` list is a step that applies a commit (`pick`,
/// `reword`, `squash`, `fixup`, `merge`, or their one-letter forms), rather than `exec`, `break`
/// or a label, which stop or finish without one.
fn last_step_applies_a_commit(done: &str) -> bool {
    let last = done.lines().map(str::trim).rfind(|l| !l.is_empty() && !l.starts_with('#'));
    let command = last.and_then(|l| l.split_whitespace().next()).unwrap_or("");
    matches!(command, "pick" | "p" | "reword" | "r" | "squash" | "s" | "fixup" | "f" | "merge" | "m")
}

/// A small file in the git directory as text, or None when it is missing or cannot be read.
async fn read_text(client: &GitClient, dir: &NativePath, name: &str) -> Option<String> {
    let bytes = client.transport.read_file(&join_repo_path(dir, name), 64 * 1024).await.ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The first line that is not empty, trimmed.
fn first_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interrupted_rebase_was_applying_a_commit() {
        assert!(last_step_applies_a_commit("pick 02cf715 Add an extra file\n"));
        assert!(last_step_applies_a_commit("pick aaa one\nfixup bbb two\n\n"));
        assert!(last_step_applies_a_commit("p aaa one\n"));
        // A failed `exec`, a `break` or a label stops without applying a commit.
        assert!(!last_step_applies_a_commit("pick aaa one\nexec make test\n"));
        assert!(!last_step_applies_a_commit("pick aaa one\nbreak\n"));
        assert!(!last_step_applies_a_commit("label onto\n"));
        assert!(!last_step_applies_a_commit(""));
    }
}
