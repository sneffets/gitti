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
            A::TakeOurs | A::TakeTheirs => self.resolve_with_side(paths, action == A::TakeOurs),
            A::MarkResolved => self.mark_resolved(paths),
            A::Keep => {
                let missing: Vec<&str> = paths
                    .iter()
                    .filter(|p| {
                        let full = crate::paths::join_repo_path(self.workdir(), p);
                        std::fs::symlink_metadata(full).is_err()
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
        assert_eq!(ConflictKind::DeletedByThem.short(&s), "deleted by feature, changed by main");
        assert!(ConflictKind::AddedByThem.explain(&s).starts_with("Only feature has"));
    }
}
