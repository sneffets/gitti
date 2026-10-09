//! The context key bar: what the bottom line offers for the focused pane and its selected
//! thing, pine-style — the pane's own keys (⏎ …), then **every entry of the selection's
//! context menu**, then the keys that always work; `o` pages through what doesn't fit
//! ("o more 1/3").
//!
//! The menu entries come from the frontend's own context-menu builders (the same item
//! list the `m` menu shows, so the bar and the menu can't drift); this module decides how
//! each one is reached and lays the hints out:
//!
//! - an entry's key acts **directly** in the pane when the pane gives that key no other
//!   meaning ([`reach`]): `d Delete…` on a branch in the sidebar;
//! - otherwise it is reached **through the menu**, shown as `m c` (press `m`, then `c`):
//!   `c` in the sidebar stays "write the commit message", so "Checkout" is `m c`;
//! - an entry without a key of its own gets one in the menu ([`auto_keys`]: a letter of
//!   its label that the menu leaves free) and shows as `m e`; such a key never acts
//!   directly in the pane (it depends on the menu's other entries).
//!
//! An entry may say that its key's command in the pane *is* the entry ([`MenuEntry::same_as`],
//! e.g. "Stage All" = `S`): then the key is direct although the table binds it.
//!
//! Everything here is computed from in-memory data — no git, no I/O.

use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use crate::keys::{self, Command, KeyboardLayout, NavScope, Scope};

/// The key that opens the focused thing's context menu.
pub const MENU_KEY: char = 'm';
/// The key that pages the bar (pine's "O OTHER CMDS"; `m` is the menu).
pub const MORE_KEY: char = 'o';
/// Columns between two hints.
pub const GAP: usize = 3;
/// Longer labels are cut with "…" (branch names can be long).
pub const MAX_LABEL: usize = 40;

/// The focused pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    Sidebar,
    Commits,
    Diff,
    Log,
    /// The commit message editor: letters type, nothing is direct, no paging.
    Message,
}

impl Pane {
    /// The scope of the shared key table ([`keys::BINDINGS`]) in this pane.
    pub fn scope(self) -> Scope {
        match self {
            Pane::Commits => Scope::Commits,
            Pane::Diff => Scope::Diff,
            Pane::Message => Scope::Global,
            Pane::Sidebar | Pane::Log => Scope::Panes,
        }
    }

    fn nav(self) -> Option<NavScope> {
        match self {
            Pane::Sidebar => Some(NavScope::Sidebar),
            Pane::Commits => Some(NavScope::Commits),
            Pane::Diff => Some(NavScope::Diff),
            Pane::Log => Some(NavScope::Log),
            Pane::Message => None,
        }
    }

    /// Whether `o` pages the bar here (not while typing).
    pub fn pages(self) -> bool {
        self != Pane::Message
    }
}

/// What the focus is on: picks the pane's own hints in front of the menu's entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    /// A sidebar section header (BRANCHES, REMOTES, …).
    Section,
    Branch,
    CurrentBranch,
    RemoteBranch,
    Remote,
    Tag,
    Stash,
    Worktree,
    Submodule,
    /// The commit list's working-directory row.
    WorkingDir,
    Commit,
    /// The working directory's diff (staging keys).
    Changes,
    /// A commit's (or comparison's) diff: read-only.
    CommitDiff,
    /// A conflicted file in the working directory's diff.
    Conflicted,
    /// Nothing selected (empty list).
    Nothing,
}

/// A context-menu entry as the bar takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuEntry<'a> {
    /// Its key inside the menu.
    pub key: Option<char>,
    pub label: &'a str,
    /// The shared command the entry's key already runs in this pane when that command
    /// does the same as the entry (then the key is direct).
    pub same_as: Option<Command>,
    /// `false`: shown, but dim (it can't run now; the label says why).
    pub enabled: bool,
    /// The key was given by [`auto_keys`]: it works only inside the menu.
    pub menu_only: bool,
}

/// What the frontend adds to the shared tables: the keyboard layout its shortcuts follow
/// (a key the layout moved onto a character counts as taken there) and its own
/// single keys in the panes (gitty: `m . o q v …`).
#[derive(Debug, Clone, Copy)]
pub struct Frontend<'a> {
    pub layout: KeyboardLayout,
    pub own_keys: &'a [char],
}

/// How a menu entry's key is reached from the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Pressing the key in the pane runs the entry.
    Direct,
    /// `m`, then the key.
    Menu,
}

/// One hint of the bar: `key` (bright) and `label` (faint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub key: String,
    pub label: String,
    pub dim: bool,
}

impl Hint {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Hint {
            key: key.into(),
            label: label.into(),
            dim: false,
        }
    }

    /// Terminal columns: key, a space, label.
    pub fn width(&self) -> usize {
        self.key.width() + 1 + self.label.width()
    }
}

/// How `key`, an entry's menu key, is reached in `pane`. Direct when nothing else in the
/// pane uses it: not a key of the shared table there on the frontend's layout (unless its
/// command is `same_as`), not a list movement, not one of the frontend's own single keys,
/// never in the message editor.
pub fn reach(key: char, pane: Pane, same_as: Option<Command>, fe: &Frontend) -> Reach {
    if pane == Pane::Message || key == MENU_KEY || key == MORE_KEY || fe.own_keys.contains(&key) {
        return Reach::Menu;
    }
    let name = keys::key_name(key, false, false);
    if let Some(cmd) = keys::lookup_in(fe.layout, &name, pane.scope()) {
        return if same_as == Some(cmd) {
            Reach::Direct
        } else {
            Reach::Menu
        };
    }
    let nav = match key {
        ' ' => "space".to_string(),
        _ => name,
    };
    if pane.nav().is_some_and(|scope| keys::nav_lookup(&nav, scope).is_some()) {
        return Reach::Menu;
    }
    Reach::Direct
}

/// The pane's own keys for `ctx` (in front of the menu's entries).
pub fn pane_hints(pane: Pane, ctx: Context) -> Vec<Hint> {
    let h = Hint::new;
    match (pane, ctx) {
        (Pane::Log, _) => vec![h("↑↓", "scroll"), h("g G", "oldest / newest"), h("esc", "back")],
        (Pane::Message, _) => vec![
            h("ctrl-s", "commit"),
            h("esc", "leave"),
            h("esc A", "toggle amend"),
        ],
        (_, Context::Nothing) => Vec::new(),
        (Pane::Sidebar, Context::Section) => vec![h("⏎", "fold")],
        (Pane::Sidebar, Context::Branch | Context::RemoteBranch) => vec![h("⏎", "checkout")],
        (Pane::Sidebar, Context::Submodule) => vec![h("⏎", "open")],
        (Pane::Sidebar, _) => vec![h("⏎", "show")],
        (Pane::Commits, Context::WorkingDir) => vec![h("⏎", "changes"), h("c", "message")],
        (Pane::Commits, _) => vec![h("⏎", "diff")],
        (Pane::Diff, Context::Changes) => vec![
            h("space", "line"),
            h("⏎", "hunk / fold"),
            h("s", "stage"),
            h("u", "unstage"),
            h("d", "discard"),
            h("n N", "hunks"),
            h("] [", "files"),
        ],
        (Pane::Diff, Context::Conflicted) => {
            vec![h("⏎", "resolve"), h("n N", "hunks"), h("] [", "files")]
        }
        (Pane::Diff, _) => vec![
            h("n N", "hunks"),
            h("] [", "files"),
            h("⏎", "fold"),
            h("y", "copy"),
            h("←→", "pan"),
        ],
    }
}

/// The keys that always work in `pane` (after the menu's entries). `menu`: whether `m`
/// opens a menu here.
pub fn global_hints(pane: Pane, menu: bool) -> Vec<Hint> {
    let h = Hint::new;
    if pane == Pane::Message {
        return vec![h("tab", "diff")];
    }
    let mut v = Vec::new();
    if menu {
        v.push(Hint::new(MENU_KEY.to_string(), "menu"));
    }
    v.extend([
        h("tab", "next pane"),
        h("f p P", "fetch pull push"),
        h("ctrl-p", "commands"),
        h("?", "keys"),
    ]);
    v
}

/// The keys a menu itself uses (close / move up and down): never given automatically.
pub const MENU_OWN_KEYS: &[char] = &[MENU_KEY, 'q', 'j', 'k'];

/// Keys for the menu entries that have none: `entries` = per entry its own key and its
/// label, `None` for separators / notes. An entry without a key gets the first free one
/// of: its words' initials, the other letters of its label (both lower case; a greyed
/// entry's "(reason)" doesn't count), `a`–`z`, `1`–`9`, `A`–`Z` — free = no other entry
/// has it and the menu doesn't use it ([`MENU_OWN_KEYS`]). The result has one element
/// per entry: the key given, or `None` (own key, not selectable, or nothing left).
/// The same labels in the same order always get the same keys.
pub fn auto_keys(entries: &[Option<(Option<char>, &str)>]) -> Vec<Option<char>> {
    let mut taken: Vec<char> = MENU_OWN_KEYS.to_vec();
    taken.extend(entries.iter().flatten().filter_map(|(k, _)| *k));
    entries
        .iter()
        .map(|e| {
            let (None, label) = (*e)? else {
                return None;
            };
            let label = label.split(" (").next().unwrap_or(label).to_lowercase();
            let initials = label
                .split(|c: char| !c.is_alphanumeric())
                .filter_map(|w| w.chars().next());
            let key = initials
                .chain(label.chars())
                .filter(char::is_ascii_lowercase)
                .chain('a'..='z')
                .chain('1'..='9')
                .chain('A'..='Z')
                .find(|c| !taken.contains(c))?;
            taken.push(key);
            Some(key)
        })
        .collect()
}

/// One menu entry as a hint: its key as reached from `pane` ([`reach`]).
pub fn entry_hint(e: &MenuEntry, pane: Pane, fe: &Frontend) -> Hint {
    let key = match e.key {
        Some(k) if e.menu_only => format!("{MENU_KEY} {k}"),
        Some(k) => match reach(k, pane, e.same_as, fe) {
            Reach::Direct => k.to_string(),
            Reach::Menu => format!("{MENU_KEY} {k}"),
        },
        // Only when a menu has more entries than keys.
        None => format!("{MENU_KEY} ↑↓"),
    };
    Hint {
        key,
        label: clip(e.label, MAX_LABEL),
        dim: !e.enabled,
    }
}

/// The whole bar for `ctx` in `pane`: the pane's keys, every menu entry in menu order,
/// the keys that always work. Separators / notes (entries without a label) are skipped.
pub fn bar(pane: Pane, ctx: Context, menu: &[MenuEntry], fe: &Frontend) -> Vec<Hint> {
    let mut v = pane_hints(pane, ctx);
    v.extend(
        menu.iter()
            .filter(|e| !e.label.trim().is_empty())
            .map(|e| entry_hint(e, pane, fe)),
    );
    v.extend(global_hints(pane, !menu.is_empty()));
    v
}

/// "more 2/3" (after the key [`MORE_KEY`]).
pub fn more_label(page: usize, pages: usize) -> String {
    format!("more {}/{pages}", page + 1)
}

/// Width of the paging hint for `pages` pages ("o more 10/12" at most).
pub fn more_width(pages: usize) -> usize {
    1 + 1 + more_label(pages.saturating_sub(1), pages).width()
}

/// All `n` hints on one page.
pub fn one_page(n: usize) -> Vec<Range<usize>> {
    std::iter::once(0..n).collect()
}

/// Splits hints of `widths` columns into pages of at most `avail` columns, [`GAP`]
/// between hints. All in one page when they fit, else every page keeps room for the
/// paging hint at its end — also on a short line: more pages rather than cut keys. A
/// hint wider than a page gets a page of its own, its label cut ([`fit`]). Never empty
/// for a non-empty input.
pub fn paginate(widths: &[usize], avail: usize) -> Vec<Range<usize>> {
    if widths.is_empty() {
        return Vec::new();
    }
    let total: usize = widths.iter().sum::<usize>() + GAP * (widths.len() - 1);
    if total <= avail {
        return one_page(widths.len());
    }
    // The paging hint's width depends on the page count: try with 9 pages' width, then
    // with more if that is exceeded (two digits).
    let mut reserve = more_width(9) + GAP;
    loop {
        let pages = pack(widths, avail.saturating_sub(reserve));
        let need = more_width(pages.len()) + GAP;
        if need <= reserve {
            return pages;
        }
        reserve = need;
    }
}

fn pack(widths: &[usize], room: usize) -> Vec<Range<usize>> {
    let mut pages = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (i, &w) in widths.iter().enumerate() {
        let add = if i == start { w } else { GAP + w };
        if i > start && used + add > room {
            pages.push(start..i);
            start = i;
            used = w;
        } else {
            used += add;
        }
    }
    pages.push(start..widths.len());
    pages
}

/// The page to show: `page` wrapped into `0..pages` (0 without pages).
pub fn wrap_page(page: usize, pages: usize) -> usize {
    if pages == 0 { 0 } else { page % pages }
}

/// `h` in at most `room` columns: the label cut with "…" — never the key, which is
/// what the hint is for.
pub fn fit(h: &Hint, room: usize) -> Hint {
    let mut h = h.clone();
    if h.width() > room {
        h.label = clip(&h.label, room.saturating_sub(h.key.width() + 1).max(1));
    }
    h
}

/// `s` cut to `max` columns with "…".
pub fn clip(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: &[char] = &['m', '.', '=', 'q', 'o', 'v', 'h', 'l', 'J', 'K', ' '];
    const FE: Frontend = Frontend {
        layout: KeyboardLayout::Us,
        own_keys: OWN,
    };

    fn entry(key: Option<char>, label: &str) -> MenuEntry<'_> {
        MenuEntry {
            key,
            label,
            same_as: None,
            enabled: true,
            menu_only: false,
        }
    }

    #[gitti_test::test]
    fn free_keys_are_direct_taken_keys_go_through_the_menu() {
        // Sidebar: d / y / a / u / s are free, c f p P r t e b are shared keys.
        for k in ['d', 'y', 'a', 'u', 's', 'i'] {
            assert_eq!(reach(k, Pane::Sidebar, None, &FE), Reach::Direct, "{k}");
        }
        for k in ['c', 'f', 'p', 'P', 'r', 't', 'e', 'b', 'z', 'C', 'L', 'j', 'k', 'g', 'G'] {
            assert_eq!(reach(k, Pane::Sidebar, None, &FE), Reach::Menu, "{k}");
        }
        // The frontend's own keys and the menu / paging keys.
        for k in ['m', 'o', 'q', 'v', ' '] {
            assert_eq!(reach(k, Pane::Sidebar, None, &FE), Reach::Menu, "{k}");
        }
        // Diff: s u d y n are the staging / copy / hunk keys there, free in the list.
        for k in ['s', 'u', 'd', 'y', 'n', 'N', ']', 'x', 'X'] {
            assert_eq!(reach(k, Pane::Diff, None, &FE), Reach::Menu, "{k}");
        }
        assert_eq!(reach('y', Pane::Commits, None, &FE), Reach::Direct);
        assert_eq!(reach('i', Pane::Diff, None, &FE), Reach::Direct);
        // The message editor types every letter.
        assert_eq!(reach('d', Pane::Message, None, &FE), Reach::Menu);
        // A key the layout moved onto a character takes that character.
        let german = Frontend {
            layout: KeyboardLayout::German,
            own_keys: OWN,
        };
        let next_file = KeyboardLayout::German.char_for(']');
        if next_file != ']' {
            assert_eq!(reach(next_file, Pane::Diff, None, &german), Reach::Menu);
            assert_eq!(reach(next_file, Pane::Diff, None, &FE), Reach::Direct);
        }
    }

    #[gitti_test::test]
    fn an_entry_that_is_its_keys_command_is_direct() {
        assert_eq!(reach('S', Pane::Commits, Some(Command::StageAll), &FE), Reach::Direct);
        assert_eq!(reach('S', Pane::Commits, None, &FE), Reach::Menu);
        // Another command on that key: through the menu.
        assert_eq!(reach('S', Pane::Commits, Some(Command::UnstageAll), &FE), Reach::Menu);
        assert_eq!(reach('f', Pane::Sidebar, Some(Command::Fetch), &FE), Reach::Direct);
    }

    /// Every key a pane's shared table or list movement uses goes through the menu — the
    /// bar never promises a key the pane gives to something else.
    #[gitti_test::test]
    fn no_direct_key_clashes_with_the_tables() {
        for pane in [Pane::Sidebar, Pane::Commits, Pane::Diff, Pane::Log] {
            for ch in (b'!'..=b'~').map(char::from) {
                if reach(ch, pane, None, &FE) != Reach::Direct {
                    continue;
                }
                let name = keys::key_name(ch, false, false);
                assert_eq!(keys::lookup(&name, pane.scope()), None, "{ch} in {pane:?}");
                assert_eq!(keys::nav_lookup(&name, pane.nav().unwrap()), None, "{ch} in {pane:?}");
            }
        }
    }

    /// The bar holds every labelled menu entry, in menu order, between the pane's own
    /// keys and the global ones.
    #[gitti_test::test]
    fn the_bar_lists_the_menu_entries_in_order() {
        let menu = [
            entry(Some('f'), "Fetch origin"),
            MenuEntry {
                menu_only: true,
                ..entry(Some('p'), "Prune Stale Branches of origin")
            },
            entry(None, ""),
            entry(Some('y'), "Copy URL"),
            entry(Some('a'), "Add Remote…"),
            MenuEntry {
                enabled: false,
                ..entry(Some('d'), "Remove… (busy)")
            },
        ];
        let bar = bar(Pane::Sidebar, Context::Remote, &menu, &FE);
        let shown: Vec<(&str, &str)> = bar.iter().map(|h| (h.key.as_str(), h.label.as_str())).collect();
        let own = pane_hints(Pane::Sidebar, Context::Remote).len();
        assert_eq!(
            &shown[own..own + 5],
            &[
                ("m f", "Fetch origin"),
                ("m p", "Prune Stale Branches of origin"),
                ("y", "Copy URL"),
                ("a", "Add Remote…"),
                ("d", "Remove… (busy)"),
            ],
        );
        assert!(bar[own + 4].dim);
        assert_eq!(shown[0], ("⏎", "show"));
        assert!(shown.contains(&("m", "menu")) && shown.ends_with(&[("?", "keys")]));
        // Without a menu: no "m menu".
        let none = super::bar(Pane::Log, Context::Nothing, &[], &FE);
        assert!(!none.iter().any(|h| h.label == "menu"));
    }

    #[gitti_test::test]
    fn pages_cover_every_hint_once_and_fit() {
        let widths: Vec<usize> = (0..30).map(|i| 6 + (i * 7) % 15).collect();
        for avail in [40, 80, 120, 1000] {
            let pages = paginate(&widths, avail);
            // Contiguous, in order, all hints.
            let mut next = 0;
            for p in &pages {
                assert_eq!(p.start, next, "{avail}: {pages:?}");
                assert!(!p.is_empty());
                next = p.end;
                let w: usize = widths[p.clone()].iter().sum::<usize>() + GAP * (p.len() - 1);
                let room = if pages.len() == 1 {
                    avail
                } else {
                    avail - more_width(pages.len()) - GAP
                };
                assert!(w <= room || p.len() == 1, "{avail}: page {p:?} is {w} > {room}");
            }
            assert_eq!(next, widths.len());
            if avail == 1000 {
                assert_eq!(pages, vec![0..30]);
            }
        }
        assert!(paginate(&[], 80).is_empty());
        // A short line pages too (one hint each), it never cuts hints away.
        assert_eq!(paginate(&[9, 9, 9], 20), vec![0..1, 1..2, 2..3]);
        // A hint wider than the line: its own page.
        assert_eq!(paginate(&[100, 5], 50), vec![0..1, 1..2]);
    }

    #[gitti_test::test]
    fn paging_wraps_around() {
        assert_eq!(wrap_page(3, 3), 0);
        assert_eq!(wrap_page(4, 3), 1);
        assert_eq!(wrap_page(5, 0), 0);
        assert_eq!(more_label(0, 3), "more 1/3");
        assert_eq!(more_width(12), "o more 12/12".len());
    }

    /// Every selectable entry gets a key of its own (no bare `m ·`, which reads as a cut
    /// key): initials first, never a key of another entry or of the
    /// menu itself.
    #[gitti_test::test]
    fn entries_without_a_key_get_a_free_one() {
        let entries = [
            Some((Some('c'), "Checkout")),
            Some((None, "Create Empty Commit")),
            None,
            Some((None, "Commit")),
            Some((None, "Stage All and Commit (no changes)")),
            Some((Some('j'), "Jump")),
            Some((None, "Keep")),
        ];
        let keys = auto_keys(&entries);
        assert_eq!(
            keys,
            [None, Some('e'), None, Some('o'), Some('s'), None, Some('p')]
        );
        // Enough entries to use up the letters: digits, then capitals, then nothing.
        let labels: Vec<String> = (0..80).map(|i| format!("Entry {i}")).collect();
        let many: Vec<_> = labels.iter().map(|l| Some((None, l.as_str()))).collect();
        let keys = auto_keys(&many);
        let given: Vec<char> = keys.iter().flatten().copied().collect();
        assert_eq!(given.len(), 22 + 9 + 26);
        assert!(given.iter().all(|k| !MENU_OWN_KEYS.contains(k)));
        let mut unique = given.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), given.len());
        assert_eq!(keys[79], None);
    }

    #[gitti_test::test]
    fn a_hint_too_wide_keeps_its_key() {
        let h = Hint::new("m e", "Create Empty Commit");
        let f = fit(&h, 10);
        assert_eq!(f.key, "m e");
        assert_eq!(f.width(), 10);
        assert!(f.label.ends_with('…'));
        assert_eq!(fit(&h, 80), h);
    }

    #[gitti_test::test]
    fn long_labels_are_cut() {
        assert_eq!(clip("Checkout main", 40), "Checkout main");
        let long = format!("Checkout {}", "x".repeat(60));
        let c = clip(&long, 20);
        assert_eq!(c.width(), 20);
        assert!(c.ends_with('…'));
    }
}
