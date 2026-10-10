//! Committing: message drafts, the message git prepared, Stage All and Commit.
//!
//! - The typed message is a draft kept per worktree in the settings db
//!   (`gitti_core::store::save_draft`, shared with gitty): written once typing pauses
//!   ([`DRAFT_DELAY`], queued on the store's writer thread — no db I/O here), when the tab /
//!   window closes or the app quits (the entity's release), read in the background when
//!   the repository opens, dropped only after a successful commit / amend. Amend's
//!   prefilled HEAD message and the merge message are no drafts.
//! - The message git left for the next commit (`MERGE_MSG` / `SQUASH_MSG`: a merge,
//!   `cherry-pick` / `revert` — also `-n`, which leaves no CHERRY_PICK_HEAD —, `merge
//!   --squash`; `gitti_core::Repo::prepared_message`) fills an empty box like `git commit`
//!   starts its editor with it, and goes again with the file if nobody edited it. Text the
//!   user typed is never replaced: a note under the box offers git's message ("Use It" /
//!   "Keep Mine"), the palette too (`gitti_core::ops::{sync_prefill, offers_prepared}`).
//! - Commit with nothing staged asks instead of failing (`gitti_core::ops::plan_commit`):
//!   "Stage All and Commit" (when there are changes) or "Create Empty Commit"; both are
//!   menu / palette commands too.

use std::rc::Rc;
use std::time::Duration;

use gitti_core::ops::{CommitKind, CommitPlan, PrefillSync};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::*;

use super::GittiApp;
use super::picker::PickerItem;
use crate::probe::ProbeExt as _;
use crate::theme::PaletteExt as _;

/// What a palette entry runs.
type PaletteRun = dyn Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>);

/// Typing pauses this long before the draft is written.
const DRAFT_DELAY: Duration = Duration::from_millis(500);

impl GittiApp {
    /// Commit: what [`gitti_core::ops::plan_commit`] says — run it, conclude the merge,
    /// say why not, or ask what to do with nothing staged.
    pub fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.message.read(cx).value().to_string();
        let plan = gitti_core::ops::plan_commit(
            self.repo_state,
            self.working.conflicted.len(),
            self.working.staged.len(),
            self.working.unstaged.len(),
            self.amend,
            &msg,
        );
        match plan {
            CommitPlan::Blocked(why) => self.set_error(why),
            CommitPlan::ConcludeMerge => {
                // Concludes the merge with git's prepared message (MERGE_MSG).
                self.run_op(
                    "Committing",
                    true,
                    window,
                    cx,
                    |r| r.continue_operation(),
                    |this, out, _, _| {
                        this.merge_prefill = None;
                        this.committed();
                        this.set_info(out.lines().next().unwrap_or("Merge committed").to_string());
                    },
                );
                return;
            }
            CommitPlan::NoMessage => self.set_error("Commit message is empty".into()),
            CommitPlan::NothingStaged { .. } => self.ask_nothing_staged(window, cx),
            CommitPlan::Run(kind) => self.run_commit(kind, window, cx),
        }
        cx.notify();
    }

    /// "Stage All and Commit" / "Create Empty Commit" from the menu or the palette (and
    /// the nothing-staged question): needs a message like Commit.
    pub(super) fn commit_kind(&mut self, kind: CommitKind, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.message.read(cx).value().to_string();
        if let Some(why) =
            gitti_core::ops::commit_blocker(self.repo_state, self.working.conflicted.len())
        {
            self.set_error(why);
        } else if kind == CommitKind::Empty
            && let Some(why) =
                gitti_core::ops::empty_commit_blocker(self.working.staged.len(), self.amend)
        {
            self.set_error(why);
        } else if msg.trim().is_empty() {
            self.set_error("Commit message is empty".into());
            self.message.update(cx, |m, cx| m.focus(window, cx));
        } else {
            self.run_commit(kind, window, cx);
        }
        cx.notify();
    }

    fn run_commit(&mut self, kind: CommitKind, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.message.read(cx).value().to_string();
        let label = match kind {
            CommitKind::Amend => "Amending",
            _ => "Committing",
        };
        self.run_op(
            label,
            true,
            window,
            cx,
            move |r| r.commit_as(&msg, kind),
            |this, out, window, cx| {
                this.message.update(cx, |m, cx| m.set_value("", window, cx));
                this.merge_prefill = None;
                this.amend = false;
                this.committed();
                this.set_info(out.lines().next().unwrap_or("Committed").to_string());
            },
        );
    }

    /// Nothing staged: Stage All and Commit (with changes) / Create Empty Commit / Cancel.
    fn ask_nothing_staged(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (title, detail, choices) =
            gitti_core::ops::nothing_staged_prompt(self.working.unstaged.len());
        let mut buttons: Vec<&str> = choices.iter().map(|(label, _)| *label).collect();
        buttons.push("Cancel");
        let answer = window.prompt(PromptLevel::Info, &title, Some(&detail), &buttons, cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(ix) = answer.await
                && let Some((_, kind)) = choices.get(ix).copied()
            {
                this.update_in(cx, |this, window, cx| this.commit_kind(kind, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// A commit / amend / merge conclusion succeeded: its message is no draft any more.
    fn committed(&mut self) {
        self.draft_dirty = false;
        self.draft_save = None;
        if let Some(repo) = &self.repo {
            gitti_core::store::clear_draft(repo.workdir());
        }
    }

    // ------------------------------------------------------------------ drafts

    /// The message box changed by typing (not by `set_value`): write the draft once the
    /// typing pauses. While amending the box holds HEAD's message, no draft.
    pub(super) fn draft_changed(&mut self, cx: &mut Context<Self>) {
        if self.amend || self.repo.is_none() {
            return;
        }
        self.draft_dirty = true;
        // Replacing the task drops (cancels) the previous wait: a debounce.
        self.draft_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DRAFT_DELAY).await;
            this.update(cx, |this, cx| this.save_draft_now(cx)).ok();
        }));
    }

    /// Writes the draft if it changed since the last write (queued, returns at once).
    pub(super) fn save_draft_now(&mut self, cx: &App) {
        if !std::mem::take(&mut self.draft_dirty) || self.amend {
            return;
        }
        let Some(repo) = &self.repo else { return };
        let text = self.message.read(cx).value().to_string();
        // The merge message git prepared is git's, not the user's.
        let text = if self.merge_prefill.as_deref() == Some(text.as_str()) {
            String::new()
        } else {
            text
        };
        gitti_core::store::save_draft(repo.workdir(), &text);
    }

    /// The repository opened: its draft (read in the background) goes into the message
    /// box unless the user typed meanwhile; it replaces a merge message nobody edited.
    pub(super) fn restore_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else { return };
        let task = cx
            .background_executor()
            .spawn(async move { gitti_core::store::load_draft(repo.workdir()) });
        cx.spawn_in(window, async move |this, cx| {
            let Some(draft) = task.await else { return };
            this.update_in(cx, |this, window, cx| {
                let current = this.message.read(cx).value().to_string();
                let untouched =
                    current.trim().is_empty() || this.merge_prefill.as_deref() == Some(&current);
                if untouched && !this.amend {
                    this.message.update(cx, |m, cx| m.set_value(draft, window, cx));
                    this.merge_prefill = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------- git's prepared message

    /// A refresh read the message git left for the next commit: put it into an empty box
    /// (or over an older prefill nobody edited), take an untouched prefill out once git's
    /// file is gone ([`gitti_core::ops::sync_prefill`]).
    pub(super) fn sync_prepared_message(
        &mut self,
        prepared: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.message.read(cx).value().to_string();
        let sync = gitti_core::ops::sync_prefill(
            prepared.as_deref(),
            self.merge_prefill.as_deref(),
            &current,
            self.amend,
        );
        match sync {
            PrefillSync::Keep => {}
            PrefillSync::Fill(m) => {
                self.message.update(cx, |e, cx| e.set_value(m.clone(), window, cx));
                self.merge_prefill = Some(m);
            }
            PrefillSync::Clear => {
                self.message.update(cx, |e, cx| e.set_value("", window, cx));
                self.merge_prefill = None;
            }
            PrefillSync::Forget => self.merge_prefill = None,
        }
        if prepared.is_none() {
            self.prepared_dismissed = None;
        }
        self.prepared = prepared;
    }

    /// git's prepared message differs from the user's text in the box (the palette offers
    /// it then, the note unless dismissed).
    fn prepared_differs(&self, cx: &App) -> Option<String> {
        let p = self.prepared.as_ref()?;
        let current = self.message.read(cx).value();
        let differs = !self.amend && current.trim() != p.as_str();
        differs.then(|| p.clone())
    }

    /// "Use It": git's prepared message replaces the box's text (and the stored draft).
    pub(super) fn use_prepared_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.prepared_differs(cx) else { return };
        self.message.update(cx, |e, cx| e.set_value(m.clone(), window, cx));
        self.merge_prefill = Some(m);
        // The draft the box held goes: git's message is no draft.
        self.draft_dirty = true;
        self.save_draft_now(cx);
        cx.notify();
    }

    /// "Keep Mine": the note goes for this prepared message (Commit uses the box's text;
    /// git removes its file with the commit).
    fn keep_my_message(&mut self, cx: &mut Context<Self>) {
        self.prepared_dismissed = self.prepared.clone();
        cx.notify();
    }

    /// Whether the note under the message box shows: git's prepared message waits next
    /// to the user's own text ([`gitti_core::ops::offers_prepared`]).
    pub(super) fn prepared_note_shown(&self, cx: &App) -> bool {
        gitti_core::ops::offers_prepared(
            self.prepared.as_deref(),
            self.merge_prefill.as_deref(),
            self.prepared_dismissed.as_deref(),
            &self.message.read(cx).value(),
            self.amend,
        )
    }

    /// The note under the message box while [`Self::prepared_note_shown`].
    pub(super) fn render_prepared_note(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.prepared_note_shown(cx) {
            return None;
        }
        let p = cx.palette().clone();
        let note = gitti_core::ops::prepared_note(self.prepared.as_deref()?);
        let full = self.prepared.clone()?;
        Some(
            h_flex()
                .id("prepared-note")
                .probe("prepared-note")
                .min_w_0()
                .gap_1()
                .items_center()
                .px_1()
                .text_size(self.settings.ui_small())
                .child(
                    div()
                        .id("prepared-note-text")
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(p.text_muted)
                        .child(note)
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(format!(
                                "git left this message for the next commit (MERGE_MSG / \
                                 SQUASH_MSG); `git commit` would start with it:\n\n{full}"
                            ))
                            .build(window, cx)
                        }),
                )
                .child(
                    Button::new("prepared-use")
                        .probe("prepared-use")
                        .label("Use It")
                        .xsmall()
                        .tooltip("Replace your text in the box with git's message")
                        .on_click(cx.listener(|this, _, w, cx| this.use_prepared_message(w, cx))),
                )
                .child(
                    Button::new("prepared-keep")
                        .probe("prepared-keep")
                        .label("Keep Mine")
                        .xsmall()
                        .ghost()
                        .tooltip("Hide this note; Commit uses your text and git drops its message")
                        .on_click(cx.listener(|this, _, _, cx| this.keep_my_message(cx))),
                )
                .into_any_element(),
        )
    }

    /// Palette: Use the Message git Prepared (while it differs from the box's text), Keep
    /// My Commit Message (while the note shows) — as gitty.
    pub(super) fn prepared_palette_items(&self, cx: &App) -> Vec<PickerItem> {
        let entry = |label: &str, detail: String, run: Rc<PaletteRun>| PickerItem {
            label: label.into(),
            detail,
            keys: String::new(),
            color: None,
            run,
            action: None,
        };
        let mut items = Vec::new();
        if let Some(m) = self.prepared_differs(cx) {
            let subject = m.lines().next().unwrap_or_default().to_string();
            let run: Rc<PaletteRun> = Rc::new(|t, w, cx| t.use_prepared_message(w, cx));
            items.push(entry(gitti_core::ops::USE_PREPARED_LABEL, subject, run));
        }
        if self.prepared_note_shown(cx) {
            let run: Rc<PaletteRun> = Rc::new(|t, _, cx| t.keep_my_message(cx));
            items.push(entry(gitti_core::ops::KEEP_MINE_LABEL, "hide the note".into(), run));
        }
        items
    }

    /// The draft as stored for this worktree (tests).
    pub fn script_draft(&self) -> Option<String> {
        gitti_core::store::load_draft(self.repo.as_ref()?.workdir())
    }
}
