//! Unified diff text for one file. The frontend parses hunks.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{GitError, Result};
use crate::git::parse::{looks_binary, quote_path, utf8_name};
use crate::git::queries::blob::{detect_eol, Eol};
use crate::git::queries::status::split_fields;
use crate::git::{GitClient, GitReq};
use crate::transport::paths::NativePath;

/// Which two versions of `path` to compare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DiffTarget {
    /// A commit against one of its parents (None = first parent / empty tree).
    Commit { commit: String, parent: Option<String> },
    /// Index vs HEAD.
    Staged,
    /// Working tree vs index.
    Unstaged,
    /// Working tree vs HEAD.
    WorktreeVsHead,
    /// A file Git does not know yet; rendered as all added.
    Untracked,
    /// A conflicted file: one of its versions against another. A version the conflict lacks (the
    /// side that deleted the file, or the base when both sides added it) counts as empty.
    Conflict { from: ConflictVersion, to: ConflictVersion },
}

/// One version of a conflicted file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ConflictVersion {
    /// Stage 1: the common ancestor both sides started from.
    Base,
    /// Stage 2: the branch being merged into (for a rebase, the branch being rebased onto).
    Ours,
    /// Stage 3: what is coming in (for a rebase, the commit being replayed).
    Theirs,
    /// The file on disk, with Git's conflict markers until it is resolved by hand.
    WorkingTree,
}

impl ConflictVersion {
    fn stage(self) -> Option<u8> {
        match self {
            ConflictVersion::Base => Some(1),
            ConflictVersion::Ours => Some(2),
            ConflictVersion::Theirs => Some(3),
            ConflictVersion::WorkingTree => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiffOptions {
    pub context: u32,
    pub ignore_whitespace: bool,
    pub ignore_cr_at_eol: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self { context: 3, ignore_whitespace: false, ignore_cr_at_eol: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiffText {
    pub text: String,
    pub is_binary: bool,
    pub truncated: bool,
    pub bytes: u64,
    /// Line endings of the lines the diff shows from the old version (context and removed lines),
    /// and from the new version (context and added lines). A diff shows only part of a file, so
    /// this describes those lines, which is where a change of line endings shows.
    pub old_eol: Eol,
    pub new_eol: Eol,
    /// Why there is nothing to show for a file that is listed as changed.
    pub note: Option<DiffNote>,
}

/// A listed file that has no diff to show, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DiffNote {
    /// An untracked folder that is a Git repository of its own. Git lists the folder, not its
    /// files; `git add` would record it as a submodule.
    NestedRepository,
    /// A submodule whose only change is files it does not track: `git status` reports it as
    /// modified, and `git diff` has nothing for it.
    SubmoduleUntracked,
}

/// The line endings of the old and new sides of the hunk lines in a unified diff.
pub fn diff_eols(diff: &[u8]) -> (Eol, Eol) {
    let (mut old, mut new) = (Vec::new(), Vec::new());
    let mut in_hunk = false;
    // The sides the line before was on.
    let mut last = (false, false);
    for line in diff.split_inclusive(|b| *b == b'\n') {
        if line.starts_with(b"@@ ") {
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        match line.first() {
            Some(b' ') => {
                old.extend_from_slice(line);
                new.extend_from_slice(line);
                last = (true, true);
            }
            Some(b'-') => {
                old.extend_from_slice(line);
                last = (true, false);
            }
            Some(b'+') => {
                new.extend_from_slice(line);
                last = (false, true);
            }
            // `\ No newline at end of file`: the line before ends the file with no line break.
            Some(b'\\') => {
                if last.0 {
                    old.pop();
                }
                if last.1 {
                    new.pop();
                }
                last = (false, false);
            }
            // An empty context line, as Git prints it with `diff.suppressBlankEmpty`.
            Some(b'\n') => {
                old.push(b'\n');
                new.push(b'\n');
                last = (true, true);
            }
            _ => in_hunk = false,
        }
    }
    (detect_eol(&old), detect_eol(&new))
}

const DIFF_CAP: usize = 8 * 1024 * 1024;

pub fn finish(mut stdout: Vec<u8>) -> DiffText {
    let bytes = stdout.len() as u64;
    let truncated = stdout.len() > DIFF_CAP;
    if truncated {
        stdout.truncate(DIFF_CAP);
    }
    // Git's own line for a binary file. Every line of a hunk starts with ` `, `+`, `-` or `\`, so
    // a file's text never starts a line with it.
    let is_binary = looks_binary(&stdout) || stdout.split(|b| *b == b'\n').any(|l| l.starts_with(b"Binary files "));
    let (old_eol, new_eol) = if is_binary { (Eol::None, Eol::None) } else { diff_eols(&stdout) };
    DiffText {
        text: String::from_utf8_lossy(&stdout).into_owned(),
        is_binary,
        truncated,
        bytes,
        old_eol,
        new_eol,
        note: None,
    }
}

/// The part of a diff of several files that is about `path`: the sections whose `diff --git`
/// line ends with its new name. Comparing a copy with its source lists the source's own changes
/// too, which are not the copy's.
fn only_file(diff: Vec<u8>, path: &str) -> Vec<u8> {
    let name = format!(" {}", quote_path(&format!("b/{path}")));
    let mut out = Vec::with_capacity(diff.len());
    let mut keep = true;
    for line in diff.split_inclusive(|b| *b == b'\n') {
        if let Some(header) = line.strip_prefix(b"diff --git ") {
            keep = header.strip_suffix(b"\n").unwrap_or(header).ends_with(name.as_bytes());
        }
        if keep {
            out.extend_from_slice(line);
        }
    }
    out
}

fn common_flags(opts: &DiffOptions) -> Vec<String> {
    // `--no-textconv` and `--submodule=short` keep the user's `diff.<driver>.textconv` and
    // `diff.submodule` from replacing the file's lines, which line staging works from.
    let mut v = vec![
        "--no-ext-diff".to_string(),
        "--no-color".into(),
        "--no-textconv".into(),
        "--submodule=short".into(),
        format!("-U{}", opts.context),
        "-M".into(),
        "--src-prefix=a/".into(),
        "--dst-prefix=b/".into(),
    ];
    if opts.ignore_whitespace {
        v.push("-w".into());
    }
    if opts.ignore_cr_at_eol {
        v.push("--ignore-cr-at-eol".into());
    }
    v
}

/// A unified diff that adds every line of `content` (`added`), or removes every line of it.
pub fn synthesize_whole(path: &str, content: &[u8], added: bool) -> DiffText {
    let (mode, old, new) = if added {
        ("new file mode 100644", "/dev/null".to_string(), format!("b/{path}"))
    } else {
        ("deleted file mode 100644", format!("a/{path}"), "/dev/null".to_string())
    };
    if looks_binary(content) {
        return DiffText {
            text: format!("diff --git a/{path} b/{path}\n{mode}\nBinary files {old} and {new} differ\n"),
            is_binary: true,
            truncated: false,
            bytes: content.len() as u64,
            old_eol: Eol::None,
            new_eol: Eol::None,
            note: None,
        };
    }
    let text = String::from_utf8_lossy(content);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::new();
    out.push_str(&format!("diff --git a/{path} b/{path}\n{mode}\n--- {old}\n+++ {new}\n"));
    // An empty file has no hunk at all, as in Git's own diff.
    if !lines.is_empty() {
        let (sign, header) = if added { ('+', format!("@@ -0,0 +1,{} @@\n", lines.len())) } else { ('-', format!("@@ -1,{} +0,0 @@\n", lines.len())) };
        out.push_str(&header);
        for l in &lines {
            out.push(sign);
            out.push_str(l.trim_end_matches('\n'));
            out.push('\n');
        }
        if !text.ends_with('\n') {
            out.push_str("\\ No newline at end of file\n");
        }
    }
    let eol = detect_eol(content);
    let (old_eol, new_eol) = if added { (Eol::None, eol) } else { (eol, Eol::None) };
    DiffText { text: out, is_binary: false, truncated: false, bytes: content.len() as u64, old_eol, new_eol, note: None }
}

/// Bytes of one version of a conflicted file, or None when that version does not exist: a stage
/// the conflict lacks, or a working tree file that has been deleted.
async fn conflict_version(client: &GitClient, path: &str, version: ConflictVersion, stages: &[u8]) -> Result<Option<Vec<u8>>> {
    match version.stage() {
        Some(n) if !stages.contains(&n) => Ok(None),
        Some(n) => {
            let out = client.exec(GitReq::new("cat-file", ["cat-file".to_string(), "blob".into(), format!(":{n}:{path}")])).await?;
            Ok(Some(out.stdout))
        }
        None => {
            let full = join_repo_path(&client.repo, path);
            match client.transport.read_file(&full, DIFF_CAP).await {
                Ok(bytes) => Ok(Some(bytes)),
                Err(GitError::Io { .. }) => Ok(None),
                Err(e) => Err(e),
            }
        }
    }
}

/// The diff between two versions of a conflicted file. When both are stages, `git diff` compares
/// the two blobs (`:2:<path>` and so on); a stage against the working tree is `git diff -1`, `-2`
/// or `-3`. When one version does not exist, the other is shown as wholly added or removed.
async fn conflict_diff(client: &GitClient, path: &str, from: ConflictVersion, to: ConflictVersion, opts: &DiffOptions) -> Result<DiffText> {
    if from == ConflictVersion::WorkingTree {
        return Err(GitError::InvalidPath { path: path.to_string(), reason: "the working tree can only be compared with, not from".into() });
    }
    let out = client.exec(GitReq::new("ls-files", ["--literal-pathspecs", "ls-files", "-u", "-z", "--", path])).await?;
    let stages: Vec<u8> = crate::git::ops::parse_unmerged(&out.stdout).remove(path).unwrap_or_default();
    if let (Some(a), Some(b)) = (from.stage(), to.stage()) {
        if stages.contains(&a) && stages.contains(&b) {
            let mut args = vec!["diff".to_string()];
            args.extend(common_flags(opts));
            args.push(format!(":{a}:{path}"));
            args.push(format!(":{b}:{path}"));
            let out = client.exec(GitReq::new("diff", args)).await?;
            return Ok(finish(out.stdout));
        }
    }
    let (from_bytes, to_bytes) =
        tokio::try_join!(conflict_version(client, path, from, &stages), conflict_version(client, path, to, &stages))?;
    match (from_bytes, to_bytes) {
        (Some(_), Some(_)) => {
            let stage = from.stage().expect("from is a stage");
            let mut args = vec!["--literal-pathspecs".to_string(), "diff".into(), format!("-{stage}")];
            args.extend(common_flags(opts));
            args.push("--".into());
            args.push(path.to_string());
            let out = client.exec(GitReq::new("diff", args)).await?;
            // Git starts the output with `* Unmerged path <path>`, which is not part of the diff.
            let text: Vec<u8> = out
                .stdout
                .split_inclusive(|b| *b == b'\n')
                .filter(|l| !l.starts_with(b"* Unmerged path "))
                .flatten()
                .copied()
                .collect();
            Ok(finish(text))
        }
        (None, Some(bytes)) => Ok(synthesize_whole(path, &bytes, true)),
        (Some(bytes), None) => Ok(synthesize_whole(path, &bytes, false)),
        (None, None) => Ok(finish(Vec::new())),
    }
}

pub async fn get_diff(
    client: &GitClient,
    target: &DiffTarget,
    path: &str,
    old_path: Option<&str>,
    opts: &DiffOptions,
) -> Result<DiffText> {
    utf8_name(path)?;
    if let Some(op) = old_path {
        utf8_name(op)?;
    }
    // A copy is told from its source only with `-C`; see `only_file`.
    let other = old_path.filter(|op| *op != path);
    // The path names exactly that file: `:x`, `*` and `a[1]` are not pathspec magic or patterns.
    let mut args: Vec<String> = vec!["--literal-pathspecs".into()];
    match target {
        DiffTarget::Commit { commit, parent } => {
            match parent {
                Some(p) => {
                    args.push("diff".into());
                    args.extend(common_flags(opts));
                    args.push(p.clone());
                    args.push(commit.clone());
                }
                None => {
                    // First parent, or the empty tree for a root commit.
                    args.push("diff-tree".into());
                    args.push("-p".into());
                    args.push("--root".into());
                    args.extend(common_flags(opts));
                    args.push(commit.clone());
                }
            }
        }
        DiffTarget::Staged => {
            args.push("diff".into());
            args.push("--cached".into());
            args.extend(common_flags(opts));
        }
        DiffTarget::Unstaged => {
            args.push("diff".into());
            args.extend(common_flags(opts));
        }
        DiffTarget::WorktreeVsHead => {
            args.push("diff".into());
            args.push("HEAD".into());
            args.extend(common_flags(opts));
        }
        DiffTarget::Untracked => {
            // Git lists a nested repository as its folder, with a slash at the end.
            if path.ends_with('/') {
                return Ok(DiffText { note: Some(DiffNote::NestedRepository), ..finish(Vec::new()) });
            }
            // The file as Git would add it: a symbolic link is its target with mode 120000, an
            // executable has mode 100755. `--no-index` exits 1 when the files differ, and also
            // when it cannot read the file, with nothing printed.
            args.push("diff".into());
            args.push("--no-index".into());
            args.extend(common_flags(opts));
            args.extend(["--".into(), "/dev/null".into(), path.to_string()]);
            let out = client.exec_allow_failure(GitReq::new("diff", args)).await?;
            if out.exit_code != Some(1) || out.stdout.is_empty() {
                return Err(diff_failed(out));
            }
            return Ok(finish(out.stdout));
        }
        DiffTarget::Conflict { from, to } => return conflict_diff(client, path, *from, *to, opts).await,
    }
    if other.is_some() {
        args.push("-C".into());
    }
    args.push("--".into());
    args.push(path.to_string());
    if let Some(op) = other {
        args.push(op.to_string());
    }
    // Internal, like the other queries: it runs whenever a file is clicked, which is viewing,
    // not an action on the repository, so it stays out of the console's Log mode.
    let out = client.exec_allow_failure(GitReq::new("diff", args)).await?;
    // `diff` exits 1 when differences exist only with --exit-code; without it, non-zero is an error.
    if !out.success() {
        return Err(diff_failed(out));
    }
    let stdout = if other.is_some() { only_file(out.stdout, path) } else { out.stdout };
    if stdout.is_empty() && *target == DiffTarget::Unstaged && submodule_untracked_only(client, path).await? {
        return Ok(DiffText { note: Some(DiffNote::SubmoduleUntracked), ..finish(stdout) });
    }
    Ok(finish(stdout))
}

fn diff_failed(out: crate::transport::Output) -> GitError {
    GitError::CommandFailed { label: "diff".into(), argv: vec![], exit_code: out.exit_code, stderr: String::from_utf8_lossy(&out.stderr).into_owned() }
}

/// Whether `path` is a submodule whose only change is files it does not track: `S..U` in
/// `git status --porcelain=v2`.
async fn submodule_untracked_only(client: &GitClient, path: &str) -> Result<bool> {
    let out = client.exec(GitReq::new("status", ["--literal-pathspecs", "status", "--porcelain=v2", "-z", "--", path])).await?;
    Ok(out.stdout.split(|b| *b == 0).any(|rec| {
        rec.starts_with(b"1 ") && split_fields(rec, 8).is_some_and(|(f, p)| f[2] == "S..U" && p == path)
    }))
}

/// Join a repo-relative path onto the native repository path.
pub fn join_repo_path(repo: &NativePath, rel: &str) -> NativePath {
    let sep = if repo.0.contains('\\') && !repo.0.starts_with('/') { '\\' } else { '/' };
    let rel = if sep == '\\' { rel.replace('/', "\\") } else { rel.to_string() };
    NativePath(format!("{}{}{}", repo.0.trim_end_matches(['/', '\\']), sep, rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesizes_untracked_diff() {
        let d = synthesize_whole("new.txt", b"a\nb", true);
        assert!(d.text.contains("@@ -0,0 +1,2 @@\n+a\n+b\n\\ No newline at end of file\n"));
        assert!(!d.is_binary);
        assert!(synthesize_whole("x.bin", b"\0\x01", true).is_binary);
    }

    #[test]
    fn binary_only_from_gits_own_line() {
        // A text file whose added line says "Binary files", and Git's line for a binary file.
        let text = b"diff --git a/notes.md b/notes.md\nindex 937dd2a..cc135f3 100644\n--- a/notes.md\n+++ b/notes.md\n@@ -1 +1,2 @@\n Some docs\n+Binary files are stored with LFS.\n";
        assert!(!finish(text.to_vec()).is_binary);
        let binary = b"diff --git a/bin.dat b/bin.dat\nindex 0000000..bdc955b\nBinary files a/bin.dat and b/bin.dat differ\n";
        assert!(finish(binary.to_vec()).is_binary);
    }

    #[test]
    fn keeps_only_the_copy_from_a_diff_with_its_source() {
        // `diff -C -- copy.txt src.txt` after copying src.txt and changing it, and a name Git quotes.
        let diff = b"diff --git a/src.txt b/copy.txt\nsimilarity index 100%\ncopy from src.txt\ncopy to copy.txt\ndiff --git a/src.txt b/src.txt\nindex ac9837c..4355ac1 100644\n--- a/src.txt\n+++ b/src.txt\n@@ -30 +30,2 @@\n line 30\n+extra\n";
        assert_eq!(only_file(diff.to_vec(), "copy.txt"), b"diff --git a/src.txt b/copy.txt\nsimilarity index 100%\ncopy from src.txt\ncopy to copy.txt\n");
        let quoted = b"diff --git a/a.txt \"b/t\\tb.txt\"\nsimilarity index 100%\ncopy from a.txt\ncopy to \"t\\tb.txt\"\ndiff --git a/a.txt b/a.txt\n";
        assert_eq!(only_file(quoted.to_vec(), "t\tb.txt").len(), quoted.len() - b"diff --git a/a.txt b/a.txt\n".len());
    }

    #[test]
    fn finds_line_endings_on_each_side() {
        let d = b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n same\n-old\n+new\r\n";
        assert_eq!(diff_eols(d), (Eol::Lf, Eol::Mixed));
        assert_eq!(diff_eols(b"@@ -1 +1 @@\n-a\r\n+a\n"), (Eol::CrLf, Eol::Lf));
        // A line with no line break after it, as in the file, has no line ending.
        assert_eq!(diff_eols(b"@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+a\n"), (Eol::None, Eol::Lf));
        assert_eq!(diff_eols(b"@@ -0,0 +1,2 @@\n+a\r\n+b\n\\ No newline at end of file\n"), (Eol::None, Eol::CrLf));
        assert_eq!(synthesize_whole("n.txt", b"x\r\ny\r\n", true).new_eol, Eol::CrLf);
        // With `diff.suppressBlankEmpty`, a blank context line is a bare newline, and the lines
        // after it are still part of the hunk.
        assert_eq!(diff_eols(b"@@ -1,3 +1,3 @@\n-a\n+A\n\n b\r\n"), (Eol::Mixed, Eol::Mixed));
    }

    #[test]
    fn joins_paths_per_flavour() {
        assert_eq!(join_repo_path(&NativePath("/home/me/code/x/".into()), "a/b.txt").0, "/home/me/code/x/a/b.txt");
        assert_eq!(join_repo_path(&NativePath(r"C:\src\x".into()), "a/b.txt").0, r"C:\src\x\a\b.txt");
    }
}
