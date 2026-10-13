//! Repository Statistics (palette, and the menu of the repository's name in the top line;
//! gitti: Repository ▸ Repository Statistics…): a text window with what the repository is
//! made of and how big it is — text and reading in `gitti_core::repo_stats`, as in gitti.
//!
//! Its three parts are read in background jobs and fill in as they arrive: the overview
//! (in-process; the commits are counted on the loaded history), the sizes (one `git
//! count-objects`) and the commits per author of the newest commits. `a` counts the
//! authors of a big history whole, with progress; closing the window stops it. `y` copies
//! the text. `e` opens Commit Encoding… (`i18n.commitEncoding` for new commits; also in
//! the palette), whose texts are `gitti_core::commit_encoding::change`'s, as in gitti.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gitti_core::commit_encoding::change;
use gitti_core::ops::ConfigScope;
use gitti_core::repo_stats::{self, AllCount, Scope, Stats, TextOptions};

use super::{After, App, MenuItem, Modal, Msg, spawn};
use crate::modal::{Live, TextKey};

/// The statistics window's state.
#[derive(Default)]
pub struct StatsUi {
    stats: Stats,
    /// Stops the whole-history count of this window.
    cancel: Arc<AtomicBool>,
    /// Counts up with every new window: results of an older one are dropped.
    generation: u64,
}

impl App {
    /// Opens the window and starts reading its parts.
    pub fn show_repo_stats(&mut self) {
        let ui = &mut self.repo_stats;
        ui.cancel.store(true, Ordering::Relaxed);
        ui.cancel = Arc::new(AtomicBool::new(false));
        ui.generation += 1;
        ui.stats = Stats::default();
        let generation = ui.generation;
        // An empty history is not loaded yet (an unborn one has the working-directory row).
        let history = (!self.history.commits.is_empty()).then(|| self.history.clone());
        self.modal = Some(Modal::text(repo_stats::TITLE, self.repo_stats_text()));
        self.repo_stats_changed();

        let (repo, h) = (self.repo.clone(), history.clone());
        self.loads_in_flight += 1;
        spawn(
            &self.tx,
            move || repo.stats_overview(h.as_deref()),
            move |o| apply(generation, move |s| s.overview = Some(o)),
        );
        let repo = self.repo.clone();
        self.loads_in_flight += 1;
        spawn(
            &self.tx,
            move || repo.stats_storage().map_err(|e| format!("{e:#}")),
            move |st| apply(generation, move |s| s.storage = Some(st)),
        );
        if let Some(history) = history {
            let scope = if history.commits.len() <= repo_stats::ALL_AT_ONCE {
                Scope::All
            } else {
                Scope::Newest(repo_stats::RECENT_COMMITS)
            };
            let cancel = self.repo_stats.cancel.clone();
            self.loads_in_flight += 1;
            spawn(
                &self.tx,
                move || repo_stats::authors(&history, scope, &cancel, &mut |_, _| {}),
                move |a| apply(generation, move |s| s.authors = a),
            );
        }
    }

    /// `a`: the authors of the whole history, counted in the background with progress.
    fn count_all_authors(&mut self) {
        if !self.repo_stats.stats.offers_count_all() {
            return;
        }
        let total = self.repo_stats.stats.authors.as_ref().map_or(0, |a| a.total);
        self.repo_stats.stats.all = AllCount::Counting(0, total);
        self.repo_stats_changed();
        let generation = self.repo_stats.generation;
        let cancel = self.repo_stats.cancel.clone();
        let history = self.history.clone();
        let tx = self.tx.clone();
        self.loads_in_flight += 1;
        spawn(
            &self.tx,
            move || {
                repo_stats::authors(&history, Scope::All, &cancel, &mut |done, _| {
                    let _ = tx.send(Msg::RepoStatsProgress(generation, done));
                })
            },
            move |a| {
                apply(generation, move |s| {
                    s.all = match a {
                        Some(a) => AllCount::Done(a),
                        None => AllCount::Offered,
                    }
                })
            },
        );
    }

    /// `Msg::RepoStatsProgress`: the whole count got this far — or stops, the window is
    /// gone.
    pub(super) fn repo_stats_progress(&mut self, generation: u64, done: usize) {
        let ui = &mut self.repo_stats;
        if ui.generation != generation {
            return;
        }
        if !self.repo_stats_shown() {
            self.repo_stats.cancel.store(true, Ordering::Relaxed);
            return;
        }
        if let AllCount::Counting(n, _) = &mut self.repo_stats.stats.all {
            *n = done;
        }
        self.repo_stats_changed();
    }

    /// The statistics window is the open one.
    fn repo_stats_shown(&self) -> bool {
        matches!(&self.modal, Some(Modal::Text { title, .. }) if title == repo_stats::TITLE)
    }

    fn repo_stats_text(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        repo_stats::text(
            &self.repo_stats.stats,
            &TextOptions {
                date: &|secs| {
                    let t = super::local_time(secs.saturating_mul(1000));
                    // Minutes are enough: `2026-10-06 09:12`.
                    t.get(..16).unwrap_or(&t).to_string()
                },
                now,
                count_all: "a",
                change_encoding: "e (also Commit Encoding… in the palette)",
            },
        )
    }

    /// A part arrived: the open window shows it (where it was scrolled to), with its keys.
    fn repo_stats_changed(&mut self) {
        if !self.repo_stats_shown() {
            return;
        }
        let text = self.repo_stats_text();
        let mut keys = vec![TextKey {
            key: 'y',
            label: "copy as text".into(),
            keep: true,
            run: |app| {
                let text = app.repo_stats_text();
                let lines = text.lines().count();
                app.copy(text);
                app.set_info(format!("Copied {lines} lines"));
            },
        }];
        keys.push(TextKey {
            key: 'e',
            label: "commit encoding".into(),
            keep: false,
            run: App::commit_encoding_dialog,
        });
        if self.repo_stats.stats.offers_count_all() {
            keys.push(TextKey {
                key: 'a',
                label: "count all commits".into(),
                keep: true,
                run: App::count_all_authors,
            });
        }
        if let Some(Modal::Text { body, keys: k, .. }) = &mut self.modal {
            *body = text;
            *k = keys;
        }
    }

    /// Commit Encoding… (`e` in the statistics, the palette): `i18n.commitEncoding` for
    /// new commits, in this repository or globally (the check) — the dialog says which
    /// file gets written and what follows, as gitti's.
    pub fn commit_encoding_dialog(&mut self) {
        let current = self.repo.commit_encoding_shown();
        let global = change::initial_scope(&current) == ConfigScope::Global;
        let scope = |checks: &[bool]| {
            if checks.first().copied().unwrap_or(false) { ConfigScope::Global } else { ConfigScope::Local }
        };
        let value = current.configured.clone().unwrap_or_default();
        let intro = change::intro(&current);
        self.modal = Some(
            Modal::input_checks(
                change::TITLE,
                vec![(change::FIELD_LABEL, value.as_str())],
                vec![(change::scope_label(ConfigScope::Global), global)],
                move |app: &mut App, values, checks| {
                    let value = values.into_iter().next().unwrap_or_default();
                    let scope = scope(&checks);
                    app.run_op("Commit encoding", true, After::Nothing, move |r| {
                        r.set_commit_encoding(scope, &value)
                    });
                },
            )
            .with_note(intro)
            .with_live(move |values, checks| {
                let value = values.first().map(String::as_str).unwrap_or("");
                let (text, blocked) = match change::describe(value, scope(checks), &current) {
                    Ok(text) => (text, None),
                    Err(why) => (String::new(), Some(why)),
                };
                Live {
                    text,
                    ok_label: "Save".into(),
                    blocked,
                    fills: change::COMMON.iter().map(|n| (0, n.to_string())).collect(),
                    fills_label: "Common".into(),
                    ..Default::default()
                }
            }),
        );
    }

    /// The repository name's menu (a click on it in the top line; gitti: a right click
    /// on it in the toolbar).
    pub fn repo_name_menu(&mut self) {
        let fm = gitti_core::desktop::file_manager_name();
        let path = self.repo.workdir().to_path_buf();
        let shown = path.display().to_string();
        let items = vec![
            MenuItem::new(repo_stats::MENU_LABEL, |app: &mut App| app.show_repo_stats()),
            MenuItem::separator(),
            MenuItem::new(format!("Open in {fm}"), |app: &mut App| app.open_folder()),
            MenuItem::new("Open in Terminal", move |app: &mut App| app.open_terminal_at(path)),
            MenuItem::new("Copy Path", move |app: &mut App| app.copy(shown)),
        ];
        self.modal = Some(Modal::menu(self.repo.name(), items));
    }
}

/// The job's message: `put` into the stats of window `generation`, if it is the latest.
fn apply(generation: u64, put: impl FnOnce(&mut Stats) + Send + 'static) -> Msg {
    Msg::Apply(Box::new(move |app: &mut App| {
        if app.repo_stats.generation == generation {
            put(&mut app.repo_stats.stats);
            app.repo_stats_changed();
        }
    }))
}
