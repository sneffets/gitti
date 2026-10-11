//! What a rebase did, in plain words — and the commits it left out on its own.
//!
//! git drops a commit while it rebases when nothing of it is left to apply: its change is
//! on the new base already (picked there before — left out of the plan without a word —,
//! or the replay comes out empty: "dropping … patch contents already upstream"), or its
//! conflicts were resolved to the new base's side and Continue finds nothing staged — then
//! git goes on without a word at all. The user should hear about it: a commit gone from
//! the branch is not something to find out later.
//!
//! [`RebaseWatch`] notes, before a rebase step (the start, a Continue, a Skip), the
//! original commits the step may replay; afterwards [`Repo::rebase_noted`] matches them
//! with the commits now on the new base — by author and author time, which a rebase keeps
//! (the message may change: an edited Continue) — and says which ones are not there.
//! All in-process (gix), no git process. A finished rebase's status line is
//! [`finished`] (+ what became of an autostash, [`autostash_note`], + [`left_out`]); a
//! rebase that stopped again keeps its stop line, the note appended. Only the merge backend (`rebase-merge`) is followed through its stops;
//! `rebase --apply` names its own skipped patches.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use gix::ObjectId;

use crate::Repo;
use crate::ops::RepoState;

/// More original commits than this: no matching (a rebase of a huge branch stays fast).
const MAX_PENDING: usize = 5_000;

/// A commit's identity across a rebase: author name, email, time and its zone.
type Key = (Vec<u8>, Vec<u8>, i64, i32);

/// The original commits a rebase step may replay, noted before it runs.
#[derive(Debug, Clone)]
pub(crate) struct RebaseWatch {
    /// The new base.
    onto: ObjectId,
    /// Its name for the user (`main`, `origin/main`, a short id).
    base: String,
    /// HEAD before the step.
    head: Option<ObjectId>,
    /// The commits to replay, oldest first where known.
    pending: Vec<ObjectId>,
}

/// The status line of a finished rebase: `branch` (`None`: a detached HEAD) on `base`
/// with `on_top` commits of its own now.
pub fn finished(branch: Option<&str>, base: &str, on_top: usize) -> String {
    let who = branch.unwrap_or("HEAD");
    match on_top {
        0 => format!("Rebased {who} onto {base}: nothing of its own was left to replay, so it now points at {base}."),
        1 => format!("Rebased {who} onto {base}: its 1 commit is now on top of {base}."),
        n => format!("Rebased {who} onto {base}: its {n} commits are now on top of {base}."),
    }
}

/// The note on commits a rebase onto `base` left out — `(short id, subject)` each —
/// because nothing of them was left to apply; `None` when it left out none.
pub fn left_out(base: &str, commits: &[(String, String)]) -> Option<String> {
    match commits {
        [] => None,
        [(short, subject)] => Some(format!(
            "Left out {short} \"{subject}\": nothing of it was left to apply on {base} (its changes are \
             there already, or its conflicts were resolved to {base}'s side)."
        )),
        many => {
            const SHOWN: usize = 3;
            let mut list: Vec<String> =
                many.iter().take(SHOWN).map(|(short, subject)| format!("{short} \"{subject}\"")).collect();
            if many.len() > SHOWN {
                list.push(format!("and {} more", many.len() - SHOWN));
            }
            Some(format!(
                "Left out {} commits — nothing of them was left to apply on {base} (their changes are \
                 there already, or their conflicts were resolved to {base}'s side): {}.",
                many.len(),
                list.join(", ")
            ))
        }
    }
}

/// What became of the uncommitted changes git stashed for the rebase itself
/// (`rebase.autoStash`, `--autostash`), from its output; `None` when it stashed none.
pub fn autostash_note(git_output: &str) -> Option<&'static str> {
    if git_output.contains("Applying autostash resulted in conflicts") {
        Some(
            "Your uncommitted changes did not go back cleanly: they are kept in the stash \
             (Stashes) — apply it when you are ready.",
        )
    } else if git_output.contains("Applied autostash") {
        Some("Your uncommitted changes were stashed for it and put back (rebase.autoStash).")
    } else {
        None
    }
}

/// The first local branch, remote-tracking branch or tag at `id`; else its short id.
fn name_at(repo: &gix::Repository, id: ObjectId) -> String {
    if let Ok(refs) = repo.references() {
        let groups = [refs.local_branches(), refs.remote_branches(), refs.tags()];
        for group in groups.into_iter().flatten() {
            for mut r in group.filter_map(Result::ok) {
                let name = r.name().shorten().to_string();
                if !name.ends_with("/HEAD") && r.peel_to_id().is_ok_and(|t| t.detach() == id) {
                    return name;
                }
            }
        }
    }
    id.to_hex_with_len(7).to_string()
}

fn key_of(repo: &gix::Repository, id: ObjectId) -> Option<Key> {
    let commit = repo.find_commit(id).ok()?;
    let decoded = commit.decode().ok()?;
    let author = decoded.author().ok()?;
    let time = author.time().ok()?;
    Some((author.name.to_vec(), author.email.to_vec(), time.seconds, time.offset))
}

fn subject_of(repo: &gix::Repository, id: ObjectId) -> String {
    repo.find_commit(id)
        .ok()
        .and_then(|c| c.message().ok().map(|m| m.summary().to_string()))
        .unwrap_or_default()
}

/// The commits of a `rebase-merge` todo file (`git-rebase-todo`, `done`) that replay a
/// commit of their own — `pick`, `reword`, `edit` lines; `squash` / `fixup` fold into
/// another and are left aside — in order.
fn todo_commits(repo: &gix::Repository, text: &str) -> Vec<ObjectId> {
    text.lines()
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let cmd = words.next()?;
            if !matches!(cmd, "pick" | "p" | "reword" | "r" | "edit" | "e") {
                return None;
            }
            let rev = words.next()?;
            repo.rev_parse_single(rev).ok().map(|id| id.detach())
        })
        .collect()
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap_or_default()
}

/// A stopped `rebase-merge`'s stopped commit (the last line of `done`) and the commits
/// still to come.
fn still_to_replay(repo: &gix::Repository, dir: &Path) -> Vec<ObjectId> {
    let mut left = todo_commits(repo, &read(dir, "git-rebase-todo"));
    if let Some(last) = read(dir, "done").lines().rev().find(|l| !l.trim().is_empty()) {
        left.extend(todo_commits(repo, last));
    }
    left
}

impl Repo {
    /// Before `git rebase <onto>`: the commits of HEAD not on `onto`, without merges and
    /// without `fixup!` / `squash!` / `amend!` commits (an autosquash folds them).
    pub(crate) fn rebase_watch_start(&self, onto: &str) -> Option<RebaseWatch> {
        let repo = self.gix().ok()?;
        let onto_id = repo.rev_parse_single(onto).ok()?.object().ok()?.peel_to_commit().ok()?.id;
        let head = repo.head_id().ok()?.detach();
        let mut pending = Vec::new();
        for info in repo.rev_walk([head]).with_hidden([onto_id]).all().ok()? {
            let info = info.ok()?;
            if pending.len() >= MAX_PENDING {
                return None;
            }
            let commit = repo.find_commit(info.id).ok()?;
            if commit.parent_ids().count() > 1 {
                continue;
            }
            let subject = commit.message().ok().map(|m| m.summary().to_string()).unwrap_or_default();
            if ["fixup! ", "squash! ", "amend! "].iter().any(|p| subject.starts_with(p)) {
                continue;
            }
            pending.push(info.id);
        }
        pending.reverse();
        Some(RebaseWatch { onto: onto_id, base: onto.to_string(), head: Some(head), pending })
    }

    /// Before a Continue (`with_stopped`: the stopped commit is replayed too) or a Skip
    /// of a stopped rebase (merge backend): the commits it may replay. `None` when no
    /// such rebase is stopped.
    pub(crate) fn rebase_watch_step(&self, with_stopped: bool) -> Option<RebaseWatch> {
        if self.state() != RepoState::Rebasing {
            return None;
        }
        let dir = self.git_dir()?.join("rebase-merge");
        if !dir.is_dir() {
            return None;
        }
        let repo = self.gix().ok()?;
        let onto = ObjectId::from_hex(read(&dir, "onto").trim().as_bytes()).ok()?;
        let mut pending = Vec::new();
        if with_stopped && let Some(last) = read(&dir, "done").lines().rev().find(|l| !l.trim().is_empty()) {
            pending.extend(todo_commits(&repo, last));
        }
        pending.extend(todo_commits(&repo, &read(&dir, "git-rebase-todo")));
        if pending.len() > MAX_PENDING {
            return None;
        }
        let head = repo.head_id().ok().map(|id| id.detach());
        Some(RebaseWatch { onto, base: name_at(&repo, onto), head, pending })
    }

    /// `result` of a rebase step watched by `watch`: a finished rebase says where the
    /// branch is now ([`finished`]) and what it left out ([`left_out`]); a rebase that
    /// stopped again keeps its line, the note appended. Errors stay as they are.
    pub(crate) fn rebase_noted(&self, watch: Option<RebaseWatch>, result: Result<String>) -> Result<String> {
        let Some(w) = watch else {
            return result;
        };
        let out = result?;
        let Ok(repo) = self.gix() else {
            return Ok(out);
        };
        let state = self.state();
        let still: HashSet<ObjectId> = match state {
            RepoState::Clean => HashSet::new(),
            RepoState::Rebasing => match self.git_dir().map(|d| d.join("rebase-merge")) {
                Some(dir) if dir.is_dir() => still_to_replay(&repo, &dir).into_iter().collect(),
                _ => return Ok(out),
            },
            _ => return Ok(out),
        };
        let head = repo.head_id().ok().map(|id| id.detach());
        let replayed: Vec<ObjectId> = w.pending.iter().copied().filter(|id| !still.contains(id)).collect();
        if state == RepoState::Clean && replayed.is_empty() && head == w.head {
            // Nothing to do ("Current branch … is up to date"): git's words.
            return Ok(out);
        }
        let mut on_base: HashMap<Key, usize> = HashMap::new();
        let mut on_top = 0;
        if let Some(head) = head
            && let Ok(walk) = repo.rev_walk([head]).with_hidden([w.onto]).all()
        {
            for info in walk.filter_map(Result::ok) {
                on_top += 1;
                if let Some(k) = key_of(&repo, info.id) {
                    *on_base.entry(k).or_default() += 1;
                }
            }
        }
        let mut dropped = Vec::new();
        for id in replayed {
            let Some(k) = key_of(&repo, id) else { continue };
            match on_base.get_mut(&k) {
                Some(n) if *n > 0 => *n -= 1,
                _ => dropped.push((id.to_hex_with_len(7).to_string(), subject_of(&repo, id))),
            }
        }
        let mut line = match state {
            RepoState::Clean => {
                let branch = self.head().ok().and_then(|h| h.branch);
                let mut line = finished(branch.as_deref(), &w.base, on_top);
                if let Some(a) = autostash_note(&out) {
                    line = format!("{line} {a}");
                }
                line
            }
            _ => out,
        };
        if let Some(n) = left_out(&w.base, &dropped) {
            line = format!("{line} {n}");
        }
        Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn says_where_the_branch_is_and_what_was_left_out() {
        assert_eq!(finished(Some("feat"), "main", 2), "Rebased feat onto main: its 2 commits are now on top of main.");
        assert_eq!(finished(None, "main", 1), "Rebased HEAD onto main: its 1 commit is now on top of main.");
        assert!(finished(Some("feat"), "main", 0).contains("now points at main"));
        assert_eq!(left_out("main", &[]), None);
        assert!(autostash_note("Created autostash: 1234\nApplied autostash.\n").is_some_and(|n| n.contains("put back")));
        assert!(autostash_note("Applying autostash resulted in conflicts.").is_some_and(|n| n.contains("kept in the stash")));
        assert_eq!(autostash_note("Successfully rebased"), None);
        let one = left_out("main", &[("abc1234".into(), "Fix a".into())]).unwrap();
        assert!(one.starts_with("Left out abc1234 \"Fix a\": nothing of it was left to apply on main"), "{one}");
        let four: Vec<(String, String)> = (0..4).map(|i| (format!("id{i}"), format!("s{i}"))).collect();
        let many = left_out("main", &four).unwrap();
        assert!(many.starts_with("Left out 4 commits") && many.ends_with("id2 \"s2\", and 1 more."), "{many}");
    }
}
