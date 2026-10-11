//! Deleting several local branches at once, in gitti and gitty alike: "Clean Up Merged
//! Branches…" (the branches whose commits are all in a base — the current branch unless
//! another one is chosen) and "Delete Branches…" (any of them; one not merged into the
//! base only with its own Force box).
//!
//! Safety rules, the same in both dialogs:
//! - never the current branch, never one checked out in another worktree (git refuses
//!   both; the dialog names them and why);
//! - "merged" = `git for-each-ref --merged=<base>`: every commit of the branch is in the
//!   base, so deleting it removes only the name;
//! - right before deleting, the list is read again ([`Repo::delete_branches`]): a branch that
//!   moved since the dialog showed it, was checked out meanwhile or is no longer merged
//!   (without Force) is skipped and reported, never deleted;
//! - the dialog lists each branch's commit, and the result says how to bring one back
//!   (`git branch <name> <commit>`) — git keeps no reflog of a deleted branch.
//!
//! The marks say what else to know about a branch: no upstream, its upstream is gone (the
//! remote branch was deleted, typically after its pull request was merged), commits not
//! pushed to its upstream, or no commits of its own (same commit as the base).
//! Remote branches are not touched.

use anyhow::{Result, anyhow};

use crate::{ObjectId, Repo};

/// A local branch as the dialogs show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// `feature/x` (without `refs/heads/`).
    pub name: String,
    pub tip: ObjectId,
    /// All its commits are in the base.
    pub merged: bool,
    /// It points at the base's commit (no commits of its own; e.g. just created).
    pub at_base: bool,
    /// Its upstream (`origin/x`), if it has one.
    pub upstream: Option<String>,
    /// The upstream is configured but gone.
    pub gone: bool,
    /// Commits not on its upstream.
    pub ahead: u32,
    /// The checked-out branch of this worktree.
    pub current: bool,
    /// Checked out in another worktree (its folder): git can't delete it there.
    pub worktree: Option<String>,
}

impl Candidate {
    /// Can be deleted at all (not checked out anywhere).
    pub fn deletable(&self) -> bool {
        !self.current && self.worktree.is_none()
    }

    /// The faint words after its name: its commit and what to know ("1a2b3c4 · upstream
    /// gone").
    pub fn marks(&self, base: &str) -> String {
        let mut parts = vec![crate::submodule::short_id(self.tip)];
        if !self.merged {
            parts.push(format!("not merged into {base}"));
        } else if self.at_base {
            parts.push(format!("same commit as {base}, no commits of its own"));
        }
        match &self.upstream {
            None => parts.push("no upstream".into()),
            Some(_) if self.gone => parts.push("upstream gone".into()),
            Some(u) if self.ahead > 0 => parts.push(format!(
                "{} commit{} not pushed to {u}",
                self.ahead,
                plural(self.ahead as usize)
            )),
            Some(u) => parts.push(format!("pushed to {u}")),
        }
        parts.join(" · ")
    }
}

/// The local branches against a base ([`Repo::branch_set`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSet {
    /// The base as given ("HEAD", a branch, a remote branch).
    pub base: String,
    /// How the dialogs name it: the branch, or "HEAD (1a2b3c4)" when detached.
    pub base_label: String,
    pub base_id: ObjectId,
    /// Every local branch, by name.
    pub branches: Vec<Candidate>,
}

/// Which dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// "Clean Up Merged Branches…": only the merged ones are offered, ticked.
    CleanUp,
    /// "Delete Branches…": every one, none ticked; an unmerged one needs its Force box.
    Delete,
}

/// One row of a dialog: a branch it offers, with the boxes it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Index into [`BranchSet::branches`].
    pub branch: usize,
    /// The label of its box: the name and its marks.
    pub label: String,
    /// Ticked when the dialog opens.
    pub ticked: bool,
    /// The label of its Force box (an unmerged branch in [`Mode::Delete`]).
    pub force: Option<String>,
}

impl BranchSet {
    /// The rows of the dialog `mode`, in order (by name).
    pub fn rows(&self, mode: Mode) -> Vec<Row> {
        self.branches
            .iter()
            .enumerate()
            .filter(|(_, c)| c.deletable() && (mode == Mode::Delete || c.merged))
            .map(|(i, c)| Row {
                branch: i,
                label: format!("{}  —  {}", c.name, c.marks(&self.base_label)),
                // Merged ones in the clean-up, except a branch without commits of its own
                // (likely just created to work on).
                ticked: mode == Mode::CleanUp && !c.at_base,
                force: (mode == Mode::Delete && !c.merged).then(|| {
                    format!("Force: delete {} although not merged into {}", c.name, self.base_label)
                }),
            })
            .collect()
    }

    /// The branches no dialog can offer and why ("main (checked out here)").
    pub fn not_offered(&self) -> Vec<String> {
        self.branches
            .iter()
            .filter_map(|c| {
                if c.current {
                    Some(format!("{} (checked out here)", c.name))
                } else {
                    c.worktree
                        .as_ref()
                        .map(|w| format!("{} (checked out in the worktree {w})", c.name))
                }
            })
            .collect()
    }

    /// What the dialog says above its boxes: what "merged" means here, what deleting
    /// does and doesn't do, how to undo it.
    pub fn message(&self, mode: Mode) -> String {
        let base = &self.base_label;
        let mut s = match mode {
            Mode::CleanUp => format!(
                "Local branches whose commits are all in {base}: deleting one removes only \
                 its name, the commits stay in {base}."
            ),
            Mode::Delete => format!(
                "Deleting a branch removes its name. One merged into {base} loses nothing; \
                 one that is not needs its Force box — the commits only it has are then \
                 found by their id alone, until git's garbage collection removes them."
            ),
        };
        s.push_str(" Remote branches stay as they are.");
        let skipped = self.not_offered();
        if !skipped.is_empty() {
            s.push_str(&format!("\nNot offered: {}.", skipped.join(", ")));
        }
        s.push_str(
            "\nTo bring one back: Create Branch at the commit shown after its name \
             (git branch <name> <commit>).",
        );
        s
    }

    /// What OK will do with the ticked boxes: `ticked[i]` / `forced[i]` per row of `rows`.
    pub fn live(&self, rows: &[Row], ticked: &[bool], forced: &[bool]) -> Live {
        let chosen: Vec<&Row> = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| ticked.get(*i).copied().unwrap_or(false))
            .map(|(_, r)| r)
            .collect();
        if chosen.is_empty() {
            return Live {
                text: String::new(),
                ok_label: "Delete Branches".into(),
                blocked: Some("Tick the branches to delete".into()),
            };
        }
        let unforced: Vec<&str> = rows
            .iter()
            .enumerate()
            .filter(|(i, r)| {
                r.force.is_some()
                    && ticked.get(*i).copied().unwrap_or(false)
                    && !forced.get(*i).copied().unwrap_or(false)
            })
            .map(|(_, r)| self.branches[r.branch].name.as_str())
            .collect();
        let names: Vec<&str> = chosen.iter().map(|r| self.branches[r.branch].name.as_str()).collect();
        let n = names.len();
        let ok_label = if n == 1 {
            "Delete 1 Branch".to_string()
        } else {
            format!("Delete {n} Branches")
        };
        let blocked = (!unforced.is_empty()).then(|| {
            format!(
                "{} not merged into {}: tick {} Force box or untick {}",
                list_words(&unforced, if unforced.len() == 1 { "is" } else { "are" }),
                self.base_label,
                if unforced.len() == 1 { "its" } else { "their" },
                if unforced.len() == 1 { "it" } else { "them" },
            )
        });
        let forced_names: Vec<&str> = rows
            .iter()
            .enumerate()
            .filter(|(i, r)| {
                r.force.is_some()
                    && ticked.get(*i).copied().unwrap_or(false)
                    && forced.get(*i).copied().unwrap_or(false)
            })
            .map(|(_, r)| self.branches[r.branch].name.as_str())
            .collect();
        let mut text = format!("Deletes {}.", names.join(", "));
        if !forced_names.is_empty() {
            text.push_str(&format!(
                " Not merged into {}: {} — commits only there are left without a branch.",
                self.base_label,
                forced_names.join(", ")
            ));
        }
        Live {
            text,
            ok_label,
            blocked,
        }
    }

    /// The branches to delete from the ticked boxes (see [`BranchSet::live`]).
    pub fn chosen(&self, rows: &[Row], ticked: &[bool], forced: &[bool]) -> Vec<Chosen> {
        rows.iter()
            .enumerate()
            .filter(|(i, _)| ticked.get(*i).copied().unwrap_or(false))
            .map(|(i, r)| {
                let c = &self.branches[r.branch];
                Chosen {
                    name: c.name.clone(),
                    tip: c.tip,
                    force: r.force.is_some() && forced.get(i).copied().unwrap_or(false),
                }
            })
            .collect()
    }
}

fn list_words(names: &[&str], verb: &str) -> String {
    format!("{} {verb}", names.join(", "))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// What a dialog says live ([`BranchSet::live`]): the sentence, OK's label, why OK can't
/// run now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Live {
    pub text: String,
    pub ok_label: String,
    pub blocked: Option<String>,
}

/// A branch to delete, as the dialog showed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub name: String,
    /// Its commit when shown: deleted only if it still points there.
    pub tip: ObjectId,
    /// Deleted even though it is not merged into the base.
    pub force: bool,
}

/// What happened to one [`Chosen`] branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Deleted { name: String, tip: ObjectId },
    /// Not deleted, and why (moved, checked out, not merged any more, gone).
    Skipped { name: String, why: String },
    /// git refused; its message.
    Failed { name: String, error: String },
}

/// The status line after [`Repo::delete_branches`]: `(info, error)`.
pub fn outcome_message(outcomes: &[Outcome]) -> (String, Option<String>) {
    let deleted: Vec<String> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Deleted { name, tip } => {
                Some(format!("{name} (was {})", crate::submodule::short_id(*tip)))
            }
            _ => None,
        })
        .collect();
    let problems: Vec<String> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Skipped { name, why } => Some(format!("{name} not deleted: {why}")),
            Outcome::Failed { name, error } => Some(format!("{name} not deleted: {error}")),
            Outcome::Deleted { .. } => None,
        })
        .collect();
    let info = match deleted.len() {
        0 => "No branch deleted".to_string(),
        n => format!(
            "Deleted {n} branch{}: {} — to bring one back: git branch <name> <commit>",
            if n == 1 { "" } else { "es" },
            deleted.join(", ")
        ),
    };
    (info, (!problems.is_empty()).then(|| problems.join("; ")))
}

impl Repo {
    /// Every local branch against `base` ("HEAD", a branch): merged or not, upstream state,
    /// where it is checked out. Two git commands (`for-each-ref`), plus the base resolved
    /// in-process.
    pub fn branch_set(&self, base: &str) -> Result<BranchSet> {
        let repo = self.gix()?;
        let base_id = repo
            .rev_parse_single(format!("{base}^{{commit}}").as_str())
            .map_err(|e| anyhow!("{base} is not a commit: {e}"))?
            .detach();
        let head_branch = self.head()?.branch;
        let base_label = if base == "HEAD" {
            match &head_branch {
                Some(b) => b.clone(),
                None => format!("HEAD ({})", crate::submodule::short_id(base_id)),
            }
        } else {
            base.to_string()
        };
        let merged_arg = format!("--merged={base}");
        let merged_out = self
            .git()
            .run(&["for-each-ref", &merged_arg, "--format=%(refname)", "refs/heads"])?
            .stdout_str();
        let merged: std::collections::HashSet<&str> = merged_out.lines().collect();
        let out = self
            .git()
            .run(&[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(upstream:short)%00%(upstream:track)%00%(worktreepath)",
                "refs/heads",
            ])?
            .stdout_str();
        let this = crate::paths::canonical(self.workdir());
        let mut branches = Vec::new();
        for line in out.lines() {
            let f: Vec<&str> = line.split('\0').collect();
            let [refname, id, upstream, track, wt] = f[..] else {
                continue;
            };
            let Some(name) = refname.strip_prefix("refs/heads/") else {
                continue;
            };
            let Ok(tip) = ObjectId::from_hex(id.as_bytes()) else {
                continue;
            };
            let current = head_branch.as_deref() == Some(name);
            let worktree = (!wt.is_empty() && !current)
                .then(|| std::path::PathBuf::from(wt))
                .filter(|p| crate::paths::canonical(p) != this)
                .map(|p| p.display().to_string());
            let ahead = track
                .split(['[', ']', ','])
                .map(str::trim)
                .find_map(|p| p.strip_prefix("ahead ").and_then(|n| n.trim().parse().ok()))
                .unwrap_or(0);
            branches.push(Candidate {
                name: name.to_string(),
                tip,
                merged: merged.contains(refname),
                at_base: tip == base_id,
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                gone: track.contains("gone"),
                ahead,
                current,
                worktree,
            });
        }
        branches.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(BranchSet {
            base: base.to_string(),
            base_label,
            base_id,
            branches,
        })
    }

    /// Deletes the `chosen` branches after checking each again against `base` (see the
    /// module docs): it must still point at the commit shown, not be checked out, and —
    /// without Force — still be merged. `git branch -D` per branch (the check above is
    /// what makes it safe; `-d` would also refuse a merged branch whose upstream lacks a
    /// commit). One outcome per branch, in order.
    pub fn delete_branches(&self, base: &str, chosen: &[Chosen]) -> Result<Vec<Outcome>> {
        let now = self.branch_set(base)?;
        let mut out = Vec::new();
        for c in chosen {
            let name = c.name.clone();
            let Some(b) = now.branches.iter().find(|b| b.name == c.name) else {
                out.push(Outcome::Skipped {
                    name,
                    why: "it no longer exists".into(),
                });
                continue;
            };
            let why = if b.tip != c.tip {
                Some(format!(
                    "it moved to {} since the list was shown",
                    crate::submodule::short_id(b.tip)
                ))
            } else if b.current {
                Some("it is checked out here".into())
            } else if let Some(w) = &b.worktree {
                Some(format!("it is checked out in the worktree {w}"))
            } else if !b.merged && !c.force {
                Some(format!("it is not merged into {} (any more)", now.base_label))
            } else {
                None
            };
            if let Some(why) = why {
                out.push(Outcome::Skipped { name, why });
                continue;
            }
            match self.git().run(&["branch", "-D", "--", &c.name]) {
                Ok(_) => out.push(Outcome::Deleted { name, tip: b.tip }),
                Err(e) => {
                    let e = anyhow::Error::from(e);
                    let error = crate::worktree::git_stderr(&e)
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|| format!("{e:#}"));
                    out.push(Outcome::Failed { name, error });
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(name: &str, merged: bool, n: u8) -> Candidate {
        Candidate {
            name: name.into(),
            tip: ObjectId::from_bytes_or_panic(&[n; 20]),
            merged,
            at_base: false,
            upstream: Some(format!("origin/{name}")),
            gone: false,
            ahead: 0,
            current: false,
            worktree: None,
        }
    }

    fn set() -> BranchSet {
        let mut main = cand("main", true, 1);
        main.current = true;
        let mut fresh = cand("fresh", true, 1);
        fresh.at_base = true;
        fresh.upstream = None;
        let mut gone = cand("done", true, 2);
        gone.gone = true;
        let mut wip = cand("wip", false, 3);
        wip.ahead = 2;
        let mut other = cand("elsewhere", true, 4);
        other.worktree = Some("/w/x".into());
        BranchSet {
            base: "HEAD".into(),
            base_label: "main".into(),
            base_id: ObjectId::from_bytes_or_panic(&[1; 20]),
            branches: vec![main, fresh, gone, wip, other],
        }
    }

    #[gitti_test::test]
    fn clean_up_offers_merged_deletable_branches() {
        let s = set();
        let rows = s.rows(Mode::CleanUp);
        let names: Vec<&str> = rows.iter().map(|r| s.branches[r.branch].name.as_str()).collect();
        assert_eq!(names, ["fresh", "done"]);
        assert!(!rows[0].ticked, "no commits of its own: not ticked");
        assert!(rows[1].ticked);
        assert!(rows[0].label.contains("same commit as main") && rows[0].label.contains("no upstream"));
        assert!(rows[1].label.contains("upstream gone"), "{}", rows[1].label);
        assert!(rows.iter().all(|r| r.force.is_none()));
        let msg = s.message(Mode::CleanUp);
        assert!(msg.contains("main (checked out here)"), "{msg}");
        assert!(msg.contains("elsewhere (checked out in the worktree /w/x)"), "{msg}");
    }

    #[gitti_test::test]
    fn delete_needs_force_for_unmerged() {
        let s = set();
        let rows = s.rows(Mode::Delete);
        let names: Vec<&str> = rows.iter().map(|r| s.branches[r.branch].name.as_str()).collect();
        assert_eq!(names, ["fresh", "done", "wip"]);
        assert!(rows.iter().all(|r| !r.ticked));
        assert!(rows[2].label.contains("not merged into main"), "{}", rows[2].label);
        assert!(rows[2].label.contains("2 commits not pushed"), "{}", rows[2].label);
        assert!(rows[2].force.is_some());
        let none = s.live(&rows, &[false; 3], &[false; 3]);
        assert!(none.blocked.is_some());
        let live = s.live(&rows, &[false, true, true], &[false; 3]);
        assert_eq!(live.ok_label, "Delete 2 Branches");
        assert!(live.blocked.as_deref().unwrap().starts_with("wip is not merged into main"), "{live:?}");
        let live = s.live(&rows, &[false, true, true], &[false, false, true]);
        assert_eq!(live.blocked, None);
        assert!(live.text.contains("Not merged into main: wip"), "{live:?}");
        let chosen = s.chosen(&rows, &[false, true, true], &[true, false, true]);
        assert_eq!(chosen.len(), 2);
        assert!(!chosen[0].force, "Force only counts for unmerged rows");
        assert!(chosen[1].force);
    }

    #[gitti_test::test]
    fn outcome_words() {
        let tip = ObjectId::from_bytes_or_panic(&[0xab; 20]);
        let (info, err) = outcome_message(&[
            Outcome::Deleted { name: "a".into(), tip },
            Outcome::Skipped {
                name: "b".into(),
                why: "it moved".into(),
            },
        ]);
        assert!(info.starts_with("Deleted 1 branch: a (was abababa)"), "{info}");
        assert_eq!(err.as_deref(), Some("b not deleted: it moved"));
    }
}
