//! SHA-1 and SHA-256 repositories. git ≥ 2.29 can create repositories whose
//! objects are named by SHA-256 (`git init --object-format=sha256`): ids of 64 hex digits
//! / 32 bytes instead of 40 / 20. gix reads both (its `sha256` feature: objects, packs,
//! refs, the commit-graph file); everything else must not assume one length. The
//! repository's format is read once when it opens ([`crate::Repo::object_hash`]); the
//! helpers here are the one place that knows the lengths of git's output.
//!
//! Mixed formats (a SHA-256 superproject with a SHA-1 submodule or the other way round)
//! are unsupported by git itself; each repository opens on its own as what it is.

use gix::ObjectId;
use gix::hash::Kind;

/// The most hex digits a full object id has (SHA-256: 64).
pub const MAX_HEX_LEN: usize = Kind::longest().len_in_hex();

/// Whether `s` is a full object id in hex of either format (40 or 64 hex digits).
pub fn is_full_hex(s: &str) -> bool {
    Kind::from_hex_len(s.len()).is_some_and(|k| k.len_in_hex() == s.len())
        && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A full object id at the start of `bytes` (the run of hex digits there must be exactly
/// 40 or 64 long) and what follows it.
pub fn split_leading_id(bytes: &[u8]) -> Option<(ObjectId, &[u8])> {
    let n = bytes
        .iter()
        .take(MAX_HEX_LEN + 1)
        .take_while(|b| b.is_ascii_hexdigit())
        .count();
    let id = ObjectId::from_hex(&bytes[..n]).ok()?;
    Some((id, &bytes[n..]))
}

/// The display name of a format (`SHA-1`, `SHA-256`).
pub fn name(kind: Kind) -> &'static str {
    match kind {
        Kind::Sha256 => "SHA-256",
        _ => "SHA-1",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn lengths_of_both_formats() {
        let (sha1, sha256) = ("a".repeat(40), "b".repeat(64));
        assert!(is_full_hex(&sha1) && is_full_hex(&sha256));
        for bad in ["a".repeat(39), "a".repeat(41), "a".repeat(63), "a".repeat(65), "g".repeat(40)] {
            assert!(!is_full_hex(&bad), "{bad}");
        }
        assert_eq!(MAX_HEX_LEN, 64);

        let rec = format!("{sha256}\nM\0a.txt");
        let (id, rest) = split_leading_id(rec.as_bytes()).unwrap();
        assert_eq!((id.kind(), rest), (Kind::Sha256, &b"\nM\0a.txt"[..]));
        let rec = format!("{sha1}\0rest");
        let (id, rest) = split_leading_id(rec.as_bytes()).unwrap();
        assert_eq!((id.kind(), rest), (Kind::Sha1, &b"\0rest"[..]));
        // Neither length, or longer than the longest id.
        assert!(split_leading_id(format!("{}\n", "c".repeat(50)).as_bytes()).is_none());
        assert!(split_leading_id("d".repeat(80).as_bytes()).is_none());
        assert!(split_leading_id(b"").is_none());
        assert_eq!((name(Kind::Sha1), name(Kind::Sha256)), ("SHA-1", "SHA-256"));
    }
}
