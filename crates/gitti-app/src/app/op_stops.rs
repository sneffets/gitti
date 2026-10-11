//! A cherry-pick / revert / rebase stopped at a commit: Skip (banner, palette); stopped
//! with nothing to commit ([`gitti_core::skip::EmptyPick`]): Skip or Commit Empty; an
//! operation git refused because of uncommitted changes
//! ([`gitti_core::local_changes::LocalChanges`]): what blocks and the ways out — the same
//! questions as gitty's.

use gitti_core::local_changes::LocalChanges;
use gitti_core::skip::{self, EmptyPick};
use gpui_kit::*;

use super::GittiApp;

impl GittiApp {
    /// Skip of the stopped commit: asks first when it throws away staged or conflicted
    /// changes.
    pub(super) fn skip_operation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let go = |this: &mut Self, w: &mut Window, cx: &mut Context<Self>| {
            this.git_op("Skip", w, cx, |r| r.skip_operation())
        };
        match skip::skip_prompt(self.repo_state, self.working.staged.len(), self.working.conflicted.len()) {
            Some((title, detail, ok)) => self.confirm(title, Some(detail), ok, window, cx, go),
            None => go(self, window, cx),
        }
    }

    /// A failed operation's questions (the error itself is reported by `op_failed`):
    /// nothing to commit (Skip / Commit Empty), refused because of uncommitted changes
    /// (the ways out).
    pub(super) fn offer_ways_out(&mut self, e: &anyhow::Error, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(empty) = e.downcast_ref::<EmptyPick>() {
            let answer = window.prompt(
                PromptLevel::Info,
                empty.title(),
                Some(&empty.describe()),
                &EmptyPick::CHOICES,
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                let Ok(ix) = answer.await else {
                    return;
                };
                this.update_in(cx, |this, w, cx| match ix {
                    0 => this.git_op("Skip", w, cx, |r| r.skip_operation()),
                    1 => this.commit_empty_pick(w, cx),
                    _ => {}
                })
                .ok();
            })
            .detach();
        } else if let Some(block) = e.downcast_ref::<LocalChanges>() {
            let block = block.clone();
            let choices = block.choices();
            let labels: Vec<&str> = choices.iter().map(|(l, _)| l.as_str()).collect();
            let answer = window.prompt(PromptLevel::Warning, &block.title(), Some(&block.describe()), &labels, cx);
            let ways: Vec<_> = choices.iter().map(|(_, w)| *w).collect();
            cx.spawn_in(window, async move |this, cx| {
                let Ok(ix) = answer.await else {
                    return;
                };
                let Some(Some(way)) = ways.get(ix).copied() else {
                    return;
                };
                this.update_in(cx, |this, w, cx| {
                    let label = block.op.title();
                    this.git_op(label, w, cx, move |r| r.resolve_local_changes(&block, way))
                })
                .ok();
            })
            .detach();
        }
    }

    /// Commit Empty of a pick with nothing to commit: with the box's message when the
    /// user edited git's, else git's (as Continue).
    pub(super) fn commit_empty_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.continue_message(cx);
        let used = msg.is_some();
        let opts = self.commit_options();
        let committed_msg = msg.clone();
        self.run_op(
            "Commit empty",
            true,
            window,
            cx,
            move |r| {
                let out = r.commit_empty_pick(msg.as_deref(), opts);
                let committed = match &out {
                    Ok(_) => used,
                    Err(e) => e.is::<gitti_core::ops::MessageCommitted>(),
                };
                if committed {
                    gitti_core::store::clear_draft(r.workdir());
                }
                out
            },
            move |this, out, window, cx| {
                if used {
                    if let Some(m) = &committed_msg {
                        this.note_committed_message(m);
                    }
                    this.message_committed(window, cx);
                }
                this.set_info(super::last_line(&out, "Committed empty"));
            },
        );
    }
}
