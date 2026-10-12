//! Reverting or applying lines, a hunk or a file's changes of a commit (or a stash) in
//! the working tree — "Revert These Lines in Working Tree", "Apply Hunk to Working Tree"
//! and so on, offered where a commit's diff is shown.
//!
//! The patch is built from the commit's diff with the machinery of partial staging
//! ([`crate::patch::build_patch`]: only the selected lines, a replaced block's lines in
//! pairs so a line stays in its place, exact bytes in any encoding, "no newline" markers,
//! names that are not UTF-8 put back into their bytes). It goes to the working tree only,
//! never the index: `git apply --recount` (reverting: `-R`). A revert builds it as a
//! discard of the commit's lines (unselected `+` lines stay as context — they are what the
//! working tree has), an apply as a stage (unselected `-` lines stay as context). The
//! result is unstaged changes; the message says so.
//!
//! `git apply --check` runs first: when the lines don't fit the file as it is now, the
//! error says which file and where, and that nothing was changed. A part of a hunk is
//! also tried with the hunk's other lines as the commit's other side has them (the rest
//! of the commit already reverted / still there: a line reverted, then applied again).
//!
//! Hunks and lines of a renamed file change the file's content only, in the name the
//! working tree has (reverting: the new name, else the old one; applying: the old name,
//! else the new one); the file entry ("… File Changes …") also renames it back / again.
//! A mode change goes only with a file entry of a diff without hunks (a `chmod` alone):
//! lines and hunks keep the file's mode, as partial staging does. Binary files, Git LFS
//! files, submodule pointers, type changes and diffs that weren't loaded have no lines to
//! apply: refused in plain words, naming what does the job instead.

use anyhow::{Result, anyhow, bail};

use crate::Repo;
use crate::diff::{FileChange, FileDiff};
use crate::patch::{PatchMode, Selection, build_patch, map_selection};
use crate::repo::DiffOptions;
use gix::ObjectId;

pub const REVERT_LINES: &str = "Revert These Lines in Working Tree";
pub const REVERT_HUNK: &str = "Revert Hunk in Working Tree";
pub const REVERT_FILE: &str = "Revert File Changes in Working Tree";
pub const APPLY_LINES: &str = "Apply These Lines to Working Tree";
pub const APPLY_HUNK: &str = "Apply Hunk to Working Tree";
pub const APPLY_FILE: &str = "Apply File Changes to Working Tree";
/// The menu bar's / palette's entries: the selected lines of the shown commit's (or
/// stash's) diff.
pub const REVERT_SELECTION_LABEL: &str = "Revert Selected Lines in Working Tree";
pub const APPLY_SELECTION_LABEL: &str = "Apply Selected Lines to Working Tree";
/// The palette's hint of both.
pub const SELECTION_HINT: &str = "of the shown commit or stash";
/// What the menu bar's / palette's entries say without selected lines.
pub const SELECT_LINES_FIRST: &str =
    "Select lines of the shown commit first (or use the menu of a hunk / file in its diff)";

/// Which way the commit's change goes into the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// Undo it (`git apply -R`).
    Revert,
    /// Make it again (`git apply`).
    Apply,
}

/// What of the file is chosen (for the wording, and a file entry renames too).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Lines,
    Hunk,
    File,
}

impl Scope {
    /// The menu entry of `self` going `way`.
    pub fn label(self, way: Way) -> &'static str {
        match (way, self) {
            (Way::Revert, Scope::Lines) => REVERT_LINES,
            (Way::Revert, Scope::Hunk) => REVERT_HUNK,
            (Way::Revert, Scope::File) => REVERT_FILE,
            (Way::Apply, Scope::Lines) => APPLY_LINES,
            (Way::Apply, Scope::Hunk) => APPLY_HUNK,
            (Way::Apply, Scope::File) => APPLY_FILE,
        }
    }
}

/// Where the shown diff comes from: a commit (against its first parent) or a stash
/// (`stash@{n}`, whose commit is `id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    pub id: ObjectId,
    pub stash: Option<usize>,
}

impl Origin {
    /// "commit 1a2b3c4" / "stash@{0}".
    pub fn name(&self) -> String {
        match self.stash {
            Some(n) => format!("stash@{{{n}}}"),
            None => format!("commit {}", short(&self.id)),
        }
    }
}

fn short(id: &ObjectId) -> String {
    let mut s = id.to_string();
    s.truncate(7);
    s
}

/// A revert / apply of part of a commit's file.
#[derive(Debug, Clone)]
pub struct Request {
    pub origin: Origin,
    /// The file's diff as shown.
    pub file: FileDiff,
    /// What is chosen, on `file` (ignored for [`Scope::File`]).
    pub sel: Selection,
    pub scope: Scope,
    pub way: Way,
    /// The options the shown diff was read with: other than the defaults (ignore
    /// whitespace, other context) the exact diff is read again (cached) and the selection
    /// carried over by line numbers ([`map_selection`]).
    pub shown: DiffOptions,
}

/// Why a file has no lines to revert / apply (`None`: it has); also what the menus grey
/// their entries with.
pub fn no_lines(file: &FileDiff) -> Option<String> {
    let path = file.path();
    if file.too_large.is_some() {
        return Some(format!("the diff of {path} isn't loaded (too large) — Load anyway first"));
    }
    if file.submodule.is_some() {
        return Some(format!(
            "{path} is a submodule: the commit changed which commit it points to, not lines \
             — check out that commit in the submodule instead"
        ));
    }
    if file.lfs.is_some() {
        return Some(format!(
            "{path} is a Git LFS file: the commit holds a pointer, not its lines — use \
             Restore This Version / Restore Version Before This Commit instead"
        ));
    }
    if file.binary {
        return Some(format!(
            "{path} is a binary file: it has no lines to apply — use Restore This Version / \
             Restore Version Before This Commit instead"
        ));
    }
    if file.change == FileChange::TypeChanged {
        return Some(format!(
            "{path} changed its type (file / symlink): no lines to apply — use Restore This \
             Version / Restore Version Before This Commit instead"
        ));
    }
    None
}

/// The number of change lines `sel` selects of `file`.
pub fn selected_lines(file: &FileDiff, sel: &Selection) -> usize {
    crate::stash::selected_lines(file, sel)
}

fn lines_word(n: usize) -> String {
    if n == 1 { "1 line".into() } else { format!("{n} lines") }
}

/// What `scope` names in a message: "2 lines of a.txt", "a hunk of a.txt", "the changes
/// to a.txt".
fn what(scope: Scope, n: usize, path: &str) -> String {
    match scope {
        Scope::Lines => format!("{} of {path}", lines_word(n)),
        Scope::Hunk => format!("a hunk of {path}"),
        Scope::File => format!("the changes to {path}"),
    }
}

impl Repo {
    /// Reverts / applies the chosen part of a commit's file in the working tree (see the
    /// module docs). Returns the status line's text; an error says what didn't fit and
    /// that nothing was changed.
    pub fn commit_lines_to_worktree(&self, req: &Request) -> Result<String> {
        let shown = &req.file;
        if let Some(why) = no_lines(shown) {
            bail!("{why}");
        }
        let verb = match req.way {
            Way::Revert => "revert",
            Way::Apply => "apply",
        };
        // The exact diff of the file: the shown one, or read again without `-w` and with
        // git's default context.
        let exact = DiffOptions::default();
        let (file, sel) = if req.shown == exact {
            (shown.clone(), req.sel.clone())
        } else {
            let data = self.load_commit(req.origin.id, req.origin.stash, &exact)?;
            let real = data
                .files
                .iter()
                .find(|f| f.old_path == shown.old_path && f.new_path == shown.new_path)
                .cloned()
                .ok_or_else(|| anyhow!("{} is not in {} any more", shown.path(), req.origin.name()))?;
            if let Some(why) = no_lines(&real) {
                bail!("{why}");
            }
            let sel = match req.scope {
                Scope::File => Selection::default(),
                _ => map_selection(shown, &req.sel, &real).ok_or_else(|| {
                    anyhow!(
                        "Could not match the selected lines of {} to its exact diff — turn off \
                         Ignore Whitespace to {verb} them",
                        shown.path()
                    )
                })?,
            };
            (real, sel)
        };
        let sel = match req.scope {
            Scope::File => Selection {
                hunks: (0..file.hunks.len().max(1)).map(|h| (h, None)).collect(),
            },
            _ => sel,
        };
        let n = selected_lines(&file, &sel);
        if req.scope != Scope::File && n == 0 {
            bail!("No changed lines selected");
        }
        // Lines and hunks: the file's content only, in the name the working tree has.
        let file = match req.scope {
            Scope::File => file,
            _ => self.as_content_change(file, req.way),
        };
        let path = file.path().to_string();
        let mode = match req.way {
            Way::Revert => PatchMode::Discard,
            Way::Apply => PatchMode::Stage,
        };
        if let Err(e) = self.check_names(&file, mode) {
            bail!(
                "The name of {} is not UTF-8 and another file's name shows the same — gitti \
                 can't tell which file it is, so it cannot {verb} its lines",
                e.path
            );
        }
        let patch_of = |mode: PatchMode| -> Result<Vec<u8>> {
            let patch = build_patch(&file, &sel, mode).ok_or_else(|| anyhow!("No changed lines selected"))?;
            self.raw_patch_names(&file, without_index_mode(&patch), mode)
        };
        let patch = patch_of(mode)?;

        let mut args = vec!["apply", "--recount", "--whitespace=nowarn"];
        if req.way == Way::Revert {
            args.push("-R");
        }
        let mut check = args.clone();
        check.insert(1, "--check");
        check.push("-");
        args.push("-");
        let what = what(req.scope, n, &path);
        let out = self.git().run_raw(&check, Some(&patch))?;
        let patch = if out.code == 0 {
            patch
        } else {
            // The commit's other lines of the hunk are taken as they were before it
            // (applying) / after it (reverting). When the working tree has the other
            // version of them — the rest of the commit is (still / already) there, e.g.
            // a line reverted and now applied again —, a patch with those as context fits.
            let other = match mode {
                PatchMode::Discard => PatchMode::Stage,
                _ => PatchMode::Discard,
            };
            let fallback = match is_partial(&file, &sel) {
                true => Some(patch_of(other)?),
                false => None,
            };
            match fallback {
                Some(p) if p != patch && self.git().run_raw(&check, Some(&p))?.code == 0 => p,
                _ => return Err(does_not_fit(verb, &what, &req.origin, &path, &out.stderr, &patch)),
            }
        };
        // `git apply` writes the file anew: a read-only file would come back writable.
        let disk = self.fs_path(&path);
        let read_only = std::fs::symlink_metadata(&disk).is_ok_and(|m| m.is_file() && m.permissions().readonly());
        if let Err(e) = self.git().run_stdin(&args, Some(&patch)) {
            let stderr = match &e {
                crate::git_cli::GitError::Failed { stderr, .. } => stderr.clone(),
                other => other.to_string(),
            };
            return Err(does_not_fit(verb, &what, &req.origin, &path, &stderr, &patch));
        }
        if read_only
            && let Ok(m) = std::fs::symlink_metadata(&disk)
            && m.is_file()
            && !m.permissions().readonly()
        {
            let mut perm = m.permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(&disk, perm)?;
        }
        let done = match req.way {
            Way::Revert => format!("Reverted {what} ({})", req.origin.name()),
            Way::Apply => format!("Applied {what} from {}", req.origin.name()),
        };
        Ok(format!("{done} — changed in the working tree; stage and commit to keep it"))
    }

    /// `file` (a renamed / copied file's diff) as a change of the content of one file, the
    /// name the working tree has: reverting the new name (else the old one), applying the
    /// old name (else the new one). Other diffs as they are.
    fn as_content_change(&self, file: FileDiff, way: Way) -> FileDiff {
        if !matches!(file.change, FileChange::Renamed | FileChange::Copied) {
            return file;
        }
        let (Some(old), Some(new)) = (file.old_path.clone(), file.new_path.clone()) else {
            return file;
        };
        let exists = |p: &str| std::fs::symlink_metadata(self.fs_path(p)).is_ok();
        let path = match way {
            Way::Revert if exists(&new) || !exists(&old) => new,
            Way::Revert => old,
            Way::Apply if exists(&old) || !exists(&new) => old,
            Way::Apply => new,
        };
        let mut header = vec![format!(
            "diff --git {} {}",
            crate::patch::quote_path("a/", &path),
            crate::patch::quote_path("b/", &path)
        )];
        // The blob ids (and mode) stay: the content is still the commit's.
        header.extend(file.header.iter().filter(|h| h.starts_with("index ")).cloned());
        header.push(format!("--- {}", crate::patch::quote_path("a/", &path)));
        header.push(format!("+++ {}", crate::patch::quote_path("b/", &path)));
        FileDiff {
            old_path: Some(path.clone()),
            new_path: Some(path),
            change: FileChange::Modified,
            header,
            ..file
        }
    }
}

/// Whether `sel` leaves out a change line of a hunk it selects lines of.
fn is_partial(file: &FileDiff, sel: &Selection) -> bool {
    sel.hunks.iter().any(|(&h, lines)| {
        let (Some(set), Some(hunk)) = (lines, file.hunks.get(h)) else {
            return false;
        };
        hunk.lines.iter().enumerate().any(|(i, l)| l.is_change() && !set.contains(&i))
    })
}

/// `patch` without the mode on its `index` line (`index a..b 100644`): the working-tree
/// file keeps the mode it has (git would only warn that it differs). A new / deleted
/// file's mode is on its own line and stays.
fn without_index_mode(patch: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(patch.len());
    let mut rest = patch;
    let mut in_header = true;
    while !rest.is_empty() {
        let end = rest.iter().position(|b| *b == b'\n').map_or(rest.len(), |i| i + 1);
        let line = &rest[..end];
        if line.starts_with(b"@@") {
            in_header = false;
        }
        if in_header && line.starts_with(b"index ") {
            let body = line.strip_suffix(b"\n").unwrap_or(line);
            let mut parts = body.split(|b| *b == b' ');
            let (_, ids) = (parts.next(), parts.next());
            if let Some(ids) = ids {
                out.extend_from_slice(b"index ");
                out.extend_from_slice(ids);
                out.push(b'\n');
                rest = &rest[end..];
                continue;
            }
        }
        out.extend_from_slice(line);
        rest = &rest[end..];
    }
    out
}

/// The error of a revert / apply whose patch doesn't fit the working tree: one plain first
/// line — which file, where, and that nothing was changed —, then git's reason and the
/// patch for the details view.
fn does_not_fit(verb: &str, what: &str, origin: &Origin, path: &str, git: &str, patch: &[u8]) -> anyhow::Error {
    let reason = if git.contains("No such file or directory") || git.contains("does not exist") {
        format!("{path} is not in the working tree")
    } else if git.contains("already exists") {
        format!("{path} already exists in the working tree")
    } else if let Some(line) = failed_line(git) {
        format!(
            "{path} differs around line {line} from {}'s version — the lines there changed since",
            origin.name()
        )
    } else {
        format!("they don't fit {path} as it is in the working tree")
    };
    let mut text = format!(
        "Could not {verb} {what} ({}): {reason}. Nothing was changed (details: Error \
         Details / Git Log).",
        origin.name()
    );
    let reasons: Vec<String> = git
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| format!("git apply: {l}"))
        .collect();
    if !reasons.is_empty() {
        text.push_str("\n\n");
        text.push_str(&reasons.join("\n"));
    }
    text.push_str("\n\nThe patch was:\n");
    for line in String::from_utf8_lossy(patch).lines() {
        text.push_str("| ");
        text.push_str(line);
        text.push('\n');
    }
    anyhow!(text.trim_end().to_string())
}

/// The line of git's "error: patch failed: a.txt:12".
fn failed_line(git: &str) -> Option<u32> {
    git.lines().find_map(|l| {
        let rest = l.split_once("patch failed: ")?.1;
        rest.rsplit_once(':')?.1.trim().parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn the_index_line_loses_its_mode_only() {
        let p = b"diff --git a/f b/f\nindex 1a..2b 100755\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-index x 1\n+y\n";
        let out = without_index_mode(p);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "diff --git a/f b/f\nindex 1a..2b\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-index x 1\n+y\n"
        );
    }

    #[gitti_test::test]
    fn a_patch_that_does_not_fit_reads_plainly_first() {
        let origin = Origin {
            id: ObjectId::from_hex(b"1a2b3c4d5e6f708192a3b4c5d6e7f80910111213").unwrap(),
            stash: None,
        };
        let e = does_not_fit(
            "revert",
            "a hunk of a.txt",
            &origin,
            "a.txt",
            "error: patch failed: a.txt:12\nerror: a.txt: patch does not apply\n",
            b"@@ -12 +12 @@\n-x\n+y\n",
        );
        let text = format!("{e:#}");
        let first = text.lines().next().unwrap();
        assert_eq!(
            first,
            "Could not revert a hunk of a.txt (commit 1a2b3c4): a.txt differs around line 12 \
             from commit 1a2b3c4's version — the lines there changed since. Nothing was \
             changed (details: Error Details / Git Log)."
        );
        assert!(text.contains("| +y"), "{text}");
        assert!(text.lines().skip(1).all(|l| !l.trim().starts_with("error")), "{text}");
    }
}
