//! Discarding unstaged changes (Discard on a file, Discard All, a group's Discard): tracked
//! files go back to their staged / committed state, untracked files are deleted.
//!
//! In one go and reliably for any number of files: the untracked files are deleted
//! in-process (a git process per file would take minutes for a `target/` folder of 10 000
//! files), with a progress meter for the status bar and one entry in the command log
//! (the Git Log shows what was deleted, though no git ran); tracked files are restored by
//! one `git restore` that reads the paths from stdin (no command-line limit). A file that
//! can't be deleted doesn't stop the others: every failure is collected and reported at
//! the end ([`DiscardFailed`]).
//!
//! An untracked folder that is a git repository of its own (it has a `.git`: a cloned
//! project, a worktree of another repository) is not deleted unless asked for
//! ([`DiscardOptions::delete_nested_repos`]) — its history and whatever was never pushed
//! from it would go with it; `git clean` keeps it too. The report names it
//! ([`DiscardReport::nested_kept`]) so the frontends say why it is still listed instead of
//! looking as if Discard had worked only partly.

use std::path::Path;
use std::time::Instant;

use anyhow::Result;

use crate::Repo;
use crate::status::StatusEntry;

/// How [`Repo::discard_with`] treats what plain discarding leaves alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscardOptions {
    /// Untracked folders that are git repositories of their own are deleted as well
    /// (`git clean -ff`): their history and everything not pushed from them is lost.
    pub delete_nested_repos: bool,
}

/// What a discard did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscardReport {
    /// Tracked files restored (submodules moved back included).
    pub restored: usize,
    /// Untracked files and folders deleted.
    pub deleted: usize,
    /// Untracked folders kept because they are git repositories of their own (`x/`).
    pub nested_kept: Vec<String>,
    /// Paths (or a group of them) that could not be discarded, and why.
    pub failed: Vec<(String, String)>,
}

impl DiscardReport {
    /// What the status line says besides "discarded" when something was kept on purpose
    /// (`None`: everything asked for is gone).
    pub fn note(&self) -> Option<String> {
        let kept = &self.nested_kept;
        match kept.len() {
            0 => None,
            1 => Some(format!(
                "{} was kept: it is a separate git repository (it has its own .git) — \
                 Discard deletes it only when asked to (its history would be lost)",
                kept[0]
            )),
            n => Some(format!(
                "{n} folders were kept: they are separate git repositories (own .git: {}) — \
                 Discard deletes them only when asked to (their history would be lost)",
                short_list(kept, 3)
            )),
        }
    }
}

/// Some files could not be discarded; everything else was. The report says what was done.
#[derive(Debug, Clone)]
pub struct DiscardFailed(pub DiscardReport);

impl std::fmt::Display for DiscardFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let failed = &self.0.failed;
        write!(f, "{} could not be discarded", count(failed.len(), "file"))?;
        for (path, why) in failed.iter().take(10) {
            write!(f, "\n  {path}: {}", why.trim())?;
        }
        if failed.len() > 10 {
            write!(f, "\n  … and {} more", failed.len() - 10)?;
        }
        if let Some(note) = self.0.note() {
            write!(f, "\n{note}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DiscardFailed {}

/// `1 file`, `3 files`.
pub fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// `a/, b/, c/ and 2 more`.
fn short_list(items: &[String], max: usize) -> String {
    let mut s = items.iter().take(max).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > max {
        s.push_str(&format!(" and {} more", items.len() - max));
    }
    s
}

/// Whether `entry` is an untracked folder that is a git repository of its own (a `.git`
/// folder or file in it). `git status` lists such a folder as one entry (`x/`) even with
/// `--untracked-files=all`.
pub fn is_nested_repo(workdir: &Path, entry: &StatusEntry) -> bool {
    entry.untracked
        && entry.path.ends_with('/')
        && std::fs::symlink_metadata(crate::paths::join_repo_path(workdir, &entry.path).join(".git"))
            .is_ok()
}

/// What a nested repository's entry shows instead of a diff, in a few words (one row,
/// short enough for a narrow pane).
pub const NESTED_REPO_NOTE: &str = "Nested repository — Discard keeps it";

/// [`NESTED_REPO_NOTE`] in full (gitti's tooltip, gitty's Enter): what it is, what Discard
/// does with it, what to do instead.
pub const NESTED_REPO_EXPLANATION: &str = "A git repository of its own inside this one (it has      a .git: a clone, a worktree of another repository): git doesn't look into it, so it shows      as one untracked folder. Discard keeps it unless you choose to delete it too — that      deletes its history and everything not pushed from it. To record it here, add it as a      submodule (Add Submodule…); to stop seeing it, add it to .gitignore.";

/// An untracked folder entry (`dir/`, `status.showUntrackedFiles=normal`) that isn't a
/// repository: in a few words, and in full.
pub const UNTRACKED_FOLDER_NOTE: &str = "Untracked folder — Discard deletes it";
pub const UNTRACKED_FOLDER_EXPLANATION: &str = "An untracked folder: git lists it as one entry      (status.showUntrackedFiles=normal) instead of each file in it. Discard deletes it with its      files (ignored files and a git repository inside it stay); Stage adds every file in it.";

/// What the diff pane says instead of a diff for a working-directory entry that has none
/// because of what it is — (one row, in full) — or `None` (a file too large or one of too
/// many to preview: the frontends say that).
pub fn no_preview_note(ws: &crate::WorkingState, wf: &crate::WorkFile) -> Option<(&'static str, &'static str)> {
    if !wf.entry.untracked || !wf.path().ends_with('/') {
        return None;
    }
    Some(if ws.is_nested_repo(wf.path()) {
        (NESTED_REPO_NOTE, NESTED_REPO_EXPLANATION)
    } else {
        (UNTRACKED_FOLDER_NOTE, UNTRACKED_FOLDER_EXPLANATION)
    })
}

/// The entries of `entries` that [`is_nested_repo`].
pub fn nested_repos<'a>(
    workdir: &Path,
    entries: impl IntoIterator<Item = &'a StatusEntry>,
) -> Vec<String> {
    entries
        .into_iter()
        .filter(|e| is_nested_repo(workdir, e))
        .map(|e| e.path.clone())
        .collect()
}

/// What a discard of some entries will do, for its confirmation (both frontends say the
/// same).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscardPlan {
    /// Tracked files reset to their staged / committed state (submodules included).
    pub tracked: usize,
    /// Untracked files (and new folders) deleted.
    pub untracked: usize,
    /// Untracked folders that are git repositories of their own: kept unless asked.
    pub nested: Vec<String>,
}

impl DiscardPlan {
    pub fn of<'a>(workdir: &Path, entries: impl IntoIterator<Item = &'a StatusEntry>) -> Self {
        let mut plan = Self::default();
        for e in entries {
            if is_nested_repo(workdir, e) {
                plan.nested.push(e.path.clone());
            } else if e.untracked {
                plan.untracked += 1;
            } else {
                plan.tracked += 1;
            }
        }
        plan
    }

    /// The confirmation's text: what happens to which files, what is kept.
    pub fn detail(&self) -> String {
        let mut s = String::new();
        if self.tracked > 0 {
            s.push_str(&format!(
                "{} will be reset to the staged / committed state.\n",
                count(self.tracked, "tracked file")
            ));
        }
        if self.untracked > 0 {
            s.push_str(&format!(
                "{} will be deleted (ignored files stay).\n",
                count(self.untracked, "untracked file")
            ));
        }
        if !self.nested.is_empty() {
            s.push_str(&format!(
                "{} {} separate git {} (own .git): {}. Kept unless you choose to delete {} \
                 too — that deletes {} history and everything not pushed from {}.\n",
                count(self.nested.len(), "folder"),
                if self.nested.len() == 1 { "is a" } else { "are" },
                if self.nested.len() == 1 { "repository" } else { "repositories" },
                short_list(&self.nested, 3),
                if self.nested.len() == 1 { "it" } else { "them" },
                if self.nested.len() == 1 { "its" } else { "their" },
                if self.nested.len() == 1 { "it" } else { "them" },
            ));
        }
        s.push_str("Staged changes are kept. This cannot be undone.");
        s
    }

    /// The label of the box that deletes the nested repositories as well.
    pub fn nested_check(&self) -> String {
        match self.nested.len() {
            1 => format!("Also delete the repository {} (and its history)", self.nested[0]),
            n => format!("Also delete the {n} nested repositories (and their history)"),
        }
    }
}

/// The status line after a discard: `done`, and what was kept on purpose.
pub fn done_note(done: &str, report: &DiscardReport) -> String {
    match report.note() {
        Some(note) => format!("{done}. {note}"),
        None => done.to_string(),
    }
}

/// Feeds a progress meter (`Deleting untracked files:  45% (450/1000)`) to the
/// operation's progress sink, once per percent.
struct Meter {
    sink: Option<crate::process::ProgressSink>,
    phase: &'static str,
    total: usize,
    shown: Option<usize>,
}

impl Meter {
    fn new(phase: &'static str, total: usize) -> Self {
        Self {
            sink: crate::process::scope_progress(),
            phase,
            total,
            shown: None,
        }
    }

    fn at(&mut self, done: usize) {
        let Some(sink) = &self.sink else {
            return;
        };
        let percent = (done * 100).checked_div(self.total).unwrap_or(100);
        if self.shown == Some(percent) && done < self.total {
            return;
        }
        self.shown = Some(percent);
        let end = if done >= self.total { ", done." } else { "" };
        sink(&format!(
            "{}: {percent:>3}% ({done}/{}){end}",
            self.phase, self.total
        ));
    }
}

impl Repo {
    /// Discards worktree changes of tracked files and deletes untracked ones; nested
    /// repositories are kept (see the module docs). Untracked files go first, and
    /// directories they leave empty with them: a tracked directory replaced by an
    /// untracked file of the same name (or a file by a directory) can only come back once
    /// that is out of the way. A submodule goes back to its recorded commit (`git
    /// submodule update`); uncommitted changes inside it are left alone, as git does.
    ///
    /// Fails with [`DiscardFailed`] (after doing all the rest) when some files could not be
    /// discarded.
    pub fn discard_paths(&self, entries: &[&StatusEntry]) -> Result<DiscardReport> {
        self.discard_with(entries, DiscardOptions::default())
    }

    /// [`Self::discard_paths`] with options.
    pub fn discard_with(
        &self,
        entries: &[&StatusEntry],
        opts: DiscardOptions,
    ) -> Result<DiscardReport> {
        let mut report = DiscardReport::default();
        if entries.len() > 1
            && let Some(sink) = crate::process::scope_progress()
        {
            sink(&format!("Discarding {}", count(entries.len(), "file")));
        }
        // Untracked folder entries: nested repositories, and with
        // `status.showUntrackedFiles=normal` whole new folders (`dir/`).
        let (mut nested, mut folders) = (Vec::new(), Vec::new());
        for e in entries.iter().filter(|e| e.untracked && e.path.ends_with('/')) {
            if is_nested_repo(self.workdir(), e) {
                nested.push(e.path.as_str());
            } else {
                folders.push(e.path.as_str());
            }
        }
        if opts.delete_nested_repos {
            self.clean(&nested, true, &mut report);
        } else {
            report.nested_kept.extend(nested.iter().map(|p| p.to_string()));
        }
        // `git clean` deletes the untracked files in a folder entry — not the ignored ones
        // (a plain recursive delete would) and not a nested repository in it.
        self.clean(&folders, false, &mut report);
        if !folders.is_empty() {
            self.nested_left_in(&folders, opts, &mut report);
        }
        self.delete_untracked_files(entries, &mut report)?;
        let is_sub = |e: &StatusEntry| e.submodule.is_some() && !e.untracked;
        let moved: Vec<&str> = entries
            .iter()
            .filter(|e| is_sub(e) && e.submodule.is_some_and(|f| f.commit_changed))
            .map(|e| e.path.as_str())
            .collect();
        if !moved.is_empty() {
            match self.update_submodules(&moved) {
                Ok(_) => report.restored += moved.len(),
                Err(e) => report.failed.push((moved.join(", "), format!("{e:#}"))),
            }
        }
        let tracked: Vec<&str> = entries
            .iter()
            .filter(|e| !e.untracked && !is_sub(e))
            .map(|e| e.path.as_str())
            .collect();
        if !tracked.is_empty() {
            if tracked.len() > 1
                && let Some(sink) = crate::process::scope_progress()
            {
                sink(&format!("Restoring {}", count(tracked.len(), "tracked file")));
            }
            // One git for any number of paths: they come on stdin (a command line is
            // limited — Windows: 32k characters).
            // A name that is not UTF-8 by its bytes (`crate::raw_names`).
            let list = self.pathspec_list(&tracked)?;
            let args = [
                "restore",
                "--worktree",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ];
            match self.git_literal().run_stdin(&args, Some(&list)) {
                Ok(_) => report.restored += tracked.len(),
                Err(e) if matches!(e, crate::git_cli::GitError::Cancelled) => return Err(e.into()),
                Err(e) => {
                    let what = match tracked.as_slice() {
                        [one] => one.to_string(),
                        many => count(many.len(), "tracked file"),
                    };
                    report.failed.push((what, e.to_string()));
                }
            }
        }
        if !report.failed.is_empty() {
            return Err(DiscardFailed(report).into());
        }
        Ok(report)
    }

    /// `git clean -f -d` (`-ff` = nested repositories too) of `paths`, 200 per call.
    fn clean(&self, paths: &[&str], nested: bool, report: &mut DiscardReport) {
        let git = self.git_literal();
        for chunk in paths.chunks(200) {
            let mut args = vec!["clean", if nested { "-ff" } else { "-f" }, "-d", "-q", "--"];
            args.extend_from_slice(chunk);
            match git.run(&args) {
                Ok(_) => report.deleted += chunk.len(),
                Err(e) => report.failed.push((chunk.join(", "), e.to_string())),
            }
        }
    }

    /// Folder entries `git clean` could not delete entirely: nested repositories in them
    /// (one `git status` of those folders; ignored files left in them are fine).
    fn nested_left_in(&self, folders: &[&str], opts: DiscardOptions, report: &mut DiscardReport) {
        let left: Vec<String> = folders
            .iter()
            .filter(|f| self.fs_path(f).exists())
            .map(|f| format!(":(literal){f}"))
            .collect();
        if left.is_empty() {
            return;
        }
        // `all`: a repository in a folder shows as its own entry (`dir/sub/`), not as
        // `dir/`.
        let mut args = vec!["status", "--porcelain=v2", "-z", "--untracked-files=all", "--"];
        args.extend(left.iter().map(String::as_str));
        let Ok(out) = self.git().run(&args) else {
            return;
        };
        let status = crate::status::parse_porcelain_v2(&out.stdout);
        let nested = nested_repos(self.workdir(), &status.entries);
        if opts.delete_nested_repos {
            let paths: Vec<&str> = nested.iter().map(String::as_str).collect();
            self.clean(&paths, true, report);
        } else {
            report.nested_kept.extend(nested);
        }
    }

    /// Deletes the untracked files of `entries` in-process — with a progress meter, every
    /// failure collected, one command-log entry — and the folders they leave empty.
    fn delete_untracked_files(
        &self,
        entries: &[&StatusEntry],
        report: &mut DiscardReport,
    ) -> Result<()> {
        let files: Vec<&str> = entries
            .iter()
            .filter(|e| e.untracked && !e.path.ends_with('/'))
            .map(|e| e.path.as_str())
            .collect();
        if files.is_empty() {
            return Ok(());
        }
        let start = Instant::now();
        let mut meter = Meter::new("Deleting untracked files", files.len());
        let mut deleted = Vec::new();
        let mut failed = Vec::new();
        let mut cancelled = false;
        for (i, path) in files.iter().enumerate() {
            if crate::process::scope_cancelled() {
                cancelled = true;
                break;
            }
            meter.at(i);
            // A name that is not UTF-8 by its bytes.
            let full = self.fs_path(path);
            match remove_file(&full) {
                Ok(()) => deleted.push(*path),
                Err(err) => {
                    failed.push((path.to_string(), err.to_string()));
                    continue;
                }
            }
            let mut dir = full.parent();
            while let Some(d) = dir.filter(|d| *d != self.workdir() && d.starts_with(self.workdir())) {
                if std::fs::remove_dir(d).is_err() {
                    break; // not empty (or gone)
                }
                dir = d.parent();
            }
        }
        if !cancelled {
            meter.at(files.len());
        }
        let what = match (files.as_slice(), cancelled) {
            ([one], _) => format!("delete untracked file {one}"),
            (_, false) => format!("delete {}", count(files.len(), "untracked file")),
            (_, true) => format!(
                "delete {} (cancelled after {})",
                count(files.len(), "untracked file"),
                deleted.len()
            ),
        };
        let mut output = String::new();
        for (path, why) in &failed {
            output.push_str(&format!("failed: {path}: {why}\n"));
        }
        if files.len() > 1 {
            for path in &deleted {
                output.push_str(&format!("deleted {path}\n"));
            }
        }
        self.git()
            .log_in_process(&what, start, failed.is_empty() && !cancelled, &output);
        report.deleted += deleted.len();
        report.failed.extend(failed);
        if cancelled {
            return Err(crate::git_cli::GitError::Cancelled.into());
        }
        Ok(())
    }
}

/// Deletes one file (or symlink); gone already is fine. A read-only file (Windows refuses
/// to delete one) is made writable first; a symlink to a folder on Windows is a directory
/// entry.
fn remove_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            let Ok(meta) = std::fs::symlink_metadata(path) else {
                return Err(e);
            };
            if meta.file_type().is_symlink() && std::fs::remove_dir(path).is_ok() {
                return Ok(());
            }
            #[cfg(windows)]
            if meta.permissions().readonly() {
                let mut perms = meta.permissions();
                perms.set_readonly(false);
                if std::fs::set_permissions(path, perms).is_ok() {
                    return std::fs::remove_file(path);
                }
            }
            Err(e)
        }
    }
}
