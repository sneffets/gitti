//! "Open in External Diff Tool" / "Open in External Merge Tool" ([`gitti_core::ext_tool`]):
//! git's own `difftool` / `mergetool` with the user's configured tool, run as an
//! ordinary operation (busy, esc cancels, Git Log entry, refresh after it). A tool that
//! needs a terminal is refused with the reason (git runs without one).

use gitti_core::conflict::ConflictAction;
use gitti_core::ext_tool::{DiffSides, DiffToolTarget};
use gitti_core::rows::Src;
use gitti_core::version_file::DiffSource;

use super::{After, App, Detail, work_files};
use crate::modal::MenuItem;

impl App {
    /// The diff tool's view of file `file` of `src` (`None` for a conflicted file).
    fn diff_tool_target(&self, src: Src, file: usize) -> Option<DiffToolTarget> {
        match src {
            Src::Unstaged | Src::Staged => {
                let wf = work_files(&self.working, src).get(file)?;
                let sides = if src == Src::Staged {
                    DiffSides::Staged
                } else {
                    DiffSides::Unstaged {
                        untracked: wf.entry.untracked,
                    }
                };
                Some(match &wf.diff {
                    Some(d) => DiffToolTarget::new(sides, d),
                    None => DiffToolTarget {
                        sides,
                        path: wf.path().to_string(),
                        old_path: None,
                    },
                })
            }
            Src::Commit => {
                let Detail::Commit(cv) = &self.detail else {
                    return None;
                };
                let view = match &cv.compare {
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
                Some(DiffToolTarget::new(DiffSides::of_view(view), cv.files.get(file)?))
            }
            Src::Conflicted => None,
        }
    }

    /// The diff menu's "Open in External Diff Tool" for file `file` of `src`.
    pub(crate) fn external_diff_item(&self, src: Src, file: usize) -> Option<MenuItem> {
        let t = self.diff_tool_target(src, file)?;
        Some(MenuItem::new("Open in External Diff Tool", move |app: &mut App| {
            app.run_op("External diff tool", false, After::Nothing, move |r| {
                r.open_diff_tool(&t)
            })
        }))
    }

    /// Resolves conflicted `path` in the external merge tool (`git mergetool`).
    pub(crate) fn open_external_merge_tool(&mut self, path: String) {
        self.run_op("External merge tool", false, After::Nothing, move |r| {
            r.resolve_conflict(&[path], ConflictAction::MergeTool)
        });
    }
}
