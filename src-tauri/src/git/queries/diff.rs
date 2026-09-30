//! Unified diff text for one file. The frontend parses hunks.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{GitError, Result};
use crate::git::parse::looks_binary;
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
}

const DIFF_CAP: usize = 8 * 1024 * 1024;

pub fn finish(mut stdout: Vec<u8>) -> DiffText {
    let bytes = stdout.len() as u64;
    let truncated = stdout.len() > DIFF_CAP;
    if truncated {
        stdout.truncate(DIFF_CAP);
    }
    let is_binary = looks_binary(&stdout)
        || String::from_utf8_lossy(&stdout[..stdout.len().min(4096)]).contains("Binary files ");
    DiffText {
        text: String::from_utf8_lossy(&stdout).into_owned(),
        is_binary,
        truncated,
        bytes,
    }
}

fn common_flags(opts: &DiffOptions) -> Vec<String> {
    let mut v = vec![
        "--no-ext-diff".to_string(),
        "--no-color".into(),
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

/// A unified diff for a brand-new file, built from its bytes.
pub fn synthesize_untracked(path: &str, content: &[u8]) -> DiffText {
    if looks_binary(content) {
        return DiffText {
            text: format!("diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n"),
            is_binary: true,
            truncated: false,
            bytes: content.len() as u64,
        };
    }
    let text = String::from_utf8_lossy(content);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::new();
    out.push_str(&format!("diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n"));
    out.push_str(&format!("@@ -0,0 +1,{} @@\n", lines.len()));
    for l in &lines {
        out.push('+');
        out.push_str(l.trim_end_matches('\n'));
        out.push('\n');
    }
    if !text.is_empty() && !text.ends_with('\n') {
        out.push_str("\\ No newline at end of file\n");
    }
    DiffText { text: out, is_binary: false, truncated: false, bytes: content.len() as u64 }
}

pub async fn get_diff(
    client: &GitClient,
    target: &DiffTarget,
    path: &str,
    old_path: Option<&str>,
    opts: &DiffOptions,
) -> Result<DiffText> {
    let mut args: Vec<String> = Vec::new();
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
            let full = join_repo_path(&client.repo, path);
            let bytes = client.transport.read_file(&full, DIFF_CAP).await?;
            return Ok(synthesize_untracked(path, &bytes));
        }
    }
    args.push("--".into());
    args.push(path.to_string());
    if let Some(op) = old_path {
        if op != path {
            args.push(op.to_string());
        }
    }
    // Internal, like the other queries: it runs whenever a file is clicked, which is viewing,
    // not an action on the repository, so it stays out of the console's Log mode.
    let out = client.exec_allow_failure(GitReq::new("diff", args)).await?;
    // `diff` exits 1 when differences exist only with --exit-code; without it, non-zero is an error.
    if !out.success() {
        return Err(GitError::CommandFailed {
            label: "diff".into(),
            argv: vec![],
            exit_code: out.exit_code,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(finish(out.stdout))
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
        let d = synthesize_untracked("new.txt", b"a\nb");
        assert!(d.text.contains("@@ -0,0 +1,2 @@\n+a\n+b\n\\ No newline at end of file\n"));
        assert!(!d.is_binary);
        assert!(synthesize_untracked("x.bin", b"\0\x01").is_binary);
    }

    #[test]
    fn joins_paths_per_flavour() {
        assert_eq!(join_repo_path(&NativePath("/home/me/code/x/".into()), "a/b.txt").0, "/home/me/code/x/a/b.txt");
        assert_eq!(join_repo_path(&NativePath(r"C:\src\x".into()), "a/b.txt").0, r"C:\src\x\a\b.txt");
    }
}
