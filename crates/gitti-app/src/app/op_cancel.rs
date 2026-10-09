//! Cancelling the running operation and the stale-lock offer.
//!
//! `run_op` and clone / init run their job inside `gitti_core::process::with_cancel`; Esc
//! (the keystroke interceptor, `overlay_keys.rs`, while no dialog or output window is open)
//! or the status bar's Cancel button (`op_progress.rs`) cancels it once its busy
//! state shows: git and its hooks / helpers are
//! stopped (SIGTERM to the process group, git removes its lock files), the status says
//! "… cancelled", and the usual refresh after the operation shows what is left. Operations
//! queued behind it are dropped. Stage / unstage / discard (`work_op`, a few ms)
//! are not cancellable.
//!
//! A write that met a `*.lock` file retries for about a second in the core; if the lock
//! stays and is stale (no git running in the repository, older than a few seconds) the
//! operation's job finds out (`gitti_core::lock::stale_lock`) and a dialog offers to remove
//! it instead of leaving only git's raw error.
//!
//! git refusing the repository because another user owns it (`safe.directory`) gets the
//! same kind of offer: the refresh failed, a dialog explains why and, confirmed, trusts the
//! folder (`Repo::trust_directory`, the user's global config); offered once per tab.

use gitti_core::lock::StaleLock;
use gitti_core::safe_directory::DubiousOwnership;
use gitti_core::process::CancelToken;
use gpui_kit::*;

use super::GittiApp;
use super::modal::ModalSpec;

impl GittiApp {
    /// The token of a cancellable operation that starts now.
    pub(super) fn start_cancellable(&mut self) -> CancelToken {
        let token = CancelToken::new();
        self.op_cancel = Some(token.clone());
        token
    }

    /// Whether the running (or just finished) operation was cancelled.
    pub(super) fn op_cancelled(&self) -> bool {
        self.op_cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }

    /// Esc / the status bar's Cancel: cancels the running operation once its busy state
    /// shows; `false` when there is none to cancel (Esc then does what it does otherwise).
    pub(super) fn cancel_op(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.busy_visible {
            return false;
        }
        let Some(token) = self.op_cancel.as_ref().filter(|t| !t.is_cancelled()) else {
            return false;
        };
        token.cancel();
        // The user stops what runs, not just its first part: queued fetch / push go too.
        self.deferred.clear();
        if let Some(b) = &mut self.busy {
            *b = format!("Cancelling: {b}");
        }
        cx.notify();
        true
    }

    /// An operation failed on a stale lock file: says so and offers to remove it.
    pub(super) fn offer_stale_lock(
        &mut self,
        label: &str,
        lock: StaleLock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = lock
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.set_error(format!("{label} failed: a stale {name} is in the way"));
        let spec = ModalSpec::new("Stale lock file")
            .message(lock.describe())
            .ok("Remove Stale Lock");
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            match lock.remove() {
                Ok(()) => this.set_info(format!("Removed {} — try again", lock.path.display())),
                Err(e) => this.set_error(format!("Removing the lock failed: {e}")),
            }
            this.refresh(false, w, cx);
            cx.notify();
        });
    }

    /// git refused the repository (owned by another user): says so and offers to trust
    /// the folder.
    pub(super) fn offer_trust_directory(
        &mut self,
        d: DubiousOwnership,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_error(d.status());
        if self.trust_offered {
            return;
        }
        self.trust_offered = true;
        let spec = ModalSpec::new(DubiousOwnership::TITLE)
            .message(d.describe())
            .ok(DubiousOwnership::CONFIRM);
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            this.git_op("Trust folder", w, cx, move |r| r.trust_directory(&d));
        });
    }
}
