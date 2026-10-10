//! The commit ids in the commit header (the commit's own and its parents') and what their
//! context menus copy — the same in both frontends. The wording follows the commit
//! list's "Copy SHA" / "Copy Short SHA".

use crate::ObjectId;

/// Which id of the header: the commit's own, or parent `n` (0-based, in the commit's
/// parent order: 0 = the first parent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderId {
    Commit,
    Parent(usize),
}

/// The id's menu entries: (label, text to copy) — the full id, then the short one.
/// `parents`: how many parents the commit has (a merge's parents are numbered).
pub fn copy_entries(id: &ObjectId, which: HeaderId, parents: usize) -> Vec<(String, String)> {
    let full = id.to_hex().to_string();
    let short = id.to_hex_with_len(7).to_string();
    let what = match which {
        HeaderId::Commit => String::new(),
        HeaderId::Parent(_) if parents <= 1 => "Parent ".to_string(),
        HeaderId::Parent(n) => format!("Parent {} ", n + 1),
    };
    vec![
        (format!("Copy {what}SHA"), full),
        (format!("Copy Short {what}SHA"), short),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn labels_name_the_id() {
        let id = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap();
        let labels = |w, n| copy_entries(&id, w, n).into_iter().map(|e| e.0).collect::<Vec<_>>();
        assert_eq!(labels(HeaderId::Commit, 1), ["Copy SHA", "Copy Short SHA"]);
        assert_eq!(labels(HeaderId::Parent(0), 1), ["Copy Parent SHA", "Copy Short Parent SHA"]);
        assert_eq!(labels(HeaderId::Parent(1), 2), ["Copy Parent 2 SHA", "Copy Short Parent 2 SHA"]);
        let texts: Vec<String> = copy_entries(&id, HeaderId::Commit, 0).into_iter().map(|e| e.1).collect();
        assert_eq!(texts, ["0123456789abcdef0123456789abcdef01234567", "0123456"]);
    }
}
