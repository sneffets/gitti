//! "Reset <branch> to here — soft / mixed / hard" (keys S / M / H) of the commit menu and
//! of a reflog entry's menu: soft and mixed lose nothing and run at once (the status says what
//! happened); a hard reset first reads what it would lose (in the background) and asks
//! with that ([`gitti_core::reset::ResetPlan`]).

use gitti_core::ops::ResetMode;
use gitti_core::reset::ResetPlan;

use super::{After, App, Msg};
use crate::modal::{MenuItem, Modal};

impl App {
    /// The three reset entries for commit `sha`, moving `head` (the branch, or `HEAD`
    /// while detached).
    pub(super) fn reset_items(&self, sha: &str, head: &str) -> Vec<MenuItem> {
        // The same keys in every menu that has them: S, M, H.
        [("soft", 'S', ResetMode::Soft), ("mixed", 'M', ResetMode::Mixed), ("hard", 'H', ResetMode::Hard)]
            .into_iter()
            .map(|(label, key, mode)| {
                let s = sha.to_string();
                MenuItem::new(format!("Reset {head} to here — {label}"), move |app: &mut App| {
                    app.reset_to(s, mode)
                })
                .key(key)
            })
            .collect()
    }

    /// Resets HEAD to `sha` with `mode`; a hard reset asks first (see the module docs).
    pub(super) fn reset_to(&mut self, sha: String, mode: ResetMode) {
        if mode != ResetMode::Hard {
            let label = if mode == ResetMode::Soft { "Reset (soft)" } else { "Reset (mixed)" };
            return self.run_op(label, true, After::Summary, move |r| r.reset(&sha, mode));
        }
        let repo = self.repo.clone();
        self.start_load(
            move || repo.reset_plan(&sha, mode).map(|plan| (sha, plan)),
            |result| {
                Msg::Apply(Box::new(move |app: &mut App| match result {
                    Ok((sha, plan)) => app.ask_hard_reset(sha, plan),
                    Err(e) => app.set_error(format!("{e:#}")),
                }))
            },
        );
    }

    /// The hard reset's question: what moves, which uncommitted changes are lost, which
    /// untracked files stay, where the commits that leave the branch are found again.
    fn ask_hard_reset(&mut self, sha: String, plan: ResetPlan) {
        self.modal = Some(Modal::confirm(plan.title(), plan.describe(), plan.ok_label(), move |app: &mut App| {
            app.run_op("Reset (hard)", true, After::Summary, move |r| r.reset(&sha, ResetMode::Hard))
        }));
    }
}
