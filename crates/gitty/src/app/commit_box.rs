//! The commit box beyond drafts (gitti: `app/commit_box.rs`; shared logic in
//! `gitti_core::commit_message`):
//!
//! - `commit.template`: an empty box starts with it, after a draft and the message git
//!   prepared (`commit_flow::sync_box_start`); Commit refuses it unedited and says so,
//!   its comment lines go when it is committed edited. A template that can't be read
//!   shows a note under the box.
//! - `i18n.commitEncoding`: with a legacy encoding the note row says which characters of
//!   the message it lacks and what is written instead (`gitti_core::commit_encoding`).
//! - Sign Off: "☐ sign off" beside amend (a click), the working directory's menu and the
//!   palette; `git commit --signoff` for every commit made from the box, remembered per
//!   repository.
//! - Earlier messages: ↑ in an empty box shows the newest of this session's commits and
//!   this repository's commits by the user, ↑ / ↓ cycle, typing ends it (in the template
//!   ↑ moves the cursor); "Recent Commit Messages…" (palette, menu) lists them. Text the
//!   user typed is replaced only after asking.

use gitti_core::commit_message::{
    self as cm, CommitOptions, MessageRecall, RecallStep, RECENT_LABEL, RECENT_LIMIT,
};

use super::{App, Msg, spawn};
use crate::modal::{Modal, PickItem};

/// The commit box's state beyond the message itself.
#[derive(Default)]
pub struct CommitBox {
    /// `commit.template` as of the last refresh.
    pub template: Option<cm::CommitTemplate>,
    /// `i18n.commitEncoding` as of the last refresh: the note under the box says what a
    /// legacy encoding lacks of the message.
    pub encoding: gitti_core::commit_encoding::CommitEncoding,
    /// Sign Off (per repository, `cm::load_sign_off`).
    pub sign_off: bool,
    /// Messages committed in this session, oldest first.
    session: Vec<String>,
    /// The message of the commit running now: joins `session` once it succeeded —
    /// `None` inside: HEAD's message then (as committed: cleaned up, signed off).
    pub(super) committing: Option<Option<String>>,
    /// The user's own messages and the HEAD they were read at.
    own: Option<(Option<String>, Vec<String>)>,
    loading: bool,
    recall: MessageRecall,
    /// The box's text started as the commit template (`note_box_text`): committed, its
    /// comment lines go as after git's editor.
    from_template: bool,
}

/// What waits for the user's own messages ([`Msg::OwnMessages`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnFor {
    Recall,
    Picker,
}

impl App {
    /// What Commit adds besides the message.
    pub(super) fn commit_options(&self) -> CommitOptions {
        CommitOptions {
            sign_off: self.commit_box.sign_off,
            from_template: self.commit_box.from_template && !self.amend,
        }
    }

    /// The box was set to `text` by gitty (not typed): whether that is the template.
    pub(super) fn note_box_text(&mut self, text: &str) {
        self.commit_box.from_template = !text.trim().is_empty()
            && self.template_start().is_some_and(|t| t.trim_end() == text.trim_end());
    }

    /// Why Commit refuses the box's text before running git: the commit template
    /// unedited (or only its comments) — as `git commit` after its editor.
    pub(super) fn template_refusal(&self, message: &str) -> Option<String> {
        if message.trim().is_empty() || self.amend {
            return None;
        }
        let from = self.commit_box.from_template;
        self.commit_box.template.as_ref()?.refusal(message, from)
    }

    /// What an empty box starts with besides git's prepared message: the template.
    pub(super) fn template_start(&self) -> Option<String> {
        self.commit_box.template.as_ref()?.start_text()
    }

    /// The box holds the commit template as it started (the Commit hint is greyed).
    pub fn message_is_unedited_template(&self) -> bool {
        !self.amend
            && self
                .template_start()
                .is_some_and(|t| t.trim_end() == self.message.text().trim_end())
    }

    /// The note under the box while `i18n.commitEncoding` lacks characters of the message
    /// (what is written instead) or names an encoding gitty can't write.
    pub fn encoding_note(&self) -> Option<String> {
        self.commit_box.encoding.note(&self.message.text())
    }

    /// The note under the box while `commit.template` can't be read.
    pub fn template_problem(&self) -> Option<String> {
        self.commit_box.template.as_ref()?.problem()
    }

    /// The commit with the box's message succeeded (`committed`): it joins this session's
    /// earlier messages.
    pub(super) fn note_committed_message(&mut self) {
        match self.commit_box.committing.take() {
            Some(Some(m)) => cm::note_session_message(&mut self.commit_box.session, &m),
            Some(None) => {
                if let Ok(m) = self.repo.head_message() {
                    cm::note_session_message(&mut self.commit_box.session, &m);
                }
            }
            None => {}
        }
        self.commit_box.recall.reset();
        self.commit_box.from_template = false;
    }

    // ------------------------------------------------------------------ Sign Off

    pub fn toggle_sign_off(&mut self) {
        let on = !self.commit_box.sign_off;
        self.commit_box.sign_off = on;
        cm::save_sign_off(self.repo.workdir(), on);
        self.set_info(if on {
            "Sign Off on: commits get \"Signed-off-by: <user.name> <user.email>\""
        } else {
            "Sign Off off"
        });
    }

    // ------------------------------------------------------------------ earlier messages

    /// The earlier messages, if read for the current HEAD.
    fn recent_list(&self) -> Option<Vec<String>> {
        let head = self.head.id.map(|id| id.to_string());
        let (at, own) = self.commit_box.own.as_ref()?;
        (*at == head).then(|| cm::recent_messages(&self.commit_box.session, own))
    }

    /// Reads the user's own messages in a job ([`Msg::OwnMessages`]).
    fn load_own_messages(&mut self, then: OwnFor) {
        if self.commit_box.loading {
            return;
        }
        self.commit_box.loading = true;
        let repo = self.repo.clone();
        let (name, email) = self.identity.clone();
        let head = self.head.id.map(|id| id.to_string());
        spawn(
            &self.tx,
            move || repo.own_messages(name.as_deref(), email.as_deref(), RECENT_LIMIT),
            move |own| Msg::OwnMessages(head, own, then),
        );
    }

    /// [`Msg::OwnMessages`]: the list is there; ↑ or the picker goes on.
    pub(super) fn own_messages_read(&mut self, head: Option<String>, own: Vec<String>, then: OwnFor) {
        self.commit_box.loading = false;
        self.commit_box.own = Some((head, own));
        match then {
            OwnFor::Recall => {
                self.recall_key(true);
            }
            OwnFor::Picker => self.open_recent_messages(),
        }
    }

    /// ↑ / ↓ in the message box: an earlier message ([`MessageRecall`]); `false` = not a
    /// recall, the editor moves the cursor.
    pub fn recall_key(&mut self, up: bool) -> bool {
        if self.amend {
            return false;
        }
        let current = self.message.text();
        let Some(list) = self.recent_list() else {
            if !up || !self.commit_box.recall.would_start(&current) {
                return false;
            }
            self.load_own_messages(OwnFor::Recall);
            return true;
        };
        let step = if up {
            self.commit_box.recall.up(&list, &current)
        } else {
            self.commit_box.recall.down(&list, &current)
        };
        match step {
            RecallStep::Pass => false,
            RecallStep::Stay => true,
            RecallStep::Set(text) => {
                self.note_box_text(&text);
                self.message.set_text(&text);
                true
            }
        }
    }

    /// "Recent Commit Messages…": the template and the earlier messages; the chosen one
    /// goes into the box (asking first over text the user typed).
    pub fn open_recent_messages(&mut self) {
        let Some(list) = self.recent_list() else {
            self.load_own_messages(OwnFor::Picker);
            return;
        };
        let mut items = Vec::new();
        if let Some(t) = self.template_start() {
            let path = self.commit_box.template.as_ref().map(|t| t.configured.clone());
            let detail = format!("commit template ({})", path.unwrap_or_default());
            items.push(PickItem::new("Start from the Commit Template", detail, move |app: &mut App| {
                app.use_earlier_message(t)
            }));
        }
        for m in list {
            let (subject, detail) = cm::message_entry(&m);
            items.push(PickItem::new(subject, detail, move |app: &mut App| {
                app.use_earlier_message(m)
            }));
        }
        if items.is_empty() {
            self.set_info(
                "No earlier commit messages: none committed in this session, none by you \
                 (user.email) in this repository's newest commits",
            );
            return;
        }
        self.modal = Some(Modal::picker("Earlier commit message", items));
    }

    /// Puts `text` into the box; over the user's own text only after asking.
    fn use_earlier_message(&mut self, text: String) {
        if self.amend {
            self.set_error("Amend is on: the box holds the message of the commit you amend".into());
            return;
        }
        let current = self.message.text();
        let untouched = current.trim().is_empty()
            || self.merge_prefill.as_deref() == Some(current.as_str())
            || current.trim_end() == text.trim_end();
        if untouched {
            self.put_earlier_message(text);
            return;
        }
        let (title, detail) = cm::replace_message_prompt(&text);
        self.modal = Some(Modal::confirm(title, detail, "Replace", move |app: &mut App| {
            app.put_earlier_message(text)
        }));
    }

    fn put_earlier_message(&mut self, text: String) {
        self.note_box_text(&text);
        self.message.set_text(&text);
        self.commit_box.recall.reset();
        self.focus = super::Focus::Message;
        // Chosen: the user's message now, kept as the draft.
        self.message_edited();
    }

    /// Palette entries (as gitti's).
    pub(super) fn commit_box_palette_items(&self) -> Vec<PickItem> {
        vec![
            PickItem::new(
                cm::sign_off_palette_label(self.commit_box.sign_off),
                "add Signed-off-by: you to each commit (git commit --signoff)",
                |app: &mut App| app.toggle_sign_off(),
            ),
            PickItem::new(RECENT_LABEL, "↑ in the empty message box", |app: &mut App| {
                app.open_recent_messages()
            }),
        ]
    }
}
