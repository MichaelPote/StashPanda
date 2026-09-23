//! Byte-level helpers for Git output. Everything is `&[u8]`; text is decoded
//! lossily at the last moment so a stray Latin-1 byte never aborts a parse.

use bstr::ByteSlice;

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

pub fn parse_i64(bytes: &[u8]) -> Option<i64> {
    std::str::from_utf8(bytes).ok()?.trim().parse().ok()
}

/// Git's own heuristic: a NUL in the first 8000 bytes means binary.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
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
}
