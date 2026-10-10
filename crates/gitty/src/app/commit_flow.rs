//! Committing (gitti: `app/commit_flow.rs`).
//!
//! - The typed message is a draft kept per worktree in the settings db shared with gitti
//!   (`gitti_core::store::save_draft`): written once typing pauses ([`DRAFT_DELAY`], a
//!   timer thread sends [`Msg::DraftDue`]; the write itself is queued on the store's
//!   writer thread) and when gitty ends, read by a job when it starts, dropped only after
//!   a successful commit / amend (by the commit's job and again as the box empties).
//!   Amend's prefilled HEAD message and the merge message are no drafts.
//! - Commit with nothing staged asks instead of failing (`gitti_core::ops::plan_commit`,
//!   `nothing_staged_prompt`, whose notes say what each choice stages): "Stage All and
//!   Commit" (`add -A`), "Stage Tracked Files and Commit" (`add -u`, when untracked files
//!   would come along) or "Create Empty Commit"; all are in the palette and the working
//!   directory's menu too.
//! - The message git left for the next commit (`MERGE_MSG` / `SQUASH_MSG`) fills an empty
//!   box and goes again with git's file if nobody edited it; the user's own text is never
//!   replaced — a note under the box offers git's message (Use It / Keep Mine: a click
//!   or the palette), as gitti (`gitti_core::ops::{sync_prefill, offers_prepared}`). Keep
//!   Mine is stored with the draft, so the note stays hidden after a restart.
//! - Quitting while an operation runs asks first: Quit When Done / Stop and Quit
//!   (`gitti_core::ops::quit_while_busy_prompt`); quitting at once would cut git off.

use std::time::{Duration, Instant};

use gitti_core::ops::{CommitKind, CommitPlan, PrefillSync};

use super::{After, App, Focus, Msg, spawn};
use crate::edit::TextEdit;
use crate::modal::{MenuItem, Modal, PickItem};

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
                    let out = r.continue_operation()?;
                    gitti_core::store::clear_draft(r.workdir());
                    Ok(out)
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

    /// "Stage All and Commit" / "Stage Tracked Files and Commit" / "Create Empty Commit"
    /// (palette, menu, the nothing-staged question): needs a message like Commit.
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
        self.run_op(label, true, After::ClearMessage, move |r| {
            let out = r.commit_as(&msg, kind)?;
            gitti_core::store::clear_draft(r.workdir());
            Ok(out)
        });
    }

    /// Nothing staged: a menu of what can be done under notes saying what each stages
    /// (one choice: a confirm).
    fn ask_nothing_staged(&mut self) {
        let counts = gitti_core::ops::UnstagedCounts::of(&self.working);
        let (title, detail, choices) = gitti_core::ops::nothing_staged_prompt(&counts);
        self.modal = Some(if let &[(label, kind)] = choices.as_slice() {
            Modal::confirm(title, detail, label, move |app: &mut App| app.commit_kind(kind))
        } else {
            let mut items = notes(&detail);
            items.push(MenuItem::separator());
            items.extend(choices.into_iter().map(|(label, kind)| {
                let key = match kind {
                    CommitKind::StageAll => 'S',
                    CommitKind::StageTracked => 't',
                    _ => 'e',
                };
                MenuItem::new(label, move |app: &mut App| app.commit_kind(kind)).key(key)
            }));
            Modal::menu(title, items)
        });
    }

    /// A commit / amend / merge conclusion succeeded (`After::ClearMessage`): its message
    /// is no draft any more.
    pub(super) fn committed(&mut self) {
        self.draft.edited = None;
        self.prepared_dismissed = None;
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
    /// it replaces a merge message nobody edited. A Keep Mine stored with it hides the
    /// note again.
    pub(super) fn restore_draft(&mut self, draft: Option<gitti_core::store::Draft>) {
        let Some(draft) = draft else { return };
        let current = self.message.text();
        let untouched =
            current.trim().is_empty() || self.merge_prefill.as_deref() == Some(current.as_str());
        if untouched && !self.amend {
            self.message.set_text(&draft.message);
            self.merge_prefill = None;
            if self.prepared_dismissed.is_none() {
                self.prepared_dismissed = draft.kept_prepared;
            }
        }
    }

    // ------------------------------------------------------- git's prepared message

    /// A refresh read the message git left for the next commit: into an empty box (or
    /// over an older prefill nobody edited), out again with git's file if untouched.
    pub(super) fn sync_prepared_message(&mut self, prepared: Option<String>) {
        let sync = gitti_core::ops::sync_prefill(
            prepared.as_deref(),
            self.merge_prefill.as_deref(),
            &self.message.text(),
            self.amend,
        );
        match sync {
            PrefillSync::Keep => {}
            PrefillSync::Fill(m) => {
                self.message.set_text(&m);
                self.merge_prefill = Some(m);
            }
            PrefillSync::Clear => {
                self.message = TextEdit::multi();
                self.merge_prefill = None;
            }
            PrefillSync::Forget => self.merge_prefill = None,
        }
        if prepared.is_none() && self.prepared_dismissed.take().is_some() {
            // git's message went (committed, aborted): Keep Mine has nothing to hide.
            gitti_core::store::save_kept_prepared(self.repo.workdir(), None);
        }
        self.prepared = prepared;
    }

    /// git's prepared message, when it differs from the user's text in the box (the
    /// palette offers it then, the note unless dismissed).
    pub fn prepared_differs(&self) -> Option<&str> {
        let p = self.prepared.as_deref()?;
        (!self.amend && self.message.text().trim() != p).then_some(p)
    }

    /// The note under the message box: shown while git's prepared message waits next to
    /// the user's own text; its text.
    pub fn prepared_note(&self) -> Option<String> {
        let shown = gitti_core::ops::offers_prepared(
            self.prepared.as_deref(),
            self.merge_prefill.as_deref(),
            self.prepared_dismissed.as_deref(),
            &self.message.text(),
            self.amend,
        );
        shown.then(|| gitti_core::ops::prepared_note(self.prepared.as_deref().unwrap_or("")))
    }

    /// Use It: git's prepared message replaces the box's text (and the stored draft).
    pub fn use_prepared_message(&mut self) {
        let Some(m) = self.prepared_differs().map(str::to_string) else { return };
        self.message.set_text(&m);
        self.merge_prefill = Some(m);
        // The draft the box held goes: git's message is no draft.
        self.draft.edited = Some(Instant::now());
        self.save_draft_now();
    }

    /// Keep Mine: the note goes for this prepared message (Commit uses the box's text;
    /// git removes its file with the commit) — stored with the draft, so also after a
    /// restart.
    pub fn keep_my_message(&mut self) {
        self.prepared_dismissed = self.prepared.clone();
        // The box's text (the user's own, else no note) as the draft first — writes are
        // queued in order: the row the choice is kept in.
        self.draft.edited = Some(Instant::now());
        self.save_draft_now();
        let kept = self.prepared_dismissed.as_deref();
        gitti_core::store::save_kept_prepared(self.repo.workdir(), kept);
    }

    /// Palette: Use the Message git Prepared (while it differs from the box's text), Keep
    /// My Commit Message (while the note shows).
    pub(super) fn prepared_palette_items(&self) -> Vec<PickItem> {
        let mut items = Vec::new();
        if let Some(m) = self.prepared_differs() {
            let subject = m.lines().next().unwrap_or_default().to_string();
            items.push(PickItem::new(gitti_core::ops::USE_PREPARED_LABEL, subject, |app: &mut App| {
                app.use_prepared_message()
            }));
        }
        if self.prepared_note().is_some() {
            items.push(PickItem::new(gitti_core::ops::KEEP_MINE_LABEL, "hide the note", |app: &mut App| {
                app.keep_my_message()
            }));
        }
        items
    }

    // ------------------------------------------------------------------ cherry-pick

    /// "Cherry-pick with Options…" (as gitti's dialog): `-x` and / or `--no-commit`, each
    /// combination a menu entry saying what follows.
    pub(super) fn cherry_pick_options(&mut self, rev: String, short: String, merge: bool) {
        use gitti_core::ops::CherryPickOptions;
        let choices = [
            (
                "-x — commit, with \"(cherry picked from commit …)\" in the message",
                'x',
                CherryPickOptions { record_origin: true, no_commit: false },
            ),
            (
                "--no-commit — only apply the changes; the message waits in the commit box",
                'n',
                CherryPickOptions { record_origin: false, no_commit: true },
            ),
            (
                "--no-commit -x — both: the message with its origin line waits in the box",
                'b',
                CherryPickOptions { record_origin: true, no_commit: true },
            ),
        ];
        let items = choices
            .into_iter()
            .map(|(label, key, opts)| {
                let rev = rev.clone();
                MenuItem::new(label, move |app: &mut App| {
                    app.run_op("Cherry-pick", true, After::Nothing, move |r| {
                        r.cherry_pick_with(&rev, merge, opts)
                    })
                })
                .key(key)
            })
            .collect();
        let title = if merge {
            format!("Cherry-pick {short} (a merge: its changes against the first parent)")
        } else {
            format!("Cherry-pick {short}")
        };
        self.modal = Some(Modal::menu(title, items));
    }

    // ------------------------------------------------------------------ quitting

    /// q / ctrl-c / Quit: while an operation runs asks first — wait for it and quit
    /// ("Quit When Done"), stop it and quit ("Stop and Quit"), or Esc. Quitting at once is
    /// not offered: git would lose its output pipes mid-way. `App::handle` quits once
    /// nothing runs; asking again while a quit waits, Esc drops it.
    pub fn request_quit(&mut self) {
        let Some((label, _)) = &self.busy else {
            self.quit = true;
            return;
        };
        let label = label.trim_start_matches("Cancelling: ").to_string();
        self.quit_when_idle = false;
        let (title, detail) = gitti_core::ops::quit_while_busy_prompt(std::slice::from_ref(&label));
        let mut items = notes(&detail);
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new(gitti_core::ops::QUIT_WHEN_DONE, move |app: &mut App| {
                app.quit_when_idle = true;
                // Done while the question was open: quit now.
                app.quit = app.busy.is_none();
                let label = label.trim_end_matches('…');
                app.set_info(format!("Quitting once {label} is done (q again to change)"));
            })
            .key('w'),
        );
        items.push(
            MenuItem::new(gitti_core::ops::STOP_AND_QUIT, |app: &mut App| {
                app.stop_op_for_ask();
                app.deferred.clear();
                app.quit_when_idle = true;
                app.quit = app.busy.is_none();
                app.set_info("Stopping git, then quitting");
            })
            .key('s'),
        );
        self.modal = Some(Modal::menu(title, items));
    }
}

/// Width of the explaining notes in the commit / quit menus.
const NOTE_WIDTH: usize = 64;

/// `text` (paragraphs) as menu notes, a blank line between paragraphs.
fn notes(text: &str) -> Vec<MenuItem> {
    let mut items = Vec::new();
    for (i, para) in text.split("\n\n").enumerate() {
        if i > 0 {
            items.push(MenuItem::separator());
        }
        items.extend(MenuItem::notes(para, NOTE_WIDTH));
    }
    items
}

fn store_draft(app: &App, text: &str) {
    gitti_core::store::save_draft(app.repo.workdir(), text);
}
