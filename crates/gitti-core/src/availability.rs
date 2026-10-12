//! Which menu commands can act now, and why not: actions that make no sense in the current
//! state are greyed out, with the reason. One table for both apps, over the menu command
//! names of [`crate::palette::MENU_COMMANDS`]: gitti greys its menu bar entries and palette
//! entries with the reason and says it when a key runs one anyway; gitty does the same in
//! its palette. The facts come from the last refresh (no git): what the apps already show.
//!
//! A command that opens something explaining itself stays available (Commit with nothing
//! staged asks what to do, Pull without an upstream opens "Pull from…", Go to Branch / Tag
//! lists what there is); only states where it could do nothing — or only fail — are here.

use crate::ops::RepoState;

/// What a selected row of the commit list is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Selection {
    /// Nothing (an empty list).
    #[default]
    None,
    /// The working directory row.
    WorkingDir,
    /// A commit; `head`: the checked-out one.
    Commit { head: bool },
    /// A stash row (or a stash's helper commit).
    Stash,
}

/// The file at the diff's cursor, for "Open in External Diff / Merge Tool".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorFile {
    /// None (no diff, or the cursor is not on a file's rows).
    #[default]
    None,
    /// A changed file (working directory, a commit, a comparison).
    Changed,
    /// A conflicted file.
    Conflicted,
}

/// The state an app is in, as far as the menu commands care (from its last refresh).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoFacts {
    /// A repository is open (not the start page, not a "not a repository" page).
    pub repo: bool,
    /// Open tabs (gitti; gitty shows one repository: 1).
    pub tabs: usize,
    /// HEAD has no commit yet (a fresh `git init`).
    pub unborn: bool,
    /// HEAD is detached (a commit, no branch).
    pub detached: bool,
    pub remotes: usize,
    pub stashes: usize,
    /// Local branches besides the checked-out one.
    pub other_branches: usize,
    pub staged: usize,
    /// Unstaged changes, untracked files included.
    pub unstaged: usize,
    /// Unstaged changes of tracked files `git add -u` stages
    /// (`ops::UnstagedCounts::tracked`).
    pub tracked_unstaged: usize,
    pub conflicted: usize,
    /// A merge / rebase / cherry-pick / revert git left half-done.
    pub state: RepoState,
    /// This is a linked worktree (not the main one).
    pub linked_worktree: bool,
    /// Linked worktrees of the repository (besides the main one).
    pub linked_worktrees: usize,
    /// Worktrees git would prune (their folder is gone).
    pub prunable_worktrees: usize,
    pub submodules: usize,
    /// Compare mode is on.
    pub comparing: bool,
    pub selection: Selection,
    /// The diff shows at least one file.
    pub diff_file: bool,
    /// The shown diff is a commit's (or a comparison's) with a new / an old side to open
    /// as a read-only copy ("Open This / Old Version in Editor").
    pub this_version: bool,
    pub old_version: bool,
    /// The file at the diff's cursor (the external tools act on it).
    pub cursor_file: CursorFile,
    /// A commit's or a stash's diff is shown (not a comparison): its lines can be
    /// reverted / applied in the working tree (`commit_lines`).
    pub commit_diff: bool,
    /// A remote points to github.com.
    pub github_remote: bool,
    /// The shown commit's signature can't be checked for a missing key that GitHub may
    /// have (`signing_keys::offer`).
    pub signing_keys: bool,
    /// Index paths this work tree's file system can't hold, still in the way
    /// ([`crate::fs_paths::PathReport::shown`]), those of them "Hide Them" hides now
    /// (`hidable`), and those hidden with skip-worktree.
    pub fs_paths: usize,
    pub fs_hidable: usize,
    pub fs_hidden: usize,
    /// Marked commits of the commit list ([`crate::commit_marks`]) and whether HEAD is
    /// one of them.
    pub marked_commits: usize,
    pub head_marked: bool,
}

impl Default for RepoFacts {
    fn default() -> Self {
        Self {
            repo: false,
            tabs: 0,
            unborn: false,
            detached: false,
            remotes: 0,
            stashes: 0,
            other_branches: 0,
            staged: 0,
            unstaged: 0,
            tracked_unstaged: 0,
            conflicted: 0,
            state: RepoState::Clean,
            linked_worktree: false,
            linked_worktrees: 0,
            prunable_worktrees: 0,
            submodules: 0,
            comparing: false,
            selection: Selection::None,
            diff_file: false,
            this_version: false,
            old_version: false,
            cursor_file: CursorFile::None,
            commit_diff: false,
            github_remote: false,
            signing_keys: false,
            fs_paths: 0,
            fs_hidable: 0,
            fs_hidden: 0,
            marked_commits: 0,
            head_marked: false,
        }
    }
}

pub const NO_REPO: &str = "no repository open";
pub const NO_TAB: &str = "no tab open";
pub const ONE_TAB: &str = "only one tab open";
pub const UNBORN: &str = "no commits yet";
pub const DETACHED: &str = "HEAD is detached";
pub const NO_REMOTES: &str = "no remotes";
pub const NO_STASHES: &str = "no stashes";
pub const NOTHING_STAGED: &str = "nothing staged";
pub const NOTHING_TO_STAGE: &str = "nothing to stage";
pub const NO_UNSTAGED: &str = "no unstaged changes";
pub const NO_CHANGES: &str = "no changes";
pub const NO_TRACKED_CHANGES: &str = "no changes to tracked files";
pub const NO_SUBMODULES: &str = "no submodules";
pub const MAIN_WORKTREE: &str = "this is the main worktree";
pub const NO_LINKED_WORKTREES: &str = "no linked worktrees";
pub const NOTHING_TO_PRUNE: &str = "no worktree folder is missing";
pub const NOT_COMPARING: &str = "not comparing";
pub const NO_COMMIT_SELECTED: &str = "no commit selected";
pub const WORKING_DIR_SELECTED: &str = "the working directory is selected";
pub const STASH_SELECTED: &str = "a stash is selected";
pub const IS_HEAD: &str = "the selected commit is HEAD";
pub const NO_FILE: &str = "no file shown";
pub const NO_COMMIT_FILE: &str = "no commit's file shown";
pub const NO_COMMIT_DIFF: &str = "no commit's changes shown";
pub const NO_OLD_VERSION: &str = "the commit has no parent";
pub const CONFLICTED_FILE: &str = "the file at the diff cursor is conflicted";
pub const NO_CONFLICTS: &str = "no conflicts";
pub const NO_CONFLICTED_FILE: &str = "no conflicted file at the diff cursor";
pub const NO_OTHER_BRANCH: &str = "no other local branch";
pub const NO_GITHUB_REMOTE: &str = "no GitHub remote";
pub const NO_KEY_MISSING: &str = "the shown commit has no signature whose key is missing";
pub const NO_FS_PATHS: &str = "every path fits this file system";
pub const NO_FS_PATH_CHANGED: &str = "none of them shows as changed";
pub const NO_HIDDEN_PATHS: &str = "no paths hidden";
pub const UNRESOLVED: &str = "conflicted files are not resolved yet";
pub const NO_MARKED_COMMITS: &str = "no commits marked";
pub const HEAD_MARKED: &str = "a marked commit is HEAD";

/// "a merge is in progress" & co. (`None`: no operation half-done).
pub fn in_progress(state: RepoState) -> Option<&'static str> {
    match state {
        RepoState::Clean => None,
        RepoState::Merging => Some("a merge is in progress"),
        RepoState::Rebasing => Some("a rebase is in progress"),
        RepoState::CherryPicking => Some("a cherry-pick is in progress"),
        RepoState::Reverting => Some("a revert is in progress"),
    }
}

/// Menu commands that act without a repository (the start page has them too).
const WITHOUT_REPO: &[&str] = &[
    "OpenRepository",
    "OpenRecent",
    "CloneRepository",
    "InitRepository",
    "CloseTab",
    "NextTab",
    "PrevTab",
    "CommandPalette",
    "ToggleFullScreen",
    "ToggleGitLog",
    "ZoomIn",
    "ZoomOut",
    "ZoomDiffIn",
    "ZoomDiffOut",
    "ResetZoom",
    "GlobalGitSettings",
    "EditGlobalConfig",
    "ShowShortcuts",
    "StartupTimings",
    "About",
    "ErrorDetails",
];

/// Why menu command `action` (a [`crate::palette::MENU_COMMANDS`] name, without
/// `gitti::`) can't act in state `f` — a short reason in plain words ("no stashes", "HEAD
/// is detached") — or `None` when it can.
pub fn why_not(action: &str, f: &RepoFacts) -> Option<&'static str> {
    let check = |cond: bool, why: &'static str| cond.then_some(why);
    match action {
        "CloseTab" => return check(f.tabs == 0, NO_TAB),
        "NextTab" | "PrevTab" => return check(f.tabs == 0, NO_TAB).or(check(f.tabs == 1, ONE_TAB)),
        _ => {}
    }
    if !f.repo {
        return check(!WITHOUT_REPO.contains(&action), NO_REPO);
    }
    let busy_op = in_progress(f.state);
    let commit = |f: &RepoFacts| match f.selection {
        Selection::Commit { .. } => None,
        Selection::WorkingDir => Some(WORKING_DIR_SELECTED),
        Selection::Stash => Some(STASH_SELECTED),
        Selection::None => Some(NO_COMMIT_SELECTED),
    };
    let changes = f.staged + f.unstaged + f.conflicted;
    match action {
        // Diff view.
        "ShowWholeFile" | "NextFile" | "PrevFile" | "NextHunk" | "PrevHunk" | "FileList" => {
            check(!f.diff_file, NO_FILE)
        }
        // Repository.
        "WriteCommitGraph" | "GoToCommit" | "GoToHead" | "MarkCompare" | "ToggleAmend" => {
            check(f.unborn, UNBORN)
        }
        "AddWorktree" => check(f.unborn, UNBORN),
        "MoveThisWorktree" | "RemoveThisWorktree" => check(!f.linked_worktree, MAIN_WORKTREE),
        "RepairWorktrees" => check(f.linked_worktrees == 0, NO_LINKED_WORKTREES),
        // Paths this file system can't hold (`fs_paths`).
        "FsPaths" => check(f.fs_paths + f.fs_hidden == 0, NO_FS_PATHS),
        "HideFsPaths" => check(f.fs_paths == 0, NO_FS_PATHS).or(check(f.fs_hidable == 0, NO_FS_PATH_CHANGED)),
        "ShowHiddenFsPaths" => check(f.fs_hidden == 0, NO_HIDDEN_PATHS),
        "PruneWorktrees" => check(f.prunable_worktrees == 0, NOTHING_TO_PRUNE),
        // `git add --renormalize` would mark them resolved (`renormalize::CONFLICTS`).
        "RenormalizeLineEndings" => check(f.conflicted > 0, UNRESOLVED),
        "UpdateSubmodules" | "UpdateSubmodulesRemote" | "CheckoutSubmoduleCommit"
        | "SyncSubmodules" | "RemoveSubmodule" | "ForeachSubmodule" => {
            check(f.submodules == 0, NO_SUBMODULES)
        }
        // Branch.
        "CreateBranch" | "CreateTag" => check(f.unborn, UNBORN),
        "MergeBranch" => check(f.unborn, UNBORN).or(busy_op),
        "RebaseCurrentOnto" => check(f.unborn, UNBORN)
            .or(check(f.detached, DETACHED))
            .or(busy_op),
        "RenameCurrentBranch" => check(f.unborn, UNBORN).or(check(f.detached, DETACHED)),
        "SetUpstreamMenu" => check(f.unborn, UNBORN)
            .or(check(f.detached, DETACHED))
            .or(check(f.remotes == 0, NO_REMOTES)),
        "DeleteBranchMenu" | "DeleteBranches" | "CleanUpMergedBranches" => {
            check(f.other_branches == 0, NO_OTHER_BRANCH)
        }
        // Commit.
        "StageAllAndCommit" => check(changes == 0, NO_CHANGES),
        "StageTrackedAndCommit" => check(
            f.staged + f.tracked_unstaged + f.conflicted == 0,
            NO_TRACKED_CHANGES,
        ),
        "StageAll" => check(f.unstaged + f.conflicted == 0, NOTHING_TO_STAGE),
        "UnstageAll" => check(f.staged == 0, NOTHING_STAGED),
        "DiscardAll" => check(f.unstaged == 0, NO_UNSTAGED),
        "RevertCommit" | "EditCommitMessage" => commit(f).or(busy_op),
        "ShowContaining" | "RestoreFromCommit" => commit(f),
        "CherryPickCommit" => commit(f)
            .or(check(f.selection == Selection::Commit { head: true }, IS_HEAD))
            .or(busy_op),
        "CherryPickMarked" | "RevertMarked" => {
            use crate::pick_several::{SeveralKind, why_not};
            let kind = if action == "RevertMarked" { SeveralKind::Revert } else { SeveralKind::CherryPick };
            why_not(kind, f.marked_commits, f.head_marked, f.state)
        }
        "RevertLinesInWorkingTree" | "ApplyLinesToWorkingTree" => check(!f.commit_diff, NO_COMMIT_DIFF),
        "OpenThisVersion" => check(!f.this_version, NO_COMMIT_FILE),
        "OpenOldVersion" => {
            check(!f.this_version && !f.old_version, NO_COMMIT_FILE).or(check(!f.old_version, NO_OLD_VERSION))
        }
        "OpenExternalDiffTool" => check(f.cursor_file == CursorFile::None, NO_FILE)
            .or(check(f.cursor_file == CursorFile::Conflicted, CONFLICTED_FILE)),
        "OpenExternalMergeTool" => check(f.conflicted == 0, NO_CONFLICTS)
            .or(check(f.cursor_file != CursorFile::Conflicted, NO_CONFLICTED_FILE)),
        "SwapCompare" | "EndCompare" => check(!f.comparing, NOT_COMPARING),
        "GetSigningKeys" => {
            check(!f.github_remote, NO_GITHUB_REMOTE).or(check(!f.signing_keys, NO_KEY_MISSING))
        }
        // Stash.
        // `git stash` would end a merge / cherry-pick half-done (`stash::refusal_during`).
        "StashChanges" | "StashIncludingUntracked" | "StashSelection" => check(f.unborn, UNBORN)
            .or(check(changes == 0, NO_CHANGES))
            .or(busy_op),
        "ApplyStash" | "PopStash" | "ApplyStashIndex" | "PopStashIndex" | "DropLatestStash"
        | "DropStash" | "RenameStash" => {
            check(f.stashes == 0, NO_STASHES)
        }
        // Remote.
        "Fetch" | "FetchFrom" => check(f.remotes == 0, NO_REMOTES),
        "Pull" | "PullFrom" | "PullWith" => check(f.remotes == 0, NO_REMOTES)
            .or(check(f.detached, DETACHED))
            .or(busy_op),
        "ResetRemoteBranch" => check(f.remotes == 0, NO_REMOTES).or(check(f.unborn, UNBORN)),
        "Push" | "PushTo" => check(f.remotes == 0, NO_REMOTES)
            .or(check(f.unborn, UNBORN))
            .or(check(f.detached, DETACHED)),
        _ => None,
    }
}

/// Every menu command that can't act in state `f`, with its reason (table order).
pub fn unavailable(f: &RepoFacts) -> Vec<(&'static str, &'static str)> {
    crate::palette::MENU_COMMANDS
        .iter()
        .filter_map(|c| why_not(c.action, f).map(|why| (c.action, why)))
        .collect()
}

/// What to do about reason `why`, where there is a next step to name ("select a commit
/// first", "Add Remote… adds one").
pub fn what_to_do(why: &str) -> Option<&'static str> {
    match why {
        WORKING_DIR_SELECTED | NO_COMMIT_SELECTED => Some("select a commit first"),
        STASH_SELECTED => Some("the Stash menu acts on stashes"),
        NO_COMMIT_FILE => Some("the working directory's files open with Open in Editor"),
        NO_COMMIT_DIFF => Some("select a commit or a stash first"),
        CONFLICTED_FILE => Some("Open in External Merge Tool resolves it"),
        NO_CONFLICTED_FILE => Some("go to a conflicted file in the working directory first"),
        MAIN_WORKTREE => Some("linked worktrees are moved or removed from the WORKTREES section"),
        NO_SUBMODULES => Some("Add Submodule… adds one"),
        NO_REMOTES => Some("Add Remote… adds one"),
        UNBORN => Some("commit something first"),
        DETACHED => Some("switch to a branch first"),
        NO_KEY_MISSING => Some("select a signed commit git can't verify yet"),
        UNRESOLVED => Some("resolve them first"),
        NO_MARKED_COMMITS => Some("mark them first: Mark for Cherry-pick / Revert in a commit's menu"),
        HEAD_MARKED => Some("unmark it: it is already the current commit"),
        _ if why.ends_with("in progress") => Some("continue or abort it first"),
        _ => None,
    }
}

/// What an app says when an unavailable command is run anyway (a key, the palette's ⏎):
/// "Pop Latest Stash is not available: no stashes", with [`what_to_do`] after a dash.
/// `label` is the command's menu or palette label ("…" and a trailing state are dropped).
pub fn refusal(label: &str, why: &str) -> String {
    let mut name = label.replace('…', "");
    for cut in [" (", ": "] {
        if let Some(i) = name.find(cut) {
            name.truncate(i);
        }
    }
    match what_to_do(why) {
        Some(next) => format!("{} is not available: {why} — {next}", name.trim()),
        None => format!("{} is not available: {why}", name.trim()),
    }
}

/// A menu label with its reason, as a greyed entry shows it: "Pop Latest Stash (no
/// stashes)" — in the form the menu tests reduce away ([`crate::palette::normalize`]).
pub fn with_reason(label: &str, why: &str) -> String {
    format!("{label} ({why})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{MENU_COMMANDS, normalize};

    /// A repository with one commit on `main`, a remote with upstream, nothing else.
    fn plain() -> RepoFacts {
        RepoFacts {
            repo: true,
            tabs: 1,
            remotes: 1,
            selection: Selection::WorkingDir,
            ..Default::default()
        }
    }

    fn off(f: &RepoFacts) -> Vec<&'static str> {
        unavailable(f).into_iter().map(|(a, _)| a).collect()
    }

    #[gitti_test::test]
    fn every_rule_names_a_menu_command() {
        // A state where every rule fires: each rule's command must be in the table (a
        // typo would silently never grey anything).
        let all_off = RepoFacts {
            repo: true,
            unborn: true,
            detached: true,
            state: RepoState::Merging,
            selection: Selection::None,
            ..Default::default()
        };
        let names: Vec<&str> = MENU_COMMANDS.iter().map(|c| c.action).collect();
        for a in off(&all_off) {
            assert!(names.contains(&a), "{a}");
        }
        // No repository: everything but the app-wide commands.
        let none = RepoFacts::default();
        for c in MENU_COMMANDS {
            let free = WITHOUT_REPO.contains(&c.action);
            let why = why_not(c.action, &none);
            match c.action {
                "CloseTab" | "NextTab" | "PrevTab" => assert_eq!(why, Some(NO_TAB), "{}", c.action),
                _ if free => assert_eq!(why, None, "{}", c.action),
                _ => assert_eq!(why, Some(NO_REPO), "{}", c.action),
            }
        }
        for a in WITHOUT_REPO {
            assert!(names.contains(a), "WITHOUT_REPO: {a}");
        }
    }

    #[gitti_test::test]
    fn a_plain_repository_offers_the_everyday_commands() {
        let f = plain();
        let off = off(&f);
        for a in [
            "Commit", "Fetch", "Pull", "Push", "PushTo", "CreateBranch", "CreateTag",
            "SwitchBranch", "MergeBranch", "RebaseCurrentOnto", "RenameCurrentBranch",
            "SetUpstreamMenu", "AddWorktree", "AddSubmodule", "AddRemote", "CreateEmptyCommit",
            "ToggleAmend", "GoToHead", "Refresh",
        ] {
            assert!(!off.contains(&a), "{a}: {:?}", why_not(a, &f));
        }
        // Clean, no stash, no submodule, the main worktree, the working directory row.
        for (a, why) in [
            ("StageAll", NOTHING_TO_STAGE),
            ("UnstageAll", NOTHING_STAGED),
            ("DiscardAll", NO_UNSTAGED),
            ("StashChanges", NO_CHANGES),
            ("StageAllAndCommit", NO_CHANGES),
            ("StageTrackedAndCommit", NO_TRACKED_CHANGES),
            ("PopStash", NO_STASHES),
            ("DropStash", NO_STASHES),
            ("UpdateSubmodules", NO_SUBMODULES),
            ("RemoveThisWorktree", MAIN_WORKTREE),
            ("PruneWorktrees", NOTHING_TO_PRUNE),
            ("RevertCommit", WORKING_DIR_SELECTED),
            ("EndCompare", NOT_COMPARING),
            ("NextTab", ONE_TAB),
            ("DeleteBranchMenu", NO_OTHER_BRANCH),
            ("DeleteBranches", NO_OTHER_BRANCH),
            ("CleanUpMergedBranches", NO_OTHER_BRANCH),
            ("ShowWholeFile", NO_FILE),
            ("RevertLinesInWorkingTree", NO_COMMIT_DIFF),
            ("ApplyLinesToWorkingTree", NO_COMMIT_DIFF),
        ] {
            assert_eq!(why_not(a, &f), Some(why), "{a}");
        }
    }

    /// Workflow: CP-17
    #[gitti_test::test]
    fn changes_stashes_and_selection_turn_them_on() {
        let f = RepoFacts {
            staged: 1,
            unstaged: 2,
            stashes: 1,
            submodules: 1,
            linked_worktree: true,
            linked_worktrees: 1,
            prunable_worktrees: 1,
            other_branches: 1,
            comparing: true,
            diff_file: true,
            tabs: 2,
            selection: Selection::Commit { head: false },
            this_version: true,
            old_version: true,
            cursor_file: CursorFile::Changed,
            commit_diff: true,
            github_remote: true,
            signing_keys: true,
            fs_paths: 2,
            fs_hidable: 1,
            fs_hidden: 1,
            marked_commits: 2,
            ..plain()
        };
        // (The merge tool needs a conflicted file at the cursor: `external_tools_…`.)
        assert_eq!(why_not("OpenExternalMergeTool", &f), Some(NO_CONFLICTS));
        let unavailable = |f: &RepoFacts| {
            let mut v = unavailable(f);
            v.retain(|(a, _)| *a != "OpenExternalMergeTool");
            v
        };
        let off = |f: &RepoFacts| unavailable(f).into_iter().map(|(a, _)| a).collect::<Vec<_>>();
        assert_eq!(off(&f), Vec::<&str>::new(), "{:?}", unavailable(&f));
        // Only staged changes: nothing to stage, the rest goes.
        let staged_only = RepoFacts { unstaged: 0, ..f.clone() };
        assert_eq!(off(&staged_only), ["StageAll", "DiscardAll"]);
        // HEAD selected: no cherry-pick of it.
        let head = RepoFacts { selection: Selection::Commit { head: true }, ..f.clone() };
        assert_eq!(unavailable(&head), [("CherryPickCommit", IS_HEAD)]);
        // A root commit: no old version.
        let root = RepoFacts { old_version: false, ..f.clone() };
        assert_eq!(unavailable(&root), [("OpenOldVersion", NO_OLD_VERSION)]);
        // Only for a GitHub repository's commit whose signing key is missing.
        let verified = RepoFacts { signing_keys: false, ..f.clone() };
        assert_eq!(unavailable(&verified), [("GetSigningKeys", NO_KEY_MISSING)]);
        let elsewhere = RepoFacts { github_remote: false, signing_keys: false, ..f.clone() };
        assert_eq!(why_not("GetSigningKeys", &elsewhere), Some(NO_GITHUB_REMOTE));
        // Marked commits: HEAD among them can't be picked (reverted it can); none marked.
        let head_marked = RepoFacts { head_marked: true, ..f.clone() };
        assert_eq!(unavailable(&head_marked), [("CherryPickMarked", HEAD_MARKED)]);
        let none = RepoFacts { marked_commits: 0, ..f.clone() };
        assert_eq!(off(&none), ["CherryPickMarked", "RevertMarked"]);
        assert_eq!(why_not("RevertMarked", &none), Some(NO_MARKED_COMMITS));
        let stash = RepoFacts { selection: Selection::Stash, ..f };
        assert_eq!(why_not("RevertCommit", &stash), Some(STASH_SELECTED));
    }

    #[gitti_test::test]
    fn external_tools_follow_the_file_at_the_cursor() {
        let tools = |f: &RepoFacts| {
            ["OpenExternalDiffTool", "OpenExternalMergeTool"].map(|a| why_not(a, f))
        };
        assert_eq!(tools(&plain()), [Some(NO_FILE), Some(NO_CONFLICTS)]);
        let changed = RepoFacts { cursor_file: CursorFile::Changed, ..plain() };
        assert_eq!(tools(&changed), [None, Some(NO_CONFLICTS)]);
        let elsewhere = RepoFacts { conflicted: 1, ..changed };
        assert_eq!(tools(&elsewhere), [None, Some(NO_CONFLICTED_FILE)]);
        let conflicted = RepoFacts { cursor_file: CursorFile::Conflicted, ..elsewhere };
        assert_eq!(tools(&conflicted), [Some(CONFLICTED_FILE), None]);
        let msg = refusal("Open in External Diff Tool", CONFLICTED_FILE);
        assert!(msg.ends_with("— Open in External Merge Tool resolves it"), "{msg}");
        // Choosing the tool needs a repository only.
        assert_eq!(why_not("ChooseExternalTool", &plain()), None);
    }

    #[gitti_test::test]
    fn fresh_repository_detached_head_no_remotes_and_a_merge() {
        // `git init`: nothing to branch, tag, push or go to.
        let fresh = RepoFacts { unborn: true, remotes: 0, ..plain() };
        for (a, why) in [
            ("CreateBranch", UNBORN),
            ("CreateTag", UNBORN),
            ("GoToHead", UNBORN),
            ("AddWorktree", UNBORN),
            ("RenameCurrentBranch", UNBORN),
            ("Fetch", NO_REMOTES),
            ("Push", NO_REMOTES),
        ] {
            assert_eq!(why_not(a, &fresh), Some(why), "{a}");
        }
        assert_eq!(why_not("Commit", &fresh), None);
        assert_eq!(why_not("AddRemote", &fresh), None);
        let detached = RepoFacts { detached: true, ..plain() };
        for a in ["RenameCurrentBranch", "SetUpstreamMenu", "RebaseCurrentOnto", "Pull", "Push"] {
            assert_eq!(why_not(a, &detached), Some(DETACHED), "{a}");
        }
        assert_eq!(why_not("Fetch", &detached), None);
        assert_eq!(why_not("CreateBranch", &detached), None);
        let merging = RepoFacts {
            state: RepoState::Merging,
            selection: Selection::Commit { head: false },
            ..plain()
        };
        let merging = RepoFacts { marked_commits: 2, ..merging };
        for a in [
            "MergeBranch", "RebaseCurrentOnto", "CherryPickCommit", "RevertCommit", "Pull", "CherryPickMarked",
            "RevertMarked",
        ] {
            assert_eq!(why_not(a, &merging), Some("a merge is in progress"), "{a}");
        }
        // A stash would end the operation half-done.
        let busy = |state| RepoFacts { state, staged: 1, unstaged: 1, ..plain() };
        for (state, why) in [
            (RepoState::Merging, "a merge is in progress"),
            (RepoState::CherryPicking, "a cherry-pick is in progress"),
            (RepoState::Reverting, "a revert is in progress"),
            (RepoState::Rebasing, "a rebase is in progress"),
        ] {
            for a in ["StashChanges", "StashIncludingUntracked", "StashSelection"] {
                assert_eq!(why_not(a, &busy(state)), Some(why), "{a}");
            }
            assert_eq!(why_not("UnstageAll", &busy(state)), None, "keeps the operation");
        }
        // Committing concludes the merge.
        assert_eq!(why_not("Commit", &merging), None);
    }

    #[gitti_test::test]
    fn paths_this_file_system_cant_hold() {
        let fs = |fs_paths, fs_hidable, fs_hidden| RepoFacts { fs_paths, fs_hidable, fs_hidden, ..plain() };
        let all = ["FsPaths", "HideFsPaths", "ShowHiddenFsPaths"];
        let why = |f: &RepoFacts| all.map(|a| why_not(a, f));
        // None at all: nothing to explain, hide or show.
        assert_eq!(why(&fs(0, 0, 0)), [Some(NO_FS_PATHS), Some(NO_FS_PATHS), Some(NO_HIDDEN_PATHS)]);
        // In the way, one shows as changed.
        assert_eq!(why(&fs(2, 1, 0)), [None, None, Some(NO_HIDDEN_PATHS)]);
        // In the way, none changed (e.g. both of a pair hold the same content).
        assert_eq!(why(&fs(2, 0, 0)), [None, Some(NO_FS_PATH_CHANGED), Some(NO_HIDDEN_PATHS)]);
        // All hidden: explained, shown again.
        assert_eq!(why(&fs(0, 0, 1)), [None, Some(NO_FS_PATHS), None]);
        for a in all {
            assert!(MENU_COMMANDS.iter().any(|c| c.action == a), "{a}");
        }
    }

    #[gitti_test::test]
    fn reasons_read_as_plain_words() {
        assert_eq!(
            refusal("Pop Latest Stash", NO_STASHES),
            "Pop Latest Stash is not available: no stashes"
        );
        assert_eq!(
            refusal("Toggle Sidebar (show / hide: on)", NO_REPO),
            "Toggle Sidebar is not available: no repository open"
        );
        assert_eq!(
            refusal("Drop Stash…", NO_STASHES),
            "Drop Stash is not available: no stashes"
        );
        // With the next step where there is one.
        assert_eq!(
            refusal("Revert Selected Commit", WORKING_DIR_SELECTED),
            "Revert Selected Commit is not available: the working directory is selected — \
             select a commit first"
        );
        assert_eq!(
            refusal("Fetch", NO_REMOTES),
            "Fetch is not available: no remotes — Add Remote… adds one"
        );
        assert_eq!(
            refusal("Merge Branch into Current…", "a merge is in progress"),
            "Merge Branch into Current is not available: a merge is in progress — continue or \
             abort it first"
        );
        // The menu tests compare labels without the reason.
        let l = with_reason("Pop Latest Stash", NO_STASHES);
        assert_eq!(l, "Pop Latest Stash (no stashes)");
        assert_eq!(normalize(&l), "pop latest stash");
    }

    /// Renormalize Line Endings stages every tracked file: not while conflicts would be
    /// marked resolved by it; a merge without conflicts left is fine.
    #[gitti_test::test]
    fn renormalize_waits_for_resolved_conflicts() {
        let mut f = plain();
        assert_eq!(why_not("RenormalizeLineEndings", &f), None);
        f.state = RepoState::Merging;
        assert_eq!(why_not("RenormalizeLineEndings", &f), None);
        f.conflicted = 1;
        assert_eq!(why_not("RenormalizeLineEndings", &f), Some(UNRESOLVED));
        assert_eq!(
            refusal(crate::renormalize::MENU_LABEL, UNRESOLVED),
            "Renormalize Line Endings is not available: conflicted files are not resolved yet — \
             resolve them first"
        );
    }
}
