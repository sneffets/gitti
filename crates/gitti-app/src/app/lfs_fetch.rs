//! "Download LFS Objects" (logic in `gitti_core::lfs_fetch`): a commit, stash or
//! comparison whose LFS rows say "content not in the local LFS store" gets a button on
//! those rows, an entry in the diff's context menu and one in the palette. It runs `git
//! lfs fetch` for the shown commit (and its parents, or both sides of the comparison) as
//! an operation — busy mark, progress, askpass for credentials — and shows the view again
//! re-annotated, keeping its place (the core drops the stale cached entries itself).

use std::rc::Rc;
use std::sync::Arc;

use gitti_core::commit_cache::CommitData;
use gitti_core::compare::CompareData;
use gitti_core::lfs::LfsCaps;
use gitti_core::lfs_fetch::{EXPLANATION, LfsDownload, LfsFetchOutcome, NOT_INSTALLED};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::picker::PickerItem;
use super::{CommitView, Detail, GittiApp};

type AppHandle = WeakEntity<GittiApp>;

/// The action's label (row button, context menu, palette).
pub(super) const LABEL: &str = "Download LFS Objects";
/// The operation's label (busy mark, errors).
const OP: &str = "Download LFS objects";

/// The diff's context menu: "Download LFS Objects" when the shown commit misses some.
pub(super) fn diff_menu_items(menu: PopupMenu, app: &AppHandle, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    if this.read(cx).lfs_download_target().is_none() {
        return menu;
    }
    let app = app.clone();
    menu.separator().item(
        super::menus::menu_entry(LABEL).on_click(move |_, window, cx| {
            app.update(cx, |this, cx| {
                this.menu_row = None;
                this.lfs_download(window, cx)
            })
            .ok();
        }),
    )
}

/// What the reloaded view brings back: the data and, for a comparison, its pair.
type Reloaded = Option<(Arc<CommitData>, Option<Arc<CompareData>>)>;

impl GittiApp {
    /// The objects the shown commit, stash or comparison misses locally.
    pub(super) fn lfs_download_target(&self) -> Option<LfsDownload> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        LfsDownload::of_view(&cv.data, cv.compare.as_deref())
    }

    /// Downloads them (`git lfs fetch`), then shows the view again with the content.
    /// Without git-lfs it says so (and how to get it) before anything runs.
    pub(super) fn lfs_download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(dl), Detail::Commit(cv)) = (self.lfs_download_target(), &self.detail) else {
            self.set_info("No LFS objects are missing here");
            cx.notify();
            return;
        };
        if !LfsCaps::get().installed() {
            self.set_error(NOT_INSTALLED.to_string());
            cx.notify();
            return;
        }
        let (id, stash) = (cv.details.id, cv.stash);
        let pair = cv.compare.as_ref().map(|c| (c.old, c.new));
        let opts = self.settings.diff_options();
        self.run_op(
            OP,
            false,
            window,
            cx,
            move |r| {
                let outcome = r.lfs_fetch(&dl);
                // Re-annotated: the cached entry is stale once an object arrived (an
                // unchanged one comes from the cache, no git).
                let view: Reloaded = match pair {
                    Some((old, new)) => r
                        .compare_diff(old, new, &opts)
                        .ok()
                        .map(|c| (c.data.clone(), Some(c))),
                    None => r.load_commit(id, stash, &opts).ok().map(|d| (d, None)),
                };
                Ok((outcome, view))
            },
            |this, (outcome, view), _, cx| {
                if let Some((data, compare)) = view {
                    this.show_reannotated(data, compare, cx);
                }
                this.lfs_downloaded(outcome);
            },
        );
    }

    /// The result in the status line (a failure, also partial, as the operation's error).
    fn lfs_downloaded(&mut self, outcome: anyhow::Result<LfsFetchOutcome>) {
        match outcome {
            Ok(out) if out.missing.is_empty() => self.set_info(out.message()),
            Ok(out) => self.op_failed(OP, out.message()),
            Err(e) => self.op_failed(OP, format!("{e:#}")),
        }
    }

    /// The shown commit / comparison replaced by its re-annotated data, if it still is on
    /// screen; the rows above stay, so does the view.
    fn show_reannotated(
        &mut self,
        data: Arc<CommitData>,
        compare: Option<Arc<CompareData>>,
        cx: &mut Context<Self>,
    ) {
        let Detail::Commit(cv) = &self.detail else {
            return;
        };
        let same = match (&cv.compare, &compare) {
            (None, None) => cv.details.id == data.details.id,
            (Some(a), Some(b)) => (a.old, a.new) == (b.old, b.new),
            _ => false,
        };
        if !same || Arc::ptr_eq(&cv.data, &data) {
            return;
        }
        let stash = cv.stash;
        self.detail = Detail::Commit(Arc::new(CommitView {
            data,
            stash,
            compare,
        }));
        self.rebuild_rows_keep_top(cx);
        self.schedule_highlight(cx);
    }

    /// Palette: "Download LFS Objects" while the shown commit misses some.
    pub(super) fn lfs_download_palette_items(&self) -> Vec<PickerItem> {
        let Some(dl) = self.lfs_download_target() else {
            return Vec::new();
        };
        let n = dl.files.len();
        let detail = format!(
            "{n} file{} — {EXPLANATION}",
            if n == 1 { "" } else { "s" }
        );
        vec![PickerItem {
            label: LABEL.into(),
            detail,
            keys: String::new(),
            color: None,
            run: Rc::new(|t, w, cx| t.lfs_download(w, cx)),
            action: None,
        }]
    }
}
