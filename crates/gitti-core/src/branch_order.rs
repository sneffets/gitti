//! The order of the sidebar's branches, in gitti and gitty alike: local branches by name,
//! by the date of their last commit or by when they were last checked out; remote
//! branches by name or date. Pinned branches stay on top of their section (local
//! branches) or remote (remote branches), among themselves in the same order.
//!
//! Everything here is in-process and cheap: commit dates come from the loaded history
//! ([`order`] gets them through a lookup), "recently checked out" from HEAD's reflog
//! ([`recent_checkouts`], read with the refresh), pins from the settings database
//! ([`load_pins`], per repository — keyed by its common git dir, so every worktree of it
//! shares them). Navigating the sidebar runs nothing.

use serde::{Deserialize, Serialize};

use crate::{ObjectId, RefInfo, RefKind, Repo};

/// How a sidebar section sorts its branches (stored as `branch_sort` /
/// `remote_branch_sort` in `gitti.db`, shared by gitti and gitty).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchSort {
    /// Alphabetical (git's own order).
    #[default]
    Name,
    /// Newest last commit first.
    Date,
    /// The branches checked out last first (HEAD's reflog), then the others by name.
    /// Local branches only; remote branches sort by name with it.
    Recent,
}

impl BranchSort {
    /// The choices of the BRANCHES section.
    pub const LOCAL: [Self; 3] = [Self::Name, Self::Date, Self::Recent];
    /// The choices of the REMOTES section.
    pub const REMOTE: [Self; 2] = [Self::Name, Self::Date];

    /// The section menu's label ("Sort by Name").
    pub fn section_label(self) -> &'static str {
        match self {
            Self::Name => "Sort by Name",
            Self::Date => "Sort by Last Commit Date",
            Self::Recent => "Sort by Recently Checked Out",
        }
    }

    /// The menu bar's / palette's label: which section it sorts, too.
    pub fn label(self, remote: bool) -> &'static str {
        match (remote, self) {
            (false, Self::Name) => "Sort Branches by Name",
            (false, Self::Date) => "Sort Branches by Last Commit Date",
            (false, Self::Recent) => "Sort Branches by Recently Checked Out",
            (true, Self::Date) => "Sort Remote Branches by Last Commit Date",
            (true, _) => "Sort Remote Branches by Name",
        }
    }

    /// The palette entry, the current choice marked: "Sort Branches by Name (current)"
    /// (a state `palette::normalize` drops).
    pub fn palette_label(self, remote: bool, current: Self) -> String {
        let mark = if self == current { " (current)" } else { "" };
        format!("{}{mark}", self.label(remote))
    }

    /// What the palette entry says it does.
    pub fn description(self) -> &'static str {
        match self {
            Self::Name => "alphabetical; pinned ones on top",
            Self::Date => "newest last commit first; pinned ones on top",
            Self::Recent => "the ones you checked out last first (HEAD's reflog); pinned ones on top",
        }
    }

    /// The status message after choosing it.
    pub fn chosen_message(self, remote: bool) -> &'static str {
        match (remote, self) {
            (false, Self::Name) => "Branches: sorted by name",
            (false, Self::Date) => "Branches: newest last commit first",
            (false, Self::Recent) => "Branches: the ones checked out last first",
            (true, Self::Date) => "Remote branches: newest last commit first",
            (true, _) => "Remote branches: sorted by name",
        }
    }
}

/// The store key of the pinned branches (per repository, [`crate::store::load_repo_pref`]).
pub const PINS_KEY: &str = "pinned_branches";

/// The pinned branches of `repo` (full ref names, `refs/heads/x`, `refs/remotes/o/x`),
/// shared by all its worktrees.
pub fn load_pins(repo: &Repo) -> Vec<String> {
    crate::store::load_repo_pref(&repo.common_dir(), PINS_KEY).unwrap_or_default()
}

/// Remembers `pins` for `repo` (all its worktrees); none = forgotten.
pub fn save_pins(repo: &Repo, pins: &[String]) {
    if pins.is_empty() {
        crate::store::forget_repo_pref(&repo.common_dir(), PINS_KEY);
    } else {
        crate::store::save_repo_pref(&repo.common_dir(), PINS_KEY, &pins);
    }
}

/// Pins `full_name` (a branch's full ref name) or, when it is pinned, unpins it; whether it
/// is pinned now.
pub fn toggle_pin(pins: &mut Vec<String>, full_name: &str) -> bool {
    match pins.iter().position(|p| p == full_name) {
        Some(i) => {
            pins.remove(i);
            false
        }
        None => {
            pins.push(full_name.to_string());
            true
        }
    }
}

/// The status message after [`toggle_pin`].
pub fn pin_message(short_name: &str, pinned: bool) -> String {
    if pinned {
        format!("{short_name} is pinned to the top of its list (for this repository)")
    } else {
        format!("{short_name} is no longer pinned")
    }
}

/// How many reflog entries [`recent_checkouts`] reads at most (newest first).
const RECENT_SCAN: usize = 5000;
/// How many branch names it keeps.
const RECENT_KEEP: usize = 200;

/// The branches checked out in this worktree, most recent first, each once: from HEAD's
/// reflog (`checkout: moving from A to B`, written by `checkout` and `switch`), the target
/// and then the branch left. Detached checkouts (a commit id) are in it too; [`order`]
/// only uses names of existing branches. In-process; empty without a reflog.
pub fn recent_checkouts(repo: &gix::Repository) -> Vec<String> {
    let Ok(head) = repo.find_reference("HEAD") else {
        return Vec::new();
    };
    let mut platform = head.log_iter();
    let Ok(Some(lines)) = platform.rev() else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for line in lines.take(RECENT_SCAN).flatten() {
        let message = line.message.to_string();
        let Some((from, to)) = parse_checkout(&message) else {
            continue;
        };
        for name in [to, from] {
            if !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
        if out.len() >= RECENT_KEEP {
            break;
        }
    }
    out
}

/// `checkout: moving from A to B` → `(A, B)` (ref names have no spaces).
fn parse_checkout(message: &str) -> Option<(&str, &str)> {
    let rest = message.trim().strip_prefix("checkout: moving from ")?;
    let (from, to) = rest.split_once(" to ")?;
    (!from.contains(' ') && !to.contains(' ')).then_some((from, to))
}

impl Repo {
    /// [`recent_checkouts`] of this worktree (in-process).
    pub fn recent_checkouts(&self) -> Vec<String> {
        self.gix().map(|r| recent_checkouts(&r)).unwrap_or_default()
    }
}

/// How the sidebar sorts: the two choices, the recently checked-out branches
/// ([`recent_checkouts`]) and the pinned ones ([`load_pins`]).
#[derive(Debug, Clone, Copy)]
pub struct Sorting<'a> {
    pub local: BranchSort,
    pub remote: BranchSort,
    pub recent: &'a [String],
    pub pins: &'a [String],
}

/// The sidebar's order: indices into the refs, local branches and remote branches (all
/// remotes; a remote's group keeps this order).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchOrder {
    pub locals: Vec<usize>,
    pub remotes: Vec<usize>,
}

/// Sorts the branches of `refs` by `sorting`; `time` gives a commit's committer time
/// (`None`: not known, e.g. not loaded yet — those come after the dated ones, by name).
pub fn order(
    refs: &[RefInfo],
    sorting: &Sorting,
    time: impl Fn(&ObjectId) -> Option<i64>,
) -> BranchOrder {
    use BranchSort as Sort;
    let sorted = |kind: RefKind, sort: BranchSort| {
        let mut idx: Vec<usize> = (0..refs.len()).filter(|&i| refs[i].kind == kind).collect();
        let pinned = |i: usize| !sorting.pins.contains(&refs[i].full_name);
        match sort {
            Sort::Name => idx.sort_by(|&a, &b| {
                (pinned(a), &refs[a].short_name).cmp(&(pinned(b), &refs[b].short_name))
            }),
            Sort::Date => {
                // Newest first; unknown dates last.
                let key = |i: usize| (pinned(i), std::cmp::Reverse(time(&refs[i].target).unwrap_or(i64::MIN)));
                idx.sort_by(|&a, &b| {
                    key(a).cmp(&key(b)).then_with(|| refs[a].short_name.cmp(&refs[b].short_name))
                });
            }
            Sort::Recent => {
                let rank = |i: usize| {
                    sorting
                        .recent
                        .iter()
                        .position(|n| *n == refs[i].short_name)
                        .unwrap_or(usize::MAX)
                };
                idx.sort_by(|&a, &b| {
                    (pinned(a), rank(a), &refs[a].short_name).cmp(&(pinned(b), rank(b), &refs[b].short_name))
                });
            }
        }
        idx
    };
    let remote = match sorting.remote {
        Sort::Recent => Sort::Name,
        s => s,
    };
    BranchOrder {
        locals: sorted(RefKind::LocalBranch, sorting.local),
        remotes: sorted(RefKind::RemoteBranch, remote),
    }
}

/// [`order`] with the commit times of `history` (the loaded commits).
pub fn order_in(refs: &[RefInfo], sorting: &Sorting, history: &crate::History) -> BranchOrder {
    order(refs, sorting, |id| {
        history.row_of(id).map(|row| history.commits.commit_time(row))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(kind: RefKind, short: &str, n: u8) -> RefInfo {
        let prefix = match kind {
            RefKind::LocalBranch => "refs/heads/",
            RefKind::RemoteBranch => "refs/remotes/",
            _ => "refs/tags/",
        };
        RefInfo {
            full_name: format!("{prefix}{short}"),
            short_name: short.into(),
            kind,
            target: ObjectId::from_bytes_or_panic(&[n; 20]),
        }
    }

    fn names(refs: &[RefInfo], idx: &[usize]) -> Vec<String> {
        idx.iter().map(|&i| refs[i].short_name.clone()).collect()
    }

    #[gitti_test::test]
    fn sorts_by_name_date_and_recent_with_pins_on_top() {
        use RefKind::{LocalBranch as L, RemoteBranch as R};
        let refs = vec![
            r(L, "alpha", 1),
            r(L, "beta", 2),
            r(L, "gamma", 3),
            r(L, "old", 4),
            r(R, "origin/a", 1),
            r(R, "origin/z", 3),
            r(RefKind::Tag, "v1", 9),
        ];
        // Commit times: gamma newest, alpha oldest; `old` not loaded.
        let time = |id: &ObjectId| match id.as_bytes()[0] {
            1 => Some(10),
            2 => Some(20),
            3 => Some(30),
            _ => None,
        };
        let recent = vec!["beta".to_string(), "deadbeef".into(), "old".into()];
        let none: Vec<String> = Vec::new();
        let mut s = Sorting {
            local: BranchSort::Name,
            remote: BranchSort::Name,
            recent: &recent,
            pins: &none,
        };
        let o = order(&refs, &s, time);
        assert_eq!(names(&refs, &o.locals), ["alpha", "beta", "gamma", "old"]);
        assert_eq!(names(&refs, &o.remotes), ["origin/a", "origin/z"]);
        s.local = BranchSort::Date;
        s.remote = BranchSort::Date;
        let o = order(&refs, &s, time);
        assert_eq!(names(&refs, &o.locals), ["gamma", "beta", "alpha", "old"]);
        assert_eq!(names(&refs, &o.remotes), ["origin/z", "origin/a"]);
        // Recent: the reflog's branches first (a detached id skipped), the rest by name;
        // remote branches can't be "recent": by name.
        s.local = BranchSort::Recent;
        s.remote = BranchSort::Recent;
        let o = order(&refs, &s, time);
        assert_eq!(names(&refs, &o.locals), ["beta", "old", "alpha", "gamma"]);
        assert_eq!(names(&refs, &o.remotes), ["origin/a", "origin/z"]);
        // Pins first, in the same order among themselves.
        let pins = vec![
            "refs/heads/gamma".to_string(),
            "refs/heads/alpha".into(),
            "refs/remotes/origin/z".into(),
        ];
        s.pins = &pins;
        s.local = BranchSort::Name;
        s.remote = BranchSort::Name;
        let o = order(&refs, &s, time);
        assert_eq!(names(&refs, &o.locals), ["alpha", "gamma", "beta", "old"]);
        assert_eq!(names(&refs, &o.remotes), ["origin/z", "origin/a"]);
        s.local = BranchSort::Date;
        let o = order(&refs, &s, time);
        assert_eq!(names(&refs, &o.locals), ["gamma", "alpha", "beta", "old"]);
    }

    #[gitti_test::test]
    fn toggling_a_pin() {
        let mut pins = vec!["refs/heads/a".to_string()];
        assert!(toggle_pin(&mut pins, "refs/heads/b"));
        assert!(!toggle_pin(&mut pins, "refs/heads/a"));
        assert_eq!(pins, ["refs/heads/b"]);
    }

    #[gitti_test::test]
    fn checkout_messages() {
        assert_eq!(
            parse_checkout("checkout: moving from main to feature/x"),
            Some(("main", "feature/x"))
        );
        assert_eq!(parse_checkout("commit: two"), None);
        assert_eq!(parse_checkout("rebase (finish): returning to refs/heads/x"), None);
    }

    #[gitti_test::test]
    fn labels_and_storage() {
        assert_eq!(
            BranchSort::Date.palette_label(false, BranchSort::Date),
            "Sort Branches by Last Commit Date (current)"
        );
        assert_eq!(BranchSort::Name.palette_label(true, BranchSort::Date), "Sort Remote Branches by Name");
        for (s, j) in [
            (BranchSort::Name, "\"name\""),
            (BranchSort::Date, "\"date\""),
            (BranchSort::Recent, "\"recent\""),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), j);
        }
    }
}
