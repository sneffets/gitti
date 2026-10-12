//! "Clean Untracked / Ignored Files…" (both apps: the Repository menu / palette): deletes
//! what `git clean` deletes — untracked files, ignored files or both — from a list the user
//! ticks first.
//!
//! **The list** ([`Repo::clean_list`]) is a dry run, read with two git processes: one
//! `git status --ignored=matching --untracked-files=normal` (folders collapsed as git
//! shows them: a new folder is one entry, an ignored folder like `target/` is one entry)
//! and one `git check-ignore -v --stdin -z` naming the rule that ignores each ignored
//! entry (`.gitignore:3: target/`, `.git/info/exclude:1: .env`, `~/.config/git/ignore:2:
//! *.swp`). A folder entry is walked in-process: how many files it holds, and whether a
//! folder with a `.git` of its own is in it (git doesn't look into an ignored folder, nor
//! into a new one shown as one entry, so its `status` can't say).
//!
//! **Ticked by default**: untracked entries yes, ignored ones no — they are build output,
//! but just as often local settings or secrets (`.env`) nobody wants gone by accident.
//!
//! **Nested repositories** — a folder with a `.git` of its own: a separate clone, a
//! worktree (of this repository or another), the checkout of a submodule no longer
//! registered — are entries of their own, never ticked by default and never deleted as
//! part of a folder around them: `git clean` keeps them too (without `-ff`). Ticked, they
//! go with their history ([`crate::discard`] does the same for Discard).
//!
//! **Deleting** ([`Repo::clean_chosen`]) is done in-process like Discard's untracked files
//! (`crate::discard`): a progress meter over the files, every failure collected instead
//! of stopping, cancellable, one entry in the command log naming what was deleted. What is
//! deleted is gone — no trash. A file inside a ticked folder that is an entry of its own
//! (an ignored file in a new folder, a nested repository) is left to its own tick.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;

use crate::Repo;
use crate::discard::{Meter, count, remove_file, short_list};

pub const MENU_LABEL: &str = "Clean Untracked / Ignored Files…";
/// The palette's detail of [`MENU_LABEL`].
pub const PALETTE_DETAIL: &str = "git clean — delete untracked and / or ignored files, picked from a list";
pub const TITLE: &str = "Clean Untracked / Ignored Files";
/// What the apps say when the list is empty.
pub const NOTHING: &str = "Nothing to clean: no untracked or ignored files";

/// Whether an entry is untracked or ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Untracked,
    Ignored,
}

/// What the dialog lists (its options, in this order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Untracked,
    Ignored,
    Both,
}

impl Choice {
    pub const ALL: [Choice; 3] = [Choice::Untracked, Choice::Ignored, Choice::Both];

    pub fn includes(self, kind: Kind) -> bool {
        match self {
            Choice::Untracked => kind == Kind::Untracked,
            Choice::Ignored => kind == Kind::Ignored,
            Choice::Both => true,
        }
    }

    /// Its place in [`Self::ALL`] (the option's index).
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    pub fn from_index(ix: usize) -> Self {
        Self::ALL.get(ix).copied().unwrap_or(Choice::Untracked)
    }
}

/// What a folder with a `.git` of its own is, from that `.git`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nested {
    /// A `.git` folder (a clone, `git init`), or a `.git` file pointing elsewhere.
    Repository,
    /// A `.git` file pointing into a repository's `worktrees/` folder.
    Worktree,
    /// A `.git` file pointing into a repository's `modules/` folder (a submodule's
    /// checkout that isn't registered here).
    Submodule,
}

impl Nested {
    /// Folder `dir`'s kind when it has a `.git` (folder, file or link), else `None`.
    pub fn of(dir: &Path) -> Option<Self> {
        let dotgit = dir.join(".git");
        let meta = std::fs::symlink_metadata(&dotgit).ok()?;
        if meta.is_dir() {
            return Some(Nested::Repository);
        }
        let text = std::fs::read_to_string(&dotgit).unwrap_or_default();
        let target = text
            .strip_prefix("gitdir:")
            .map(|t| t.trim().replace('\\', "/"))
            .unwrap_or_default();
        Some(if target.contains("/worktrees/") {
            Nested::Worktree
        } else if target.contains("/modules/") {
            Nested::Submodule
        } else {
            Nested::Repository
        })
    }

    /// In a word or two ("separate repository").
    pub fn describe(self) -> &'static str {
        match self {
            Nested::Repository => "separate repository",
            Nested::Worktree => "worktree",
            Nested::Submodule => "submodule checkout",
        }
    }
}

/// One line of the list: a file or a folder (`dir/`) as git shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Repository-relative, `/`-separated; a folder ends with `/`.
    pub path: String,
    /// Where it is on disk (a name that is not UTF-8 by its bytes).
    pub disk: PathBuf,
    pub kind: Kind,
    /// Files Clean deletes for it: 1 for a file; a folder's, without what is an entry of
    /// its own inside it; 0 for a nested repository (not counted: it goes whole).
    pub files: usize,
    /// It has a `.git` of its own: deleted only when ticked itself, with its history.
    pub nested: Option<Nested>,
    /// The rule that ignores it, as `<file>:<line>: <pattern>` (ignored entries).
    pub rule: Option<String>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.path.ends_with('/')
    }

    /// Ticked when the dialog opens: untracked entries, but no nested repository.
    pub fn ticked(&self) -> bool {
        self.kind == Kind::Untracked && self.nested.is_none()
    }

    /// The check box's label: the path, what it is, why it is ignored.
    pub fn label(&self) -> String {
        let mut s = self.path.clone();
        if let Some(n) = self.nested {
            s.push_str(&format!(" — {} (own .git)", n.describe()));
        } else if self.is_dir() {
            s.push_str(&format!(" ({})", count(self.files, "file")));
        }
        if let Some(rule) = &self.rule {
            s.push_str(&format!(" — ignored by {rule}"));
        }
        s
    }
}

/// The dialog's live answer for the ticked entries: a sentence, OK's label, why OK can't
/// run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Live {
    pub text: String,
    pub ok_label: String,
    pub blocked: Option<String>,
}

/// What Clean can delete ([`Repo::clean_list`]), sorted by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanList {
    pub entries: Vec<Entry>,
}

impl CleanList {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries of `kind`.
    pub fn of_kind(&self, kind: Kind) -> usize {
        self.entries.iter().filter(|e| e.kind == kind).count()
    }

    /// The option the dialog starts with: untracked files, or the ignored ones when there
    /// are none.
    pub fn default_choice(&self) -> Choice {
        if self.of_kind(Kind::Untracked) > 0 {
            Choice::Untracked
        } else {
            Choice::Ignored
        }
    }

    /// The options' labels, with their counts: `Untracked (3)`.
    pub fn choice_label(&self, choice: Choice) -> String {
        let (name, n) = match choice {
            Choice::Untracked => ("Untracked", self.of_kind(Kind::Untracked)),
            Choice::Ignored => ("Ignored", self.of_kind(Kind::Ignored)),
            Choice::Both => ("Untracked and Ignored", self.entries.len()),
        };
        format!("{name} ({n})")
    }

    /// The indices of the entries `choice` shows.
    pub fn shown(&self, choice: Choice) -> Vec<usize> {
        (0..self.entries.len())
            .filter(|&i| choice.includes(self.entries[i].kind))
            .collect()
    }

    /// The ticks the dialog opens with, one per entry ([`Entry::ticked`]).
    pub fn default_ticks(&self) -> Vec<bool> {
        self.entries.iter().map(Entry::ticked).collect()
    }

    /// The indices of the entries Clean deletes: ticked (`ticks`, one per entry) and
    /// shown by `choice`.
    pub fn chosen(&self, choice: Choice, ticks: &[bool]) -> Vec<usize> {
        self.shown(choice)
            .into_iter()
            .filter(|&i| ticks.get(i).copied().unwrap_or(false))
            .collect()
    }

    /// The dialog's text: what Clean does, what is ticked and why, that nothing comes
    /// back.
    pub fn message(&self) -> String {
        let mut s = String::from(
            "Deletes files git doesn't track. Deleted files are gone for good: they don't go \
             to the trash, and git can't bring them back.\n",
        );
        s.push_str("• Untracked: new files never added to git. All are ticked.\n");
        s.push_str(
            "• Ignored: files your ignore rules hide (.gitignore, .git/info/exclude, \
             core.excludesFile — each entry names its rule). Often build output, but also \
             local settings or secrets such as .env: none is ticked, tick what you want gone.\n",
        );
        if self.entries.iter().any(|e| e.nested.is_some()) {
            s.push_str(
                "• A folder with a .git of its own (a separate repository, a worktree) is \
                 deleted only when ticked itself — with its history and everything not \
                 pushed from it. A ticked folder around it leaves it alone.\n",
            );
        }
        s.push_str("Tracked files and staged changes are not touched.");
        s
    }

    /// What OK will do with `ticks` (one per entry) under `choice`.
    pub fn live(&self, choice: Choice, ticks: &[bool]) -> Live {
        let chosen = self.chosen(choice, ticks);
        if chosen.is_empty() {
            let blocked = if self.shown(choice).is_empty() {
                match choice {
                    Choice::Untracked => "There are no untracked files",
                    Choice::Ignored => "There are no ignored files",
                    Choice::Both => NOTHING,
                }
            } else {
                "Nothing ticked: tick what to delete"
            };
            return Live {
                text: String::new(),
                ok_label: "Delete".into(),
                blocked: Some(blocked.into()),
            };
        }
        let (mut untracked, mut ignored) = (0, 0);
        let mut nested = Vec::new();
        for &i in &chosen {
            let e = &self.entries[i];
            match (e.nested, e.kind) {
                (Some(n), _) => nested.push((e.path.clone(), n)),
                (None, Kind::Untracked) => untracked += e.files,
                (None, Kind::Ignored) => ignored += e.files,
            }
        }
        let mut parts = Vec::new();
        if untracked > 0 {
            parts.push(count(untracked, "untracked file"));
        }
        if ignored > 0 {
            parts.push(count(ignored, "ignored file"));
        }
        match nested.as_slice() {
            [] => {}
            [(path, n)] => parts.push(format!("the {} {path} with its history", n.describe())),
            many => {
                let paths: Vec<String> = many.iter().map(|(p, _)| p.clone()).collect();
                parts.push(format!(
                    "{} folders with a .git of their own, with their history ({})",
                    many.len(),
                    short_list(&paths, 3)
                ))
            }
        }
        let text = format!("Deletes {}. This can't be undone.", join_and(&parts));
        let files = untracked + ignored;
        let repos = match nested.len() {
            1 => "1 Repository".to_string(),
            n => format!("{n} Repositories"),
        };
        let ok_label = match (files, nested.len()) {
            (_, 0) => format!("Delete {}", count(files, "File")),
            (0, _) => format!("Delete {repos}"),
            _ => format!("Delete {} and {repos}", count(files, "File")),
        };
        Live {
            text,
            ok_label,
            blocked: None,
        }
    }
}

/// `a`, `a and b`, `a, b and c`.
fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What a clean did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanReport {
    /// Files deleted (nested repositories' files included).
    pub files: usize,
    /// Chosen entries that are gone now.
    pub deleted: Vec<String>,
    /// Folders with a `.git` of their own found inside a chosen folder that weren't in the
    /// list (new since it was read): kept.
    pub kept: Vec<String>,
    /// Files (or folders that couldn't be read) not deleted, and why.
    pub failed: Vec<(String, String)>,
}

impl CleanReport {
    /// The status line after a clean.
    pub fn note(&self) -> String {
        let mut s = match self.deleted.as_slice() {
            [] => format!("Cleaned: deleted {}", count(self.files, "file")),
            names => format!(
                "Cleaned: deleted {} ({})",
                count(self.files, "file"),
                short_list(names, 3)
            ),
        };
        if !self.kept.is_empty() {
            s.push_str(&format!(
                "; kept {}: a folder with a .git of its own (a separate repository) — \
                 Clean deletes one only when it is ticked itself",
                short_list(&self.kept, 3)
            ));
        }
        s
    }
}

/// Some files could not be deleted; everything else was.
#[derive(Debug, Clone)]
pub struct CleanFailed(pub CleanReport);

impl std::fmt::Display for CleanFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let failed = &self.0.failed;
        write!(f, "{} could not be deleted", count(failed.len(), "file"))?;
        for (path, why) in failed.iter().take(10) {
            write!(f, "\n  {path}: {}", why.trim())?;
        }
        if failed.len() > 10 {
            write!(f, "\n  … and {} more", failed.len() - 10)?;
        }
        write!(f, "\n{}", self.0.note())
    }
}

impl std::error::Error for CleanFailed {}

/// What a walk of a folder found.
#[derive(Default)]
struct Walk {
    files: Vec<PathBuf>,
    /// Folders, each after the folders in it.
    dirs: Vec<PathBuf>,
    /// Folders with a `.git` of their own (not walked).
    nested: Vec<PathBuf>,
    /// Folders that couldn't be read.
    errors: Vec<(PathBuf, String)>,
}

/// Walks folder `dir` without following links: its files, its folders (deepest first),
/// what is in `skip` left out; with `stop_at_nested` a folder with a `.git` is noted, not
/// walked.
fn walk(dir: &Path, skip: &HashSet<PathBuf>, stop_at_nested: bool, w: &mut Walk) {
    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            w.errors.push((dir.to_path_buf(), e.to_string()));
            return;
        }
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if skip.contains(&path) {
            continue;
        }
        let is_dir = ent.file_type().is_ok_and(|t| t.is_dir());
        if !is_dir {
            w.files.push(path);
            continue;
        }
        if stop_at_nested && Nested::of(&path).is_some() {
            w.nested.push(path);
            continue;
        }
        walk(&path, skip, stop_at_nested, w);
        w.dirs.push(path);
    }
}

impl Repo {
    /// The dry run of Clean (see the module docs): every untracked and ignored entry as
    /// git shows it, each folder's file count, the rule of each ignored entry, and the
    /// folders with a `.git` of their own — also those inside a listed folder. Two git
    /// processes (one without ignored entries).
    pub fn clean_list(&self) -> Result<CleanList> {
        let out = self.git().run(&[
            "status",
            "--porcelain=v2",
            "-z",
            "--ignored=matching",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ])?;
        // Learns the names that are not UTF-8 (`fs_path` finds them by their bytes).
        self.parse_status(&out.stdout);
        let mut entries = Vec::new();
        for rec in String::from_utf8_lossy(&out.stdout).split('\0') {
            let kind = match rec.as_bytes().first() {
                Some(b'?') => Kind::Untracked,
                Some(b'!') => Kind::Ignored,
                _ => continue,
            };
            let Some(path) = rec.get(2..).filter(|p| !p.is_empty()) else {
                continue;
            };
            entries.push(Entry {
                path: path.to_string(),
                disk: self.fs_path(path.trim_end_matches('/')),
                kind,
                files: 1,
                nested: None,
                rule: None,
            });
        }
        self.add_ignore_rules(&mut entries)?;
        // Folders: their files, and folders with a `.git` in them.
        let listed: HashSet<PathBuf> = entries.iter().map(|e| e.disk.clone()).collect();
        let mut found = Vec::new();
        for e in entries.iter_mut().filter(|e| e.is_dir()) {
            if let Some(n) = Nested::of(&e.disk) {
                e.nested = Some(n);
                e.files = 0;
                continue;
            }
            let mut w = Walk::default();
            walk(&e.disk, &listed, true, &mut w);
            e.files = w.files.len();
            for dir in w.nested {
                found.push(Entry {
                    path: format!("{}/", self.repo_relative(&dir)),
                    nested: Nested::of(&dir),
                    disk: dir,
                    kind: e.kind,
                    files: 0,
                    rule: e.rule.clone(),
                });
            }
        }
        // A folder git shows for a repository inside it only (`vendor/` for `vendor/lib/`)
        // holds nothing Clean would delete: the repository is the entry.
        entries.retain(|e| !e.is_dir() || e.nested.is_some() || e.files > 0);
        entries.extend(found);
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(CleanList { entries })
    }

    /// Fills in the rule of each ignored entry: one `git check-ignore -v --stdin -z`.
    fn add_ignore_rules(&self, entries: &mut [Entry]) -> Result<()> {
        let ignored: Vec<&str> = entries
            .iter()
            .filter(|e| e.kind == Kind::Ignored)
            .map(|e| e.path.as_str())
            .collect();
        if ignored.is_empty() {
            return Ok(());
        }
        let list = self.pathspec_list(&ignored)?;
        // Exit code 1: none of them is ignored (any more).
        let out = self
            .git()
            .run_raw(&["check-ignore", "-v", "--stdin", "-z"], Some(&list))?;
        if out.code > 1 {
            anyhow::bail!("git check-ignore failed: {}", out.stderr.trim());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let fields: Vec<&str> = text.split('\0').collect();
        let home = dirs::home_dir();
        for &[source, line, pattern, path] in fields.as_chunks::<4>().0 {
            if source.is_empty() {
                continue;
            }
            let source = match &home {
                Some(h) => match Path::new(source).strip_prefix(h) {
                    Ok(rest) => format!("~/{}", rest.to_string_lossy().replace('\\', "/")),
                    Err(_) => source.to_string(),
                },
                None => source.to_string(),
            };
            let rule = format!("{source}:{line}: {pattern}");
            if let Some(e) = entries
                .iter_mut()
                .find(|e| e.kind == Kind::Ignored && e.path == path)
            {
                e.rule = Some(rule);
            }
        }
        Ok(())
    }

    /// `path` (inside the work tree) as a repository path: `/`-separated.
    fn repo_relative(&self, path: &Path) -> String {
        let rel = path.strip_prefix(self.workdir()).unwrap_or(path);
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Deletes the entries `chosen` (indices into `list.entries`) in-process: their files
    /// with a progress meter, then the folders that are empty now. An entry inside a
    /// chosen folder is left to its own tick, and a folder with a `.git` found there that
    /// wasn't listed is kept (and named). Fails with [`CleanFailed`] (after doing all the
    /// rest) when some files could not be deleted.
    pub fn clean_chosen(&self, list: &CleanList, chosen: &[usize]) -> Result<CleanReport> {
        let start = Instant::now();
        let mut report = CleanReport::default();
        let listed: HashSet<PathBuf> = list.entries.iter().map(|e| e.disk.clone()).collect();
        let entries: Vec<&Entry> = chosen.iter().filter_map(|&i| list.entries.get(i)).collect();
        let mut w = Walk::default();
        // Files whose emptied folders go too (up to the work tree).
        let mut lone_files = Vec::new();
        for e in &entries {
            if !e.is_dir() {
                w.files.push(e.disk.clone());
                lone_files.push(e.disk.clone());
                continue;
            }
            if e.nested.is_some() {
                // The whole repository, its `.git` included.
                walk(&e.disk, &HashSet::new(), false, &mut w);
            } else if Nested::of(&e.disk).is_some() {
                // It got a `.git` since the list was read.
                report.kept.push(e.path.clone());
                continue;
            } else {
                walk(&e.disk, &listed, true, &mut w);
            }
            w.dirs.push(e.disk.clone());
        }
        for dir in std::mem::take(&mut w.nested) {
            if !listed.contains(&dir) {
                report.kept.push(format!("{}/", self.repo_relative(&dir)));
            }
        }
        for (dir, why) in std::mem::take(&mut w.errors) {
            report.failed.push((format!("{}/", self.repo_relative(&dir)), why));
        }
        let total = w.files.len();
        let mut meter = Meter::new("Deleting files", total);
        let mut cancelled = false;
        for (i, file) in w.files.iter().enumerate() {
            if crate::process::scope_cancelled() {
                cancelled = true;
                break;
            }
            meter.at(i);
            match remove_file(file) {
                Ok(()) => report.files += 1,
                Err(e) => report.failed.push((self.repo_relative(file), e.to_string())),
            }
        }
        if !cancelled {
            meter.at(total);
        }
        // Deepest first; a folder something was left in stays.
        w.dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for dir in &w.dirs {
            let _ = std::fs::remove_dir(dir);
        }
        for file in &lone_files {
            let mut dir = file.parent();
            while let Some(d) = dir.filter(|d| *d != self.workdir() && d.starts_with(self.workdir())) {
                if std::fs::remove_dir(d).is_err() {
                    break; // not empty (or gone)
                }
                dir = d.parent();
            }
        }
        report.deleted = entries
            .iter()
            .filter(|e| std::fs::symlink_metadata(&e.disk).is_err())
            .map(|e| e.path.clone())
            .collect();
        let mut what = format!("clean: delete {}", count(total, "file"));
        if cancelled {
            what.push_str(&format!(" (cancelled after {})", report.files));
        }
        let mut output = String::new();
        for (path, why) in &report.failed {
            output.push_str(&format!("failed: {path}: {why}\n"));
        }
        for path in &report.kept {
            output.push_str(&format!("kept {path} (it has a .git of its own)\n"));
        }
        for e in &entries {
            let state = if report.deleted.contains(&e.path) { "deleted" } else { "partly deleted" };
            output.push_str(&format!("{state} {}\n", e.label()));
        }
        self.git()
            .log_in_process(&what, start, report.failed.is_empty() && !cancelled, &output);
        if cancelled {
            return Err(crate::git_cli::GitError::Cancelled.into());
        }
        if !report.failed.is_empty() {
            return Err(CleanFailed(report).into());
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, kind: Kind, files: usize) -> Entry {
        Entry {
            path: path.into(),
            disk: PathBuf::from(path),
            kind,
            files,
            nested: None,
            rule: (kind == Kind::Ignored).then(|| ".gitignore:1: x".into()),
        }
    }

    fn list() -> CleanList {
        let mut nested = entry("lib/", Kind::Untracked, 0);
        nested.nested = Some(Nested::Repository);
        CleanList {
            entries: vec![
                entry("a.txt", Kind::Untracked, 1),
                entry("app.log", Kind::Ignored, 1),
                nested,
                entry("new/", Kind::Untracked, 3),
                entry("target/", Kind::Ignored, 120),
            ],
        }
    }

    #[gitti_test::test]
    fn ticks_choices_and_live_text() {
        let l = list();
        assert_eq!(l.default_ticks(), [true, false, false, true, false]);
        assert_eq!(l.default_choice(), Choice::Untracked);
        assert_eq!(l.choice_label(Choice::Untracked), "Untracked (3)");
        assert_eq!(l.choice_label(Choice::Ignored), "Ignored (2)");
        assert_eq!(l.choice_label(Choice::Both), "Untracked and Ignored (5)");
        assert_eq!(l.shown(Choice::Ignored), [1, 4]);
        let live = l.live(Choice::Untracked, &l.default_ticks());
        assert_eq!(live.text, "Deletes 4 untracked files. This can't be undone.");
        assert_eq!(live.ok_label, "Delete 4 Files");
        // Ticks of entries the choice doesn't show don't count.
        let ticks = [true, true, true, false, true];
        assert_eq!(l.chosen(Choice::Ignored, &ticks), [1, 4]);
        let live = l.live(Choice::Both, &ticks);
        assert_eq!(
            live.text,
            "Deletes 1 untracked file, 121 ignored files and the separate repository lib/ with its \
             history. \
             This can't be undone."
        );
        assert_eq!(live.ok_label, "Delete 122 Files and 1 Repository");
        let none = l.live(Choice::Ignored, &[true, false, false, true, false]);
        assert_eq!(none.blocked.as_deref(), Some("Nothing ticked: tick what to delete"));
        let empty = CleanList::default();
        assert_eq!(empty.live(Choice::Both, &[]).blocked.as_deref(), Some(NOTHING));
        assert_eq!(empty.default_choice(), Choice::Ignored);
    }

    #[gitti_test::test]
    fn labels_say_what_an_entry_is() {
        let l = list();
        let labels: Vec<String> = l.entries.iter().map(Entry::label).collect();
        assert_eq!(
            labels,
            [
                "a.txt",
                "app.log — ignored by .gitignore:1: x",
                "lib/ — separate repository (own .git)",
                "new/ (3 files)",
                "target/ (120 files) — ignored by .gitignore:1: x",
            ]
        );
        assert!(l.message().contains("don't go to the trash"));
        assert!(l.message().contains("A folder with a .git of its own"));
        let report = CleanReport {
            files: 124,
            deleted: vec!["a.txt".into(), "new/".into(), "target/".into(), "x".into()],
            kept: vec!["target/wt/".into()],
            failed: Vec::new(),
        };
        assert_eq!(
            report.note(),
            "Cleaned: deleted 124 files (a.txt, new/, target/ and 1 more); kept target/wt/: a \
             folder with a .git of its own (a separate repository) — Clean deletes one only \
             when it is ticked itself"
        );
    }
}
