//! Several commits at once: marked commits of the commit list
//! (`gitti_core::commit_marks`) and Cherry-pick / Revert Marked Commits…
//! (`gitti_core::pick_several`), one git sequence whose stops the operation banner's
//! Continue / Skip / Abort handle.
//!
//! Marking (as marked files, `file_marks.rs`): ⌘-click (ctrl elsewhere) on a commit marks /
//! unmarks it — the first one marks the selected row too, so "click one, ⌘-click two more"
//! gives three —, shift-click marks the rows from the last one marked; marks stay while
//! the selection moves (a lighter tint than the selected row) until Esc in the list or
//! Clear Marks. The commit's context menu has Mark / Unmark for Cherry-pick / Revert and
//! Clear Marks; on a marked commit it starts with the two commands for them. Menu bar (Commit) and palette:
//! Cherry-pick Marked Commits… / Revert Marked Commits…, greyed with the reason. The
//! dialog says the order (oldest first / newest first), lists the commits and says what to
//! do on a stop. gitty: `crates/gitty/src/app/pick_several.rs` (space marks).

use std::rc::Rc;

use gitti_core::commit_marks::{self, CLEAR_LABEL, MARK_LABEL, UNMARK_LABEL};
use gitti_core::pick_several::{self, RECORD_ORIGIN_LABEL, SeveralKind};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::GittiApp;
use super::menus::{Entry, menu_action};
use super::modal::ModalSpec;
use super::picker::PickerItem;
use crate::{CherryPickMarked, RevertMarked};

type AppHandle = WeakEntity<GittiApp>;

fn item(
    label: impl Into<SharedString>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> Entry {
    let app = app.clone();
    menu_action(label, move |window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

/// What clears the marks, as the status line says it.
const HOW_TO_CLEAR: &str = "Esc in the commit list clears them";

impl GittiApp {
    /// Registers the handlers of this module's menu-bar actions.
    pub(super) fn pick_several_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &CherryPickMarked, w, cx| {
            this.several_dialog(SeveralKind::CherryPick, w, cx)
        }))
        .on_action(cx.listener(|this, _: &RevertMarked, w, cx| this.several_dialog(SeveralKind::Revert, w, cx)))
    }

    /// Whether commit row `ix` is marked.
    pub(super) fn is_commit_marked(&self, ix: usize) -> bool {
        !self.commit_marks.is_empty()
            && self.history.commits.get(ix).is_some_and(|c| self.commit_marks.contains(&c.id))
    }

    /// A left click on commit row `ix`: with ⌘ / ctrl it marks or unmarks the row (the
    /// first mark takes the selected row along), with shift it marks the range from the
    /// last one marked; a plain click leaves the marks. The row is selected in every case.
    pub(super) fn click_commit_row(&mut self, ix: usize, m: Modifiers, window: &mut Window, cx: &mut Context<Self>) {
        let history = self.history.clone();
        if m.secondary() {
            if self.commit_marks.is_empty() && self.selected != ix {
                self.commit_marks.insert(&history, self.selected);
            }
            self.commit_marks.toggle(&history, ix);
            self.say_commit_marks();
        } else if m.shift {
            self.commit_marks.extend_to(&history, ix, self.selected);
            self.say_commit_marks();
        }
        self.select_row(ix, window, cx);
        cx.notify();
    }

    fn say_commit_marks(&mut self) {
        self.set_info(commit_marks::marked_line(self.commit_marks.len(), HOW_TO_CLEAR));
    }

    /// Esc in the commit list: clears the marks; false when none were set.
    pub(super) fn clear_commit_marks(&mut self, cx: &mut Context<Self>) -> bool {
        let any = self.commit_marks.clear();
        if any {
            self.set_info(commit_marks::marked_line(0, ""));
            cx.notify();
        }
        any
    }

    fn toggle_commit_mark(&mut self, ix: usize, cx: &mut Context<Self>) {
        let history = self.history.clone();
        self.commit_marks.toggle(&history, ix);
        self.say_commit_marks();
        cx.notify();
    }

    /// Why Cherry-pick / Revert Marked Commits… can't run now.
    fn several_why_not(&self, kind: SeveralKind) -> Option<&'static str> {
        pick_several::why_not(kind, self.commit_marks.len(), self.commit_marks.has(self.head.id), self.repo_state)
    }

    /// Cherry-pick / Revert Marked Commits…: the dialog with the commits in git's order,
    /// then one sequence (`Repo::pick_several`); the marks go once it runs.
    pub(super) fn several_dialog(&mut self, kind: SeveralKind, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(why) = self.several_why_not(kind) {
            self.set_info(gitti_core::availability::refusal(kind.menu_label(), why));
            cx.notify();
            return;
        }
        let Some(plan) = pick_several::plan(kind, &self.history, &self.commit_marks, self.head.branch.as_deref())
        else {
            return;
        };
        let mut spec = ModalSpec::new(plan.title()).message(plan.describe());
        if kind == SeveralKind::CherryPick {
            spec = spec.check(RECORD_ORIGIN_LABEL, self.last_cherry_pick.record_origin);
        }
        spec = spec.ok(plan.ok_label());
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let mut plan = plan.clone();
            if kind == SeveralKind::CherryPick {
                plan.record_origin = res.checks.first().copied().unwrap_or(false);
                this.last_cherry_pick.record_origin = plan.record_origin;
            }
            this.commit_marks.clear();
            this.git_op(kind.verb(), w, cx, move |r| r.pick_several(&plan));
        });
    }

    /// Palette entries of this module.
    pub(super) fn pick_several_palette_items(&self) -> Vec<PickerItem> {
        let n = self.commit_marks.len();
        let detail = match n {
            0 => "the marked commits (⌘-click / shift-click in the list)".to_string(),
            1 => "1 marked commit".to_string(),
            n => format!("{n} marked commits"),
        };
        let entry = |kind: SeveralKind, detail: String| PickerItem {
            label: kind.menu_label().to_string(),
            detail,
            keys: String::new(),
            color: None,
            run: Rc::new(move |t: &mut GittiApp, w: &mut Window, cx: &mut Context<GittiApp>| {
                t.several_dialog(kind, w, cx)
            }),
            action: None,
        };
        vec![
            entry(SeveralKind::CherryPick, format!("{detail}, oldest first")).act(&CherryPickMarked),
            entry(SeveralKind::Revert, format!("{detail}, newest first")).act(&RevertMarked),
        ]
    }
}

/// The commit menu's first entries when row `ix` is marked: Cherry-pick / Revert N
/// Marked Commits… (greyed with the reason), then a separator.
pub(super) fn marked_commands(menu: PopupMenu, app: &AppHandle, ix: usize, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let this = this.read(cx);
    let n = this.commit_marks.len();
    if n == 0 || !this.is_commit_marked(ix) {
        return menu;
    }
    let mut menu = menu;
    for kind in [SeveralKind::CherryPick, SeveralKind::Revert] {
        let why = this.several_why_not(kind);
        let label = kind.menu_label_n(n);
        let label = match why {
            Some(why) => gitti_core::availability::with_reason(&label, why),
            None => label,
        };
        menu = menu.item(item(label, app, move |t, w, cx| t.several_dialog(kind, w, cx)).disabled(why.is_some()));
    }
    menu.separator()
}

/// The commit menu's last entries: Mark / Unmark row `ix` for Cherry-pick / Revert, and
/// Clear Marks while some are set.
pub(super) fn mark_entries(menu: PopupMenu, app: &AppHandle, ix: usize, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let this = this.read(cx);
    let marked = this.is_commit_marked(ix);
    let label = if marked { UNMARK_LABEL } else { MARK_LABEL };
    let mut menu = menu.separator().item(item(label, app, move |t, _, cx| t.toggle_commit_mark(ix, cx)));
    if !this.commit_marks.is_empty() {
        menu = menu.item(item(CLEAR_LABEL, app, |t, _, cx| {
            t.clear_commit_marks(cx);
        }));
    }
    menu
}
