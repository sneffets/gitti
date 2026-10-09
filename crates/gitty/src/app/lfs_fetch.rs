//! "Download LFS Objects" (as gitti's `app/lfs_fetch.rs`; logic in
//! `gitti_core::lfs_fetch`): a commit, stash or comparison whose LFS rows say "content not
//! in the local LFS store" offers it — Enter on such a row, the diff menu (`F`) and the
//! palette (the explanation: `gitti_core::lfs_fetch::EXPLANATION`). `git lfs fetch` runs as an operation (busy label, progress, askpass); the
//! view comes back re-annotated through `Msg::Detail` (same commit: the cursor stays).

use gitti_core::lfs::LfsCaps;
use gitti_core::lfs_fetch::{LfsDownload, NOT_INSTALLED};
use gitti_core::rows::{DiffRow, Src};

use super::{After, App, CommitView, Detail, MenuItem, Msg, PickItem};

/// The action's label (menu, palette).
pub const LABEL: &str = "Download LFS Objects";
/// The operation's label (busy line, errors).
const OP: &str = "Download LFS objects";

impl App {
    /// The objects the shown commit, stash or comparison misses locally.
    fn lfs_download_target(&self) -> Option<LfsDownload> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        LfsDownload::of_view(&cv.data, cv.compare.as_deref())
    }

    /// Whether diff row `row` is the LFS row of a commit file missing an object (Enter
    /// downloads).
    pub fn lfs_download_row(&self, row: usize) -> bool {
        match self.rows.get(row) {
            Some(&DiffRow::Note {
                src: src @ Src::Commit,
                file,
            }) => self
                .file_diff(src, file)
                .is_some_and(gitti_core::lfs_fetch::downloadable),
            _ => false,
        }
    }

    /// Downloads them (`git lfs fetch`); the view follows re-annotated. Without git-lfs
    /// it says so before anything runs.
    pub fn lfs_download(&mut self) {
        let (Some(dl), Detail::Commit(cv)) = (self.lfs_download_target(), &self.detail) else {
            self.set_info("No LFS objects are missing here");
            return;
        };
        if !LfsCaps::get().installed() {
            self.set_error(NOT_INSTALLED.to_string());
            return;
        }
        let (id, stash) = (cv.id, cv.stash);
        let pair = cv.compare.as_ref().map(|c| (c.old, c.new));
        let (generation, tx) = (self.detail_gen, self.tx.clone());
        let opts = self.opts.diff_options();
        self.run_op(OP, false, After::Nothing, move |r| {
            let outcome = r.lfs_fetch(&dl);
            // Re-annotated (the cached entry is stale once an object arrived; an
            // unchanged one comes from the cache): shown before the operation's end.
            let view = match pair {
                Some((old, new)) => r.compare_diff(old, new, &opts).map(|c| CommitView {
                    id,
                    data: c.data.clone(),
                    stash: None,
                    compare: Some(c),
                }),
                None => r.load_commit(id, stash, &opts).map(|data| CommitView {
                    id,
                    data,
                    stash,
                    compare: None,
                }),
            };
            if let Ok(cv) = view {
                let _ = tx.send(Msg::Detail(generation, Ok(cv)));
            }
            let out = outcome?;
            if out.missing.is_empty() {
                Ok(out.message())
            } else {
                Err(anyhow::anyhow!(out.message()))
            }
        });
    }

    /// The diff menu's entry while the shown commit misses objects.
    pub(super) fn lfs_download_menu_items(&self) -> Vec<MenuItem> {
        if self.lfs_download_target().is_none() {
            return Vec::new();
        }
        vec![MenuItem::new(LABEL, |app: &mut App| app.lfs_download()).key('F')]
    }

    /// The palette's entry while the shown commit misses objects.
    pub(super) fn lfs_download_palette_items(&self) -> Vec<PickItem> {
        let Some(dl) = self.lfs_download_target() else {
            return Vec::new();
        };
        let n = dl.files.len();
        let hint = format!(
            "{n} file{} — into the local LFS store (git lfs fetch), working files unchanged",
            if n == 1 { "" } else { "s" }
        );
        vec![PickItem::new(LABEL, hint, |app: &mut App| app.lfs_download())]
    }
}
