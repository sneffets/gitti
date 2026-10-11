//! "Renormalize Line Endings…" (both apps: the Repository menu / palette, and the
//! context menu of a `.gitattributes` file): applies the current line-ending rules — the
//! `text` / `eol` attributes of `.gitattributes` and `core.autocrlf` — to every tracked
//! file again and stages the result (`git add --renormalize`).
//!
//! git doesn't do that by itself when the rules change: a file stored with CRLF stays so,
//! and it usually doesn't even show as changed (with `text=auto` / `core.autocrlf` git
//! keeps CRLF that is already in the index; otherwise only files whose timestamp changed
//! are looked at again). What git does, checked against git 2.54:
//!
//! - `--renormalize` implies `-u`: every tracked file is added again from the working tree
//!   — a file with unstaged edits gets them staged too. [`Plan`] names those files and
//!   they are left out unless the user includes them.
//! - A tracked file deleted in the working tree makes git stop ("unable to stat") without
//!   staging anything: such files are always left out (there is nothing to renormalize).
//! - The working-tree files are not rewritten: they keep their bytes until git writes them
//!   again (a checkout, switching branches). Since the index now holds what the rules make
//!   of them, they show no unstaged change.
//! - Untracked files (a new `.gitattributes` among them) are not staged.
//! - Conflicted files would be marked resolved: refused while there are any.
//! - Submodules (gitlinks) have no line endings: left out, so a submodule's new commit is
//!   not staged on the side.

use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::discard::count;
use crate::repo::{Repo, WorkingState, gx};

/// The menu and palette label.
pub const MENU_LABEL: &str = "Renormalize Line Endings…";
/// The dialog's title.
pub const TITLE: &str = "Renormalize line endings?";
/// The dialog's confirm button.
pub const BUTTON: &str = "Renormalize";
/// The refusal while conflicts are unresolved.
pub const CONFLICTS: &str = "Renormalize Line Endings stages every tracked file, which would \
    mark the conflicted files as resolved: resolve them first.";

/// What Renormalize Line Endings will act on, from the working state the app shows (no
/// git): what the dialog has to say beyond the general text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Tracked files with unstaged changes: renormalizing re-adds them from the working
    /// tree, which stages those changes too — left out unless the user includes them.
    pub changed: Vec<String>,
    /// Tracked files deleted in the working tree: always left out.
    pub deleted: Vec<String>,
    /// Untracked `.gitattributes` files: not staged by this.
    pub new_attributes: Vec<String>,
    /// Conflicted files: the action is refused while there are any.
    pub conflicted: usize,
}

impl Plan {
    /// The plan for `state`.
    pub fn of(state: &WorkingState) -> Self {
        let mut plan = Plan { conflicted: state.conflicted.len(), ..Default::default() };
        for w in &state.unstaged {
            let e = &w.entry;
            if e.conflicted {
                continue;
            }
            if e.untracked {
                if is_attributes_file(&e.path) {
                    plan.new_attributes.push(e.path.clone());
                }
                continue;
            }
            // A submodule: left out by the action anyway.
            if e.submodule.is_some() {
                continue;
            }
            if e.worktree == Some('D') {
                plan.deleted.push(e.path.clone());
            } else {
                plan.changed.push(e.path.clone());
            }
        }
        plan
    }

    /// The dialog's text: what the action does, what follows from it, and the files this
    /// state makes special.
    pub fn detail(&self) -> String {
        let mut s = String::from(
            "Applies the current line-ending rules — the text / eol attributes in \
             .gitattributes and core.autocrlf — to every tracked file again, and stages the \
             result (git add --renormalize). git doesn't do this by itself when the rules \
             change: files it stored with other line endings stay as they are, and usually \
             don't even show as changed.\n\n\
             Files whose stored line endings change are then staged: commit them (together \
             with the change of .gitattributes, if you made one). The files in your working \
             folder are not rewritten — they keep their line endings on disk until git \
             writes them again (for example when switching branches) and show no unstaged \
             change. Untracked files are not touched.",
        );
        if !self.new_attributes.is_empty() {
            let names = self.new_attributes.join(", ");
            s.push_str(&format!(
                "\n\n{names} {} new and not staged by this: stage it too, so the rules and \
                 the renormalized files are committed together.",
                if self.new_attributes.len() == 1 { "is" } else { "are" }
            ));
        }
        if !self.changed.is_empty() {
            let n = self.changed.len();
            s.push_str(&format!(
                "\n\n{} unstaged changes — left out unless you include {} below \
                 (renormalizing {} stages {} changes too):\n{}",
                if n == 1 { "1 file has".to_string() } else { format!("{n} files have") },
                if n == 1 { "it" } else { "them" },
                if n == 1 { "it" } else { "them" },
                if n == 1 { "its" } else { "their" },
                list(&self.changed)
            ));
        }
        if !self.deleted.is_empty() {
            s.push_str(&format!(
                "\n\n{} left out (nothing to renormalize):\n{}",
                match self.deleted.len() {
                    1 => "1 deleted file is".to_string(),
                    n => format!("{n} deleted files are"),
                },
                list(&self.deleted)
            ));
        }
        s
    }

    /// The label of the box that includes [`Self::changed`] (`None`: there are none).
    pub fn include_label(&self) -> Option<String> {
        match self.changed.len() {
            0 => None,
            1 => Some("Include that file (stages its changes too)".into()),
            n => Some(format!("Include those {n} files (stages their changes too)")),
        }
    }

    /// The paths to leave out: [`Self::changed`] unless `include_changed`, and
    /// [`Self::deleted`] (which [`Repo::renormalize`] finds itself too).
    pub fn leave_out(&self, include_changed: bool) -> Vec<String> {
        let mut out = self.deleted.clone();
        if !include_changed {
            out.extend(self.changed.iter().cloned());
        }
        out
    }
}

/// `path` is a `.gitattributes` file (at the top or in a folder).
pub fn is_attributes_file(path: &str) -> bool {
    path.rsplit('/').next() == Some(".gitattributes")
}

/// Up to eight paths, indented, one per line ("… and N more").
fn list(paths: &[String]) -> String {
    let mut s: String = paths.iter().take(8).map(|p| format!("    {p}\n")).collect();
    if paths.len() > 8 {
        s.push_str(&format!("    … and {} more\n", paths.len() - 8));
    }
    s.trim_end().to_string()
}

/// What a renormalization staged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The paths whose staged content changed (sorted).
    pub staged: Vec<String>,
    /// How many paths were left out (asked to, deleted, submodules).
    pub left_out: usize,
}

impl Report {
    /// The status line after it.
    pub fn note(&self) -> String {
        if self.staged.is_empty() {
            return "Renormalized line endings: no file changed — the stored files already \
                    follow the current rules"
                .to_string();
        }
        let mut names = self.staged.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        if self.staged.len() > 3 {
            names.push_str(&format!(" and {} more", self.staged.len() - 3));
        }
        format!(
            "Renormalized line endings: {} staged ({names}) — commit to record it",
            count(self.staged.len(), "file")
        )
    }
}

/// A stage-0 index entry's content: (id, mode bits).
type Content = (gix::ObjectId, u32);

/// The stage-0 entries of the index by path, and the paths always left out.
type IndexRead = (HashMap<Vec<u8>, Content>, Vec<Vec<u8>>);

impl Repo {
    /// Renormalizes every tracked file but `leave_out` (repository-relative paths): `git
    /// add --renormalize` over `:/` minus those, deleted files and submodules (one git
    /// process; the index is read in-process before and after). Refused while the index
    /// has conflicts ([`CONFLICTS`]).
    pub fn renormalize(&self, leave_out: &[String]) -> Result<Report> {
        let (before, skip) = self.renormalize_index()?;
        // Nothing tracked: `:/` would match nothing, which git reports as an error.
        if before.is_empty() {
            return Ok(Report::default());
        }
        let mut specs = b":/\0".to_vec();
        let mut left_out = 0;
        let exclude = |specs: &mut Vec<u8>, path: &[u8]| {
            specs.extend_from_slice(b":(top,exclude,literal)");
            specs.extend_from_slice(path);
            specs.push(0);
        };
        for path in &skip {
            exclude(&mut specs, path);
            left_out += 1;
        }
        for path in leave_out {
            if !skip.iter().any(|p| p == path.as_bytes()) {
                exclude(&mut specs, path.as_bytes());
                left_out += 1;
            }
        }
        // Not `git_literal`: `:/` and the exclusions are pathspec magic (each excluded path
        // itself is `literal`).
        self.git().run_stdin(
            &["add", "--renormalize", "--pathspec-from-file=-", "--pathspec-file-nul"],
            Some(&specs),
        )?;
        let (after, _) = self.renormalize_index()?;
        let mut staged: Vec<String> = after
            .iter()
            .filter(|(path, content)| before.get(*path) != Some(content))
            .map(|(path, _)| String::from_utf8_lossy(path).into_owned())
            .collect();
        staged.sort();
        Ok(Report { staged, left_out })
    }

    /// The stage-0 entries of the index (fresh from disk) and the paths to leave out
    /// always: submodules and files missing in the working tree (not checked out ones
    /// included — git skips those itself). Errs with [`CONFLICTS`] on a conflict.
    fn renormalize_index(&self) -> Result<IndexRead> {
        use gix::index::entry::{Flags, Mode, Stage};
        let repo = self.gix()?;
        let index = match repo.open_index() {
            Ok(i) => i,
            // No index yet (a fresh repository): nothing tracked.
            Err(_) if !repo.index_path().exists() => return Ok((HashMap::new(), Vec::new())),
            Err(e) => return Err(gx(e)),
        };
        let mut entries = HashMap::new();
        let mut skip = Vec::new();
        for e in index.entries() {
            if e.stage() != Stage::Unconflicted {
                bail!(CONFLICTS);
            }
            let path = e.path(&index).to_vec();
            let gone = || {
                let rel = gix::path::from_bstr(e.path(&index));
                std::fs::symlink_metadata(self.workdir().join(rel)).is_err()
            };
            if e.mode == Mode::COMMIT || (!e.flags.contains(Flags::SKIP_WORKTREE) && gone()) {
                skip.push(path.clone());
            }
            entries.insert(path, (e.id, e.mode.bits()));
        }
        Ok((entries, skip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn the_report_says_what_was_staged() {
        let r = Report::default();
        assert_eq!(
            r.note(),
            "Renormalized line endings: no file changed — the stored files already follow the current rules"
        );
        let r = Report { staged: vec!["a.txt".into()], left_out: 0 };
        assert_eq!(r.note(), "Renormalized line endings: 1 file staged (a.txt) — commit to record it");
        let staged = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        let r = Report { staged, left_out: 2 };
        assert_eq!(
            r.note(),
            "Renormalized line endings: 5 files staged (a, b, c and 2 more) — commit to record it"
        );
    }

    #[gitti_test::test]
    fn the_dialog_names_what_this_state_makes_special() {
        let plan = Plan::default();
        assert!(plan.detail().contains("git add --renormalize"));
        assert!(plan.detail().contains("are not rewritten"));
        assert_eq!(plan.include_label(), None);
        let plan = Plan {
            changed: vec!["d.txt".into()],
            deleted: vec!["g.txt".into(), "h.txt".into()],
            new_attributes: vec![".gitattributes".into()],
            conflicted: 0,
        };
        let d = plan.detail();
        assert!(d.contains(".gitattributes is new and not staged by this"), "{d}");
        assert!(d.contains("1 file has unstaged changes — left out unless you include it below"), "{d}");
        assert!(d.contains("    d.txt"), "{d}");
        assert!(d.contains("2 deleted files are left out"), "{d}");
        assert_eq!(
            plan.include_label().as_deref(),
            Some("Include that file (stages its changes too)")
        );
        assert_eq!(plan.leave_out(false), ["g.txt", "h.txt", "d.txt"]);
        assert_eq!(plan.leave_out(true), ["g.txt", "h.txt"]);
        assert!(is_attributes_file("sub/.gitattributes") && !is_attributes_file("x.gitattributes"));
    }
}
