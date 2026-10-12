//! Repository ▸ Fetch Full History / Fetch More History… (`gitti_core::shallow`): a
//! shallow clone's history fetched in full or N commits deeper, after a dialog that says
//! from which remote and for which branches. Greyed in a complete repository
//! (`availability`); the fetch runs like Fetch (busy mark, progress, cancel) and the
//! history reloads afterwards.

use gitti_core::shallow::{self, ShallowFetch};
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalSpec};

impl GittiApp {
    /// Where the fetch goes (in-process); a failure is said in the status line.
    fn shallow_plan(&mut self, what: &str, cx: &mut Context<Self>) -> Option<ShallowFetch> {
        let repo = self.repo.clone()?;
        match repo.shallow_fetch() {
            Ok(p) => Some(p),
            Err(e) => {
                self.set_error(format!("{}: {e:#}", what.trim_end_matches('…')));
                cx.notify();
                None
            }
        }
    }

    /// Asks, then `git fetch --unshallow <remote>`.
    pub(super) fn fetch_full_history_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plan) = self.shallow_plan(shallow::FULL_LABEL, cx) else {
            return;
        };
        let spec = ModalSpec::new(shallow::FULL_LABEL)
            .message(plan.full_message())
            .ok("Fetch");
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            let remote = plan.remote;
            this.git_op("Fetch full history", w, cx, move |r| r.fetch_full_history(&remote));
        });
    }

    /// Asks how many commits (100), then `git fetch --deepen=N <remote>`.
    pub(super) fn fetch_more_history_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plan) = self.shallow_plan(shallow::MORE_LABEL, cx) else {
            return;
        };
        let spec = ModalSpec::new(shallow::MORE_LABEL.trim_end_matches('…'))
            .message(plan.more_message())
            .field(shallow::MORE_FIELD, shallow::MORE_DEFAULT.to_string())
            .live(|res| match shallow::parse_count(&res.values[0]) {
                Ok(n) => ModalLive {
                    ok_label: Some(format!("Fetch {n} More")),
                    ..Default::default()
                },
                Err(why) => ModalLive {
                    blocked: Some(why),
                    ..Default::default()
                },
            })
            .ok("Fetch");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let n = match shallow::parse_count(&res.values[0]) {
                Ok(n) => n,
                Err(why) => {
                    this.set_error(format!("Fetch More History: {why}"));
                    cx.notify();
                    return;
                }
            };
            let remote = plan.remote;
            this.git_op("Fetch more history", w, cx, move |r| r.fetch_more_history(&remote, n));
        });
    }
}
