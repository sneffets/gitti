//! Tooltips that name the keys of a button's action ("Fetch  ⌘⇧F / F"). The keys are read
//! from the key map gitti binds (`init` in `lib.rs` and the table shared with gitty,
//! `gitti_core::keys`), never written by hand, so a tooltip can't drift from the bindings.
//! Every tooltip with keys is one of the [`Tip`]s below; `ALL` lists them for the test that
//! each one resolves to a real binding (`tests/ui/keys.rs`).

use std::cell::RefCell;
use std::collections::HashMap;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

/// Key contexts of the panes, where the single keys shared with gitty act.
const PANES: [&str; 3] = ["CommitList", "DiffView", "Sidebar"];

/// The key context of a repository tab's root: keys bound for the whole tab outside text
/// fields (`crate::GRAPH_KEY_CONTEXT`) match it.
pub const TAB: &str = "Gitti";

/// A tooltip naming an action and its keys.
pub struct Tip {
    pub label: &'static str,
    action: fn() -> Box<dyn Action>,
    /// Keys bound in this key context (a dialog's Escape, the sidebar's keys); `None` = the
    /// global binding plus the panes' single key.
    context: Option<&'static str>,
}

macro_rules! tips {
    ($($name:ident = $label:literal, $action:ident $(in $ctx:literal)?;)*) => {
        $(
            pub static $name: Tip = Tip {
                label: $label,
                action: || Box::new(crate::$action),
                context: tips!(@ctx $($ctx)?),
            };
        )*
        /// Every tip (for the test that each one resolves to a binding).
        pub static ALL: &[&Tip] = &[$(&$name),*];
    };
    (@ctx) => { None };
    (@ctx $ctx:literal) => { Some($ctx) };
}

tips! {
    // Toolbar.
    STASH = "Stash changes…", StashChanges;
    FETCH = "Fetch", Fetch;
    PULL = "Pull", Pull;
    PUSH = "Push", Push;
    REFRESH = "Refresh", Refresh;
    // Working directory: section headers, hunk buttons with selected lines, commit box.
    STAGE_ALL = "Stage all changes", StageAll;
    UNSTAGE_ALL = "Unstage all changes", UnstageAll;
    DISCARD_ALL = "Discard all unstaged changes", DiscardAll;
    SHOW_WHITESPACE = "Show whitespace-only changes", ToggleWhitespace;
    STAGE_LINES = "Stage the selected lines", StageSelection;
    UNSTAGE_LINES = "Unstage the selected lines", UnstageSelection;
    DISCARD_LINES = "Discard the selected lines", DiscardSelection;
    COMMIT = "Commit the staged changes", Commit;
    AMEND = "Amend the previous commit", ToggleAmend;
    // Panels, tabs, dialogs.
    CLOSE_GIT_LOG = "Hide the git command log", ToggleGitLog;
    OPEN_GIT_LOG = "Click: show the git command log", ToggleGitLog;
    LOG_NEWEST = "Scroll to the newest commands (follows new ones again)", ScrollEnd in "GitLog";
    OPEN_REPO = "Open repository…", OpenRepository;
    CLOSE_TAB = "Close tab", CloseTab;
    CLOSE_SETTINGS = "Close settings", ModalCancel in "GittiSettings";
    RESET_FONTS = "Reset font sizes", ResetZoom;
    FOLD_SECTION = "Fold / unfold", SideActivate in "Sidebar";
    CANCEL_DIALOG = "Cancel", ModalCancel in "GittiModal";
    ERROR_DETAILS = "Show the whole error", ErrorDetails;
}

thread_local! {
    /// Tooltip texts by label, for the key map version they were made from.
    static TEXTS: RefCell<(Option<KeymapVersion>, HashMap<&'static str, SharedString>)> =
        RefCell::new((None, HashMap::new()));
}

impl Tip {
    /// The keys of the action as shown: the global binding first (as the menus show it),
    /// then the single key of the panes — or the binding of `context`.
    pub fn keys(&self, keymap: &Keymap) -> Vec<String> {
        let action = (self.action)();
        let in_context = |b: &KeyBinding, ctx: &str| {
            let ctx = KeyContext::parse(ctx).expect("key context");
            b.predicate()
                .is_some_and(|p| p.eval(std::slice::from_ref(&ctx)))
        };
        let mut found: Vec<&KeyBinding> = Vec::new();
        let mut first = |f: &dyn Fn(&KeyBinding) -> bool| {
            if let Some(b) = keymap.bindings_for_action(action.as_ref()).find(|b| f(b)) {
                found.push(b);
            }
        };
        match self.context {
            Some(ctx) => first(&|b| in_context(b, ctx)),
            None => {
                first(&|b| b.predicate().is_none());
                first(&|b| PANES.iter().any(|ctx| in_context(b, ctx)));
            }
        }
        let mut out: Vec<String> = Vec::new();
        for b in found {
            let text = b
                .keystrokes()
                .iter()
                .map(|k| format_keystroke(k.modifiers(), k.key(), cfg!(target_os = "macos")))
                .collect::<Vec<_>>()
                .join(" ");
            if !out.contains(&text) {
                out.push(text);
            }
        }
        out
    }

    /// "Label  keys" (made once per key map; cheap enough to call while rendering).
    pub fn text(&'static self, cx: &App) -> SharedString {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        TEXTS.with(|t| {
            let mut t = t.borrow_mut();
            if t.0 != Some(keymap.version()) {
                *t = (Some(keymap.version()), HashMap::new());
            }
            t.1.entry(self.label)
                .or_insert_with(|| {
                    let keys = self.keys(&keymap);
                    if keys.is_empty() {
                        self.label.into()
                    } else {
                        format!("{}  {}", self.label, keys.join(" / ")).into()
                    }
                })
                .clone()
        })
    }

    /// A tooltip builder for `StatefulInteractiveElement::tooltip`; the text is made when
    /// the tooltip shows.
    pub fn tooltip(&'static self) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        move |window, cx| Tooltip::new(self.text(cx)).build(window, cx)
    }

    /// A tooltip builder with a longer explanation above the label and keys.
    pub fn tooltip_with(
        &'static self,
        detail: &'static str,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        move |window, cx| {
            Tooltip::new(format!("{detail}\n{}", self.text(cx))).build(window, cx)
        }
    }
}

/// Every key bound to `action` in `context` (`None` = the global bindings), as written in
/// hints, highest precedence first (the one a native menu shows), without duplicates. On
/// Windows / Linux the bindings with the platform key (Super) are left out: they exist for
/// macOS (⌃⌘) and nobody presses them elsewhere.
pub fn binding_texts(keymap: &Keymap, action: &dyn Action, context: Option<&str>) -> Vec<String> {
    let mac = cfg!(target_os = "macos");
    let ctx = context.map(|c| KeyContext::parse(c).expect("key context"));
    let mut out: Vec<String> = Vec::new();
    for b in keymap.bindings_for_action(action).rev() {
        let matches = match (&ctx, b.predicate()) {
            (None, None) => true,
            (Some(ctx), Some(p)) => p.eval(std::slice::from_ref(ctx)),
            _ => false,
        };
        if !matches || (!mac && b.keystrokes().iter().any(|k| k.modifiers().platform)) {
            continue;
        }
        let text = b
            .keystrokes()
            .iter()
            .map(|k| format_keystroke(k.modifiers(), k.key(), mac))
            .collect::<Vec<_>>()
            .join(" ");
        if !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

/// The keys of `action` for a palette entry: its global keys (at most two), then the
/// single key of the panes (or the tab-wide key, the graph keys); empty when it has
/// none.
pub fn hint(action: &dyn Action, cx: &App) -> String {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let mut keys = binding_texts(&keymap, action, None);
    keys.truncate(2);
    for ctx in PANES.into_iter().chain([TAB]) {
        if let Some(k) = binding_texts(&keymap, action, Some(ctx)).into_iter().next() {
            if !keys.contains(&k) {
                keys.push(k);
            }
            break;
        }
    }
    keys.join(" / ")
}

/// A context-menu entry of a pane (`DiffView`, …) with the keys of `action` there:
/// "Copy  ⌘C / Y" — global keys and the pane's own, keys with modifiers first; the bare
/// label when it has none.
pub fn menu_label(label: &str, action: &dyn Action, context: &str, cx: &App) -> String {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let mut keys = binding_texts(&keymap, action, None);
    for k in binding_texts(&keymap, action, Some(context)) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    // A single character is a bare key; the ones with modifiers are the familiar ones.
    keys.sort_by_key(|k| k.chars().count() == 1);
    if keys.is_empty() {
        label.to_string()
    } else {
        format!("{label}  {}", keys.join(" / "))
    }
}

/// A key of the table shared with gitty (`gitti_core::keys`, gpui syntax: `shift-g`,
/// `ctrl-d`, `pageup`) as written in hints (⇧G, ⌃D, PgUp).
pub fn key_text(source: &str) -> String {
    match Keystroke::parse(source) {
        Ok(k) => format_keystroke(&k.modifiers, &k.key, cfg!(target_os = "macos")),
        Err(_) => source.to_string(),
    }
}

/// A keystroke as written in hints: macOS symbols in the order the app's hints use
/// (`⌃⌘⌥⇧`, e.g. ⌘⇧F), elsewhere `Ctrl+Alt+Shift+F`.
pub fn format_keystroke(m: &Modifiers, key: &str, mac: bool) -> String {
    // A shifted letter outside ASCII (`Ö`, a keyboard layout's `{`) shows its Shift
    // like ⇧B does — the label is upper case either way.
    let shifted = {
        let mut c = key.chars();
        matches!((c.next(), c.next()), (Some(ch), None) if ch.is_uppercase())
    };
    let m = &Modifiers {
        shift: m.shift || shifted,
        ..*m
    };
    let key = key_label(key, mac);
    if mac {
        let mut s = String::new();
        for (on, sym) in [
            (m.control, "⌃"),
            (m.platform, "⌘"),
            (m.alt, "⌥"),
            (m.shift, "⇧"),
        ] {
            if on {
                s.push_str(sym);
            }
        }
        s + &key
    } else {
        let mut parts: Vec<&str> = [
            (m.control, "Ctrl"),
            (m.alt, "Alt"),
            (m.shift, "Shift"),
            (m.platform, "Super"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        parts.push(&key);
        parts.join("+")
    }
}

fn key_label(key: &str, mac: bool) -> String {
    match key {
        "enter" if mac => "↩".into(),
        "enter" => "Enter".into(),
        "escape" => "Esc".into(),
        "backspace" if mac => "⌫".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        "delete" => "Del".into(),
        "-" => "−".into(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => {
            let mut c = k.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: with `test-support` the gpui glob would shadow `#[test]`.
    use super::{ALL, format_keystroke};
    use gpui_kit::Keystroke;

    fn fmt(source: &str, mac: bool) -> String {
        let k = Keystroke::parse(source).unwrap();
        format_keystroke(&k.modifiers, &k.key, mac)
    }

    #[gitti_test::test]
    fn keystrokes_are_written_per_platform() {
        assert_eq!(fmt("cmd-shift-f", true), "⌘⇧F");
        assert_eq!(fmt("ctrl-shift-f", false), "Ctrl+Shift+F");
        assert_eq!(fmt("cmd-alt-down", true), "⌘⌥↓");
        assert_eq!(fmt("ctrl-alt-up", false), "Ctrl+Alt+↑");
        assert_eq!(fmt("cmd-ctrl--", true), "⌃⌘−");
        assert_eq!(fmt("cmd-enter", true), "⌘↩");
        assert_eq!(fmt("ctrl-enter", false), "Ctrl+Enter");
        assert_eq!(fmt("shift-p", true), "⇧P");
        assert_eq!(fmt("shift-p", false), "Shift+P");
        assert_eq!(fmt("escape", true), "Esc");
        assert_eq!(fmt("f5", false), "F5");
        assert_eq!(fmt("space", false), "Space");
        assert_eq!(fmt("?", true), "?");
        // A keyboard layout's shifted letter (German `{` = Shift-Ö) and its base.
        assert_eq!(fmt("cmd-Ö", true), "⌘⇧Ö");
        assert_eq!(fmt("ctrl-Ö", false), "Ctrl+Shift+Ö");
        assert_eq!(fmt("ö", true), "Ö");
        // `secondary` is cmd on macOS, ctrl elsewhere.
        let secondary = fmt("secondary-o", cfg!(target_os = "macos"));
        assert_eq!(
            secondary,
            if cfg!(target_os = "macos") { "⌘O" } else { "Ctrl+O" }
        );
    }

    #[gitti_test::test]
    fn tip_labels_are_unique() {
        // Texts are cached by label.
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.label, b.label);
            }
        }
    }
}
