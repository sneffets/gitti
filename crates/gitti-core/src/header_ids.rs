//! The commit ids in the commit header (the commit's own and its parents') and what their
//! context menus copy — the same in both frontends; gitty's author / committer lines too
//! (gitti selects their text instead). The wording follows the commit
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

/// The menu entries of a person line of the header (`role`: "Author", "Committer"):
/// (label, text to copy) — "Name <email>", the name, the email.
pub fn person_entries(role: &str, name: &str, email: &str) -> Vec<(String, String)> {
    vec![
        (format!("Copy {role}"), format!("{name} <{email}>")),
        (format!("Copy {role} Name"), name.to_string()),
        (format!("Copy {role} Email"), email.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn person_entries_copy_whole_name_and_email() {
        assert_eq!(
            person_entries("Author", "Ada Lovelace", "ada@example.org"),
            [
                ("Copy Author".to_string(), "Ada Lovelace <ada@example.org>".to_string()),
                ("Copy Author Name".to_string(), "Ada Lovelace".to_string()),
                ("Copy Author Email".to_string(), "ada@example.org".to_string()),
            ]
        );
    }

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
