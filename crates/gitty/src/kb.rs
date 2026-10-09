//! The keyboard layout the shortcuts follow (`gitti_core::keys::layout`): one per
//! process, read from the settings shared with gitti (`keyboard_layout`), changed in the
//! palette (Keyboard Layout…). Key lookups and every text that names keys go through it.

use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};

use gitti_core::keys::{KeyboardLayout, KeyboardSettings};
use gitti_core::store;

static LAYOUT: AtomicUsize = AtomicUsize::new(0);

pub fn layout() -> KeyboardLayout {
    KeyboardLayout::ALL
        .get(LAYOUT.load(Ordering::Relaxed))
        .copied()
        .unwrap_or_default()
}

pub fn set(layout: KeyboardLayout) {
    let ix = KeyboardLayout::ALL
        .iter()
        .position(|l| *l == layout)
        .unwrap_or(0);
    LAYOUT.store(ix, Ordering::Relaxed);
}

/// Reads the persisted layout (start, focus gained); whether it changed.
pub fn reload() -> bool {
    let saved = store::load_settings::<KeyboardSettings>().keyboard_layout;
    let changed = saved != layout();
    set(saved);
    changed
}

/// Saves `layout` for both apps and uses it.
pub fn save(layout: KeyboardLayout) {
    let before: KeyboardSettings = store::load_settings();
    let after = KeyboardSettings {
        keyboard_layout: layout,
        ..before
    };
    store::save_changed_settings(&before, &after);
    set(layout);
}

/// The character a US single-character key is typed as now (`]` → `ä` on German).
pub fn ch(us: char) -> char {
    layout().char_for(us)
}

/// A shared-table key (`secondary-g`, `]`) as the terminal names it on this layout.
pub fn key(us: &str) -> String {
    gitti_core::keys::terminal_key(&layout().remap(us))
}

/// A help / hint text with the bracket keys as this layout types them: the texts name
/// keys only where they have `[` / `]` (gitty's help, the merge editor's, the hint bar).
pub fn text(s: &str) -> Cow<'_, str> {
    let l = layout();
    if l == KeyboardLayout::Us || !s.contains(['[', ']']) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        s.chars()
            .map(|c| match c {
                '[' | ']' => l.char_for(c),
                c => c,
            })
            .collect(),
    )
}

impl crate::app::App {
    /// Palette ▸ Keyboard Layout…: the layouts, the current one marked; picking one moves
    /// the shortcuts its keyboard can't type and says where they went.
    pub fn keyboard_layout_picker(&mut self) {
        use crate::modal::{Modal, PickItem};
        let current = layout();
        let items = KeyboardLayout::ALL
            .into_iter()
            .map(|l| {
                let moved = describe(l);
                let mut it = PickItem::new(l.name(), moved, move |app: &mut crate::app::App| {
                    save(l);
                    app.set_info(format!("Keyboard layout: {}. {}", l.name(), summary(l)));
                });
                if l == current {
                    it.hint = format!("current · {}", it.hint);
                    it.strong = true;
                }
                it
            })
            .collect();
        self.modal = Some(Modal::picker("Keyboard Layout (shortcuts)", items));
    }
}

/// What a layout changes, short: "[ ] → ö ä" (the keys every shortcut list shows).
fn describe(l: KeyboardLayout) -> String {
    if l.char_for('[') == '[' && l.char_for(']') == ']' {
        return "keys as written".into();
    }
    format!("[ ] → {} {}", l.char_for('['), l.char_for(']'))
}

/// The sentence after choosing `l`.
fn summary(l: KeyboardLayout) -> String {
    if l.char_for('[') == '[' && l.char_for(']') == ']' {
        "Shortcuts as written (US keys).".into()
    } else {
        format!(
            "Next / previous file: {} {} (US ] [); all other keys stay — see ? for the list.",
            l.char_for(']'),
            l.char_for('[')
        )
    }
}
