//! Git's configuration as the Repository Settings dialog shows it: every setting from every file
//! Git reads for this repository (system, global, local, worktree), with the file it came from.
//! One `git config --list --show-scope --show-origin -z`, run through the repository's own
//! transport, so for a WSL or SSH repository "global" is the global file of that machine.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::Result;
use crate::git::parse::lossy;
use crate::git::{GitClient, GitReq};

/// One setting as Git lists it. A key with several values (`safe.directory`) is several entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEntry {
    /// `system`, `global`, `local` or `worktree`.
    pub scope: String,
    /// Where Git read it: `file:.git/config`, `file:/home/me/.gitconfig`.
    pub origin: String,
    /// The key as Git lists it, lower case apart from any subsection (`remote.origin.url`).
    pub key: String,
    /// None for a key written with no `=` at all, which Git reads as true.
    pub value: Option<String>,
}

pub async fn get_config_entries(client: &GitClient) -> Result<Vec<ConfigEntry>> {
    let out = client.exec(GitReq::new("config", ["config", "--list", "--show-scope", "--show-origin", "-z"])).await?;
    Ok(parse_config_entries(&out.stdout))
}

/// Parses `git config --list --show-scope --show-origin -z`: each entry is the scope, NUL, the
/// origin, NUL, then the key, followed by a newline and the value when it has one (the value may
/// itself hold newlines), then NUL. Entries from the command line (the `-c` options every command
/// of StashPanda's carries) are left out: they are not in any file.
fn parse_config_entries(stdout: &[u8]) -> Vec<ConfigEntry> {
    let text = lossy(stdout);
    let fields: Vec<&str> = text.split('\0').collect();
    fields
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|f| f[0] != "command")
        .map(|f| {
            let (key, value) = match f[2].split_once('\n') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (f[2], None),
            };
            ConfigEntry { scope: f[0].to_string(), origin: f[1].to_string(), key: key.to_string(), value }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scopes_values_without_equals_multiline_values_and_skips_the_command_line() {
        // Captured from Git 2.43, with the scratch folder's path shortened to <S>.
        let raw = b"global\0file:<S>/g.cfg\0user.name\nGlobal Name\0local\0file:.git/config\0core.flag\0local\0file:.git/config\0safe.directory\n/a\0local\0file:.git/config\0safe.directory\n/b\0local\0file:.git/config\0test.multi\nline1\nline2\0command\0command line:\0color.ui\nnever\0";
        let entries = parse_config_entries(raw);
        let keys: Vec<(&str, &str, Option<&str>)> = entries.iter().map(|e| (e.scope.as_str(), e.key.as_str(), e.value.as_deref())).collect();
        assert_eq!(
            keys,
            [
                ("global", "user.name", Some("Global Name")),
                ("local", "core.flag", None),
                ("local", "safe.directory", Some("/a")),
                ("local", "safe.directory", Some("/b")),
                ("local", "test.multi", Some("line1\nline2")),
            ]
        );
        assert_eq!(entries[0].origin, "file:<S>/g.cfg");
        assert!(parse_config_entries(b"").is_empty());
    }
}
