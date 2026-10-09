//! The file watcher's changes: the refresh each one needs — an outside edit of work-tree
//! files re-reads just those paths, the index or worktree metadata the working state, refs
//! everything (`gitti_core::watch::WatchFilter::needs`).

use std::time::SystemTime;

use gitti_core::watch::{Change, Refresh};
use gpui_kit::*;

use super::{GittiApp, highlighting};

impl GittiApp {
    /// Starts the file watcher; its changes arrive here. It adds its watches on its own
    /// thread, so the first frame doesn't wait for it.
    pub(super) fn start_watcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let (tx, rx) = async_channel::unbounded();
        match gitti_core::watch::watch(repo.workdir(), move |change| {
            let _ = tx.send_blocking(change);
        }) {
            Ok(w) => self._watcher = Some(w),
            Err(e) => {
                self.set_error(format!("file watcher failed: {e:#}"));
                return;
            }
        }
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(change) = rx.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    if this.busy.is_none() {
                        this.on_watch(change, window, cx);
                    } else {
                        this.watch_pending = Some(match this.watch_pending.take() {
                            Some(c) => c.merge(change),
                            None => change,
                        });
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// A file-system change: refreshes what it touched, unless that was read after it
    /// already. The watcher's notice (watch limit reached) shows once.
    pub(super) fn on_watch(&mut self, change: Change, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if let Some(notice) = &change.notice {
            self.set_error(notice.clone());
            cx.notify();
        }
        match self.watch_filter.needs(&change, || repo.index_stamp()) {
            Some(Refresh::Full) => self.refresh(true, window, cx),
            Some(Refresh::Snapshot) => self.refresh(false, window, cx),
            Some(Refresh::Paths(paths)) => self.refresh_paths(paths, window, cx),
            None => {}
        }
    }

    /// An outside change of work-tree files only (an editor saved, a tool wrote): re-reads
    /// the working state of those paths (status + diffs, as `work_op` after an
    /// operation) and highlights them — no whole-repository status, refs, remotes, ….
    /// One at a time; a refresh queued meanwhile reads them anyway.
    pub(super) fn refresh_paths(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if self.refresh_queued.is_some() {
            return;
        }
        if self.refresh_in_flight || self.paths_in_flight {
            self.paths_queued.get_or_insert_with(Vec::new).extend(paths);
            return;
        }
        self.paths_in_flight = true;
        let generation = self.working_generation;
        let prev = self.working.clone();
        let opts = self.settings.diff_options();
        let cache = self
            .settings
            .syntax_highlight
            .then(|| self.hl_cache.clone());
        let prev_subs = self.submodules.clone();
        let task = cx.background_executor().spawn(async move {
            let started = SystemTime::now();
            let working = repo.working_state_after(&opts, &prev, &paths);
            // A directory stands for the files below it: those are highlighted again.
            let touched = working
                .as_ref()
                .ok()
                .map(|w| gitti_core::touched_files(&prev, w, &paths));
            let styles = match (&working, &touched, cache) {
                (Ok(w), Some(t), Some(cache)) => Some(highlighting::highlight_paths(&repo, w, t, &cache)),
                _ => None,
            };
            let subs = working
                .as_ref()
                .ok()
                .map(|w| repo.submodules_after(&w.status, &prev_subs));
            (working, styles, touched, subs, started, paths, opts)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (working, styles, touched, subs, started, paths, read_with) = task.await;
            this.update_in(cx, |this, window, cx| {
                this.paths_in_flight = false;
                let current = this.working_generation == generation
                    && this.settings.diff_options() == read_with;
                match working {
                    Ok(w) if current => {
                        this.working_generation += 1;
                        this.watch_filter.paths_read(&paths, started);
                        if let Some(subs) = subs {
                            this.submodules = subs;
                        }
                        this.apply_working(w, styles, touched, cx);
                    }
                    // An operation or a reload changed the working state meanwhile (this
                    // read started from an older one), or it failed: read it all.
                    _ => this.refresh(false, window, cx),
                }
                if let Some(full) = this.refresh_queued.take() {
                    this.refresh(full, window, cx);
                } else if let Some(paths) = this.paths_queued.take() {
                    this.refresh_paths(paths, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
