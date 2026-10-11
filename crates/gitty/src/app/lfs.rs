//! Git LFS in gitty (as gitti's `app/lfs.rs`; logic in `gitti_core::lfs`): the
//! refusal of hunk / line staging of LFS files,
//! Lock / Unlock in the diff's menu and the palette, the locks for the file rows' badge
//! (read once with `git lfs locks` and again after each lock / unlock).
//!
//! "Track with Git LFS…" (as gitti's; logic in `gitti_core::lfs_track`): git-lfs is
//! checked in the background (`Msg::LfsSetup`) — missing, a text window says how to
//! install it — then a menu explaining LFS with the patterns for the top-level
//! `.gitattributes` and one entry per folder `.gitattributes`, then the pattern line.

use anyhow::Result;

use gitti_core::lfs::{LfsLock, RepoLfs};
use gitti_core::lfs_track::{
    EXPLANATION, LfsSetup, LfsTrackOptions, NOT_INSTALLED, NOT_SET_UP, refusal,
    track_suggestions,
};
use gitti_core::patch::PatchMode;
use gitti_core::rows::Src;

use super::{After, App, MenuItem, Modal, Msg, PickItem, spawn};

/// Width of the explanation lines in the "Track with Git LFS" menu.
const NOTE_WIDTH: usize = 64;

/// gitty's LFS state.
#[derive(Debug, Default)]
pub struct LfsUi {
    /// What is wrong with LFS, if anything (`lfs_health.rs`).
    pub health: super::lfs_health::LfsHealthUi,
    pub state: RepoLfs,
    locks_asked: bool,
    pub locks: Vec<LfsLock>,
}

impl App {
    /// A refresh's LFS state: the health check when due (git-lfs missing included),
    /// the locks when not read yet.
    pub(super) fn apply_lfs_state(&mut self, state: RepoLfs) {
        self.lfs.state = state;
        self.lfs_health_after_refresh(state.uses);
        if state.can_lock() && !self.lfs.locks_asked {
            self.lfs.locks_asked = true;
            let repo = self.repo.clone();
            self.loads_in_flight += 1;
            spawn(&self.tx, move || repo.lfs_locks(), Msg::LfsLocks);
        }
    }

    /// `Msg::LfsLocks` (a server without locking: none shown).
    pub(super) fn lfs_locks_read(&mut self, locks: Result<Vec<LfsLock>>) {
        if let Ok(locks) = locks {
            self.lfs.locks = locks;
        }
    }

    pub(crate) fn lfs_lock_of(&self, path: &str) -> Option<&LfsLock> {
        self.lfs.locks.iter().find(|l| l.path == path)
    }

    /// The LFS file at the cursor and whether it is locked — when locking is possible.
    fn lfs_lock_target(&self) -> Option<(String, bool)> {
        if !self.lfs.state.can_lock() {
            return None;
        }
        let (src, file) = self.rows.get(self.diff_cursor)?.file_ref()?;
        let diff = self.file_diff(src, file)?;
        diff.lfs.as_ref()?;
        let path = diff.path().to_string();
        let locked = self.lfs_lock_of(&path).is_some();
        Some((path, locked))
    }

    fn lfs_toggle_lock(&mut self, path: String, locked: bool) {
        // The refresh after the operation reads the locks again.
        self.lfs.locks_asked = false;
        let label = if locked { "Unlock" } else { "Lock" };
        self.run_op(label, true, After::Nothing, move |r| {
            if locked {
                r.lfs_unlock(&path, false)?;
                Ok(format!("Unlocked {path}"))
            } else {
                r.lfs_lock(&path)?;
                Ok(format!("Locked {path}"))
            }
        });
    }

    /// The diff menu's Lock / Unlock entry for the LFS file at the cursor.
    pub(super) fn lfs_menu_items(&self) -> Vec<MenuItem> {
        let Some((path, locked)) = self.lfs_lock_target() else {
            return Vec::new();
        };
        let label = if locked {
            "Unlock File (LFS)"
        } else {
            "Lock File (LFS)"
        };
        vec![MenuItem::new(label, move |app| {
            app.lfs_toggle_lock(path, locked)
        })]
    }

    /// The palette's Lock / Unlock entry for the LFS file at the cursor.
    pub(super) fn lfs_palette_items(&self) -> Vec<PickItem> {
        let Some((path, locked)) = self.lfs_lock_target() else {
            return Vec::new();
        };
        let label = if locked {
            "Unlock File (LFS)"
        } else {
            "Lock File (LFS)"
        };
        vec![PickItem::new(label, path.clone(), move |app: &mut App| {
            app.lfs_toggle_lock(path.clone(), locked)
        })]
    }

    /// "Track with Git LFS…" for `path` (empty: no file): git-lfs is checked first.
    pub(super) fn lfs_track(&mut self, path: String) {
        if let Some(why) = refusal(&path) {
            self.set_error(why);
            return;
        }
        let repo = self.repo.clone();
        self.loads_in_flight += 1;
        spawn(&self.tx, move || repo.lfs_setup(), move |s| Msg::LfsSetup(path, s));
    }

    /// `Msg::LfsSetup`: how to install git-lfs, or the menu of patterns.
    pub(super) fn lfs_setup_read(&mut self, path: String, setup: LfsSetup) {
        if setup.version.is_none() {
            self.modal = Some(Modal::text("Git LFS is not installed", NOT_INSTALLED));
            return;
        }
        let mut items = MenuItem::notes(EXPLANATION, NOTE_WIDTH);
        if !setup.filters {
            items.push(MenuItem::separator());
            items.extend(MenuItem::notes(
                &format!("{NOT_SET_UP} The next step offers git lfs install."),
                NOTE_WIDTH,
            ));
        }
        items.push(MenuItem::separator());
        items.extend(lfs_track_items(&path, "", setup.filters));
        let nested: Vec<String> = self
            .repo
            .rule_dirs(&path, ".gitattributes")
            .into_iter()
            .filter(|d| !d.is_empty())
            .collect();
        if !nested.is_empty() {
            items.push(MenuItem::separator());
        }
        for (dir, key) in nested.into_iter().zip('a'..='h') {
            let exists = self.repo.fs_path(&dir).join(".gitattributes").is_file();
            let label = format!(
                "Patterns for {dir}/.gitattributes…{}",
                if exists { "" } else { " (new file)" }
            );
            let p = path.clone();
            let filters = setup.filters;
            items.push(
                MenuItem::new(label, move |app: &mut App| {
                    let mut items = MenuItem::notes(
                        &format!("Patterns here are relative to {dir}/ and apply only inside it."),
                        NOTE_WIDTH,
                    );
                    items.push(MenuItem::separator());
                    items.extend(lfs_track_items(&p, &dir, filters));
                    app.modal = Some(Modal::menu(
                        format!("Track with Git LFS → {dir}/.gitattributes"),
                        items,
                    ));
                })
                .key(key),
            );
        }
        self.modal = Some(Modal::menu("Track with Git LFS", items));
    }

    /// The pattern line for the `.gitattributes` of folder `dir`, staging, `lockable` and —
    /// when git-lfs isn't set up — `git lfs install`.
    fn lfs_track_dialog(&mut self, pattern: &str, dir: String, filters: bool) {
        let label = if dir.is_empty() {
            "Pattern (relative to the top of the repository)".to_string()
        } else {
            format!("Pattern (relative to {dir}/ — applies only inside it)")
        };
        let mut checks = vec![
            ("Stage .gitattributes", true),
            ("Lockable: read-only until locked (files nobody can merge)", false),
        ];
        if !filters {
            checks.push(("Set up Git LFS for my user first (git lfs install)", true));
        }
        let file = if dir.is_empty() {
            ".gitattributes".to_string()
        } else {
            format!("{dir}/.gitattributes")
        };
        self.modal = Some(Modal::input_checks(
            format!("Track with Git LFS → {file}"),
            vec![(label.as_str(), pattern)],
            checks,
            move |app, v, c| {
                let pattern = v[0].clone();
                let opts = LfsTrackOptions {
                    stage: c[0],
                    lockable: c[1],
                    install: c.get(2).copied().unwrap_or(false),
                };
                app.run_op("Track with Git LFS", true, After::Nothing, move |r| {
                    let out = r.lfs_track(&dir, &pattern, opts)?;
                    match out.hook_warning {
                        Some(_) => anyhow::bail!(out.message()),
                        None => Ok(out.message()),
                    }
                });
            },
        ));
    }

    /// An LFS file has no hunk / line operations: says so, true when refused.
    pub(super) fn refuse_lfs(&mut self, src: Src, file: usize, mode: PatchMode) -> bool {
        let Some(diff) = self.file_diff(src, file) else {
            return false;
        };
        match gitti_core::lfs::check_partial(diff, mode) {
            Ok(()) => false,
            Err(e) => {
                self.set_error(e.to_string());
                true
            }
        }
    }
}

/// The suggestions for `path` in the `.gitattributes` of folder `dir` (keys 1–9) and
/// "Other pattern…".
fn lfs_track_items(path: &str, dir: &str, filters: bool) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = track_suggestions(path, dir)
        .into_iter()
        .zip('1'..='9')
        .map(|(c, key)| {
            let (pattern, dir) = (c.pattern.clone(), dir.to_string());
            MenuItem::new(format!("{}   {}", c.label, c.pattern), move |app: &mut App| {
                app.lfs_track_dialog(&pattern, dir, filters)
            })
            .key(key)
        })
        .collect();
    let dir = dir.to_string();
    items.push(
        MenuItem::new("Other pattern…", move |app: &mut App| {
            app.lfs_track_dialog("", dir, filters)
        })
        .key('o'),
    );
    items
}
