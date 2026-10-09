//! Spell checking in the message editors: misspelled words get a red wavy underline, right
//! click on one offers the checker's suggestions, "Learn Spelling" and "Ignore Spelling"
//! above the usual edit entries.
//!
//! Which editors ([`SpellTarget`]): the commit message, the text area of the Edit Commit
//! Message dialog and the message fields of the tag / merge / stash dialogs
//! (`ModalSpec::spell`). One machinery for all of them, per target its own [`SpellState`].
//!
//! - Which words: `gitti_core::spell::words` (prose only, code-like tokens skipped).
//! - Verdicts: `crate::spell::Speller` (OS checker on its own thread, cached per word).
//!   Every text change classifies the words from the cache right away (no flicker, no
//!   wait); the unknown ones go to the checker after a short pause in typing, and the
//!   marks are redone when it answers.
//! - Drawing: gpui-component's inputs have no decoration hook, so a canvas over the input
//!   paints the underlines at the input's own layout of the word (`range_to_bounds`, read
//!   while painting, after the input laid out this frame), clipped to its text area.
//!   The word the cursor sits at the end of (being typed) is not marked yet.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gitti_core::spell::{word_at, words};
use gpui_kit::component::input::{Copy, Cut, InputState, Paste, SelectAll, TextareaState};
use gpui_kit::component::native_menu::NativeMenu;
use gpui_kit::*;

use super::GittiApp;
use crate::spell::{Speller, Verdict};
use crate::theme::{PaletteExt as _, mono_font};

/// Pause in typing before unknown words go to the checker.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// A spell-checked editor.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SpellTarget {
    /// The commit message editor.
    Message,
    /// The open dialog's text area (Edit Commit Message…).
    ModalText,
    /// Field `n` of the open dialog (tag / merge / stash message).
    ModalField(usize),
}

impl SpellTarget {
    /// Probe kind of its marks: `spell-mark:N` for the commit message, `modal-spell-mark:N`
    /// in a dialog.
    fn probe_kind(self) -> &'static str {
        match self {
            SpellTarget::Message => "spell-mark",
            SpellTarget::ModalText | SpellTarget::ModalField(_) => "modal-spell-mark",
        }
    }
}

/// The misspelled words of one editor as of its last change.
#[derive(Default)]
pub(super) struct SpellState {
    /// Byte ranges into `text`.
    marks: Rc<Vec<Range<usize>>>,
    /// The text the marks belong to (not drawn for any other text).
    text: SharedString,
    /// Classified with spell checking on (else `marks` is just empty).
    checked: bool,
    /// Debounced check of the unknown words (dropping it cancels).
    task: Option<Task<()>>,
}

/// Per editor (a dialog's entries are dropped when the next dialog opens).
pub(super) type SpellStates = HashMap<SpellTarget, SpellState>;

/// The editor behind a [`SpellTarget`]: a multi-line text area or a one-line field (the
/// same engine underneath, but two types in gpui-component's API).
#[derive(Clone)]
pub(super) enum SpellEditor {
    Area(Entity<TextareaState>),
    Line(Entity<InputState>),
}

/// Runs `$body` with `$m` = the editor's state, read.
macro_rules! read_editor {
    ($e:expr, $cx:expr, |$m:ident| $body:expr) => {
        match $e {
            SpellEditor::Area(a) => {
                let $m = a.read($cx);
                $body
            }
            SpellEditor::Line(l) => {
                let $m = l.read($cx);
                $body
            }
        }
    };
}

impl SpellEditor {
    fn value(&self, cx: &App) -> SharedString {
        read_editor!(self, cx, |m| m.value())
    }

    fn cursor(&self, cx: &App) -> usize {
        read_editor!(self, cx, |m| m.cursor())
    }

    fn has_selection(&self, cx: &App) -> bool {
        read_editor!(self, cx, |m| !m.selected_range().is_empty())
    }

    pub(super) fn focused(&self, window: &Window, cx: &App) -> bool {
        read_editor!(self, cx, |m| m.focus_handle(cx).is_focused(window))
    }

    /// Where the editor laid out `r`: one bounds per visual line (a word only spans two
    /// when the soft wrap breaks inside it).
    fn word_lines(&self, text: &str, r: &Range<usize>, cx: &App) -> Vec<Bounds<Pixels>> {
        read_editor!(self, cx, |m| {
            let bounds = |r: &Range<usize>| m.range_to_bounds(r);
            let Some(b) = bounds(r) else {
                return Vec::new();
            };
            let line_h = m.line_height().unwrap_or(b.size.height);
            if b.size.height < line_h * 1.5 && b.size.width > px(0.) {
                return vec![b];
            }
            let mut out: Vec<Bounds<Pixels>> = Vec::new();
            for (i, c) in text[r.clone()].char_indices() {
                let at = r.start + i;
                let Some(cb) = bounds(&(at..at + c.len_utf8())) else {
                    continue;
                };
                match out.last_mut() {
                    Some(last) if (last.top() - cb.top()).abs() < px(1.) => {
                        *last = last.union(&cb)
                    }
                    _ => out.push(cb),
                }
            }
            out
        })
    }

    fn line_height(&self, cx: &App) -> Option<Pixels> {
        read_editor!(self, cx, |m| m.line_height())
    }

    /// Replaces `range` by `with` (undoable, like typing).
    fn replace(&self, range: Range<usize>, with: String, window: &mut Window, cx: &mut App) {
        match self {
            SpellEditor::Area(a) => a.update(cx, |m, cx| {
                m.set_selected_range(range, cx);
                m.replace(with, window, cx);
            }),
            SpellEditor::Line(l) => l.update(cx, |m, cx| {
                m.set_selected_range(range, cx);
                m.replace(with, window, cx);
            }),
        }
    }

    fn set_cursor(&self, offset: usize, cx: &mut App) {
        match self {
            SpellEditor::Area(a) => a.update(cx, |m, cx| m.set_selected_range(offset..offset, cx)),
            SpellEditor::Line(l) => l.update(cx, |m, cx| m.set_selected_range(offset..offset, cx)),
        }
    }
}

/// How an editor draws its text, for placing the underlines.
pub(super) struct SpellLook {
    /// The editor's font; `None` = the inherited one (UI font).
    pub font: Option<Font>,
    pub size: Pixels,
    /// Border + padding around the text area (the marks are clipped to it).
    pub inset: Point<Pixels>,
}

impl SpellLook {
    /// A monospaced text area (commit editor, Edit Commit Message dialog).
    pub(super) fn mono(size: Pixels, inset: Point<Pixels>) -> Self {
        Self {
            font: Some(font(mono_font())),
            size,
            inset,
        }
    }
}

/// Replace the misspelled `word` at `range` (as of the menu) of `target` by `with`.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = gitti, no_json)]
pub struct SpellReplace {
    pub target: SpellTarget,
    pub range: Range<usize>,
    pub word: String,
    pub with: String,
}

/// Add `word` to the OS user dictionary.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = gitti, no_json)]
pub struct SpellLearn {
    pub word: String,
}

/// Accept `word` for the rest of the session.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = gitti, no_json)]
pub struct SpellIgnore {
    pub word: String,
}

impl GittiApp {
    pub(super) fn spell_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(Self::on_spell_replace))
            .on_action(cx.listener(|this, a: &SpellLearn, _, cx| {
                Speller::global().learn(&a.word);
                this.update_spell(cx);
            }))
            .on_action(cx.listener(|this, a: &SpellIgnore, _, cx| {
                Speller::global().ignore(&a.word);
                this.update_spell(cx);
            }))
    }

    /// Settings switch / palette: spell checking of the messages on / off.
    pub(super) fn toggle_spell_check(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.spell_check = !s.spell_check);
        self.update_spell(cx);
        self.set_info(if self.settings.spell_check {
            "Spell checking on"
        } else {
            "Spell checking off"
        });
    }

    fn on_spell_replace(&mut self, a: &SpellReplace, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.spell_editor(a.target) else {
            return;
        };
        let still_there = editor
            .value(cx)
            .get(a.range.clone())
            .is_some_and(|w| w == a.word);
        if !still_there {
            return;
        }
        editor.replace(a.range.clone(), a.with.clone(), window, cx);
        self.update_spell_of(a.target, cx);
    }

    /// The editor behind `target`, if it exists and is spell checked.
    pub(super) fn spell_editor(&self, target: SpellTarget) -> Option<SpellEditor> {
        match target {
            SpellTarget::Message => Some(SpellEditor::Area(self.message.clone())),
            _ => self.modal.as_ref()?.spell_editor(target),
        }
    }

    /// The setting or the dictionary changed: every checked editor again.
    pub(super) fn update_spell(&mut self, cx: &mut Context<Self>) {
        let mut targets = vec![SpellTarget::Message];
        if let Some(m) = &self.modal {
            targets.extend(m.spell_targets());
        }
        for t in targets {
            self.classify_spelling(t, true, cx);
        }
    }

    /// `target`'s text changed: marks from the cache now, unknown words to the checker
    /// after [`DEBOUNCE`].
    pub(super) fn update_spell_of(&mut self, target: SpellTarget, cx: &mut Context<Self>) {
        self.classify_spelling(target, true, cx);
    }

    fn classify_spelling(&mut self, target: SpellTarget, request: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.spell_editor(target) else {
            self.spell.remove(&target);
            return;
        };
        let text = editor.value(cx);
        let enabled = self.settings.spell_check;
        let state = self.spell.entry(target).or_default();
        if !enabled || text.trim().is_empty() {
            if !state.marks.is_empty() {
                cx.notify();
            }
            *state = SpellState {
                text,
                checked: enabled,
                ..Default::default()
            };
            return;
        }
        let speller = Speller::global();
        let mut marks = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        for r in words(&text) {
            let w = &text[r.clone()];
            match speller.verdict(w) {
                Some(Verdict::Misspelled(_)) => marks.push(r),
                Some(Verdict::Correct) => {}
                None => {
                    if !unknown.iter().any(|u| u == w) {
                        unknown.push(w.to_string());
                    }
                }
            }
        }
        if *state.marks != marks {
            state.marks = Rc::new(marks);
            cx.notify();
        }
        state.text = text;
        state.checked = true;
        if !request {
            return;
        }
        if unknown.is_empty() {
            state.task = None;
            return;
        }
        state.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            // Closed without an answer when there is no checker: nothing to mark then.
            let _ = Speller::global().check(unknown).recv().await;
            this.update(cx, |this, cx| this.classify_spelling(target, false, cx))
                .ok();
        }));
    }

    /// The marks of `target`'s current text for drawing (`None` = nothing to draw, or the
    /// marks belong to another text — e.g. right after `set_value`; then they are redone).
    pub(super) fn current_spell_marks(
        &self,
        target: SpellTarget,
        cx: &mut Context<Self>,
    ) -> Option<Rc<Vec<Range<usize>>>> {
        if !self.settings.spell_check {
            return None;
        }
        let value = self.spell_editor(target)?.value(cx);
        let state = self.spell.get(&target);
        if state.is_none_or(|s| s.text != value || !s.checked) {
            // Set programmatically (commit, amend, MERGE_MSG, a dialog's start text,
            // script) — no change event came — or switched on in another tab.
            let app = cx.entity().downgrade();
            cx.defer(move |cx| {
                app.update(cx, |this, cx| this.update_spell_of(target, cx))
                    .ok();
            });
            return None;
        }
        state
            .filter(|s| !s.marks.is_empty())
            .map(|s| s.marks.clone())
    }

    /// The underlines of `target` as an absolutely placed canvas over its editor (`None`
    /// when there is nothing to mark). `focused`: the word ending at the cursor waits.
    pub(super) fn spell_marks_layer(
        &self,
        target: SpellTarget,
        look: SpellLook,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let marks = self.current_spell_marks(target, cx)?;
        let editor = self.spell_editor(target)?;
        Some(render_spell_marks(editor, target, marks, look, focused, cx))
    }
}

/// A canvas over an editor painting the wavy underlines of `marks`.
fn render_spell_marks(
    editor: SpellEditor,
    target: SpellTarget,
    marks: Rc<Vec<Range<usize>>>,
    look: SpellLook,
    focused: bool,
    cx: &mut Context<GittiApp>,
) -> AnyElement {
    let color = cx.palette().danger;
    let SpellLook {
        font,
        size: font_size,
        inset,
    } = look;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let clip = Bounds {
                origin: bounds.origin + inset,
                size: size(
                    bounds.size.width - inset.x * 2.,
                    bounds.size.height - inset.y * 2.,
                ),
            };
            let value = editor.value(cx);
            // The word being typed (cursor at its end) waits until the cursor leaves.
            let typing = focused.then(|| editor.cursor(cx));
            let mut lines = Vec::new();
            for (i, r) in marks.iter().enumerate() {
                if typing == Some(r.end) || value.get(r.clone()).is_none() {
                    continue;
                }
                for seg in editor.word_lines(&value, r, cx) {
                    lines.push((i, seg));
                }
            }
            let Some(first) = lines.first() else {
                return;
            };
            let font = font.clone().unwrap_or_else(|| window.text_style().font());
            let line_h = editor.line_height(cx).unwrap_or(first.1.size.height);
            let ts = window.text_system();
            let font_id = ts.resolve_font(&font);
            let dy = underline_y_offset(
                line_h,
                ts.ascent(font_id, font_size),
                ts.descent(font_id, font_size),
            );
            let style = UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: true,
            };
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                for (i, seg) in lines {
                    window.paint_underline(
                        point(seg.left(), seg.top() + dy),
                        seg.size.width,
                        &style,
                    );
                    if crate::probe::enabled() {
                        crate::probe::record(
                            format!("{}:{i}", target.probe_kind()),
                            seg,
                            window.content_mask().bounds,
                        );
                    }
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// The misspelled word at the editor's cursor (a right click puts it there): its range,
/// text and suggestions.
fn misspelled_at_cursor(
    editor: &SpellEditor,
    cx: &App,
) -> Option<(Range<usize>, String, std::sync::Arc<Vec<String>>)> {
    let text = editor.value(cx);
    let r = word_at(&words(&text), editor.cursor(cx))?;
    let word = text.get(r.clone())?.to_string();
    match Speller::global().verdict(&word)? {
        Verdict::Misspelled(suggestions) => Some((r, word, suggestions)),
        Verdict::Correct => None,
    }
}

impl GittiApp {
    /// Script / tests: the words of `target` marked misspelled right now (as drawn:
    /// current text).
    pub fn script_spell_marks(&self, target: SpellTarget, cx: &App) -> Vec<String> {
        let Some(text) = self.spell_editor(target).map(|e| e.value(cx)) else {
            return Vec::new();
        };
        match self.spell.get(&target) {
            Some(s) if self.settings.spell_check && s.text == text => s
                .marks
                .iter()
                .filter_map(|r| text.get(r.clone()).map(str::to_string))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Script / tests: what the right-click menu of `target` offers at the cursor — the
    /// misspelled word and its suggestions.
    pub fn script_spell_at_cursor(
        &self,
        target: SpellTarget,
        cx: &App,
    ) -> Option<(String, Vec<String>)> {
        if !self.settings.spell_check {
            return None;
        }
        misspelled_at_cursor(&self.spell_editor(target)?, cx).map(|(_, w, s)| (w, s.to_vec()))
    }

    /// Script / tests: the menu's suggestion `index` for the word at the cursor, as its
    /// action (dispatched from the focused editor, like the menu does).
    pub fn script_spell_choose(
        &self,
        target: SpellTarget,
        index: usize,
        cx: &App,
    ) -> Option<Box<dyn Action>> {
        let (range, word, suggestions) = misspelled_at_cursor(&self.spell_editor(target)?, cx)?;
        let with = suggestions.get(index)?.clone();
        Some(Box::new(SpellReplace {
            target,
            range,
            word,
            with,
        }))
    }

    /// Script / tests: puts `target`'s cursor at byte `offset`.
    pub fn script_spell_cursor(&mut self, target: SpellTarget, offset: usize, cx: &mut App) {
        if let Some(e) = self.spell_editor(target) {
            e.set_cursor(offset, cx);
        }
    }
}

/// A checked editor's right-click menu: for a misspelled word under the cursor its
/// suggestions, Learn / Ignore; then Cut / Copy / Paste / Select All (the input's own
/// entries, which a custom menu replaces).
pub(super) fn editor_menu(
    menu: NativeMenu,
    target: SpellTarget,
    editor: &SpellEditor,
    spell_check: bool,
    cx: &App,
) -> NativeMenu {
    let has_selection = editor.has_selection(cx);
    let mut menu = menu;
    if spell_check && let Some((r, word, suggestions)) = misspelled_at_cursor(editor, cx) {
        if suggestions.is_empty() {
            menu = menu.menu_with_disabled(
                "No Guesses Found",
                true,
                Box::new(SpellIgnore { word: word.clone() }),
            );
        }
        for s in suggestions.iter() {
            menu = menu.menu(
                s.clone(),
                Box::new(SpellReplace {
                    target,
                    range: r.clone(),
                    word: word.clone(),
                    with: s.clone(),
                }),
            );
        }
        menu = menu
            .separator()
            .menu(
                "Learn Spelling",
                Box::new(SpellLearn { word: word.clone() }),
            )
            .menu("Ignore Spelling", Box::new(SpellIgnore { word }))
            .separator();
    }
    menu.menu_with_disabled("Cut", !has_selection, Box::new(Cut))
        .menu_with_disabled("Copy", !has_selection, Box::new(Copy))
        .menu("Paste", Box::new(Paste))
        .separator()
        .menu("Select All", Box::new(SelectAll))
}
