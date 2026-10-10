//! `git subtree`: another repository's files in a folder of this one, with or without its
//! history — add, pull, merge, push, split (all through the git CLI, so hooks, signing,
//! credentials and askpass work as everywhere).
//!
//! - [`detect`]: which folders are subtrees, read from the trailers `git subtree` writes
//!   (git keeps no list), in-process and cached per HEAD ([`Repo::subtrees`]);
//! - [`source`]: where a subtree comes from — not recorded by git; remembered in the
//!   repository's config when gitti adds / pulls / pushes one;
//! - [`request`]: the dialogs' input, the plain-words plan they show, the command line;
//! - [`Repo::run_subtree`]: runs it (a missing `git subtree`, conflicts and the split's
//!   commit id are reported as such).

pub mod detect;
pub mod request;
pub mod source;

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use gix::ObjectId;

use crate::git_cli::GitError;
use crate::ops::RepoState;
use crate::repo::Repo;

pub use request::{Action, Context, Form, Plan, Request, form, from_form, normalize_prefix, plan, prefix_problem};
pub use source::Source;

/// How a subtree came in last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Update {
    /// `git subtree add`.
    Added,
    /// `git subtree pull` / `merge`.
    Merged,
    /// `git subtree split --rejoin` (or `push --rejoin`).
    Rejoined,
}

impl Update {
    pub fn label(self) -> &'static str {
        match self {
            Update::Added => "added",
            Update::Merged => "updated",
            Update::Rejoined => "split + rejoined",
        }
    }
}

/// A subtree of the checked-out history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subtree {
    /// Its folder, relative to the top of the work tree (no trailing `/`).
    pub prefix: String,
    /// The commit of the subtree's own history last added / merged / split
    /// (`git-subtree-split`).
    pub split: Option<ObjectId>,
    /// Brought in with `--squash` (one commit per update, without the subtree's history).
    pub squash: bool,
    /// The commit of this history that last brought it in.
    pub commit: ObjectId,
    /// That commit's time (seconds since the epoch).
    pub time: i64,
    pub update: Update,
    /// The commit that added it.
    pub added: ObjectId,
    /// The folder is in HEAD's tree (it may have been removed since).
    pub present: bool,
    /// Where it comes from, as remembered by gitti / gitty ([`source`]).
    pub source: Option<Source>,
}

impl Subtree {
    /// The sidebar's faint detail: "squash · updated 3d ago"-like parts without the time.
    pub fn detail(&self) -> String {
        let mut parts = Vec::new();
        if !self.present {
            parts.push("folder removed".to_string());
        }
        if self.squash {
            parts.push("squash".to_string());
        }
        if let Some(s) = &self.source {
            let r = s.reference.trim();
            if !r.is_empty() {
                parts.push(r.to_string());
            }
        }
        parts.join(" · ")
    }

    /// Everything about it, for a tooltip / an info line.
    pub fn describe(&self, remotes: &[crate::ops::RemoteInfo]) -> String {
        let mut s = format!("Subtree {}/", self.prefix);
        if !self.present {
            s.push_str(" — the folder is no longer in HEAD");
        }
        s.push_str(&format!(
            "\nLast {} in {}{}",
            self.update.label(),
            short(self.commit),
            if self.squash { " (squashed)" } else { "" }
        ));
        if let Some(split) = self.split {
            s.push_str(&format!(", subtree commit {}", short(split)));
        }
        match &self.source {
            Some(src) => {
                s.push_str(&format!("\nFrom {}", src.describe(remotes)));
                if !src.reference.is_empty() {
                    s.push_str(&format!(", branch {}", src.reference));
                }
                if !src.push_ref.is_empty() && src.push_ref != src.reference {
                    s.push_str(&format!(" (pushes to {})", src.push_ref));
                }
            }
            None => s.push_str(
                "\nWhere it comes from is not known yet (git doesn't record it): Pull / Push ask once, Edit Source… sets it",
            ),
        }
        s
    }
}

/// Tracked files with staged or unstaged changes, and conflicted files, in `w`
/// (untracked files don't stop `git subtree`).
pub fn tracked_changes(w: &crate::repo::WorkingState) -> (usize, usize) {
    let mut paths: Vec<&str> = w
        .staged
        .iter()
        .chain(w.unstaged.iter().filter(|f| !f.entry.untracked))
        .map(|f| f.path())
        .collect();
    paths.sort_unstable();
    paths.dedup();
    (paths.len(), w.conflicted.len())
}

/// The folder an Add suggests: the repository's name (`https://x/lib.git` → `lib`), else
/// the last part of the branch / ref (`origin/tools` → `tools`).
pub fn default_prefix(repository: &str, reference: &str) -> String {
    let from_repo = crate::submodule::default_path(repository);
    if !repository.trim().is_empty() && from_repo != "." && !from_repo.is_empty() {
        return from_repo;
    }
    let r = reference.trim().trim_end_matches('/');
    r.rsplit('/').next().unwrap_or(r).to_string()
}

/// A short commit id.
pub fn short(id: ObjectId) -> String {
    id.to_hex_with_len(7).to_string()
}

/// What a missing `git subtree` says (some Linux distributions package it separately).
pub const MISSING: &str = "git subtree is not installed. It is part of git, but some Linux \
     distributions ship it as a package of its own, usually named git-subtree (e.g. `sudo dnf \
     install git-subtree`, `sudo apk add git-subtree`) — install it and try again.";

/// Set once a run found `git subtree` missing: the dialogs say so before running.
static KNOWN_MISSING: AtomicBool = AtomicBool::new(false);

/// Whether a run found `git subtree` missing in this process.
pub fn known_missing() -> bool {
    KNOWN_MISSING.load(Ordering::Relaxed)
}

/// `git subtree` is not installed (see [`MISSING`]).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", MISSING)]
pub struct SubtreeMissing;

/// Whether git's error says the `subtree` command doesn't exist.
pub fn is_missing(stderr: &str) -> bool {
    stderr.contains("'subtree' is not a git command")
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The result in plain words (for the status line).
    pub message: String,
    /// The merge stopped with conflicts (the repository is in the merge state).
    pub conflicts: bool,
    /// Split / Push: the newest commit of the split history.
    pub split: Option<ObjectId>,
}

impl Repo {
    /// The subtrees of HEAD's history (in-process; cached per HEAD — a new HEAD on top
    /// of a scanned one only reads the new commits), with the folders' presence and the
    /// remembered sources. Run it in the background: the first scan of a long history
    /// reads every commit message once.
    pub fn subtrees(&self) -> Result<Vec<Subtree>> {
        let repo = self.gix()?;
        let head = repo.head_id().ok().map(|id| id.detach());
        let scan = self.subtree_scan(&repo, head);
        let mut list = scan.subtrees();
        if !list.is_empty() {
            let tree = repo
                .head_commit()
                .ok()
                .and_then(|c| c.tree().ok());
            for s in &mut list {
                s.present = tree.as_ref().is_some_and(|t| {
                    t.lookup_entry_by_path(&s.prefix)
                        .ok()
                        .flatten()
                        .is_some_and(|e| e.mode().is_tree())
                });
            }
        }
        let sources = self.subtree_sources();
        for s in &mut list {
            s.source = sources
                .iter()
                .find(|(p, _)| *p == s.prefix)
                .map(|(_, src)| src.clone());
        }
        Ok(list)
    }

    /// The cached scan for `head`, or a new one built on the newest cached scan.
    fn subtree_scan(&self, repo: &gix::Repository, head: Option<ObjectId>) -> detect::Scan {
        const KEEP: usize = 4;
        let cache = self.subtree_cache();
        let base = {
            let scans = cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(hit) = scans.iter().find(|s| s.head == head) {
                return hit.clone();
            }
            scans.last().cloned().unwrap_or_default()
        };
        let scan = base.update(repo, head);
        let mut scans = cache.lock().unwrap_or_else(|e| e.into_inner());
        scans.retain(|s| s.head != head);
        scans.push(scan.clone());
        if scans.len() > KEEP {
            scans.remove(0);
        }
        scan
    }

    /// Runs `req` (`git subtree …`; inside an operation's progress scope the split's
    /// progress shows as a meter). Remembers the repository / branch of an add, pull or push
    /// ([`source`]). Conflicts of a pull / merge are an [`Outcome`] with `conflicts`, not an
    /// error; a missing `git subtree` is a [`SubtreeMissing`].
    pub fn run_subtree(&self, req: &Request) -> Result<Outcome> {
        if let Some(p) = prefix_problem(&req.prefix) {
            anyhow::bail!("{p}");
        }
        let prefix = normalize_prefix(&req.prefix);
        if req.needs_clean() {
            // `git subtree` checks with `diff-index`, which counts files touched but
            // unchanged as modified until the index is refreshed (gitti's own reads never
            // write the index).
            let _ = self.git().run_raw(&["update-index", "-q", "--refresh"], None);
        }
        let sign = req.action.merges() && self.signs_commits();
        let args = req.args(sign);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        // `git merge` opens no editor for its message.
        let git = self.git().with_env(&[("GIT_MERGE_AUTOEDIT", "no")]);
        let result = if matches!(req.action, Action::Split | Action::Push) {
            let sink = crate::process::scope_progress();
            git.run_with_progress(&args, &mut |line| {
                if let Some(sink) = &sink {
                    sink(&request::progress_line(line));
                }
            })
        } else {
            git.run_raw(&args, None).and_then(|out| {
                if out.code == 0 {
                    Ok(out)
                } else {
                    Err(GitError::Failed {
                        args: args.join(" "),
                        code: out.code,
                        stderr: out.stderr,
                    })
                }
            })
        };
        let out = match result {
            Ok(out) => out,
            Err(GitError::Failed { stderr, .. }) if is_missing(&stderr) => {
                KNOWN_MISSING.store(true, Ordering::Relaxed);
                return Err(SubtreeMissing.into());
            }
            Err(e) if !matches!(e, GitError::Cancelled)
                && req.commits()
                && self.state() == RepoState::Merging =>
            {
                return Ok(Outcome {
                    message: format!(
                        "Conflicts merging into {prefix}/ — resolve them, stage, and commit to \
                         finish the merge (nothing was pushed)"
                    ),
                    conflicts: true,
                    split: None,
                });
            }
            Err(e) => return Err(e.into()),
        };
        let stdout = out.stdout_str();
        let mut note = String::new();
        if let Err(e) = self.remember_source(req, &prefix) {
            note = format!(" (could not remember the source: {e:#})");
        }
        let split = matches!(req.action, Action::Split | Action::Push)
            .then(|| last_object_id(&stdout))
            .flatten();
        let r = req.reference.trim();
        let on = self.head().ok().and_then(|h| h.branch).unwrap_or_else(|| "HEAD".into());
        let up_to_date = stdout.contains("Already up to date") || out.stderr.contains("already at commit");
        let message = match req.action {
            Action::Add => format!("Added {r} as subtree {prefix}/ on {on} — push {on} to share it"),
            Action::Pull | Action::Merge if up_to_date => format!("Subtree {prefix}/ is already up to date with {r}"),
            Action::Pull | Action::Merge => format!("Merged {r} into {prefix}/ on {on} — push {on} to share it"),
            Action::Push => format!(
                "Pushed the history of {prefix}/ to {r} of {}{}",
                req.repository.trim(),
                if req.rejoin { format!(" (rejoined on {on} — push {on} too)") } else { String::new() }
            ),
            Action::Split => {
                let id = split.map(short).unwrap_or_else(|| "?".into());
                match req.branch.trim() {
                    "" => format!("Split {prefix}/ into commit {id}"),
                    b => format!("Split {prefix}/ into commit {id} — branch {b}"),
                }
            }
        };
        Ok(Outcome {
            message: message + &note,
            conflicts: false,
            split,
        })
    }

    /// After an add / pull / push with a repository: remember it (and the branch) for the
    /// next time.
    fn remember_source(&self, req: &Request, prefix: &str) -> Result<()> {
        let repo = req.repository.trim();
        let r = req.reference.trim();
        let mut src = self.subtree_source(prefix).unwrap_or_default();
        match req.action {
            Action::Add | Action::Pull if !repo.is_empty() => {
                src.repository = repo.to_string();
                src.reference = r.to_string();
            }
            Action::Push => {
                if src.repository.is_empty() {
                    src.repository = repo.to_string();
                }
                if src.repository == repo {
                    if src.reference.is_empty() {
                        src.reference = r.to_string();
                    }
                    src.push_ref = if r == src.reference { String::new() } else { r.to_string() };
                }
            }
            _ => return Ok(()),
        }
        if self.subtree_source(prefix).as_ref() == Some(&src) {
            return Ok(());
        }
        self.set_subtree_source(prefix, &src)
    }

    /// Whether commits are signed by the user's config (`commit.gpgSign`).
    fn signs_commits(&self) -> bool {
        self.gix()
            .ok()
            .and_then(|r| r.config_snapshot().boolean("commit.gpgSign"))
            .unwrap_or(false)
    }

    /// Creates branch `name` at `id` (a split's result).
    pub fn create_branch_at(&self, name: &str, id: ObjectId) -> Result<String> {
        let out = self.git().run(&["branch", name, &id.to_string()])?;
        Ok(out.stderr)
    }

    pub(crate) fn subtree_cache(&self) -> &std::sync::Mutex<Vec<detect::Scan>> {
        self.statics_subtrees()
    }
}

/// The last full object id printed on its own line (split's result).
fn last_object_id(stdout: &str) -> Option<ObjectId> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find_map(|l| ObjectId::from_hex(l.as_bytes()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn split_output_ends_with_the_commit() {
        let id = "0123456789abcdef0123456789abcdef01234567";
        let out = format!("Created branch 'x'\n{id}\n");
        assert_eq!(last_object_id(&out).unwrap().to_string(), id);
        assert_eq!(last_object_id("nothing\n"), None);
    }

    #[gitti_test::test]
    fn suggested_folders() {
        assert_eq!(default_prefix("https://example.com/team/lib.git", "main"), "lib");
        assert_eq!(default_prefix("", "origin/tools"), "tools");
        assert_eq!(default_prefix(".", "upstream"), "upstream");
        assert_eq!(default_prefix("", ""), "");
    }

    #[gitti_test::test]
    fn a_missing_subtree_command_is_recognised() {
        assert!(is_missing("git: 'subtree' is not a git command. See 'git --help'."));
        assert!(!is_missing("fatal: prefix 'x' already exists."));
        assert!(MISSING.contains("git-subtree"));
    }
}
