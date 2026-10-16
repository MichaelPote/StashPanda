//! Byte-level helpers for Git output. Everything is `&[u8]`; text is decoded
//! lossily at the last moment so a stray Latin-1 byte never aborts a parse.

use bstr::ByteSlice;

use crate::error::{GitError, Result};

/// Split on NUL, dropping a trailing empty record.
pub fn split_nul(bytes: &[u8]) -> Vec<&[u8]> {
    let mut v: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

/// Split one record on the unit separator (0x1F).
pub fn split_us(record: &[u8]) -> Vec<&[u8]> {
    record.split(|b| *b == 0x1f).collect()
}

pub fn lossy(bytes: &[u8]) -> String {
    bytes.to_str_lossy().into_owned()
}

/// Refuses a path that came from a file name that is not valid UTF-8. Such a name reaches the app
/// with U+FFFD in place of the bytes it could not read, so it no longer names the file: Git would
/// act on a file that does not exist, or on another one.
pub fn utf8_name(path: &str) -> Result<()> {
    if path.contains('\u{fffd}') {
        return Err(GitError::InvalidPath {
            path: path.to_string(),
            reason: "the file name is not valid UTF-8, so it cannot be passed back to Git; rename the file to work on it here".into(),
        });
    }
    Ok(())
}

pub fn parse_i64(bytes: &[u8]) -> Option<i64> {
    std::str::from_utf8(bytes).ok()?.trim().parse().ok()
}

/// Git's own heuristic: a NUL in the first 8000 bytes means binary.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

/// A path as Git prints it outside `-z` output: as it is, or, when it holds a double quote, a
/// backslash or a control character, in double quotes with C escapes (`"a\tb"`, `"\303\274"`
/// for a byte Git chose to escape). `core.quotepath=false` keeps other non-ASCII text as it is.
pub fn unquote_path(bytes: &[u8]) -> String {
    let [b'"', inner @ .., b'"'] = bytes else { return lossy(bytes) };
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let b = inner[i];
        i += 1;
        if b != b'\\' || i >= inner.len() {
            out.push(b);
            continue;
        }
        let e = inner[i];
        i += 1;
        let plain = match e {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            b'0'..=b'7' => {
                // Three octal digits.
                let mut value = u32::from(e - b'0');
                for _ in 0..2 {
                    match inner.get(i) {
                        Some(d @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(d - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                value as u8
            }
            other => other,
        };
        out.push(plain);
    }
    lossy(&out)
}

/// `name` as Git prints it outside `-z` output with `core.quotepath=false`: the reverse of
/// `unquote_path`.
pub fn quote_path(name: &str) -> String {
    if !name.bytes().any(|b| b < 0x20 || b == 0x7f || b == b'"' || b == b'\\') {
        return name.to_string();
    }
    let mut out = String::from("\"");
    for c in name.chars() {
        match c {
            '\u{07}' => out.push_str("\\a"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0b}' => out.push_str("\\v"),
            '\u{0c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\{:03o}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_records_and_fields() {
        let recs = split_nul(b"a\x1fb\0c\x1fd\0");
        assert_eq!(recs.len(), 2);
        assert_eq!(split_us(recs[1]), vec![b"c" as &[u8], b"d"]);
        assert!(looks_binary(b"ab\0cd"));
        assert!(!looks_binary(b"plain text"));
        assert_eq!(parse_i64(b" 42\n"), Some(42));
    }

    #[test]
    fn unquotes_c_style_paths() {
        assert_eq!(unquote_path(b"plain name.txt"), "plain name.txt");
        assert_eq!(unquote_path("fünf.txt".as_bytes()), "fünf.txt");
        assert_eq!(unquote_path(b"\"tab\\there.txt\""), "tab\there.txt");
        assert_eq!(unquote_path(b"\"say \\\"hi\\\" \\\\ back\""), "say \"hi\" \\ back");
        assert_eq!(unquote_path(b"\"f\\303\\274nf\""), "fünf");
        assert_eq!(unquote_path(b"\""), "\"", "a lone quote is a name, not a quoted one");
        for name in ["plain name.txt", "fünf.txt", "tab\there.txt", "say \"hi\" \\ back", "bell\u{7}del\u{7f}esc\u{1b}"] {
            assert_eq!(unquote_path(quote_path(name).as_bytes()), name);
        }
        assert_eq!(quote_path("tab\there.txt"), "\"tab\\there.txt\"");
        assert_eq!(quote_path("fünf.txt"), "fünf.txt");
    }

    #[test]
    fn refuses_a_name_that_was_not_utf8() {
        // `caf\xe9.txt` as Git prints it in status, decoded the way every parser here decodes it.
        let name = lossy(b"caf\xe9.txt");
        assert!(matches!(utf8_name(&name), Err(GitError::InvalidPath { .. })));
        assert!(utf8_name("café.txt").is_ok());
    }
}
