//! What the sidebar's context menus offer, in gitti and gitty alike: the entries
//! each kind of row (and each section header) must have. Both apps' tests open every menu
//! and check it against this table (gitti `tests/menus.rs`, gitty `tests/sidebar_menus.rs`),
//! so a menu can't silently lose an entry and the two apps keep the same choices. An app
//! may offer more (gitti: "Open in New Tab", "Merge … into current…" with options; gitty:
//! "Switch to Worktree"), never less.
//!
//! Labels are compared with [`menu_key`]: lower case, without "…", a trailing state
//! ("(checked out here)", ": on") or a key hint after two spaces ("Mark for Comparison  ⇧C").
//! Placeholders: `{name}` the row's own name (branch, `origin/main`, tag), `{head}` the
//! current branch, `{remote}` the row's remote (a remote branch's, else the first remote),
//! `{branch}` a remote branch's name without the remote, `{upstream}` a local branch's
//! upstream (`origin/x`; "Upstream" when it has none), `{fm}` the platform's file manager
//! name.

use crate::palette::normalize;

/// A kind of sidebar row, or a section header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SideMenu {
    Remote,
    /// A local branch other than the checked-out one.
    Branch,
    /// The checked-out branch.
    CurrentBranch,
    RemoteBranch,
    Tag,
    Stash,
    /// A linked worktree other than this one.
    Worktree,
    /// The main worktree when it is the only one (setting `show_main_worktree_alone`):
    /// the folder's entries without tab / lock / move / remove.
    MainWorktreeAlone,
    Submodule,
    Subtree,
    BranchesSection,
    RemotesSection,
    TagsSection,
    StashesSection,
    WorktreesSection,
    SubmodulesSection,
    SubtreesSection,
}

/// Every menu of the table, for tests that walk them all.
pub const ALL: &[SideMenu] = &[
    SideMenu::Remote,
    SideMenu::Branch,
    SideMenu::CurrentBranch,
    SideMenu::RemoteBranch,
    SideMenu::Tag,
    SideMenu::Stash,
    SideMenu::Worktree,
    SideMenu::MainWorktreeAlone,
    SideMenu::Submodule,
    SideMenu::Subtree,
    SideMenu::BranchesSection,
    SideMenu::RemotesSection,
    SideMenu::TagsSection,
    SideMenu::StashesSection,
    SideMenu::WorktreesSection,
    SideMenu::SubmodulesSection,
    SideMenu::SubtreesSection,
];

/// The entries of `menu`, with placeholders.
pub fn entries(menu: SideMenu) -> &'static [&'static str] {
    match menu {
        SideMenu::Remote => &[
            "Fetch {remote}",
            "Pull from {remote}",
            "Push {head} to {remote}",
            "Prune Stale Branches of {remote}",
            "Copy URL",
            "Edit URL…",
            "Rename…",
            "Remove…",
            "Add Remote…",
        ],
        SideMenu::Branch => &[
            "Checkout {name}",
            "Merge {name} into {head}",
            "Rebase {head} onto {name}",
            "Fast-Forward {name} to {upstream}",
            "Push {name} to {remote}",
            "Force Push {name} to {remote}…",
            "Push Options…",
            "Set Upstream…",
            "Create Branch From…",
            "Create Tag…",
            "Mark for Comparison",
            "Show Reflog…",
            "Add as Subtree…",
            "Rename…",
            "Delete…",
            "Force Delete…",
            "Pin to Top",
            "Copy Name",
        ],
        SideMenu::CurrentBranch => &[
            "Pull",
            "Push",
            "Push {name} to {remote}",
            "Push Options…",
            "Set Upstream…",
            "Create Branch From…",
            "Create Tag…",
            "Mark for Comparison",
            "Show Reflog…",
            "Rename…",
            "Pin to Top",
            "Copy Name",
        ],
        SideMenu::RemoteBranch => &[
            "Checkout as local {branch}",
            "Merge {name} into {head}",
            "Rebase {head} onto {name}",
            "Create Branch From…",
            "Create Tag…",
            "Set as Upstream of {head}",
            "Mark for Comparison",
            "Reset {name} to Selected Commit…",
            "Add as Subtree…",
            "Delete {branch} on {remote}…",
            "Pin to Top",
            "Copy Name",
        ],
        SideMenu::Tag => &[
            "Show Tag",
            "Checkout {name}",
            "Merge {name} into {head}",
            "Create Branch From…",
            "Push Tag to {remote}",
            "Mark for Comparison",
            "Add as Subtree…",
            "Delete Tag…",
            "Delete Tag on {remote}…",
            "Copy Name",
        ],
        SideMenu::Stash => &[
            "Apply",
            "Pop",
            crate::stash::APPLY_INDEX_LABEL,
            crate::stash::POP_INDEX_LABEL,
            "Create Branch from Stash…",
            "Copy Message",
            "Drop…",
        ],
        SideMenu::Worktree => &[
            "Reveal in {fm}",
            "Open in Terminal",
            "Sparse Folders…",
            "Move…",
            "Remove Worktree…",
            "Copy Path",
            "Copy SHA",
        ],
        SideMenu::MainWorktreeAlone => &[
            "Reveal in {fm}",
            "Open in Terminal",
            "Sparse Folders…",
            "Copy Path",
            "Copy SHA",
        ],
        SideMenu::Submodule => &[
            "Update",
            "Update to Remote Branch",
            "Reveal in {fm}",
            "Open in Terminal",
            "Sync URL",
            "Copy Path",
            "Copy SHA",
            "Remove Submodule…",
        ],
        // `{name}` = the folder.
        SideMenu::Subtree => &[
            "Pull…",
            "Merge…",
            "Push…",
            "Split…",
            "Show History of {name}/",
            "Edit Source…",
            "Copy Path",
        ],
        SideMenu::BranchesSection => &[
            "Create Branch at HEAD…",
            "Switch Branch…",
            "Delete Branches…",
            "Clean Up Merged Branches…",
            crate::branch_update::ALL_LABEL,
            "Sort by Name",
            "Sort by Last Commit Date",
            "Sort by Recently Checked Out",
        ],
        SideMenu::RemotesSection => &[
            "Add Remote…",
            "Fetch All",
            "Sort by Name",
            "Sort by Last Commit Date",
        ],
        SideMenu::TagsSection => &["Create Tag at HEAD…", "Push All Tags to {remote}"],
        SideMenu::StashesSection => &[
            "Stash Changes…",
            "Stash Including Untracked",
            "Apply Latest Stash",
            "Pop Latest Stash",
            crate::stash::APPLY_LATEST_INDEX_LABEL,
            crate::stash::POP_LATEST_INDEX_LABEL,
        ],
        SideMenu::WorktreesSection => &["Add Worktree…", "Prune Worktrees", "Repair Worktrees…"],
        SideMenu::SubmodulesSection => &[
            "Add Submodule…",
            "Update Submodules",
            "Update Submodules to Remote Branches",
            "Sync Submodule URLs",
            "Run Command in Each Submodule…",
        ],
        SideMenu::SubtreesSection => &[
            "Add Subtree…",
            "Pull Subtree…",
            "Merge into Subtree…",
            "Push Subtree…",
            "Split Subtree…",
        ],
    }
}

/// Values for the placeholders of [`entries`].
#[derive(Debug, Clone, Default)]
pub struct Vars {
    pub name: String,
    pub head: String,
    pub remote: String,
    pub branch: String,
    /// A local branch's upstream; empty: none ("Upstream").
    pub upstream: String,
    pub fm: String,
}

impl Vars {
    fn fill(&self, label: &str) -> String {
        label
            .replace("{name}", &self.name)
            .replace("{head}", &self.head)
            .replace("{remote}", &self.remote)
            .replace("{branch}", &self.branch)
            .replace(
                "{upstream}",
                if self.upstream.is_empty() { "Upstream" } else { &self.upstream },
            )
            .replace("{fm}", &self.fm)
    }
}

/// A menu label reduced for comparing (see the module docs).
pub fn menu_key(label: &str) -> String {
    let label = label.split("  ").next().unwrap_or(label);
    normalize(label)
}

/// The entries of `menu` (filled in) that `labels` — what an app's menu shows — lacks.
pub fn missing(menu: SideMenu, vars: &Vars, labels: &[String]) -> Vec<String> {
    let have: Vec<String> = labels.iter().map(|l| menu_key(l)).collect();
    entries(menu)
        .iter()
        .map(|e| vars.fill(e))
        .filter(|e| !have.contains(&menu_key(e)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn keys_drop_hints_and_states() {
        assert_eq!(menu_key("Mark for Comparison  ⇧C"), "mark for comparison");
        assert_eq!(menu_key("Delete… (checked out here)"), "delete");
        assert_eq!(menu_key("Set Upstream… (now origin/main)"), "set upstream");
    }

    #[gitti_test::test]
    fn table_is_unique_and_missing_fills_in() {
        for &m in ALL {
            let mut seen = std::collections::HashSet::new();
            for e in entries(m) {
                assert!(seen.insert(menu_key(e)), "{m:?}: {e} twice");
            }
        }
        let vars = Vars {
            name: "v1".into(),
            head: "main".into(),
            remote: "origin".into(),
            ..Default::default()
        };
        let have = vec!["Checkout v1 (detached)".to_string(), "Push Tag to origin".into()];
        let miss = missing(SideMenu::Tag, &vars, &have);
        assert!(miss.contains(&"Merge v1 into main".to_string()), "{miss:?}");
        assert!(!miss.iter().any(|m| m.starts_with("Checkout")), "{miss:?}");
        assert!(!miss.iter().any(|m| m.starts_with("Push Tag")), "{miss:?}");
    }
}
