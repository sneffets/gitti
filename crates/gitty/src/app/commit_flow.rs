//! Committing (gitti: `app/commit_flow.rs`).
//!
//! - The typed message is a draft kept per worktree in the settings db shared with gitti
//!   (`gitti_core::store::save_draft`): written once typing pauses ([`DRAFT_DELAY`], a
//!   timer thread sends [`Msg::DraftDue`]; the write itself is queued on the store's
//!   writer thread) and when gitty ends, read by a job when it starts, dropped only after
//!   a successful commit / amend. Amend's prefilled HEAD message and the merge message are
//!   no drafts.
//! - Commit with nothing staged asks instead of failing (`gitti_core::ops::plan_commit`):
//!   "Stage All and Commit" (when there are changes) or "Create Empty Commit"; both are in
//!   the palette and the working directory's menu too.
//! - Quitting while an operation runs asks first.

use std::time::{Duration, Instant};

use gitti_core::ops::{CommitKind, CommitPlan};

use super::{After, App, Focus, Msg, spawn};
use crate::modal::{MenuItem, Modal};

/// Typing pauses this long before the draft is written.
const DRAFT_DELAY: Duration = Duration::from_millis(500);

/// The draft's pending write: when the message was last typed in, and whether a
/// [`Msg::DraftDue`] timer is out.
#[derive(Default)]
pub struct DraftState {
    edited: Option<Instant>,
    timer: bool,
}

impl App {
    pub fn commit(&mut self) {
        let msg = self.message.text();
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
                self.focus = Focus::Detail;
                self.merge_prefill = None;
                self.run_op("Commit", true, After::ClearMessage, |r| {
                    r.continue_operation()
                });
            }
            CommitPlan::NoMessage => {
                self.set_error("Enter a commit message first (c)".into());
                self.focus = Focus::Message;
            }
            CommitPlan::NothingStaged { .. } => self.ask_nothing_staged(),
            CommitPlan::Run(kind) => self.run_commit(kind),
        }
    }

    /// "Stage All and Commit" / "Create Empty Commit" (palette, menu, the nothing-staged
    /// question): needs a message like Commit.
    pub fn commit_kind(&mut self, kind: CommitKind) {
        if let Some(why) =
            gitti_core::ops::commit_blocker(self.repo_state, self.working.conflicted.len())
        {
            self.set_error(why);
        } else if kind == CommitKind::Empty
            && let Some(why) =
                gitti_core::ops::empty_commit_blocker(self.working.staged.len(), self.amend)
        {
            self.set_error(why);
        } else if self.message.text().trim().is_empty() {
            self.set_error("Enter a commit message first (c)".into());
            self.focus = Focus::Message;
        } else {
            self.run_commit(kind);
        }
    }

    fn run_commit(&mut self, kind: CommitKind) {
        let msg = self.message.text();
        self.focus = Focus::Detail;
        let label = if kind == CommitKind::Amend { "Amend" } else { "Commit" };
        self.run_op(label, true, After::ClearMessage, move |r| r.commit_as(&msg, kind));
    }

    /// Nothing staged: a menu of what can be done (one choice: a confirm).
    fn ask_nothing_staged(&mut self) {
        let (title, detail, choices) =
            gitti_core::ops::nothing_staged_prompt(self.working.unstaged.len());
        self.modal = Some(if let &[(label, kind)] = choices.as_slice() {
            Modal::confirm(title, detail, label, move |app: &mut App| app.commit_kind(kind))
        } else {
            let items = choices
                .into_iter()
                .map(|(label, kind)| {
                    let key = if kind == CommitKind::StageAll { 'S' } else { 'e' };
                    MenuItem::new(label, move |app: &mut App| app.commit_kind(kind)).key(key)
                })
                .collect();
            Modal::menu(title, items)
        });
    }

    /// A commit / amend / merge conclusion succeeded (`After::ClearMessage`): its message
    /// is no draft any more.
    pub(super) fn committed(&mut self) {
        self.draft.edited = None;
        store_draft(self, "");
    }

    // ------------------------------------------------------------------ drafts

    /// The message box was typed in: the draft is written once typing pauses. While
    /// amending the box holds HEAD's message, no draft.
    pub fn message_edited(&mut self) {
        if self.amend {
            return;
        }
        self.draft.edited = Some(Instant::now());
        if !std::mem::replace(&mut self.draft.timer, true) {
            spawn(&self.tx, || std::thread::sleep(DRAFT_DELAY), |_| Msg::DraftDue);
        }
    }

    /// [`Msg::DraftDue`]: writes the draft once the typing paused long enough, else waits
    /// the rest.
    pub(super) fn draft_due(&mut self) {
        self.draft.timer = false;
        let Some(at) = self.draft.edited else { return };
        let since = at.elapsed();
        if since >= DRAFT_DELAY {
            self.save_draft_now();
        } else {
            self.draft.timer = true;
            spawn(&self.tx, move || std::thread::sleep(DRAFT_DELAY - since), |_| Msg::DraftDue);
        }
    }

    /// Writes the draft if it changed since the last write (queued, returns at once):
    /// debounced, before amend fills in HEAD's message, and when gitty ends.
    pub fn save_draft_now(&mut self) {
        if self.draft.edited.take().is_none() || self.amend {
            return;
        }
        let text = self.message.text();
        // The merge message git prepared is git's, not the user's.
        let text = if self.merge_prefill.as_deref() == Some(text.as_str()) {
            String::new()
        } else {
            text
        };
        store_draft(self, &text);
    }

    /// Reads the draft in the background when gitty starts (`Msg::Draft`).
    pub(super) fn load_draft(&self) {
        let workdir = self.repo.workdir().to_path_buf();
        spawn(
            &self.tx,
            move || gitti_core::store::load_draft(&workdir),
            Msg::Draft,
        );
    }

    /// The draft read at start goes into the message box unless the user typed meanwhile;
    /// it replaces a merge message nobody edited.
    pub(super) fn restore_draft(&mut self, draft: Option<String>) {
        let Some(draft) = draft else { return };
        let current = self.message.text();
        let untouched =
            current.trim().is_empty() || self.merge_prefill.as_deref() == Some(current.as_str());
        if untouched && !self.amend {
            self.message.set_text(&draft);
            self.merge_prefill = None;
        }
    }

    // ------------------------------------------------------------------ quitting

    /// q / ctrl-c / Quit: asks first while an operation runs (it may be left unfinished).
    pub fn request_quit(&mut self) {
        let Some((label, _)) = &self.busy else {
            self.quit = true;
            return;
        };
        let (title, detail) = gitti_core::ops::quit_while_busy_prompt(std::slice::from_ref(label));
        self.modal = Some(Modal::confirm(title, detail, "Quit Anyway", |app: &mut App| {
            app.quit = true
        }));
    }
}

fn store_draft(app: &App, text: &str) {
    gitti_core::store::save_draft(app.repo.workdir(), text);
}
