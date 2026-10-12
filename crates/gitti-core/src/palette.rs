//! What the command palettes must offer: every command of gitti's menu bar, with
//! whether gitty's palette has it (its label there) or why not. Tests hold the apps to it:
//! gitti's `tests/menus.rs` checks that this table and the menu bar list the same actions
//! and that gitti's palette has each one; gitty's `tests/palette.rs` checks its palette
//! against the `Gitty::Has` labels. A new menu item therefore fails a test until it is
//! listed here — and in gitty's palette or with a reason.

/// gitty's side of a menu command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gitty {
    /// In gitty's palette under this label (compared with [`normalize`]; `{fm}` = the
    /// platform's file manager name).
    Has(&'static str),
    /// Not in gitty's palette, and why (no such thing in a terminal, or a noted gap).
    No(&'static str),
}

/// A command of gitti's menu bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuCommand {
    /// gitti's action name (`gitti::<action>`).
    pub action: &'static str,
    /// `Some(why)`: gitti's palette lists it only in some states (not checked).
    pub gitti_only_when: Option<&'static str>,
    pub gitty: Gitty,
}

const fn both(action: &'static str, gitty: &'static str) -> MenuCommand {
    MenuCommand {
        action,
        gitti_only_when: None,
        gitty: Gitty::Has(gitty),
    }
}

const fn gitti(action: &'static str, why_not_gitty: &'static str) -> MenuCommand {
    MenuCommand {
        action,
        gitti_only_when: None,
        gitty: Gitty::No(why_not_gitty),
    }
}

const fn when(action: &'static str, when: &'static str, gitty: Gitty) -> MenuCommand {
    MenuCommand {
        action,
        gitti_only_when: Some(when),
        gitty,
    }
}

const TABS: &str = "gitty shows one repository (no tabs)";
const FONT: &str = "the terminal's font";
const CONFIG: &str = "git config dialogs: gitty gap (`:` runs git config)";
const STATE: &str = "only while comparing";

pub const MENU_COMMANDS: &[MenuCommand] = &[
    // File
    gitti("OpenRepository", TABS),
    gitti("OpenRecent", TABS),
    both("CloneRepository", "Clone Repository…"),
    both("InitRepository", "Init Repository…"),
    gitti("CloseTab", TABS),
    gitti("NextTab", TABS),
    gitti("PrevTab", TABS),
    // View
    when(
        "CommandPalette",
        "the palette itself",
        Gitty::No("the palette itself"),
    ),
    both("ToggleSidebar", "Toggle Sidebar"),
    both(
        "ToggleMainWorktreeAlone",
        "Toggle Main Worktree When It Is the Only One",
    ),
    gitti("ToggleFullScreen", "a terminal"),
    both("ToggleGitLog", "Toggle Git Command Log"),
    both("RunGitCommand", "Run Git Command…"),
    both("RunCustomCommand", "Run Custom Command…"),
    both("CustomCommands", "Custom Commands…"),
    both("ToggleWrap", "Wrap Lines"),
    both("ToggleChangesTree", "Changes as Tree"),
    both("ToggleWhitespace", "Ignore Whitespace"),
    both("MoreContext", "More Context Lines"),
    both("LessContext", "Fewer Context Lines"),
    both("ShowWholeFile", "Show Whole File"),
    gitti("ZoomIn", FONT),
    gitti("ZoomOut", FONT),
    gitti("ZoomDiffIn", FONT),
    gitti("ZoomDiffOut", FONT),
    gitti("ResetZoom", FONT),
    both("ToggleOrder", "History Order"),
    // The commit list layout (`list_layout::CommitListLayout::label`).
    both("CommitListAuto", "Auto Commit List Layout"),
    both("CommitListOneLine", "One-Line Commit List"),
    both("CommitListTwoLines", "Two-Line Commit List"),
    // The sidebar's branch order (`branch_order::BranchSort::label`).
    both("SortBranchesByName", "Sort Branches by Name"),
    both("SortBranchesByDate", "Sort Branches by Last Commit Date"),
    both("SortBranchesByRecent", "Sort Branches by Recently Checked Out"),
    both("SortRemoteBranchesByName", "Sort Remote Branches by Name"),
    both("SortRemoteBranchesByDate", "Sort Remote Branches by Last Commit Date"),
    both("Refresh", "Refresh"),
    // Repository
    both("OpenInFileManager", "Open in {fm}"),
    gitti("OpenTerminal", "gitty runs in a terminal"),
    both("AddWorktree", "Add Worktree…"),
    both("PruneWorktrees", "Prune Worktrees"),
    // gitty: "Remove This Worktree…" too, but only in a linked worktree (not checked).
    when(
        "RemoveThisWorktree",
        "only in a linked worktree's tab",
        Gitty::No("only in a linked worktree"),
    ),
    // gitty: "Move This Worktree…" too, but only in a linked worktree (not checked).
    when(
        "MoveThisWorktree",
        "only in a linked worktree's tab",
        Gitty::No("only in a linked worktree"),
    ),
    both("RepairWorktrees", "Repair Worktrees…"),
    // Always in both palettes (without submodules they say there are none).
    both("AddSubmodule", "Add Submodule…"),
    both("UpdateSubmodules", "Update Submodules"),
    both("UpdateSubmodulesRemote", "Update Submodules to Remote Branches"),
    both("CheckoutSubmoduleCommit", "Check Out Commit in Submodule…"),
    both("SyncSubmodules", "Sync Submodule URLs"),
    both("RemoveSubmodule", "Remove Submodule…"),
    both("ForeachSubmodule", "Run Command in Each Submodule…"),
    // Always in both palettes (without subtrees they open with an empty folder).
    both("AddSubtree", "Add Subtree…"),
    both("PullSubtree", "Pull Subtree…"),
    both("MergeIntoSubtree", "Merge into Subtree…"),
    both("PushSubtree", "Push Subtree…"),
    both("SplitSubtree", "Split Subtree…"),
    both("AddToGitignore", "Add to .gitignore…"),
    both("EditGitignore", "Edit .gitignore…"),
    both("TrackWithLfs", "Track with Git LFS…"),
    both("EditGitattributes", "Edit .gitattributes…"),
    both("RenormalizeLineEndings", crate::renormalize::MENU_LABEL),
    both("CleanUntrackedIgnored", crate::clean::MENU_LABEL),
    // Paths this file system can't hold (`fs_paths`): the note, hide (skip-worktree),
    // the undo.
    both("FsPaths", "Paths This File System Can't Hold…"),
    both("HideFsPaths", "Hide Paths This File System Can't Hold"),
    both("ShowHiddenFsPaths", "Show Hidden Paths Again"),
    gitti("RepoGitSettings", CONFIG),
    gitti("GlobalGitSettings", CONFIG),
    both("ChangeCommitEncoding", crate::commit_encoding::change::MENU_LABEL),
    gitti("EditRepoConfig", CONFIG),
    gitti("EditGlobalConfig", CONFIG),
    // The external diff / merge tool (`tool_setup`).
    both("ChooseExternalTool", crate::tool_setup::CHOOSE_LABEL),
    both("WriteCommitGraph", "Write Commit-Graph"),
    // A shallow clone's history (`shallow`); greyed in a complete one.
    both("FetchFullHistory", crate::shallow::FULL_LABEL),
    both("FetchMoreHistory", crate::shallow::MORE_LABEL),
    both("RepositoryStatistics", crate::repo_stats::MENU_LABEL),
    // Branch
    both("SwitchBranch", "Switch Branch…"),
    both("CreateBranch", "Create Branch at HEAD…"),
    both("MergeBranch", "Merge Branch into Current…"),
    both("RebaseCurrentOnto", "Rebase Current Branch onto…"),
    both("RenameCurrentBranch", "Rename Current Branch…"),
    both("SetUpstreamMenu", "Set Upstream of Current Branch…"),
    both("DeleteBranchMenu", "Delete Branch…"),
    both("DeleteBranches", "Delete Branches…"),
    both("CleanUpMergedBranches", "Clean Up Merged Branches…"),
    both("FastForwardAllBranches", crate::branch_update::ALL_LABEL),
    // Commit
    both("Commit", "Commit"),
    both("StageAllAndCommit", "Stage All and Commit"),
    both("StageTrackedAndCommit", "Stage Tracked Files and Commit"),
    both("CreateEmptyCommit", "Create Empty Commit"),
    both("ToggleAmend", "Toggle Amend"),
    both("ToggleSignOff", "Sign Off Commits"),
    both("RecentMessages", "Recent Commit Messages…"),
    both("GoToWorkingDirectory", "Go to Working Directory"),
    both("StageAll", "Stage All"),
    both("UnstageAll", "Unstage All"),
    both("DiscardAll", "Discard All Unstaged…"),
    both("RevertCommit", "Revert Selected Commit"),
    both("CherryPickCommit", "Cherry-pick Selected Commit"),
    both("CherryPickMarked", "Cherry-pick Marked Commits…"),
    both("RevertMarked", "Revert Marked Commits…"),
    both("EditCommitMessage", "Edit Commit Message…"),
    both("ShowContaining", "Branches and Tags Containing Selected Commit…"),
    both("RestoreFromCommit", "Restore File or Folder from Selected Commit…"),
    // The shown commit's / stash's selected lines (`commit_lines`).
    both("RevertLinesInWorkingTree", crate::commit_lines::REVERT_SELECTION_LABEL),
    both("ApplyLinesToWorkingTree", crate::commit_lines::APPLY_SELECTION_LABEL),
    both("OpenThisVersion", crate::version_file::THIS_LABEL),
    both("OpenOldVersion", crate::version_file::OLD_LABEL),
    // On the file at the diff cursor (`ext_tool`).
    both("OpenExternalDiffTool", "Open in External Diff Tool"),
    both("OpenExternalMergeTool", "Open in External Merge Tool"),
    // Also on the signature (gitti: right-click / details; gitty: `m`, `K`).
    both("GetSigningKeys", "Get Signing Keys from GitHub…"),
    both("MarkCompare", "Mark Selected Commit for Comparison"),
    when("SwapCompare", STATE, Gitty::No(STATE)),
    both("ToggleCompareMergeBase", "Changes Since Common Ancestor"),
    when("EndCompare", STATE, Gitty::No(STATE)),
    both("CreateTag", "Create Tag at HEAD…"),
    // Stash
    both("StashChanges", "Stash…"),
    both("StashIncludingUntracked", "Stash Including Untracked"),
    both("StashSelection", crate::stash::SELECTION_LABEL),
    both("ApplyStash", "Apply Latest Stash"),
    both("PopStash", "Pop Latest Stash"),
    both("ApplyStashIndex", crate::stash::APPLY_LATEST_INDEX_LABEL),
    both("PopStashIndex", crate::stash::POP_LATEST_INDEX_LABEL),
    both("DropLatestStash", "Drop Latest Stash…"),
    both("DropStash", "Drop Stash…"),
    both("RenameStash", "Rename Stash…"),
    // Remote
    both("Fetch", "Fetch"),
    both("FetchFrom", "Fetch from…"),
    both("Pull", "Pull"),
    both("PullFrom", "Pull from…"),
    both("PullWith", "Pull with…"),
    both("Push", "Push"),
    both("PushTo", "Push…"),
    both("ResetRemoteBranch", "Reset Remote Branch to Selected Commit…"),
    both("AddRemote", "Add Remote…"),
    // Navigate
    both("GoToCommit", "Go to Commit…"),
    both("SearchChanges", "Search in Changes…"),
    gitti("GoToRef", "gitty: b switches, the sidebar lists them"),
    both("GoToHead", "Go to HEAD"),
    both("ShowReflog", "Reflog (Recover Lost Commits)…"),
    // The file list (`t`): gitti's file tabs ▾ (or under the command bar without
    // tabs), gitty's file picker.
    both("FileList", "Go to File…"),
    both("SelectNext", "Next Commit"),
    both("SelectPrev", "Previous Commit"),
    gitti("FollowBranchDown", "gitty: alt-↓ (keys only)"),
    gitti("FollowBranchUp", "gitty: alt-↑ (keys only)"),
    gitti("OtherBranchDown", "gitty: alt-shift-↓ (keys only)"),
    gitti("OtherBranchUp", "gitty: alt-shift-↑ (keys only)"),
    both("NextFile", "Next File"),
    both("PrevFile", "Previous File"),
    both("NextHunk", "Next Hunk"),
    both("PrevHunk", "Previous Hunk"),
    // Help
    both("ShowShortcuts", "Keyboard Shortcuts"),
    both("StartupTimings", "Start-up Timings…"),
    both("ThirdPartyLicences", "Third-Party Licences…"),
    both("About", "About gitty"),
    both("ErrorDetails", "Show Error Details"),
];

/// A key of the shared table (`crate::keys`, gpui syntax) as a terminal palette writes it:
/// a shifted letter is the capital (`shift-p` → `P`, as typed), function keys capitalised
/// (`f10` → `F10`), the rest as is (`ctrl-p`, `:`, `]`).
pub fn terminal_key(key: &str) -> String {
    if let Some(rest) = key.strip_prefix("shift-")
        && rest.len() == 1
        && rest.chars().all(|c| c.is_ascii_lowercase())
    {
        return rest.to_ascii_uppercase();
    }
    if let Some(n) = key.strip_prefix('f')
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
    {
        return format!("F{n}");
    }
    key.to_string()
}

/// The keys of shared command `c` for a terminal palette's keys column, from the
/// key table — never written by hand: `"f"`, `"P"`, `"F10"`; several joined by " / ";
/// empty when it has none.
pub fn command_keys(c: crate::keys::Command) -> String {
    command_keys_in(c, crate::keys::KeyboardLayout::Us)
}

/// [`command_keys`] as the keyboard layout `layout` types them (`ä` for `]` on German).
pub fn command_keys_in(c: crate::keys::Command, layout: crate::keys::KeyboardLayout) -> String {
    crate::keys::BINDINGS
        .iter()
        .filter(|b| b.command == c)
        .map(|b| terminal_key(&layout.remap(b.key)))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Words the palettes treat as one, so a command is found under either word: typing
/// any word of a group finds the commands labelled with the others too — "new" finds
/// "Create Tag at HEAD…" and "Add Remote…", "remove" finds "Delete Branch…" and "Drop
/// Stash…", "checkout" finds "Switch Branch…". Lower case, ASCII; a phrase is matched as
/// written ("check out").
pub const SYNONYMS: &[&[&str]] = &[
    &["create", "new", "add"],
    &["delete", "remove", "drop"],
    &["switch", "checkout", "check out"],
    &["search", "find", "filter"],
];

/// At most this many variants per label ([`label_variants`]).
pub const MAX_VARIANTS: usize = 16;

/// The longest [`SYNONYMS`] word at byte `i` of `label` that ends a word: its length and
/// its group. `i` must start a word.
fn synonym_at(label: &str, i: usize) -> Option<(usize, &'static [&'static str])> {
    let rest = &label.as_bytes()[i..];
    SYNONYMS
        .iter()
        .flat_map(|g| g.iter().map(move |w| (*w, *g)))
        .filter(|(w, _)| {
            rest.len() >= w.len()
                && rest[..w.len()].eq_ignore_ascii_case(w.as_bytes())
                && label[i + w.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric())
        })
        .map(|(w, g)| (w.len(), g))
        .max_by_key(|(len, _)| *len)
}

/// Synonym `w` written in the case of the label's `word`: capitalised words when it
/// starts with a capital ("Check Out"), else as is.
fn in_case_of(w: &str, word: &str) -> String {
    if !word.starts_with(|c: char| c.is_uppercase()) {
        return w.to_string();
    }
    w.split(' ')
        .map(|part| {
            let mut cs = part.chars();
            cs.next()
                .map(|c| c.to_uppercase().chain(cs).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `label` and its synonym variants: each [`SYNONYMS`] word in it (a whole word,
/// any case) swapped for the other words of its group, in the word's case ("Add Remote…"
/// → "Create Remote…", "New Remote…"); with several such words every combination, at
/// most [`MAX_VARIANTS`]. The label itself comes first; one without such a word has no
/// variants.
pub fn label_variants(label: &str) -> Vec<String> {
    // The label as segments: fixed text, or a synonym word with its alternatives (the
    // word itself first).
    let mut segments: Vec<Vec<String>> = Vec::new();
    let (mut i, mut fixed_from) = (0, 0);
    while i < label.len() {
        let word_start = label[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        match word_start.then(|| synonym_at(label, i)).flatten() {
            Some((len, group)) => {
                if fixed_from < i {
                    segments.push(vec![label[fixed_from..i].to_string()]);
                }
                let word = &label[i..i + len];
                let mut alts = vec![word.to_string()];
                alts.extend(
                    group
                        .iter()
                        .filter(|w| !w.eq_ignore_ascii_case(word))
                        .map(|w| in_case_of(w, word)),
                );
                segments.push(alts);
                i += len;
                fixed_from = i;
            }
            None => i += label[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    if segments.is_empty() {
        return vec![label.to_string()];
    }
    if fixed_from < label.len() {
        segments.push(vec![label[fixed_from..].to_string()]);
    }
    let mut out = vec![String::new()];
    for seg in &segments {
        out = out
            .iter()
            .flat_map(|head| seg.iter().map(move |s| format!("{head}{s}")))
            .take(MAX_VARIANTS)
            .collect();
    }
    out
}

/// A palette entry's score for `query` with synonyms: `score` is the app's own
/// fuzzy matcher (higher is better, `None` = no match); a [`label_variants`] variant
/// counts `penalty` less than the label itself. So every command under either word is
/// found, and the word as labelled ranks first.
pub fn synonym_score<S>(
    query: &str,
    label: &str,
    penalty: S,
    score: impl Fn(&str, &str) -> Option<S>,
) -> Option<S>
where
    S: Ord + Copy + std::ops::Sub<Output = S>,
{
    let own = score(query, label);
    label_variants(label)
        .iter()
        .skip(1)
        .filter_map(|v| score(query, v))
        .map(|s| s - penalty)
        .chain(own)
        .max()
}

/// A palette / menu label reduced for comparing: lower case, without "…", a trailing
/// state ("(on)", ": off", "(3)") and surrounding space.
pub fn normalize(label: &str) -> String {
    let mut s = label.replace('…', "");
    for cut in [" (", ": "] {
        if let Some(i) = s.find(cut) {
            s.truncate(i);
        }
    }
    s.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn normalize_drops_states() {
        assert_eq!(normalize("Wrap Lines: on"), "wrap lines");
        assert_eq!(normalize("More Context Lines (3)"), "more context lines");
        assert_eq!(normalize("Push…"), "push");
    }

    #[gitti_test::test]
    fn terminal_keys_from_the_table() {
        use crate::keys::Command;
        assert_eq!(terminal_key("shift-p"), "P");
        assert_eq!(terminal_key("f10"), "F10");
        assert_eq!(terminal_key("f"), "f");
        assert_eq!(terminal_key("ctrl-p"), "ctrl-p");
        assert_eq!(terminal_key("shift-tab"), "shift-tab");
        assert_eq!(command_keys(Command::Push), "P");
        assert_eq!(command_keys(Command::Quit), "F10");
        assert_eq!(command_keys(Command::FileList), "t");
    }

    #[gitti_test::test]
    fn label_variants_swap_synonyms_in_their_case() {
        assert_eq!(
            label_variants("Add Remote…"),
            ["Add Remote…", "Create Remote…", "New Remote…"]
        );
        assert_eq!(
            label_variants("Create Tag at HEAD…"),
            ["Create Tag at HEAD…", "New Tag at HEAD…", "Add Tag at HEAD…"]
        );
        assert_eq!(
            label_variants("Drop Latest Stash…"),
            ["Drop Latest Stash…", "Delete Latest Stash…", "Remove Latest Stash…"]
        );
        let sub = label_variants("Check Out Commit in Submodule…");
        assert_eq!(sub[0], "Check Out Commit in Submodule…");
        assert!(sub.contains(&"Switch Commit in Submodule…".to_string()), "{sub:?}");
        assert!(sub.contains(&"Checkout Commit in Submodule…".to_string()), "{sub:?}");
        // Lower case stays lower case; whole words only; no word, no variants.
        assert_eq!(
            label_variants("git stash drop"),
            ["git stash drop", "git stash delete", "git stash remove"]
        );
        assert_eq!(label_variants("Address Book"), ["Address Book"]);
        assert_eq!(label_variants("Re-add"), ["Re-add", "Re-create", "Re-new"]);
        assert_eq!(label_variants("Fetch"), ["Fetch"]);
        assert_eq!(label_variants("Größe ändern"), ["Größe ändern"]);
        // Two words: every combination (3 × 3), the label first.
        let two = label_variants("Delete New");
        assert_eq!(two.len(), 9);
        assert_eq!(two[0], "Delete New");
        assert!(two.contains(&"Remove Create".to_string()), "{two:?}");
        assert!(label_variants("add add add add").len() <= MAX_VARIANTS);
    }

    /// A small stand-in for the apps' fuzzy matchers: a substring scores by its position,
    /// a scattered subsequence low.
    fn test_score(query: &str, text: &str) -> Option<i32> {
        let q: String = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        let t = text.to_lowercase();
        if let Some(at) = t.find(&q) {
            return Some(100 - at as i32);
        }
        let mut cs = t.chars();
        q.chars().all(|c| cs.any(|x| x == c)).then_some(10)
    }

    #[gitti_test::test]
    fn synonyms_find_every_command_under_either_word() {
        let labels = [
            "Create Branch at HEAD…",
            "Create Tag at HEAD…",
            "Add Remote…",
            "Add Worktree…",
            "Delete Branch…",
            "Drop Stash…",
            "Remove Submodule…",
            "Switch Branch…",
            "Fetch",
        ];
        let found = |q: &str| -> Vec<&str> {
            let mut v: Vec<(i32, &str)> = labels
                .iter()
                .filter_map(|l| synonym_score(q, l, 30, test_score).map(|s| (s, *l)))
                .collect();
            v.sort_by_key(|(s, _)| -s);
            v.into_iter().map(|(_, l)| l).collect()
        };
        for q in ["create", "new", "add"] {
            let f = found(q);
            for l in &labels[..4] {
                assert!(f.contains(l), "{q}: {l} missing in {f:?}");
            }
        }
        // The typed word's own commands first.
        assert_eq!(&found("add")[..2], ["Add Remote…", "Add Worktree…"]);
        assert_eq!(&found("create")[..2], ["Create Branch at HEAD…", "Create Tag at HEAD…"]);
        for q in ["delete", "remove", "drop"] {
            let f = found(q);
            for l in &labels[4..7] {
                assert!(f.contains(l), "{q}: {l} missing in {f:?}");
            }
        }
        assert_eq!(found("remove")[0], "Remove Submodule…");
        assert_eq!(found("checkout"), ["Switch Branch…"]);
        assert_eq!(found("check out"), ["Switch Branch…"]);
        assert!(!found("new").contains(&"Fetch"));
        // The label as written never scores below a synonym of it.
        for l in labels {
            for q in ["create", "new", "add", "delete", "remove", "drop", "switch", "branch"] {
                if let Some(own) = test_score(q, l) {
                    assert_eq!(synonym_score(q, l, 30, test_score), Some(own), "{q} / {l}");
                }
            }
        }
    }

    #[gitti_test::test]
    fn table_is_unique_and_explained() {
        let mut seen = std::collections::HashSet::new();
        for c in MENU_COMMANDS {
            assert!(seen.insert(c.action), "{} twice", c.action);
            if let Gitty::No(why) = c.gitty {
                assert!(!why.is_empty(), "{}: a reason", c.action);
            }
        }
    }
}
