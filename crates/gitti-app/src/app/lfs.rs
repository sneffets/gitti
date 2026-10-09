//! Git LFS in gitti (logic in `gitti_core::lfs`): the LFS row of a pointer change
//! (`DiffRow::Note`, `diff_view.rs`), the refusal of hunk / line staging of LFS files, and
//! file locking — "Lock File (LFS)" / "Unlock File (LFS)" in the diff's context menu and
//! the palette for the file at the cursor, the "locked by …" badge on file rows (the locks
//! are read once per tab with `git lfs locks`, which asks the server, and after each lock
//! / unlock).
//!
//! "Track with Git LFS…" (logic in `gitti_core::lfs_track`): git-lfs is checked in the
//! background first — missing, a dialog says how to install it; not set up, the dialog
//! offers `git lfs install` — then the pattern for the top-level `.gitattributes` or a
//! folder's own one (as "Add to .gitignore…").

use std::rc::Rc;

use gitti_core::ignore::rule_dirs;
use gitti_core::lfs::{LfsLock, RepoLfs};
use gitti_core::lfs_track::{
    EXPLANATION, LfsSetup, LfsTrackOptions, NOT_INSTALLED, NOT_SET_UP, refusal,
    track_suggestions,
};
use gitti_core::patch::PatchMode;
use gitti_core::rows::Src;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::GittiApp;
use super::modal::ModalSpec;
use super::picker::PickerItem;
use super::repo_actions::{RuleTarget, pattern_label, rule_targets};

type AppHandle = WeakEntity<GittiApp>;

/// A tab's LFS state.
#[derive(Debug, Default)]
pub(super) struct LfsUi {
    /// What is wrong with LFS, if anything (`lfs_health.rs`).
    pub health: super::lfs_health::LfsHealthUi,
    pub state: RepoLfs,
    locks_asked: bool,
    pub locks: Vec<LfsLock>,
}

/// The diff's context menu: Lock / Unlock for the LFS file right-clicked on (or at the
/// cursor), when git-lfs is installed.
pub(super) fn diff_menu_items(menu: PopupMenu, app: &AppHandle, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let row = {
        let a = this.read(cx);
        a.menu_row.unwrap_or_else(|| a.diff_cursor_row())
    };
    let Some((path, locked)) = this.read(cx).lfs_lock_target(row) else {
        return menu;
    };
    let label = if locked {
        "Unlock File (LFS)"
    } else {
        "Lock File (LFS)"
    };
    let app = app.clone();
    menu.separator().item(
        super::menus::menu_entry(label).on_click(move |_, window, cx| {
            app.update(cx, |this, cx| {
                this.menu_row = None;
                this.lfs_toggle_lock(path.clone(), locked, window, cx)
            })
            .ok();
        }),
    )
}

impl GittiApp {
    /// A refresh's LFS state: the health check when due (git-lfs missing included),
    /// the locks once (when git-lfs can lock).
    pub(super) fn apply_lfs_state(&mut self, state: RepoLfs, cx: &mut Context<Self>) {
        self.lfs.state = state;
        self.lfs_health_after_refresh(state.uses, cx);
        if state.can_lock() && !self.lfs.locks_asked {
            self.lfs.locks_asked = true;
            self.load_lfs_locks(cx);
        }
    }

    /// Reads the locks in the background (a server without locking: none shown).
    fn load_lfs_locks(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { repo.lfs_locks() });
        cx.spawn(async move |this, cx| {
            let locks = task.await;
            this.update(cx, |this, cx| {
                if let Ok(locks) = locks {
                    this.lfs.locks = locks;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The lock of `path`, if someone holds one.
    pub(super) fn lfs_lock_of(&self, path: &str) -> Option<&LfsLock> {
        self.lfs.locks.iter().find(|l| l.path == path)
    }

    /// The LFS file of diff row `row` and whether it is locked — when locking is possible.
    pub(super) fn lfs_lock_target(&self, row: usize) -> Option<(String, bool)> {
        if !self.lfs.state.can_lock() {
            return None;
        }
        let (src, file) = self.rows.get(row)?.file_ref()?;
        let diff = self.file_diff(src, file)?;
        diff.lfs.as_ref()?;
        let path = diff.path().to_string();
        let locked = self.lfs_lock_of(&path).is_some();
        Some((path, locked))
    }

    /// `git lfs lock` / `unlock` of `path`, then the locks read again.
    pub(super) fn lfs_toggle_lock(
        &mut self,
        path: String,
        locked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = if locked { "Unlock" } else { "Lock" };
        let done = format!("{label}ed {path}");
        self.run_op(
            label,
            false,
            window,
            cx,
            move |r| {
                if locked {
                    r.lfs_unlock(&path, false)?;
                } else {
                    r.lfs_lock(&path)?;
                }
                // The new list (a failure to read it keeps the old one).
                Ok(r.lfs_locks().ok())
            },
            move |this, locks, _, _| {
                if let Some(locks) = locks {
                    this.lfs.locks = locks;
                }
                this.set_info(done);
            },
        );
    }

    /// Palette entries: Lock / Unlock of the LFS file at the diff cursor.
    pub(super) fn lfs_palette_items(&self) -> Vec<PickerItem> {
        let Some((path, locked)) = self.lfs_lock_target(self.diff_cursor_row()) else {
            return Vec::new();
        };
        let label = if locked {
            "Unlock File (LFS)"
        } else {
            "Lock File (LFS)"
        };
        vec![PickerItem {
            label: label.into(),
            detail: path.clone(),
            keys: String::new(),
            color: None,
            run: Rc::new(move |t, w, cx| t.lfs_toggle_lock(path.clone(), locked, w, cx)),
            action: None,
        }]
    }

    /// "Track with Git LFS…" for the working-directory file of diff row `row` (an empty
    /// pattern elsewhere).
    pub(super) fn lfs_track_dialog_for_row(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.work_target(row).map(|(p, _)| p).unwrap_or_default();
        self.lfs_track_dialog(path, window, cx);
    }

    /// "Track with Git LFS…" for `path` (empty: no file): git-lfs checked in the background
    /// (`git lfs version` once per process, its filters in the config), then the dialog.
    pub fn lfs_track_dialog(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if let Some(why) = refusal(&path) {
            self.set_error(why);
            cx.notify();
            return;
        }
        let task = cx
            .background_executor()
            .spawn(async move { repo.lfs_setup() });
        cx.spawn_in(window, async move |this, cx| {
            let setup = task.await;
            this.update_in(cx, |this, window, cx| {
                this.open_lfs_track_dialog(&path, setup, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// git-lfs missing: what to do. Else the target `.gitattributes` (the top-level one or
    /// a folder's own one; the nearest existing one preselected) with the pattern and its
    /// suggestions, staging the file, `lockable`, and `git lfs install` when not set up.
    fn open_lfs_track_dialog(
        &mut self,
        path: &str,
        setup: LfsSetup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if setup.version.is_none() {
            let spec = ModalSpec::new("Git LFS is not installed")
                .message(NOT_INSTALLED)
                .ok("OK")
                .cancel(None);
            self.open_modal(spec, window, cx, |_, _, _, _| {});
            return;
        }
        let dirs = rule_dirs(repo.workdir(), path, ".gitattributes");
        let file_of = |d: &str| {
            if d.is_empty() {
                ".gitattributes".to_string()
            } else {
                format!("{d}/.gitattributes")
            }
        };
        let selected = dirs
            .iter()
            .rposition(|d| !d.is_empty() && repo.workdir().join(file_of(d)).is_file())
            .unwrap_or(0);
        let targets: Vec<RuleTarget> = dirs
            .iter()
            .map(|d| RuleTarget {
                option: file_of(d),
                field: pattern_label(d),
                choices: track_suggestions(path, d),
            })
            .collect();
        let mut message = EXPLANATION.to_string();
        if !setup.filters {
            message.push_str(&format!(
                "\n\n{NOT_SET_UP} Keep \"Set up Git LFS\" checked to run git lfs install \
                 (adds its filters to your global git config)."
            ));
        }
        let mut spec = rule_targets(
            ModalSpec::new("Track with Git LFS").message(message),
            &targets,
            selected,
        )
        .check("Stage .gitattributes", true)
        .check(
            "Lockable: matching files stay read-only until you lock them (for files nobody can merge)",
            false,
        );
        if !setup.filters {
            spec = spec.check(
                "Set up Git LFS for my user account first (git lfs install)",
                true,
            );
        }
        let spec = spec.ok("Track");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let Some(dir) = dirs.get(res.option).cloned() else {
                return;
            };
            let pattern = res.values[res.option].clone();
            let opts = LfsTrackOptions {
                stage: res.checks[0],
                lockable: res.checks[1],
                install: res.checks.get(2).copied().unwrap_or(false),
            };
            this.run_op(
                "Track with Git LFS",
                true,
                w,
                cx,
                move |r| r.lfs_track(&dir, &pattern, opts),
                |this, out, _, _| {
                    if out.hook_warning.is_some() {
                        this.set_error(out.message())
                    } else {
                        this.set_info(out.message())
                    }
                },
            );
        });
    }

    /// An LFS file has no hunk / line operations (its index entry is the pointer): says so
    /// and returns true, before anything runs.
    pub(super) fn refuse_lfs(
        &mut self,
        src: Src,
        file: usize,
        mode: PatchMode,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(diff) = self.file_diff(src, file) else {
            return false;
        };
        match gitti_core::lfs::check_partial(diff, mode) {
            Ok(()) => false,
            Err(e) => {
                self.set_error(e.to_string());
                cx.notify();
                true
            }
        }
    }
}
