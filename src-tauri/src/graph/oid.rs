//! A compact object id: 20 bytes for SHA-1, 32 for SHA-256, 0 for the
//! synthetic Working Tree row.

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid {
    len: u8,
    bytes: [u8; 32],
}

impl Oid {
    pub const WORKTREE: Oid = Oid { len: 0, bytes: [0; 32] };

    pub fn parse_hex(s: &str) -> Option<Oid> {
        let s = s.trim();
        if s.len() != 40 && s.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            bytes[i] = (hi * 16 + lo) as u8;
        }
        Some(Oid { len: (s.len() / 2) as u8, bytes })
    }

    pub fn from_bytes(b: &[u8]) -> Option<Oid> {
        if b.len() != 20 && b.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 32];
        bytes[..b.len()].copy_from_slice(b);
        Some(Oid { len: b.len() as u8, bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    // Byte length of the hash (20 for SHA-1, 32 for SHA-256), never zero, so an
    // `is_empty` would have nothing to report.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_worktree(&self) -> bool {
        self.len == 0
    }

    pub fn to_hex(&self) -> String {
        if self.is_worktree() {
            return "WORKTREE".to_string();
        }
        let mut s = String::with_capacity(self.len() * 2);
        for b in self.as_bytes() {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.to_hex()[..self.to_hex().len().min(8)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips_hex() {
        let h = "dcefad8863f82f21498145fd83275669b27fe10b";
        let o = Oid::parse_hex(h).unwrap();
        assert_eq!(o.to_hex(), h);
        assert_eq!(o.len(), 20);
        assert!(Oid::parse_hex("abc").is_none());
        assert!(Oid::WORKTREE.is_worktree());
    }
}
