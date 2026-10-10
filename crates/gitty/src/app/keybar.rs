//! The status line's context key hints (`gitti_core::keybar`): the focused pane's
//! keys, every entry of the focused thing's context menu (the same items `m` opens, so
//! the two can't drift), the keys that always work. `o` pages through them; the page
//! starts over when the hints change (another selection, another pane). An entry whose
//! key is free in the pane acts directly ([`App::keybar_key`]); the others show as
//! `m <key>`. Computed from in-memory state while drawing — no git.

use std::hash::{Hash, Hasher};

use gitti_core::keybar::{self, Context, Frontend, Hint, MenuEntry, Pane, Reach};
use gitti_core::rows::{DiffRow, Src};

use super::{App, Detail, Focus};
use crate::sidebar::SideItem;

/// gitty's own single keys in the panes (besides the shared table and the list keys):
/// menu `m .`, paging `o`, context `=`, quit `q`, the running operation's output `v`,
/// pane / pan / selection keys `h l J K space`. A menu entry with one of them is reached
/// through the menu. `.` and `=` as the keyboard layout types them.
fn own_keys() -> [char; 11] {
    let kb = crate::kb::ch;
    ['m', kb('.'), 'o', kb('='), 'q', 'v', 'h', 'l', 'J', 'K', ' ']
}

/// [`own_keys`] and the layout, for `gitti_core::keybar`.
fn with_frontend<T>(f: impl FnOnce(&Frontend) -> T) -> T {
    let own = own_keys();
    f(&Frontend {
        layout: crate::kb::layout(),
        own_keys: &own,
    })
}

/// The bar's page and what it was computed for.
#[derive(Debug, Default)]
pub struct KeybarState {
    /// Hash of the hints the page belongs to; other hints start at page 0.
    ctx: u64,
    pub page: usize,
    /// Pages at the last draw.
    pub pages: usize,
    /// The bar was drawn (no status text, no operation over it).
    pub shown: bool,
}

impl KeybarState {
    /// Called by the renderer with the hints it shows and their page count: a new
    /// context starts at page 0.
    pub fn sync(&mut self, hints: &[Hint], focus: Focus, pages: usize) {
        let mut h = std::hash::DefaultHasher::new();
        (focus as u8).hash(&mut h);
        for hint in hints {
            hint.key.hash(&mut h);
            hint.label.hash(&mut h);
        }
        let ctx = h.finish();
        if ctx != self.ctx {
            self.ctx = ctx;
            self.page = 0;
        }
        self.pages = pages;
        self.page = keybar::wrap_page(self.page, pages);
        self.shown = true;
    }
}

impl App {
    pub fn keybar_pane(&self) -> Pane {
        match self.focus {
            Focus::Sidebar => Pane::Sidebar,
            Focus::Commits => Pane::Commits,
            Focus::Detail => Pane::Diff,
            Focus::Log => Pane::Log,
            Focus::Message => Pane::Message,
        }
    }

    /// What the focus is on.
    fn keybar_context(&self) -> Context {
        match self.focus {
            Focus::Sidebar => match self.sidebar.get(self.side_cursor) {
                None => Context::Nothing,
                Some(SideItem::Header { .. }) => Context::Section,
                Some(SideItem::Branch { current: true, .. }) => Context::CurrentBranch,
                Some(SideItem::Branch { .. }) => Context::Branch,
                Some(SideItem::RemoteBranch { .. }) => Context::RemoteBranch,
                Some(SideItem::Remote { .. }) => Context::Remote,
                Some(SideItem::Tag { .. }) => Context::Tag,
                Some(SideItem::Stash { .. }) => Context::Stash,
                Some(SideItem::Worktree { .. }) => Context::Worktree,
                Some(SideItem::Submodule { .. }) => Context::Submodule,
                Some(SideItem::Subtree { .. }) => Context::Subtree,
            },
            Focus::Commits => {
                let row = self.selected;
                if self.history.is_worktree_row(row) {
                    Context::WorkingDir
                } else {
                    match self.history.commits.get(row) {
                        None => Context::Nothing,
                        Some(c) if c.stash.is_some() => Context::Stash,
                        Some(_) => Context::Commit,
                    }
                }
            }
            Focus::Detail => match &self.detail {
                Detail::Worktree => match self.rows.get(self.diff_cursor) {
                    Some(
                        DiffRow::File { src: Src::Conflicted, .. }
                        | DiffRow::Note { src: Src::Conflicted, .. },
                    ) => Context::Conflicted,
                    _ => Context::Changes,
                },
                Detail::Commit(_) => Context::CommitDiff,
                _ => Context::Nothing,
            },
            Focus::Log | Focus::Message => Context::Nothing,
        }
    }

    /// The bar's hints for the focus and its selection.
    pub fn keybar_hints(&self) -> Vec<Hint> {
        let pane = self.keybar_pane();
        // In the message editor `m` types: no menu entries there.
        let items = match pane {
            Pane::Message => Vec::new(),
            _ => self.context_menu_spec().map(|(_, items)| items).unwrap_or_default(),
        };
        let entries: Vec<MenuEntry> = items
            .iter()
            .filter(|i| i.action.is_some())
            .map(|i| MenuEntry {
                key: i.key,
                label: &i.label,
                same_as: i.same_as,
                // Greyed with its reason in the label; its key says why.
                enabled: i.disabled.is_none(),
                menu_only: i.auto_key,
            })
            .collect();
        let mut hints =
            with_frontend(|fe| keybar::bar(pane, self.keybar_context(), &entries, fe));
        // The bracket keys as the keyboard layout types them.
        for h in &mut hints {
            if let std::borrow::Cow::Owned(k) = crate::kb::text(&h.key) {
                h.key = k;
            }
        }
        hints
    }

    /// Whether the bar shows: not while the Search in Changes query is typed (its line has
    /// its own keys).
    pub fn keybar_visible(&self) -> bool {
        !(self.change.editing && self.change.open && self.focus == Focus::Commits)
    }

    /// A plain key the focused thing's menu has and the pane leaves free
    /// ([`keybar::reach`]): runs that entry, as the bar shows it. Entries whose key is the
    /// same shared command already ran through the key table.
    pub fn keybar_key(&mut self, ch: char) -> bool {
        let pane = self.keybar_pane();
        if pane == Pane::Message
            || with_frontend(|fe| keybar::reach(ch, pane, None, fe)) != Reach::Direct
        {
            return false;
        }
        let Some((_, items)) = self.context_menu_spec() else {
            return false;
        };
        let Some(action) = items
            .into_iter()
            .find(|i| i.key == Some(ch) && !i.auto_key)
            .and_then(|i| i.action)
        else {
            return false;
        };
        action(self);
        true
    }

    /// A shared key whose entry in the focused thing's menu is greyed (an entry
    /// marked `same_as` that command): says the entry's reason, as its menu line would,
    /// instead of running the command.
    pub fn keybar_disabled_key(&mut self, ch: char) -> bool {
        if self.keybar_pane() == Pane::Message {
            return false;
        }
        let Some((_, items)) = self.context_menu_spec() else {
            return false;
        };
        let Some(action) = items
            .into_iter()
            .find(|i| i.key == Some(ch) && i.same_as.is_some() && i.disabled.is_some())
            .and_then(|i| i.action)
        else {
            return false;
        };
        action(self);
        true
    }

    /// `o`: the bar's next page (wraps around). While a status text covers the bar, the
    /// key only uncovers it (it went with the key press).
    pub fn keybar_more(&mut self) {
        if self.keybar.shown && self.keybar.pages > 1 {
            self.keybar.page = keybar::wrap_page(self.keybar.page + 1, self.keybar.pages);
        }
    }
}
