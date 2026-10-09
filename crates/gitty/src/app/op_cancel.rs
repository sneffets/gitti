//! Cancelling the running operation and the stale-lock offer (gitti: `app/op_cancel.rs`).
//!
//! `run_op` runs its job inside `gitti_core::process::with_cancel`; Esc while its busy label
//! shows cancels it: git and its hooks / helpers are stopped (SIGTERM to the process group,
//! git removes its lock files), the status says "… cancelled", the usual refresh follows.
//! Operations queued behind it are dropped. Stage / unstage / discard (`work_op`)
//! are not cancellable. A write that failed on a stale `*.lock` file (no git running in the
//! repository, older than a few seconds; `gitti_core::lock`) gets a confirm dialog that
//! removes it. git refusing the repository because another user owns it
//! (`safe.directory`) gets one too: it explains why and, confirmed, trusts the
//! folder (`Repo::trust_directory`, the user's global config); offered once.

use gitti_core::lock::StaleLock;
use gitti_core::safe_directory::DubiousOwnership;

use super::App;
use crate::modal::Modal;

impl App {
    /// Esc: cancels the running operation once its busy label shows; `false` when there
    /// is none to cancel (Esc then does what it does otherwise).
    pub fn cancel_op(&mut self) -> bool {
        if self.busy_label().is_none() {
            return false;
        }
        let Some(token) = self.op_cancel.as_ref().filter(|t| !t.is_cancelled()) else {
            return false;
        };
        token.cancel();
        // The user stops what runs, not just its first part: queued fetch / push go too.
        self.deferred.clear();
        if let Some((label, _)) = &mut self.busy {
            *label = format!("Cancelling: {label}");
        }
        true
    }

    /// Esc in a credential prompt: stops the running operation at once (also before its
    /// busy label shows) and drops the operations queued behind it.
    pub(super) fn stop_op_for_ask(&mut self) {
        if self.busy.is_none() {
            return;
        }
        let Some(token) = self.op_cancel.as_ref().filter(|t| !t.is_cancelled()) else {
            return;
        };
        token.cancel();
        self.deferred.clear();
        if let Some((label, _)) = &mut self.busy {
            *label = format!("Cancelling: {label}");
        }
    }

    /// An operation failed on a stale lock file: says so and offers to remove it.
    pub(super) fn offer_stale_lock(&mut self, label: &str, lock: StaleLock) {
        let name = lock
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.set_error(format!("{label} failed: a stale {name} is in the way"));
        self.modal = Some(Modal::confirm(
            "Stale lock file",
            lock.describe(),
            "Remove Stale Lock",
            move |app| {
                match lock.remove() {
                    Ok(()) => app.set_info(format!("Removed {} — try again", lock.path.display())),
                    Err(e) => app.set_error(format!("Removing the lock failed: {e}")),
                }
                app.refresh(false);
            },
        ));
    }

    /// git refused the repository (owned by another user): says so and offers to trust
    /// the folder.
    pub(super) fn offer_trust_directory(&mut self, d: DubiousOwnership) {
        self.set_error(d.status());
        if self.trust_offered {
            return;
        }
        self.trust_offered = true;
        self.modal = Some(Modal::confirm(
            DubiousOwnership::TITLE,
            d.describe(),
            DubiousOwnership::CONFIRM,
            move |app| {
                app.run_op("Trust folder", true, super::After::Summary, move |r| {
                    r.trust_directory(&d)
                });
            },
        ));
    }
}
