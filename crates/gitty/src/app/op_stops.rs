//! A cherry-pick / revert / rebase stopped at a commit: Skip (palette); stopped with
//! nothing to commit ([`gitti_core::skip::EmptyPick`]): Skip or Commit Empty; an
//! operation git refused because of uncommitted changes
//! ([`gitti_core::local_changes::LocalChanges`]): what blocks and the ways out — the
//! same questions as gitti's.

use gitti_core::local_changes::{LocalChanges, LocalChangesWay};
use gitti_core::skip::{self, EmptyPick};

use super::{After, App, SWITCH_NOTE_WIDTH};
use crate::modal::{MenuItem, Modal};

/// `text` as menu notes, line by line (an indented line — a file — stays indented).
fn note_lines(text: &str) -> Vec<MenuItem> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .flat_map(|l| {
            let indent = l.starts_with("    ");
            let mut notes = MenuItem::notes(l, SWITCH_NOTE_WIDTH);
            if indent && let Some(n) = notes.first_mut() {
                n.label = format!("  {}", n.label);
            }
            notes
        })
        .collect()
}

impl App {
    /// Skip of the stopped commit: asks first when it throws away staged or conflicted
    /// changes.
    pub fn skip_operation(&mut self) {
        let go = |app: &mut App| app.run_op("Skip", true, After::Nothing, |r| r.skip_operation());
        match skip::skip_prompt(self.repo_state, self.working.staged.len(), self.working.conflicted.len()) {
            Some((title, detail, ok)) => self.modal = Some(Modal::confirm(title, detail, ok, go)),
            None => go(self),
        }
    }

    /// A failed operation's questions: nothing to commit (Skip / Commit Empty), refused
    /// because of uncommitted changes (the ways out).
    pub(super) fn offer_ways_out(&mut self, e: &anyhow::Error) {
        if let Some(empty) = e.downcast_ref::<EmptyPick>() {
            let mut items = note_lines(&empty.describe());
            items.push(MenuItem::separator());
            items.push(MenuItem::new(skip::SKIP_LABEL, |app: &mut App| app.skip_operation()).key('s'));
            items.push(MenuItem::new("Commit Empty", |app: &mut App| app.commit_empty_pick()).key('e'));
            self.modal = Some(Modal::menu(empty.title(), items));
        } else if let Some(block) = e.downcast_ref::<LocalChanges>() {
            let mut items = note_lines(&block.blocking());
            for (way, key) in LocalChangesWay::ALL.into_iter().zip(['s', 'd']) {
                items.push(MenuItem::separator());
                items.extend(MenuItem::notes(&way.explain(block), SWITCH_NOTE_WIDTH));
                let b = block.clone();
                let label = block.op.title();
                items.push(
                    MenuItem::new(way.label(block), move |app: &mut App| {
                        app.run_op(label, true, After::Nothing, move |r| r.resolve_local_changes(&b, way))
                    })
                    .key(key),
                );
            }
            self.modal = Some(Modal::menu(block.title(), items));
        }
    }

    /// Commit Empty of a pick with nothing to commit: with the box's message when the
    /// user edited git's, else git's.
    pub fn commit_empty_pick(&mut self) {
        let msg = self.continue_message();
        let after = if msg.is_some() { After::ClearMessage } else { After::Nothing };
        let opts = self.commit_options();
        self.run_op("Commit empty", true, after, move |r| {
            let used = msg.is_some();
            let out = r.commit_empty_pick(msg.as_deref(), opts);
            let committed = match &out {
                Ok(_) => used,
                Err(e) => e.is::<gitti_core::ops::MessageCommitted>(),
            };
            if committed {
                gitti_core::store::clear_draft(r.workdir());
            }
            out
        });
    }
}
