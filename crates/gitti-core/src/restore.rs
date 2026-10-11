//! Restoring a file or folder as it is in a commit — "Restore This Version to the
//! Working Tree…", "Restore Version Before This Commit…" (a commit's file) and "Restore
//! File or Folder from This Commit…" (any path, folders too):
//! `git restore --source=<commit> --worktree [--staged] -- <path>`.
//!
//! git restores like this (no-overlay mode): every file the commit has under the path is
//! written, and tracked files under it that the commit doesn't have are deleted — the
//! path ends up as it was in that commit. With `--staged` the index gets the same
//! version (a staged change), without it the index stays (an unstaged change). Filters
//! run as on checkout: Git LFS files get their content (smudge), `eol` / `ident` /
//! custom filters apply. Paths go to git as literal pathspecs (`GIT_LITERAL_PATHSPECS`),
//! so `*`, `?`, `[` and a leading `:` in a name mean themselves.
//!
//! Before that, [`Repo::restore_plan`] reads what it would do — in-process from the
//! commit's tree and the index, plus one `git status` of the paths — so the dialog says
//! which files are written or deleted and which uncommitted changes are lost
//! ([`RestorePlan::lost`]; the apps ask for an explicit confirmation then). It refuses
//! (with what to do) when git-lfs is missing but LFS files would be written: they would
//! become pointer files instead of their content.
//!
//! Afterwards the apps re-read only the restored paths in the same background job, like
//! after a stage ([`RestorePlan::reread`]); a restore that writes rule files (or the whole
//! tree) gets the snapshot read instead.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use anyhow::{Result, anyhow, bail};
use gix::ObjectId;

use crate::Repo;
use crate::diff::{FileChange, FileDiff};
use crate::repo::gx;
use crate::version_file::VersionSide;

/// A commit file's menu entry: its version in the shown commit.
pub const THIS_LABEL: &str = "Restore This Version to the Working Tree…";
/// A commit file's menu entry: its version in the commit's parent (the old side).
pub const BEFORE_LABEL: &str = "Restore Version Before This Commit…";
/// A commit's menu entry: any path (a file or a folder) of it.
pub const PICK_LABEL: &str = "Restore File or Folder from This Commit…";
/// The same in the Commit menu / palettes (acts on the selected commit).
pub const MENU_LABEL: &str = "Restore File or Folder from Selected Commit…";

/// The dialog's choice: the working tree only, or the index too.
pub const STAGE_OPTIONS: [&str; 2] = ["Working Tree Only", "Working Tree and Index (Staged)"];
/// The path dialog's choice of version.
pub const VERSION_OPTIONS: [&str; 2] = ["This Commit's Version", "Version Before This Commit"];
/// The check box the dialog requires while uncommitted changes would be lost.
pub const OVERWRITE_CHECK: &str = "Overwrite my uncommitted changes to these files";
/// Paths listed per group; the rest is counted.
pub const SHOWN: usize = 20;

/// What to restore: `paths` as they are in `commit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreTarget {
    pub commit: ObjectId,
    pub paths: Vec<String>,
    /// How the dialog names the version: `abc1234`, or `abc1234 (the version before
    /// def5678)`.
    pub source: String,
}

impl RestoreTarget {
    /// `path` (a file or folder; `.` = everything) as it is in commit `id`, or (`before`)
    /// in its first parent `parent` — the path dialog of a commit. `Err` = why not.
    pub fn of_commit(
        id: ObjectId,
        parent: Option<ObjectId>,
        path: &str,
        before: bool,
    ) -> std::result::Result<Self, String> {
        let path = path.trim();
        if path.is_empty() {
            return Err("enter a file or folder path (. = everything)".into());
        }
        if !before {
            return Ok(RestoreTarget {
                commit: id,
                paths: vec![path.to_string()],
                source: short(&id),
            });
        }
        let Some(p) = parent else {
            return Err("this commit has no parent: there is no version before it".into());
        };
        Ok(RestoreTarget {
            commit: p,
            paths: vec![path.to_string()],
            source: format!("{} (the version before {})", short(&p), short(&id)),
        })
    }
}

fn short(id: &ObjectId) -> String {
    id.to_hex_with_len(7).to_string()
}

/// The version of `file` (of a commit or comparison view with `sides` = (new, old), see
/// [`crate::version_file::version_sides`]) to restore: the new side's (`before ==
/// false`) or the old side's. `Err` = why there is none: a file deleted here has no new
/// version, a root commit no old side, a stash is applied instead. The old version of a
/// renamed file is both paths (the old one comes back, the new one goes); of an added
/// or copied file, its path (restoring it deletes it: it didn't exist before).
pub fn restore_target(
    sides: (Option<VersionSide>, Option<VersionSide>),
    file: &FileDiff,
    before: bool,
) -> std::result::Result<RestoreTarget, String> {
    let (new, old) = sides;
    if new.is_some_and(|s| s.stash) {
        return Err("a stash: Apply Stash or Pop Stash restores its files".into());
    }
    let path = file.path().to_string();
    if !before {
        let Some(side) = new else {
            return Err("the new side is the working tree".into());
        };
        if file.change == FileChange::Deleted {
            return Err(format!(
                "{path} is deleted in this commit: Restore Version Before This Commit… brings it back"
            ));
        }
        return Ok(RestoreTarget {
            commit: side.commit,
            paths: vec![path],
            source: short(&side.commit),
        });
    }
    let Some(side) = old else {
        return Err("this commit has no parent: there is no version before it".into());
    };
    let paths = match (file.change, &file.old_path) {
        (FileChange::Renamed, Some(o)) if *o != path => vec![o.clone(), path],
        (FileChange::Deleted, Some(o)) => vec![o.clone()],
        _ => vec![path],
    };
    let source = match new {
        Some(n) => format!("{} (the version before {})", short(&side.commit), short(&n.commit)),
        None => short(&side.commit),
    };
    Ok(RestoreTarget {
        commit: side.commit,
        paths,
        source,
    })
}

/// An uncommitted change of a path the restore touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalChange {
    pub path: String,
    /// The index differs from HEAD.
    pub staged: bool,
    /// The working tree differs from the index.
    pub unstaged: bool,
    /// A file git doesn't track (untracked or ignored) where the commit has one.
    pub untracked: bool,
    pub conflicted: bool,
}

impl LocalChange {
    /// "modified", "staged and modified", "untracked or ignored", …
    pub fn describe(&self) -> &'static str {
        match (self.conflicted, self.untracked, self.staged, self.unstaged) {
            (true, ..) => "conflicted",
            (_, true, ..) => "untracked or ignored",
            (_, _, true, true) => "staged and modified",
            (_, _, true, false) => "staged",
            _ => "modified",
        }
    }
}

/// What restoring paths from a commit does, read before it runs ([`Repo::restore_plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
    pub commit: ObjectId,
    /// How the dialog names the version ([`RestoreTarget::source`]).
    pub source: String,
    /// The paths as given, relative to the top level (`""` = the whole tree).
    pub paths: Vec<String>,
    /// Files the commit has under the paths: written (sorted).
    pub written: Vec<String>,
    /// A path is a folder in the commit.
    pub folder: bool,
    /// Tracked files under the paths the commit doesn't have: deleted (sorted).
    pub deleted: Vec<String>,
    /// Uncommitted changes of written / deleted files (sorted by path).
    pub changes: Vec<LocalChange>,
    /// Submodules under the paths: their checked-out commit stays.
    pub submodules: usize,
    /// Git LFS files among the written ones.
    pub lfs_files: usize,
    /// git-lfs is installed (when not, LFS files would be written as pointers).
    pub lfs_installed: bool,
}

/// `path` relative to the top level, `/`-separated, without `./` or a trailing `/`;
/// `""` for the whole tree. `Err`: outside the repository.
fn normalize(workdir: &std::path::Path, path: &str) -> Result<String> {
    let mut p = path.trim().to_string();
    if cfg!(windows) {
        p = p.replace('\\', "/");
    }
    let as_path = std::path::Path::new(&p);
    if as_path.is_absolute() {
        let rel = as_path
            .strip_prefix(workdir)
            .or_else(|_| as_path.strip_prefix(crate::paths::canonical(workdir)))
            .map_err(|_| anyhow!("{path} is outside the repository"))?;
        p = rel.to_string_lossy().replace('\\', "/");
    }
    while let Some(rest) = p.strip_prefix("./") {
        p = rest.to_string();
    }
    let p = p.trim_end_matches('/');
    let p = if p == "." { "" } else { p };
    if p.split('/').any(|c| c == "..") {
        bail!("{path}: give a path inside the repository (no `..`)");
    }
    Ok(p.to_string())
}

/// Whether `file` is `path` or inside it (`""` = everything).
fn under(file: &str, path: &str) -> bool {
    path.is_empty()
        || file == path
        || (file.len() > path.len() && file.starts_with(path) && file.as_bytes()[path.len()] == b'/')
}

/// The pathspec for `path` (literal; `.` for the whole tree).
fn pathspec(path: &str) -> &str {
    if path.is_empty() { "." } else { path }
}

/// The files (blobs and links) of `tree` below `prefix` (its bytes) into `files`, shown
/// by `repo` ([`Repo::show_path`]: a name that is not UTF-8 is learned); submodules
/// counted.
fn tree_files(
    repo: &Repo,
    gix_repo: &gix::Repository,
    tree: ObjectId,
    prefix: &[u8],
    files: &mut Vec<(String, ObjectId)>,
    submodules: &mut usize,
) -> Result<()> {
    let tree = gix_repo.find_tree(tree).map_err(gx)?;
    let decoded = tree.decode().map_err(gx)?;
    for e in &decoded.entries {
        let path: Vec<u8> = if prefix.is_empty() {
            e.filename.to_vec()
        } else {
            [prefix, b"/", e.filename.as_ref()].concat()
        };
        if e.mode.is_tree() {
            tree_files(repo, gix_repo, e.oid.to_owned(), &path, files, submodules)?;
        } else if e.mode.is_commit() {
            *submodules += 1;
        } else {
            files.push((repo.show_path(&path), e.oid.to_owned()));
        }
    }
    Ok(())
}

impl Repo {
    /// What restoring `target` does (see the module docs): no change yet. One `git
    /// status` of the paths; the rest is read in-process.
    pub fn restore_plan(&self, target: &RestoreTarget) -> Result<RestorePlan> {
        let workdir = self.workdir().to_path_buf();
        let mut paths = Vec::new();
        for p in &target.paths {
            let p = normalize(&workdir, p)?;
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
        if paths.is_empty() {
            bail!("no path given");
        }
        let repo = self.gix()?;
        let commit = repo.find_commit(target.commit).map_err(gx)?;
        let tree = commit.tree().map_err(gx)?;
        let (mut files, mut submodules, mut folder) = (Vec::new(), 0, false);
        for p in &paths {
            if p.is_empty() {
                folder = true;
                tree_files(self, &repo, tree.id, b"", &mut files, &mut submodules)?;
                continue;
            }
            let Some(entry) = self.tree_entry(&tree, p)? else {
                continue;
            };
            let mode = entry.mode();
            if mode.is_tree() {
                folder = true;
                let prefix = self.path_bytes(p)?.into_owned();
                tree_files(self, &repo, entry.object_id(), &prefix, &mut files, &mut submodules)?;
            } else if mode.is_commit() {
                submodules += 1;
            } else {
                files.push((p.clone(), entry.object_id()));
            }
        }
        files.sort();
        files.dedup();
        let written: Vec<String> = files.iter().map(|(p, _)| p.clone()).collect();
        let in_commit: BTreeSet<&str> = written.iter().map(String::as_str).collect();
        let lfs_files = files
            .iter()
            .filter(|(_, id)| crate::lfs_health::is_pointer_blob(&repo, *id))
            .count();

        // Tracked files under the paths (the index), and those the commit doesn't have.
        let index = repo.index_or_empty().map_err(gx)?;
        let mut tracked = BTreeSet::new();
        for e in index.entries() {
            if e.mode == gix::index::entry::Mode::COMMIT {
                continue;
            }
            let path = self.show_path(e.path(&index));
            if paths.iter().any(|p| under(&path, p)) {
                tracked.insert(path);
            }
        }
        let deleted: Vec<String> = tracked
            .iter()
            .filter(|p| !in_commit.contains(p.as_str()))
            .cloned()
            .collect();
        if written.is_empty() && deleted.is_empty() {
            let what = paths
                .iter()
                .map(|p| pathspec(p))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "{what} is neither in {} nor tracked now: there is nothing to restore",
                short(&target.commit)
            );
        }

        // Uncommitted changes of the files it touches: tracked ones from git status,
        // untracked / ignored files where the commit has one from the file system.
        let mut args = vec!["status", "--porcelain=v2", "-z", "--untracked-files=no", "--"];
        args.extend(paths.iter().map(|p| pathspec(p)));
        let out = self.git_literal().run(&args)?;
        let touched = |p: &str| in_commit.contains(p) || deleted.binary_search_by(|d| d.as_str().cmp(p)).is_ok();
        let mut changes: Vec<LocalChange> = self.parse_status(&out.stdout)
            .entries
            .into_iter()
            .filter(|e| e.submodule.is_none() && touched(&e.path))
            .map(|e| LocalChange {
                staged: e.is_staged(),
                unstaged: e.worktree.is_some(),
                untracked: false,
                conflicted: e.conflicted,
                path: e.path,
            })
            .collect();
        for f in &written {
            if !tracked.contains(f)
                && std::fs::symlink_metadata(self.fs_path(f)).is_ok()
            {
                changes.push(LocalChange {
                    path: f.clone(),
                    staged: false,
                    unstaged: false,
                    untracked: true,
                    conflicted: false,
                });
            }
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(RestorePlan {
            commit: target.commit,
            source: target.source.clone(),
            paths,
            written,
            folder,
            deleted,
            changes,
            submodules,
            lfs_files,
            lfs_installed: crate::lfs::LfsCaps::get().installed(),
        })
    }

    /// Runs the restore `plan` describes: `git restore --source=<commit> --worktree
    /// [--staged] -- <paths>` (literal pathspecs).
    pub fn restore(&self, plan: &RestorePlan, staged: bool) -> Result<String> {
        if let Some(why) = plan.blocked() {
            bail!("{why}");
        }
        let source = format!("--source={}", plan.commit);
        let mut args = vec!["restore", source.as_str(), "--worktree"];
        if staged {
            args.push("--staged");
        }
        args.push("--");
        args.extend(plan.paths.iter().map(|p| pathspec(p)));
        self.git_literal().run(&args)?;
        Ok(plan.done(staged))
    }
}

/// What the apps read again after a restore ran ([`RestorePlan::reread`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reread {
    /// The working state of these paths only, in the restore's own background job (like a
    /// stage / discard: [`Repo::working_state_after`]; a folder stands for everything
    /// below it).
    Paths(Vec<String>),
    /// The snapshot (the whole working state and what the snapshot derives from the work
    /// tree) — not the history: a restore moves no ref.
    Snapshot,
}

impl RestorePlan {
    /// What to read again after this restore ran. A restore writes and deletes files
    /// under its paths only, in the work tree and (staged) the index, so re-reading those
    /// paths is enough ([`Repo::working_state_after`] itself falls back to a whole read
    /// where a rename shows up or an untracked folder folds) — except where a written or
    /// deleted file changes what git reports elsewhere:
    /// - the whole tree (`.`): every rule file may change, read everything;
    /// - a `.gitattributes` anywhere: whether the repository uses Git LFS (the
    ///   snapshot's LFS state) and the filters / eol of other files;
    /// - a `.gitmodules` anywhere: the submodule list (the snapshot's);
    /// - the top-level `.gitignore`: which files are untracked anywhere;
    /// - a nested `.gitignore`: its folder is re-read too (what is ignored below it).
    pub fn reread(&self) -> Reread {
        if self.paths.iter().any(String::is_empty) {
            return Reread::Snapshot;
        }
        let mut paths = self.paths.clone();
        for f in self.written.iter().chain(&self.deleted) {
            let (dir, name) = f.rsplit_once('/').unwrap_or(("", f.as_str()));
            match name {
                ".gitattributes" | ".gitmodules" => return Reread::Snapshot,
                ".gitignore" if dir.is_empty() => return Reread::Snapshot,
                ".gitignore" if !paths.iter().any(|p| under(dir, p)) => paths.push(dir.to_string()),
                _ => {}
            }
        }
        Reread::Paths(paths)
    }

    /// Why it can't run (the dialog's OK stays disabled with this reason).
    pub fn blocked(&self) -> Option<String> {
        (self.lfs_files > 0 && !self.lfs_installed).then(|| {
            format!(
                "git-lfs is not installed: {} would be written as a pointer file, not its \
                 content. Install git-lfs (git-lfs.com) and run git lfs install first.",
                count(self.lfs_files, "Git LFS file")
            )
        })
    }

    /// The uncommitted changes the restore overwrites: with `staged`, staged ones too
    /// (without, the index keeps them).
    pub fn lost(&self, staged: bool) -> Vec<&LocalChange> {
        self.changes
            .iter()
            .filter(|c| c.unstaged || c.untracked || c.conflicted || (staged && c.staged))
            .collect()
    }

    /// The dialog's title.
    pub fn title(&self) -> String {
        let at = short(&self.commit);
        match self.paths.as_slice() {
            [p] if p.is_empty() => format!("Restore the whole working tree from {at}"),
            [p] if self.folder => format!("Restore {p}/ from {at}"),
            [p] => format!("Restore {p} from {at}"),
            many => format!("Restore {} paths from {at}", many.len()),
        }
    }

    /// The OK button's label.
    pub fn ok_label(&self, staged: bool) -> &'static str {
        if staged { "Restore and Stage" } else { "Restore" }
    }

    /// The dialog text: what is written and deleted, what is lost, what follows.
    pub fn describe(&self, staged: bool) -> String {
        let mut s = String::new();
        let into = if staged {
            "the working tree and the index"
        } else {
            "the working tree"
        };
        let src = &self.source;
        match self.written.as_slice() {
            [] => {}
            [one] if !self.folder => {
                let _ = writeln!(s, "Writes {one} as it is in {src} into {into}.");
            }
            files => {
                let what = match self.paths.as_slice() {
                    [p] if p.is_empty() => "the whole tree".to_string(),
                    [p] => format!("{p}/"),
                    _ => "these paths".to_string(),
                };
                let _ = writeln!(
                    s,
                    "Writes the {} of {what} as they are in {src} into {into}.",
                    count(files.len(), "file")
                );
            }
        }
        let at = short(&self.commit);
        match self.deleted.as_slice() {
            [] => {}
            [one] if self.written.is_empty() && !self.folder => {
                let _ = writeln!(
                    s,
                    "Deletes {one} from {into}: {src} doesn't have it."
                );
            }
            files => {
                let _ = writeln!(
                    s,
                    "Deletes {} tracked there that {at} doesn't have:",
                    count(files.len(), "file")
                );
                list(&mut s, files.iter().map(String::as_str));
            }
        }
        s.push('\n');
        let lost = self.lost(staged);
        if lost.is_empty() {
            s.push_str("No uncommitted changes are overwritten.\n");
        } else {
            s.push_str("Your uncommitted changes to these files are lost:\n");
            let lines: Vec<String> = lost
                .iter()
                .map(|c| format!("{} ({})", c.path, c.describe()))
                .collect();
            list(&mut s, lines.iter().map(String::as_str));
        }
        if !staged && self.changes.iter().any(|c| c.staged) {
            s.push_str("Staged changes stay in the index.\n");
        }
        if self.submodules > 0 {
            let _ = writeln!(
                s,
                "{} inside keep{} their checked-out commit.",
                count(self.submodules, "submodule"),
                if self.submodules == 1 { "s" } else { "" }
            );
        }
        if self.lfs_files > 0 && self.lfs_installed {
            let _ = writeln!(
                s,
                "{}: git-lfs writes their content (and downloads what isn't here yet).",
                count(self.lfs_files, "Git LFS file")
            );
        }
        s.push('\n');
        s.push_str(if staged {
            "Nothing is committed: afterwards the restored version shows as a staged change — \
             commit it to keep it."
        } else {
            "Nothing is committed: afterwards the restored version shows as an unstaged change \
             (where it differs from the index) — stage and commit it to keep it, or discard it \
             to go back."
        });
        s
    }

    /// The status line after it ran.
    pub fn done(&self, staged: bool) -> String {
        let what = match self.paths.as_slice() {
            [p] if p.is_empty() => "the whole tree".to_string(),
            [p] if self.folder => format!("{p}/"),
            [p] => p.clone(),
            many => count(many.len(), "path"),
        };
        let next = if staged {
            "staged, commit to keep it"
        } else {
            "unstaged, stage and commit to keep it"
        };
        format!("Restored {what} from {} — {next}", short(&self.commit))
    }
}

/// "1 file", "3 files".
fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// Indented lines, at most [`SHOWN`], then "… and N more".
fn list<'a>(s: &mut String, items: impl ExactSizeIterator<Item = &'a str>) {
    let n = items.len();
    for line in items.take(SHOWN) {
        let _ = writeln!(s, "  {line}");
    }
    if n > SHOWN {
        let _ = writeln!(s, "  … and {} more", n - SHOWN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn paths_are_normalized() {
        let wd = std::path::Path::new(if cfg!(windows) { r"C:\r" } else { "/r" });
        assert_eq!(normalize(wd, "./src/").unwrap(), "src");
        assert_eq!(normalize(wd, ".").unwrap(), "");
        assert_eq!(normalize(wd, " a b ").unwrap(), "a b");
        let abs = wd.join("x").join("y");
        assert_eq!(normalize(wd, &abs.to_string_lossy()).unwrap(), "x/y");
        assert!(normalize(wd, "a/../b").is_err());
        assert!(under("src/a", "src") && under("src", "src") && under("x", ""));
        assert!(!under("srcx/a", "src") && !under("sr", "src"));
    }

    fn plan() -> RestorePlan {
        RestorePlan {
            commit: ObjectId::null(gix::hash::Kind::Sha1),
            source: "0000000".into(),
            paths: vec!["d".into()],
            written: vec!["d/a".into(), "d/b".into()],
            folder: true,
            deleted: vec!["d/c".into()],
            changes: vec![
                LocalChange {
                    path: "d/a".into(),
                    staged: true,
                    unstaged: false,
                    untracked: false,
                    conflicted: false,
                },
                LocalChange {
                    path: "d/b".into(),
                    staged: false,
                    unstaged: false,
                    untracked: true,
                    conflicted: false,
                },
            ],
            submodules: 0,
            lfs_files: 0,
            lfs_installed: true,
        }
    }

    #[gitti_test::test]
    fn staged_changes_are_lost_only_with_the_index() {
        let p = plan();
        let paths = |v: Vec<&LocalChange>| v.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths(p.lost(false)), ["d/b"]);
        assert_eq!(paths(p.lost(true)), ["d/a", "d/b"]);
        let text = p.describe(false);
        assert!(text.starts_with("Writes the 2 files of d/ as they are in 0000000 into the working tree.\n"), "{text}");
        assert!(text.contains("Deletes 1 file tracked there that 0000000 doesn't have:\n  d/c\n"), "{text}");
        assert!(text.contains("lost:\n  d/b (untracked or ignored)\nStaged changes stay in the index."), "{text}");
        assert!(text.ends_with("or discard it to go back."), "{text}");
        let staged = p.describe(true);
        assert!(staged.contains("  d/a (staged)\n") && staged.contains("the working tree and the index"));
        assert_eq!(p.title(), "Restore d/ from 0000000");
        assert_eq!(p.done(true), "Restored d/ from 0000000 — staged, commit to keep it");
    }

    #[gitti_test::test]
    fn rereads_its_paths_unless_rule_files_change() {
        let mut p = plan();
        assert_eq!(p.reread(), Reread::Paths(vec!["d".into()]));
        // A nested `.gitignore`: its folder too (here inside `d` already).
        p.written.push("d/e/.gitignore".into());
        assert_eq!(p.reread(), Reread::Paths(vec!["d".into()]));
        p.paths = vec!["x/.gitignore".into()];
        p.written = vec!["x/.gitignore".into()];
        p.deleted.clear();
        assert_eq!(p.reread(), Reread::Paths(vec!["x/.gitignore".into(), "x".into()]));
        for rules in [".gitignore", ".gitattributes", "x/.gitattributes", "x/.gitmodules"] {
            p.paths = vec![rules.into()];
            p.written.clear();
            p.deleted = vec![rules.into()];
            assert_eq!(p.reread(), Reread::Snapshot, "{rules}");
        }
        p.paths = vec!["".into()];
        p.deleted = vec!["a".into()];
        assert_eq!(p.reread(), Reread::Snapshot);
    }

    #[gitti_test::test]
    fn lfs_files_without_git_lfs_are_refused() {
        let mut p = plan();
        assert_eq!(p.blocked(), None);
        p.lfs_files = 2;
        assert!(p.describe(false).contains("2 Git LFS files: git-lfs writes their content"));
        p.lfs_installed = false;
        let why = p.blocked().unwrap();
        assert!(why.starts_with("git-lfs is not installed: 2 Git LFS files would be written"), "{why}");
    }
}
