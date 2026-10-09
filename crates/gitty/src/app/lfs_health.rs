//! Git LFS health in gitty (as gitti's `app/lfs_health.rs`; logic in
//! `gitti_core::lfs_health`). LFS stays invisible until something is wrong — git-lfs
//! missing, not set up for the user, the repository's hooks missing or someone else's,
//! files still pointers — then a badge in the toolbar names it (`3 LFS FILES NOT
//! DOWNLOADED — ctrl-p: Git LFS`); the palette's "Git LFS Problems…" explains each problem
//! in plain words with what follows, offers the fixes and hides the badge until the
//! problems change. A push that wouldn't upload the LFS content asks first, offering to
//! write the hooks.
//!
//! Checked in the background (`Msg::LfsHealth`) once at start, when HEAD moves and after a
//! fix — never per refresh or on navigation. Before a push only the hook files are read
//! again.

use std::cell::RefCell;
use std::rc::Rc;

use gitti_core::ObjectId;
use gitti_core::lfs_health::{LfsFix, LfsHealth, LfsIssue, NO_VERIFY_LABEL, PUSH_WARNING_TITLE};
use gitti_core::Repo;

use super::{After, App, MenuItem, Modal, Msg, PickItem, spawn};

/// Width of the explanation lines in the LFS menus.
const NOTE_WIDTH: usize = 64;

/// gitty's LFS health as last checked.
#[derive(Debug, Default)]
pub struct LfsHealthUi {
    health: Option<LfsHealth>,
    issues: Vec<LfsIssue>,
    /// HEAD at the last check started (`None`: check on the next refresh).
    checked_head: Option<Option<ObjectId>>,
    in_flight: bool,
    /// The badge was hidden for these problems.
    dismissed: Option<Vec<String>>,
}

impl LfsHealthUi {
    fn key(&self) -> Vec<String> {
        self.issues.iter().map(|i| i.short.clone()).collect()
    }
}

/// A push operation waiting for the answer to the LFS warning.
type PendingPush = Rc<RefCell<Option<Box<dyn FnOnce(&mut App)>>>>;

impl App {
    /// A refresh's LFS state: the health is checked when the repository uses LFS and HEAD
    /// moved since the last check (the first refresh included).
    pub(super) fn lfs_health_after_refresh(&mut self, uses: bool) {
        if !uses {
            if self.lfs.health.health.is_some() {
                self.lfs.health = LfsHealthUi::default();
            }
            return;
        }
        let head = self.head.id;
        if self.lfs.health.in_flight || self.lfs.health.checked_head == Some(head) {
            return;
        }
        self.start_lfs_health_check();
    }

    fn start_lfs_health_check(&mut self) {
        self.lfs.health.in_flight = true;
        self.lfs.health.checked_head = Some(self.head.id);
        let repo = self.repo.clone();
        self.loads_in_flight += 1;
        spawn(&self.tx, move || repo.lfs_health(), |h| Msg::LfsHealth(Box::new(h)));
    }

    /// `Msg::LfsHealth`.
    pub(super) fn lfs_health_read(&mut self, health: LfsHealth) {
        let ui = &mut self.lfs.health;
        ui.in_flight = false;
        ui.issues = health.issues(self.repo.workdir());
        ui.health = Some(health);
    }

    /// The toolbar badge: the first problem's few words (`None`: nothing wrong, or hidden).
    pub(crate) fn lfs_badge(&self) -> Option<String> {
        let ui = &self.lfs.health;
        if ui.dismissed.as_ref() == Some(&ui.key()) {
            return None;
        }
        let first = ui.issues.first()?;
        let more = ui.issues.len() - 1;
        Some(if more > 0 {
            format!("{} (+{more})", first.short)
        } else {
            first.short.clone()
        })
    }

    /// `fix` as an operation; the health is checked again after it.
    fn run_lfs_fix(&mut self, fix: LfsFix) {
        self.lfs.health.checked_head = None;
        self.run_op(fix.label(), true, After::Nothing, move |r| r.lfs_fix(fix));
    }

    /// "Git LFS Problems…": each problem explained, its fix, hiding the badge.
    pub(crate) fn lfs_health_menu(&mut self) {
        let mut items = Vec::new();
        let mut keys = '1'..='9';
        for issue in &self.lfs.health.issues {
            if !items.is_empty() {
                items.push(MenuItem::separator());
            }
            items.extend(MenuItem::notes(&issue.text, NOTE_WIDTH));
            if let Some(fix) = issue.fix {
                let mut item = MenuItem::new(format!("{} ({})", fix.label(), fix.detail()), move |app: &mut App| {
                    app.run_lfs_fix(fix)
                });
                if let Some(k) = keys.next() {
                    item = item.key(k);
                }
                items.push(item);
            }
        }
        if items.is_empty() {
            items.extend(MenuItem::notes("Git LFS: nothing wrong found.", NOTE_WIDTH));
        }
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("Hide until the problems change", |app: &mut App| {
                app.lfs.health.dismissed = Some(app.lfs.health.key());
            })
            .key('h'),
        );
        items.push(
            MenuItem::new("Check again", |app: &mut App| {
                app.lfs.health.dismissed = None;
                app.start_lfs_health_check();
            })
            .key('c'),
        );
        self.modal = Some(Modal::menu("Git LFS", items));
    }

    /// Palette: "Git LFS Problems…" while there are any, the fixes, "Check Git LFS Setup"
    /// in an LFS repository.
    pub(super) fn lfs_health_palette_items(&self) -> Vec<PickItem> {
        if !self.lfs.state.uses {
            return Vec::new();
        }
        let ui = &self.lfs.health;
        let mut items = Vec::new();
        if !ui.issues.is_empty() {
            let detail = ui.key().join(" · ");
            items.push(PickItem::new("Git LFS Problems…", detail, |app: &mut App| {
                app.lfs_health_menu()
            }));
        }
        for issue in &ui.issues {
            if let Some(fix) = issue.fix {
                items.push(PickItem::new(fix.label(), issue.short.clone(), move |app: &mut App| {
                    app.run_lfs_fix(fix)
                }));
            }
        }
        items.push(PickItem::new(
            "Check Git LFS Setup",
            "git-lfs, its filters, the hooks, files not downloaded",
            |app: &mut App| {
                app.lfs.health.dismissed = None;
                app.start_lfs_health_check();
            },
        ));
        items
    }

    /// The push dialog's "Skip hooks" label (it skips the LFS upload too).
    pub(super) fn no_verify_label(&self) -> &'static str {
        if self.lfs.state.uses {
            NO_VERIFY_LABEL
        } else {
            "Skip hooks (--no-verify)"
        }
    }

    /// `run_op` of a push — after asking when it wouldn't upload the LFS content (the hook
    /// files read again; `no_verify`: hooks skipped). The menu offers writing the hooks.
    pub(crate) fn run_push_op(
        &mut self,
        label: &str,
        after: After,
        no_verify: bool,
        op: impl FnOnce(&Repo) -> anyhow::Result<String> + Send + 'static,
    ) {
        let warning = self.lfs.health.health.as_ref().and_then(|h| {
            let hooks = h.hooks.as_ref().map(|k| k.reread());
            let w = h.push_warning(self.repo.workdir(), hooks.as_ref(), no_verify)?;
            Some((w, hooks))
        });
        let label = label.to_string();
        let Some((warning, hooks)) = warning else {
            return self.run_op(&label, true, after, op);
        };
        let pending: PendingPush = Rc::new(RefCell::new(Some(Box::new(move |app: &mut App| {
            app.run_op(&label, true, after, op)
        }))));
        let mut items = MenuItem::notes(&warning.text, NOTE_WIDTH);
        items.push(MenuItem::separator());
        if let (Some(_), Some(hooks)) = (warning.fix, hooks) {
            let go = pending.clone();
            items.push(
                MenuItem::new("Install the Git LFS hooks, then push (recommended)", move |app: &mut App| {
                    if let Err(e) = hooks.install() {
                        app.set_error(format!("Installing the Git LFS hooks failed: {e:#}"));
                        return;
                    }
                    app.lfs.health.checked_head = None;
                    if let Some(push) = go.borrow_mut().take() {
                        push(app)
                    }
                })
                .key('i'),
            );
        }
        items.push(
            MenuItem::new("Push anyway (without the LFS content)", move |app: &mut App| {
                if let Some(push) = pending.borrow_mut().take() {
                    push(app)
                }
            })
            .key('p'),
        );
        self.modal = Some(Modal::menu(PUSH_WARNING_TITLE, items));
    }
}
