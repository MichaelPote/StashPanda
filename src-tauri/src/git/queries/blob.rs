//! File contents: a committed or indexed blob, or the working tree copy.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::Result;
use crate::git::parse::looks_binary;
use crate::git::queries::diff::join_repo_path;
use crate::git::{GitClient, GitReq};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Eol {
    Lf,
    CrLf,
    Mixed,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BlobContent {
    /// Lossy UTF-8 text with `\r` stripped for display; empty when binary.
    pub text: String,
    pub is_binary: bool,
    pub eol: Eol,
    pub size: u64,
    pub truncated: bool,
}

pub const BLOB_CAP: usize = 16 * 1024 * 1024;

pub fn detect_eol(bytes: &[u8]) -> Eol {
    let mut lf = 0usize;
    let mut crlf = 0usize;
    let mut prev_cr = false;
    for b in bytes {
        if b == &b'\n' {
            if prev_cr { crlf += 1 } else { lf += 1 }
        }
        prev_cr = *b == b'\r';
    }
    match (lf, crlf) {
        (0, 0) => Eol::None,
        (_, 0) => Eol::Lf,
        (0, _) => Eol::CrLf,
        _ => Eol::Mixed,
    }
}

pub fn from_bytes(bytes: Vec<u8>, truncated: bool) -> BlobContent {
    let size = bytes.len() as u64;
    if looks_binary(&bytes) {
        return BlobContent { text: String::new(), is_binary: true, eol: Eol::None, size, truncated };
    }
    let eol = detect_eol(&bytes);
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    BlobContent { text, is_binary: false, eol, size, truncated }
}

/// `spec` is anything `cat-file` accepts: `<sha>:<path>`, `:0:<path>`
/// (index), `:2:<path>` / `:3:<path>` (ours / theirs in a conflict).
pub async fn get_blob(client: &GitClient, spec: &str) -> Result<BlobContent> {
    let out = client
        .exec(GitReq::new("cat-file", ["cat-file", "blob", spec]))
        .await?;
    let mut bytes = out.stdout;
    let truncated = bytes.len() > BLOB_CAP;
    if truncated {
        bytes.truncate(BLOB_CAP);
    }
    Ok(from_bytes(bytes, truncated))
}

pub async fn get_worktree_file(client: &GitClient, rel_path: &str) -> Result<BlobContent> {
    let full = join_repo_path(&client.repo, rel_path);
    let bytes = client.transport.read_file(&full, BLOB_CAP).await?;
    Ok(from_bytes(bytes, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_eol_and_binary() {
        assert_eq!(detect_eol(b"a\nb\n"), Eol::Lf);
        assert_eq!(detect_eol(b"a\r\nb\r\n"), Eol::CrLf);
        assert_eq!(detect_eol(b"a\r\nb\n"), Eol::Mixed);
        assert_eq!(detect_eol(b"abc"), Eol::None);
        let c = from_bytes(b"x\r\ny".to_vec(), false);
        assert_eq!(c.text, "x\ny");
        assert!(from_bytes(b"\0\0".to_vec(), false).is_binary);
    }
}
