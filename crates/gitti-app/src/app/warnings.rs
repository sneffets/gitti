//! How long warnings and messages stay.
//!
//! - A failed operation's warning — the toolbar's "⚠ Merge failed" and the error in
//!   the status bar — stays until the next git operation the user starts (any one, not
//!   only the same; background refreshes don't count), until its details are opened, or
//!   until the same operation succeeds. View ▸ Last Error Details shows it afterwards too
//!   (`ErrorMarks::last_details`). The mark has no timeout.
//! - A plain one-line info message ("Fetch done", "Copied 2 line(s)") fades after
//!   [`INFO_FADE`]; multi-line output ("N lines of output (click for details)") stays.

use std::time::Duration;

use gpui_kit::*;

use super::GittiApp;

/// How long a plain info message stays in the status bar.
pub(super) const INFO_FADE: Duration = Duration::from_secs(10);

impl GittiApp {
    /// The user starts a git operation: earlier warnings go (their details stay).
    pub(super) fn user_op_started(&mut self) {
        let status_error = self
            .status
            .as_ref()
            .filter(|s| s.error)
            .map(|s| s.text.clone());
        self.errors.dismiss(status_error.as_deref());
        if status_error.is_some() {
            self.status = None;
        }
    }

    /// Starts the fade timer of a new one-line info message (once per message; called
    /// from `render`, where every message shows up).
    pub(super) fn schedule_status_fade(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.status else {
            return;
        };
        if s.error || s.text.contains('\n') || self.status_fading == self.status_gen {
            return;
        }
        let generation = self.status_gen;
        self.status_fading = generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INFO_FADE).await;
            this.update(cx, |this, cx| {
                if this.status_gen == generation && this.status.as_ref().is_some_and(|s| !s.error) {
                    this.status = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}
