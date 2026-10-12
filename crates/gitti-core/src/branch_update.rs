//! Fast-forwarding local branches without checking them out, in gitti and gitty alike:
//! "Fast-Forward {branch} to {upstream}" in a branch's sidebar menu and "Fast-Forward All
//! Branches" (BRANCHES section menu, Branch menu, palette).
//!
//! A branch moves to its upstream **as last fetched** (the remote-tracking branch, or a
//! local branch it follows) only when that is a fast-forward: the branch is behind and has
//! no commits of its own. The work is `git fetch . <upstream>:refs/heads/<branch>` — git
//! itself refuses a non-fast-forward (no `+`) and a branch checked out in any worktree, so
//! a race between the check and the update never loses a commit. Nothing in a working tree
//! changes.
//!
//! Refused, in plain words (checked first, from one `for-each-ref`):
//! - checked out here: that is a normal Pull (or a merge of the upstream) — it changes the
//!   files;
//! - checked out in another worktree (named): pull there;
//! - diverged: "N ahead, M behind — merge or rebase it instead";
//! - only ahead, up to date, no upstream, upstream gone.
//!
//! The menus grey the entry with the same reason from the last refresh's
//! [`crate::ops::Tracking`] ([`why_not`]) — no git while a menu opens.

use anyhow::{Result, anyhow};

use crate::ops::Tracking;
use crate::{ObjectId, Repo};

/// "Fast-Forward All Branches": the menu, palette and dialog title.
pub const ALL_LABEL: &str = "Fast-Forward All Branches";
/// What "Fast-Forward All Branches" does (the palette's description).
pub const ALL_DESCRIPTION: &str =
    "move each branch that is only behind its upstream to it, without checking it out";

/// A branch's menu entry: "Fast-Forward feat to origin/feat" ("… to Upstream" without one).
pub fn label(branch: &str, upstream: Option<&str>) -> String {
    format!("Fast-Forward {branch} to {}", upstream.unwrap_or("Upstream"))
}

/// Where a local branch is checked out.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CheckedOut {
    /// Nowhere.
    #[default]
    No,
    /// In this worktree.
    Here,
    /// In another worktree (its folder).
    Elsewhere(String),
}

/// Why branch `t` can't be fast-forwarded to its upstream now, in plain words — `None`
/// when it can (behind, nothing of its own). The menus' reason and the refusal.
pub fn why_not(t: &Tracking, at: &CheckedOut) -> Option<String> {
    match at {
        CheckedOut::Here => return Some("checked out here — Pull updates it".into()),
        CheckedOut::Elsewhere(path) => {
            return Some(format!("checked out in the worktree {path} — pull there"));
        }
        CheckedOut::No => {}
    }
    let Some(u) = &t.upstream else {
        return Some("no upstream — Set Upstream… gives it one".into());
    };
    if t.gone {
        return Some(format!("its upstream {u} is gone"));
    }
    match (t.ahead, t.behind) {
        (0, 0) => Some(format!("up to date with {u}")),
        (a, 0) => Some(format!("{a} ahead of {u}, nothing to fast-forward")),
        (a, b) if a > 0 => Some(format!("{a} ahead, {b} behind {u} — merge or rebase it instead")),
        _ => None,
    }
}

/// A [`why_not`] reason as a greyed menu entry shows it: without the advice after " — "
/// (the entry would grow too wide; running it says the whole reason).
pub fn menu_reason(why: &str) -> &str {
    why.split(" — ").next().unwrap_or(why)
}

/// [`why_not`] for branch `name` from a refresh's `tracking` (no entry: no upstream known).
pub fn why_not_in(tracking: &[Tracking], name: &str, at: &CheckedOut) -> Option<String> {
    let none = Tracking { branch: name.to_string(), ..Default::default() };
    why_not(tracking.iter().find(|t| t.branch == name).unwrap_or(&none), at)
}

/// How many local branches besides `current` "Fast-Forward All Branches" could move
/// (behind and not ahead; where they are checked out aside) — `RepoFacts::ff_branches`.
pub fn movable(tracking: &[Tracking], current: Option<&str>) -> usize {
    tracking
        .iter()
        .filter(|t| Some(t.branch.as_str()) != current && why_not(t, &CheckedOut::No).is_none())
        .count()
}

/// A local branch as the fast-forward sees it (from [`Repo::fast_forward_candidates`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub tracking: Tracking,
    pub tip: ObjectId,
    /// The upstream's full ref name (`refs/remotes/origin/x`), what git fetches from.
    pub upstream_ref: Option<String>,
    /// The upstream's commit, when it exists.
    pub upstream_tip: Option<ObjectId>,
    pub checked_out: CheckedOut,
}

impl Candidate {
    pub fn name(&self) -> &str {
        &self.tracking.branch
    }

    /// [`why_not`] for this branch.
    pub fn why_not(&self) -> Option<String> {
        why_not(&self.tracking, &self.checked_out)
    }
}

/// What happened to one branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Moved {
        name: String,
        from: ObjectId,
        to: ObjectId,
        upstream: String,
        commits: u32,
    },
    /// Not moved, and why ([`why_not`]).
    Skipped { name: String, why: String },
    /// git refused; its message.
    Failed { name: String, error: String },
}

impl Outcome {
    /// One line of the report: "feat: moved 1a2b3c4 → 5d6e7f8 (2 commits from
    /// origin/feat)", "wip: not moved — 1 ahead, 2 behind origin/wip — merge or rebase it
    /// instead".
    pub fn line(&self) -> String {
        match self {
            Outcome::Moved { name, from, to, upstream, commits } => format!(
                "{name}: moved {} → {} ({} from {upstream})",
                crate::submodule::short_id(*from),
                crate::submodule::short_id(*to),
                commit_count(*commits)
            ),
            Outcome::Skipped { name, why } => format!("{name}: not moved — {why}"),
            Outcome::Failed { name, error } => format!("{name}: not moved — git refused: {error}"),
        }
    }
}

fn commit_count(n: u32) -> String {
    if n == 1 { "1 commit".into() } else { format!("{n} commits") }
}

/// The report of [`Repo::fast_forward_all`]: `(summary, text)` — the status line ("Moved
/// 2 of 5 branches") and the result dialog's text, one line per branch, the moved ones
/// first, then what it measured against.
pub fn report(outcomes: &[Outcome]) -> (String, String) {
    let moved = outcomes.iter().filter(|o| matches!(o, Outcome::Moved { .. })).count();
    let failed = outcomes.iter().filter(|o| matches!(o, Outcome::Failed { .. })).count();
    let total = outcomes.len();
    let mut summary = match (moved, total) {
        (_, 0) => "No local branch to fast-forward".to_string(),
        (0, _) => "No branch fast-forwarded".to_string(),
        (m, t) => format!("Fast-forwarded {m} of {t} branch{}", if t == 1 { "" } else { "es" }),
    };
    if failed > 0 {
        summary.push_str(&format!(" ({failed} refused by git)"));
    }
    let mut lines: Vec<String> = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::Moved { .. }))
        .chain(outcomes.iter().filter(|o| !matches!(o, Outcome::Moved { .. })))
        .map(Outcome::line)
        .collect();
    lines.push(String::new());
    lines.push(
        "Each branch moved only where it was behind its upstream as last fetched and had no \
         commits of its own; no files changed. Fetch first to bring the upstreams up to date. \
         Nothing was pushed."
            .into(),
    );
    (summary, lines.join("\n"))
}

/// `[ahead 2, behind 1]` → (2, 1).
fn ahead_behind(track: &str) -> (u32, u32) {
    let num = |key: &str| {
        track
            .split(['[', ']', ','])
            .map(str::trim)
            .find_map(|p| p.strip_prefix(key).and_then(|n| n.trim().parse().ok()))
            .unwrap_or(0)
    };
    (num("ahead"), num("behind"))
}

impl Repo {
    /// Every local branch with its upstream, ahead / behind, tips and where it is checked
    /// out. One git command (`for-each-ref` over the local and remote branches).
    pub fn fast_forward_candidates(&self) -> Result<Vec<Candidate>> {
        let out = self
            .git()
            .run(&[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(upstream)%00%(upstream:short)%00%(upstream:track)%00%(worktreepath)",
                "refs/heads",
                "refs/remotes",
            ])?
            .stdout_str();
        let this = crate::paths::canonical(self.workdir());
        let rows: Vec<Vec<&str>> = out.lines().map(|l| l.split('\0').collect()).collect();
        let tips: std::collections::HashMap<&str, ObjectId> = rows
            .iter()
            .filter_map(|f| Some((*f.first()?, ObjectId::from_hex(f.get(1)?.as_bytes()).ok()?)))
            .collect();
        let mut v = Vec::new();
        for f in &rows {
            let [refname, id, up_ref, up_short, track, wt] = f[..] else {
                continue;
            };
            let Some(name) = refname.strip_prefix("refs/heads/") else {
                continue;
            };
            let Ok(tip) = ObjectId::from_hex(id.as_bytes()) else {
                continue;
            };
            let checked_out = if wt.is_empty() {
                CheckedOut::No
            } else if crate::paths::canonical(std::path::Path::new(wt)) == this {
                CheckedOut::Here
            } else {
                CheckedOut::Elsewhere(wt.to_string())
            };
            let (ahead, behind) = ahead_behind(track);
            v.push(Candidate {
                tracking: Tracking {
                    branch: name.to_string(),
                    upstream: (!up_short.is_empty()).then(|| up_short.to_string()),
                    ahead,
                    behind,
                    gone: track.contains("gone"),
                },
                tip,
                upstream_ref: (!up_ref.is_empty()).then(|| up_ref.to_string()),
                upstream_tip: tips.get(up_ref).copied(),
                checked_out,
            });
        }
        Ok(v)
    }

    /// Fast-forwards local branch `name` to its upstream without checking it out — or
    /// refuses in plain words ([`why_not`]). Two git commands. The status line's text.
    pub fn fast_forward_branch(&self, name: &str) -> Result<String> {
        let all = self.fast_forward_candidates()?;
        let c = all
            .iter()
            .find(|c| c.name() == name)
            .ok_or_else(|| anyhow!("There is no local branch {name}"))?;
        if let Some(why) = c.why_not() {
            anyhow::bail!("Can't fast-forward {name}: {why}");
        }
        match self.fast_forward(std::slice::from_ref(c))?.pop() {
            Some(o @ Outcome::Moved { .. }) => Ok(format!("Fast-forwarded {}", o.line())),
            Some(Outcome::Failed { error, .. }) => Err(anyhow!("Can't fast-forward {name}: {error}")),
            Some(Outcome::Skipped { why, .. }) => Err(anyhow!("Can't fast-forward {name}: {why}")),
            None => Err(anyhow!("Can't fast-forward {name}")),
        }
    }

    /// "Fast-Forward All Branches": every local branch that is only behind its upstream
    /// moves to it (one `git fetch .` for all of them); every other one is reported with
    /// why not. One outcome per local branch, in git's order. Two git commands (three
    /// when git refused one).
    pub fn fast_forward_all(&self) -> Result<Vec<Outcome>> {
        let all = self.fast_forward_candidates()?;
        let movable: Vec<Candidate> = all.iter().filter(|c| c.why_not().is_none()).cloned().collect();
        let moved = self.fast_forward(&movable)?;
        Ok(all
            .iter()
            .map(|c| match c.why_not() {
                Some(why) => Outcome::Skipped { name: c.name().to_string(), why },
                None => moved
                    .iter()
                    .find(|o| match o {
                        Outcome::Moved { name, .. }
                        | Outcome::Skipped { name, .. }
                        | Outcome::Failed { name, .. } => name == c.name(),
                    })
                    .cloned()
                    .unwrap_or_else(|| Outcome::Failed {
                        name: c.name().to_string(),
                        error: "no result".into(),
                    }),
            })
            .collect())
    }

    /// `git fetch . <upstream>:refs/heads/<branch>` for each of `branches` (all checked
    /// with [`why_not`] already) in one command; when git refuses one, the branches are
    /// read again to tell which moved.
    fn fast_forward(&self, branches: &[Candidate]) -> Result<Vec<Outcome>> {
        let planned: Vec<(&Candidate, &str, ObjectId)> = branches
            .iter()
            .filter_map(|c| Some((c, c.upstream_ref.as_deref()?, c.upstream_tip?)))
            .collect();
        let mut out: Vec<Outcome> = branches
            .iter()
            .filter(|c| c.upstream_ref.is_none() || c.upstream_tip.is_none())
            .map(|c| Outcome::Skipped {
                name: c.name().to_string(),
                why: "its upstream doesn't exist".into(),
            })
            .collect();
        if planned.is_empty() {
            return Ok(out);
        }
        let specs: Vec<String> = planned
            .iter()
            .map(|(c, up, _)| format!("{up}:refs/heads/{}", c.name()))
            .collect();
        // No tags, FETCH_HEAD, pruning, submodules or maintenance: only these refs move.
        let mut args = vec![
            "fetch",
            "--no-tags",
            "--no-prune",
            "--no-write-fetch-head",
            "--recurse-submodules=no",
            "--no-auto-maintenance",
            ".",
        ];
        args.extend(specs.iter().map(String::as_str));
        let moved = |c: &Candidate, to: ObjectId| Outcome::Moved {
            name: c.name().to_string(),
            from: c.tip,
            to,
            upstream: c.tracking.upstream.clone().unwrap_or_default(),
            commits: c.tracking.behind,
        };
        match self.git().run(&args) {
            Ok(_) => out.extend(planned.iter().map(|(c, _, to)| moved(c, *to))),
            Err(e) => {
                let e = anyhow::Error::from(e);
                let stderr = crate::worktree::git_stderr(&e)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| format!("{e:#}"));
                let now = self.fast_forward_candidates()?;
                for (c, _, to) in &planned {
                    let tip = now.iter().find(|n| n.name() == c.name()).map(|n| n.tip);
                    if tip == Some(*to) && *to != c.tip {
                        out.push(moved(c, *to));
                    } else {
                        // git's line about this branch, else all it said.
                        let mine = stderr
                            .lines()
                            .find(|l| l.contains(&format!("-> {}", c.name())) || l.contains(&format!("refs/heads/{}'", c.name())))
                            .map(|l| l.trim().to_string());
                        out.push(Outcome::Failed {
                            name: c.name().to_string(),
                            error: mine.unwrap_or_else(|| stderr.clone()),
                        });
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(upstream: Option<&str>, ahead: u32, behind: u32) -> Tracking {
        Tracking {
            branch: "feat".into(),
            upstream: upstream.map(str::to_string),
            ahead,
            behind,
            gone: false,
        }
    }

    #[gitti_test::test]
    fn only_a_branch_behind_and_not_ahead_can_move() {
        let u = Some("origin/feat");
        assert_eq!(why_not(&t(u, 0, 2), &CheckedOut::No), None);
        assert_eq!(
            why_not(&t(u, 1, 2), &CheckedOut::No).as_deref(),
            Some("1 ahead, 2 behind origin/feat — merge or rebase it instead")
        );
        assert_eq!(
            why_not(&t(u, 3, 0), &CheckedOut::No).as_deref(),
            Some("3 ahead of origin/feat, nothing to fast-forward")
        );
        assert_eq!(why_not(&t(u, 0, 0), &CheckedOut::No).as_deref(), Some("up to date with origin/feat"));
        assert!(why_not(&t(None, 0, 0), &CheckedOut::No).unwrap().starts_with("no upstream"));
        let gone = Tracking { gone: true, ..t(u, 0, 0) };
        assert_eq!(why_not(&gone, &CheckedOut::No).as_deref(), Some("its upstream origin/feat is gone"));
        // Where it is checked out comes first.
        assert_eq!(
            why_not(&t(u, 0, 2), &CheckedOut::Here).as_deref(),
            Some("checked out here — Pull updates it")
        );
        assert_eq!(
            why_not(&t(u, 0, 2), &CheckedOut::Elsewhere("/w/x".into())).as_deref(),
            Some("checked out in the worktree /w/x — pull there")
        );
    }

    #[gitti_test::test]
    fn labels_and_report() {
        assert_eq!(label("feat", Some("origin/feat")), "Fast-Forward feat to origin/feat");
        assert_eq!(label("feat", None), "Fast-Forward feat to Upstream");
        assert_eq!(menu_reason("1 ahead, 2 behind origin/feat — merge or rebase it instead"), "1 ahead, 2 behind origin/feat");
        assert_eq!(menu_reason("up to date with origin/feat"), "up to date with origin/feat");
        let id = |n: u8| ObjectId::from_bytes_or_panic(&[n; 20]);
        let outcomes = [
            Outcome::Skipped { name: "main".into(), why: "checked out here — Pull updates it".into() },
            Outcome::Moved { name: "feat".into(), from: id(1), to: id(2), upstream: "origin/feat".into(), commits: 2 },
        ];
        let (summary, text) = report(&outcomes);
        assert_eq!(summary, "Fast-forwarded 1 of 2 branches");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "feat: moved 0101010 → 0202020 (2 commits from origin/feat)");
        assert_eq!(lines[1], "main: not moved — checked out here — Pull updates it");
        assert!(text.contains("Fetch first"), "{text}");
        assert_eq!(report(&[]).0, "No local branch to fast-forward");
        assert_eq!(report(&outcomes[..1]).0, "No branch fast-forwarded");
    }
}
