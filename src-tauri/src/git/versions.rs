//! Git version parsing and the capability flags derived from it.

use crate::error::{GitError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl std::fmt::Display for GitVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Oldest Git StashPanda supports: `cat-file --batch-command` arrived in 2.36.
pub const MIN_GIT: GitVersion = GitVersion { major: 2, minor: 36, patch: 0 };

impl GitVersion {
    /// Parse `git version 2.47.1.windows.1` or `git version 2.43.0`.
    pub fn parse(output: &[u8]) -> Result<GitVersion> {
        let text = String::from_utf8_lossy(output);
        let line = text.lines().next().unwrap_or("").trim();
        let nums = line
            .strip_prefix("git version ")
            .ok_or_else(|| GitError::ParseError {
                what: "git --version".into(),
                detail: format!("unexpected output: {line:?}"),
            })?;
        let mut parts = nums.split('.').map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .unwrap_or(0)
        });
        let major = parts.next().unwrap_or(0);
        let minor = parts.next().unwrap_or(0);
        let patch = parts.next().unwrap_or(0);
        if major == 0 {
            return Err(GitError::ParseError {
                what: "git --version".into(),
                detail: format!("unexpected output: {line:?}"),
            });
        }
        Ok(GitVersion { major, minor, patch })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    /// `cat-file --batch-command` (2.36).
    pub batch_command: bool,
    /// `for-each-ref --format=%(worktreepath)` (2.23).
    pub worktreepath: bool,
    /// `diff --diff-merges` (2.31).
    pub diff_merges: bool,
    /// `status --porcelain=v2 --show-stash` (2.24) and `--ahead-behind` (2.17).
    pub status_show_stash: bool,
}

impl Capabilities {
    pub fn for_version(v: GitVersion) -> Capabilities {
        let at_least = |maj: u32, min: u32| v >= GitVersion { major: maj, minor: min, patch: 0 };
        Capabilities {
            batch_command: at_least(2, 36),
            worktreepath: at_least(2, 23),
            diff_merges: at_least(2, 31),
            status_show_stash: at_least(2, 24),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_and_windows_versions() {
        assert_eq!(GitVersion::parse(b"git version 2.43.0\n").unwrap(), GitVersion { major: 2, minor: 43, patch: 0 });
        assert_eq!(GitVersion::parse(b"git version 2.47.1.windows.1\r\n").unwrap(), GitVersion { major: 2, minor: 47, patch: 1 });
        assert!(GitVersion::parse(b"not git").is_err());
        assert!(GitVersion { major: 2, minor: 35, patch: 9 } < MIN_GIT);
    }
}
