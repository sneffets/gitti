//! The commit box beyond drafts (`gitti_core::commit_message`, shared with gitty):
//!
//! - `commit.template`: an empty box starts with it, after a draft and the message git
//!   prepared (`commit_flow::sync_box_start`); Commit refuses it unedited and says so
//!   (`gitti_core::commit_message::CommitTemplate::refusal`), its comment lines go when it
//!   is committed edited. A template that can't be read shows a note under the box.
//! - `i18n.commitEncoding`: with a legacy encoding a note under the box says which
//!   characters of the message it lacks and what is written instead
//!   (`gitti_core::commit_encoding`); a name gitti can't write says so before Commit.
//! - Sign Off: the toggle beside Amend, the Commit menu and the palette; `git commit
//!   --signoff` for every commit gitti makes from the box, remembered per repository.
//! - Earlier messages: ↑ in an empty box shows the newest of this session's commits and
//!   this repository's commits by the user, ↑ / ↓ cycle, typing ends it (in the template
//!   ↑ moves the cursor); "Recent Commit Messages…" (menu, palette) lists them. Text the
//!   user typed is replaced only after asking.
//! - Add Co-Author… (Commit menu, palette): a picker over the repository's authors
//!   (`gitti_core::co_author::co_authors`, in-process from the loaded history), the chosen
//!   one goes into the message's trailer block as `Co-authored-by: Name <mail>`, never
//!   twice.

use gitti_core::co_author::{self, CoAuthor};
use gitti_core::commit_message::{
    self as cm, CommitOptions, MessageRecall, RecallStep, RECENT_LABEL, RECENT_LIMIT,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::*;

use super::GittiApp;
use super::picker::{PickerItem, item};
use crate::probe::ProbeExt as _;
use crate::theme::PaletteExt as _;
use crate::{AddCoAuthor, MessageHistoryNext, MessageHistoryPrev, RecentMessages, ToggleSignOff};

/// The commit box's state beyond the message itself.
#[derive(Default)]
pub(super) struct CommitBox {
    /// `commit.template` as of the last refresh.
    pub template: Option<cm::CommitTemplate>,
    /// `i18n.commitEncoding` as of the last refresh: the note under the box says what a
    /// legacy encoding lacks of the message.
    pub encoding: gitti_core::commit_encoding::CommitEncoding,
    /// Sign Off (per repository, `cm::load_sign_off`).
    pub sign_off: bool,
    /// Messages committed in this session, oldest first.
    session: Vec<String>,
    /// The user's own messages and the HEAD they were read at.
    own: Option<(Option<String>, Vec<String>)>,
    /// A read of them runs; ↑ waits for it (`recall_waiting`).
    loading: bool,
    recall_waiting: bool,
    recall: MessageRecall,
    /// The box's text started as the commit template (`note_box_text`): committed, its
    /// comment lines go as after git's editor.
    from_template: bool,
}

impl GittiApp {
    /// What Commit adds besides the message.
    pub(super) fn commit_options(&self) -> CommitOptions {
        CommitOptions {
            sign_off: self.commit_box.sign_off,
            from_template: self.commit_box.from_template && !self.amend,
        }
    }

    /// Why Commit refuses the box's text before running git: the commit template unedited
    /// (or only its comments) — as `git commit` after its editor.
    pub(super) fn template_refusal(&self, message: &str) -> Option<String> {
        if message.trim().is_empty() || self.amend {
            return None;
        }
        let from = self.commit_box.from_template;
        self.commit_box.template.as_ref()?.refusal(message, from)
    }

    /// The box was set to `text` by gitti (not typed): whether that is the template.
    pub(super) fn note_box_text(&mut self, text: &str) {
        self.commit_box.from_template = !text.trim().is_empty()
            && self.template_start().is_some_and(|t| t.trim_end() == text.trim_end());
    }

    /// What an empty box starts with besides git's prepared message: the template.
    pub(super) fn template_start(&self) -> Option<String> {
        self.commit_box.template.as_ref()?.start_text()
    }

    /// The box is blank or holds the commit template as it started: with nothing staged
    /// the Commit button is off (Commit would refuse the template anyway).
    pub(super) fn message_counts_as_blank(&self, cx: &App) -> bool {
        let current = self.message.read(cx).value();
        current.trim().is_empty()
            || (!self.amend
                && self.template_start().is_some_and(|t| t.trim_end() == current.trim_end()))
    }

    /// A commit with `message` succeeded: it joins this session's earlier messages.
    pub(super) fn note_committed_message(&mut self, message: &str) {
        cm::note_session_message(&mut self.commit_box.session, message);
        self.commit_box.recall.reset();
        self.commit_box.from_template = false;
    }

    // ------------------------------------------------------------------ Sign Off

    pub(super) fn toggle_sign_off(&mut self, cx: &mut Context<Self>) {
        let on = !self.commit_box.sign_off;
        self.commit_box.sign_off = on;
        if let Some(repo) = &self.repo {
            cm::save_sign_off(repo.workdir(), on);
        }
        self.set_info(if on {
            "Sign Off on: commits get \"Signed-off-by: <user.name> <user.email>\""
        } else {
            "Sign Off off"
        });
        cx.notify();
    }

    /// The toggle beside Amend.
    pub(super) fn render_sign_off_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.commit_box.sign_off;
        Button::new("sign-off")
            .probe("commit-sign-off")
            .label(if on { "☑ Sign off" } else { "☐ Sign off" })
            .small()
            .ghost()
            .tooltip(cm::SIGN_OFF_TIP)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_sign_off(cx)))
    }

    /// The note under the box while `commit.template` names a file that can't be read.
    pub(super) fn render_template_problem(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let problem = self.commit_box.template.as_ref()?.problem()?;
        let p = cx.palette().clone();
        let full = problem.clone();
        Some(
            h_flex()
                .id("template-problem")
                .probe("template-problem")
                .min_w_0()
                .px_1()
                .text_size(self.settings.ui_small())
                .text_color(p.warning)
                .child(div().min_w_0().truncate().child(problem))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                })
                .into_any_element(),
        )
    }

    /// The note under the box while `i18n.commitEncoding` lacks characters of the message
    /// (what is written instead) or names an encoding gitti can't write.
    pub(super) fn render_encoding_note(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let note = self.encoding_note(cx)?;
        let p = cx.palette().clone();
        let full = note.clone();
        Some(
            h_flex()
                .id("encoding-note")
                .probe("encoding-note")
                .min_w_0()
                .px_1()
                .text_size(self.settings.ui_small())
                .text_color(p.warning)
                .child(div().min_w_0().truncate().child(note))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                })
                .into_any_element(),
        )
    }

    /// The encoding note for the box's text ([`Self::render_encoding_note`]).
    pub(super) fn encoding_note(&self, cx: &App) -> Option<String> {
        let text = self.message.read(cx).value();
        self.commit_box.encoding.note(&text)
    }

    /// What a dialog that takes a commit message (reword, merge) adds to its text about
    /// `i18n.commitEncoding`.
    pub(super) fn with_encoding_line(&self, text: Option<String>) -> Option<String> {
        match (text, self.commit_box.encoding.dialog_line()) {
            (Some(t), Some(l)) => Some(format!("{t}\n{l}")),
            (t, l) => t.or(l),
        }
    }

    // ------------------------------------------------------------------ earlier messages

    /// The earlier messages, if read for the current HEAD.
    fn recent_list(&self) -> Option<Vec<String>> {
        let head = self.head.id.map(|id| id.to_string());
        let (at, own) = self.commit_box.own.as_ref()?;
        (*at == head).then(|| cm::recent_messages(&self.commit_box.session, own))
    }

    /// Reads the user's own messages in the background, then runs `then`.
    fn load_own_messages(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(repo) = self.repo.clone() else { return };
        self.commit_box.loading = true;
        let (name, email) = self.identity.clone();
        let head = self.head.id.map(|id| id.to_string());
        let task = cx.background_executor().spawn(async move {
            repo.own_messages(name.as_deref(), email.as_deref(), RECENT_LIMIT)
        });
        cx.spawn_in(window, async move |this, cx| {
            let own = task.await;
            this.update_in(cx, |this, window, cx| {
                this.commit_box.loading = false;
                this.commit_box.own = Some((head, own));
                then(this, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// ↑ / ↓ in the message box: an earlier message ([`MessageRecall`]); `false` = not a
    /// recall, the editor moves the cursor.
    fn recall_key(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.amend || self.repo.is_none() {
            return false;
        }
        let current = self.message.read(cx).value().to_string();
        let Some(list) = self.recent_list() else {
            if !up || !self.commit_box.recall.would_start(&current) {
                return false;
            }
            self.commit_box.recall_waiting = true;
            if !self.commit_box.loading {
                self.load_own_messages(window, cx, |this, window, cx| {
                    if std::mem::take(&mut this.commit_box.recall_waiting) {
                        this.recall_key(true, window, cx);
                    }
                });
            }
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
                self.message.update(cx, |m, cx| m.set_value(text, window, cx));
                cx.notify();
                true
            }
        }
    }

    /// "Recent Commit Messages…": the template and the earlier messages; the chosen one
    /// goes into the box (asking first over text the user typed).
    pub(super) fn open_recent_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        let Some(list) = self.recent_list() else {
            if !self.commit_box.loading {
                self.load_own_messages(window, cx, |this, w, cx| this.open_recent_messages(w, cx));
            }
            return;
        };
        let mut items: Vec<PickerItem> = Vec::new();
        if let Some(t) = self.template_start() {
            let path = self.commit_box.template.as_ref().map(|t| t.configured.clone());
            let detail = format!("commit template ({})", path.unwrap_or_default());
            items.push(item("Start from the Commit Template", detail, move |this, w, cx| {
                this.use_earlier_message(t.clone(), w, cx)
            }));
        }
        for m in list {
            let (subject, detail) = cm::message_entry(&m);
            items.push(item(subject, detail, move |this, w, cx| {
                this.use_earlier_message(m.clone(), w, cx)
            }));
        }
        if items.is_empty() {
            self.set_info(
                "No earlier commit messages: none committed in this session, none by you \
                 (user.email) in this repository's newest commits",
            );
            cx.notify();
            return;
        }
        self.open_picker("Earlier commit message…", items, window, cx);
    }

    /// Puts `text` into the box; over the user's own text only after asking.
    fn use_earlier_message(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.message.read(cx).value().to_string();
        let untouched = current.trim().is_empty()
            || self.merge_prefill.as_deref() == Some(current.as_str())
            || current.trim_end() == text.trim_end();
        if self.amend {
            self.set_error("Amend is on: the box holds the message of the commit you amend".into());
            cx.notify();
            return;
        }
        if untouched {
            self.put_earlier_message(text, window, cx);
            return;
        }
        let (title, detail) = cm::replace_message_prompt(&text);
        let answer = window.prompt(PromptLevel::Warning, &title, Some(&detail), &["Replace", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, w, cx| this.put_earlier_message(text, w, cx)).ok();
            }
        })
        .detach();
    }

    fn put_earlier_message(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.note_box_text(&text);
        self.message.update(cx, |m, cx| {
            m.set_value(text, window, cx);
            m.focus(window, cx);
        });
        self.commit_box.recall.reset();
        // Chosen: the user's message now, kept as the draft.
        self.draft_dirty = true;
        self.save_draft_now(cx);
        cx.notify();
    }

    // ------------------------------------------------------------------ co-authors

    /// "Add Co-Author…": the repository's authors (read in the background, in-process),
    /// then a picker; the chosen one's trailer goes into the message.
    pub(super) fn open_co_author_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        let history = self.history.clone();
        let own = self.identity.1.clone();
        let task = cx
            .background_executor()
            .spawn(async move { co_author::co_authors(&history, own.as_deref()) });
        cx.spawn_in(window, async move |this, cx| {
            let list = task.await;
            this.update_in(cx, |this, w, cx| this.show_co_author_picker(list, w, cx)).ok();
        })
        .detach();
    }

    fn show_co_author_picker(&mut self, list: Vec<CoAuthor>, window: &mut Window, cx: &mut Context<Self>) {
        if list.is_empty() {
            self.set_info(
                "No co-authors to offer: no one but you wrote this repository's newest commits",
            );
            cx.notify();
            return;
        }
        let items = list
            .into_iter()
            .map(|c| {
                let ident = c.ident();
                item(ident.clone(), c.detail(), move |this, w, cx| {
                    this.add_co_author(&ident, w, cx)
                })
            })
            .collect();
        self.open_picker("Add co-author (Co-authored-by trailer)…", items, window, cx);
    }

    /// Adds `Co-authored-by: <ident>` to the message's trailers (not twice).
    fn add_co_author(&mut self, ident: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.message.read(cx).value().to_string();
        match co_author::add_co_author(&current, ident) {
            None => self.set_info(format!("{ident} is a co-author already")),
            Some(text) => {
                self.message.update(cx, |m, cx| {
                    m.set_value(text, window, cx);
                    m.focus(window, cx);
                });
                self.draft_dirty = true;
                self.save_draft_now(cx);
                self.set_info(format!("Added \"{}: {ident}\"", co_author::TRAILER));
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------------ wiring

    pub(super) fn commit_box_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &ToggleSignOff, _, cx| this.toggle_sign_off(cx)))
            .on_action(cx.listener(|this, _: &RecentMessages, w, cx| this.open_recent_messages(w, cx)))
            .on_action(cx.listener(|this, _: &AddCoAuthor, w, cx| this.open_co_author_picker(w, cx)))
            .on_action(cx.listener(|this, _: &MessageHistoryPrev, w, cx| {
                if !this.recall_key(true, w, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &MessageHistoryNext, w, cx| {
                if !this.recall_key(false, w, cx) {
                    cx.propagate();
                }
            }))
    }

    pub(super) fn commit_box_palette_items(&self) -> Vec<PickerItem> {
        vec![
            item(
                cm::sign_off_palette_label(self.commit_box.sign_off),
                "add Signed-off-by: you to each commit (git commit --signoff)",
                |t, _, cx| t.toggle_sign_off(cx),
            )
            .act(&ToggleSignOff),
            item(RECENT_LABEL, "↑ in the empty message box", |t, w, cx| {
                t.open_recent_messages(w, cx)
            })
            .act(&RecentMessages),
            item(co_author::LABEL, co_author::DETAIL, |t, w, cx| t.open_co_author_picker(w, cx))
                .act(&AddCoAuthor),
        ]
    }

    /// Sign Off as stored for this repository (read with the draft, in the background).
    pub(super) fn set_sign_off_loaded(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.commit_box.sign_off != on {
            self.commit_box.sign_off = on;
            cx.notify();
        }
    }

    /// Sign Off (tests).
    pub fn script_sign_off(&self) -> bool {
        self.commit_box.sign_off
    }
}
