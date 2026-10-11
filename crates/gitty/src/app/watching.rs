//! The file watcher's changes (as gitti's `app/watching.rs`): the refresh each one
//! needs — an outside edit of work-tree files re-reads just those paths, the index or
//! worktree metadata the working state, refs everything
//! (`gitti_core::watch::WatchFilter::needs`).

use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime};

use anyhow::Result;
use gitti_core::rows::Src;
use gitti_core::watch::{Change, Refresh};
use gitti_core::{DiffOptions, ReadScope, WorkingState};

use super::{App, FileHighlight, Msg, compute_highlights, spawn, worktree_jobs};

/// How long the first read waits at most for the file watcher's first report.
const FIRST_READ_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// A path-limited re-read of an outside edit ([`App::refresh_paths`]).
pub struct PathsRead {
    /// The working-state generation it started from.
    generation: u64,
    opts: DiffOptions,
    started: (Instant, SystemTime),
    working: Result<(WorkingState, ReadScope)>,
    /// The files the paths cover (a directory: those below it) and their highlights.
    touched: Option<Vec<String>>,
    highlights: Option<HashMap<(Src, String), FileHighlight>>,
}

impl App {
    /// A batch from the file watcher.
    pub(super) fn on_fs(&mut self, change: Change) {
        self.watch_ready |= change.armed;
        if change.armed && std::mem::take(&mut self.first_read_pending) {
            self.refresh(true);
        }
        self.merge_check_disk();
        if self.busy.is_some() {
            // The operation's own re-read decides when it ends.
            self.watch_pending = Some(match self.watch_pending.take() {
                Some(c) => c.merge(change),
                None => change,
            });
        } else {
            self.on_watch(change);
        }
    }

    /// The first full read (working state, refs, history) — once the file watcher's
    /// watches are in place: with a recursive OS watch (macOS, Windows) a change before
    /// that point is never reported, so a read started earlier could miss it for good
    /// (Linux finds such changes by their mtime). Waiting costs the watcher's setup time
    /// (a few ms); without a report within [`FIRST_READ_WAIT`] it reads anyway (the
    /// report then asks for a re-read).
    pub(super) fn first_read(&mut self) {
        if self.watch_ready {
            self.refresh(true);
            return;
        }
        self.first_read_pending = true;
        spawn(&self.tx, || std::thread::sleep(FIRST_READ_WAIT), |_| Msg::FirstReadDue);
    }

    pub(super) fn first_read_due(&mut self) {
        if std::mem::take(&mut self.first_read_pending) {
            self.refresh(true);
        }
    }

    /// Refreshes what `change` touched, unless that was read after it already. The
    /// watcher's notice (watch limit reached) shows once.
    pub(super) fn on_watch(&mut self, change: Change) {
        if let Some(notice) = &change.notice {
            self.set_error(notice.clone());
        }
        match self.watch_filter.needs(&change, || self.repo.index_stamp()) {
            Some(Refresh::Full) => self.refresh(true),
            Some(Refresh::Snapshot) => self.refresh(false),
            Some(Refresh::Paths(paths)) => self.refresh_paths(paths),
            None => {}
        }
    }

    /// An outside change of work-tree files only (an editor saved, a tool wrote): re-reads
    /// the working state of those paths (status + diffs, as `work_op` after an
    /// operation) and highlights them — no whole-repository status, refs, remotes, ….
    /// One at a time; a refresh queued meanwhile reads them anyway.
    pub(super) fn refresh_paths(&mut self, paths: Vec<String>) {
        if self.refresh_queued.is_some() {
            return;
        }
        if self.refresh_in_flight || self.paths_in_flight {
            self.paths_queued.get_or_insert_with(Vec::new).extend(paths);
            return;
        }
        self.paths_in_flight = true;
        let generation = self.working_gen;
        let repo = self.repo.clone();
        let prev = self.working.clone();
        let opts = self.opts.diff_options();
        let cache = self.opts.syntax.then(|| self.hl_cache.clone());
        spawn(
            &self.tx,
            move || {
                let started = (Instant::now(), SystemTime::now());
                let working = repo.working_state_reread(&opts, &prev, &paths);
                // A directory stands for the files below it: those are highlighted again.
                let touched = working
                    .as_ref()
                    .ok()
                    .map(|(w, _)| gitti_core::touched_files(&prev, w, &paths));
                let highlights = match (&working, &touched, cache) {
                    (Ok((w, _)), Some(t), Some(cache)) => {
                        let only: HashSet<&str> = t.iter().map(String::as_str).collect();
                        Some(compute_highlights(&repo, worktree_jobs(w, Some(&only)), &cache))
                    }
                    _ => None,
                };
                PathsRead {
                    generation,
                    opts,
                    started,
                    working,
                    touched,
                    highlights,
                }
            },
            |r| Msg::PathsRead(Box::new(r)),
        );
    }

    pub(super) fn on_paths_read(&mut self, r: PathsRead) {
        self.paths_in_flight = false;
        let current = r.generation == self.working_gen && r.opts == self.opts.diff_options();
        match r.working {
            Ok((w, scope)) if current => {
                self.working_gen += 1;
                self.watch_filter.reread(&scope, r.started);
                self.apply_working(w, r.highlights, r.touched);
                self.refresh_submodule_states();
            }
            // An operation or a reload changed the working state meanwhile (this read
            // started from an older one), or it failed: read it all.
            _ => self.refresh(false),
        }
        if let Some(full) = self.refresh_queued.take() {
            self.refresh(full);
        } else if let Some(paths) = self.paths_queued.take() {
            self.refresh_paths(paths);
        }
    }
}
