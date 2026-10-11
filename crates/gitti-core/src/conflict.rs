//! What kind of conflict a file is in — git's `XY` of an unmerged entry (`git status`
//! porcelain v2) — and how it can be resolved, in the words both apps show.
//!
//! A content conflict (both sides changed / added the file) goes to the merge editor or
//! takes one side. The others are about the file's existence: one side deleted (or
//! renamed away) what the other changed, only one side has the file (the other renamed it
//! elsewhere), or both removed it. Those are resolved by keeping the file (`git add`),
//! deleting it (`git rm`) or, when both sides removed it, restoring the version both
//! started from. During a rebase git swaps the roles ("ours" is the branch rebased onto);
//! the texts name the sides ([`ConflictSides`]), so they read right either way.

use anyhow::{Result, bail};

use crate::Repo;
use crate::ops::ConflictSides;
use crate::status::StatusEntry;

/// git's kind of an unmerged path (`git status`: "both modified", "deleted by us", …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    /// `UU`: both sides changed the content.
    BothModified,
    /// `AA`: both sides added a file at this path.
    BothAdded,
    /// `DU`: ours deleted (or renamed away) the file, theirs changed it.
    DeletedByUs,
    /// `UD`: theirs deleted (or renamed away) the file, ours changed it.
    DeletedByThem,
    /// `AU`: only ours has the file here (e.g. both renamed one file to different names).
    AddedByUs,
    /// `UA`: only theirs has the file here.
    AddedByThem,
    /// `DD`: both sides removed the file (e.g. renamed it to different names).
    BothDeleted,
}

/// One way to resolve a conflicted file ([`ConflictKind::choices`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictAction {
    /// Open the merge editor (content conflicts; the apps handle it).
    MergeEditor,
    /// The configured external merge tool (`git mergetool`, [`crate::ext_tool`]); git
    /// stages the file when the tool merged it.
    MergeTool,
    /// The checked-out side's version (`checkout --ours` + `add`; `rm` if it has none).
    TakeOurs,
    /// The incoming side's version (`checkout --theirs` + `add`; `rm` if it has none).
    TakeTheirs,
    /// Stage the working-tree file as it is (`git add`), markers and all.
    MarkResolved,
    /// Keep the file as it is in the working directory (`git add`); refused when there
    /// is none (`git add` would stage a deletion).
    Keep,
    /// Delete the file from the working directory and the index (`git rm`).
    Delete,
    /// Restore the version both sides started from and stage it.
    RestoreBase,
}

impl ConflictAction {
    /// A button's label (the menus use [`ConflictChoice::label`], which names the side).
    pub fn button_label(self) -> &'static str {
        match self {
            Self::MergeEditor => "Merge Editor",
            Self::MergeTool => "External Merge Tool",
            Self::TakeOurs => "Take Ours",
            Self::TakeTheirs => "Take Theirs",
            Self::MarkResolved => "Mark Resolved",
            Self::Keep => "Keep File",
            Self::Delete => "Delete File",
            Self::RestoreBase => "Restore Base Version",
        }
    }
}

/// A resolution as the menus offer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictChoice {
    pub action: ConflictAction,
    /// The menu entry, e.g. "Keep the File — feature's version".
    pub label: String,
    /// What it does, in plain words (tooltip / note).
    pub detail: String,
}

/// What a content conflict's sides are, which decides its texts and its ways out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConflictShape {
    /// Text git merged with markers: the merge editor applies.
    #[default]
    Text,
    /// Binary, by content or by `.gitattributes` ([`Repo::binary_conflict`]): git left
    /// ours, no markers.
    Binary,
    /// A submodule pointer (gitlink) both sides moved to different commits: git doesn't
    /// merge it here; its diff shows ours → theirs ([`Repo::add_conflict_placeholders`]).
    Submodule,
}

impl ConflictShape {
    /// The shape of a conflicted file from its diff in the working state.
    pub fn of(diff: Option<&crate::diff::FileDiff>) -> Self {
        match diff {
            Some(d) if d.submodule.is_some() => Self::Submodule,
            Some(d) if d.binary => Self::Binary,
            _ => Self::Text,
        }
    }
}

impl ConflictKind {
    /// The kind of a conflicted status entry (`None`: not conflicted).
    pub fn of(entry: &StatusEntry) -> Option<Self> {
        entry.conflicted.then(|| Self::from_xy(entry.index, entry.worktree))
    }

    /// From porcelain v2's `XY` of an unmerged entry (`None` = `.`).
    pub fn from_xy(x: Option<char>, y: Option<char>) -> Self {
        match (x, y) {
            (Some('A'), Some('A')) => Self::BothAdded,
            (Some('D'), Some('U')) => Self::DeletedByUs,
            (Some('U'), Some('D')) => Self::DeletedByThem,
            (Some('A'), Some('U')) => Self::AddedByUs,
            (Some('U'), Some('A')) => Self::AddedByThem,
            (Some('D'), Some('D')) => Self::BothDeleted,
            _ => Self::BothModified,
        }
    }

    /// Both sides have the file: the merge editor and ours / theirs apply.
    pub fn is_content(self) -> bool {
        matches!(self, Self::BothModified | Self::BothAdded)
    }

    /// git's own words (`git status`): "both modified", "deleted by us", …
    pub fn git_label(self) -> &'static str {
        match self {
            Self::BothModified => "both modified",
            Self::BothAdded => "both added",
            Self::DeletedByUs => "deleted by us",
            Self::DeletedByThem => "deleted by them",
            Self::AddedByUs => "added by us",
            Self::AddedByThem => "added by them",
            Self::BothDeleted => "both deleted",
        }
    }

    /// A few words for the file header, with the sides' names.
    pub fn short(self, sides: &ConflictSides) -> String {
        let (o, t) = (&sides.ours, &sides.theirs);
        match self {
            Self::BothModified => "changed on both sides".into(),
            Self::BothAdded => "added on both sides".into(),
            Self::DeletedByUs => format!("deleted by {o}, changed by {t}"),
            Self::DeletedByThem => format!("deleted by {t}, changed by {o}"),
            Self::AddedByUs => format!("only in {o}"),
            Self::AddedByThem => format!("only in {t}"),
            Self::BothDeleted => "deleted on both sides".into(),
        }
    }

    /// [`Self::short`] of a file of `shape` ([`ConflictShape`]).
    pub fn short_for(self, sides: &ConflictSides, shape: ConflictShape) -> String {
        use ConflictShape as S;
        match (self, shape) {
            (Self::BothModified, S::Binary) => "binary, changed on both sides".into(),
            (Self::BothAdded, S::Binary) => "binary, added on both sides".into(),
            (Self::BothModified, S::Submodule) => "submodule, moved on both sides".into(),
            (Self::BothAdded, S::Submodule) => "submodule, added on both sides".into(),
            _ => self.short(sides),
        }
    }

    /// [`Self::explain`] of a file of `shape`: a binary or submodule content conflict says
    /// there is no merge editor for it and what to do instead.
    pub fn explain_for(self, sides: &ConflictSides, shape: ConflictShape) -> String {
        if !self.is_content() || shape == ConflictShape::Text {
            return self.explain(sides);
        }
        let (o, t) = (&sides.ours, &sides.theirs);
        if shape == ConflictShape::Submodule {
            return format!(
                "{o} and {t} both {} this submodule at different commits (the diff shows \
                 {o}'s commit → {t}'s). git doesn't merge submodule commits here, so there is \
                 no merge editor for it: take one side's commit, or check out the commit you \
                 want inside the submodule (e.g. a merge of both, made there) and mark it \
                 resolved, which records the submodule's checked-out commit. Taking a side \
                 records its commit and leaves the submodule's checkout as it is: update the \
                 submodule afterwards to check that commit out.",
                if self == Self::BothAdded { "added" } else { "moved" }
            );
        }
        format!(
            "{o} and {t} both {} this binary file (binary by its contents, or by \
             .gitattributes: -merge, merge=binary or the binary macro). git doesn't combine \
             binary contents, so there is no merge editor for it: take one side's version, \
             or replace the file (e.g. in the program that made it) and mark it resolved. \
             The working directory has {o}'s version.",
            if self == Self::BothAdded { "added" } else { "changed" }
        )
    }

    /// What happened to the file and what the choices are, in plain words.
    pub fn explain(self, sides: &ConflictSides) -> String {
        let (o, t) = (&sides.ours, &sides.theirs);
        match self {
            Self::BothModified => format!(
                "{o} and {t} both changed this file in ways git could not combine. Open it \
                 in the merge editor, or take one side's version."
            ),
            Self::BothAdded => format!(
                "{o} and {t} both added a file at this path, with different contents. Open \
                 it in the merge editor, or take one side's version."
            ),
            Self::DeletedByUs => format!(
                "{o} deleted this file (or renamed it away); {t} changed it. The working \
                 directory has {t}'s version: keep the file (git add) or delete it (git rm)."
            ),
            Self::DeletedByThem => format!(
                "{t} deleted this file (or renamed it away); {o} changed it (or renamed it \
                 here). The working directory has {o}'s version: keep the file (git add) or \
                 delete it (git rm)."
            ),
            Self::AddedByUs => format!(
                "Only {o} has this file at this path: {t} deleted it or moved it elsewhere \
                 (e.g. both sides renamed the same file to different names). Keep it (git \
                 add) or delete it (git rm)."
            ),
            Self::AddedByThem => format!(
                "Only {t} has this file at this path: {o} deleted it or moved it elsewhere \
                 (e.g. both sides renamed the same file to different names). Keep it (git \
                 add) or delete it (git rm)."
            ),
            Self::BothDeleted => format!(
                "{o} and {t} both deleted this file or renamed it away (to different names: \
                 see the files only one side has). Delete it (git rm), or restore the version \
                 both started from."
            ),
        }
    }

    /// The ways to resolve it, the usual one first.
    pub fn choices(self, sides: &ConflictSides) -> Vec<ConflictChoice> {
        self.choices_for(sides, ConflictShape::Text)
    }

    /// [`Self::choices`] of a file of `shape`: a binary content conflict offers no merge
    /// editor (git can't combine binary contents, nor can it) — take a side first, the
    /// external merge tool, or stage the file as it is (e.g. after replacing it in another
    /// program); a submodule's neither the merge editor nor the external merge tool (git
    /// mergetool asks about a submodule in the terminal) — take a side's commit, or record
    /// the commit checked out in the submodule.
    pub fn choices_for(self, sides: &ConflictSides, shape: ConflictShape) -> Vec<ConflictChoice> {
        use ConflictAction as A;
        let mut choices = self.all_choices(sides);
        if shape != ConflictShape::Text && self.is_content() {
            choices.retain(|c| {
                c.action != A::MergeEditor
                    && (shape != ConflictShape::Submodule || c.action != A::MergeTool)
            });
            // The sides first: the usual way out of a binary or submodule conflict.
            choices.sort_by_key(|c| !matches!(c.action, A::TakeOurs | A::TakeTheirs));
            if shape == ConflictShape::Submodule {
                for c in &mut choices {
                    c.detail = match c.action {
                        A::TakeOurs => format!("Record {}'s commit for the submodule", sides.ours),
                        A::TakeTheirs => format!("Record {}'s commit for the submodule", sides.theirs),
                        A::MarkResolved => {
                            "Record the commit checked out in the submodule (git add)".into()
                        }
                        _ => std::mem::take(&mut c.detail),
                    };
                }
            }
        }
        choices
    }

    fn all_choices(self, sides: &ConflictSides) -> Vec<ConflictChoice> {
        use ConflictAction as A;
        let (o, t) = (&sides.ours, &sides.theirs);
        let c = |action, label: String, detail: &str| ConflictChoice {
            action,
            label,
            detail: detail.to_string(),
        };
        const KEEP: &str = "Stage the file as it is in the working directory (git add)";
        const DELETE: &str = "Remove the file from the working directory and the index (git rm)";
        let keep_delete = |keeper: &String, deleter: Option<&String>| {
            vec![
                c(A::Keep, format!("Keep the File — {keeper}'s version"), KEEP),
                c(
                    A::Delete,
                    match deleter {
                        Some(d) => format!("Delete the File — as {d} did"),
                        None => "Delete the File".to_string(),
                    },
                    DELETE,
                ),
            ]
        };
        match self {
            Self::BothModified | Self::BothAdded => vec![
                c(
                    A::MergeEditor,
                    "Open in Merge Editor…".into(),
                    "Combine both versions chunk by chunk",
                ),
                c(
                    A::MergeTool,
                    "Open in External Merge Tool".into(),
                    "Resolve in the merge tool git is set up with (git mergetool, \
                     merge.tool); git stages the file when the tool merged it",
                ),
                c(
                    A::TakeOurs,
                    format!("Take Ours — {o}"),
                    "Replace the file with this side's version and stage it",
                ),
                c(
                    A::TakeTheirs,
                    format!("Take Theirs — {t}"),
                    "Replace the file with this side's version and stage it",
                ),
                c(
                    A::MarkResolved,
                    "Mark Resolved (stage as is)".into(),
                    "Stage the file as it is now in the working directory (git add)",
                ),
            ],
            Self::DeletedByUs => keep_delete(t, Some(o)),
            Self::DeletedByThem => keep_delete(o, Some(t)),
            Self::AddedByUs => keep_delete(o, None),
            Self::AddedByThem => keep_delete(t, None),
            Self::BothDeleted => vec![
                c(A::Delete, "Delete the File — as both did".into(), DELETE),
                c(
                    A::RestoreBase,
                    "Keep the File — the version both started from".into(),
                    "Restore the common base version into the working directory and stage it",
                ),
            ],
        }
    }
}

/// A binary conflict's note in the diff (where a text conflict shows its markers): git
/// left the checked-out side's version in the working directory. Binary by content or by
/// `.gitattributes` alike ([`ConflictKind::explain_for`] says which ways there are).
pub fn binary_note(sides: &ConflictSides) -> String {
    format!(
        "Binary file changed on both sides — git doesn't merge it; the working directory has {}'s version",
        sides.ours
    )
}

/// Whether git merges a file as binary — no markers, the checked-out side's version left —
/// by its `merge` attribute, as `git check-attr` reports it: `unset` (`-merge`, also by the
/// `binary` macro or a macro of the user's) always; the name `binary`, given or — the
/// attribute `unspecified` — by `merge.default`, unless a merge driver of the user's
/// (`merge.binary.*`, `user_driver`) takes that name; `set`, any other name (a driver of the
/// user's, `text`, `union`, an unknown one: git's text merge) don't. Those still merge as
/// binary when a side's content is binary ([`Repo::binary_conflict`]).
pub fn merge_attr_is_binary(
    value: &str,
    default: Option<&str>,
    user_driver: impl Fn(&str) -> bool,
) -> bool {
    let name = match value {
        "unset" => return true,
        "set" => return false,
        "unspecified" => match default {
            Some(d) => d,
            None => return false,
        },
        v => v,
    };
    name == "binary" && !user_driver(name)
}

/// What a conflicted path's index stages hold ([`Repo::conflict_stages`]).
enum Stages {
    Text,
    Binary,
    /// Ours and theirs are both submodule commits.
    Gitlinks { ours: gix::ObjectId, theirs: gix::ObjectId },
}

/// What a conflicted file's diff (against HEAD) shows when it has none: the file is not
/// in the working directory (`exists` false), or is as HEAD (ours) has it.
pub fn no_diff_note(sides: &ConflictSides, exists: bool) -> String {
    if exists {
        format!("Same as in {} — no changes to show", sides.ours)
    } else {
        "Not in the working directory".to_string()
    }
}

impl Repo {
    /// Resolves the conflicted files `paths` with `action` (not [`ConflictAction::
    /// MergeEditor`], which the apps open). Every path is checked before any is changed.
    pub fn resolve_conflict(&self, paths: &[String], action: ConflictAction) -> Result<String> {
        use ConflictAction as A;
        match action {
            A::MergeEditor => bail!("the merge editor is opened by the app"),
            A::MergeTool => {
                let mut notes = Vec::new();
                for p in paths {
                    notes.push(self.open_merge_tool(p)?.describe());
                }
                Ok(notes.join("\n"))
            }
            A::TakeOurs | A::TakeTheirs => self.resolve_with_side(paths, action == A::TakeOurs),
            A::MarkResolved => self.mark_resolved(paths),
            A::Keep => {
                let missing: Vec<&str> = paths
                    .iter()
                    .filter(|p| {
                        std::fs::symlink_metadata(self.fs_path(p)).is_err()
                    })
                    .map(String::as_str)
                    .collect();
                if !missing.is_empty() {
                    bail!(
                        "{} not in the working directory: there is nothing to keep (git add \
                         would stage a deletion). Delete it, or restore a version first.",
                        missing.join(", ") + if missing.len() == 1 { " is" } else { " are" }
                    );
                }
                self.mark_resolved(paths)
            }
            A::Delete => {
                let mut args = vec!["rm", "-q", "--"];
                args.extend(paths.iter().map(String::as_str));
                self.out_text(&args)
            }
            A::RestoreBase => {
                let mut args = vec!["checkout-index", "-f", "--stage=1", "--"];
                args.extend(paths.iter().map(String::as_str));
                let out = self.out_text(&args)?;
                Ok(out + &self.mark_resolved(paths)?)
            }
        }
    }

    /// Whether git merges the conflicted `path` as binary: a side is binary by content
    /// ([`Self::conflict_stages`]) or the file by its attributes ([`Self::merged_as_binary`]).
    /// git then leaves the checked-out side's version without markers, and the apps offer
    /// no merge editor for it.
    pub fn binary_conflict(&self, path: &str) -> bool {
        matches!(self.conflict_stages(&[path])[0], Stages::Binary)
            || !self.merged_as_binary(&[path]).is_empty()
    }

    /// Whether the conflicted `path` is a submodule pointer both sides moved (ours and
    /// theirs are submodule commits). In-process.
    pub fn submodule_conflict(&self, path: &str) -> bool {
        matches!(self.conflict_stages(&[path])[0], Stages::Gitlinks { .. })
    }

    /// What the index stages of each conflicted path in `paths` hold: ours and theirs both
    /// submodule commits (gitlinks), or a binary side — a NUL byte in the first 8000 bytes
    /// of its base, ours or theirs, as git decides before it merges. In-process (the index
    /// read once, the stages' blobs), no git.
    fn conflict_stages(&self, paths: &[&str]) -> Vec<Stages> {
        use gix::bstr::ByteSlice;
        use gix::index::entry::{Mode, Stage};
        let none = || paths.iter().map(|_| Stages::Text).collect();
        let Ok(repo) = self.gix() else {
            return none();
        };
        let Ok(index) = repo.index_or_empty() else {
            return none();
        };
        let stages = |path: &str| {
            // A name that is not UTF-8 by its bytes.
            let raw = self.git().raw_names().bytes(path).unwrap_or(std::borrow::Cow::Borrowed(path.as_bytes()));
            let entry = |stage| index.entry_by_path_and_stage(raw.as_bstr(), stage);
            let gitlink = |stage| entry(stage).filter(|e| e.mode == Mode::COMMIT).map(|e| e.id);
            if let (Some(ours), Some(theirs)) = (gitlink(Stage::Ours), gitlink(Stage::Theirs)) {
                return Stages::Gitlinks { ours, theirs };
            }
            let binary = [Stage::Base, Stage::Ours, Stage::Theirs].into_iter().any(|stage| {
                entry(stage)
                    .filter(|e| e.mode != Mode::COMMIT)
                    .and_then(|e| repo.find_object(e.id).ok())
                    .is_some_and(|o| o.data.iter().take(8000).any(|b| *b == 0))
            });
            if binary { Stages::Binary } else { Stages::Text }
        };
        paths.iter().map(|p| stages(p)).collect()
    }

    /// Those of `paths` git merges as binary by their attributes ([`merge_attr_is_binary`]):
    /// one `git check-attr` for all — nested `.gitattributes`, `info/attributes`,
    /// `core.attributesFile` and macros exactly as git's merge reads them — the config
    /// (`merge.default`, `merge.<driver>.*`) in-process. A failure counts as "none".
    fn merged_as_binary(&self, paths: &[&str]) -> std::collections::HashSet<String> {
        let input = self.query_list(paths);
        let Ok(out) = self.git().run_stdin(&["check-attr", "-z", "--stdin", "merge"], Some(&input))
        else {
            return Default::default();
        };
        let repo = self.gix().ok();
        let snapshot = repo.as_ref().map(|r| r.config_snapshot());
        let default = snapshot.as_ref().and_then(|c| c.string("merge.default")).map(|v| v.to_string());
        let user_driver = |name: &str| {
            snapshot.as_ref().is_some_and(|c| {
                c.plumbing().sections_by_name("merge").into_iter().flatten().any(|s| {
                    s.header().subsection_name().is_some_and(|n| n == name.as_bytes())
                })
            })
        };
        // `<path> NUL merge NUL <value> NUL` per path.
        let fields: Vec<&[u8]> = out.stdout.split(|b| *b == 0).collect();
        fields
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|c| merge_attr_is_binary(&String::from_utf8_lossy(c[2]), default.as_deref(), user_driver))
            .map(|c| String::from_utf8_lossy(c[0]).into_owned())
            .collect()
    }

    /// Gives the conflicted files of `files` whose shape the diff against HEAD doesn't
    /// show their own diff ([`ConflictShape::of`]), so the apps say what they are and offer
    /// no merge editor: a submodule pointer both sides moved shows ours → theirs (and the
    /// commits between them when the submodule is checked out); a file git merged as binary
    /// ([`Self::binary_conflict`]) whose diff shows nothing (the working directory has
    /// ours) a binary file's placeholder. The index read once in-process; one `git
    /// check-attr` for the text files among those with nothing to show, none without them.
    pub(crate) fn add_conflict_placeholders(&self, files: &mut [crate::WorkFile]) {
        let content: Vec<usize> = (0..files.len())
            .filter(|&i| ConflictKind::of(&files[i].entry).is_some_and(ConflictKind::is_content))
            .collect();
        if content.is_empty() {
            return;
        }
        let paths: Vec<&str> = content.iter().map(|&i| files[i].entry.path.as_str()).collect();
        let stages = self.conflict_stages(&paths);
        // Text with nothing to show: binary by attributes?
        let text: Vec<&str> = content
            .iter()
            .zip(&stages)
            .filter(|&(&i, s)| matches!(s, Stages::Text) && files[i].diff.is_none())
            .map(|(&i, _)| files[i].entry.path.as_str())
            .collect();
        let by_attributes =
            if text.is_empty() { Default::default() } else { self.merged_as_binary(&text) };
        for (i, st) in content.into_iter().zip(stages) {
            let f = &mut files[i];
            // `Some`: the pointers; `None`: binary.
            let submodule = match st {
                Stages::Gitlinks { ours, theirs } => Some(crate::submodule::SubmoduleDiff {
                    old: Some(ours),
                    new: Some(theirs),
                    ..Default::default()
                }),
                Stages::Binary if f.diff.is_none() => None,
                Stages::Text if f.diff.is_none() && by_attributes.contains(&f.entry.path) => None,
                _ => continue,
            };
            let gitlinks = submodule.is_some();
            let path = f.entry.path.clone();
            f.diff = Some(crate::diff::FileDiff {
                old_path: Some(path.clone()),
                new_path: Some(path),
                change: crate::diff::FileChange::Modified,
                binary: !gitlinks,
                header: Vec::new(),
                hunks: Vec::new(),
                submodule,
                too_large: None,
                encoding: Default::default(),
                lossy_name: false,
                raw_paths: Vec::new(),
                lfs: None,
            });
            if gitlinks {
                crate::submodule::fill_logs(self.workdir(), std::slice::from_mut(f.diff.as_mut().unwrap()));
            }
        }
    }

    fn out_text(&self, args: &[&str]) -> Result<String> {
        let o = self.git().run(args)?;
        Ok(format!("{}{}", o.stdout_str(), o.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sides() -> ConflictSides {
        ConflictSides {
            ours: "main".into(),
            theirs: "feature".into(),
        }
    }

    #[gitti_test::test]
    fn kinds_from_porcelain_letters() {
        let k = |x: char, y: char| ConflictKind::from_xy(Some(x), Some(y));
        assert_eq!(k('U', 'U'), ConflictKind::BothModified);
        assert_eq!(k('A', 'A'), ConflictKind::BothAdded);
        assert_eq!(k('D', 'U'), ConflictKind::DeletedByUs);
        assert_eq!(k('U', 'D'), ConflictKind::DeletedByThem);
        assert_eq!(k('A', 'U'), ConflictKind::AddedByUs);
        assert_eq!(k('U', 'A'), ConflictKind::AddedByThem);
        assert_eq!(k('D', 'D'), ConflictKind::BothDeleted);
        assert!(k('U', 'U').is_content() && k('A', 'A').is_content());
        assert!(!k('U', 'D').is_content() && !k('D', 'D').is_content());
    }

    /// git's choice of the binary merge driver by the `merge` attribute (`check-attr`'s
    /// value) and the config, as `ll-merge.c` makes it.
    #[gitti_test::test]
    fn merge_attribute_picks_the_binary_driver() {
        let no_driver = |_: &str| false;
        let binary = |v: &str, d: Option<&str>| merge_attr_is_binary(v, d, no_driver);
        // `-merge` (also through the `binary` macro), `merge=binary`.
        assert!(binary("unset", None) && binary("binary", None));
        // `merge`, no attribute, `merge=text` / `union` / an unknown name: the text merge.
        for v in ["set", "unspecified", "text", "union", "nosuch"] {
            assert!(!binary(v, None), "{v}");
        }
        // No attribute: `merge.default` names the driver.
        assert!(binary("unspecified", Some("binary")) && !binary("unspecified", Some("union")));
        assert!(!binary("set", Some("binary")));
        // A driver of the user's called `binary` (`merge.binary.driver`) is not git's.
        assert!(!merge_attr_is_binary("binary", None, |n| n == "binary"));
        assert!(!merge_attr_is_binary("unspecified", Some("binary"), |n| n == "binary"));
        assert!(merge_attr_is_binary("unset", None, |_| true));
    }

    /// The deletion kinds offer keep / delete naming the sides, no merge editor; the
    /// content kinds the merge editor first.
    #[gitti_test::test]
    fn choices_name_the_sides() {
        let s = sides();
        let labels = |k: ConflictKind| -> Vec<String> {
            k.choices(&s).into_iter().map(|c| c.label).collect()
        };
        assert_eq!(
            labels(ConflictKind::DeletedByThem),
            ["Keep the File — main's version", "Delete the File — as feature did"]
        );
        assert_eq!(
            labels(ConflictKind::DeletedByUs),
            ["Keep the File — feature's version", "Delete the File — as main did"]
        );
        assert_eq!(ConflictKind::BothModified.choices(&s)[0].action, ConflictAction::MergeEditor);
        assert!(
            ConflictKind::BothDeleted
                .choices(&s)
                .iter()
                .all(|c| c.action != ConflictAction::MergeEditor && c.action != ConflictAction::Keep)
        );
        // A binary content conflict: the sides first, no merge editor; other kinds as they are.
        let actions = |k: ConflictKind, shape| -> Vec<ConflictAction> {
            k.choices_for(&s, shape).into_iter().map(|c| c.action).collect()
        };
        use ConflictShape as S;
        assert_eq!(
            actions(ConflictKind::BothModified, S::Binary),
            [ConflictAction::TakeOurs, ConflictAction::TakeTheirs, ConflictAction::MergeTool, ConflictAction::MarkResolved]
        );
        assert_eq!(ConflictKind::DeletedByThem.choices_for(&s, S::Binary), ConflictKind::DeletedByThem.choices(&s));
        assert_eq!(ConflictKind::BothModified.short_for(&s, S::Binary), "binary, changed on both sides");
        assert!(ConflictKind::BothModified.explain_for(&s, S::Binary).contains("no merge editor"));
        assert_eq!(ConflictKind::DeletedByThem.explain_for(&s, S::Binary), ConflictKind::DeletedByThem.explain(&s));
        // A submodule's pointer: the sides' commits, or the checked-out one; no merge editor
        // nor external merge tool.
        let sub = ConflictKind::BothModified.choices_for(&s, S::Submodule);
        let got: Vec<_> = sub.iter().map(|c| c.action).collect();
        assert_eq!(got, [ConflictAction::TakeOurs, ConflictAction::TakeTheirs, ConflictAction::MarkResolved]);
        assert_eq!(sub[1].detail, "Record feature's commit for the submodule");
        assert_eq!(ConflictKind::BothModified.short_for(&s, S::Submodule), "submodule, moved on both sides");
        let explained = ConflictKind::BothModified.explain_for(&s, S::Submodule);
        assert!(explained.contains("main's commit → feature's") && explained.contains("update the submodule"), "{explained}");
        assert_eq!(ConflictKind::DeletedByThem.short(&s), "deleted by feature, changed by main");
        assert!(ConflictKind::AddedByThem.explain(&s).starts_with("Only feature has"));
    }
}
