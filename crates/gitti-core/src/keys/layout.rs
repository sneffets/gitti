//! Keyboard layouts for the shortcuts: many European layouts (German especially) can't type
//! a US binding's square brackets with one key. Every key binding of both apps is
//! written for a US keyboard; the layout setting turns each one into a key the chosen layout
//! can type, and everything that shows keys (menus, palette, shortcut lists, help, tooltips)
//! shows the result. gitty only gets characters from the terminal, so this is a table, not
//! physical key codes — and gitti uses the same table so both apps agree.
//!
//! The rule, per key of a binding:
//! - **Letters stay letters.** `s` stages, `z` stashes, `y` copies on QWERTZ and AZERTY too:
//!   the mnemonic is the letter, every layout types it, and a terminal reports the letter.
//!   (QWERTZ swaps y / z, AZERTY a / q, z / w, m: following the position would move `z`
//!   onto the key labelled Y — nobody would find it.)
//! - **A character the layout types with one key, with or without Shift, stays.** `?` (help),
//!   `:` (git command), `+` / `-` (context lines), `=`, `,`, `.`, digits: on a German
//!   keyboard `?` is Shift-ß and `:` Shift-. — that is where Germans look for them, and ⌘+ /
//!   ⌘− zoom stay the familiar ones. This is also how macOS localizes shortcuts (key
//!   equivalents move only what a layout can't type).
//! - **A character the layout can't type directly** (it needs AltGr / Option, or its key is
//!   a dead key) **moves to the key at its US position** — same Shift level, or the other
//!   level of that key when that one is a dead key. In practice that is `[` `]` `{` `}` on
//!   every European layout: AltGr-8 / ⌥5 on German. Where the US position itself is taken
//!   or dead, the layout's table names the key instead (German: `+` sits where US `]` is but
//!   is "more context" itself, so the pair moves one row down to ö / ä — the keys macOS uses
//!   for ⌘[ / ⌘] on German keyboards; Nordic: ö ä / ø æ / æ ø; Spanish ñ / ç; French ù / $).
//!
//! Shifted results are written as the character the key types (`Ö`, like `?` today), never
//! `shift-ö`: gpui on macOS / Windows / Linux reports a shifted non-letter key as that
//! character without Shift, and a terminal sends the character.
//!
//! Positions are the ISO / ANSI block: the number row (13 keys, US `` ` `` … `=`), the three
//! letter rows (US `q` … `]`, `a` … `\` — on ISO keyboards `\` is the key left of Enter), and
//! the key left of `z` that ISO keyboards have (`<` on German). The tables are the Windows /
//! Linux layouts; macOS differs only where it matters for these keys (French AZERTY), see
//! [`KeyboardLayout::rows`].

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{BINDINGS, Command, Scope, same_key};

/// A keyboard layout the shortcuts can follow. [`KeyboardLayout::Us`] = the bindings as
/// written (default; nothing changes for anyone who doesn't pick another one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum KeyboardLayout {
    #[default]
    #[serde(rename = "us")]
    Us,
    #[serde(rename = "uk")]
    Uk,
    #[serde(rename = "de")]
    German,
    #[serde(rename = "ch")]
    SwissGerman,
    #[serde(rename = "fr")]
    French,
    #[serde(rename = "es")]
    Spanish,
    /// Swedish and Finnish (the same layout).
    #[serde(rename = "se")]
    Swedish,
    #[serde(rename = "no")]
    Norwegian,
    #[serde(rename = "dk")]
    Danish,
}

/// The persisted choice (settings table, shared by gitti and gitty: field
/// `keyboard_layout`; `keyboard_layout_hint` = gitti told about it once already).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardSettings {
    pub keyboard_layout: KeyboardLayout,
    pub keyboard_layout_hint: bool,
}

/// The keys of a layout, aligned with the US keys: per row the characters without and with
/// Shift (`' '` = the key types nothing there).
struct Rows {
    rows: [(&'static str, &'static str); 4],
    /// The ISO key left of `z` (US keyboards don't have it).
    iso: Option<(char, char)>,
    /// Dead keys (accents that wait for the next key; a terminal doesn't send them).
    dead: &'static str,
    /// `(US key, US key whose place it takes)`: where the US position is taken or dead.
    moves: &'static [(char, char)],
}

const US: Rows = Rows {
    rows: [
        ("`1234567890-=", "~!@#$%^&*()_+"),
        ("qwertyuiop[]", "QWERTYUIOP{}"),
        ("asdfghjkl;'\\", "ASDFGHJKL:\"|"),
        ("zxcvbnm,./", "ZXCVBNM<>?"),
    ],
    iso: None,
    dead: "",
    moves: &[],
};

const UK: Rows = Rows {
    rows: [
        ("`1234567890-=", "¬!\"£$%^&*()_+"),
        ("qwertyuiop[]", "QWERTYUIOP{}"),
        ("asdfghjkl;'#", "ASDFGHJKL:@~"),
        ("zxcvbnm,./", "ZXCVBNM<>?"),
    ],
    iso: Some(('\\', '|')),
    dead: "",
    moves: &[],
};

/// The brackets move one row down, to the US `;` / `'` keys (German ö ä, Nordic ø æ …).
const ROW_DOWN: &[(char, char)] = &[('[', ';'), (']', '\'')];

const GERMAN: Rows = Rows {
    rows: [
        ("^1234567890ß´", "°!\"§$%&/()=?`"),
        ("qwertzuiopü+", "QWERTZUIOPÜ*"),
        ("asdfghjklöä#", "ASDFGHJKLÖÄ'"),
        ("yxcvbnm,.-", "YXCVBNM;:_"),
    ],
    iso: Some(('<', '>')),
    dead: "^´`",
    moves: ROW_DOWN,
};

const SWISS_GERMAN: Rows = Rows {
    rows: [
        ("§1234567890'^", "°+\"*ç%&/()=?`"),
        ("qwertzuiopü¨", "QWERTZUIOPè!"),
        ("asdfghjklöä$", "ASDFGHJKLéà£"),
        ("yxcvbnm,.-", "YXCVBNM;:_"),
    ],
    iso: Some(('<', '>')),
    dead: "^`¨",
    moves: ROW_DOWN,
};

/// French AZERTY on Windows / Linux.
const FRENCH: Rows = Rows {
    rows: [
        ("²&é\"'(-è_çà)=", " 1234567890°+"),
        ("azertyuiop^$", "AZERTYUIOP¨£"),
        ("qsdfghjklmù*", "QSDFGHJKLM%µ"),
        ("wxcvbn,;:!", "WXCVBN?./§"),
    ],
    iso: Some(('<', '>')),
    dead: "^¨",
    // `[`: its key is dead; ù is the key below it. `]` keeps its place ($).
    moves: &[('[', '\'')],
};

/// French AZERTY on macOS (`-` and `=` sit elsewhere, `*` is Shift-$).
const FRENCH_MAC: Rows = Rows {
    rows: [
        ("@&é\"'(§è!çà)-", "#1234567890°_"),
        ("azertyuiop^$", "AZERTYUIOP¨*"),
        ("qsdfghjklmù`", "QSDFGHJKLM%£"),
        ("wxcvbn,;:=", "WXCVBN?./+"),
    ],
    iso: Some(('<', '>')),
    dead: "^¨`",
    moves: &[('[', '\'')],
};

const SPANISH: Rows = Rows {
    rows: [
        ("º1234567890'¡", "ª!\"·$%&/()=?¿"),
        ("qwertyuiop`+", "QWERTYUIOP^*"),
        ("asdfghjklñ´ç", "ASDFGHJKLÑ¨Ç"),
        ("zxcvbnm,.-", "ZXCVBNM;:_"),
    ],
    iso: Some(('<', '>')),
    dead: "`^´¨",
    // `+` is where US `]` is and is "more context" itself; the key left of Enter is free.
    moves: &[('[', ';'), (']', '\\')],
};

const SWEDISH: Rows = Rows {
    rows: [
        ("§1234567890+´", "½!\"#¤%&/()=?`"),
        ("qwertyuiopå¨", "QWERTYUIOPÅ^"),
        ("asdfghjklöä'", "ASDFGHJKLÖÄ*"),
        ("zxcvbnm,.-", "ZXCVBNM;:_"),
    ],
    iso: Some(('<', '>')),
    dead: "´`¨^",
    moves: ROW_DOWN,
};

const NORWEGIAN: Rows = Rows {
    rows: [
        ("|1234567890+\\", "§!\"#¤%&/()=?`"),
        ("qwertyuiopå¨", "QWERTYUIOPÅ^"),
        ("asdfghjkløæ'", "ASDFGHJKLØÆ*"),
        ("zxcvbnm,.-", "ZXCVBNM;:_"),
    ],
    iso: Some(('<', '>')),
    dead: "`¨^",
    moves: ROW_DOWN,
};

const DANISH: Rows = Rows {
    rows: [
        ("½1234567890+´", "§!\"#¤%&/()=?`"),
        ("qwertyuiopå¨", "QWERTYUIOPÅ^"),
        ("asdfghjklæø'", "ASDFGHJKLÆØ*"),
        ("zxcvbnm,.-", "ZXCVBNM;:_"),
    ],
    iso: Some(('<', '>')),
    dead: "´`¨^",
    moves: ROW_DOWN,
};

impl Rows {
    fn at(&self, row: usize, col: usize, shifted: bool) -> Option<char> {
        let (base, shift) = self.rows[row];
        let s = if shifted { shift } else { base };
        s.chars().nth(col).filter(|c| *c != ' ')
    }

    /// Where `c` sits: (row, column, with Shift).
    fn find(&self, c: char) -> Option<(usize, usize, bool)> {
        self.rows
            .iter()
            .enumerate()
            .find_map(|(row, (base, shift))| {
                base.chars()
                    .position(|x| x == c)
                    .map(|col| (row, col, false))
                    .or_else(|| {
                        shift
                            .chars()
                            .position(|x| x == c)
                            .map(|col| (row, col, true))
                    })
            })
    }

    fn types(&self, c: char) -> bool {
        c != ' '
            && !self.dead.contains(c)
            && (self.find(c).is_some() || self.iso.is_some_and(|(a, b)| a == c || b == c))
    }

    /// A key a moved shortcut can use: typeable, and not an ASCII letter (those are the
    /// letter shortcuts' own).
    fn usable(&self, c: char) -> bool {
        self.types(c) && !c.is_ascii_alphabetic()
    }
}

impl KeyboardLayout {
    pub const ALL: [KeyboardLayout; 9] = [
        KeyboardLayout::Us,
        KeyboardLayout::Uk,
        KeyboardLayout::German,
        KeyboardLayout::SwissGerman,
        KeyboardLayout::French,
        KeyboardLayout::Spanish,
        KeyboardLayout::Swedish,
        KeyboardLayout::Norwegian,
        KeyboardLayout::Danish,
    ];

    /// The persisted name (`de`), also in element names.
    pub fn id(self) -> &'static str {
        match self {
            KeyboardLayout::Us => "us",
            KeyboardLayout::Uk => "uk",
            KeyboardLayout::German => "de",
            KeyboardLayout::SwissGerman => "ch",
            KeyboardLayout::French => "fr",
            KeyboardLayout::Spanish => "es",
            KeyboardLayout::Swedish => "se",
            KeyboardLayout::Norwegian => "no",
            KeyboardLayout::Danish => "dk",
        }
    }

    /// The name in settings and pickers.
    pub fn name(self) -> &'static str {
        match self {
            KeyboardLayout::Us => "US (as written)",
            KeyboardLayout::Uk => "UK",
            KeyboardLayout::German => "German (QWERTZ)",
            KeyboardLayout::SwissGerman => "Swiss German",
            KeyboardLayout::French => "French (AZERTY)",
            KeyboardLayout::Spanish => "Spanish",
            KeyboardLayout::Swedish => "Swedish / Finnish",
            KeyboardLayout::Norwegian => "Norwegian",
            KeyboardLayout::Danish => "Danish",
        }
    }

    /// The layout's keys; `mac`: as macOS lays them out (only French differs in a way that
    /// matters here).
    fn rows(self, mac: bool) -> &'static Rows {
        match self {
            KeyboardLayout::Us => &US,
            KeyboardLayout::Uk => &UK,
            KeyboardLayout::German => &GERMAN,
            KeyboardLayout::SwissGerman => &SWISS_GERMAN,
            KeyboardLayout::French if mac => &FRENCH_MAC,
            KeyboardLayout::French => &FRENCH,
            KeyboardLayout::Spanish => &SPANISH,
            KeyboardLayout::Swedish => &SWEDISH,
            KeyboardLayout::Norwegian => &NORWEGIAN,
            KeyboardLayout::Danish => &DANISH,
        }
    }

    /// Whether the layout types `c` with one key (Shift allowed, no AltGr / Option, no
    /// dead key).
    pub fn types(self, c: char, mac: bool) -> bool {
        self.rows(mac).types(c)
    }

    /// A binding's key (gpui syntax `secondary-shift-[`, or a terminal's `ctrl-x`) on this
    /// layout, for this OS (see [`KeyboardLayout::remap_for`]).
    pub fn remap(self, key: &str) -> Cow<'_, str> {
        self.remap_for(key, cfg!(target_os = "macos"))
    }

    /// A binding's key on this layout: unchanged unless its character can't be typed
    /// directly here, then the character at its US position (see the module docs). Named
    /// keys (`pageup`, `f10`), letters and modifiers stay; `shift-` goes into the character
    /// (`secondary-shift-[` → `secondary-Ö` on German).
    pub fn remap_for(self, key: &str, mac: bool) -> Cow<'_, str> {
        if self == KeyboardLayout::Us {
            return Cow::Borrowed(key);
        }
        let (prefix, k) = split_key(key);
        let mut chars = k.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return Cow::Borrowed(key);
        };
        if c.is_ascii_alphabetic() {
            return Cow::Borrowed(key);
        }
        let shift = prefix.split('-').any(|m| m == "shift");
        // The US character the binding means: `shift-]` is `}`.
        let us = match US.find(c) {
            Some((row, col, false)) if shift => US.at(row, col, true).unwrap_or(c),
            _ => c,
        };
        let rows = self.rows(mac);
        if rows.types(us) {
            return Cow::Borrowed(key);
        }
        let Some((mut row, mut col, level)) = US.find(us) else {
            return Cow::Borrowed(key);
        };
        let base = US.at(row, col, false).unwrap_or(us);
        if let Some((_, to)) = rows.moves.iter().find(|(from, _)| *from == base)
            && let Some((r, c, _)) = US.find(*to)
        {
            (row, col) = (r, c);
        }
        let Some(target) = [level, !level]
            .into_iter()
            .filter_map(|l| rows.at(row, col, l))
            .find(|t| rows.usable(*t))
        else {
            return Cow::Borrowed(key);
        };
        let mods: String = prefix
            .split_inclusive('-')
            .filter(|m| *m != "shift-")
            .collect();
        Cow::Owned(format!("{mods}{target}"))
    }

    /// The character a single-character binding becomes (gitty's own keys: `]` in the merge
    /// view, `=`).
    pub fn char_for(self, us: char) -> char {
        let s = us.to_string();
        let mapped = self.remap(&s);
        mapped.chars().next().unwrap_or(us)
    }

    /// The layout of the system's current keyboard, from the id gpui reports (macOS input
    /// source `com.apple.keylayout.German`, a Windows KLID `00000407`, an xkb layout `de`),
    /// for a one-time hint; `None` = US or not one of ours.
    pub fn from_system_id(id: &str) -> Option<KeyboardLayout> {
        use KeyboardLayout::*;
        let id = id.trim();
        let mac = id.strip_prefix("com.apple.keylayout.");
        let found = match mac {
            Some(name) => match name {
                "German" | "Austrian" | "German-DIN-2137" => German,
                "SwissGerman" => SwissGerman,
                "French" | "French-PC" | "French-numerical" => French,
                "British" | "British-PC" => Uk,
                "Spanish" | "Spanish-ISO" => Spanish,
                "Swedish" | "Swedish-Pro" | "Finnish" | "FinnishExtended" => Swedish,
                "Norwegian" | "NorwegianExtended" => Norwegian,
                "Danish" => Danish,
                _ => return None,
            },
            None => match id.to_ascii_lowercase().as_str() {
                // Windows keyboard layout ids (KLID).
                "00000407" | "00000c07" | "de" | "at" => German,
                "00000807" | "ch" => SwissGerman,
                "0000040c" | "fr" => French,
                "00000809" | "gb" => Uk,
                "0000040a" | "00000c0a" | "es" => Spanish,
                "0000041d" | "0000040b" | "se" | "fi" => Swedish,
                "00000414" | "no" => Norwegian,
                "00000406" | "dk" => Danish,
                _ => return None,
            },
        };
        Some(found)
    }
}

/// `secondary-shift-[` → (`secondary-shift-`, `[`); `ctrl--` → (`ctrl-`, `-`).
fn split_key(key: &str) -> (&str, &str) {
    if key == "-" {
        return ("", key);
    }
    if let Some(prefix) = key.strip_suffix("--") {
        return (&key[..prefix.len() + 1], "-");
    }
    match key.rfind('-') {
        Some(i) if i + 1 < key.len() => (&key[..=i], &key[i + 1..]),
        _ => ("", key),
    }
}

/// [`super::lookup`] on `layout`: `key` as typed (a terminal's `ö`) finds the binding the
/// layout moved there.
pub fn lookup_in(layout: KeyboardLayout, key: &str, scope: Scope) -> Option<Command> {
    lookup_for(layout, cfg!(target_os = "macos"), key, scope)
}

fn lookup_for(layout: KeyboardLayout, mac: bool, key: &str, scope: Scope) -> Option<Command> {
    BINDINGS
        .iter()
        .find(|b| {
            same_key(&layout.remap_for(b.key, mac), key)
                && (b.scope == scope
                    || b.scope == Scope::Global
                    || (b.scope == Scope::Panes && scope != Scope::Global))
        })
        .map(|b| b.command)
}

/// Keys the apps bind besides [`BINDINGS`] with a character a layout may move (gitti's tab /
/// zoom / settings keys, the merge editor's, gitty's `=` and `.`) — for the test that every
/// layout can type them all.
pub const OTHER_APP_KEYS: &[&str] = &[
    "secondary-,",
    "secondary-shift-]",
    "secondary-shift-[",
    "secondary-=",
    "secondary-+",
    "secondary--",
    "secondary-0",
    "secondary-alt-=",
    "secondary-alt-+",
    "secondary-alt--",
    "]",
    "[",
    "?",
    "=",
    ".",
    "0",
    "1",
    "2",
    "3",
    "4",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{NAV_BINDINGS, graph_lookup};

    fn key_char(key: &str) -> Option<char> {
        let (_, k) = split_key(key);
        let mut it = k.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        }
    }

    #[gitti_test::test]
    fn german_moves_only_what_it_cannot_type() {
        let de = |k: &str| KeyboardLayout::German.remap_for(k, false).into_owned();
        assert_eq!(de("["), "ö");
        assert_eq!(de("]"), "ä");
        assert_eq!(de("secondary-shift-["), "secondary-Ö");
        assert_eq!(de("secondary-shift-]"), "secondary-Ä");
        for kept in [
            "?",
            ":",
            "+",
            "-",
            "secondary--",
            "secondary-=",
            "secondary-,",
            "s",
            "z",
        ] {
            assert_eq!(de(kept), kept);
        }
        assert_eq!(de("pageup"), "pageup");
        assert_eq!(de("ctrl-alt-shift-up"), "ctrl-alt-shift-up");
        // macOS German is the same here.
        assert_eq!(KeyboardLayout::German.remap_for("]", true), "ä");
        assert_eq!(KeyboardLayout::German.char_for('['), 'ö');
        assert_eq!(KeyboardLayout::Us.remap_for("[", false), "[");
        assert_eq!(
            KeyboardLayout::Uk.remap_for("secondary-shift-[", false),
            "secondary-shift-["
        );
    }

    #[gitti_test::test]
    fn other_layouts_place_the_brackets() {
        let cases: &[(KeyboardLayout, bool, [&str; 4])] = &[
            (
                KeyboardLayout::SwissGerman,
                false,
                ["ö", "ä", "secondary-é", "secondary-à"],
            ),
            (
                KeyboardLayout::French,
                false,
                ["ù", "$", "secondary-%", "secondary-£"],
            ),
            (
                KeyboardLayout::French,
                true,
                ["ù", "$", "secondary-%", "secondary-*"],
            ),
            (
                KeyboardLayout::Spanish,
                false,
                ["ñ", "ç", "secondary-Ñ", "secondary-Ç"],
            ),
            (
                KeyboardLayout::Swedish,
                false,
                ["ö", "ä", "secondary-Ö", "secondary-Ä"],
            ),
            (
                KeyboardLayout::Norwegian,
                false,
                ["ø", "æ", "secondary-Ø", "secondary-Æ"],
            ),
            (
                KeyboardLayout::Danish,
                false,
                ["æ", "ø", "secondary-Æ", "secondary-Ø"],
            ),
        ];
        for (layout, mac, want) in cases {
            let got = ["[", "]", "secondary-shift-[", "secondary-shift-]"]
                .map(|k| layout.remap_for(k, *mac).into_owned());
            assert_eq!(got, *want, "{layout:?} mac={mac}");
        }
    }

    /// Every key both apps bind is typeable on every layout (with or without Shift, no AltGr
    /// / Option, no dead key), and the moved keys collide with nothing.
    #[gitti_test::test]
    fn every_layout_types_every_binding_without_clashes() {
        for layout in KeyboardLayout::ALL {
            for mac in [false, true] {
                let rows = layout.rows(mac);
                let remap = |k: &str| layout.remap_for(k, mac).into_owned();
                let all = BINDINGS
                    .iter()
                    .map(|b| b.key)
                    .chain(OTHER_APP_KEYS.iter().copied());
                for key in all {
                    let mapped = remap(key);
                    if let Some(c) = key_char(&mapped) {
                        assert!(
                            c.is_ascii_alphabetic() || rows.types(c),
                            "{layout:?} mac={mac}: {key} → {mapped} can't be typed"
                        );
                    }
                }
                // The shared table after the move: still one command per key and scope.
                for (i, a) in BINDINGS.iter().enumerate() {
                    let ka = remap(a.key);
                    for other in &BINDINGS[i + 1..] {
                        let wide = |s: Scope| matches!(s, Scope::Panes | Scope::Global);
                        let overlap = a.scope == other.scope || wide(a.scope) || wide(other.scope);
                        assert!(
                            !(overlap && ka == remap(other.key)),
                            "{layout:?} mac={mac}: {} and {} both on {ka}",
                            a.key,
                            other.key
                        );
                    }
                    // No list / graph key of a pane either.
                    if ka != a.key {
                        assert!(
                            NAV_BINDINGS.iter().all(|n| !same_key(n.key, &ka)),
                            "{layout:?}: {ka} is a list key"
                        );
                        assert_eq!(graph_lookup(&ka), None);
                    }
                    let typed = crate::keys::terminal_key(&ka);
                    assert_eq!(lookup_for(layout, mac, &typed, a.scope), Some(a.command));
                }
                // gitty's own `=` (more context) and `.` (menu) don't land on a shared key.
                for key in ["=", "."] {
                    let m = remap(key);
                    assert_eq!(
                        lookup_for(layout, mac, &m, Scope::Diff),
                        None,
                        "{layout:?} {m}"
                    );
                }
            }
        }
    }

    #[gitti_test::test]
    fn layouts_are_well_formed_and_detected() {
        for layout in KeyboardLayout::ALL {
            for mac in [false, true] {
                let rows = layout.rows(mac);
                for (i, (base, shift)) in rows.rows.iter().enumerate() {
                    let (ub, us) = US.rows[i];
                    assert_eq!(
                        base.chars().count(),
                        ub.chars().count(),
                        "{layout:?} row {i}"
                    );
                    assert_eq!(
                        shift.chars().count(),
                        us.chars().count(),
                        "{layout:?} row {i}"
                    );
                }
                // Letters are where a typist expects them: all 26, one key each.
                for c in 'a'..='z' {
                    assert!(rows.types(c), "{layout:?} lacks {c}");
                }
            }
            let json = serde_json::to_string(&layout).unwrap();
            assert_eq!(json, format!("\"{}\"", layout.id()));
            assert_eq!(
                serde_json::from_str::<KeyboardLayout>(&json).ok(),
                Some(layout)
            );
        }
        assert_eq!(
            KeyboardLayout::from_system_id("com.apple.keylayout.German"),
            Some(KeyboardLayout::German)
        );
        assert_eq!(
            KeyboardLayout::from_system_id("00000407"),
            Some(KeyboardLayout::German)
        );
        assert_eq!(
            KeyboardLayout::from_system_id("com.apple.keylayout.US"),
            None
        );
        assert_eq!(KeyboardLayout::from_system_id("00000409"), None);
        assert_eq!(split_key("secondary--"), ("secondary-", "-"));
        assert_eq!(split_key("-"), ("", "-"));
        assert_eq!(split_key("ctrl-shift-]"), ("ctrl-shift-", "]"));
        assert_eq!(split_key("pageup"), ("", "pageup"));
    }
}
