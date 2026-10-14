//! One conflicted file as the merge editor opens it: the three versions Git keeps in the index
//! (the base both sides started from, ours and theirs), and the working tree file with Git's
//! conflict markers taken out and described as blocks.
//!
//! The editor starts from the working tree file rather than merging the three versions again,
//! so it keeps everything Git and the user have already done to the file: changes that merged
//! cleanly, a merge driver's output, `rerere`'s earlier resolutions and hand edits. Each block of
//! conflict markers is replaced by its base text (or by nothing when the markers carry no base),
//! which gives the text the editor's middle pane starts with.
//!
//! Every text this returns uses `\n` line endings and has no byte order mark, so the frontend
//! deals with one form of text. The working tree file's own line endings, byte order mark and
//! final newline are reported alongside, for the frontend to put back when it saves.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;
use tokio_util::sync::CancellationToken;

use crate::error::{GitError, Result};
use crate::git::catfile::CatFile;
use crate::git::ops::parse_unmerged;
use crate::git::parse::split_nul;
use crate::git::queries::blob::{detect_eol, Eol};
use crate::git::queries::diff::join_repo_path;
use crate::git::{GitClient, GitReq};

/// The largest version of a file the merge editor opens, in bytes.
pub const CONFLICT_FILE_CAP: usize = 5 * 1024 * 1024;

/// How many characters make a conflict marker when the file's `conflict-marker-size` attribute
/// does not say otherwise.
pub const DEFAULT_MARKER_SIZE: usize = 7;

const BYTE_ORDER_MARK: &str = "\u{feff}";

/// Why the merge editor cannot open a conflicted file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ConflictRefusal {
    /// The index has no conflict stages for the path: it was never conflicted, or has been
    /// marked resolved.
    NotConflicted,
    /// One side deleted the file and the other changed it, so there is no second version to
    /// merge with. The take-a-side actions handle these.
    DeletedOnOneSide,
    /// A version contains a NUL byte.
    Binary,
    /// A version is not valid UTF-8 text.
    NotUtf8,
    /// A version is larger than 5 MB (`CONFLICT_FILE_CAP`).
    TooLarge,
}

/// One block of conflict markers in the working tree file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConflictBlock {
    /// Line range in `result`, counting from 0, end exclusive. start_line == end_line when the
    /// markers carried no base text, so the block takes no lines in `result`.
    pub start_line: u32,
    pub end_line: u32,
    /// Each side's lines, every line followed by "\n"; "" when a side has no lines in the block.
    pub ours: String,
    /// None when the markers carry no base section (merge style); Some("") for an empty base.
    pub base: Option<String>,
    pub theirs: String,
    /// The text Git wrote after `<<<<<<< ` and `>>>>>>> `, such as "HEAD" and "feature/x".
    pub ours_label: String,
    pub theirs_label: String,
    /// The text after `||||||| `, when there is a base section.
    pub base_label: Option<String>,
}

/// A conflicted file, ready for the merge editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConflictFile {
    pub path: String,
    /// Why the merge editor cannot open this file; when Some, the text fields are empty.
    pub refused: Option<ConflictRefusal>,
    /// Stage 1 (base), 2 (ours) and 3 (theirs) of the file, read through the long-lived
    /// cat-file (`:1:<path>` etc.); None when that stage is missing.
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    /// The working tree file with each conflict marker block replaced by its base text (or by
    /// nothing when the markers carry no base).
    pub result: String,
    pub blocks: Vec<ConflictBlock>,
    /// The working tree file's line endings, whether it starts with a UTF-8 byte order mark, and
    /// whether it ends with a newline. When Git's markers open the file, the byte order mark sits
    /// after the first marker line, at the start of each side's first line; `bom` is true then
    /// as well, and the mark is taken out of the block texts like everywhere else.
    pub eol: Eol,
    pub bom: bool,
    pub final_newline: bool,
    /// A hash of the working tree file's bytes, opaque to the frontend, passed back to
    /// `write_worktree_file` to detect a change on disk.
    pub hash: String,
    /// The length of the file's conflict markers, from its `conflict-marker-size` attribute (7
    /// unless set), for writing an unresolved block back the way Git would.
    pub marker_size: u32,
    /// Save the file with CRLF line endings: more of its lines end in CRLF than in LF alone. For a
    /// file with mixed endings this keeps the one it mostly uses.
    pub crlf: bool,
}

impl ConflictFile {
    fn refused(path: &str, reason: ConflictRefusal) -> ConflictFile {
        ConflictFile {
            path: path.to_string(),
            refused: Some(reason),
            base: None,
            ours: None,
            theirs: None,
            result: String::new(),
            blocks: Vec::new(),
            eol: Eol::None,
            bom: false,
            final_newline: false,
            hash: String::new(),
            marker_size: DEFAULT_MARKER_SIZE as u32,
            crlf: false,
        }
    }
}

/// The hash `ConflictFile::hash` carries: MD5 of the bytes, as 32 lowercase hexadecimal digits.
/// It only has to notice that a file changed, not resist someone forging a match.
pub fn content_hash(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    Md5::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Read conflicted file `path` (relative to the repository) for the merge editor: its three
/// stages, through the long-lived `cat-file` when `helper` is given, and the working tree file
/// with its conflict markers parsed. A file the editor cannot open comes back with `refused` set
/// and no text, rather than as an error, so the dialog can say why.
pub async fn get_conflict_file(client: &GitClient, helper: Option<(&Arc<CatFile>, &CancellationToken)>, path: &str) -> Result<ConflictFile> {
    crate::git::ops::valid_worktree_path(path)?;
    let unmerged = GitReq::new("ls-files", ["--literal-pathspecs", "ls-files", "-u", "-z", "--", path]);
    let attribute = GitReq::new("check-attr", ["check-attr", "-z", "conflict-marker-size", "--", path]);
    let (unmerged, attribute) = tokio::try_join!(client.exec(unmerged), client.exec(attribute))?;
    let stages = parse_unmerged(&unmerged.stdout).remove(path).unwrap_or_default();
    let oids = stage_oids(&unmerged.stdout, path);
    if stages.is_empty() {
        return Ok(ConflictFile::refused(path, ConflictRefusal::NotConflicted));
    }
    if !stages.contains(&2) || !stages.contains(&3) {
        return Ok(ConflictFile::refused(path, ConflictRefusal::DeletedOnOneSide));
    }
    let marker_size = marker_size_from_check_attr(&attribute.stdout);

    // The stages one after another, since the helper takes one request at a time anyway, and
    // the working tree file meanwhile.
    let stages_read = async {
        let base = read_stage(client, helper, 1, &oids).await?;
        let ours = read_stage(client, helper, 2, &oids).await?;
        let theirs = read_stage(client, helper, 3, &oids).await?;
        Ok::<_, GitError>((base, ours, theirs))
    };
    let full_path = join_repo_path(&client.repo, path);
    let worktree_read = client.transport.read_file(&full_path, CONFLICT_FILE_CAP);
    // `join!`, not `try_join!`: a failed working tree read must not drop a `cat-file` request
    // half way, which would leave its answer in the helper's output for the next request to read.
    let ((base, ours, theirs), worktree) = match tokio::join!(stages_read, worktree_read) {
        (Ok(stages), Ok(worktree)) => (stages, worktree),
        (Err(GitError::TooLarge { .. }), _) | (_, Err(GitError::TooLarge { .. })) => {
            return Ok(ConflictFile::refused(path, ConflictRefusal::TooLarge));
        }
        (Err(e), _) | (_, Err(e)) => return Err(e),
    };

    let versions = [base.as_deref(), ours.as_deref(), theirs.as_deref(), Some(worktree.as_slice())];
    if versions.iter().flatten().any(|bytes| bytes.contains(&0)) {
        return Ok(ConflictFile::refused(path, ConflictRefusal::Binary));
    }
    let decode = |bytes: Option<Vec<u8>>| bytes.map(|b| decode_text(&b)).transpose();
    let (Ok(base), Ok(ours), Ok(theirs), Ok(file)) = (decode(base), decode(ours), decode(theirs), decode_text(&worktree)) else {
        return Ok(ConflictFile::refused(path, ConflictRefusal::NotUtf8));
    };

    let parsed = parse_markers(&file.text, marker_size);
    Ok(ConflictFile {
        path: path.to_string(),
        refused: None,
        base: base.map(|t| t.text),
        ours: ours.map(|t| t.text),
        theirs: theirs.map(|t| t.text),
        result: parsed.result,
        blocks: parsed.blocks,
        eol: detect_eol(&worktree),
        bom: file.bom || parsed.bom_in_first_block,
        final_newline: worktree.ends_with(b"\n"),
        hash: content_hash(&worktree),
        marker_size: marker_size as u32,
        crlf: mostly_crlf(&worktree),
    })
}

/// Each stage's blob of `path`, from `git ls-files -u -z` (`<mode> <oid> <stage>\t<path>` NUL).
///
/// The stages are read by these object IDs rather than by `:<stage>:<path>`: the long-lived
/// `cat-file` reads the index the first time it is asked for a stage and keeps that copy for as
/// long as it runs, so after a merge is aborted and done again it would hand back the old
/// conflict's versions, or none. `ls-files` runs afresh for every request, and an object never
/// changes.
fn stage_oids(stdout: &[u8], path: &str) -> std::collections::HashMap<u8, String> {
    let mut out = std::collections::HashMap::new();
    for record in stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(record);
        let Some((meta, file)) = text.split_once('\t') else { continue };
        if file != path {
            continue;
        }
        let mut fields = meta.split(' ');
        let (Some(_mode), Some(oid), Some(stage)) = (fields.next(), fields.next(), fields.next()) else { continue };
        if let Ok(stage) = stage.parse::<u8>() {
            out.insert(stage, oid.to_string());
        }
    }
    out
}

/// The bytes of stage `stage`, by its object ID in `oids`, or None when the conflict has no such
/// stage. Fails with `tooLarge` for a version over `CONFLICT_FILE_CAP`.
async fn read_stage(
    client: &GitClient,
    helper: Option<(&Arc<CatFile>, &CancellationToken)>,
    stage: u8,
    oids: &std::collections::HashMap<u8, String>,
) -> Result<Option<Vec<u8>>> {
    let Some(spec) = oids.get(&stage).cloned() else {
        return Ok(None);
    };
    if let Some((catfile, cancel)) = helper.filter(|_| client.caps.batch_command) {
        if let Some(bytes) = catfile.read(client, cancel, &spec, CONFLICT_FILE_CAP).await? {
            return Ok(Some(bytes));
        }
        // Not found through the long-lived helper, which can lose sight of the repository (its
        // `.git` folder replaced while it runs): a fresh `cat-file` sees it as it is now.
    }
    let out = client.exec(GitReq::new("cat-file", ["cat-file".to_string(), "blob".into(), spec])).await?;
    if out.stdout.len() > CONFLICT_FILE_CAP {
        return Err(GitError::TooLarge { bytes: out.stdout.len() as u64, limit: CONFLICT_FILE_CAP as u64 });
    }
    Ok(Some(out.stdout))
}

/// True when more lines of `bytes` end in CRLF than in a bare LF.
pub(crate) fn mostly_crlf(bytes: &[u8]) -> bool {
    let mut crlf = 0usize;
    let mut lf = 0usize;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            if i > 0 && bytes[i - 1] == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
    }
    crlf > lf
}

/// The marker length `git check-attr -z conflict-marker-size -- <path>` gives
/// (`<path> NUL conflict-marker-size NUL <value> NUL`). Git uses the default for `unspecified`,
/// `unset`, `set` and anything that is not a positive number, and so does this.
fn marker_size_from_check_attr(stdout: &[u8]) -> usize {
    split_nul(stdout)
        .get(2)
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_MARKER_SIZE)
}

/// A version of the file as text, in the form every text field takes.
#[derive(Debug, PartialEq, Eq)]
struct Text {
    /// With `\r\n` turned into `\n` and no byte order mark.
    text: String,
    /// The bytes started with a UTF-8 byte order mark.
    bom: bool,
}

/// Fails when `bytes` are not UTF-8 once a leading byte order mark is set aside.
fn decode_text(bytes: &[u8]) -> std::result::Result<Text, std::str::Utf8Error> {
    let text = std::str::from_utf8(bytes)?;
    let (text, bom) = match text.strip_prefix(BYTE_ORDER_MARK) {
        Some(rest) => (rest, true),
        None => (text, false),
    };
    Ok(Text { text: text.replace("\r\n", "\n"), bom })
}

/// The working tree file's text with its conflict markers taken out.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedMarkers {
    pub result: String,
    pub blocks: Vec<ConflictBlock>,
    /// The file opened with a block whose sides started with a byte order mark, which has been
    /// taken out of the block texts.
    pub bom_in_first_block: bool,
}

/// The four kinds of conflict marker line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    /// `<<<<<<<`, before our side.
    Ours,
    /// `|||||||`, before the base, in the `diff3` and `zdiff3` styles.
    Base,
    /// `=======`, before their side.
    Theirs,
    /// `>>>>>>>`, closing the block.
    End,
}

/// Which marker `line` is, and its label, when it is one: exactly `size` marker characters,
/// followed by the end of the line, or by a space and the label.
fn marker(line: &str, size: usize) -> Option<(Marker, &str)> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let bytes = line.as_bytes();
    let kind = match bytes.first()? {
        b'<' => Marker::Ours,
        b'|' => Marker::Base,
        b'=' => Marker::Theirs,
        b'>' => Marker::End,
        _ => return None,
    };
    if bytes.len() < size || bytes[..size].iter().any(|b| *b != bytes[0]) {
        return None;
    }
    // The marker characters are ASCII, so `size` is a character boundary.
    let rest = &line[size..];
    if rest.is_empty() {
        return Some((kind, ""));
    }
    rest.strip_prefix(' ').map(|label| (kind, label))
}

/// Take the conflict markers out of `text` (in the form `decode_text` gives), with markers
/// `marker_size` characters long. Reads the three styles Git writes: `merge` (`<<<<<<<`,
/// `=======`, `>>>>>>>`), `diff3` (with a `|||||||` base section) and `zdiff3` (the same, with
/// the lines both sides share moved out of the block).
///
/// A block is only taken as one when it is complete, in order. A marker line anywhere else is
/// ordinary text: a `<<<<<<<` that a new `<<<<<<<` follows before the block ends, a block that
/// ends before its `=======`, and markers with no `<<<<<<<` before them. Once in a block, a line
/// is a marker only where one can come next, so a `=======` among their lines, or a `|||||||`
/// among the base lines, is text in that side.
pub fn parse_markers(text: &str, marker_size: usize) -> ParsedMarkers {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut parsed = ParsedMarkers { result: String::with_capacity(text.len()), blocks: Vec::new(), bom_in_first_block: false };
    let mut result_lines = 0u32;
    let mut i = 0;
    while i < lines.len() {
        if !matches!(marker(lines[i], marker_size), Some((Marker::Ours, _))) {
            parsed.result.push_str(lines[i]);
            result_lines += lines[i].ends_with('\n') as u32;
            i += 1;
            continue;
        }
        match read_block(&lines, i, marker_size) {
            Ok((mut block, next)) => {
                if i == 0 {
                    parsed.bom_in_first_block = strip_block_bom(&mut block);
                }
                block.start_line = result_lines;
                if let Some(base) = &block.base {
                    parsed.result.push_str(base);
                    result_lines += base.matches('\n').count() as u32;
                }
                block.end_line = result_lines;
                parsed.blocks.push(block);
                i = next;
            }
            Err(next) => {
                // Nothing from here to `next` can open a block, so it is all ordinary text.
                for line in &lines[i..next] {
                    parsed.result.push_str(line);
                    result_lines += line.ends_with('\n') as u32;
                }
                i = next;
            }
        }
    }
    parsed
}

/// Read the block whose `<<<<<<<` line is `lines[start]`. Returns it (with its line range still
/// to fill in) and the index of the line after its `>>>>>>>`; or, when the block is not complete,
/// the index to carry on from, which is the next `<<<<<<<` line or the line after the point where
/// the block went wrong.
fn read_block(lines: &[&str], start: usize, size: usize) -> std::result::Result<(ConflictBlock, usize), usize> {
    #[derive(PartialEq)]
    enum Section {
        Ours,
        Base,
        Theirs,
    }
    let (_, ours_label) = marker(lines[start], size).expect("the caller found a <<<<<<< line");
    let mut block = ConflictBlock {
        start_line: 0,
        end_line: 0,
        ours: String::new(),
        base: None,
        theirs: String::new(),
        ours_label: ours_label.to_string(),
        theirs_label: String::new(),
        base_label: None,
    };
    let mut section = Section::Ours;
    for (j, line) in lines.iter().enumerate().skip(start + 1) {
        match (&section, marker(line, size)) {
            (_, Some((Marker::Ours, _))) => return Err(j),
            (Section::Ours, Some((Marker::Base, label))) => {
                section = Section::Base;
                block.base = Some(String::new());
                block.base_label = Some(label.to_string());
            }
            (Section::Ours | Section::Base, Some((Marker::Theirs, _))) => section = Section::Theirs,
            (Section::Ours | Section::Base, Some((Marker::End, _))) => return Err(j + 1),
            (Section::Theirs, Some((Marker::End, label))) => {
                block.theirs_label = label.to_string();
                return Ok((block, j + 1));
            }
            _ => {
                let side = match section {
                    Section::Ours => &mut block.ours,
                    Section::Base => block.base.as_mut().expect("the base section has begun"),
                    Section::Theirs => &mut block.theirs,
                };
                // Only the file's last line can lack a newline, and a block ends after it.
                side.push_str(line);
            }
        }
    }
    Err(lines.len())
}

/// Take a byte order mark off the start of each side of `block`, for a block that opens the
/// file. Returns whether there was one.
fn strip_block_bom(block: &mut ConflictBlock) -> bool {
    let mut found = false;
    let sides = [Some(&mut block.ours), block.base.as_mut(), Some(&mut block.theirs)];
    for side in sides.into_iter().flatten() {
        if let Some(rest) = side.strip_prefix(BYTE_ORDER_MARK) {
            *side = rest.to_string();
            found = true;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(start: u32, end: u32, ours: &str, base: Option<&str>, theirs: &str, labels: (&str, Option<&str>, &str)) -> ConflictBlock {
        ConflictBlock {
            start_line: start,
            end_line: end,
            ours: ours.into(),
            base: base.map(String::from),
            theirs: theirs.into(),
            ours_label: labels.0.into(),
            base_label: labels.1.map(String::from),
            theirs_label: labels.2.into(),
        }
    }

    /// `parsed.result` with every block's lines replaced by one side's text, chosen by `side`.
    /// Taking ours everywhere must give our version back, and theirs theirs.
    fn resolve(parsed: &ParsedMarkers, side: impl Fn(&ConflictBlock) -> &str) -> String {
        let lines: Vec<&str> = parsed.result.split_inclusive('\n').collect();
        let mut out = String::new();
        let mut at = 0usize;
        for b in &parsed.blocks {
            out.extend(lines[at..b.start_line as usize].iter().copied());
            out.push_str(side(b));
            at = b.end_line as usize;
        }
        out.extend(lines[at.min(lines.len())..].iter().copied());
        out
    }

    #[test]
    fn reads_the_merge_style() {
        // Captured from `git merge` with merge.conflictStyle=merge (Git 2.43).
        let text = "1\nX\n<<<<<<< HEAD\nY\n=======\nQ\n>>>>>>> feature\nZ\n3\n";
        let parsed = parse_markers(text, 7);
        assert_eq!(parsed.result, "1\nX\nZ\n3\n");
        assert_eq!(parsed.blocks, [block(2, 2, "Y\n", None, "Q\n", ("HEAD", None, "feature"))]);
        assert_eq!(resolve(&parsed, |b| &b.ours), "1\nX\nY\nZ\n3\n");
        assert_eq!(resolve(&parsed, |b| &b.theirs), "1\nX\nQ\nZ\n3\n");
    }

    #[test]
    fn reads_the_diff3_and_zdiff3_styles() {
        // Captured from `git merge` with merge.conflictStyle=diff3: the lines both sides share
        // stay in the block.
        let diff3 = "1\n<<<<<<< HEAD\nX\nY\nZ\n||||||| 8ca0987\n2\n=======\nX\nQ\nZ\n>>>>>>> feature\n3\n";
        let parsed = parse_markers(diff3, 7);
        assert_eq!(parsed.result, "1\n2\n3\n");
        assert_eq!(parsed.blocks, [block(1, 2, "X\nY\nZ\n", Some("2\n"), "X\nQ\nZ\n", ("HEAD", Some("8ca0987"), "feature"))]);
        assert_eq!(resolve(&parsed, |b| &b.ours), "1\nX\nY\nZ\n3\n");
        // zdiff3 moves them out of the block.
        let zdiff3 = "1\nX\n<<<<<<< HEAD\nY\n||||||| 8ca0987\n2\n=======\nQ\n>>>>>>> feature\nZ\n3\n";
        let parsed = parse_markers(zdiff3, 7);
        assert_eq!(parsed.result, "1\nX\n2\nZ\n3\n");
        assert_eq!(parsed.blocks, [block(2, 3, "Y\n", Some("2\n"), "Q\n", ("HEAD", Some("8ca0987"), "feature"))]);
        assert_eq!(resolve(&parsed, |b| &b.theirs), "1\nX\nQ\nZ\n3\n");
        // Both sides added the file: the base section is there and empty.
        let both = "<<<<<<< HEAD\nmain added\n||||||| 8ca0987\n=======\nfeature added\n>>>>>>> feature\n";
        let parsed = parse_markers(both, 7);
        assert_eq!(parsed.result, "");
        assert_eq!(parsed.blocks, [block(0, 0, "main added\n", Some(""), "feature added\n", ("HEAD", Some("8ca0987"), "feature"))]);
    }

    #[test]
    fn line_ranges_follow_earlier_blocks() {
        let text = "a\n<<<<<<< ours\n1\n||||||| base\nb1\nb2\n=======\n2\n>>>>>>> theirs\nmid\n<<<<<<< ours\n3\n=======\n>>>>>>> theirs\nend";
        let parsed = parse_markers(text, 7);
        assert_eq!(parsed.result, "a\nb1\nb2\nmid\nend");
        assert_eq!(parsed.blocks[0], block(1, 3, "1\n", Some("b1\nb2\n"), "2\n", ("ours", Some("base"), "theirs")));
        // A side with no lines in the block, and a block with no base, which takes no lines.
        assert_eq!(parsed.blocks[1], block(4, 4, "3\n", None, "", ("ours", None, "theirs")));
        assert_eq!(parsed.result.split('\n').nth(4), Some("end"));
        assert_eq!(resolve(&parsed, |b| &b.ours), "a\n1\nmid\n3\nend");
    }

    #[test]
    fn marker_lines_need_the_exact_length_and_a_space_before_the_label() {
        assert_eq!(marker("<<<<<<< HEAD\n", 7), Some((Marker::Ours, "HEAD")));
        assert_eq!(marker("=======\n", 7), Some((Marker::Theirs, "")));
        assert_eq!(marker(">>>>>>> feature/x y", 7), Some((Marker::End, "feature/x y")));
        assert_eq!(marker("<<<<<<< \n", 7), Some((Marker::Ours, "")));
        assert_eq!(marker("<<<<<<<< HEAD\n", 7), None);
        assert_eq!(marker("<<<<<<<HEAD\n", 7), None);
        assert_eq!(marker("<<<<<< HEAD\n", 7), None);
        assert_eq!(marker("<<<<<<<<<< HEAD\n", 10), Some((Marker::Ours, "HEAD")));
        assert_eq!(marker("<<<<<<< HEAD\n", 10), None);
        assert_eq!(marker("", 7), None);
    }

    #[test]
    fn a_longer_marker_size_leaves_seven_character_markers_as_text() {
        let text = "<<<<<<< not a marker\n<<<<<<<<<< HEAD\n=======\nours\n========== \ntheirs\n>>>>>>>>>> feature\n=======\n";
        let parsed = parse_markers(text, 10);
        assert_eq!(parsed.result, "<<<<<<< not a marker\n=======\n");
        assert_eq!(parsed.blocks, [block(1, 1, "=======\nours\n", None, "theirs\n", ("HEAD", None, "feature"))]);
    }

    #[test]
    fn markers_outside_a_complete_block_are_ordinary_text() {
        // A stray `<<<<<<<` before a real block, and a stray `=======` and `>>>>>>>` after it.
        let text = "<<<<<<< stray\ntext\n<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\n=======\n>>>>>>> y\n";
        let parsed = parse_markers(text, 7);
        assert_eq!(parsed.result, "<<<<<<< stray\ntext\n=======\n>>>>>>> y\n");
        assert_eq!(parsed.blocks, [block(2, 2, "a\n", None, "b\n", ("HEAD", None, "x"))]);
        // A block that ends before its `=======`, and one the file ends inside.
        let text = "<<<<<<< HEAD\na\n>>>>>>> x\nb\n<<<<<<< HEAD\nc\n=======\nd\n";
        let parsed = parse_markers(text, 7);
        assert_eq!(parsed.result, text);
        assert!(parsed.blocks.is_empty());
        // Inside a block, a `=======` among their lines and a `|||||||` among the base lines are
        // text in that side.
        let text = "<<<<<<< HEAD\na\n||||||| base\nb\n||||||| more\n=======\nTitle\n=======\n>>>>>>> x\n";
        let parsed = parse_markers(text, 7);
        assert_eq!(parsed.blocks, [block(0, 2, "a\n", Some("b\n||||||| more\n"), "Title\n=======\n", ("HEAD", Some("base"), "x"))]);
        // No markers at all: the text as it is.
        let plain = "no markers\nhere";
        assert_eq!(parse_markers(plain, 7), ParsedMarkers { result: plain.into(), blocks: Vec::new(), bom_in_first_block: false });
        assert_eq!(parse_markers("", 7).result, "");
    }

    #[test]
    fn a_byte_order_mark_inside_a_block_that_opens_the_file_is_taken_out() {
        let text = "<<<<<<< HEAD\n\u{feff}first-main\n=======\n\u{feff}first-feature\n>>>>>>> feature\nsecond\n";
        let parsed = parse_markers(text, 7);
        assert!(parsed.bom_in_first_block);
        assert_eq!(parsed.blocks, [block(0, 0, "first-main\n", None, "first-feature\n", ("HEAD", None, "feature"))]);
        assert!(!parse_markers("a\n<<<<<<< HEAD\n\u{feff}x\n=======\ny\n>>>>>>> f\n", 7).bom_in_first_block);
    }

    #[test]
    fn decodes_text_without_crlf_or_a_byte_order_mark() {
        assert_eq!(decode_text(b"\xef\xbb\xbfa\r\nb\r\n").unwrap(), Text { text: "a\nb\n".into(), bom: true });
        assert_eq!(decode_text(b"a\rb\n").unwrap(), Text { text: "a\rb\n".into(), bom: false });
        assert!(decode_text(b"caf\xe9\n").is_err());
    }

    #[test]
    fn reads_the_marker_size_attribute() {
        assert_eq!(marker_size_from_check_attr(b"a.txt\0conflict-marker-size\x0010\0"), 10);
        assert_eq!(marker_size_from_check_attr(b"a.txt\0conflict-marker-size\0unspecified\0"), 7);
        assert_eq!(marker_size_from_check_attr(b"a.txt\0conflict-marker-size\0unset\0"), 7);
        assert_eq!(marker_size_from_check_attr(b"a.txt\0conflict-marker-size\0set\0"), 7);
        assert_eq!(marker_size_from_check_attr(b"a.txt\0conflict-marker-size\x000\0"), 7);
        assert_eq!(marker_size_from_check_attr(b""), 7);
    }

    #[test]
    fn mostly_crlf_counts_line_endings() {
        assert!(mostly_crlf(b"a\r\nb\r\nc\n"));
        assert!(!mostly_crlf(b"a\r\nb\nc\n"));
        assert!(!mostly_crlf(b"no newline"));
        assert!(!mostly_crlf(b"\n"));
    }

    #[test]
    fn hashes_bytes_as_hexadecimal_md5() {
        assert_eq!(content_hash(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_ne!(content_hash(b"a\n"), content_hash(b"a\r\n"));
    }
}
