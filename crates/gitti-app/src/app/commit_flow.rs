//! Committing: message drafts, the message git prepared, Stage All and Commit.
//!
//! - The typed message is a draft kept per worktree in the settings db
//!   (`gitti_core::store::save_draft`, shared with gitty): written once typing pauses
//!   ([`DRAFT_DELAY`], queued on the store's writer thread — no db I/O here), when the tab /
//!   window closes or the app quits (the entity's release), read in the background when
//!   the repository opens, dropped only after a successful commit / amend — by the
//!   commit's own job, so also when the tab was closed while it ran. Amend's prefilled
//!   HEAD message and the merge message are no drafts. An edited amend message is a draft
//!   of its own (`store::save_amend_draft`), kept for the commit it amends: turning amend
//!   on again brings it back while that commit is still HEAD, else HEAD's message.
//! - The message git left for the next commit (`MERGE_MSG` / `SQUASH_MSG`: a merge,
//!   `cherry-pick` / `revert` — also `-n`, which leaves no CHERRY_PICK_HEAD —, `merge
//!   --squash`; `gitti_core::Repo::prepared_message`) fills an empty box like `git commit`
//!   starts its editor with it, and goes again with the file if nobody edited it. Text the
//!   user typed is never replaced: a note under the box offers git's message ("Use It" /
//!   "Keep Mine"), the palette too (`gitti_core::ops::{sync_prefill, offers_prepared}`).
//!   Keep Mine is stored with the draft (`store::save_kept_prepared`): after a restart the
//!   note stays hidden while git's message is the same.
//! - Continue (banner, palette) of a merge / cherry-pick / revert / rebase commits with the
//!   box's text when the user edited git's message (`gitti_core::ops::continue_message`,
//!   `Repo::continue_with`), else with git's own; a used message empties the box like a
//!   commit — also when git stops again after it (`gitti_core::ops::MessageCommitted`).
//! - Commit with nothing staged asks instead of failing (`gitti_core::ops::plan_commit`,
//!   `nothing_staged_prompt`, which says what each choice stages): "Stage All and Commit"
//!   (`add -A`), "Stage Tracked Files and Commit" (`add -u`, when untracked files would
//!   come along) or "Create Empty Commit"; all three are menu / palette commands too.

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
                    |r| {
                        let out = r.continue_operation()?;
                        gitti_core::store::clear_draft(r.workdir());
                        Ok(out)
                    },
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

    /// The message Continue commits with: the box's text when the user edited git's
    /// ([`gitti_core::ops::continue_message`]); `None`: git's own.
    pub(super) fn continue_message(&self, cx: &App) -> Option<String> {
        let current = self.message.read(cx).value().to_string();
        gitti_core::ops::continue_message(self.prepared.as_deref(), &current, self.amend)
    }

    /// Continue (the banner, the palette): the operation goes on, its stopped commit with
    /// the box's text when the user edited git's message (`Repo::continue_with`).
    pub(super) fn continue_operation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.continue_message(cx);
        let used = msg.is_some();
        self.run_op(
            "Continue",
            true,
            window,
            cx,
            move |r| {
                let out = r.continue_with(msg.as_deref());
                let committed = match &out {
                    Ok(_) => used,
                    Err(e) => e.is::<gitti_core::ops::MessageCommitted>(),
                };
                if committed {
                    // Here, not in `message_committed`: a tab closed meanwhile keeps no
                    // stale draft.
                    gitti_core::store::clear_draft(r.workdir());
                }
                out
            },
            move |this, out, window, cx| {
                if used {
                    this.message_committed(window, cx);
                }
                this.set_info(super::last_line(&out, "Continue done"));
            },
        );
    }

    /// Continue committed the box's text (`run_op` also calls it when git stopped again
    /// right after, [`gitti_core::ops::MessageCommitted`]): the box empties as after a
    /// commit; git's next message can fill it.
    pub(super) fn message_committed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.message.update(cx, |m, cx| m.set_value("", window, cx));
        self.merge_prefill = None;
        self.committed();
    }

    /// "Stage All and Commit" / "Stage Tracked Files and Commit" / "Create Empty Commit"
    /// from the menu or the palette (and the nothing-staged question): needs a message like
    /// Commit.
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
            move |r| {
                let out = r.commit_as(&msg, kind)?;
                // Here, not in `committed`: a tab closed meanwhile keeps no stale draft.
                gitti_core::store::clear_draft(r.workdir());
                if kind == CommitKind::Amend {
                    gitti_core::store::clear_amend_draft(r.workdir());
                }
                Ok(out)
            },
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
        let counts = gitti_core::ops::UnstagedCounts::of(&self.working);
        let (title, detail, choices) = gitti_core::ops::nothing_staged_prompt(&counts);
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
    /// Its job dropped the stored one already (so does a tab closed meanwhile); again here,
    /// as the box empties: text typed while the commit ran must not come back as a draft.
    fn committed(&mut self) {
        self.draft_dirty = false;
        self.draft_save = None;
        // HEAD moved: an amend draft was for the commit before.
        self.amend_draft = None;
        self.prepared_dismissed = None;
        if let Some(repo) = &self.repo {
            gitti_core::store::clear_draft(repo.workdir());
        }
    }

    // ------------------------------------------------------------------ drafts

    /// The message box changed by typing (not by `set_value`): write the draft once the
    /// typing pauses — while amending the amend draft.
    pub(super) fn draft_changed(&mut self, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        self.draft_dirty = true;
        // Replacing the task drops (cancels) the previous wait: a debounce.
        self.draft_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DRAFT_DELAY).await;
            this.update(cx, |this, cx| this.save_draft_now(cx)).ok();
        }));
    }

    /// Writes the draft if it changed since the last write (queued, returns at once);
    /// while amending the amend draft.
    pub(super) fn save_draft_now(&mut self, cx: &App) {
        if !std::mem::take(&mut self.draft_dirty) {
            return;
        }
        let Some(repo) = self.repo.clone() else { return };
        let text = self.message.read(cx).value().to_string();
        if self.amend {
            self.save_amend_draft(&repo, &text);
            return;
        }
        // The merge message git prepared is git's, not the user's.
        let text = if self.merge_prefill.as_deref() == Some(text.as_str()) {
            String::new()
        } else {
            text
        };
        gitti_core::store::save_draft(repo.workdir(), &text);
    }

    /// The amend draft: the box's text while amending, kept for HEAD — none while it is
    /// HEAD's own message ([`gitti_core::ops::amend_draft_text`]).
    fn save_amend_draft(&mut self, repo: &gitti_core::Repo, text: &str) {
        let Some(head) = self.head.id.map(|id| id.to_string()) else { return };
        let head_message = repo.head_message().unwrap_or_default();
        let text = gitti_core::ops::amend_draft_text(text, &head_message);
        gitti_core::store::save_amend_draft(repo.workdir(), &head, text);
        self.amend_draft = (!text.trim().is_empty()).then(|| gitti_core::store::AmendDraft {
            head,
            message: text.to_string(),
        });
    }

    /// What amend starts the empty box with: the amend draft while its commit is still
    /// HEAD, else HEAD's message.
    pub(super) fn amend_start_message(&self) -> Option<String> {
        let head = self.head.id.map(|id| id.to_string());
        if let Some(m) = self.amend_draft.as_ref().and_then(|d| d.for_head(head.as_deref())) {
            return Some(m.to_string());
        }
        let msg = self.repo.as_ref()?.head_message().ok()?;
        Some(msg.trim_end().to_string())
    }

    /// The repository opened: its draft (read in the background) goes into the message
    /// box unless the user typed meanwhile; it replaces a merge message nobody edited.
    /// A Keep Mine stored with it hides the note again. The amend draft is kept for when
    /// amend is turned on.
    pub(super) fn restore_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else { return };
        let task = cx.background_executor().spawn(async move {
            let w = repo.workdir();
            (gitti_core::store::load_draft(w), gitti_core::store::load_amend_draft(w))
        });
        cx.spawn_in(window, async move |this, cx| {
            let (draft, amend) = task.await;
            if amend.is_some() {
                this.update(cx, |this, _| {
                    if this.amend_draft.is_none() {
                        this.amend_draft = amend;
                    }
                })
                .ok();
            }
            let Some(draft) = draft else { return };
            this.update_in(cx, |this, window, cx| {
                let current = this.message.read(cx).value().to_string();
                let untouched =
                    current.trim().is_empty() || this.merge_prefill.as_deref() == Some(&current);
                if untouched && !this.amend {
                    this.message.update(cx, |m, cx| m.set_value(draft.message, window, cx));
                    this.merge_prefill = None;
                    if this.prepared_dismissed.is_none() {
                        this.prepared_dismissed = draft.kept_prepared;
                    }
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
        if prepared.is_none()
            && self.prepared_dismissed.take().is_some()
            && let Some(repo) = &self.repo
        {
            // git's message went (committed, aborted): Keep Mine has nothing to hide.
            gitti_core::store::save_kept_prepared(repo.workdir(), None);
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
    /// git removes its file with the commit) — stored with the draft, so also after a
    /// restart.
    fn keep_my_message(&mut self, cx: &mut Context<Self>) {
        self.prepared_dismissed = self.prepared.clone();
        if let Some(repo) = self.repo.clone() {
            // The box's text (the user's own, else no note) as the draft first — writes are
            // queued in order: the row the choice is kept in.
            self.draft_dirty = true;
            self.save_draft_now(cx);
            let kept = self.prepared_dismissed.as_deref();
            gitti_core::store::save_kept_prepared(repo.workdir(), kept);
        }
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
        gitti_core::store::load_draft(self.repo.as_ref()?.workdir()).map(|d| d.message)
    }

    /// The amend draft stored for this worktree: (commit, message) (tests).
    pub fn script_amend_draft(&self) -> Option<(String, String)> {
        let d = gitti_core::store::load_amend_draft(self.repo.as_ref()?.workdir())?;
        Some((d.head, d.message))
    }

    /// The Keep Mine stored with this worktree's draft (tests).
    pub fn script_kept_prepared(&self) -> Option<String> {
        gitti_core::store::load_draft(self.repo.as_ref()?.workdir())?.kept_prepared
    }
}
