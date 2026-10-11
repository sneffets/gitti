//! The kind of a conflicted file ([`gitti_core::conflict`]) in the working directory's
//! diff: a few words in its header (the whole explanation in the tooltip) and its own
//! ways to resolve — keep / delete for a file one side deleted, the merge editor and
//! ours / theirs for a content conflict, ours / theirs (no merge editor) for a binary one
//! or a submodule's pointer.

use gitti_core::conflict::{ConflictAction, ConflictChoice, ConflictKind, ConflictShape};
use gitti_core::ops::ConflictSides;
use gitti_core::rows::Src;
use gpui_kit::*;

use super::GittiApp;

impl GittiApp {
    /// The sides' names of the current conflicts ("HEAD" / "incoming" until known).
    pub(super) fn conflict_sides_or_default(&self) -> ConflictSides {
        self.conflict_sides.clone().unwrap_or(ConflictSides {
            ours: "HEAD".into(),
            theirs: "incoming".into(),
        })
    }

    /// Conflicted file `file`'s kind.
    pub(super) fn conflict_kind(&self, file: usize) -> Option<ConflictKind> {
        ConflictKind::of(&self.working.conflicted.get(file)?.entry)
    }

    /// What conflicted file `file`'s sides are (from its diff, [`ConflictShape::of`]): no
    /// merge editor for a binary file or a submodule pointer.
    pub(super) fn conflict_shape(&self, file: usize) -> ConflictShape {
        ConflictShape::of(self.file_diff(Src::Conflicted, file))
    }

    /// Conflicted file `file`'s ways to resolve, the usual one first.
    pub(super) fn conflict_choices(&self, file: usize) -> Vec<ConflictChoice> {
        let kind = self.conflict_kind(file).unwrap_or(ConflictKind::BothModified);
        kind.choices_for(&self.conflict_sides_or_default(), self.conflict_shape(file))
    }

    /// The note of conflicted file `file` when its diff shows nothing.
    pub(super) fn conflict_no_diff_note(&self, file: usize) -> String {
        let exists = self.file_path(Src::Conflicted, file).is_some_and(|p| {
            self.repo.as_ref().is_some_and(|r| {
                std::fs::symlink_metadata(r.fs_path(&p)).is_ok()
            })
        });
        gitti_core::conflict::no_diff_note(&self.conflict_sides_or_default(), exists)
    }

    /// Resolves conflicted file `file` with `action`.
    pub(super) fn run_conflict_choice(
        &mut self,
        file: usize,
        action: ConflictAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ConflictAction::MergeEditor => self.open_merge_editor(file, window, cx),
            ConflictAction::MergeTool => self.open_external_merge_tool(file, window, cx),
            ConflictAction::TakeOurs => self.resolve_conflict(file, Some(true), window, cx),
            ConflictAction::TakeTheirs => self.resolve_conflict(file, Some(false), window, cx),
            ConflictAction::MarkResolved => self.resolve_conflict(file, None, window, cx),
            ConflictAction::Keep | ConflictAction::Delete | ConflictAction::RestoreBase => {
                let Some(path) = self.file_path(Src::Conflicted, file) else {
                    return;
                };
                let label = match action {
                    ConflictAction::Delete => "Delete conflicted file",
                    ConflictAction::RestoreBase => "Restore base version",
                    _ => "Keep conflicted file",
                };
                self.set_pending(Src::Conflicted, file, None, "Resolving");
                self.git_op(label, window, cx, move |r| r.resolve_conflict(&[path], action))
            }
        }
    }
}
