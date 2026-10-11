//! "Open This Version in Editor" / "Open Old Version in Editor", as in gitti
//! (`gitti-app/src/app/menu_cmds.rs`): a file of a commit, a stash or a comparison as it
//! is there (or on the old side), written to a read-only copy under the temp directory
//! ([`gitti_core::version_file`]) and opened in the editor at the cursor's line — the
//! working-tree file stays what `O` opens. Offered in a commit file's diff menu (`m`;
//! `V` acts directly) and in the palette (the file at the diff cursor). The copy is
//! written on a job; a terminal editor then gets the terminal like `O`'s.

use gitti_core::rows::Src;
use gitti_core::version_file::{
    self, DiffSource, VersionCopy, VersionSide, VersionTarget, version_sides, version_target,
};

use super::{App, Detail, Msg};
use crate::modal::{MenuItem, PickItem};

/// The diff menu's key of "Open This Version in Editor" (free in the diff: it acts
/// directly there).
pub const THIS_KEY: char = 'V';

impl App {
    /// The (new, old) sides of the shown diff that have a commit to read versions from
    /// (`None`: no commit / stash / comparison shown).
    pub(crate) fn version_sides(&self) -> Option<(Option<VersionSide>, Option<VersionSide>)> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        let src = match &cv.compare {
            Some(c) => DiffSource::Compare {
                old: c.old.commit(),
                new: c.new.commit(),
            },
            None => DiffSource::Commit {
                id: cv.details.id,
                parents: &cv.details.parents,
                stash: cv.stash.is_some(),
            },
        };
        Some(version_sides(src))
    }

    /// The diff menu's "Open This / Old Version in Editor — read-only copy at …" for file
    /// `file` of `src` (a commit's file only); greyed with the reason where a side has no
    /// such file (added / deleted here).
    pub(crate) fn version_items(&self, src: Src, file: usize) -> Vec<MenuItem> {
        if src != Src::Commit {
            return Vec::new();
        }
        let (Some((new, old)), Some(f)) = (self.version_sides(), self.file_diff(src, file)) else {
            return Vec::new();
        };
        let mut items = Vec::new();
        for (side, is_old) in [(new, false), (old, true)] {
            let Some(side) = side else {
                continue;
            };
            let label = version_file::menu_label(&side, is_old);
            let item = match version_target(Some(side), f, is_old) {
                Ok(_) => MenuItem::new(label, move |app: &mut App| app.open_version(is_old)),
                Err(why) => MenuItem::new(label, |_| {}).unless(Some(&why)),
            };
            items.push(if is_old { item } else { item.key(THIS_KEY) });
        }
        items
    }

    /// Palette entries: the file at the diff cursor, greyed by the shared rules where
    /// there is none.
    pub(crate) fn version_palette_items(&self) -> Vec<PickItem> {
        vec![
            PickItem::new(version_file::THIS_LABEL, "file at the diff cursor", |app: &mut App| {
                app.open_version(false)
            }),
            PickItem::new(version_file::OLD_LABEL, "file at the diff cursor", |app: &mut App| {
                app.open_version(true)
            }),
        ]
    }

    /// Writes the file at the diff cursor as it is in the shown commit (`old`: its parent
    /// / the comparison's old side) to its read-only copy — on a job — and then opens that
    /// in the editor at the cursor's line ([`Self::version_written`]).
    pub(crate) fn open_version(&mut self, old: bool) {
        let Some((new_side, old_side)) = self.version_sides() else {
            self.set_info(version_file::NOT_HERE);
            return;
        };
        let side = if old { old_side } else { new_side };
        let found = self.rows.get(self.diff_cursor).copied().and_then(|r| {
            let (src, file) = r.file_ref().filter(|(s, _)| *s == Src::Commit)?;
            let f = self.file_diff(src, file)?;
            let line = if old { r.editor_line_old(f) } else { r.editor_line(f) };
            Some((version_target(side, f, old), line))
        });
        let Some((target, line)) = found else {
            self.set_info("No file here");
            return;
        };
        let target = match target {
            Ok(t) => t,
            Err(why) => {
                self.set_info(why);
                return;
            }
        };
        let repo = self.repo.clone();
        let t = target.clone();
        self.start_load(
            move || repo.export_target(&t).map_err(|e| format!("{e:#}")),
            move |copy| Msg::VersionWritten(Box::new((copy, target, line))),
        );
    }

    /// The copy of [`Self::open_version`] is written: open it in the editor.
    pub(super) fn version_written(&mut self, copy: Result<VersionCopy, String>, t: VersionTarget, line: u32) {
        match copy {
            Ok(copy) => {
                let info = version_file::opened_message(&t, &copy);
                self.open_file_in_editor(&copy.path, line, info);
            }
            Err(e) => self.set_error(e),
        }
    }
}
