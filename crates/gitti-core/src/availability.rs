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
    /// A remote points to github.com.
    pub github_remote: bool,
    /// The shown commit's signature can't be checked for a missing key that GitHub may
    /// have (`signing_keys::offer`).
    pub signing_keys: bool,
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
            github_remote: false,
            signing_keys: false,
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
pub const NO_OLD_VERSION: &str = "the commit has no parent";
pub const NO_OTHER_BRANCH: &str = "no other local branch";
pub const NO_GITHUB_REMOTE: &str = "no GitHub remote";
pub const NO_KEY_MISSING: &str = "the shown commit has no signature whose key is missing";

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
        "PruneWorktrees" => check(f.prunable_worktrees == 0, NOTHING_TO_PRUNE),
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
        "DeleteBranchMenu" => check(f.other_branches == 0, NO_OTHER_BRANCH),
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
        "OpenThisVersion" => check(!f.this_version, NO_COMMIT_FILE),
        "OpenOldVersion" => {
            check(!f.this_version && !f.old_version, NO_COMMIT_FILE).or(check(!f.old_version, NO_OLD_VERSION))
        }
        "SwapCompare" | "EndCompare" => check(!f.comparing, NOT_COMPARING),
        "GetSigningKeys" => {
            check(!f.github_remote, NO_GITHUB_REMOTE).or(check(!f.signing_keys, NO_KEY_MISSING))
        }
        // Stash.
        "StashChanges" | "StashIncludingUntracked" => {
            check(f.unborn, UNBORN).or(check(changes == 0, NO_CHANGES))
        }
        "ApplyStash" | "PopStash" | "DropLatestStash" | "DropStash" => {
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
        MAIN_WORKTREE => Some("linked worktrees are moved or removed from the WORKTREES section"),
        NO_SUBMODULES => Some("Add Submodule… adds one"),
        NO_REMOTES => Some("Add Remote… adds one"),
        UNBORN => Some("commit something first"),
        DETACHED => Some("switch to a branch first"),
        NO_KEY_MISSING => Some("select a signed commit git can't verify yet"),
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
            ("ShowWholeFile", NO_FILE),
        ] {
            assert_eq!(why_not(a, &f), Some(why), "{a}");
        }
    }

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
            github_remote: true,
            signing_keys: true,
            ..plain()
        };
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
        let stash = RepoFacts { selection: Selection::Stash, ..f };
        assert_eq!(why_not("RevertCommit", &stash), Some(STASH_SELECTED));
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
        for a in ["MergeBranch", "RebaseCurrentOnto", "CherryPickCommit", "RevertCommit", "Pull"] {
            assert_eq!(why_not(a, &merging), Some("a merge is in progress"), "{a}");
        }
        // Committing concludes the merge.
        assert_eq!(why_not("Commit", &merging), None);
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
}
