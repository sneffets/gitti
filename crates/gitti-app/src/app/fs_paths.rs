//! Paths this work tree's file system can't hold, in gitti (logic in
//! `gitti_core::fs_paths`): case collisions, Windows names, Unicode forms, … Such files
//! carry a "⚠ collides with xt_TCPMSS.h" mark on their file header and a few explanation
//! rows under it (`DiffRow::PathNote`: why, what follows, what to do; "Hide This Path" on
//! the first). A banner under the toolbar says "N paths can't be checked out on this file
//! system" with "Hide Them" (skip-worktree on those that show as changed) and ✕; the
//! palette has the same and "Show Hidden Paths Again" (the undo). Staging such a file asks
//! first (what is on disk isn't its own content), Discard says what it can't do, and lines
//! of such a file can't be staged or discarded.

use std::rc::Rc;

use gitti_core::fs_paths::{PathIssue, STAGE_THEM_TOO, discard_warning, partial_refusal, stage_question};
use gitti_core::patch::PatchMode;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use super::modal::ModalSpec;
use super::picker::PickerItem;
use crate::probe::ProbeExt as _;
use crate::rows::Src;
use crate::theme::{Palette, PaletteExt as _};

impl GittiApp {
    /// The problem of a working-directory file's path on this file system.
    pub(super) fn fs_issue_of(&self, src: Src, file: usize) -> Option<&PathIssue> {
        let path = self.work_files(src)?.get(file)?.path();
        self.working.fs_paths.issue(path)
    }

    /// The issues of `paths` (files about to be staged / discarded).
    fn fs_issues_among<'a>(&'a self, paths: impl IntoIterator<Item = &'a str>) -> Vec<&'a PathIssue> {
        paths
            .into_iter()
            .filter_map(|p| self.working.fs_paths.issue(p))
            .filter(|i| i.problem.cannot_hold())
            .collect()
    }

    /// Row `line` of a file's explanation (`DiffRow::PathNote`).
    pub(super) fn render_path_note_row(
        &self,
        base: Stateful<Div>,
        (src, file, line): (Src, usize, u16),
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(issue) = self.fs_issue_of(src, file) else {
            return base.into_any_element();
        };
        let lines = issue.problem.lines(&issue.path, &self.working.fs_paths.rules);
        let text = lines.get(line as usize).cloned().unwrap_or_default();
        let full = lines.join("\n");
        let hide = (line == 0 && src == Src::Unstaged && issue.problem.cannot_hold())
            .then(|| issue.path.clone());
        base.pl(px(48.))
            .pr_2()
            .gap_3()
            .text_size(self.settings.diff_small())
            .text_color(if line == 0 { p.warning } else { p.text_muted })
            .child(
                div()
                    .id(("path-note", ix))
                    .probe(format!("path-note:{ix}"))
                    .min_w_0()
                    .truncate()
                    .child(text)
                    .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx)),
            )
            .when_some(hide, |row, path| {
                row.child(
                    Button::new(("hide-path", ix))
                        .flex_none()
                        .probe(format!("hide-path:{ix}"))
                        .label("Hide This Path")
                        .xsmall()
                        .ghost()
                        .tooltip("git update-index --skip-worktree: git ignores this copy here until shown again")
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.hide_fs_paths(vec![path.clone()], w, cx)
                        })),
                )
            })
            .into_any_element()
    }

    /// The paths "Hide Them" hides now.
    fn fs_hidable(&self) -> Vec<String> {
        self.working.fs_paths.hidable(&self.working)
    }

    /// The banner's key: the paths it is about (✕ hides it until they change).
    fn fs_banner_key(&self) -> Vec<String> {
        self.working.fs_paths.shown().map(|i| i.path.clone()).collect()
    }

    /// "N paths can't be checked out on this file system …" with "Hide Them" and ✕.
    pub(super) fn render_fs_paths_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let note = self.working.fs_paths.note()?;
        if self.fs_banner_dismissed.as_ref() == Some(&self.fs_banner_key()) {
            return None;
        }
        let p = cx.palette().clone();
        let hidable = self.fs_hidable();
        Some(
            h_flex()
                .probe("fs-paths-banner")
                .px_3()
                .py_1()
                .gap_2()
                .items_center()
                .bg(p.warning.opacity(0.12))
                .border_b_1()
                .border_color(p.border)
                .text_color(p.warning)
                .child(div().flex_1().min_w_0().child(note))
                .when(!hidable.is_empty(), |row| {
                    let n = hidable.len();
                    row.child(
                        Button::new("fs-paths-hide")
                            .probe("fs-paths-hide")
                            .label("Hide Them")
                            .small()
                            .tooltip(format!(
                                "git update-index --skip-worktree on the {} that show as changed: \
                                 git ignores their copies here until shown again (palette: Show \
                                 Hidden Paths Again)",
                                gitti_core::discard::count(n, "path")
                            ))
                            .on_click(cx.listener(|this, _, w, cx| {
                                let paths = this.fs_hidable();
                                this.hide_fs_paths(paths, w, cx)
                            })),
                    )
                })
                .child(
                    Button::new("fs-paths-dismiss")
                        .probe("fs-paths-dismiss")
                        .label("✕")
                        .small()
                        .ghost()
                        .tooltip("Hide this note until these paths change")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.fs_banner_dismissed = Some(this.fs_banner_key());
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// Hides `paths` (skip-worktree), then a full refresh (they leave the file list).
    pub(super) fn hide_fs_paths(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.git_op("Hiding paths", window, cx, move |r| r.hide_fs_paths(&paths));
    }

    /// The undo of [`Self::hide_fs_paths`].
    pub(super) fn show_hidden_fs_paths(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git_op("Showing hidden paths", window, cx, |r| r.show_hidden_fs_paths());
    }

    /// Palette: hide / show again while there is something to hide / show.
    pub(super) fn fs_paths_palette_items(&self) -> Vec<PickerItem> {
        let report = &self.working.fs_paths;
        let mut items = Vec::new();
        let hidable = self.fs_hidable();
        if !hidable.is_empty() {
            items.push(PickerItem {
                label: "Hide Paths This File System Can't Hold".into(),
                detail: format!(
                    "{} showing as changed — skip-worktree",
                    gitti_core::discard::count(hidable.len(), "path")
                ),
                keys: String::new(),
                color: None,
                run: Rc::new(move |t, w, cx| t.hide_fs_paths(hidable.clone(), w, cx)),
                action: None,
            });
        }
        let hidden = report.hidden().count();
        if hidden > 0 {
            items.push(PickerItem {
                label: "Show Hidden Paths Again".into(),
                detail: format!(
                    "{} this file system can't hold — no-skip-worktree",
                    gitti_core::discard::count(hidden, "path")
                ),
                keys: String::new(),
                color: None,
                run: Rc::new(|t, w, cx| t.show_hidden_fs_paths(w, cx)),
                action: None,
            });
        }
        items
    }

    /// Lines of a file this file system can't hold are not staged / discarded (its lines on
    /// disk aren't its own): says why (`true`).
    pub(super) fn refuse_fs_path(&mut self, src: Src, file: usize, mode: PatchMode, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.fs_issue_of(src, file).and_then(|i| partial_refusal(i, mode)) else {
            return false;
        };
        self.set_error(text);
        cx.notify();
        true
    }

    /// Staging `paths`: when some can't be held on this file system, asks first (one file:
    /// "Stage Anyway"; several: the others, or with a box ticked all of them) and runs
    /// `stage` with the paths to stage; otherwise runs it right away (`None`: all of them).
    pub(super) fn guard_fs_stage(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        stage: impl FnOnce(&mut Self, Option<Vec<String>>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let issues = self.fs_issues_among(paths.iter().map(String::as_str));
        if issues.is_empty() {
            return stage(self, None, window, cx);
        }
        let (title, text) = stage_question(&issues);
        let several = issues.len() > 1;
        let skip: Vec<String> = issues.iter().map(|i| i.path.clone()).collect();
        let mut spec = ModalSpec::new(title).message(text);
        spec = if several {
            spec.check(STAGE_THEM_TOO, false).ok("Stage")
        } else {
            spec.ok("Stage Anyway")
        };
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            if !several || res.checks.first().copied().unwrap_or(false) {
                return stage(this, None, w, cx);
            }
            let rest: Vec<String> = paths.into_iter().filter(|p| !skip.contains(p)).collect();
            if rest.is_empty() {
                this.set_info("Nothing staged");
                cx.notify();
                return;
            }
            stage(this, Some(rest), w, cx)
        });
    }

    /// What Discard of `paths` can't do on this file system, for its question.
    pub(super) fn fs_discard_warning<'a>(&'a self, paths: impl IntoIterator<Item = &'a str>) -> Option<String> {
        discard_warning(&self.fs_issues_among(paths))
    }

    /// A notice with just OK (e.g. a clone's paths that collided, in the new tab).
    pub fn show_notice(&mut self, title: &str, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let spec = ModalSpec::new(title).message(text).ok("OK").cancel(None);
        self.open_modal(spec, window, cx, |_, _, _, _| {});
    }

    /// The banner's text (tests; `None`: not shown).
    pub fn script_fs_paths_banner(&self) -> Option<String> {
        let note = self.working.fs_paths.note()?;
        (self.fs_banner_dismissed.as_ref() != Some(&self.fs_banner_key())).then_some(note)
    }

    /// `fspaths:ACTION` — `hide` (what "Hide Them" hides), `show` (the undo), `dismiss`.
    pub fn script_fs_paths(&mut self, action: &str, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            "hide" => {
                let paths = self.fs_hidable();
                self.hide_fs_paths(paths, window, cx)
            }
            "show" => self.show_hidden_fs_paths(window, cx),
            "dismiss" => {
                self.fs_banner_dismissed = Some(self.fs_banner_key());
                cx.notify();
            }
            other => eprintln!("[script] unknown fspaths action {other}"),
        }
    }
}
