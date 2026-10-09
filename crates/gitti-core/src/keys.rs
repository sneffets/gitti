//! The keys gitti and gitty share. Both apps bind exactly this table, so a key added here
//! works in both. Keys are written in gpui's keystroke syntax (`f`, `shift-p`, `ctrl-p`,
//! `:`); gitty translates its terminal key events into the same strings ([`key_name`]).
//!
//! These keys act while a list or the diff has focus — never while typing in a text field —
//! except the [`Scope::Global`] ones (a modifier key no text field uses).
//!
//! `secondary-` is gpui's platform key: cmd on macOS, ctrl elsewhere. Terminals only have
//! ctrl, so gitty reads it as `ctrl-` ([`lookup`], [`nav_lookup`], [`terminal_key`]).
//!
//! The keys are written for a US keyboard; [`layout`] turns them into keys another layout
//! can type.

pub mod layout;

pub use layout::{KeyboardLayout, KeyboardSettings, lookup_in};

/// Where a key acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Everywhere: the panes, lists, and also text fields (a modifier key no text field
    /// uses for editing).
    Global,
    /// Any pane (commit list, diff, sidebar), not in text fields.
    Panes,
    /// The commit list.
    Commits,
    /// The diff / working directory view.
    Diff,
}

/// A shared command. [`Command::name`] is the gpui action name in gitti (`gitti::<name>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Fetch,
    Pull,
    Push,
    SwitchBranch,
    CommandPalette,
    ShowShortcuts,
    Refresh,
    StashChanges,
    RunGitCommand,
    FocusMessage,
    ToggleAmend,
    StageAll,
    UnstageAll,
    DiscardAll,
    ToggleWhitespace,
    ToggleWrap,
    MoreContext,
    LessContext,
    ToggleOrder,
    ToggleGitLog,
    ErrorDetails,
    SelectFirst,
    SelectLast,
    StageSelection,
    UnstageSelection,
    DiscardSelection,
    CopySelection,
    OpenInEditor,
    NextHunk,
    PrevHunk,
    NextFile,
    PrevFile,
    FileList,
    MarkCompare,
    ShowMoreLines,
    ShowWholeFile,
    ToggleSidebar,
    GoToCommit,
    SearchChanges,
    Quit,
}

impl Command {
    pub fn name(self) -> &'static str {
        match self {
            Command::Fetch => "Fetch",
            Command::Pull => "Pull",
            Command::Push => "Push",
            Command::SwitchBranch => "SwitchBranch",
            Command::CommandPalette => "CommandPalette",
            Command::ShowShortcuts => "ShowShortcuts",
            Command::Refresh => "Refresh",
            Command::StashChanges => "StashChanges",
            Command::RunGitCommand => "RunGitCommand",
            Command::FocusMessage => "FocusMessage",
            Command::ToggleAmend => "ToggleAmend",
            Command::StageAll => "StageAll",
            Command::UnstageAll => "UnstageAll",
            Command::DiscardAll => "DiscardAll",
            Command::ToggleWhitespace => "ToggleWhitespace",
            Command::ToggleWrap => "ToggleWrap",
            Command::MoreContext => "MoreContext",
            Command::LessContext => "LessContext",
            Command::ToggleOrder => "ToggleOrder",
            Command::ToggleGitLog => "ToggleGitLog",
            Command::ErrorDetails => "ErrorDetails",
            Command::SelectFirst => "SelectFirst",
            Command::SelectLast => "SelectLast",
            Command::StageSelection => "StageSelection",
            Command::UnstageSelection => "UnstageSelection",
            Command::DiscardSelection => "DiscardSelection",
            Command::CopySelection => "CopySelection",
            Command::OpenInEditor => "OpenInEditor",
            Command::NextHunk => "NextHunk",
            Command::PrevHunk => "PrevHunk",
            Command::NextFile => "NextFile",
            Command::PrevFile => "PrevFile",
            Command::FileList => "FileList",
            Command::MarkCompare => "MarkCompare",
            Command::ShowMoreLines => "ShowMoreLines",
            Command::ShowWholeFile => "ShowWholeFile",
            Command::ToggleSidebar => "ToggleSidebar",
            Command::GoToCommit => "GoToCommit",
            Command::SearchChanges => "SearchChanges",
            Command::Quit => "Quit",
        }
    }
}

pub struct Binding {
    pub key: &'static str,
    pub scope: Scope,
    pub command: Command,
    /// For the shortcut lists of both apps.
    pub description: &'static str,
}

const fn b(key: &'static str, scope: Scope, command: Command, description: &'static str) -> Binding {
    Binding {
        key,
        scope,
        command,
        description,
    }
}

use Command as C;
use Scope::{Commits, Diff, Global, Panes};

pub const BINDINGS: &[Binding] = &[
    // ctrl-g everywhere. On macOS cmd-g is "find next" by convention; gitti has
    // no find, so it is free there too.
    b(
        "secondary-g",
        Global,
        C::GoToCommit,
        "go to commit (hash, message, author or file)",
    ),
    // Search in changes; ⌘F / Ctrl+F as find elsewhere, `/` as in less / vim.
    b(
        "secondary-f",
        Global,
        C::SearchChanges,
        "search in changes (commits whose changed lines have the words)",
    ),
    b("/", Panes, C::SearchChanges, "search in changes"),
    b("f", Panes, C::Fetch, "fetch"),
    b("p", Panes, C::Pull, "pull"),
    b("shift-p", Panes, C::Push, "push"),
    b("b", Panes, C::SwitchBranch, "switch branch"),
    b("ctrl-p", Panes, C::CommandPalette, "command palette"),
    b("?", Panes, C::ShowShortcuts, "keyboard shortcuts"),
    b("r", Panes, C::Refresh, "refresh"),
    b("z", Panes, C::StashChanges, "stash…"),
    b(":", Panes, C::RunGitCommand, "run a git command"),
    b("c", Panes, C::FocusMessage, "write the commit message"),
    b("shift-a", Panes, C::ToggleAmend, "amend on / off"),
    b("shift-s", Panes, C::StageAll, "stage all"),
    b("shift-u", Panes, C::UnstageAll, "unstage all"),
    b("shift-d", Panes, C::DiscardAll, "discard all (asks first)"),
    b("shift-w", Panes, C::ToggleWhitespace, "ignore whitespace on / off"),
    b("w", Panes, C::ToggleWrap, "wrap lines on / off"),
    b("+", Panes, C::MoreContext, "more context lines"),
    b("-", Panes, C::LessContext, "fewer context lines"),
    b("shift-t", Panes, C::ToggleOrder, "topological / date order"),
    b("shift-l", Panes, C::ToggleGitLog, "git command log panel"),
    b("e", Panes, C::ErrorDetails, "details of the last error"),
    b("shift-h", Panes, C::ToggleSidebar, "hide / show the sidebar"),
    // F10 = quit, as in Midnight Commander & co.; gitty's q stays its own.
    b("f10", Panes, C::Quit, "quit"),
    b(
        "shift-c",
        Panes,
        C::MarkCompare,
        "compare: mark the selected commit, others show the diff to it (again: end)",
    ),
    b("g", Commits, C::SelectFirst, "first commit"),
    b("shift-g", Commits, C::SelectLast, "last commit"),
    b("s", Diff, C::StageSelection, "stage selected lines / hunk / file"),
    b("u", Diff, C::UnstageSelection, "unstage (same rules)"),
    b("d", Diff, C::DiscardSelection, "discard (asks first)"),
    b("y", Diff, C::CopySelection, "copy selection"),
    // `o` pages the context key bar (pine's "O OTHER CMDS").
    b("shift-o", Diff, C::OpenInEditor, "open in editor"),
    b("n", Diff, C::NextHunk, "next hunk"),
    b("shift-n", Diff, C::PrevHunk, "previous hunk"),
    b("]", Diff, C::NextFile, "next file"),
    b("[", Diff, C::PrevFile, "previous file"),
    b("t", Panes, C::FileList, "go to a file of the shown diff (type to filter)"),
    b("x", Diff, C::ShowMoreLines, "show more unchanged lines (the nearest expander)"),
    b("shift-x", Diff, C::ShowWholeFile, "show the whole file (all unchanged lines)"),
];

/// The command bound to `key` in `scope` (or for all panes, or everywhere). `Global`
/// finds only the keys that act everywhere (for text fields).
pub fn lookup(key: &str, scope: Scope) -> Option<Command> {
    BINDINGS
        .iter()
        .find(|b| {
            same_key(b.key, key)
                && (b.scope == scope
                    || b.scope == Scope::Global
                    || (b.scope == Scope::Panes && scope != Scope::Global))
        })
        .map(|b| b.command)
}

/// Whether the table key `table` is the key `key`: equal, or `secondary-x` in the table and
/// `ctrl-x` from a terminal (which has no cmd key).
pub fn same_key(table: &str, key: &str) -> bool {
    table == key
        || table
            .strip_prefix("secondary-")
            .is_some_and(|rest| key.strip_prefix("ctrl-") == Some(rest))
}

/// A table key as a terminal frontend names it (`secondary-g` → `ctrl-g`).
pub fn terminal_key(key: &str) -> String {
    match key.strip_prefix("secondary-") {
        Some(rest) => format!("ctrl-{rest}"),
        None => key.to_string(),
    }
}

/// A key in the table's syntax from a typed character and modifiers: uppercase letters become
/// `shift-<lower>`, other characters stay as typed (layouts differ in what needs shift).
pub fn key_name(ch: char, ctrl: bool, alt: bool) -> String {
    let mut s = String::new();
    if ctrl {
        s.push_str("ctrl-");
    }
    if alt {
        s.push_str("alt-");
    }
    if ch.is_ascii_uppercase() {
        s.push_str("shift-");
        s.push(ch.to_ascii_lowercase());
    } else {
        s.push(ch);
    }
    s
}

/// A named key (`pageup`, `tab`, `home`, …) or a character in the table's syntax with its
/// modifiers, e.g. `named_key("tab", false, false, true)` = `shift-tab`. For frontends that
/// get key codes rather than characters (gitty maps crossterm's `KeyCode::PageUp` to
/// `"pageup"`).
pub fn named_key(name: &str, ctrl: bool, alt: bool, shift: bool) -> String {
    let mut s = String::new();
    if ctrl {
        s.push_str("ctrl-");
    }
    if alt {
        s.push_str("alt-");
    }
    if shift {
        s.push_str("shift-");
    }
    s.push_str(name);
    s
}

// ---------------------------------------------------------------------- commit graph keys

/// A move along the commit graph: towards parents (`down`) or children, along the branch
/// or the other way at merges / forks (`other`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphMove {
    pub down: bool,
    pub other: bool,
}

impl GraphMove {
    pub fn description(self) -> &'static str {
        match (self.down, self.other) {
            (true, false) => "follow the branch down (first parent)",
            (false, false) => "follow the branch up (the child on the lane)",
            (true, true) => "the other way down (merged parent)",
            (false, true) => "the other way up (other child)",
        }
    }

    /// Where this move goes from `targets` — the parents (`down`) or children of the
    /// selected row, the branch-continuing one first (`History::parent_rows` /
    /// `child_rows`). Both frontends act on the answer the same way.
    pub fn target(self, targets: &[usize]) -> GraphTarget {
        let what = if self.down { "parent" } else { "child" };
        match (targets, self.other) {
            ([], _) => GraphTarget::None(format!("No {what} commit")),
            ([first, ..], false) => GraphTarget::Row(*first),
            ([_], true) => GraphTarget::None(format!(
                "No other {what} — the branch continues only one way"
            )),
            ([_, second], true) => GraphTarget::Row(*second),
            // Several other ways: a chooser that lists the branch-continuing one too
            // (first, marked) but starts on the first alternative.
            (_, true) => GraphTarget::Choose,
        }
    }
}

/// The answer of [`GraphMove::target`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphTarget {
    /// Nowhere to go: the status line says why.
    None(String),
    Row(usize),
    /// More than one other way: let the user pick one of all `targets`.
    Choose,
}

pub struct GraphBinding {
    pub key: &'static str,
    pub nav: GraphMove,
}

const fn gk(key: &'static str, down: bool, other: bool) -> GraphBinding {
    GraphBinding {
        key,
        nav: GraphMove { down, other },
    }
}

/// The commit graph keys act wherever the focus is — sidebar, commit list,
/// diff, git log — except in text fields (commit message, dialog fields, picker queries,
/// search), where the keys keep their editing meaning (⌥↑ / ⌘↓ move the cursor there). The
/// commit list's selection moves and the diff follows; the focus stays. A pane that used
/// these keys for its own movement gives them up (they were just ↑ / ↓ there).
pub const GRAPH_BINDINGS: &[GraphBinding] = &[
    gk("alt-down", true, false),
    gk("alt-up", false, false),
    gk("secondary-down", true, false),
    gk("secondary-up", false, false),
    gk("alt-shift-down", true, true),
    gk("alt-shift-up", false, true),
    gk("secondary-shift-down", true, true),
    gk("secondary-shift-up", false, true),
];

/// The graph move bound to `key` (`ctrl-` from a terminal finds `secondary-`).
pub fn graph_lookup(key: &str) -> Option<GraphMove> {
    GRAPH_BINDINGS
        .iter()
        .find(|b| same_key(b.key, key))
        .map(|b| b.nav)
}

// ---------------------------------------------------------------------- list / scroll keys

/// Where a list / scroll key acts: the panes and the overlays with a list or a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NavScope {
    /// Sidebar (locations): a cursor that selects the entry's commit.
    Sidebar,
    /// The commit list: the selection moves.
    Commits,
    /// The diff (gitti scrolls it; gitty moves its line cursor).
    Diff,
    /// The git command log panel (focused): it scrolls; the newest end follows new entries.
    Log,
    /// A list with a query field (command palette, branch switcher): keys that don't type.
    Picker,
    /// A menu (context menus): a list without a text field.
    Menu,
    /// A read-only text window (command output, errors, signature, shortcut list).
    Text,
}

impl NavScope {
    /// The panes, where the single keys of [`BINDINGS`] act as well (`Scope::Panes`).
    pub fn is_pane(self) -> bool {
        matches!(
            self,
            NavScope::Sidebar | NavScope::Commits | NavScope::Diff | NavScope::Log
        )
    }

    pub fn title(self) -> &'static str {
        match self {
            NavScope::Sidebar => "Sidebar",
            NavScope::Commits => "Commit list",
            NavScope::Diff => "Diff",
            NavScope::Log => "Git command log (focused)",
            NavScope::Picker => "Palette / branch switcher",
            NavScope::Menu => "Context menus",
            NavScope::Text => "Text windows (output, errors, shortcuts)",
        }
    }
}

/// A list / scroll movement. Lists move their cursor, views without one scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nav {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
    /// Top / first entry (the git log: the oldest entry).
    First,
    /// End / last entry (the git log: the newest one, following new entries again).
    Last,
    /// Leave: close the overlay, or from the git log back to the commit list.
    Back,
}

impl Nav {
    pub fn description(self) -> &'static str {
        match self {
            Nav::LineUp => "up",
            Nav::LineDown => "down",
            Nav::PageUp => "a page up",
            Nav::PageDown => "a page down",
            Nav::HalfPageUp => "half a page up",
            Nav::HalfPageDown => "half a page down",
            Nav::First => "to the top / first",
            Nav::Last => "to the end / last",
            Nav::Back => "close / back",
        }
    }
}

pub struct NavBinding {
    pub key: &'static str,
    pub scope: NavScope,
    pub nav: Nav,
}

const fn n(key: &'static str, scope: NavScope, nav: Nav) -> NavBinding {
    NavBinding { key, scope, nav }
}

/// The list / scroll keys of both apps, per scope. `g` / `G` in the commit list are
/// [`Command::SelectFirst`] / [`Command::SelectLast`] in [`BINDINGS`] (gitty runs them
/// through the command table), so they are not repeated here.
pub const NAV_BINDINGS: &[NavBinding] = {
    use Nav::*;
    use NavScope::*;
    &[
        // Sidebar.
        n("down", Sidebar, LineDown),
        n("j", Sidebar, LineDown),
        n("up", Sidebar, LineUp),
        n("k", Sidebar, LineUp),
        n("pagedown", Sidebar, PageDown),
        n("pageup", Sidebar, PageUp),
        n("home", Sidebar, First),
        n("secondary-home", Sidebar, First),
        n("g", Sidebar, First),
        n("end", Sidebar, Last),
        n("secondary-end", Sidebar, Last),
        n("shift-g", Sidebar, Last),
        // Commit list.
        n("down", Commits, LineDown),
        n("j", Commits, LineDown),
        n("up", Commits, LineUp),
        n("k", Commits, LineUp),
        n("pagedown", Commits, PageDown),
        n("pageup", Commits, PageUp),
        n("ctrl-d", Commits, HalfPageDown),
        n("ctrl-u", Commits, HalfPageUp),
        n("home", Commits, First),
        n("secondary-home", Commits, First),
        n("end", Commits, Last),
        n("secondary-end", Commits, Last),
        // Diff.
        n("down", Diff, LineDown),
        n("j", Diff, LineDown),
        n("up", Diff, LineUp),
        n("k", Diff, LineUp),
        n("pagedown", Diff, PageDown),
        n("pageup", Diff, PageUp),
        n("ctrl-d", Diff, HalfPageDown),
        n("ctrl-u", Diff, HalfPageUp),
        n("home", Diff, First),
        n("secondary-home", Diff, First),
        n("g", Diff, First),
        n("end", Diff, Last),
        n("secondary-end", Diff, Last),
        n("shift-g", Diff, Last),
        // Git command log.
        n("down", Log, LineDown),
        n("j", Log, LineDown),
        n("up", Log, LineUp),
        n("k", Log, LineUp),
        n("pagedown", Log, PageDown),
        n("pageup", Log, PageUp),
        n("ctrl-d", Log, HalfPageDown),
        n("ctrl-u", Log, HalfPageUp),
        n("home", Log, First),
        n("secondary-home", Log, First),
        n("g", Log, First),
        n("end", Log, Last),
        n("secondary-end", Log, Last),
        n("shift-g", Log, Last),
        n("escape", Log, Back),
        // Pickers: no plain letters (they type into the query).
        n("down", Picker, LineDown),
        n("ctrl-n", Picker, LineDown),
        n("ctrl-j", Picker, LineDown),
        n("tab", Picker, LineDown),
        n("up", Picker, LineUp),
        n("ctrl-p", Picker, LineUp),
        n("ctrl-k", Picker, LineUp),
        n("shift-tab", Picker, LineUp),
        n("pagedown", Picker, PageDown),
        n("pageup", Picker, PageUp),
        n("home", Picker, First),
        n("secondary-home", Picker, First),
        n("end", Picker, Last),
        n("secondary-end", Picker, Last),
        n("escape", Picker, Back),
        // Menus.
        n("down", Menu, LineDown),
        n("j", Menu, LineDown),
        n("tab", Menu, LineDown),
        n("up", Menu, LineUp),
        n("k", Menu, LineUp),
        n("shift-tab", Menu, LineUp),
        n("pagedown", Menu, PageDown),
        n("pageup", Menu, PageUp),
        n("home", Menu, First),
        n("secondary-home", Menu, First),
        n("end", Menu, Last),
        n("secondary-end", Menu, Last),
        n("escape", Menu, Back),
        n("q", Menu, Back),
        // Text windows.
        n("down", Text, LineDown),
        n("j", Text, LineDown),
        n("up", Text, LineUp),
        n("k", Text, LineUp),
        n("pagedown", Text, PageDown),
        n("space", Text, PageDown),
        n("pageup", Text, PageUp),
        n("home", Text, First),
        n("secondary-home", Text, First),
        n("g", Text, First),
        n("end", Text, Last),
        n("secondary-end", Text, Last),
        n("shift-g", Text, Last),
        n("escape", Text, Back),
        n("q", Text, Back),
        n("enter", Text, Back),
    ]
};

/// The movements as shortcut lists show them: a pair per line ("down / up") and what it
/// does.
pub const NAV_PAIRS: &[(Nav, Option<Nav>, &str)] = &[
    (Nav::LineDown, Some(Nav::LineUp), "down / up"),
    (Nav::PageDown, Some(Nav::PageUp), "a page down / up"),
    (Nav::HalfPageDown, Some(Nav::HalfPageUp), "half a page down / up"),
    (Nav::First, Some(Nav::Last), "top / end (first / last)"),
    (Nav::Back, None, "close / back"),
];

/// Every scope of [`NAV_BINDINGS`], in the order shortcut lists show them.
pub const NAV_SCOPES: [NavScope; 7] = [
    NavScope::Commits,
    NavScope::Diff,
    NavScope::Sidebar,
    NavScope::Log,
    NavScope::Picker,
    NavScope::Menu,
    NavScope::Text,
];

/// The movement bound to `key` in `scope` (`ctrl-` from a terminal finds `secondary-`).
pub fn nav_lookup(key: &str, scope: NavScope) -> Option<Nav> {
    NAV_BINDINGS
        .iter()
        .find(|b| same_key(b.key, key) && b.scope == scope)
        .map(|b| b.nav)
}

/// The keys of `nav` in `scope`, in table order (for shortcut lists and hints).
pub fn nav_keys(scope: NavScope, nav: Nav) -> impl Iterator<Item = &'static str> {
    NAV_BINDINGS
        .iter()
        .filter(move |b| b.scope == scope && b.nav == nav)
        .map(|b| b.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn nav_keys_are_unique_and_dont_shadow_commands() {
        for (i, a) in NAV_BINDINGS.iter().enumerate() {
            for other in &NAV_BINDINGS[i + 1..] {
                assert!(
                    !(a.key == other.key && a.scope == other.scope),
                    "{} bound twice in {:?}",
                    a.key,
                    a.scope
                );
            }
            // In the panes the single-key commands act too: no key may mean both.
            if a.scope.is_pane() {
                let scope = match a.scope {
                    NavScope::Commits => Scope::Commits,
                    NavScope::Diff => Scope::Diff,
                    _ => Scope::Panes,
                };
                assert_eq!(lookup(a.key, scope), None, "{} is also a command", a.key);
            }
        }
        // Pickers type plain characters into their query.
        for b in NAV_BINDINGS.iter().filter(|b| b.scope == NavScope::Picker) {
            assert!(b.key.contains('-') || b.key.len() > 1, "{} would not type", b.key);
        }
        assert_eq!(nav_lookup("ctrl-d", NavScope::Log), Some(Nav::HalfPageDown));
        assert_eq!(nav_lookup("g", NavScope::Commits), None);
        assert_eq!(lookup("g", Scope::Commits), Some(Command::SelectFirst));
        assert_eq!(nav_lookup("shift-g", NavScope::Text), Some(Nav::Last));
        assert_eq!(nav_lookup("j", NavScope::Picker), None);
        assert_eq!(
            nav_keys(NavScope::Log, Nav::Last).collect::<Vec<_>>(),
            ["end", "secondary-end", "shift-g"]
        );
        // ctrl-home / ctrl-end (cmd on macOS) go to the ends in every list too.
        for scope in NAV_SCOPES {
            assert_eq!(nav_lookup("secondary-home", scope), Some(Nav::First), "{scope:?}");
            assert_eq!(nav_lookup("ctrl-end", scope), Some(Nav::Last), "{scope:?}");
        }
        assert_eq!(named_key("tab", false, false, true), "shift-tab");
        assert_eq!(named_key("pageup", true, false, false), "ctrl-pageup");
    }

    /// The graph keys act in every pane, so no list / scroll key or command of a
    /// pane may be one of them; each move has a key in both families (⌥ and ⌘ / ctrl).
    #[gitti_test::test]
    fn graph_keys_act_everywhere_without_clashes() {
        for (i, a) in GRAPH_BINDINGS.iter().enumerate() {
            assert!(
                GRAPH_BINDINGS[i + 1..].iter().all(|b| b.key != a.key),
                "{} bound twice",
                a.key
            );
            assert!(
                !NAV_BINDINGS
                    .iter()
                    .any(|n| n.scope.is_pane() && same_key(n.key, a.key)),
                "{} is also a list key",
                a.key
            );
            for scope in [Scope::Global, Scope::Panes, Scope::Commits, Scope::Diff] {
                assert_eq!(lookup(a.key, scope), None, "{} is also a command", a.key);
            }
        }
        for down in [true, false] {
            for other in [true, false] {
                let keys: Vec<_> = GRAPH_BINDINGS
                    .iter()
                    .filter(|b| b.nav == GraphMove { down, other })
                    .map(|b| b.key)
                    .collect();
                assert_eq!(keys.len(), 2, "{down} {other}: {keys:?}");
            }
        }
        // A terminal names them with ctrl / alt / shift in this order (`named_key`).
        let follow = Some(GraphMove { down: true, other: false });
        assert_eq!(graph_lookup(&named_key("down", false, true, false)), follow);
        assert_eq!(graph_lookup(&named_key("down", true, false, false)), follow);
        assert_eq!(
            graph_lookup(&named_key("up", false, true, true)),
            Some(GraphMove { down: false, other: true })
        );
        assert_eq!(graph_lookup("down"), None);
        assert_eq!(graph_lookup("shift-down"), None);
        // Where a move goes: the first target, the second, or a chooser of all.
        let up = GraphMove { down: false, other: false };
        let other_up = GraphMove { down: false, other: true };
        assert_eq!(up.target(&[]), GraphTarget::None("No child commit".into()));
        assert_eq!(up.target(&[7, 3, 9]), GraphTarget::Row(7));
        assert!(matches!(other_up.target(&[7]), GraphTarget::None(m) if m.contains("only one way")));
        assert_eq!(other_up.target(&[7, 3]), GraphTarget::Row(3));
        assert_eq!(other_up.target(&[7, 3, 9]), GraphTarget::Choose);
    }

    #[gitti_test::test]
    fn keys_are_unique_per_scope_and_names_round_trip() {
        for (i, a) in BINDINGS.iter().enumerate() {
            for other in &BINDINGS[i + 1..] {
                let wide = |s: Scope| matches!(s, Scope::Panes | Scope::Global);
                let overlap = a.scope == other.scope || wide(a.scope) || wide(other.scope);
                assert!(!(a.key == other.key && overlap), "{} bound twice", a.key);
            }
        }
        assert_eq!(key_name('P', false, false), "shift-p");
        assert_eq!(key_name('p', true, false), "ctrl-p");
        assert_eq!(key_name(':', false, false), ":");
        assert_eq!(lookup("shift-p", Scope::Diff), Some(Command::Push));
        assert_eq!(lookup("s", Scope::Commits), None);
        assert_eq!(lookup("s", Scope::Diff), Some(Command::StageSelection));
        assert_eq!(lookup("f10", Scope::Commits), Some(Command::Quit));
        assert_eq!(lookup("shift-h", Scope::Panes), Some(Command::ToggleSidebar));
        // Go to Commit acts everywhere; a terminal sends ctrl for secondary.
        for scope in [Scope::Global, Scope::Panes, Scope::Commits, Scope::Diff] {
            assert_eq!(lookup("ctrl-g", scope), Some(Command::GoToCommit), "{scope:?}");
            assert_eq!(lookup("secondary-g", scope), Some(Command::GoToCommit));
        }
        assert_eq!(lookup("f", Scope::Global), None, "pane keys don't act in text fields");
        assert_eq!(lookup("g", Scope::Commits), Some(Command::SelectFirst));
        assert_eq!(terminal_key("secondary-g"), "ctrl-g");
        assert_eq!(terminal_key("shift-x"), "shift-x");
        assert!(!same_key("secondary-g", "ctrl-shift-g"));
    }
}
