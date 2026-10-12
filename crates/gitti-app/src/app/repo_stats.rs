//! Repository ▸ Repository Statistics… (also the palette and a right click on the
//! repository's name in the toolbar): a text window with what the repository is made of
//! and how big it is — text and reading in `gitti_core::repo_stats`, the same in gitty.
//!
//! Its three parts are read in the background and fill in as they arrive: the overview
//! (in-process; the commits are counted on the loaded history), the sizes (one `git
//! count-objects`) and the commits per author of the newest commits. "Count All Commits"
//! counts the authors of a big history whole, with progress; closing the window stops it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use gitti_core::History;
use gitti_core::repo_stats::{self, AllCount, Authors, Scope, Stats, TextOptions};
use gpui_kit::component::input::TextareaState;
use gpui_kit::*;

use super::GittiApp;
use super::modal::ModalSpec;

/// How often a running whole-history count shows its progress (and checks the window).
const PROGRESS_POLL: std::time::Duration = std::time::Duration::from_millis(150);

/// The statistics window's state.
#[derive(Default)]
pub(super) struct StatsUi {
    stats: Stats,
    /// The open window's text (`None`: closed or replaced by another dialog).
    area: Option<Entity<TextareaState>>,
    /// Stops this window's whole-history count.
    cancel: Arc<AtomicBool>,
    /// Counts up with every new window: results of an older one are dropped.
    generation: u64,
}

impl GittiApp {
    /// Opens the window and starts reading its parts.
    pub(super) fn show_repo_stats(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let ui = &mut self.repo_stats;
        ui.cancel.store(true, Ordering::Relaxed);
        ui.cancel = Arc::new(AtomicBool::new(false));
        ui.generation += 1;
        ui.stats = Stats::default();
        // An empty history is not loaded yet (an unborn one has the working-directory row).
        let history = (!self.history.commits.is_empty()).then(|| self.history.clone());
        self.open_repo_stats_window(window, cx);

        let overview = {
            let (repo, history) = (repo.clone(), history.clone());
            cx.background_executor()
                .spawn(async move { repo.stats_overview(history.as_deref()) })
        };
        self.repo_stats_part(overview, window, cx, |s, o| s.overview = Some(o));
        let storage = {
            let repo = repo.clone();
            cx.background_executor()
                .spawn(async move { repo.stats_storage().map_err(|e| format!("{e:#}")) })
        };
        self.repo_stats_part(storage, window, cx, |s, st| s.storage = Some(st));
        if let Some(history) = history {
            let scope = if history.commits.len() <= repo_stats::ALL_AT_ONCE {
                Scope::All
            } else {
                Scope::Newest(repo_stats::RECENT_COMMITS)
            };
            let cancel = self.repo_stats.cancel.clone();
            let authors = cx.background_executor().spawn(async move {
                repo_stats::authors(&history, scope, &cancel, &mut |_, _| {})
            });
            self.repo_stats_part(authors, window, cx, |s, a| s.authors = a);
        }
    }

    /// The window over what has arrived so far: Copy as Text, and Count All Commits while
    /// the authors are only the newest commits' and the whole count hasn't started.
    fn open_repo_stats_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.repo_stats_text();
        let mut spec = ModalSpec::new(repo_stats::TITLE)
            .ok("Close")
            .cancel(None)
            .copy(repo_stats::COPY_LABEL, text.clone());
        let big = self.history.commits.len() > repo_stats::ALL_AT_ONCE;
        if big && matches!(self.repo_stats.stats.all, AllCount::Offered) {
            spec = spec.extra(repo_stats::COUNT_ALL_LABEL, |t, w, cx| t.count_all_authors(w, cx));
        }
        self.show_text_modal_spec(spec, text, window, cx);
        self.repo_stats.area = self.modal.as_ref().and_then(|m| m.text_area()).cloned();
    }

    /// Waits for `task` in the background and puts its result into the stats with `put`
    /// (when the window it was started for is still the latest).
    fn repo_stats_part<T: 'static>(
        &mut self,
        task: Task<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
        put: impl FnOnce(&mut Stats, T) + 'static,
    ) {
        let generation = self.repo_stats.generation;
        cx.spawn_in(window, async move |this, cx| {
            let value = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.repo_stats.generation == generation {
                    put(&mut this.repo_stats.stats, value);
                    this.repo_stats_changed(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Count All Commits": the window again (its button closed it), the authors of the
    /// whole history counted in the background with progress; closing the window stops it.
    fn count_all_authors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let history = self.history.clone();
        let total = self
            .repo_stats
            .stats
            .authors
            .as_ref()
            .map_or(history.commits.len(), |a| a.total);
        self.repo_stats.stats.all = AllCount::Counting(0, total);
        self.open_repo_stats_window(window, cx);
        let generation = self.repo_stats.generation;
        let cancel = self.repo_stats.cancel.clone();
        let done = Arc::new(AtomicUsize::new(0));
        let result: Arc<std::sync::Mutex<Option<Option<Authors>>>> = Arc::default();
        {
            let (cancel, done, result) = (cancel.clone(), done.clone(), result.clone());
            cx.background_executor()
                .spawn(async move {
                    let a = count_all(&history, &cancel, &done);
                    if let Ok(mut r) = result.lock() {
                        *r = Some(a);
                    }
                })
                .detach();
        }
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_POLL).await;
                let finished = result.lock().ok().and_then(|mut r| r.take());
                let go_on = this.update_in(cx, |this, window, cx| {
                    let ui = &mut this.repo_stats;
                    if ui.generation != generation {
                        return false;
                    }
                    // The window is gone: stop counting.
                    if !this.repo_stats_shown() {
                        cancel.store(true, Ordering::Relaxed);
                        this.repo_stats.stats.all = AllCount::Offered;
                        return false;
                    }
                    let all = &mut this.repo_stats.stats.all;
                    let keep_going = match finished {
                        Some(Some(a)) => {
                            *all = AllCount::Done(a);
                            false
                        }
                        Some(None) => {
                            *all = AllCount::Offered;
                            false
                        }
                        None => {
                            *all = AllCount::Counting(done.load(Ordering::Relaxed), total);
                            true
                        }
                    };
                    this.repo_stats_changed(window, cx);
                    keep_going
                });
                if !matches!(go_on, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The statistics window is the open dialog.
    fn repo_stats_shown(&self) -> bool {
        let area = self.modal.as_ref().and_then(|m| m.text_area());
        area.is_some() && area == self.repo_stats.area.as_ref()
    }

    fn repo_stats_text(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        repo_stats::text(
            &self.repo_stats.stats,
            &TextOptions {
                date: &local_date,
                now,
                count_all: "The Count All Commits button",
                change_encoding: "Repository ▸ Commit Encoding… (also in the palette)",
            },
        )
    }

    /// A part arrived: the open window shows it (where it was scrolled to), Copy copies it.
    fn repo_stats_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.repo_stats_shown() {
            return;
        }
        let text = self.repo_stats_text();
        if let Some(area) = self.repo_stats.area.clone() {
            area.update(cx, |s, cx| {
                let offset = s.scroll_offset();
                s.set_value(text.clone(), window, cx);
                s.set_scroll_offset(offset, cx);
            });
        }
        if let Some(m) = self.modal.as_mut() {
            m.set_copy_text(text);
        }
        cx.notify();
    }
}

/// The whole history's authors, `done` counting up.
fn count_all(history: &History, cancel: &AtomicBool, done: &AtomicUsize) -> Option<Authors> {
    repo_stats::authors(history, Scope::All, cancel, &mut |d, _| {
        done.store(d, Ordering::Relaxed)
    })
}

/// `2026-10-06 09:12` in local time (seconds since the epoch).
fn local_date(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}
