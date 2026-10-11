//! The paths of a commit, a comparison or a stash that this work tree's file system can't
//! hold — told *before* a checkout, switch, cherry-pick, revert or stash apply fails or
//! collides. Decided from the diff's paths and the index's paths already read
//! ([`PathReport::index`]): no git, no tree walk, so navigating the history costs nothing
//! more.
//!
//! What counts: every path the diff leaves (not the deleted ones) against the same rules
//! as the index's ([`check`]: its own name — Windows names, characters, `.git` aliases,
//! length, Unicode form, symbolic links — and collisions among the diff's paths), and,
//! where names fold, a path the diff *adds* (added, renamed or copied to) whose name
//! another path of the work tree already has in another spelling (`xt_TCPMSS.h` next to
//! `xt_tcpmss.h`, a file `docs` where `Docs/` is a folder). A changed path that collides
//! in the index already keeps that collision. Paths the diff removes (deleted, renamed
//! away) are not in the way.

use std::collections::{HashMap, HashSet};

use super::{Entry, FsRules, NoSymlinks, PathIssue, PathReport, Problem, check, fold, how};
use crate::diff::{FileChange, FileDiff};

/// What can be done about such paths of a commit (the last explanation line).
pub const CHANGE_WHAT_TO_DO: &str = "To work with it: use a file system that takes it (e.g. a \
     case-sensitive volume), or leave it out with git sparse-checkout before checking the \
     commit out.";

/// The index's paths as a folding file system sees them ([`PathReport::index`]).
#[derive(Default, PartialEq, Eq)]
pub struct IndexPaths {
    /// Folded path → the index's spelling (the first one when several fold alike).
    files: HashMap<Box<str>, Box<str>>,
    /// Folded folder (without the trailing `/`) → its spelling.
    dirs: HashMap<Box<str>, Box<str>>,
}

/// Without its contents: a working state's debug text (tests compare them) doesn't depend
/// on hash order or on which read of the index it carries.
impl std::fmt::Debug for IndexPaths {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IndexPaths(..)")
    }
}

impl IndexPaths {
    pub(crate) fn new<'a>(paths: impl Iterator<Item = &'a str>, rules: &FsRules) -> Self {
        let mut out = IndexPaths::default();
        for path in paths {
            let key = fold(path, rules);
            let ascii = path.is_ascii();
            for (at, _) in path.match_indices('/') {
                // ASCII folds byte for byte; other names fold their prefix on their own.
                let dir_key = if ascii {
                    key[..at].to_string()
                } else {
                    fold(&path[..at], rules)
                };
                if out.dirs.contains_key(dir_key.as_str()) {
                    continue;
                }
                out.dirs.insert(dir_key.into(), path[..at].into());
            }
            out.files.entry(key.into()).or_insert_with(|| path.into());
        }
        out
    }

    /// The index's spelling of the file `key` (folded) names.
    fn file(&self, key: &str) -> Option<&str> {
        self.files.get(key).map(|p| &**p)
    }

    /// The index's spelling of the folder `key` (folded) names.
    fn dir(&self, key: &str) -> Option<&str> {
        self.dirs.get(key).map(|p| &**p)
    }
}

/// A symbolic link after the change (`new file mode 120000`, `new mode 120000`, `index
/// a..b 120000`).
fn is_symlink(f: &FileDiff) -> bool {
    f.header.iter().any(|l| {
        (l.starts_with("new file mode ") || l.starts_with("new mode ") || l.starts_with("index "))
            && l.ends_with(" 120000")
    })
}

impl PathReport {
    /// The paths of `files` (a commit's, comparison's or stash's diff) this file system
    /// can't hold, with this report's rules; sorted by path, each with its first problem.
    /// Their explanation is [`Problem::change_lines`].
    pub fn of_changes(&self, files: &[FileDiff]) -> PathReport {
        let rules = self.rules;
        let present: Vec<&FileDiff> = files
            .iter()
            .filter(|f| f.change != FileChange::Deleted && f.new_path.is_some())
            .collect();
        let entries: Vec<Entry<'_>> = present
            .iter()
            .map(|f| Entry { path: f.path(), symlink: is_symlink(f), hidden: false })
            .collect();
        let mut issues = check(&entries, &rules);
        if rules.folds() {
            let removed: HashSet<&str> = files
                .iter()
                .filter(|f| matches!(f.change, FileChange::Deleted | FileChange::Renamed))
                .filter_map(|f| f.old_path.as_deref())
                .collect();
            let mut more = Vec::new();
            for f in &present {
                let path = f.path();
                if issues.iter().any(|i| i.path == path) {
                    continue;
                }
                if let Some(problem) = self.collision_with_index(f, &removed) {
                    more.push(PathIssue { path: path.to_string(), problem, hidden: false });
                }
            }
            issues.extend(more);
            issues.sort_by(|a, b| a.path.cmp(&b.path));
        }
        PathReport::new(rules, issues)
    }

    /// The collision of `f`'s path with a path of the work tree (the index) that the
    /// change doesn't remove (`removed`).
    fn collision_with_index(&self, f: &FileDiff, removed: &HashSet<&str>) -> Option<Problem> {
        let path = f.path();
        let kept = |other: &str| !removed.contains(other);
        // A path the index has already: its collision there, if any.
        if let Some(i) = self.issue(path) {
            return match &i.problem {
                Problem::Collides { other, .. } if kept(other.trim_end_matches('/')) => {
                    Some(i.problem.clone())
                }
                _ => None,
            };
        }
        let adds = matches!(f.change, FileChange::Added | FileChange::Renamed | FileChange::Copied);
        let index = self.index.as_deref()?;
        if !adds {
            return None;
        }
        let rules = &self.rules;
        let key = fold(path, rules);
        if let Some(other) = index.file(&key).filter(|o| *o != path && kept(o)) {
            return Some(Problem::Collides { other: other.to_string(), by: how(path, other) });
        }
        // A file where the work tree has a folder of another spelling.
        if let Some(dir) = index.dir(&key).filter(|d| *d != path) {
            return Some(Problem::Collides { other: format!("{dir}/"), by: how(path, dir) });
        }
        // A folder of this path where the work tree has a file.
        for (at, _) in path.match_indices('/') {
            let prefix = &path[..at];
            if let Some(other) = index
                .file(&fold(prefix, rules))
                .filter(|o| *o != prefix && kept(o))
            {
                return Some(Problem::Collides { other: other.to_string(), by: how(prefix, other) });
            }
        }
        None
    }
}

impl Problem {
    /// The explanation under the file header of a commit's (comparison's, stash's) file
    /// whose path this file system can't hold: the reason, what checking the commit out or
    /// applying it does, what to do. One line per row.
    pub fn change_lines(&self, rules: &FsRules) -> Vec<String> {
        let first = format!("Can't be checked out on this file system: {}.", self.why(rules));
        let follows = match self {
            Problem::Collides { other, .. } if other.ends_with('/') => {
                "Only one of the two fits on disk: checking out this commit, or cherry-picking, \
                 reverting or applying it here, fails or leaves the other showing as deleted."
            }
            Problem::Collides { .. } => {
                "Both are one file on disk: checking out this commit, or cherry-picking, \
                 reverting or applying it here, writes one over the other — the other then \
                 shows as modified right away, or git refuses to overwrite it."
            }
            Problem::Decomposed => {
                "Checked out here, it shows as deleted and its composed name as untracked."
            }
            Problem::TooLong { .. } => {
                "git can't create it: checking out, cherry-picking or applying this commit \
                 fails for it unless long paths are on (git config core.longpaths true, \
                 Windows 10 or later)."
            }
            Problem::Symlink(NoSymlinks::Config) => {
                "It is checked out as a small text file holding the link's target."
            }
            _ => {
                "git can't create it: checking out this commit (or a branch that has it), \
                 cherry-picking, reverting or applying it here fails with \"invalid path\"."
            }
        };
        vec![first, follows.to_string(), CHANGE_WHAT_TO_DO.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_paths::Fold;

    fn rules(f: impl FnOnce(&mut FsRules)) -> FsRules {
        let mut r = FsRules::default();
        f(&mut r);
        r
    }

    fn report(index: &[&str], r: FsRules) -> PathReport {
        let entries: Vec<Entry<'_>> =
            index.iter().map(|p| Entry { path: p, symlink: false, hidden: false }).collect();
        let mut rep = PathReport::new(r, check(&entries, &r));
        rep.index = Some(std::sync::Arc::new(IndexPaths::new(index.iter().copied(), &r)));
        rep
    }

    fn file(change: FileChange, old: Option<&str>, new: Option<&str>) -> FileDiff {
        FileDiff {
            old_path: old.map(String::from),
            new_path: new.map(String::from),
            change,
            binary: false,
            header: Vec::new(),
            hunks: Vec::new(),
            submodule: None,
            too_large: None,
            encoding: Default::default(),
            lossy_name: false,
            raw_paths: Vec::new(),
            lfs: None,
        }
    }

    fn added(p: &str) -> FileDiff {
        file(FileChange::Added, None, Some(p))
    }

    fn found(rep: &PathReport) -> Vec<(&str, &Problem)> {
        rep.issues.iter().map(|i| (i.path.as_str(), &i.problem)).collect()
    }

    #[gitti_test::test]
    fn an_added_path_collides_with_the_work_trees_other_spelling() {
        let ci = rules(|r| r.case_insensitive = true);
        let rep = report(&["net/xt_TCPMSS.h", "src/a.rs", "Docs/a.md", "lib"], ci);
        let got = rep.of_changes(&[
            added("net/xt_tcpmss.h"),
            added("docs"),
            added("LIB/x.rs"),
            added("src/b.rs"),
            file(FileChange::Modified, Some("src/a.rs"), Some("src/a.rs")),
        ]);
        assert_eq!(
            found(&got),
            vec![
                ("LIB/x.rs", &Problem::Collides { other: "lib".into(), by: Fold::Case }),
                ("docs", &Problem::Collides { other: "Docs/".into(), by: Fold::Case }),
                ("net/xt_tcpmss.h", &Problem::Collides { other: "net/xt_TCPMSS.h".into(), by: Fold::Case }),
            ]
        );
        // On a case-sensitive file system nothing collides.
        let plain = report(&["net/xt_TCPMSS.h"], FsRules::default());
        assert!(plain.of_changes(&[added("net/xt_tcpmss.h")]).issues.is_empty());
    }

    #[gitti_test::test]
    fn what_the_change_removes_is_not_in_the_way() {
        let ci = rules(|r| r.case_insensitive = true);
        let rep = report(&["README", "old/x"], ci);
        // A case-only rename: README goes, readme comes.
        let rename = file(FileChange::Renamed, Some("README"), Some("readme"));
        assert!(rep.of_changes(&[rename]).issues.is_empty());
        // A modification of an old spelling isn't a new path: no false alarm.
        let old = file(FileChange::Modified, Some("Readme"), Some("Readme"));
        assert!(rep.of_changes(&[old]).issues.is_empty());
        // Deleted paths are never checked.
        let gone = file(FileChange::Deleted, Some("aux.c"), None);
        let win = report(&[], rules(|r| r.windows_reserved = true));
        assert!(win.of_changes(&[gone]).issues.is_empty());
    }

    #[gitti_test::test]
    fn the_changes_own_names_and_pairs() {
        let r = rules(|r| {
            r.case_insensitive = true;
            r.windows_reserved = true;
            r.windows_chars = true;
        });
        let rep = report(&["a.c"], r);
        let got = rep.of_changes(&[
            added("drivers/aux.c"),
            added("q?.txt"),
            added("X.h"),
            added("x.h"),
            file(FileChange::Modified, Some("a.c"), Some("a.c")),
        ]);
        let paths: Vec<&str> = got.issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["X.h", "drivers/aux.c", "q?.txt", "x.h"]);
        assert_eq!(got.issue("drivers/aux.c").unwrap().problem, Problem::Reserved { name: "aux.c".into() });
        // A pair already in the index: a change of one of them keeps its collision.
        let both = report(&["X.h", "x.h"], r);
        let m = both.of_changes(&[file(FileChange::Modified, Some("x.h"), Some("x.h"))]);
        assert_eq!(m.issue("x.h").unwrap().problem, Problem::Collides { other: "X.h".into(), by: Fold::Case });
        // … unless the change deletes the other one.
        let m = both.of_changes(&[
            file(FileChange::Modified, Some("x.h"), Some("x.h")),
            file(FileChange::Deleted, Some("X.h"), None),
        ]);
        assert!(m.issues.is_empty(), "{:?}", m.issues);
    }

    #[gitti_test::test]
    fn symlinks_where_the_file_system_has_none() {
        let mut link = added("link");
        link.header = vec!["diff --git a/link b/link".into(), "new file mode 120000".into()];
        let none = report(&[], rules(|r| r.no_symlinks = Some(NoSymlinks::FileSystem)));
        let got = none.of_changes(std::slice::from_ref(&link));
        assert_eq!(got.issues[0].problem, Problem::Symlink(NoSymlinks::FileSystem));
        // Checked out as text files (core.symlinks=false): no mark.
        let config = report(&[], rules(|r| r.no_symlinks = Some(NoSymlinks::Config)));
        assert!(config.of_changes(&[link]).issues.is_empty());
    }

    #[gitti_test::test]
    fn change_lines_say_what_checking_out_does() {
        let r = rules(|r| r.case_insensitive = true);
        let p = Problem::Collides { other: "net/xt_TCPMSS.h".into(), by: Fold::Case };
        let lines = p.change_lines(&r);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("Can't be checked out on this file system: it differs only in case from net/xt_TCPMSS.h"), "{lines:?}");
        assert!(lines[1].contains("cherry-picking"), "{lines:?}");
        assert_eq!(lines[2], CHANGE_WHAT_TO_DO);
        let lines = Problem::Reserved { name: "aux.c".into() }.change_lines(&r);
        assert!(lines[1].contains("invalid path"), "{lines:?}");
    }

    #[gitti_test::test]
    fn index_paths_fold_files_and_folders() {
        let r = rules(|r| {
            r.case_insensitive = true;
            r.unicode_insensitive = true;
        });
        let idx = IndexPaths::new(["Src/Main.rs", "caf\u{e9}/x"].into_iter(), &r);
        assert_eq!(idx.file("src/main.rs"), Some("Src/Main.rs"));
        assert_eq!(idx.dir("src"), Some("Src"));
        assert_eq!(idx.dir(&fold("cafe\u{301}", &r)), Some("caf\u{e9}"));
    }
}
