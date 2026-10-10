//! Overlays: input dialogs, confirmations, action menus (the TUI's context menus), the
//! fuzzy picker (command palette, branch switcher), text viewers and askpass prompts.

use std::sync::mpsc::Sender;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::edit::TextEdit;

pub type Action = Box<dyn FnOnce(&mut App)>;
/// Called with the field values and checkbox states of an input dialog.
pub type Submit = Box<dyn FnOnce(&mut App, Vec<String>, Vec<bool>)>;
/// Computes what an input dialog says live about its values and checkbox states
/// ([`Modal::with_live`]).
pub type LiveFn = Box<dyn Fn(&[String], &[bool]) -> Live>;
/// Called with the text of an edit dialog.
pub type SubmitText = Box<dyn FnOnce(&mut App, String)>;

/// What an input dialog says live about its current input: a sentence under the fields,
/// Enter's label, why Enter can't run now (it does nothing then), matching names, values
/// to fill fields with (ctrl-n in the field, like gitti's fill buttons).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Live {
    pub text: String,
    pub ok_label: String,
    pub blocked: Option<String>,
    /// Existing names matching the focused text (listed; gitty has no completion key).
    pub matches: Vec<String>,
    /// Values for field `.0`, listed under it; ctrl-n there puts the next one in.
    pub fills: Vec<(usize, String)>,
    /// What they are ("Remotes"), in front of the list.
    pub fills_label: String,
}

/// A field's chooser ([`Modal::with_pick`]).
pub struct Pick {
    pub field: usize,
    pub hint: String,
    pub f: Submit,
}

/// A dialog's further action ([`Modal::with_extra`]).
pub struct Extra {
    pub key: char,
    pub label: String,
    pub f: Action,
}

/// The key that opens a field's chooser ([`Modal::with_pick`]) and its label.
pub const PICK_KEY: char = 'o';
pub const PICK_KEY_LABEL: &str = "ctrl-o";

/// The key that puts the next of a field's [`Live::fills`] into it (and its label).
pub const FILL_KEY: char = 'n';
pub const FILL_KEY_LABEL: &str = "ctrl-n";

/// The fill after the one field `field` holds now (`text`), or the first.
pub fn next_fill<'a>(fills: &'a [(usize, String)], field: usize, text: &str) -> Option<&'a str> {
    let mine: Vec<&str> = fills.iter().filter(|(f, _)| *f == field).map(|(_, v)| v.as_str()).collect();
    let next = mine.iter().position(|v| *v == text).map_or(0, |i| (i + 1) % mine.len().max(1));
    mine.get(next).copied()
}

pub struct Field {
    pub label: String,
    pub edit: TextEdit,
    pub secret: bool,
    /// The text was put in for the user ([`Modal::prefill`]) and is shown selected: the
    /// first typed character or a paste replaces it, Backspace / Delete clear it, any
    /// other key keeps it as text.
    pub fresh: bool,
    /// A muted line under the field and the value it is about: shown while the field
    /// holds that value ("From the clipboard").
    pub note: Option<(String, String)>,
}

impl Field {
    /// An editing key in the field.
    pub fn key(&mut self, key: KeyEvent) {
        if std::mem::take(&mut self.fresh) {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Char(_) if !ctrl => self.edit.set_text(""),
                KeyCode::Backspace | KeyCode::Delete => {
                    self.edit.set_text("");
                    return;
                }
                _ => {}
            }
        }
        self.edit.handle_key(key);
    }

    /// Pasted text (bracketed paste) at the cursor — instead of a fresh text.
    pub fn paste(&mut self, text: &str) {
        if std::mem::take(&mut self.fresh) {
            self.edit.set_text("");
        }
        self.edit.insert_str(text);
    }

    /// The note under the field now.
    pub fn note_now(&self) -> Option<&str> {
        let (value, text) = self.note.as_ref()?;
        (self.edit.text() == *value).then_some(text.as_str())
    }
}

pub struct Check {
    pub label: String,
    pub on: bool,
}

pub struct MenuItem {
    pub label: String,
    /// Shortcut shown on the right (and accepted while the menu is open).
    pub key: Option<char>,
    /// `key` was given by [`assign_keys`] (the entry had none): it works only inside the
    /// menu, never directly in the pane.
    pub auto_key: bool,
    /// The shared command the key already runs in the pane, when it does the same as
    /// this entry (the key bar shows the key as direct).
    pub same_as: Option<gitti_core::keys::Command>,
    pub action: Option<Action>,
    /// Why it can't act now: drawn greyed with the reason after the label; its
    /// action says the reason in the status line.
    pub disabled: Option<String>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, action: impl FnOnce(&mut App) + 'static) -> Self {
        Self {
            label: label.into(),
            key: None,
            auto_key: false,
            same_as: None,
            action: Some(Box::new(action)),
            disabled: None,
        }
    }

    pub fn key(mut self, key: char) -> Self {
        self.key = Some(key);
        self
    }

    /// Its key and the shared command bound to that key, which does the same.
    pub fn same_as(mut self, key: char, cmd: gitti_core::keys::Command) -> Self {
        self.key = Some(key);
        self.same_as = Some(cmd);
        self
    }

    /// Greyed with reason `why` when set: "Pop Latest Stash (no stashes)"; ⏎ / its
    /// key say why in the status line instead of running it.
    pub fn unless(mut self, why: Option<&str>) -> Self {
        if let Some(why) = why {
            let msg = gitti_core::availability::refusal(&self.label, why);
            self.label = gitti_core::availability::with_reason(&self.label, why);
            self.action = Some(Box::new(move |app: &mut App| app.set_info(msg)));
            self.disabled = Some(why.to_string());
        }
        self
    }

    /// A non-selectable separator line.
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            key: None,
            auto_key: false,
            same_as: None,
            action: None,
            disabled: None,
        }
    }

    /// A non-selectable line of explanation.
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            label: text.into(),
            key: None,
            auto_key: false,
            same_as: None,
            action: None,
            disabled: None,
        }
    }

    /// `text` as explanation lines of at most `width` columns (wrapped at spaces).
    pub fn notes(text: &str, width: usize) -> Vec<Self> {
        let mut out = Vec::new();
        let mut line = String::new();
        for word in text.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                out.push(Self::note(std::mem::take(&mut line)));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            out.push(Self::note(line));
        }
        out
    }
}

/// Gives every selectable entry without a key a free one (`gitti_core::keybar::auto_keys`):
/// the menu and the status line's key bar show the same keys.
pub fn assign_keys(items: &mut [MenuItem]) {
    let entries: Vec<_> = items
        .iter()
        .map(|i| i.action.as_ref().map(|_| (i.key, i.label.as_str())))
        .collect();
    let keys = gitti_core::keybar::auto_keys(&entries);
    for (item, key) in items.iter_mut().zip(keys) {
        if key.is_some() {
            item.key = key;
            item.auto_key = true;
        }
    }
}

pub struct PickItem {
    pub label: String,
    /// What it acts on / does ("a remote", a commit's hash · author), muted, right of the
    /// label and left of the keys column.
    pub hint: String,
    /// Its keys: the right-aligned keys column, never in the label or the hint.
    pub keys: String,
    /// Accent for current items.
    pub strong: bool,
    /// A dot in this commit graph lane colour in front of the label (the graph chooser).
    pub lane: Option<u8>,
    pub action: Option<Action>,
    /// Why its command can't act now: drawn greyed, the reason as its hint; its
    /// action says the reason in the status line.
    pub disabled: Option<String>,
}

impl PickItem {
    pub fn new(
        label: impl Into<String>,
        hint: impl Into<String>,
        action: impl FnOnce(&mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
            keys: String::new(),
            strong: false,
            lane: None,
            action: Some(Box::new(action)),
            disabled: None,
        }
    }

    /// Greyed with reason `why`: the reason as its hint, ⏎ says it in the status
    /// line instead of running the command.
    pub fn unavailable(mut self, why: &str) -> Self {
        let msg = gitti_core::availability::refusal(&self.label, why);
        self.hint = why.to_string();
        self.action = Some(Box::new(move |app: &mut App| app.set_info(msg)));
        self.disabled = Some(why.to_string());
        self
    }

    /// The entry of shared command `c`: its keys from the key table.
    pub fn cmd(
        label: impl Into<String>,
        c: gitti_core::keys::Command,
        action: impl FnOnce(&mut App) + 'static,
    ) -> Self {
        Self::new(label, "", action).keys(gitti_core::palette::command_keys(c))
    }

    /// Sets the keys column (gitty's own keys, which the shared table doesn't have).
    pub fn keys(mut self, keys: impl Into<String>) -> Self {
        self.keys = keys.into();
        self
    }
}

pub enum Modal {
    Input {
        title: String,
        fields: Vec<Field>,
        checks: Vec<Check>,
        /// Index into fields, then checks.
        focus: usize,
        submit: Option<Submit>,
        /// Plain words above the fields: what the dialog does and what follows from it
        /// ([`Modal::with_note`], word-wrapped).
        note: Option<String>,
        /// What the dialog says live under its fields ([`Modal::with_live`]).
        live: Option<LiveFn>,
        /// A field filled from a chooser of its own ([`Modal::with_pick`]).
        pick: Option<Pick>,
        /// A further action beside Enter ([`Modal::with_extra`]), e.g. Delete….
        extra: Option<Extra>,
    },
    Confirm {
        title: String,
        body: String,
        button: String,
        action: Option<Action>,
    },
    Menu {
        title: String,
        items: Vec<MenuItem>,
        cursor: usize,
        /// First item shown (menus taller than the terminal scroll like a list: the
        /// window moves only when the cursor would leave it, or by the wheel).
        offset: usize,
    },
    Picker {
        title: String,
        query: TextEdit,
        items: Vec<PickItem>,
        /// Indices into `items`, best match first.
        filtered: Vec<usize>,
        cursor: usize,
        /// First entry of `filtered` shown (see `Menu::offset`).
        offset: usize,
        /// A line between the query and the entries (what was searched, a hint).
        note: Option<String>,
        /// A search picker (Go to Commit): the app replaces `items` per query instead of
        /// filtering them (`App::start_commit_search`).
        search: bool,
        /// The command palette: its filter knows synonyms
        /// (`palette::synonym_score`), "new" finds the Create… and Add… commands too.
        synonyms: bool,
        /// The graph chooser: the commit row of every item (the commit list marks them,
        /// and the edge to the one under the cursor).
        graph: Vec<usize>,
    },
    Text {
        title: String,
        body: String,
        scroll: usize,
    },
    Ask {
        prompt: String,
        input: TextEdit,
        secret: bool,
        /// The helper waiting for the answer (the dialog closes when it goes away).
        asker: gitti_core::askpass::Asker,
        reply: Option<Sender<Option<String>>>,
    },
    Help {
        scroll: usize,
    },
    /// Multi-line text editor (e.g. a commit message); ctrl-s saves.
    Edit {
        title: String,
        hint: String,
        edit: TextEdit,
        submit: Option<SubmitText>,
    },
}

impl Modal {
    pub fn input(
        title: impl Into<String>,
        fields: Vec<(&str, &str)>,
        submit: impl FnOnce(&mut App, Vec<String>) + 'static,
    ) -> Self {
        Self::input_checks(title, fields, Vec::new(), move |app, values, _| {
            submit(app, values)
        })
    }

    pub fn input_checks(
        title: impl Into<String>,
        fields: Vec<(&str, &str)>,
        checks: Vec<(&str, bool)>,
        submit: impl FnOnce(&mut App, Vec<String>, Vec<bool>) + 'static,
    ) -> Self {
        Modal::Input {
            title: title.into(),
            fields: fields
                .into_iter()
                .map(|(label, value)| Field {
                    label: label.to_string(),
                    edit: TextEdit::single(value),
                    secret: false,
                    fresh: false,
                    note: None,
                })
                .collect(),
            checks: checks
                .into_iter()
                .map(|(label, on)| Check {
                    label: label.to_string(),
                    on,
                })
                .collect(),
            focus: 0,
            submit: Some(Box::new(submit)),
            note: None,
            live: None,
            pick: None,
            extra: None,
        }
    }

    /// A further action of the dialog on ctrl-`key` (`label` beside Enter's in the bottom
    /// line): it closes the dialog and runs `f` (which asks first where it must).
    pub fn with_extra(mut self, key: char, label: impl Into<String>, f: impl FnOnce(&mut App) + 'static) -> Self {
        if let Modal::Input { extra, .. } = &mut self {
            *extra = Some(Extra {
                key,
                label: label.into(),
                f: Box::new(f),
            });
        }
        self
    }

    /// [`PICK_KEY`] in field `field` closes the dialog and runs `f` with its values (it
    /// opens a chooser, which opens the dialog again with the choice); `hint` says what
    /// it chooses, under the field.
    pub fn with_pick(
        mut self,
        field: usize,
        hint: impl Into<String>,
        f: impl FnOnce(&mut App, Vec<String>, Vec<bool>) + 'static,
    ) -> Self {
        if let Modal::Input { pick, .. } = &mut self {
            *pick = Some(Pick {
                field,
                hint: hint.into(),
                f: Box::new(f),
            });
        }
        self
    }

    /// An input dialog that says live what Enter will do with the values entered so far
    /// (`f`, asked on every draw): a sentence under the fields, Enter's label, and why
    /// Enter can't run now — Enter then keeps the dialog open.
    pub fn with_live(mut self, f: impl Fn(&[String], &[bool]) -> Live + 'static) -> Self {
        if let Modal::Input { live, .. } = &mut self {
            *live = Some(Box::new(f));
        }
        self
    }

    /// The live answer of an input dialog for its current values ([`Modal::with_live`]).
    pub fn live_now(&self) -> Option<Live> {
        match self {
            Modal::Input {
                fields,
                checks,
                live: Some(f),
                ..
            } => {
                let values: Vec<String> = fields.iter().map(|f| f.edit.text()).collect();
                let flags: Vec<bool> = checks.iter().map(|c| c.on).collect();
                Some(f(&values, &flags))
            }
            _ => None,
        }
    }

    /// Puts `value` into field `ix` of an input dialog for the user (e.g. from the
    /// clipboard): selected, so typing replaces it, with `note` under it while it is
    /// unchanged.
    pub fn prefill(&mut self, ix: usize, value: &str, note: &str) {
        if let Modal::Input { fields, .. } = self
            && let Some(f) = fields.get_mut(ix)
        {
            f.edit.set_text(value);
            f.fresh = true;
            f.note = Some((value.to_string(), note.to_string()));
        }
    }

    /// An input dialog with `text` above its fields (paragraphs split by blank lines).
    pub fn with_note(mut self, text: impl Into<String>) -> Self {
        if let Modal::Input { note, .. } = &mut self {
            *note = Some(text.into());
        }
        self
    }

    /// `Kind "title"` for the script's `state` line (tests: which window is open).
    pub fn debug_name(&self) -> String {
        let (kind, title) = match self {
            Modal::Input { title, .. } => ("Input", title.as_str()),
            Modal::Confirm { title, .. } => ("Confirm", title.as_str()),
            Modal::Menu { title, .. } => ("Menu", title.as_str()),
            Modal::Picker { title, .. } => ("Picker", title.as_str()),
            Modal::Text { title, .. } => ("Text", title.as_str()),
            Modal::Ask { .. } => ("Ask", ""),
            Modal::Help { .. } => ("Help", ""),
            Modal::Edit { title, .. } => ("Edit", title.as_str()),
        };
        format!("{kind} {title:?}")
    }

    pub fn confirm(
        title: impl Into<String>,
        body: impl Into<String>,
        button: impl Into<String>,
        action: impl FnOnce(&mut App) + 'static,
    ) -> Self {
        Modal::Confirm {
            title: title.into(),
            body: body.into(),
            button: button.into(),
            action: Some(Box::new(action)),
        }
    }

    pub fn menu(title: impl Into<String>, mut items: Vec<MenuItem>) -> Self {
        assign_keys(&mut items);
        let cursor = items.iter().position(|i| i.action.is_some()).unwrap_or(0);
        Modal::Menu {
            title: title.into(),
            items,
            cursor,
            offset: 0,
        }
    }

    pub fn picker(title: impl Into<String>, items: Vec<PickItem>) -> Self {
        let mut m = Modal::Picker {
            title: title.into(),
            query: TextEdit::single(""),
            filtered: Vec::new(),
            items,
            cursor: 0,
            offset: 0,
            note: None,
            search: false,
            synonyms: false,
            graph: Vec::new(),
        };
        m.refilter();
        m
    }

    /// The command palette: a [`Modal::picker`] whose filter knows synonyms.
    pub fn palette(title: impl Into<String>, items: Vec<PickItem>) -> Self {
        let mut m = Self::picker(title, items);
        if let Modal::Picker { synonyms, .. } = &mut m {
            *synonyms = true;
        }
        m
    }

    pub fn edit(
        title: impl Into<String>,
        hint: impl Into<String>,
        text: &str,
        submit: impl FnOnce(&mut App, String) + 'static,
    ) -> Self {
        let mut edit = TextEdit::multi();
        edit.set_text(text);
        Modal::Edit {
            title: title.into(),
            hint: hint.into(),
            edit,
            submit: Some(Box::new(submit)),
        }
    }

    pub fn text(title: impl Into<String>, body: impl Into<String>) -> Self {
        Modal::Text {
            title: title.into(),
            body: body.into(),
            scroll: 0,
        }
    }

    /// Re-runs the picker's fuzzy filter after the query changed.
    pub fn refilter(&mut self) {
        if let Modal::Picker {
            query,
            items,
            filtered,
            cursor,
            offset,
            search: false,
            synonyms,
            ..
        } = self
        {
            let q = query.text();
            let score = |label: &str| match *synonyms {
                true => gitti_core::palette::synonym_score(&q, label, SYNONYM_PENALTY, fuzzy_score),
                false => fuzzy_score(&q, label),
            };
            let mut scored: Vec<(i64, usize)> = items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| score(&it.label).map(|s| (s, i)))
                .collect();
            // Stable: equal scores keep the item order.
            scored.sort_by_key(|&(s, i)| (-s, i));
            *filtered = scored.into_iter().map(|(_, i)| i).collect();
            *cursor = 0;
            *offset = 0;
        }
    }
}

/// What a palette entry found by a synonym (`palette::synonym_score`) scores less than
/// by its own words: a little more than a word start and a run, so "new" lists "New …"
/// before "Create …" / "Add …".
const SYNONYM_PENALTY: i64 = 6;

/// Subsequence match of `query` in `text` (case-insensitive); higher is better.
/// Consecutive characters and word starts score extra.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0i64;
    let mut pos = 0usize;
    let mut prev: Option<usize> = None;
    for qc in query.chars().flat_map(char::to_lowercase) {
        if qc == ' ' {
            continue;
        }
        let found = (pos..t.len()).find(|&i| t[i] == qc)?;
        score += 1;
        if prev.is_some_and(|p| p + 1 == found) {
            score += 5;
        }
        if found == 0 || !t[found - 1].is_alphanumeric() {
            score += 3;
        }
        score -= (found - pos).min(10) as i64 / 3;
        prev = Some(found);
        pos = found + 1;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[gitti_test::test]
    fn fuzzy_prefers_word_starts_and_runs() {
        assert!(fuzzy_score("xyz", "Fetch").is_none());
        let fetch = fuzzy_score("fe", "Fetch").unwrap();
        let other = fuzzy_score("fe", "Unstage File").unwrap();
        assert!(fetch > other);
        assert!(
            fuzzy_score("sb", "Switch Branch…").unwrap()
                > fuzzy_score("sb", "Stash…").unwrap_or(-99)
        );
    }
}
