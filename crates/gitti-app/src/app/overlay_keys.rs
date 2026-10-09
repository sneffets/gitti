//! Esc closes the topmost popup / dialog wherever the keyboard focus is.
//!
//! The overlays — a dialog (`Modal`), the folder picker over it, the pickers (palette,
//! branch switcher, …) and the settings window — take Esc through their key contexts
//! (`GittiModal`, `GittiPicker`, `GittiSettings`, `GittiFolderPicker`), but only while the
//! focus is inside them. When it ends up behind one (a menu or the window giving it back to
//! the commit list after the dialog opened, a click that moved it), Esc went to the panes
//! (End Compare, back to the list) and the dialog stayed open; the pane keys even acted
//! behind it. One keystroke interceptor per window (gpui runs it before matching key
//! bindings) handles that case for every overlay:
//! - Esc closes the topmost overlay (the same order as `ModalCancel`);
//! - any other key without ⌘ / Ctrl / Alt, while the focus is on a pane or nowhere, puts the
//!   focus back into the overlay and is dropped, so nothing acts behind the dialog;
//!   shortcuts with modifiers pass (they work over a dialog anyway).
//!
//! An open context menu / dropdown (`tooltip_gate::menu_open`) keeps its keys, Esc included.
//! While the focus is inside the overlay nothing changes: its own bindings act.

use gpui_kit::*;

use super::GittiApp;
use crate::workspace::Workspace;

/// The overlays, topmost first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlay {
    Picker,
    Folders,
    Modal,
    Settings,
}

impl GittiApp {
    /// The overlay on top, if one is open.
    fn top_overlay(&self) -> Option<Overlay> {
        if self.picker.is_some() {
            Some(Overlay::Picker)
        } else if self.folder_picker_focus().is_some() {
            Some(Overlay::Folders)
        } else if self.modal.is_some() {
            Some(Overlay::Modal)
        } else if self.settings_open {
            Some(Overlay::Settings)
        } else {
            None
        }
    }

    fn overlay_has_focus(&self, o: Overlay, window: &Window, cx: &App) -> bool {
        match o {
            Overlay::Picker => self
                .picker
                .as_ref()
                .is_some_and(|p| p.focus_handle(cx).contains_focused(window, cx)),
            Overlay::Folders => self
                .folder_picker_focus()
                .is_some_and(|f| f.contains_focused(window, cx)),
            Overlay::Modal => self
                .modal
                .as_ref()
                .is_some_and(|m| m.contains_focus(window, cx)),
            Overlay::Settings => self.settings_focus.contains_focused(window, cx),
        }
    }

    /// Where the overlay's focus belongs.
    fn overlay_focus(&self, o: Overlay, cx: &App) -> Option<FocusHandle> {
        match o {
            Overlay::Picker => self.picker.as_ref().map(|p| p.focus_handle(cx)),
            Overlay::Folders => self.folder_picker_focus(),
            Overlay::Modal => self.modal.as_ref().map(|m| m.home_focus(cx)),
            Overlay::Settings => Some(self.settings_focus.clone()),
        }
    }

    /// Closes the topmost overlay (Esc, `ModalCancel`); `false` when none is open.
    pub(super) fn close_top_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.top_overlay() {
            Some(Overlay::Picker) => self.close_picker(window, cx),
            Some(Overlay::Folders) => self.close_folder_picker(false, window, cx),
            Some(Overlay::Modal) => self.cancel_modal(window, cx),
            Some(Overlay::Settings) => self.close_settings(window, cx),
            None => return false,
        }
        true
    }

    /// The focus is on one of the panes (or the commit message / command line) or nowhere.
    fn focus_on_pane(&self, window: &Window, cx: &App) -> bool {
        let Some(focused) = window.focused(cx) else {
            return true;
        };
        [
            &self.list_focus,
            &self.side_focus,
            &self.diff_focus,
            &self.log_focus,
        ]
        .into_iter()
        .any(|h| *h == focused)
            || self.message.read(cx).focus_handle(cx) == focused
            || self.cmd_input.read(cx).focus_handle(cx) == focused
    }

    /// The interceptor's part (see the module docs): `true` when the keystroke was taken.
    fn intercept_overlay_key(
        &mut self,
        k: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(top) = self.top_overlay() else {
            // No dialog: Esc closes the running operation's output window, else
            // cancels the operation once its busy state shows (`op_cancel.rs`);
            // otherwise it goes on to the panes.
            return k.key == "escape"
                && !k.modifiers.modified()
                && !crate::tooltip_gate::menu_open(cx)
                && (self.close_op_output(cx) || self.cancel_op(cx));
        };
        if crate::tooltip_gate::menu_open(cx) || self.overlay_has_focus(top, window, cx) {
            return false;
        }
        let m = &k.modifiers;
        if k.key == "escape" && !m.modified() {
            self.close_top_overlay(window, cx);
            cx.notify();
            return true;
        }
        if m.control || m.alt || m.platform || m.function || !self.focus_on_pane(window, cx) {
            return false;
        }
        if let Some(f) = self.overlay_focus(top, cx) {
            f.focus(window, cx);
            cx.notify();
        }
        true
    }
}

/// Installs the interceptor for the workspace's window: it acts on the active tab (or the
/// start page) of that window only.
pub(crate) fn intercept(
    workspace: WeakEntity<Workspace>,
    window: &Window,
    cx: &mut App,
) -> Subscription {
    let handle = window.window_handle();
    cx.intercept_keystrokes(move |ev, window, cx| {
        if window.window_handle() != handle {
            return;
        }
        let Some(app) = workspace.upgrade().and_then(|ws| ws.read(cx).script_app()) else {
            return;
        };
        if app.update(cx, |a, cx| {
            a.intercept_overlay_key(&ev.keystroke, window, cx)
        }) {
            cx.stop_propagation();
        }
    })
}
