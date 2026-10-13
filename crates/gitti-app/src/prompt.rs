//! Questions with buttons (`Window::prompt`) drawn in the window, in gitti's look.
//!
//! macOS and Windows answer `Window::prompt` with their own system dialog. gpui has none
//! for Linux: there it draws a stand-in — a white box with black text in no particular
//! font, grey-bordered white buttons, the window greyed out behind it. [`install`]
//! replaces that with this dialog on Linux / BSD; `GITTI_WINDOW_PROMPTS=1` forces it
//! elsewhere too (to look at it on macOS).
//!
//! It looks like gitti's own dialogs (`app/modal.rs`): theme colours, the UI font, the
//! first button (the default) on the right. Keys: Enter / Space press the marked button
//! (the default at the start), Escape the Cancel button (the only button of a message),
//! Tab / → move the mark right, Shift-Tab / ← left (round).

use std::cell::RefCell;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::probe::ProbeExt as _;
use crate::theme::PaletteExt as _;

actions!(gitti, [PromptPress, PromptCancel, PromptRight, PromptLeft]);

/// The dialog's key context (its bindings in `crate::init`).
pub const KEY_CONTEXT: &str = "GittiPrompt";

/// The key bindings of the dialog.
pub fn key_bindings() -> Vec<KeyBinding> {
    let c = Some(KEY_CONTEXT);
    vec![
        KeyBinding::new("enter", PromptPress, c),
        KeyBinding::new("space", PromptPress, c),
        KeyBinding::new("escape", PromptCancel, c),
        KeyBinding::new("tab", PromptRight, c),
        KeyBinding::new("right", PromptRight, c),
        KeyBinding::new("shift-tab", PromptLeft, c),
        KeyBinding::new("left", PromptLeft, c),
    ]
}

/// Whether this platform needs the dialog: gpui has a system dialog on macOS and Windows.
pub fn wanted() -> bool {
    !cfg!(any(target_os = "macos", target_os = "windows"))
        || std::env::var_os("GITTI_WINDOW_PROMPTS").is_some()
}

/// Draws every `Window::prompt` of the app with [`GittiPrompt`].
pub fn install(cx: &mut App) {
    cx.set_prompt_builder(build);
}

thread_local! {
    /// The dialog up now (one per app: gpui prompts aren't re-entrant), for scripts.
    static OPEN: RefCell<Option<WeakEntity<GittiPrompt>>> = const { RefCell::new(None) };
}

fn build(
    level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    actions: &[PromptButton],
    handle: PromptHandle,
    window: &mut Window,
    cx: &mut App,
) -> RenderablePromptHandle {
    let view = cx.new(|cx| GittiPrompt {
        level,
        message: message.to_string(),
        detail: detail.filter(|d| !d.is_empty()).map(str::to_string),
        actions: actions.to_vec(),
        marked: 0,
        focus: cx.focus_handle(),
        detail_scroll: ScrollHandle::new(),
    });
    OPEN.with(|o| *o.borrow_mut() = Some(view.downgrade()));
    handle.with_view(view, window, cx)
}

/// The open dialog's answer by its button label (a script's `answer:LABEL`); false
/// without a dialog or such a button.
pub fn answer(label: &str, cx: &mut App) -> bool {
    with_open(cx, |p, cx| match p.actions.iter().position(|a| a.label() == label) {
        Some(ix) => {
            p.press(ix, cx);
            true
        }
        None => false,
    })
}

/// Cancels the open dialog as Escape does (a script's `cancel`); false without one.
pub fn cancel(cx: &mut App) -> bool {
    with_open(cx, |p, cx| {
        p.cancel(cx);
        true
    })
}

/// The open dialog's title, text and button labels (default first), for tests.
pub fn open_prompt(cx: &App) -> Option<(String, Option<String>, Vec<String>)> {
    let p = OPEN.with(|o| o.borrow().as_ref().and_then(|w| w.upgrade()))?;
    let p = p.read(cx);
    let labels = p.actions.iter().map(|a| a.label().to_string()).collect();
    Some((p.message.clone(), p.detail.clone(), labels))
}

fn with_open(cx: &mut App, f: impl FnOnce(&mut GittiPrompt, &mut Context<GittiPrompt>) -> bool) -> bool {
    let Some(p) = OPEN.with(|o| o.borrow().as_ref().and_then(|w| w.upgrade())) else {
        return false;
    };
    p.update(cx, f)
}

pub struct GittiPrompt {
    level: PromptLevel,
    message: String,
    detail: Option<String>,
    actions: Vec<PromptButton>,
    /// The button Enter presses: the default (first) one until Tab / arrows move it.
    marked: usize,
    focus: FocusHandle,
    detail_scroll: ScrollHandle,
}

impl GittiPrompt {
    fn press(&mut self, ix: usize, cx: &mut Context<Self>) {
        OPEN.with(|o| *o.borrow_mut() = None);
        cx.emit(PromptResponse(ix));
    }

    /// Escape: the Cancel button; a message with one button closes with it. Otherwise
    /// the question stays (no answer is assumed).
    fn cancel(&mut self, cx: &mut Context<Self>) {
        let ix = self.actions.iter().position(PromptButton::is_cancel);
        let ix = ix.or((self.actions.len() == 1).then_some(0));
        if let Some(ix) = ix {
            self.press(ix, cx);
        }
    }

    /// Moves the mark one button right / left as drawn; the buttons stand in reverse
    /// order (the first on the right), so right is the one before.
    fn step(&mut self, right: bool, cx: &mut Context<Self>) {
        let n = self.actions.len();
        if n > 1 {
            self.marked = if right { (self.marked + n - 1) % n } else { (self.marked + 1) % n };
            cx.notify();
        }
    }
}

impl EventEmitter<PromptResponse> for GittiPrompt {}

impl Focusable for GittiPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GittiPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        // Drawn as a root of its own: nothing is inherited from the window's view.
        let font = cx.theme().font_family.clone();
        let text_size = rems(crate::theme::DEFAULT_UI_FONT / 16.);
        let icon = match self.level {
            PromptLevel::Info => None,
            PromptLevel::Warning => Some(p.warning),
            PromptLevel::Critical => Some(p.danger),
        };
        // Right to left: the first (default) button stands on the right, as in gitti's
        // own dialogs and the system dialogs.
        let buttons = self.actions.iter().enumerate().rev().map(|(ix, action)| {
            let marked = ix == self.marked;
            div().probe(format!("prompt-button:{ix}")).child(
                Button::new(("prompt-button", ix))
                    .label(action.label().clone())
                    .small()
                    .when(marked && self.level == PromptLevel::Critical, |b| b.danger())
                    .when(marked && self.level != PromptLevel::Critical, |b| b.primary())
                    .when(!marked, |b| b.ghost())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.press(ix, cx);
                    })),
            )
        });
        let dialog = v_flex()
            .id("prompt")
            .probe("prompt")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &PromptPress, _, cx| this.press(this.marked, cx)))
            .on_action(cx.listener(|this, _: &PromptCancel, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &PromptRight, _, cx| this.step(true, cx)))
            .on_action(cx.listener(|this, _: &PromptLeft, _, cx| this.step(false, cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .cursor_default()
            .w(rems(28.75))
            .max_w(relative(0.95))
            .max_h(relative(0.85))
            .p_4()
            .gap_3()
            .bg(p.bg_modal)
            .border_1()
            .border_color(p.border)
            .shadow_lg()
            .font_family(font)
            .text_size(text_size)
            .text_color(p.text)
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .items_center()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.text_strong)
                    .when_some(icon, |d, color| {
                        d.child(Icon::new(IconName::TriangleAlert).flex_none().text_color(color))
                    })
                    .child(div().probe("prompt-title").child(self.message.clone())),
            )
            .when_some(self.detail.clone(), |d, detail| {
                d.child(
                    div()
                        .id("prompt-detail")
                        .probe("prompt-detail")
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.detail_scroll)
                        .child(detail),
                )
            })
            .child(
                h_flex()
                    .flex_none()
                    .flex_wrap()
                    .gap_2()
                    .justify_end()
                    .children(buttons),
            );
        div()
            .id("prompt-backdrop")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.45))
            .occlude()
            .child(dialog)
    }
}
