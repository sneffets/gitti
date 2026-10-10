//! Paths this work tree's file system can't hold, in gitti (logic in
//! `gitti_core::fs_paths`): case collisions, Windows names, Unicode forms, … Such files
//! carry a "⚠ collides with xt_TCPMSS.h" mark on their file header (with "Hide Path"), on
//! their file tab and in the file list, and a few explanation rows under the header
//! (`DiffRow::PathNote`: why, what follows, what to do), wrapped at the pane's width
//! ([`NoteWrapper`]). A banner under the toolbar says "N paths can't be checked out on
//! this file system" with "Hide Them" (skip-worktree on those that show as changed) and ✕;
//! Repository ▸ "Paths This File System Can't Hold…" / "Hide Paths …" / "Show Hidden Paths
//! Again" (the undo) and the palette have the same. Staging such a file asks first (what is
//! on disk isn't its own content), Discard says what it can't do, and lines of such a file
//! can't be staged or discarded.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gitti_core::fs_paths::{PathIssue, STAGE_THEM_TOO, discard_warning, partial_refusal, stage_question};
use gitti_core::patch::PatchMode;
use gitti_core::rows::NoteWrap;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use super::modal::ModalSpec;
use super::picker::{self, PickerItem};
use crate::probe::ProbeExt as _;
use crate::rows::Src;
use crate::theme::{Palette, PaletteExt as _};
use crate::{FsPaths, HideFsPaths, ShowHiddenFsPaths};

/// Left padding of an explanation row (under the file name).
const NOTE_LEFT: Pixels = px(48.);
/// What an explanation row's text can't use of the diff list's width: its left and right
/// padding and the scrollbar.
const NOTE_INSET: Pixels = px(48. + 8. + 16.);

/// The width an explanation row's text has in a diff list `list_w` wide.
pub(super) fn note_width(list_w: Pixels) -> Pixels {
    (list_w - NOTE_INSET).max(px(1.))
}

/// Wrapped explanation lines (byte ranges of their rows) for one (width, font size).
type WrapCache = ((Pixels, Pixels), HashMap<String, Vec<Range<usize>>>);

/// How the explanation rows wrap: the text system's line wrapper with the rows' font at
/// the diff list's width (measured each frame, `GittiApp::render_diff_list`) — always,
/// whatever "Line Wrap in Diffs" says: cut prose hides what to do. Wrapped lines are
/// memoised per width and size.
pub(super) struct NoteWrapper {
    text_system: Arc<TextSystem>,
    family: SharedString,
    /// The list's width for the text (`px(0.)` before the first frame: no wrapping yet)
    /// and the font size the rows were last wrapped for (`set_note_width`).
    pub(super) key: Rc<Cell<(Pixels, Pixels)>>,
    cache: RefCell<WrapCache>,
}

impl NoteWrapper {
    pub(super) fn new(cx: &App) -> Self {
        Self {
            text_system: cx.text_system().clone(),
            family: cx.theme().font_family.clone(),
            key: Rc::new(Cell::new((px(0.), px(0.)))),
            cache: Default::default(),
        }
    }

    /// The byte ranges of `text`'s rows at font size `size`.
    fn parts(&self, text: &str, size: Pixels) -> Vec<Range<usize>> {
        let width = self.key.get().0;
        if width <= px(0.) {
            return std::iter::once(0..text.len()).collect();
        }
        let mut cache = self.cache.borrow_mut();
        if cache.0 != (width, size) {
            *cache = ((width, size), HashMap::new());
        }
        if let Some(parts) = cache.1.get(text) {
            return parts.clone();
        }
        let mut wrapper = self.text_system.line_wrapper(font(self.family.clone()), size);
        let mut start = 0;
        let mut parts = Vec::new();
        for b in wrapper.wrap_line(&[LineFragment::text(text)], width) {
            parts.push(start..b.ix);
            start = b.ix;
        }
        parts.push(start..text.len());
        cache.1.insert(text.to_string(), parts.clone());
        parts
    }

    /// The rows' font family (the text is drawn in exactly the one measured).
    fn family(&self) -> SharedString {
        self.family.clone()
    }
}

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

    /// How the explanation rows wrap now (`NoteWrap::None` before the list's width is
    /// known); `f` is the measuring function [`NoteWrap::By`] borrows.
    pub(super) fn with_note_wrap<R>(&self, f: impl FnOnce(&NoteWrap) -> R) -> R {
        let size = self.settings.diff_small();
        let by = |t: &str| self.note_wrap.parts(t, size);
        if self.note_wrap.key.get().0 <= px(0.) {
            f(&NoteWrap::None)
        } else {
            f(&NoteWrap::By(&by))
        }
    }

    /// The diff list's width or the diff font changed (its paint pass): the explanation
    /// rows wrap anew when there are any.
    pub(super) fn set_note_width(&mut self, list_w: Pixels, cx: &mut Context<Self>) {
        let key = (note_width(list_w), self.settings.diff_small());
        if self.note_wrap.key.get() == key {
            return;
        }
        self.note_wrap.key.set(key);
        if self.rows.iter().any(|r| matches!(r, crate::rows::DiffRow::PathNote { .. })) {
            self.rebuild_rows_keep_top(cx);
            cx.notify();
        }
    }

    /// Row `part` of line `line` of a file's explanation (`DiffRow::PathNote`): the text
    /// (probe `path-note:N`, as wide as its text — it never reaches past the row), the
    /// whole explanation as its tooltip.
    pub(super) fn render_path_note_row(
        &self,
        base: Stateful<Div>,
        (src, file, line, part): (Src, usize, u16, u16),
        ix: usize,
        p: &Palette,
    ) -> AnyElement {
        let Some(issue) = self.fs_issue_of(src, file) else {
            return base.into_any_element();
        };
        let lines = issue.problem.lines(&issue.path, &self.working.fs_paths.rules);
        let whole = lines.get(line as usize).cloned().unwrap_or_default();
        let text = self.with_note_wrap(|w| w.part(&whole, part).trim_end().to_string());
        let full = lines.join("\n\n");
        base.pl(NOTE_LEFT)
            .pr_2()
            .overflow_hidden()
            .text_size(self.settings.diff_small())
            .text_color(if line == 0 { p.warning } else { p.text_muted })
            .child(
                div()
                    .id(("path-note", ix))
                    .probe(format!("path-note:{ix}"))
                    .flex_none()
                    .whitespace_nowrap()
                    .font_family(self.note_wrap.family())
                    .child(text)
                    .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx)),
            )
            .into_any_element()
    }

    /// "Hide Path" on the header of a working-directory file this file system can't hold
    /// (probe `hide-path:N`); `None` for others.
    pub(super) fn render_hide_path_button(
        &self,
        src: Src,
        file: usize,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let issue = self.fs_issue_of(src, file)?;
        if src != Src::Unstaged || !issue.problem.cannot_hold() {
            return None;
        }
        let path = issue.path.clone();
        Some(
            Button::new(("hide-path", ix))
                .flex_none()
                .probe(format!("hide-path:{ix}"))
                .label("Hide Path")
                .xsmall()
                .ghost()
                .tooltip(HIDE_PATH_TIP)
                .on_click(cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    this.hide_fs_paths(vec![path.clone()], w, cx)
                }))
                .into_any_element(),
        )
    }

    /// The paths "Hide Them" hides now.
    pub(super) fn fs_hidable(&self) -> Vec<String> {
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

    /// Palette: the note, hide, show again — always listed (greyed with the reason when
    /// there is nothing to do, `unavailable.rs`), like their Repository menu entries.
    pub(super) fn fs_paths_palette_items(&self) -> Vec<PickerItem> {
        let report = &self.working.fs_paths;
        let count = |n| gitti_core::discard::count(n, "path");
        let shown = report.shown().count();
        let hidable = self.fs_hidable().len();
        let hidden = report.hidden().count();
        vec![
            picker::item(
                "Paths This File System Can't Hold…",
                format!("{} in the way, {hidden} hidden", count(shown)),
                |t, w, cx| t.show_fs_paths_note(w, cx),
            )
            .act(&FsPaths),
            picker::item(
                "Hide Paths This File System Can't Hold",
                format!("{} showing as changed — skip-worktree", count(hidable)),
                |t, w, cx| {
                    let paths = t.fs_hidable();
                    t.hide_fs_paths(paths, w, cx)
                },
            )
            .act(&HideFsPaths),
            picker::item(
                "Show Hidden Paths Again",
                format!("{} this file system can't hold — no-skip-worktree", count(hidden)),
                |t, w, cx| t.show_hidden_fs_paths(w, cx),
            )
            .act(&ShowHiddenFsPaths),
        ]
    }

    /// The handlers of Repository ▸ "Paths This File System Can't Hold…", "Hide Paths …",
    /// "Show Hidden Paths Again" on the app's root (greyed, and refused with the reason,
    /// while there is nothing to do: `unavailable.rs`).
    pub(super) fn fs_paths_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &FsPaths, w, cx| this.show_fs_paths_note(w, cx)))
            .on_action(cx.listener(|this, _: &HideFsPaths, w, cx| {
                let paths = this.fs_hidable();
                this.hide_fs_paths(paths, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowHiddenFsPaths, w, cx| {
                this.show_hidden_fs_paths(w, cx)
            }))
    }

    /// "Paths This File System Can't Hold…": the repository's note and the paths in the
    /// way (also after the banner was closed), with "Hide Them" — or, with all of them
    /// hidden, "Show Them Again".
    pub(super) fn show_fs_paths_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let report = self.working.fs_paths.clone();
        let hidable = self.fs_hidable();
        let hidden = report.hidden().count();
        let mut text = report.note().unwrap_or_else(|| "Every path fits this file system.".into());
        let shown: Vec<String> = report
            .shown()
            .map(|i| format!("{} — {}", i.path, i.problem.short()))
            .collect();
        if !shown.is_empty() {
            text.push_str("\n\n");
            text.push_str(&list_of(&shown, 12));
        }
        if hidden > 0 {
            text.push_str(&format!(
                "\n\n{} hidden (skip-worktree): git ignores their copies here until shown again.",
                gitti_core::discard::count(hidden, "path")
            ));
        }
        let title = "Paths This File System Can't Hold";
        if !hidable.is_empty() {
            let spec = ModalSpec::new(title).message(text).ok("Hide Them").cancel(Some("Close"));
            self.open_modal(spec, window, cx, move |this, _, w, cx| {
                this.hide_fs_paths(hidable.clone(), w, cx)
            });
        } else if hidden > 0 {
            let spec = ModalSpec::new(title).message(text).ok("Show Them Again").cancel(Some("Close"));
            self.open_modal(spec, window, cx, |this, _, w, cx| this.show_hidden_fs_paths(w, cx));
        } else {
            self.show_notice(title, text, window, cx);
        }
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

/// The tooltip of "Hide Path".
const HIDE_PATH_TIP: &str = "Hide This Path — git update-index --skip-worktree: git ignores \
     this copy here until shown again (Repository ▸ Show Hidden Paths Again)";

/// `items` one per line, at most `max` ("… and 3 more").
fn list_of(items: &[String], max: usize) -> String {
    let mut s = items.iter().take(max).cloned().collect::<Vec<_>>().join("\n");
    if items.len() > max {
        s.push_str(&format!("\n… and {} more", items.len() - max));
    }
    s
}
