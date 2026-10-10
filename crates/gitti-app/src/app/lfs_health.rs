//! Git LFS health in gitti (logic in `gitti_core::lfs_health`). LFS stays invisible
//! until something is wrong — git-lfs missing, not set up for the user, the repository's
//! hooks missing or someone else's, files still pointers — then a banner under the toolbar
//! says so in plain words, with what follows and the button that fixes it (✕ hides it
//! until the problems change). The palette offers the same fixes and "Check Git LFS Setup".
//! A push that wouldn't upload the LFS content (no LFS pre-push hook, `--no-verify`) asks
//! first, offering to write the hooks.
//!
//! A repository that doesn't use LFS (any more) but has git-lfs's hooks gets the same
//! banner: what the hooks cost on every push, "Remove LFS Hooks" (`gitti_core::lfs_unused`).
//!
//! Checked in the background once per tab, when HEAD moves (checkout, pull, …), when the
//! repository starts or stops using LFS (the last pattern removed) and after a fix
//! (`Repo::lfs_check`: in an LFS repository one `git config`, a `git check-attr` only when
//! pointer files are found; otherwise in-process) — never per refresh or on navigation.
//! Before a push only the hook files are read again.

use std::rc::Rc;
use std::sync::Arc;

use gitti_core::ObjectId;
use gitti_core::lfs_health::{
    LfsFix, LfsHealth, LfsIssue, LfsIssueKind, NO_VERIFY_LABEL, PUSH_WARNING_TITLE,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use super::GittiApp;
use super::modal::ModalSpec;
use super::picker::PickerItem;
use crate::probe::ProbeExt as _;
use crate::theme::PaletteExt as _;

/// A tab's LFS health as last checked.
#[derive(Debug, Default)]
pub(super) struct LfsHealthUi {
    health: Option<Arc<LfsHealth>>,
    issues: Vec<LfsIssue>,
    /// HEAD and whether the repository used LFS at the last check started (`None`: check
    /// on the next refresh).
    checked: Option<(Option<ObjectId>, bool)>,
    in_flight: bool,
    /// The banner was hidden for these problems.
    dismissed: Option<Vec<String>>,
}

impl LfsHealthUi {
    fn key(&self) -> Vec<String> {
        self.issues.iter().map(|i| i.short.clone()).collect()
    }

    fn banner_hidden(&self) -> bool {
        self.issues.is_empty() || self.dismissed.as_ref() == Some(&self.key())
    }
}

impl GittiApp {
    /// A refresh's LFS state: the health is checked when HEAD moved or the repository
    /// started / stopped using LFS since the last check (the first refresh included).
    pub(super) fn lfs_health_after_refresh(&mut self, uses: bool, cx: &mut Context<Self>) {
        let now = (self.head.id, uses);
        if self.lfs.health.in_flight || self.lfs.health.checked == Some(now) {
            return;
        }
        self.start_lfs_health_check(cx);
    }

    fn start_lfs_health_check(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let uses = self.lfs.state.uses;
        self.lfs.health.in_flight = true;
        self.lfs.health.checked = Some((self.head.id, uses));
        let task = cx
            .background_executor()
            .spawn(async move { repo.lfs_check(uses) });
        cx.spawn(async move |this, cx| {
            let health = task.await;
            this.update(cx, |this, cx| {
                this.set_lfs_health(health);
                // HEAD moved or LFS came / went while it ran: check again.
                if this.lfs.health.checked.is_some_and(|c| c != (this.head.id, this.lfs.state.uses)) {
                    this.start_lfs_health_check(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A check's result (tests: a made-up one).
    pub(crate) fn set_lfs_health(&mut self, health: LfsHealth) {
        let workdir = self.repo.as_ref().map(|r| r.workdir().to_path_buf());
        let ui = &mut self.lfs.health;
        ui.in_flight = false;
        ui.issues = workdir.map(|w| health.issues(&w)).unwrap_or_default();
        ui.health = Some(Arc::new(health));
    }

    /// `fix` as an operation; the health is checked again after it.
    pub(super) fn run_lfs_fix(&mut self, fix: LfsFix, window: &mut Window, cx: &mut Context<Self>) {
        self.lfs.health.checked = None;
        self.run_op(
            fix.label(),
            true,
            window,
            cx,
            move |r| r.lfs_fix(fix),
            |this, msg, _, _| {
                this.lfs.health.checked = None;
                this.set_info(msg);
            },
        );
    }

    /// The banner: one row per problem with its fix, ✕ on the first.
    pub(super) fn render_lfs_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ui = &self.lfs.health;
        if ui.banner_hidden() {
            return None;
        }
        let p = cx.palette().clone();
        let rows = ui.issues.iter().enumerate().map(|(i, issue)| {
            let kind = match issue.kind {
                LfsIssueKind::NotInstalled => "not-installed",
                LfsIssueKind::NotSetUp => "not-set-up",
                LfsIssueKind::Hooks => "hooks",
                LfsIssueKind::NotDownloaded => "not-downloaded",
                LfsIssueKind::UnusedHooks => "unused-hooks",
            };
            let mut row = h_flex().gap_2().items_center().child(
                div()
                    .id(("lfs-issue", i))
                    .probe(format!("lfs-issue:{kind}"))
                    .flex_1()
                    .min_w_0()
                    .child(issue.text.clone()),
            );
            if let Some(fix) = issue.fix {
                row = row.child(
                    Button::new(("lfs-fix", i))
                        .probe(format!("lfs-fix:{}", fix.key()))
                        .label(fix.label())
                        .small()
                        .tooltip(fix.detail())
                        .on_click(cx.listener(move |this, _, w, cx| this.run_lfs_fix(fix, w, cx))),
                );
            }
            if i == 0 {
                row = row.child(
                    Button::new("lfs-dismiss")
                        .probe("lfs-dismiss")
                        .label("✕")
                        .small()
                        .ghost()
                        .tooltip("Hide until the Git LFS problems change")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.lfs.health.dismissed = Some(this.lfs.health.key());
                            cx.notify();
                        })),
                );
            }
            row
        });
        Some(
            v_flex()
                .probe("lfs-banner")
                .px_3()
                .py_1()
                .gap_1()
                .bg(p.warning.opacity(0.12))
                .border_b_1()
                .border_color(p.border)
                .text_color(p.warning)
                .children(rows)
                .into_any_element(),
        )
    }

    /// Runs `push` — after asking when it wouldn't upload the LFS content (the hooks read
    /// again first; `no_verify`: hooks skipped). The dialog offers writing the hooks.
    pub(super) fn lfs_guarded_push(
        &mut self,
        no_verify: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        push: impl FnOnce(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
    ) {
        let (Some(health), Some(repo)) = (self.lfs.health.health.clone(), self.repo.clone()) else {
            return push(self, window, cx);
        };
        let hooks = health.hooks.as_ref().map(|h| h.reread());
        let Some(warning) = health.push_warning(repo.workdir(), hooks.as_ref(), no_verify) else {
            return push(self, window, cx);
        };
        let fixable = warning.fix.is_some();
        let mut spec = ModalSpec::new(PUSH_WARNING_TITLE).message(warning.text);
        if fixable {
            spec = spec.check("Install the Git LFS hooks first (recommended)", true);
        }
        let spec = spec.ok("Push");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            if fixable
                && res.checks.first().copied().unwrap_or(false)
                && let Some(hooks) = &hooks
            {
                if let Err(e) = hooks.install() {
                    this.set_error(format!("Installing the Git LFS hooks failed: {e:#}"));
                    cx.notify();
                    return;
                }
                this.lfs.health.checked = None;
            }
            push(this, w, cx)
        });
    }

    /// `git_op` of a push, through [`Self::lfs_guarded_push`].
    pub(super) fn git_push_op(
        &mut self,
        label: &str,
        no_verify: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&gitti_core::Repo) -> anyhow::Result<String> + Send + 'static,
    ) {
        let label = label.to_string();
        self.lfs_guarded_push(no_verify, window, cx, move |this, w, cx| {
            this.git_op(&label, w, cx, op)
        });
    }

    /// The push dialog's "Skip hooks" label (it skips the LFS upload too).
    pub(super) fn no_verify_label(&self) -> &'static str {
        if self.lfs.state.uses {
            NO_VERIFY_LABEL
        } else {
            "Skip hooks (--no-verify)"
        }
    }

    /// Palette: the fixes of the current problems ("Remove LFS Hooks" also outside an
    /// LFS repository), "Check Git LFS Setup" in an LFS repository.
    pub(super) fn lfs_health_palette_items(&self) -> Vec<PickerItem> {
        let mut items: Vec<PickerItem> = self
            .lfs
            .health
            .issues
            .iter()
            .filter_map(|i| {
                let fix = i.fix?;
                Some(PickerItem {
                    label: fix.label().into(),
                    detail: i.short.clone(),
                    keys: String::new(),
                    color: None,
                    run: Rc::new(move |t, w, cx| t.run_lfs_fix(fix, w, cx)),
                    action: None,
                })
            })
            .collect();
        if !self.lfs.state.uses {
            return items;
        }
        items.push(PickerItem {
            label: "Check Git LFS Setup".into(),
            detail: "git-lfs, its filters, the hooks, files not downloaded".into(),
            keys: String::new(),
            color: None,
            run: Rc::new(|t, _, cx| {
                t.lfs.health.dismissed = None;
                t.start_lfs_health_check(cx);
                cx.notify();
            }),
            action: None,
        });
        items
    }

    /// `lfs:ACTION[,ARG]` — `fix,KEY` (setup / hooks / download / remove-hooks), `dismiss`,
    /// `check`.
    pub fn script_lfs(&mut self, action: &str, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            "fix" => match LfsFix::from_key(arg) {
                Some(fix) => self.run_lfs_fix(fix, window, cx),
                None => eprintln!("[script] unknown LFS fix {arg}"),
            },
            "dismiss" => {
                self.lfs.health.dismissed = Some(self.lfs.health.key());
                cx.notify();
            }
            "check" => {
                self.lfs.health.dismissed = None;
                self.start_lfs_health_check(cx);
            }
            other => eprintln!("[script] unknown lfs action {other}"),
        }
    }

    /// The problems shown (tests): `short` per issue, empty when the banner is hidden.
    pub fn script_lfs_issues(&self) -> Vec<String> {
        if self.lfs.health.banner_hidden() {
            return Vec::new();
        }
        self.lfs.health.key()
    }
}
