//! Cherry-pick or revert several commits at once — the marked commits of the commit list
//! ([`crate::commit_marks`]) — as **one** git sequence (`git cherry-pick A B C`,
//! `git revert C B A`), so a stop on the way is git's own: the operation banner's
//! Continue goes on with the rest, Skip leaves the stopped commit out, Abort puts the
//! branch back where it was before the first one.
//!
//! Order: a cherry-pick applies the commits **oldest first** (the order they were made
//! in, so a commit comes after the ones it builds on); a revert undoes them **newest
//! first** (later changes are undone before the ones they build on). The commit list has
//! children above their parents, so its rows give both orders without asking git.
//!
//! The dialog ([`SeveralPlan::describe`]) says the order, what gets written where, and
//! what to do on a stop; commits a pick would find already in the branch (or a revert
//! not in it) are named in the list.

use anyhow::Result;

use crate::Repo;
use crate::commit_marks::CommitMarks;
use crate::history::History;
use crate::local_changes::BlockedOp;
use crate::ops::RepoState;

/// Cherry-pick or revert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeveralKind {
    CherryPick,
    Revert,
}

impl SeveralKind {
    /// The menu entry for `n` marked commits ("Cherry-pick 3 Marked Commits…"); the menu
    /// bar's and palette's entry is [`Self::menu_label`].
    pub fn menu_label_n(self, n: usize) -> String {
        format!("{} {n} Marked {}…", self.verb(), if n == 1 { "Commit" } else { "Commits" })
    }

    /// The menu bar's / palette's entry.
    pub fn menu_label(self) -> &'static str {
        match self {
            SeveralKind::CherryPick => CHERRY_PICK_MARKED,
            SeveralKind::Revert => REVERT_MARKED,
        }
    }

    /// "Cherry-pick" / "Revert" (titles, the busy label).
    pub fn verb(self) -> &'static str {
        match self {
            SeveralKind::CherryPick => "Cherry-pick",
            SeveralKind::Revert => "Revert",
        }
    }
}

/// The menu bar's / palette's entries (both apps).
pub const CHERRY_PICK_MARKED: &str = "Cherry-pick Marked Commits…";
pub const REVERT_MARKED: &str = "Revert Marked Commits…";

/// The `-x` checkbox of the cherry-pick dialog.
pub const RECORD_ORIGIN_LABEL: &str = "-x — append \"(cherry picked from commit …)\" to each message";

/// One commit of a [`SeveralPlan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeveralCommit {
    /// Full id.
    pub id: String,
    pub subject: String,
    /// A merge commit: picked / reverted against its first parent (`-m 1`).
    pub is_merge: bool,
    /// A pick: the commit is already in the branch (picking it changes nothing); a revert:
    /// it is not in the branch's history.
    pub odd: bool,
}

impl SeveralCommit {
    pub fn short(&self) -> &str {
        &self.id[..7.min(self.id.len())]
    }
}

/// What Cherry-pick / Revert Marked Commits… does: the commits in the order git applies
/// them, onto which branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeveralPlan {
    pub kind: SeveralKind,
    /// In the order git applies them: oldest first for a pick, newest first for a revert.
    pub commits: Vec<SeveralCommit>,
    /// The checked-out branch, or "HEAD" when detached.
    pub onto: String,
    /// `-x` (a pick only).
    pub record_origin: bool,
}

/// The plan for the marked commits of `history` (in-process, no git): `None` when nothing
/// is marked. `head_branch`: the checked-out branch (`None`: detached).
pub fn plan(kind: SeveralKind, history: &History, marks: &CommitMarks, head_branch: Option<&str>) -> Option<SeveralPlan> {
    let mut rows = marks.rows(history);
    if rows.is_empty() {
        return None;
    }
    // The rows are newest first: a pick goes the other way.
    if kind == SeveralKind::CherryPick {
        rows.reverse();
    }
    let commits = rows
        .into_iter()
        .map(|row| {
            let c = history.commits.at(row);
            let in_head = history.in_head_history(&c.id);
            SeveralCommit {
                id: c.id.to_string(),
                subject: history.meta(row).summary.clone(),
                is_merge: c.parents.len() > 1,
                odd: match kind {
                    SeveralKind::CherryPick => in_head == Some(true),
                    SeveralKind::Revert => in_head == Some(false),
                },
            }
        })
        .collect();
    Some(SeveralPlan {
        kind,
        commits,
        onto: head_branch.unwrap_or("HEAD").to_string(),
        record_origin: false,
    })
}

fn commits_word(n: usize) -> String {
    if n == 1 { "1 commit".to_string() } else { format!("{n} commits") }
}

impl SeveralPlan {
    /// The dialog's title: "Cherry-pick 3 Commits onto main", "Revert 3 Commits on main".
    pub fn title(&self) -> String {
        let n = self.commits.len();
        let what = if n == 1 { "1 Commit".to_string() } else { format!("{n} Commits") };
        match self.kind {
            SeveralKind::CherryPick => format!("Cherry-pick {what} onto {}", self.onto),
            SeveralKind::Revert => format!("Revert {what} on {}", self.onto),
        }
    }

    /// The dialog's button: "Cherry-pick 3 Commits", "Revert 3 Commits".
    pub fn ok_label(&self) -> String {
        let n = self.commits.len();
        let what = if n == 1 { "1 Commit".to_string() } else { format!("{n} Commits") };
        format!("{} {what}", self.kind.verb())
    }

    /// The commits as the dialog lists them, in the order git applies them:
    /// "1. abc1234 Subject (already in main)".
    pub fn list_lines(&self) -> Vec<String> {
        self.commits
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut line = format!("{}. {} {}", i + 1, c.short(), c.subject);
                if c.is_merge {
                    line.push_str(" (merge)");
                }
                if c.odd {
                    line.push_str(&match self.kind {
                        SeveralKind::CherryPick => format!(" (already in {})", self.onto),
                        SeveralKind::Revert => format!(" (not in {})", self.onto),
                    });
                }
                line
            })
            .collect()
    }

    /// The dialog's text: the order, what gets written where, the commits, and what to do
    /// when git stops on the way.
    pub fn describe(&self) -> String {
        let n = self.commits.len();
        let onto = &self.onto;
        let mut text = match self.kind {
            SeveralKind::CherryPick => format!(
                "Applies the changes of {} to {onto}, oldest first (the order they were made \
                 in) — one new commit on {onto} each, with its author and message:",
                if n == 1 { "this commit".to_string() } else { format!("these {n} commits") }
            ),
            SeveralKind::Revert => format!(
                "Undoes {} on {onto}, newest first (later changes are undone before the ones \
                 they build on) — one new commit \"Revert …\" on {onto} each:",
                if n == 1 { "this commit".to_string() } else { format!("these {n} commits") }
            ),
        };
        text.push_str("\n\n");
        text.push_str(&self.list_lines().join("\n"));
        text.push_str("\n\n");
        let cmd = match self.kind {
            SeveralKind::CherryPick => "git cherry-pick",
            SeveralKind::Revert => "git revert",
        };
        text.push_str(&format!(
            "{cmd} runs them as one sequence; the new commits are local — nothing is \
             pushed. If a commit conflicts, git stops there: resolve \
             its files, then Continue in the banner and git goes on with the rest; Skip leaves \
             that commit out; Abort puts {onto} back where it was before the first one."
        ));
        let odd = self.commits.iter().filter(|c| c.odd).count();
        if odd > 0 {
            text.push_str("\n\n");
            text.push_str(&match self.kind {
                SeveralKind::CherryPick => format!(
                    "{} already in {onto}: picking it changes nothing, so git stops there with \
                     nothing to commit — Skip it then.",
                    if odd == 1 { "1 of them is".to_string() } else { format!("{odd} of them are") }
                ),
                SeveralKind::Revert => format!(
                    "{} not in {onto}'s history: undoing it here may conflict or change nothing \
                     (then Skip it).",
                    if odd == 1 { "1 of them is".to_string() } else { format!("{odd} of them are") }
                ),
            });
        }
        if self.commits.iter().any(|c| c.is_merge) {
            text.push_str("\n\nMerge commits are taken against their first parent (-m 1).");
        }
        text
    }

    /// The git command line (without `git`).
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = match self.kind {
            SeveralKind::CherryPick => vec!["cherry-pick".into()],
            SeveralKind::Revert => vec!["revert".into(), "--no-edit".into()],
        };
        // `-m 1` applies to the whole sequence; git takes it for ordinary commits too.
        if self.commits.iter().any(|c| c.is_merge) {
            a.extend(["-m".into(), "1".into()]);
        }
        if self.record_origin && self.kind == SeveralKind::CherryPick {
            a.push("-x".into());
        }
        a.extend(self.commits.iter().map(|c| c.id.clone()));
        a
    }

    /// The status line once every commit went through.
    pub fn done_line(&self) -> String {
        let n = commits_word(self.commits.len());
        match self.kind {
            SeveralKind::CherryPick => format!("Picked {n} onto {}, oldest first.", self.onto),
            SeveralKind::Revert => format!("Reverted {n} on {}, newest first.", self.onto),
        }
    }

    /// The status line when git stopped on `conflicts` conflicted files with `left`
    /// commits still in its to-do list (the stopped one included).
    pub fn stop_line(&self, conflicts: usize, left: Option<usize>) -> String {
        let total = self.commits.len();
        let n = if conflicts == 1 { "1 conflict".to_string() } else { format!("{conflicts} conflicts") };
        let them = if conflicts == 1 { "it" } else { "them" };
        let at = match left {
            Some(left) if left >= 1 && left <= total => {
                let i = total - left;
                format!(" at commit {} of {total} ({})", i + 1, self.commits[i].short())
            }
            _ => String::new(),
        };
        let rest = match left {
            Some(l) if l > 1 => format!(" — Continue goes on with the {} after it", commits_word(l - 1)),
            _ => String::new(),
        };
        format!(
            "{} stopped{at} on {n} — resolve {them}, then Continue (or Skip / Abort){rest}.",
            self.kind.verb()
        )
    }
}

/// Why Cherry-pick / Revert Marked Commits… can't run: nothing marked, HEAD marked (a
/// pick), an operation in progress.
pub fn why_not(kind: SeveralKind, marked: usize, head_marked: bool, state: RepoState) -> Option<&'static str> {
    if marked == 0 {
        return Some(crate::availability::NO_MARKED_COMMITS);
    }
    if kind == SeveralKind::CherryPick && head_marked {
        return Some(crate::availability::HEAD_MARKED);
    }
    crate::availability::in_progress(state)
}

impl Repo {
    /// Runs `plan` as one `git cherry-pick` / `git revert` sequence. The result is
    /// [`SeveralPlan::done_line`]; a stop on conflicts is a result too
    /// ([`SeveralPlan::stop_line`]); a stop with nothing to commit is an
    /// [`crate::skip::EmptyPick`], a refusal because of uncommitted changes a
    /// [`crate::local_changes::LocalChanges`] (its ways out run the plan again).
    pub fn pick_several(&self, plan: &SeveralPlan) -> Result<String> {
        anyhow::ensure!(!plan.commits.is_empty(), "No commits to {}", plan.kind.verb().to_lowercase());
        let args = plan.args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let unmerged = || self.unmerged_index_paths().map_or(0, |u| u.len());
        let before = unmerged();
        let head = self.head().ok().and_then(|h| h.id);
        let result = {
            let out = self.empty_pick_as(self.noting_fs_paths(|| {
                self.git().run(&args)?;
                Ok(plan.done_line())
            }));
            self.local_changes_as(out, || BlockedOp::Several(plan.clone()))
        };
        if result.is_ok() || before > 0 {
            return result;
        }
        match unmerged() {
            // One revert whose changes are undone already: git leaves no state.
            0 if plan.kind == SeveralKind::Revert && plan.commits.len() == 1 => {
                self.nothing_to_revert_as(result, &plan.commits[0].id, head)
            }
            0 => result,
            n => Ok(plan.stop_line(n, self.sequence_left())),
        }
    }

    /// The commits left in the to-do list of a stopped cherry-pick / revert sequence, the
    /// stopped one included (`sequencer/todo`; `None`: no sequence).
    pub fn sequence_left(&self) -> Option<usize> {
        let todo = std::fs::read_to_string(self.git_dir()?.join("sequencer").join("todo")).ok()?;
        Some(
            todo.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .count(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, subject: &str, is_merge: bool, odd: bool) -> SeveralCommit {
        SeveralCommit { id: id.repeat(40 / id.len()), subject: subject.into(), is_merge, odd }
    }

    #[gitti_test::test]
    fn the_dialog_says_the_order_the_commits_and_a_stop() {
        let mut p = SeveralPlan {
            kind: SeveralKind::CherryPick,
            commits: vec![commit("a", "One", false, false), commit("b", "Two", true, true)],
            onto: "main".into(),
            record_origin: true,
        };
        assert_eq!(p.title(), "Cherry-pick 2 Commits onto main");
        assert_eq!(p.ok_label(), "Cherry-pick 2 Commits");
        assert_eq!(p.list_lines(), ["1. aaaaaaa One", "2. bbbbbbb Two (merge) (already in main)"]);
        let d = p.describe();
        assert!(d.contains("oldest first") && d.contains("Continue in the banner"), "{d}");
        assert!(d.contains("1 of them is already in main") && d.contains("-m 1"), "{d}");
        assert_eq!(p.args()[..4], ["cherry-pick", "-m", "1", "-x"]);
        assert_eq!(p.done_line(), "Picked 2 commits onto main, oldest first.");
        assert_eq!(
            p.stop_line(1, Some(2)),
            "Cherry-pick stopped at commit 1 of 2 (aaaaaaa) on 1 conflict — resolve it, then \
             Continue (or Skip / Abort) — Continue goes on with the 1 commit after it."
        );
        assert_eq!(
            p.stop_line(2, Some(1)),
            "Cherry-pick stopped at commit 2 of 2 (bbbbbbb) on 2 conflicts — resolve them, then \
             Continue (or Skip / Abort)."
        );
        p.kind = SeveralKind::Revert;
        p.commits[1].is_merge = false;
        assert_eq!(p.title(), "Revert 2 Commits on main");
        assert_eq!(p.args(), ["revert", "--no-edit", &"a".repeat(40), &"b".repeat(40)]);
        assert!(p.describe().contains("newest first") && p.describe().contains("not in main's history"));
        assert_eq!(SeveralKind::Revert.menu_label_n(3), "Revert 3 Marked Commits…");
        assert_eq!(why_not(SeveralKind::CherryPick, 2, true, RepoState::Clean), Some(crate::availability::HEAD_MARKED));
        assert_eq!(why_not(SeveralKind::Revert, 2, true, RepoState::Clean), None);
        assert_eq!(why_not(SeveralKind::Revert, 0, false, RepoState::Clean), Some(crate::availability::NO_MARKED_COMMITS));
    }
}
