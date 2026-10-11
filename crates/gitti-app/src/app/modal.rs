//! In-window modal dialogs: input dialogs for branch/tag/remote operations, a read-only text
//! viewer for long errors, and credential prompts forwarded by the askpass helper.

use gitti_core::askpass::PromptKind;
use gitti_core::keys::Nav;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use super::spell_ui::{SpellEditor, SpellLook, SpellTarget, editor_context_menu};
use crate::probe::ProbeExt as _;
use crate::theme::PaletteExt as _;

/// How often an open credential dialog checks whether its helper is still waiting.
const ASK_POLL: std::time::Duration = std::time::Duration::from_millis(150);

/// A prompt from git/ssh; the answer (None = cancel) goes back through `reply`.
pub struct AskRequest {
    pub prompt: gitti_core::askpass::Prompt,
    pub reply: std::sync::mpsc::Sender<Option<String>>,
}

pub struct ModalResult {
    pub values: Vec<String>,
    pub checks: Vec<bool>,
    /// The chosen option of [`ModalSpec::options`] (0 without options).
    pub option: usize,
    /// Content of the text area, if the modal has one.
    pub text: Option<String>,
}

/// What a dialog says live about its current input (`ModalSpec::live`): a sentence under
/// the fields, the OK button's label, why OK can't run now, and completion buttons.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModalLive {
    pub text: String,
    /// The OK button's label now (`None`: the spec's).
    pub ok_label: Option<String>,
    /// Why OK can't run now: the button is disabled and the reason shown.
    pub blocked: Option<String>,
    /// Buttons under field `.0` that put `.1` into it (completion of the typed text).
    pub fills: Vec<(usize, String)>,
    /// Muted text in front of the fill buttons ("Existing branches:").
    pub fills_label: String,
}

/// Computes a dialog's [`ModalLive`] from its values / option / checks.
type LiveFn = std::rc::Rc<dyn Fn(&ModalResult) -> ModalLive>;

type OkFn = Box<dyn FnOnce(&mut GittiApp, ModalResult, &mut Window, &mut Context<GittiApp>)>;
type CancelFn = Box<dyn FnOnce(&mut GittiApp, &mut Window, &mut Context<GittiApp>)>;
/// A further button's action (`ModalSpec::extra`); the dialog is closed first.
pub type ExtraFn = Box<dyn FnOnce(&mut GittiApp, &mut Window, &mut Context<GittiApp>)>;

pub struct Modal {
    pub title: String,
    /// Has a password field → secure keyboard entry while shown.
    pub secret: bool,
    /// A credential prompt (askpass): a key icon before the title, on every platform.
    auth: bool,
    message: Option<String>,
    fields: Vec<(String, Entity<InputState>)>,
    checks: Vec<(String, bool)>,
    text: Option<Entity<TextareaState>>,
    /// The text is read-only (output, errors, …): the list / scroll keys scroll it.
    viewer: bool,
    /// Height of the text box as last drawn (a page for the keys).
    text_h: std::rc::Rc<std::cell::Cell<Pixels>>,
    /// The part between title and buttons, scrolled when the window is too small for it.
    body_scroll: ScrollHandle,
    /// Each field's bounds as last drawn (`None`: not shown), and the field whose focus
    /// was last scrolled into view: Tab to a field out of view scrolls it in.
    field_bounds: std::rc::Rc<std::cell::RefCell<Vec<Option<Bounds<Pixels>>>>>,
    focus_shown: Option<usize>,
    ok_label: String,
    cancel_label: Option<String>,
    danger: bool,
    on_ok: Option<OkFn>,
    on_cancel: Option<CancelFn>,
    /// A further button left of Cancel / OK (`ModalSpec::extra`).
    extra: Option<(String, ExtraFn)>,
    choices: Vec<Choice>,
    /// Field `dst` follows field `src` until the user edits it (see `ModalSpec::derive`).
    derive: Option<Derive>,
    /// Notes under prefilled fields (`ModalSpec::prefilled`).
    notes: Vec<FieldNote>,
    /// Mutually exclusive options shown as a segmented row, and the chosen one.
    options: Vec<String>,
    option: usize,
    /// Fields shown only for some options (`ModalSpec::only_for`).
    field_options: Vec<Option<Vec<usize>>>,
    /// Focus of the dialog itself while no field has it (text viewer, no fields): Esc
    /// reaches `ModalCancel` and the pane keys don't act behind the dialog.
    focus: FocusHandle,
    /// Gets the focus back when the dialog closes (default: the commit list).
    return_focus: Option<FocusHandle>,
    /// A folder picker over the dialog (`super::folder_picker`), filling one of its fields.
    pub(super) folders: Option<super::folder_picker::FolderPick>,
    /// Its spell-checked editors (`ModalSpec::spell`, `spell_check_modal_text`).
    spell: Vec<SpellTarget>,
    /// `ModalSpec::live` and its answer for the values as they are now.
    live: Option<LiveFn>,
    live_now: Option<ModalLive>,
    _subscriptions: Vec<Subscription>,
}

/// A button under a dialog's fields that fills one of them: with a fixed value (a
/// suggestion; the field stays editable) or with a folder picked in the system dialog —
/// or, right of its field, one that opens a sub-dialog (`ModalSpec::pick`).
#[derive(Clone)]
pub struct Choice {
    pub label: String,
    pub field: usize,
    /// `None` = pick a folder in the system dialog.
    pub value: Option<String>,
    /// Opens a sub-dialog instead (gets the field and the dialog's values so far).
    pub pick: Option<PickFn>,
}

/// See [`ModalSpec::pick`].
pub type PickFn =
    std::rc::Rc<dyn Fn(&mut GittiApp, usize, ModalResult, &mut Window, &mut Context<GittiApp>)>;

/// Computes a derived field from its source field (`ModalSpec::derive`).
type DeriveFn = std::rc::Rc<dyn Fn(&str) -> String>;

/// A note under a prefilled field ("From the clipboard"), shown while the field still
/// holds the value it was opened with (`ModalSpec::prefilled`).
struct FieldNote {
    field: usize,
    text: String,
    value: String,
}

struct Derive {
    src: usize,
    dst: usize,
    f: DeriveFn,
    /// The value `dst` got from `src` last; once `dst` differs, the user owns it.
    last: String,
}

/// Declarative description of a dialog; turned into a [`Modal`] by `open_modal`.
pub struct ModalSpec {
    title: String,
    message: Option<String>,
    fields: Vec<(String, String, bool)>,
    checks: Vec<(String, bool)>,
    ok_label: String,
    cancel_label: Option<String>,
    danger: bool,
    auth: bool,
    choices: Vec<Choice>,
    derive: Option<(usize, usize, DeriveFn)>,
    /// Prefilled fields and their notes (`ModalSpec::prefilled`).
    prefilled: Vec<(usize, String)>,
    options: Vec<String>,
    option: usize,
    field_options: Vec<(usize, Vec<usize>)>,
    spell: Vec<usize>,
    live: Option<LiveFn>,
    extra: Option<(String, ExtraFn)>,
}

impl ModalSpec {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: None,
            fields: Vec::new(),
            checks: Vec::new(),
            ok_label: "OK".into(),
            cancel_label: Some("Cancel".into()),
            danger: false,
            auth: false,
            choices: Vec::new(),
            derive: None,
            prefilled: Vec::new(),
            options: Vec::new(),
            option: 0,
            field_options: Vec::new(),
            spell: Vec::new(),
            live: None,
            extra: None,
        }
    }

    /// A further button (`label`) left of Cancel / OK that closes the dialog and runs `f`
    /// (e.g. the signature window's "Get Signing Keys from GitHub…").
    pub fn extra(
        mut self,
        label: impl Into<String>,
        f: impl FnOnce(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
    ) -> Self {
        self.extra = Some((label.into(), Box::new(f)));
        self
    }

    /// The dialog says live what OK will do with the values entered so far (`f`, run on
    /// every change): a sentence under the fields, OK's label, and OK disabled with the
    /// reason while it can't work (Enter does nothing then either).
    pub fn live(mut self, f: impl Fn(&ModalResult) -> ModalLive + 'static) -> Self {
        self.live = Some(std::rc::Rc::new(f));
        self
    }

    /// Field `field` is a message (prose): spell checked like the commit message.
    pub fn spell(mut self, field: usize) -> Self {
        self.spell.push(field);
        self
    }

    /// Mutually exclusive options (a segmented row above the fields), `selected` first.
    pub fn options(mut self, labels: &[&str], selected: usize) -> Self {
        self.options = labels.iter().map(|l| l.to_string()).collect();
        self.option = selected.min(labels.len().saturating_sub(1));
        self
    }

    /// Field `field` is only shown (and meant) while one of `options` is chosen.
    pub fn only_for(mut self, field: usize, options: &[usize]) -> Self {
        self.field_options.push((field, options.to_vec()));
        self
    }

    /// A button that puts `value` into field `field`.
    pub fn choice(
        mut self,
        label: impl Into<String>,
        field: usize,
        value: impl Into<String>,
    ) -> Self {
        self.choices.push(Choice {
            label: label.into(),
            field,
            value: Some(value.into()),
            pick: None,
        });
        self
    }

    /// A button (`label`) that fills field `field` with a folder from the system dialog.
    pub fn browse(mut self, label: impl Into<String>, field: usize) -> Self {
        self.choices.push(Choice {
            label: label.into(),
            field,
            value: None,
            pick: None,
        });
        self
    }

    /// A button right of field `field` that opens a sub-dialog (e.g. the folder picker);
    /// `f` gets the field and the dialog's values / option at that moment.
    pub fn pick(
        mut self,
        label: impl Into<String>,
        field: usize,
        f: impl Fn(&mut GittiApp, usize, ModalResult, &mut Window, &mut Context<GittiApp>) + 'static,
    ) -> Self {
        self.choices.push(Choice {
            label: label.into(),
            field,
            value: None,
            pick: Some(std::rc::Rc::new(f)),
        });
        self
    }

    /// Field `dst` is computed from field `src` while the user hasn't changed it (from ""
    /// while `src` is hidden by the chosen option).
    pub fn derive(mut self, src: usize, dst: usize, f: impl Fn(&str) -> String + 'static) -> Self {
        self.derive = Some((src, dst, std::rc::Rc::new(f)));
        self
    }

    /// Field `field`'s initial value was put in for the user (e.g. from the clipboard): it
    /// opens selected (typing replaces it), a field derived from it follows as if it had
    /// been typed, and `note` shows under it while it still holds that value.
    pub fn prefilled(mut self, field: usize, note: impl Into<String>) -> Self {
        self.prefilled.push((field, note.into()));
        self
    }

    pub fn message(mut self, m: impl Into<String>) -> Self {
        self.message = Some(m.into());
        self
    }

    pub fn field(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push((label.into(), value.into(), false));
        self
    }

    pub fn secret(mut self, label: impl Into<String>) -> Self {
        self.fields.push((label.into(), String::new(), true));
        self
    }

    pub fn check(mut self, label: impl Into<String>, value: bool) -> Self {
        self.checks.push((label.into(), value));
        self
    }

    pub fn ok(mut self, label: impl Into<String>) -> Self {
        self.ok_label = label.into();
        self
    }

    pub fn cancel(mut self, label: Option<&str>) -> Self {
        self.cancel_label = label.map(str::to_string);
        self
    }

    /// Red OK button for destructive actions.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// A credential prompt: a key icon before the title.
    pub fn auth(mut self) -> Self {
        self.auth = true;
        self
    }
}

impl Modal {
    /// A read-only text window (`show_text_modal`).
    pub(super) fn is_text_view(&self) -> bool {
        self.viewer
    }

    /// The spell-checked editor behind `target`, if this dialog has it.
    pub(super) fn spell_editor(&self, target: SpellTarget) -> Option<SpellEditor> {
        if !self.spell.contains(&target) {
            return None;
        }
        match target {
            SpellTarget::ModalText => self.text.clone().map(SpellEditor::Area),
            SpellTarget::ModalField(ix) => {
                self.fields.get(ix).map(|(_, f)| SpellEditor::Line(f.clone()))
            }
            SpellTarget::Message => None,
        }
    }

    /// Its spell-checked editors.
    pub(super) fn spell_targets(&self) -> impl Iterator<Item = SpellTarget> + '_ {
        self.spell.iter().copied()
    }

    /// Field `ix`'s input.
    pub(super) fn field_input(&self, ix: usize) -> Option<&Entity<InputState>> {
        self.fields.get(ix).map(|(_, f)| f)
    }

    /// The dialog's text area (tests).
    pub(super) fn text_area(&self) -> Option<&Entity<TextareaState>> {
        self.text.as_ref()
    }

    /// The dialog, a field or the text area has the focus (for the key tests).
    pub(super) fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.is_focused(window)
            || self
                .fields
                .iter()
                .any(|(_, f)| f.read(cx).focus_handle(cx).is_focused(window))
            || self
                .text
                .as_ref()
                .is_some_and(|t| t.read(cx).focus_handle(cx).is_focused(window))
    }

    /// The focus is in the dialog: its fields, its text, or a menu over one of them
    /// (`overlay_keys.rs`).
    pub(super) fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.has_focus(window, cx) || self.focus.contains_focused(window, cx)
    }

    /// Where the focus belongs in the dialog: its first field, else its editable text,
    /// else the dialog itself (`open_modal_full` / `show_edit_modal` focus the same).
    pub(super) fn home_focus(&self, cx: &App) -> FocusHandle {
        match (self.fields.first(), &self.text) {
            (Some((_, f)), _) => f.read(cx).focus_handle(cx),
            (None, Some(t)) if !self.viewer => t.read(cx).focus_handle(cx),
            _ => self.focus.clone(),
        }
    }
}

impl GittiApp {
    pub(super) fn open_modal(
        &mut self,
        spec: ModalSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_ok: impl FnOnce(&mut GittiApp, ModalResult, &mut Window, &mut Context<GittiApp>) + 'static,
    ) {
        self.open_modal_full(spec, window, cx, Box::new(on_ok), None);
    }

    fn open_modal_full(
        &mut self,
        spec: ModalSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_ok: OkFn,
        on_cancel: Option<CancelFn>,
    ) {
        if let Some(old) = self.modal.take() {
            // Never silently drop a pending callback (e.g. an askpass reply).
            if let Some(c) = old.on_cancel {
                self.replacing_modal = true;
                c(self, window, cx);
                self.replacing_modal = false;
            }
        }
        let mut subs = Vec::new();
        let mut fields = Vec::new();
        let any_secret = spec.fields.iter().any(|f| f.2);
        for (ix, (label, value, secret)) in spec.fields.into_iter().enumerate() {
            let input = cx.new(|cx| {
                let mut s = InputState::new(window, cx).masked(secret);
                s.set_value(value, window, cx);
                s
            });
            subs.push(cx.subscribe_in(
                &input,
                window,
                move |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::PressEnter { .. } => this.confirm_modal(window, cx),
                    InputEvent::Change => {
                        this.modal_field_changed(ix, window, cx);
                        this.update_spell_of(SpellTarget::ModalField(ix), cx);
                    }
                    _ => {}
                },
            ));
            fields.push((label, input));
        }
        let derive = spec.derive.map(|(src, dst, f)| Derive {
            src,
            dst,
            f,
            last: fields
                .get(dst)
                .map(|(_, d)| d.read(cx).value().to_string())
                .unwrap_or_default(),
        });
        let notes = spec
            .prefilled
            .iter()
            .filter_map(|(ix, text)| {
                let (_, input) = fields.get(*ix)?;
                Some(FieldNote {
                    field: *ix,
                    text: text.clone(),
                    value: input.read(cx).value().to_string(),
                })
            })
            .collect();
        let mut field_options = vec![None; fields.len()];
        for (f, opts) in spec.field_options {
            if let Some(slot) = field_options.get_mut(f) {
                *slot = Some(opts);
            }
        }
        // The previous dialog's spell states (their editors are gone).
        self.spell.retain(|t, _| *t == SpellTarget::Message);
        let spell = spec
            .spell
            .iter()
            .filter(|&&ix| ix < fields.len())
            .map(|&ix| SpellTarget::ModalField(ix))
            .collect();
        let focus = cx.focus_handle();
        match fields.first() {
            Some((_, first)) => first.update(cx, |s, cx| s.focus(window, cx)),
            None => focus.focus(window, cx),
        }
        // The first field has the focus and stays where it is: a long message above it is
        // read first.
        let focus_shown = (!fields.is_empty()).then_some(0);
        self.modal = Some(Modal {
            secret: any_secret,
            title: spec.title,
            message: spec.message,
            fields,
            checks: spec.checks,
            text: None,
            viewer: false,
            text_h: Default::default(),
            body_scroll: ScrollHandle::new(),
            field_bounds: Default::default(),
            focus_shown,
            ok_label: spec.ok_label,
            cancel_label: spec.cancel_label,
            danger: spec.danger,
            auth: spec.auth,
            on_ok: Some(on_ok),
            on_cancel,
            extra: spec.extra,
            choices: spec.choices,
            derive,
            notes,
            options: spec.options,
            option: spec.option,
            field_options,
            focus,
            return_focus: None,
            folders: None,
            spell,
            live: spec.live,
            live_now: None,
            _subscriptions: subs,
        });
        // Prefilled fields: selected, and what follows from them as when typed.
        for (ix, _) in spec.prefilled {
            if let Some(input) = self.modal.as_ref().and_then(|m| m.field_input(ix)).cloned() {
                input.update(cx, |s, cx| s.select_all(window, cx));
                self.derive_modal_field(ix, window, cx);
            }
        }
        self.refresh_modal_live(cx);
        self.sync_secure_input(window);
        cx.notify();
    }

    /// Asks the open dialog's `ModalSpec::live` again (after any change of its input).
    fn refresh_modal_live(&mut self, cx: &App) {
        let Some(f) = self.modal.as_ref().and_then(|m| m.live.clone()) else {
            return;
        };
        let now = f(&self.modal_state(cx));
        if let Some(m) = self.modal.as_mut() {
            m.live_now = Some(now);
        }
    }

    /// Field `ix` changed: a derived field follows, the live text is asked again.
    fn modal_field_changed(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.derive_modal_field(ix, window, cx);
        self.refresh_modal_live(cx);
    }

    /// Keeps a derived field in step with its source (see [`ModalSpec::derive`]).
    fn derive_modal_field(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.modal.as_mut() else {
            return;
        };
        let Some(d) = m.derive.as_mut().filter(|d| d.src == ix) else {
            return;
        };
        let (Some((_, src)), Some((_, dst))) = (m.fields.get(d.src), m.fields.get(d.dst)) else {
            return;
        };
        // The user typed their own value: leave it alone.
        if dst.read(cx).value().as_ref() != d.last {
            return;
        }
        let shown = m.field_options[d.src]
            .as_ref()
            .is_none_or(|o| o.contains(&m.option));
        let new = (d.f)(if shown { src.read(cx).value() } else { "".into() }.as_ref());
        if new == d.last {
            return;
        }
        d.last = new.clone();
        let dst = dst.clone();
        dst.update(cx, |s, cx| s.set_value(new, window, cx));
        cx.notify();
    }

    /// Chooses option `ix` of the open dialog (a derived field follows).
    pub fn modal_option(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.modal.as_mut().filter(|m| ix < m.options.len()) else {
            return;
        };
        m.option = ix;
        if let Some(src) = m.derive.as_ref().map(|d| d.src) {
            self.derive_modal_field(src, window, cx);
        }
        self.refresh_modal_live(cx);
        cx.notify();
    }

    /// Runs choice `ix` of the open dialog: fills its field, or asks for a folder first.
    pub fn modal_choice(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.modal.as_ref() else {
            return;
        };
        let Some(choice) = m.choices.get(ix).cloned() else {
            return;
        };
        let Some((_, input)) = m.fields.get(choice.field).cloned() else {
            return;
        };
        if let Some(pick) = choice.pick {
            let state = self.modal_state(cx);
            pick(self, choice.field, state, window, cx);
            return;
        }
        match choice.value {
            Some(v) => {
                input.update(cx, |s, cx| {
                    s.set_value(v, window, cx);
                    s.focus(window, cx);
                });
                self.modal_field_changed(choice.field, window, cx);
                cx.notify();
            }
            None => {
                let answer = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some("Choose".into()),
                });
                cx.spawn_in(window, async move |_, cx| {
                    if let Ok(Ok(Some(paths))) = answer.await
                        && let Some(path) = paths.into_iter().next()
                    {
                        input
                            .update_in(cx, |s, window, cx| {
                                s.set_value(path.display().to_string(), window, cx)
                            })
                            .ok();
                    }
                })
                .detach();
            }
        }
    }

    /// The open dialog's values so far (as its OK button would hand them over).
    fn modal_state(&self, cx: &App) -> ModalResult {
        let m = self.modal.as_ref();
        ModalResult {
            values: self.script_modal_fields(cx),
            checks: m
                .map(|m| m.checks.iter().map(|(_, c)| *c).collect())
                .unwrap_or_default(),
            option: m.map_or(0, |m| m.option),
            text: None,
        }
    }

    /// Puts `text` into field `ix` of the open dialog and focuses it (a sub-dialog's
    /// result).
    pub(super) fn set_modal_field(
        &mut self,
        ix: usize,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self
            .modal
            .as_ref()
            .and_then(|m| m.fields.get(ix))
            .map(|(_, f)| f.clone())
        else {
            return;
        };
        input.update(cx, |s, cx| {
            s.set_value(text, window, cx);
            s.focus(window, cx);
        });
        self.modal_field_changed(ix, window, cx);
        cx.notify();
    }

    /// A completion button of the live text (`ModalLive::fills`): its value into its
    /// field, the cursor at the end.
    pub fn modal_fill(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((field, value)) = self
            .modal
            .as_ref()
            .and_then(|m| m.live_now.as_ref()?.fills.get(ix).cloned())
        else {
            return;
        };
        self.set_modal_field(field, value, window, cx);
    }

    /// Script hook: what the open dialog says live (`ModalSpec::live`).
    pub fn script_modal_live(&self) -> Option<ModalLive> {
        self.modal.as_ref()?.live_now.clone()
    }

    /// Script hook: the note shown under field `ix` of the open dialog (`ModalSpec::prefilled`).
    pub fn script_modal_note(&self, ix: usize, cx: &App) -> Option<String> {
        let m = self.modal.as_ref()?;
        let input = m.field_input(ix)?.read(cx);
        m.notes
            .iter()
            .find(|n| n.field == ix && input.value().as_ref() == n.value)
            .map(|n| n.text.clone())
    }

    /// Script hook: the selected text range (bytes) of field `ix` of the open dialog.
    pub fn script_modal_selection(&self, ix: usize, cx: &App) -> Option<std::ops::Range<usize>> {
        Some(self.modal.as_ref()?.field_input(ix)?.read(cx).selected_range())
    }

    /// Script hook: the open dialog's message.
    pub fn script_modal_message(&self) -> Option<String> {
        self.modal.as_ref()?.message.clone()
    }

    /// Script hook: the text of the open text window.
    pub fn script_modal_viewer_text(&self, cx: &App) -> Option<String> {
        let m = self.modal.as_ref().filter(|m| m.viewer)?;
        Some(m.text.as_ref()?.read(cx).value().to_string())
    }

    /// Script hook: title of the open dialog.
    pub fn script_modal_title(&self) -> Option<String> {
        self.modal.as_ref().map(|m| m.title.clone())
    }

    /// Script hook: values of the open dialog's fields.
    pub fn script_modal_fields(&self, cx: &App) -> Vec<String> {
        self.modal
            .as_ref()
            .map(|m| {
                m.fields
                    .iter()
                    .map(|(_, f)| f.read(cx).value().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Script hook: types `text` into field `ix` of the open dialog (a derived field
    /// follows, as when typing).
    pub fn script_modal_field(
        &mut self,
        ix: usize,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self
            .modal
            .as_ref()
            .and_then(|m| m.fields.get(ix))
            .map(|(_, f)| f.clone())
        else {
            return;
        };
        input.update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
        self.modal_field_changed(ix, window, cx);
    }

    /// Script hook: the chosen option of the open dialog.
    pub fn script_modal_option(&self) -> Option<usize> {
        self.modal.as_ref().map(|m| m.option)
    }

    /// Script hook: flips checkbox `ix` of the open dialog.
    pub fn script_modal_check(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(c) = self.modal.as_mut().and_then(|m| m.checks.get_mut(ix)) {
            c.1 = !c.1;
        }
        self.refresh_modal_live(cx);
        cx.notify();
    }

    /// Script hook: confirms the open dialog.
    pub fn script_confirm_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_modal(window, cx);
    }

    /// The open dialog gives the focus back to `handle` when it closes.
    pub(super) fn modal_returns_focus_to(&mut self, handle: FocusHandle) {
        if let Some(m) = self.modal.as_mut() {
            m.return_focus = Some(handle);
        }
    }

    /// Read-only, selectable text (full error output etc.).
    pub(super) fn show_text_modal(
        &mut self,
        title: &str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = ModalSpec::new(title).ok("Close").cancel(None);
        self.show_text_modal_spec(spec, text, window, cx);
    }

    /// [`Self::show_text_modal`] with the dialog's buttons from `spec` (OK closes it).
    pub(super) fn show_text_modal_spec(
        &mut self,
        spec: ModalSpec,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let area = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx);
            s.set_value(text, window, cx);
            s
        });
        self.open_modal(spec, window, cx, |_, _, _, _| {});
        if let Some(m) = self.modal.as_mut() {
            m.text = Some(area);
            m.viewer = true;
        }
    }

    /// The list / scroll keys in a text window: a line, a page (less a line), half a page,
    /// to the top / end. The text area clamps the offset in its next layout.
    pub(super) fn scroll_text_view(&mut self, nav: Nav, cx: &mut Context<Self>) {
        let Some((area, text_h)) = self
            .modal
            .as_ref()
            .filter(|m| m.viewer)
            .and_then(|m| Some((m.text.clone()?, m.text_h.get())))
        else {
            return;
        };
        area.update(cx, |s, cx| {
            let line = f32::from(s.line_height().unwrap_or(px(16.))).max(1.);
            // Full lines in the box (less its padding), less one kept for context.
            let page = ((f32::from(text_h) / line).floor() as usize).saturating_sub(2).max(1);
            let offset = s.scroll_offset();
            let first = (f32::from(-offset.y) / line).round().max(0.) as usize;
            let y = match nav {
                Nav::Last => 1e9,
                _ => gitti_core::nav::scroll_to(first, usize::MAX / 2, nav, page) as f32 * line,
            };
            s.set_scroll_offset(point(offset.x, px(-y)), cx);
        });
        cx.notify();
    }

    /// Script hook: the text window's first line (scroll offset in lines), the end of the
    /// rows shown, and its line count.
    pub(super) fn script_text_view(&self, cx: &App) -> Option<(usize, usize, usize)> {
        let area = self.modal.as_ref().filter(|m| m.viewer)?.text.as_ref()?;
        let s = area.read(cx);
        let rows = s.visible_row_range()?;
        let line = f32::from(s.line_height()?).max(1.);
        let first = (f32::from(-s.scroll_offset().y) / line).round().max(0.) as usize;
        Some((first, rows.end, gpui_kit::component::RopeExt::lines_len(s.text())))
    }

    /// Editable text (e.g. a config file); `on_save` gets the edited content.
    pub(super) fn show_edit_modal(
        &mut self,
        title: &str,
        message: Option<String>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_save: impl FnOnce(&mut GittiApp, String, &mut Window, &mut Context<GittiApp>) + 'static,
    ) {
        let area = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx);
            s.set_value(text, window, cx);
            s
        });
        let mut spec = ModalSpec::new(title).ok("Save");
        if let Some(m) = message {
            spec = spec.message(m);
        }
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            if let Some(text) = res.text {
                on_save(this, text, w, cx);
            }
        });
        if let Some(m) = self.modal.as_mut() {
            area.update(cx, |s, cx| s.focus(window, cx));
            m.text = Some(area);
        }
    }

    /// Gives the open dialog a multi-line text area under its fields (e.g. a tag's message;
    /// Enter there starts a new line), spell checked like the commit message; its text
    /// comes back as `ModalResult::text`. The focus stays in the first field.
    pub(super) fn add_modal_text_area(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let area = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx);
            s.set_value(text, window, cx);
            s
        });
        if let Some(m) = self.modal.as_mut() {
            m.text = Some(area);
        }
        self.spell_check_modal_text(cx);
    }

    /// The open dialog's text area is a message (Edit Commit Message…): spell checked like
    /// the commit message.
    pub(super) fn spell_check_modal_text(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.modal.as_mut() else {
            return;
        };
        let Some(text) = m.text.clone() else {
            return;
        };
        m.spell.push(SpellTarget::ModalText);
        m._subscriptions
            .push(cx.subscribe(&text, |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.update_spell_of(SpellTarget::ModalText, cx);
                }
            }));
        self.update_spell_of(SpellTarget::ModalText, cx);
    }

    pub(super) fn confirm_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // OK is disabled while the live text says it can't work (its reason is shown).
        if self
            .modal
            .as_ref()
            .and_then(|m| m.live_now.as_ref())
            .is_some_and(|l| l.blocked.is_some())
        {
            return;
        }
        let Some(mut modal) = self.modal.take() else {
            return;
        };
        let result = ModalResult {
            values: modal
                .fields
                .iter()
                .map(|(_, f)| f.read(cx).value().to_string())
                .collect(),
            checks: modal.checks.iter().map(|(_, c)| *c).collect(),
            option: modal.option,
            text: modal.text.as_ref().map(|t| t.read(cx).value().to_string()),
        };
        if let Some(ok) = modal.on_ok.take() {
            ok(self, result, window, cx);
        }
        self.after_modal_closed(modal.return_focus.take(), window, cx);
    }

    /// The dialog's further button (`ModalSpec::extra`): closes it, runs its action.
    pub(super) fn run_modal_extra(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut modal) = self.modal.take() else {
            return;
        };
        if let Some((_, f)) = modal.extra.take() {
            f(self, window, cx);
        }
        self.after_modal_closed(modal.return_focus.take(), window, cx);
    }

    pub(super) fn cancel_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut modal) = self.modal.take() else {
            return;
        };
        if let Some(c) = modal.on_cancel.take() {
            c(self, window, cx);
        }
        self.after_modal_closed(modal.return_focus.take(), window, cx);
    }

    /// Secure keyboard entry on while a password field is shown *and* our window is in
    /// front (it is system-wide, so it must not stay on behind another app). Scripted
    /// windows are never active; they count as active so tests can observe it.
    pub(super) fn sync_secure_input(&mut self, window: &Window) {
        let active = window.is_window_active() || std::env::var_os("GITTI_SCRIPT").is_some();
        let want = active && self.modal.as_ref().is_some_and(|m| m.secret);
        self.secure_input.set(want);
    }

    fn after_modal_closed(
        &mut self,
        return_focus: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        while self.modal.is_none()
            && let Some(req) = self.ask_queue.pop_front()
        {
            // Asked by a helper that is gone meanwhile: nobody waits for it.
            if req.prompt.asker.is_gone() {
                let _ = req.reply.send(None);
                continue;
            }
            self.show_ask(req, window, cx);
        }
        if self.modal.is_none() {
            return_focus.unwrap_or_else(|| self.list_focus.clone()).focus(window, cx);
        }
        self.sync_secure_input(window);
        cx.notify();
    }

    // ------------------------------------------------------------------ askpass

    /// Shows a credential prompt from git/ssh (queued behind an open dialog). Refused right
    /// away while the running operation is being cancelled — its ssh / hooks may ask once
    /// more before they are stopped — and when the helper is gone already.
    pub fn enqueue_ask(&mut self, req: AskRequest, window: &mut Window, cx: &mut Context<Self>) {
        if req.prompt.asker.is_gone() || (self.is_busy() && self.op_cancelled()) {
            let _ = req.reply.send(None);
            return;
        }
        if self.modal.is_some() {
            self.ask_queue.push_back(req);
        } else {
            self.show_ask(req, window, cx);
        }
    }

    /// The dialog of a credential prompt. Cancel (or "No") stops the whole operation, not
    /// only this question: ssh would otherwise move on to the next key or ask for a
    /// password, and the next ssh connection of the operation asks again. The dialog closes
    /// itself when its helper goes away (operation cancelled or finished).
    fn show_ask(&mut self, req: AskRequest, window: &mut Window, cx: &mut Context<Self>) {
        let kind = req.prompt.kind();
        // The prompt, which git command asked, and why it asks again.
        let prompt = req.prompt.message();
        let stop = if self.is_busy() { "Stop Operation" } else { "Cancel" };
        let reply_ok = req.reply.clone();
        let reply_cancel = req.reply;
        let spec = match kind {
            PromptKind::YesNo => ModalSpec::new("Confirm")
                .message(prompt)
                .ok("Yes")
                .cancel(Some(if self.is_busy() { "No, Stop Operation" } else { "No" })),
            PromptKind::Secret => ModalSpec::new("Authentication")
                .auth()
                .message(prompt)
                .secret("")
                .ok("OK")
                .cancel(Some(stop)),
            PromptKind::Text => ModalSpec::new("Authentication")
                .auth()
                .message(prompt)
                .field("", "")
                .ok("OK")
                .cancel(Some(stop)),
        };
        let asker = req.prompt.asker.clone();
        self.ask_shown = Some(asker.clone());
        self.open_modal_full(
            spec,
            window,
            cx,
            Box::new(move |this, result, _, _| {
                this.ask_shown = None;
                let answer = match kind {
                    PromptKind::YesNo => "yes".to_string(),
                    _ => result.values.into_iter().next().unwrap_or_default(),
                };
                let _ = reply_ok.send(Some(answer));
            }),
            Some(Box::new(move |this, _, cx| {
                this.ask_shown = None;
                let _ = reply_cancel.send(None);
                // Pushed aside by another dialog: only this question is unanswered.
                if !this.replacing_modal {
                    this.stop_op_for_ask(cx);
                }
            })),
        );
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(ASK_POLL).await;
                let open = this.update_in(cx, |this, window, cx| {
                    let shown = this.ask_shown.as_ref().is_some_and(|a| a.same(&asker));
                    if shown && asker.is_gone() {
                        this.dismiss_gone_ask(window, cx);
                        return false;
                    }
                    shown
                });
                if !matches!(open, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Cancel in a credential dialog: stops the running operation at once (also before
    /// its busy mark shows, unlike Esc on the mark), drops the operations queued behind it
    /// and refuses the prompts already waiting.
    fn stop_op_for_ask(&mut self, cx: &mut Context<Self>) {
        for req in self.ask_queue.drain(..) {
            let _ = req.reply.send(None);
        }
        if !self.is_busy() {
            return;
        }
        let Some(token) = self.op_cancel.as_ref().filter(|t| !t.is_cancelled()) else {
            return;
        };
        token.cancel();
        self.deferred.clear();
        if let Some(b) = &mut self.busy {
            *b = format!("Cancelling: {b}");
        }
        cx.notify();
    }

    /// The shown prompt's helper went away: its dialog closes without an answer (and
    /// without stopping anything — the asking command is over already).
    fn dismiss_gone_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ask_shown = None;
        let Some(mut modal) = self.modal.take() else {
            return;
        };
        modal.on_cancel = None;
        self.after_modal_closed(modal.return_focus.take(), window, cx);
    }

    /// Script hook (`askgone`): the helpers of the shown and queued prompts went away.
    pub fn script_asks_gone(&mut self) {
        for req in &self.ask_queue {
            req.prompt.asker.set_gone();
        }
        if let Some(a) = &self.ask_shown {
            a.set_gone();
        }
    }

    /// Script hook: type `text` into the first field (else the text area; `\n` = newline)
    /// and confirm.
    pub fn script_answer_modal(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = &self.modal {
            if let Some((_, f)) = m.fields.first() {
                let f = f.clone();
                f.update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
            } else if let Some(area) = m.text.clone() {
                let text = text.replace("\\n", "\n");
                area.update(cx, |s, cx| s.set_value(text, window, cx));
            }
        }
        self.confirm_modal(window, cx);
    }

    /// Script hook: puts `text` into the open dialog's text area (no confirming).
    pub fn script_modal_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(area) = self.modal.as_ref().and_then(|m| m.text.clone()) {
            area.update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
        }
    }

    pub fn script_cancel_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_modal(window, cx);
    }

    // ------------------------------------------------------------------ rendering

    pub(super) fn render_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let folders = self.render_folder_picker(window, cx);
        self.reveal_modal_focus(window, cx);
        let modal = self.modal.as_ref()?;
        let p = cx.palette().clone();
        let s = self.settings.clone();
        // The dialog's right padding holds the scrolling part's scrollbar: that part reaches
        // into it and keeps its content clear of the bar, so no field's right edge lies
        // under it (Linux / Windows draw the bar all the time) and nothing reflows when the
        // part starts to scroll.
        let gutter = window.rem_size().max(px(crate::scroll_guard::TRACK));
        let mut body = v_flex()
            .id("modal")
            .probe("modal")
            // rems: dialogs grow with the UI font, the same px at the default.
            .w(rems(if modal.text.is_some() {
                47.5
            } else if modal.fields.len() > 4 {
                38.75
            } else {
                28.75
            }))
            .max_w(relative(0.95))
            .max_h(relative(0.85))
            .p_4()
            .pr(gutter)
            .gap_3()
            .bg(p.bg_modal)
            .border_1()
            .border_color(p.border)
            .shadow_lg()
            // A text window also takes the list / scroll keys (`GittiTextView`).
            .key_context(if modal.viewer {
                "GittiModal GittiTextView"
            } else {
                "GittiModal"
            })
            .track_focus(&modal.focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .items_center()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.text_strong)
                    // An SVG key: the emoji gitty shows depends on the system fonts, and
                    // gitti showed nothing outside macOS.
                    .when(modal.auth, |d| {
                        d.child(
                            div()
                                .flex()
                                .probe("modal-key-icon")
                                .child(Icon::new(crate::assets::KEY_ICON).text_color(p.accent)),
                        )
                    })
                    .child(modal.title.clone()),
            );
        // Everything between the title and the buttons scrolls when the window is too
        // small for it (big fonts, a small window): the title and the buttons stay. Its
        // parts keep their height (a scrolling part of its own would shrink to nothing),
        // only the text box gives some up first.
        let mut middle = v_flex()
            .id("modal-body")
            .gap_3()
            .min_h_0()
            .pr(gutter)
            .overflow_y_scroll()
            .track_scroll(&modal.body_scroll);
        // The whole text: a long one scrolls with the rest of this part (its own scrolling
        // box showed no bar — a cut text looked complete).
        if let Some(m) = &modal.message {
            middle = middle.child(
                div()
                    .id("modal-message")
                    .flex_none()
                    .whitespace_normal()
                    .text_color(p.text)
                    .child(m.clone()),
            );
        }
        if !modal.options.is_empty() {
            let chosen = modal.option;
            let buttons = modal.options.iter().enumerate().map(|(i, label)| {
                div().probe(format!("modal-option:{i}")).child(
                    Button::new(("modal-option", i))
                        .label(label.clone())
                        .small()
                        .when(i == chosen, |b| b.primary())
                        .when(i != chosen, |b| b.ghost())
                        .on_click(cx.listener(move |this, _, w, cx| this.modal_option(i, w, cx))),
                )
            });
            // Many options wrap (they ran under the scrollbar, past the dialog's edge).
            middle = middle.child(h_flex().flex_none().flex_wrap().gap_1().children(buttons));
        }
        let mut fields = v_flex().id("modal-fields").flex_none().gap_3();
        // A field (and its fill-in buttons) only shown for some options.
        let hidden = |ix: usize| {
            modal
                .field_options
                .get(ix)
                .and_then(Option::as_ref)
                .is_some_and(|o| !o.contains(&modal.option))
        };
        {
            let mut b = modal.field_bounds.borrow_mut();
            b.clear();
            b.resize(modal.fields.len(), None);
        }
        for (ix, (label, input)) in modal.fields.iter().enumerate() {
            if hidden(ix) {
                continue;
            }
            let bounds = modal.field_bounds.clone();
            let focused = input.read(cx).focus_handle(cx).is_focused(window);
            fields = fields.child(
                v_flex()
                    .probe(format!("modal-field:{ix}"))
                    .relative()
                    .gap_1()
                    .child(
                        canvas(
                            move |b, _, _| {
                                if let Some(slot) = bounds.borrow_mut().get_mut(ix) {
                                    *slot = Some(b);
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    // Which field has the focus (for the tests: Tab scrolls it into view).
                    .when(focused, |d| {
                        d.child(div().absolute().inset_0().probe("modal-focus"))
                    })
                    .when(!label.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(s.ui_small())
                                .text_color(p.text_muted)
                                .child(label.clone()),
                        )
                    })
                    .child({
                        let input = self.modal_field_input(ix, input, window, cx);
                        // `pick` buttons sit right of their field.
                        let picks: Vec<AnyElement> = modal
                            .choices
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| c.pick.is_some() && c.field == ix)
                            .map(|(i, c)| choice_button(i, c, cx))
                            .collect();
                        if picks.is_empty() {
                            input
                        } else {
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().flex_1().min_w_0().child(input))
                                .children(picks)
                                .into_any_element()
                        }
                    })
                    .children(fill_buttons(modal.live_now.as_ref(), ix, cx))
                    .children(
                        modal
                            .notes
                            .iter()
                            .find(|n| n.field == ix && input.read(cx).value().as_ref() == n.value)
                            .map(|n| {
                                div()
                                    .probe(format!("modal-note:{ix}"))
                                    .text_size(s.ui_small())
                                    .text_color(p.text_muted)
                                    .child(n.text.clone())
                            }),
                    ),
            );
        }
        if !modal.fields.is_empty() {
            middle = middle.child(fields);
        }
        if modal.choices.iter().any(|c| c.pick.is_none() && !hidden(c.field)) {
            let buttons = modal
                .choices
                .iter()
                .enumerate()
                .filter(|(_, c)| c.pick.is_none() && !hidden(c.field))
                .map(|(i, c)| choice_button(i, c, cx));
            middle = middle.child(h_flex().flex_none().flex_wrap().gap_1().children(buttons));
        }
        // Whether macOS really has secure keyboard entry on (queried, not assumed).
        if modal.secret && cfg!(target_os = "macos") {
            let on = crate::secure_input::system_enabled();
            middle = middle.child(
                div()
                    .flex_none()
                    .text_size(s.ui_small())
                    .text_color(if on { p.success } else { p.text_muted })
                    .child(if on {
                        "🔒 Secure keyboard entry on — other apps cannot read what you type"
                    } else {
                        "Secure keyboard entry off (window not in front)"
                    }),
            );
        }
        for (i, (label, value)) in modal.checks.iter().enumerate() {
            let label = label.clone();
            let value = *value;
            middle = middle.child(
                h_flex()
                    .id(("modal-check", i))
                    .flex_none()
                    .probe(format!("modal-check:{i}"))
                    .gap_2()
                    .items_start()
                    .cursor_pointer()
                    .child(if value { "☑" } else { "☐" })
                    // A long label wraps inside the dialog.
                    .child(
                        div()
                            .probe(format!("modal-check-label:{i}"))
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(label),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(m) = this.modal.as_mut() {
                            m.checks[i].1 = !m.checks[i].1;
                        }
                        this.refresh_modal_live(cx);
                        cx.notify();
                    })),
            );
        }
        // What OK will do, right above it.
        if let Some(live) = &modal.live_now {
            middle = middle.child(
                v_flex()
                    .id("modal-live")
                    .flex_none()
                    .probe("modal-live")
                    .gap_1()
                    .whitespace_normal()
                    .when(!live.text.is_empty(), |d| {
                        d.child(div().text_color(p.text).child(live.text.clone()))
                    })
                    .when_some(live.blocked.clone(), |d, why| {
                        d.child(
                            div()
                                .probe("modal-blocked")
                                .text_color(p.warning)
                                .child(why),
                        )
                    }),
            );
        }
        if let Some(text) = &modal.text {
            let text_h = modal.text_h.clone();
            middle = middle.child(
                div()
                    .relative()
                    .child(
                        canvas(move |b, _, _| text_h.set(b.size.height), |_, _, _, _| {})
                            .absolute()
                            .inset_0(),
                    )
                    .h(px(380.))
                    .min_h(rems(6.))
                    .font_family(crate::theme::mono_font())
                    .text_size(s.ui_small())
                    .probe("modal-text")
                    .child(
                        Textarea::new(text)
                            .readonly(modal.viewer)
                            .h_full()
                            .when_some(self.spell_editor(SpellTarget::ModalText), |t, editor| {
                                t.context_menu(editor_context_menu(
                                    SpellTarget::ModalText,
                                    editor,
                                    s.spell_check,
                                ))
                            }),
                    )
                    .children(self.spell_editor(SpellTarget::ModalText).and_then(|e| {
                        let focused = e.focused(window, cx);
                        let look =
                            SpellLook::mono(s.ui_small(), super::diff_view::textarea_inset());
                        self.spell_marks_layer(SpellTarget::ModalText, look, focused, cx)
                    })),
            );
        }
        let live = modal.live_now.as_ref();
        let ok = Button::new("modal-ok")
            .label(
                live.and_then(|l| l.ok_label.clone())
                    .unwrap_or_else(|| modal.ok_label.clone()),
            )
            .disabled(live.is_some_and(|l| l.blocked.is_some()))
            .small()
            .when(modal.danger, |b| b.danger())
            .when(!modal.danger, |b| b.primary())
            .on_click(cx.listener(|this, _, w, cx| this.confirm_modal(w, cx)));
        let buttons = h_flex()
            .flex_none()
            .gap_2()
            .justify_end()
            .when_some(modal.extra.as_ref().map(|(l, _)| l.clone()), |d, label| {
                d.child(
                    div().probe("modal-extra").child(
                        Button::new("modal-extra")
                            .label(label)
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, w, cx| this.run_modal_extra(w, cx))),
                    ),
                )
            })
            .when_some(modal.cancel_label.clone(), |d, label| {
                d.child(
                    Button::new("modal-cancel")
                        .label(label)
                        .small()
                        .ghost()
                        .tooltip(crate::shortcuts::CANCEL_DIALOG.text(cx))
                        .on_click(cx.listener(|this, _, w, cx| this.cancel_modal(w, cx))),
                )
            })
            .child(div().probe("modal-ok").child(ok));
        // The probe on a frame around it: the scrolled element's own bounds move with it.
        body = body
            .child(
                v_flex()
                    .probe("modal-body")
                    .relative()
                    .min_h_0()
                    .mr(-gutter)
                    .child(middle)
                    .children(crate::scroll_guard::scrollbar("modal-body", &modal.body_scroll, false)),
            )
            .child(buttons);

        Some(
            div()
                .id("modal-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.45))
                .occlude()
                .child(body)
                .children(folders)
                .into_any_element(),
        )
    }
}

impl GittiApp {
    /// When the focus moved to another field of the open dialog (Tab, a click, a script)
    /// and that field is not whole in view of the scrolling part, scrolls it in — by as
    /// little as needed, before this frame is laid out (the bounds are the last frame's;
    /// moving the focus changes no layout).
    fn reveal_modal_focus(&mut self, window: &Window, cx: &App) {
        let Some(m) = self.modal.as_mut() else {
            return;
        };
        let focused = m
            .fields
            .iter()
            .position(|(_, f)| f.read(cx).focus_handle(cx).is_focused(window));
        if focused == m.focus_shown {
            return;
        }
        let Some(ix) = focused else {
            m.focus_shown = None;
            return;
        };
        // Not drawn yet (the dialog just opened): next frame.
        let Some(field) = m.field_bounds.borrow().get(ix).copied().flatten() else {
            return;
        };
        m.focus_shown = Some(ix);
        let view = m.body_scroll.bounds();
        if let Some(y) = reveal_offset(
            f32::from(m.body_scroll.offset().y),
            f32::from(m.body_scroll.max_offset().y),
            (f32::from(view.top()), f32::from(view.bottom())),
            (f32::from(field.top()), f32::from(field.bottom())),
        ) {
            let mut offset = m.body_scroll.offset();
            offset.y = px(y);
            m.body_scroll.set_offset(offset);
        }
    }

    /// Field `ix` of the open dialog as drawn: a plain input, or with spell checking
    /// (`ModalSpec::spell`) its right-click menu and the marks over it.
    fn modal_field_input(
        &self,
        ix: usize,
        input: &Entity<InputState>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &self.settings;
        let target = SpellTarget::ModalField(ix);
        let Some(editor) = self.spell_editor(target) else {
            return super::text_input(input, s).into_any_element();
        };
        let field = super::text_input(input, s).context_menu(editor_context_menu(
            target,
            editor.clone(),
            s.spell_check,
        ));
        // One line: clipped left / right only (the text scrolls sideways).
        let look = SpellLook {
            font: None,
            size: s.ui(),
            inset: point(px(1. + 10.), px(0.)),
        };
        let focused = editor.focused(window, cx);
        div()
            .relative()
            .child(field)
            .children(self.spell_marks_layer(target, look, focused, cx))
            .into_any_element()
    }
}

/// The completion buttons of the live text under field `ix` (`ModalLive::fills`).
fn fill_buttons(
    live: Option<&ModalLive>,
    ix: usize,
    cx: &mut Context<GittiApp>,
) -> Option<AnyElement> {
    let fills: Vec<AnyElement> = live?
        .fills
        .iter()
        .enumerate()
        .filter(|(_, (field, _))| *field == ix)
        .map(|(i, (_, value))| {
            div()
                .flex_none()
                .probe(format!("modal-fill:{i}"))
                .child(
                    Button::new(("modal-fill", i))
                        .label(value.clone())
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, w, cx| this.modal_fill(i, w, cx))),
                )
                .into_any_element()
        })
        .collect();
    let label = live?.fills_label.clone();
    let muted = cx.palette().text_muted;
    (!fills.is_empty()).then(|| {
        h_flex()
            .probe(format!("modal-fills:{ix}"))
            .flex_wrap()
            .items_center()
            .gap_1()
            .when(!label.is_empty(), |d| d.child(div().text_color(muted).child(label)))
            .children(fills)
            .into_any_element()
    })
}

/// A choice's button (under the fields, or right of its field for `pick`).
fn choice_button(i: usize, c: &Choice, cx: &mut Context<GittiApp>) -> AnyElement {
    div()
        .flex_none()
        .probe(format!("modal-choice:{i}"))
        .child(
            Button::new(("modal-choice", i))
                .label(c.label.clone())
                .xsmall()
                .ghost()
                .on_click(cx.listener(move |this, _, w, cx| this.modal_choice(i, w, cx))),
        )
        .into_any_element()
}

/// The scroll offset (negative or zero, px) that shows an element spanning `item` (top,
/// bottom) in a scrolling part spanning `view` at offset `offset` (both as drawn at that
/// offset), moving as little as needed; `None` when it is whole in view already. Taller
/// than the view: its top is shown. Clamped to `0..=max` scrolled.
fn reveal_offset(offset: f32, max: f32, view: (f32, f32), item: (f32, f32)) -> Option<f32> {
    let (top, bottom) = view;
    let delta = if item.0 < top {
        top - item.0
    } else if item.1 > bottom {
        // Up by what sticks out below, but never past the item's top.
        -((item.1 - bottom).min(item.0 - top))
    } else {
        return None;
    };
    let y = (offset + delta).clamp(-max.max(0.), 0.);
    (y != offset).then_some(y)
}

#[cfg(test)]
mod tests {
    use super::reveal_offset;

    #[gitti_test::test]
    fn reveal_offset_moves_as_little_as_needed() {
        let view = (100., 300.);
        // Whole in view: nothing.
        assert_eq!(reveal_offset(0., 500., view, (120., 160.)), None);
        // Below: up until its bottom is the view's bottom.
        assert_eq!(reveal_offset(0., 500., view, (320., 360.)), Some(-60.));
        // Above (scrolled past): down until its top is the view's top.
        assert_eq!(reveal_offset(-200., 500., view, (40., 80.)), Some(-140.));
        // Taller than the view: its top at the view's top.
        assert_eq!(reveal_offset(0., 500., view, (350., 700.)), Some(-250.));
        // Clamped to what can be scrolled.
        assert_eq!(reveal_offset(0., 30., view, (320., 360.)), Some(-30.));
        assert_eq!(reveal_offset(0., 0., view, (320., 360.)), None);
    }
}
