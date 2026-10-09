//! Committing: message drafts and Stage All and Commit.
//!
//! - The typed message is a draft kept per worktree in the settings db
//!   (`gitti_core::store::save_draft`, shared with gitty): written once typing pauses
//!   ([`DRAFT_DELAY`], queued on the store's writer thread — no db I/O here), when the tab /
//!   window closes or the app quits (the entity's release), read in the background when
//!   the repository opens, dropped only after a successful commit / amend. Amend's
//!   prefilled HEAD message and the merge message are no drafts.
//! - Commit with nothing staged asks instead of failing (`gitti_core::ops::plan_commit`):
//!   "Stage All and Commit" (when there are changes) or "Create Empty Commit"; both are
//!   menu / palette commands too.

use std::time::Duration;

use gitti_core::ops::{CommitKind, CommitPlan};
use gpui_kit::*;

use super::GittiApp;

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

    /// The draft as stored for this worktree (tests).
    pub fn script_draft(&self) -> Option<String> {
        gitti_core::store::load_draft(self.repo.as_ref()?.workdir())
    }
}
