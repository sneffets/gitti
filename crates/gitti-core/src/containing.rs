//! "Branches and Tags Containing This Commit…": the local branches, remote branches and
//! tags whose history has a commit — "which release has this fix", "is it on main yet".
//!
//! Asked on demand only (a menu entry, never on navigation). From a fully loaded history
//! it is answered in-process: the rows are sorted children before parents, so the
//! commit's descendants are found by one sweep up from its row (a row is a descendant
//! when one of its parents is), and a ref contains the commit when its tip is one of
//! them. No git runs. While only the quick partial history is loaded (a ref's tip or a
//! path down to the commit may not be loaded yet), one
//! `git for-each-ref --contains` answers instead.
//!
//! Tags are listed nearest first: the tag closest above the commit (the first release
//! that has it) leads. Each group is capped at [`SHOWN`] names ("… and N more").

use anyhow::Result;
use gix::ObjectId;

use crate::commits::EXTERNAL;
use crate::history::History;
use crate::{RefKind, Repo};

/// Names listed per group; the rest is counted.
pub const SHOWN: usize = 30;

/// The menu entry (both apps' commit menus).
pub const MENU_LABEL: &str = "Branches and Tags Containing This Commit…";
/// The same in the Commit menu / palettes (acts on the selected commit).
pub const SELECTED_LABEL: &str = "Branches and Tags Containing Selected Commit…";

/// The refs whose history has a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Containing {
    pub id: ObjectId,
    /// HEAD's history has it (named when HEAD is detached).
    pub head: bool,
    /// Short names, sorted.
    pub local: Vec<String>,
    /// Short names (`origin/main`), sorted.
    pub remote: Vec<String>,
    /// Short names, nearest first (see the module docs).
    pub tags: Vec<String>,
    /// Read by `git for-each-ref --contains` (the history was partial).
    pub from_git: bool,
}

impl History {
    /// The refs whose history has commit `id`, from the loaded rows (no git). `None`
    /// while the history is partial: unloaded commits may lead to it.
    pub fn refs_containing(&self, id: &gix::oid) -> Option<Containing> {
        if self.partial {
            return None;
        }
        let mut out = Containing::none(id.to_owned());
        let Some(row) = self.row_of(id) else {
            // Not reachable from any ref or HEAD.
            return Some(out);
        };
        let marked = descendants(self, row);
        let is_marked = |r: usize| marked.get(r / 64).is_some_and(|w| w & (1 << (r % 64)) != 0);
        out.head = self.head().and_then(|h| self.row_of(&h)).is_some_and(is_marked);
        let mut tags = Vec::new();
        for (target, refs) in &self.refs_by_commit {
            let Some(r) = self.row_of(target).filter(|&r| is_marked(r)) else {
                continue;
            };
            for info in refs {
                match info.kind {
                    RefKind::LocalBranch => out.local.push(info.short_name.clone()),
                    RefKind::RemoteBranch => out.remote.push(info.short_name.clone()),
                    // The tag nearest the commit: the deepest row above it.
                    RefKind::Tag => tags.push((std::cmp::Reverse(r), info.short_name.clone())),
                    _ => {}
                }
            }
        }
        out.local.sort();
        out.remote.sort();
        tags.sort();
        out.tags = tags.into_iter().map(|(_, name)| name).collect();
        Some(out)
    }
}

/// One bit per row above and at `row`: the row is `row` or one of its descendants.
fn descendants(history: &History, row: usize) -> Vec<u64> {
    let mut marks = vec![0u64; row / 64 + 1];
    marks[row / 64] |= 1 << (row % 64);
    for r in (0..row).rev() {
        let hit = history.commits.keys(r).iter().any(|&k| {
            let k = k as usize;
            k & EXTERNAL as usize == 0 && k <= row && marks[k / 64] & (1 << (k % 64)) != 0
        });
        if hit {
            marks[r / 64] |= 1 << (r % 64);
        }
    }
    marks
}

impl Repo {
    /// The refs whose history has commit `id`: from `history` when it can tell (no git),
    /// else one `git for-each-ref --contains`.
    pub fn refs_containing(&self, history: &History, id: ObjectId) -> Result<Containing> {
        if let Some(c) = history.refs_containing(&id) {
            return Ok(c);
        }
        let hex = id.to_string();
        // Oldest first: for tags that is nearest first.
        let out = self.git().run(&[
            "for-each-ref",
            "--contains",
            &hex,
            "--sort=creatordate",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ])?;
        let mut c = Containing {
            from_git: true,
            ..Containing::none(id)
        };
        for name in out.stdout_str().lines() {
            if let Some(b) = name.strip_prefix("refs/heads/") {
                c.local.push(b.to_string());
            } else if let Some(r) = name.strip_prefix("refs/remotes/") {
                // `origin/HEAD` names another remote branch.
                if !r.ends_with("/HEAD") {
                    c.remote.push(r.to_string());
                }
            } else if let Some(t) = name.strip_prefix("refs/tags/") {
                c.tags.push(t.to_string());
            }
        }
        c.local.sort();
        c.remote.sort();
        c.head = self
            .git()
            .run(&["merge-base", "--is-ancestor", &hex, "HEAD"])
            .is_ok();
        Ok(c)
    }
}

impl Containing {
    /// Nothing contains `id` (yet).
    pub fn none(id: ObjectId) -> Self {
        Containing {
            id,
            head: false,
            local: Vec::new(),
            remote: Vec::new(),
            tags: Vec::new(),
            from_git: false,
        }
    }

    /// Branches and tags found (HEAD not counted).
    pub fn total(&self) -> usize {
        self.local.len() + self.remote.len() + self.tags.len()
    }

    /// The window title: "Branches and tags containing abc1234".
    pub fn title(&self) -> String {
        format!("Branches and tags containing {}", short(&self.id))
    }

    /// The window text: the groups with their counts, each capped at [`SHOWN`];
    /// `head_branch` is the checked-out branch (`None`: HEAD is detached), marked in the
    /// list (or HEAD named when detached); `subject` is the commit's first line.
    pub fn describe(&self, subject: &str, head_branch: Option<&str>) -> String {
        let id = short(&self.id);
        let mut s = format!("{id} {subject}\n\n");
        if self.total() == 0 {
            s.push_str("No branch or tag contains this commit");
            if self.head && head_branch.is_none() {
                s.push_str(" — only the detached HEAD has it in its history");
            }
            s.push_str(
                ".\nIt is reachable only from HEAD, a stash or a ref outside branches and \
                 tags; a branch created here (Create Branch Here…) keeps it.",
            );
            return s;
        }
        s.push_str("Its history is part of:\n");
        if self.head && head_branch.is_none() {
            s.push_str("\nHEAD (detached) has it.\n");
        }
        let current = |name: &str| {
            if Some(name) == head_branch {
                format!("{name} (checked out)")
            } else {
                name.to_string()
            }
        };
        let local: Vec<String> = self.local.iter().map(|b| current(b)).collect();
        group(&mut s, "Local branches", &local, "");
        group(&mut s, "Remote branches", &self.remote, "");
        group(&mut s, "Tags", &self.tags, ", nearest first");
        if self.from_git {
            s.push_str("\n(Read with git for-each-ref --contains: the history was still loading.)\n");
        }
        s
    }
}

fn group(s: &mut String, title: &str, names: &[String], note: &str) {
    use std::fmt::Write as _;
    if names.is_empty() {
        let _ = writeln!(s, "\n{title}: none");
        return;
    }
    let _ = writeln!(s, "\n{title} ({}){note}:", names.len());
    for n in names.iter().take(SHOWN) {
        let _ = writeln!(s, "  {n}");
    }
    if names.len() > SHOWN {
        let _ = writeln!(s, "  … and {} more", names.len() - SHOWN);
    }
}

fn short(id: &ObjectId) -> String {
    id.to_hex_with_len(7).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix}{i}")).collect()
    }

    #[gitti_test::test]
    fn describe_caps_groups_and_marks_the_current_branch() {
        let c = Containing {
            id: ObjectId::null(gix::hash::Kind::Sha1),
            head: true,
            local: vec!["feature".into(), "main".into()],
            remote: Vec::new(),
            tags: names("v", SHOWN + 5),
            from_git: false,
        };
        let text = c.describe("Fix it", Some("main"));
        assert!(text.starts_with("0000000 Fix it\n"), "{text}");
        assert!(text.contains("Local branches (2):\n  feature\n  main (checked out)\n"), "{text}");
        assert!(text.contains("Remote branches: none"), "{text}");
        assert!(text.contains(&format!("Tags ({}), nearest first:\n  v0\n", SHOWN + 5)), "{text}");
        assert!(text.contains("  … and 5 more"), "{text}");
        assert!(!text.contains(&format!("  v{SHOWN}\n")), "{text}");
        assert!(!text.contains("HEAD (detached)"), "{text}");
    }

    #[gitti_test::test]
    fn describe_says_when_nothing_contains_it() {
        let c = Containing {
            head: true,
            ..Containing::none(ObjectId::null(gix::hash::Kind::Sha1))
        };
        let text = c.describe("Lost", None);
        assert!(text.contains("No branch or tag contains this commit — only the detached HEAD"));
        assert!(text.contains("Create Branch Here…"));
    }
}
