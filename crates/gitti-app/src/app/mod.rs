//! Main window: toolbar, sidebar (refs), commit list with graph, detail/diff panel.
//!
//! Split by area: `sidebar`, `commit_list`, `diff_view`, `modal` (dialogs + askpass),
//! `menus` (context menus and the operations behind them).

mod branch_list;
mod change_search;
mod clean;
mod commit_box;
mod commit_lines;
mod commit_list;
mod commit_flow;
mod compare;
mod context_lines;
mod config_ui;
mod conflict_ui;
mod diff_view;
mod ext_tools;
mod file_marks;
mod file_tabs;
mod folder_picker;
mod from_commit;
mod fs_paths;
mod git_log;
mod graph_nav;
mod header_ids;
mod highlighting;
mod key_actions;
mod lfs;
mod lfs_fetch;
mod lfs_health;
mod menu_cmds;
mod menus;
mod message_editor;
mod modal;
mod nav_keys;
mod navigate;
mod op_cancel;
mod op_progress;
mod op_stops;
mod overlay_keys;
mod panes;
mod pick_several;
mod picker;
mod push_ui;
mod reflog;
mod renormalize;
mod repo_actions;
mod repo_stats;
mod script_scroll;
mod shallow_ui;
mod settings_view;
mod sidebar;
mod signers;
mod stash_ui;
mod signing_keys;
mod spell_ui;
mod startup_ui;
mod submodules;
mod subtrees;
mod custom_commands;
mod tab_marks;
mod tag_ui;
mod test_state;
mod toolbar;
mod unavailable;
mod view_anchor;
mod warnings;
mod watching;
mod worktree_tabs;
mod worktrees;

pub(crate) use overlay_keys::intercept as intercept_overlay_keys;
pub(crate) use menus::{run_menu_entry, take_menu_entries, take_menu_labels};
pub(crate) use signers::set_open_repos;
pub(crate) use unavailable::set_tab_count;
pub(crate) use startup_ui::WHERE as STARTUP_TIMINGS_WHERE;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gitti_core::commit_cache::CommitData;
use gitti_core::diff::FileDiff;
use gitti_core::discard::{DiscardOptions, DiscardPlan};
use gitti_core::ops::{ConflictSides, RemoteInfo, RepoState, StashInfo, Tracking};
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::reselect::Reselect;
use gitti_core::{
    HeadInfo, History, LogOrder, RefInfo, RefKind, Repo, RepoSnapshot, WorkingState,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::component::{
    Disableable as _, ResizableState, Sizable as _, h_flex, h_resizable, resizable_panel, v_flex,
    v_resizable,
};
use gitti_core::store::PaneLayout;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::rows::{DiffRow, Expansion, LineSel, Src, build_commit_rows_noted, line_part_range};
use crate::probe::ProbeExt as _;
use crate::shortcuts as tips;
use crate::theme::{PaletteExt as _, Settings};
use crate::{
    CmdHistoryNext, CmdHistoryPrev, CommandPalette, Commit, Fetch, FocusCommitList, FocusMessage,
    ModalCancel, OpenSettings, PickerDown, PickerUp, Pull, Push, Refresh, ResetZoom, RunGitCommand,
    CopySelection, OpenInEditor, SelectNext, SelectPrev, StageSelection, StashChanges, SwitchBranch, ToggleGitLog, ToggleOrder,
    ToggleWhitespace, ToggleWrap, UnstageSelection, ZoomDiffIn, ZoomDiffOut, ZoomIn, ZoomOut,
};

pub use change_search::ChangeSearchState;
pub use reflog::ReflogState;
pub use modal::AskRequest;
pub use settings_view::{ShortcutLine, shortcut_lines};
pub use repo_actions::AppEvent;
pub use submodules::RetryUpdate;
pub use spell_ui::SpellTarget;
pub(crate) use spell_ui::take_last_editor_menu;
use modal::Modal;

const BUSY_SHOW_DELAY: std::time::Duration = std::time::Duration::from_millis(250);
const PENDING_SHOW_DELAY: std::time::Duration = std::time::Duration::from_millis(100);
/// A commit load running this long replaces the previous commit with "Loading…".
const SLOW_LOAD: std::time::Duration = std::time::Duration::from_millis(300);

/// Unwrapped diff lines are cut off after this many characters.
pub const MAX_LINE_CHARS: usize = 400;

/// A single-line input sized for the UI font. gpui-component's inputs have a fixed height
/// and line height (rems), which clips descenders once the UI font is bigger than default.
pub(crate) fn text_input(state: &Entity<InputState>, s: &Settings) -> Input {
    let line = (s.ui_font_size * 1.4).ceil();
    let input = Input::new(state)
        .text_size(s.ui())
        .line_height(px(line))
        .py(px(6.));
    // `Input::h` is an inherent method that only applies to multi-line inputs.
    Styled::h(input, px(line + 12.))
}

/// Starts the user's editor on `file` at `line` (gitti is no terminal: a bare terminal
/// editor like `nvim` needs a terminal wrapper in the command).
pub(crate) fn open_editor(
    template: &str,
    file: &std::path::Path,
    line: u32,
    workdir: &std::path::Path,
) -> anyhow::Result<()> {
    let argv = gitti_core::editor::editor_argv(template, file, line)?;
    if gitti_core::editor::is_terminal_editor(&argv) {
        anyhow::bail!(
            "{} runs in a terminal — set the editor command to open one, e.g. \
             `ghostty -e {} +{{line}} {{file}}` (Settings → Diff → Editor)",
            argv[0],
            argv[0]
        );
    }
    // A file name that is not UTF-8 reaches the editor as its bytes.
    gitti_core::editor::spawn_editor(&gitti_core::editor::os_argv(&argv, file), workdir)
}

/// A shown commit: the (cached, shared) data plus how it was reached. Derefs to
/// [`CommitData`] (`details`, `sig_kind` — verification arrives later in `signature` —,
/// `files`).
struct CommitView {
    data: Arc<CommitData>,
    /// `Some(n)` when showing `stash@{n}`.
    stash: Option<usize>,
    /// Compare mode: `data` is the diff between two versions, not a commit (no header,
    /// read-only; `compare.rs`).
    compare: Option<Arc<gitti_core::compare::CompareData>>,
}

impl std::ops::Deref for CommitView {
    type Target = CommitData;
    fn deref(&self) -> &CommitData {
        &self.data
    }
}

enum Detail {
    Empty,
    Worktree,
    Loading,
    /// The selected commit / comparison couldn't be loaded: why (git's message). Selecting
    /// it again loads it again.
    Failed(String),
    Commit(Arc<CommitView>),
}

#[derive(Clone)]
struct Status {
    text: String,
    error: bool,
}

/// Everything a refresh loads besides history.
struct Pending {
    src: Src,
    file: usize,
    /// `None` = the whole file.
    hunks: Option<BTreeSet<usize>>,
    label: &'static str,
}

/// A key action that came while an operation ran; it runs when that one is done, against
/// the state the user then sees (so `s` pressed twice stages two hunks, not one twice).
type Deferred = Box<dyn FnOnce(&mut GittiApp, &mut Window, &mut Context<GittiApp>)>;

pub struct GittiApp {
    repo: Option<Arc<Repo>>,
    open_error: Option<String>,
    /// Why the folder didn't open, when it is more than "not a repository" (a bare
    /// repository, no git installed): the start page's heading and buttons follow it.
    open_kind: Option<gitti_core::OpenError>,
    /// The folder this tab was opened on (`None`: the start page without any).
    path: Option<PathBuf>,
    /// Askpass setup for git commands outside a repository (clone).
    askpass_env: Vec<(String, String)>,
    settings: Settings,
    history: Arc<History>,
    head: HeadInfo,
    working: Arc<WorkingState>,
    refs: Vec<RefInfo>,
    remotes: Vec<RemoteInfo>,
    stashes: Vec<StashInfo>,
    repo_state: RepoState,
    tracking: Vec<Tracking>,
    /// The current branch's commits that are not pushed (the tab's hollow mark).
    unpushed: Option<gitti_core::tab_marks::Unpushed>,
    /// The working state the shown rows were built from (working-directory view).
    rows_for: Option<Arc<WorkingState>>,
    /// The file watcher's first report arrived (its watches are in place; with a
    /// recursive OS watch it asks for one more read of what it couldn't see before).
    watch_ready: bool,
    /// The first full read waits for the watcher's first report (`watching.rs`).
    first_read_pending: bool,
    /// A read of this repository's other worktrees' marks, started by this tab, runs
    /// (`tab_marks.rs`; the marks are in the app-wide store).
    worktree_marks_loading: bool,
    /// Effective user.name / user.email.
    identity: (Option<String>, Option<String>),
    /// Names of "ours" / "theirs" while there are conflicted files.
    conflict_sides: Option<ConflictSides>,
    /// Every worktree of the repository, main first (`worktrees.rs`).
    worktrees: Vec<gitti_core::worktree::Worktree>,
    /// Every submodule with its state (`submodules.rs`; empty without `.gitmodules`).
    submodules: Vec<gitti_core::submodule::Submodule>,
    /// The subtrees of HEAD's history (`subtrees.rs`, read in the background when HEAD
    /// moves), the HEAD they were read for, a read running / asked for again.
    subtrees: Vec<gitti_core::subtree::Subtree>,
    subtrees_head: Option<Option<gitti_core::ObjectId>>,
    subtrees_loading: bool,
    subtrees_queued: bool,
    /// Git LFS: the repository's state, the notice shown, the locks (`lfs.rs`).
    lfs: lfs::LfsUi,
    /// The "out of date" banner was hidden for these pointers.
    submodule_banner_dismissed: Option<submodules::BannerKey>,
    /// The paths the "can't be checked out on this file system" banner was hidden for
    /// (`fs_paths.rs`).
    fs_banner_dismissed: Option<Vec<String>>,
    /// Verification result of the shown commit's signature (`None` inside = failed).
    signature: Option<(gitti_core::ObjectId, Option<gitti_core::signature::SignatureInfo>)>,
    /// The shown SSH signature can't be checked for want of an allowed signers file: the
    /// files that could be used (`signers.rs`).
    signers_offer: Option<signers::SignersOffer>,

    selected: usize,
    commit_scroll: UniformListScrollHandle,
    /// The sidebar's scroll position (it is virtualised by hand, see `render_sidebar`).
    sidebar_scroll: ScrollHandle,
    diff_scroll: UniformListScrollHandle,
    detail: Detail,
    detail_generation: u64,
    /// A commit load runs; when it ends, the then selected row is shown (coalesces key
    /// repeat into one load at a time).
    commit_loading: bool,
    /// Counts commit loads: the [`SLOW_LOAD`] timer of one that has ended does nothing.
    commit_load_gen: u64,
    rows: Arc<Vec<DiffRow>>,
    /// The folder rows of `rows` (the working directory as a tree; empty when flat).
    folders: gitti_core::rows::Folders,
    /// Files marked for an action on several (⌘ / shift-click on file headers).
    file_marks: gitti_core::rows::FileMarks,
    /// Marked commits of the commit list (`pick_several.rs`).
    commit_marks: gitti_core::commit_marks::CommitMarks,
    expansion: Expansion,
    line_sel: Option<LineSel>,
    dragging: bool,
    /// Character selection in the detail panel (message + diff lines), for copying.
    text_sel: Option<crate::text_sel::TextSel>,
    text_dragging: bool,
    /// The message / diff lines laid out last frame, for dragging outside them.
    drag_targets: crate::text_sel::DragTargets,
    /// Mouse position of a running selection drag (text or lines) and whether the
    /// auto-scroll ticker runs (`drag_autoscroll`); the diff list's bounds last frame.
    drag_pos: Option<Point<Pixels>>,
    autoscrolling: bool,
    diff_list_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Diff row under the last right click (context menu target).
    menu_row: Option<usize>,

    message: Entity<TextareaState>,
    /// Misspelled words of the message (`spell_ui`).
    spell: spell_ui::SpellStates,
    /// The message git left for the next commit (`MERGE_MSG` / `SQUASH_MSG`) as put into
    /// the message box, so an untouched copy can follow it and go with it
    /// (`commit_flow::sync_prepared_message`).
    merge_prefill: Option<String>,
    /// That message as of the last refresh (`None`: git left none).
    prepared: Option<String>,
    /// The prepared message the user answered "Keep Mine" for: its note stays hidden.
    prepared_dismissed: Option<String>,
    amend: bool,
    /// Template, Sign Off, earlier messages (`commit_box`).
    commit_box: commit_box::CommitBox,
    /// The message was typed in since its draft was last written (`commit_flow`);
    /// the pending debounced write.
    draft_dirty: bool,
    draft_save: Option<Task<()>>,
    /// The message edited for an amend, as stored (`commit_flow`): it comes back when
    /// amend is turned on again while its commit is still HEAD.
    amend_draft: Option<gitti_core::store::AmendDraft>,
    /// The message box was blank in the last frame (with nothing staged the Commit button
    /// is off only then): typing re-renders when that flips.
    message_was_blank: Cell<bool>,
    status: Option<Status>,
    /// Failed operations not looked at yet: the toolbar's ⚠ mark (`toolbar.rs`).
    errors: gitti_core::error_mark::ErrorMarks,
    /// Counts status messages; the one a fade timer was started for (`warnings.rs`).
    status_gen: u64,
    status_fading: u64,
    /// Height between the toolbar's command bar and the window bottom (last frame): the
    /// palette dropping down under the bar shows no more entries than fit.
    palette_room: Rc<Cell<Option<Pixels>>>,
    busy: Option<String>,
    /// File / hunks whose stage / unstage / discard is running: shown dimmed with a label
    /// right away, until the refreshed status replaces them.
    pending: Option<Pending>,
    /// Secure keyboard entry while a password dialog is shown (macOS).
    secure_input: crate::secure_input::SecureInput,
    /// Busy state is only shown (label, disabled buttons) once an operation takes longer
    /// than `BUSY_SHOW_DELAY` — quick ones like staging would make the UI flicker.
    busy_visible: bool,
    busy_generation: u64,
    /// The pending marks (dimmed lines, "Staging…") are shown once an operation takes
    /// longer than `PENDING_SHOW_DELAY`; a quick one just shows its result.
    pending_visible: bool,
    /// Bumped whenever an operation changes the working state; a refresh that started
    /// before keeps its working-state part out (it would show the old state again).
    working_generation: u64,
    /// File-system changes reported while an operation ran (`Some(refs moved)`), refreshed
    /// when it ends.
    watch_pending: Option<gitti_core::watch::Change>,
    /// Which watcher changes are already covered by what we read (see `WatchFilter`).
    watch_filter: gitti_core::watch::WatchFilter,
    /// Tests (probes on): the paths the file watcher reported (bounded).
    watch_seen: Vec<std::path::PathBuf>,
    deferred: std::collections::VecDeque<Deferred>,
    /// Cancels the running operation's git (`op_cancel.rs`; `None`: not cancellable).
    op_cancel: Option<gitti_core::process::CancelToken>,
    /// Live output of the running operation: progress meter, last hook / git line and
    /// the whole output, shown in the status bar (`op_progress.rs`).
    op_progress: gitti_core::progress::OpProgress,
    /// The running operation's output window over the status bar.
    op_output: op_progress::OpOutputView,
    list_focus: FocusHandle,
    side_focus: FocusHandle,
    /// Keyboard cursor in the sidebar (index into `sidebar_items`).
    side_cursor: Option<usize>,
    /// The sidebar entries' keys when `side_cursor` was last set or followed
    /// (`sidebar.rs`: `sync_side_cursor`).
    side_cursor_keys: Vec<(String, u8)>,
    diff_focus: FocusHandle,
    loaded_once: bool,
    /// A restored background tab that hasn't started reading its repository yet
    /// ([`Self::load_if_deferred`]): its turn comes after the shown tab, or when shown.
    load_deferred: bool,
    /// File tab of the working-directory view (`None` = all changes); dropped when its
    /// path has no changes any more.
    worktree_tab: Option<String>,
    /// The working-directory file tabs and the state they were made from.
    worktree_tabs: worktree_tabs::TabCache,
    refresh_in_flight: bool,
    /// A refresh requested while one was running (`Some(full)`).
    refresh_queued: Option<bool>,
    /// The history loads on its own, beside the working state.
    history_in_flight: bool,
    /// A history load asked for while one runs (`Some(force)`).
    history_queued: Option<bool>,
    /// A path-limited re-read of an outside edit is running (`watching.rs`); refreshes
    /// wait for it.
    paths_in_flight: bool,
    /// Paths changed meanwhile, re-read after it or the running refresh.
    paths_queued: Option<Vec<String>>,
    /// Collapsed sidebar groups ("branches", "remotes", "remote:origin", "tags", "stashes").
    collapsed: HashSet<String>,
    /// The sidebar's branch order, pins, recent checkouts (`branch_list.rs`).
    branches: branch_list::BranchState,
    modal: Option<Modal>,
    ask_queue: std::collections::VecDeque<AskRequest>,
    /// The helper waiting for the shown credential dialog (it closes when that goes away).
    ask_shown: Option<gitti_core::askpass::Asker>,
    /// A dialog is being replaced by another one (its cancel callback runs, but that is
    /// not the user cancelling).
    replacing_modal: bool,
    picker: Option<picker::Picker>,
    /// Diff panel width in monospace columns, measured while painting (for line wrap).
    wrap_cols: Rc<Cell<usize>>,
    /// How the explanation rows of paths this file system can't hold wrap
    /// (`fs_paths.rs`).
    note_wrap: fs_paths::NoteWrapper,
    /// The shown commit's (comparison's, stash's) paths this file system can't hold,
    /// against the index's paths (`fs_paths.rs`; set with the rows).
    commit_fs: Arc<gitti_core::fs_paths::PathReport>,
    /// Height of the diff list viewport (measured while painting).
    diff_viewport_h: Rc<Cell<Pixels>>,
    log_scroll: UniformListScrollHandle,
    /// The git log panel's list (Tab / click; the list / scroll keys scroll it).
    log_focus: FocusHandle,
    /// Command log generation last shown (to auto-scroll on new entries).
    log_seen: Cell<u64>,
    /// `(log generation, show reads, filtered entries)` of the git log panel.
    log_view: RefCell<Option<git_log::LogView>>,
    log_view_builds: Cell<u64>,
    /// Syntax styles per (source, path) of the files in the diff panel.
    highlights: std::collections::HashMap<(Src, String), crate::highlight::FileHighlight>,
    hl_cache: highlighting::SharedCache,
    commit_hl: highlighting::SharedCommitHighlights,
    /// Commits being prefetched (`prefetch_around`).
    prefetching: Arc<std::sync::Mutex<std::collections::HashSet<gitti_core::ObjectId>>>,
    /// Row of the commit shown before, for the prefetch direction.
    last_shown_row: usize,
    hl_generation: u64,
    /// The working-directory highlights and the state they belong to: going back to the
    /// working directory shows them in the first frame instead of re-reading the files.
    worktree_hl: Option<(Arc<WorkingState>, highlighting::Highlights)>,
    /// Command line in the git log panel.
    cmd_input: Entity<InputState>,
    cmd_history: Vec<String>,
    cmd_history_pos: Option<usize>,
    settings_open: bool,
    settings_section: settings_view::Section,
    settings_focus: FocusHandle,
    /// The settings content (scrolled by the list / scroll keys too).
    settings_scroll: ScrollHandle,
    /// Rows offered by the graph navigation chooser (highlighted in the graph).
    nav_candidates: Vec<usize>,
    /// Options of the last merge / cherry-pick dialog (preset for the next one).
    last_merge: gitti_core::ops::MergeOptions,
    last_cherry_pick: gitti_core::ops::CherryPickOptions,
    /// File tab of the commit view (`None` = all files).
    file_tab: Option<usize>,
    /// The diff's place across a view toggle (`view_anchor.rs`).
    view_anchor: Option<view_anchor::PendingAnchor>,
    /// Go to Commit file hit: show this file once this commit's diff is shown.
    pending_file: Option<(gitti_core::ObjectId, String)>,
    /// Compare mode (marked base, its loads and caches).
    compare: compare::CompareState,
    /// Search in Changes: the bar above the list, its scan and hits.
    change_search: change_search::ChangeSearchUi,
    /// The reflog view in place of the commit list (`reflog.rs`).
    reflog: reflog::ReflogUi,
    /// Repository ▸ Repository Statistics… (`repo_stats.rs`).
    repo_stats: repo_stats::StatsUi,
    /// `DiffRow::HeaderSpace` rows reserved for the commit header (from its measured height).
    header_rows: usize,
    /// Line totals of the working-directory sections / groups and the state they were
    /// summed from (once per state, not per frame).
    worktree_totals: RefCell<Option<(Arc<WorkingState>, gitti_core::rows::WorktreeTotals)>>,
    /// Measured full height of the commit box.
    commit_box_h: Pixels,
    /// Window bounds of the message editor inside it, as last painted.
    commit_editor_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Row limit of the growing message editor: the commit box takes at most about half of
    /// the panel (`fit_commit_editor`).
    editor_max_rows: usize,
    /// Rows the message editor is fixed at, counted by gitti (`message_editor.rs`).
    editor_rows: message_editor::EditorRows,
    /// Height of the worktree detail panel at the last `fit_commit_editor`.
    detail_panel_h: Pixels,
    /// How far the commit box is shrunk (0 = full): scrolling down over the worktree diff
    /// shrinks it before the list scrolls, down to its first line (`commit_box_wheel`).
    commit_box_collapse: Cell<Pixels>,
    /// The commit message editor has the focus (the commit box is shown in full then);
    /// updated while rendering (focus events only come while the window is active).
    message_focused: Cell<bool>,
    /// Commit list width in px, measured while painting (responsive row layout).
    list_width: Rc<Cell<f32>>,
    /// Row layout the commit list is drawn with, its scroll offset is in these rows
    /// (`GittiApp::list_layout`).
    row_layout: Cell<commit_list::ListLayout>,
    /// The one-line rows' date column width, measured for (UI font family, size).
    date_col: RefCell<Option<(SharedString, f32, Pixels)>>,
    panes: PaneStates,
    /// Size of the area below the toolbar (panes + git log) and the window's viewport size,
    /// both measured while painting; the stored pane fractions are relative to the first
    /// (`render_panes` corrects it by the viewport change since).
    body_size: Rc<Cell<(Size<Pixels>, Size<Pixels>)>>,
    /// A complete history has been applied (the quick first screen must not replace it).
    history_full: bool,
    commit_graph_hint_shown: bool,
    /// git refused the repository (another user owns it, `safe.directory`) and the trust
    /// dialog was offered: once per tab, not on every refresh.
    trust_offered: bool,
    _subscriptions: Vec<Subscription>,
    _watcher: Option<gitti_core::watch::RepoWatcher>,
}

/// Resize states of the pane groups, shared by all tabs: one arrangement for every
/// repository, as last dragged (persisted as fractions, see [`PaneLayout`]).
#[derive(Clone)]
pub struct PaneStates {
    main: Entity<ResizableState>,
    vertical: Entity<ResizableState>,
    /// Commit list | detail while the sidebar is hidden (a group of two panes).
    main_no_sidebar: Entity<ResizableState>,
    /// The commit list's width at a press on the sidebar's edge: while it is dragged, the
    /// list keeps it and the detail gives way (`panes.rs`). `None` after any other press.
    keep_list: Rc<Cell<Option<f32>>>,
}

impl PaneStates {
    pub fn new(cx: &mut App) -> Self {
        Self {
            main: cx.new(|_| ResizableState::default()),
            vertical: cx.new(|_| ResizableState::default()),
            main_no_sidebar: cx.new(|_| ResizableState::default()),
            keep_list: Rc::default(),
        }
    }
}

impl GittiApp {
    pub fn new(
        path: PathBuf,
        askpass_env: Vec<(String, String)>,
        panes: PaneStates,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(Some(path), askpass_env, panes, Settings::load(), false, window, cx)
    }

    /// A tab restored at start-up: `settings` read once for all of them, and the
    /// repository not read until [`Self::load_if_deferred`] — the shown tab first, the
    /// others one at a time after it (their git processes slowed the shown one down).
    pub fn restored(
        path: PathBuf,
        askpass_env: Vec<(String, String)>,
        panes: PaneStates,
        settings: Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(Some(path), askpass_env, panes, settings, true, window, cx)
    }

    /// The start page shown while no repository tab is open (Open / Clone / Init).
    pub fn start_page(
        askpass_env: Vec<(String, String)>,
        panes: PaneStates,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(None, askpass_env, panes, Settings::load(), false, window, cx)
    }

    /// Whether this tab shows a repository (else the start / "not a repository" page).
    pub fn has_repo(&self) -> bool {
        self.repo.is_some()
    }

    /// This tab's folder was not there when it opened (e.g. on a drive not connected).
    pub fn folder_missing(&self) -> bool {
        matches!(self.open_kind, Some(gitti_core::OpenError::FolderMissing(_)))
    }

    fn create(
        path: Option<PathBuf>,
        askpass_env: Vec<(String, String)>,
        panes: PaneStates,
        settings: Settings,
        defer_load: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::theme::apply_text_antialiasing(settings.text_antialiasing, cx);
        let message = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).placeholder("Commit message");
            // Small while empty; grows with the text.
            s.set_auto_grow(1, EDITOR_MAX_ROWS, cx);
            s
        });
        let cmd_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("git command, e.g. log --oneline -5  (Enter to run, ↑↓ history)")
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &cmd_input,
                window,
                |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. }) {
                        this.run_typed_command(window, cx);
                    }
                },
            ),
            // The commit box expands while the editor has the focus (`render_detail`).
            cx.subscribe(&message, |this, _, ev: &InputEvent, cx| match ev {
                InputEvent::Focus | InputEvent::Blur => cx.notify(),
                // Spell marks follow the text before the next frame (`spell_ui`); the
                // draft is written once typing pauses (`commit_flow`).
                InputEvent::Change => {
                    this.update_spell_of(SpellTarget::Message, cx);
                    this.draft_changed(cx);
                    let blank = this.message_counts_as_blank(cx);
                    if blank != this.message_was_blank.get() {
                        cx.notify();
                    }
                }
                _ => {}
            }),
        ];
        let (repo, open_error, open_kind) = match path.as_ref().map(Repo::open) {
            Some(Ok(mut r)) => {
                r.set_env(askpass_env.clone());
                (Some(Arc::new(r)), None, None)
            }
            Some(Err(e)) => match e.downcast_ref::<gitti_core::OpenError>() {
                // Its message names the folder itself.
                Some(kind) => (None, Some(e.to_string()), Some(kind.clone())),
                None => (
                    None,
                    Some(format!("{}: {e}", path.clone().unwrap_or_default().display())),
                    None,
                ),
            },
            None => (None, None, None),
        };
        // The worktree marks of every tab (`tab_marks.rs`): redraw when any changed; this
        // tab's own entry goes when it closes (it stays as it was until read again).
        cx.observe_global::<tab_marks::SharedMarks>(|_, cx| cx.notify()).detach();
        let owner = cx.entity_id().as_u64();
        cx.on_release(move |_, cx| {
            if cx.has_global::<tab_marks::SharedMarks>() {
                cx.global_mut::<tab_marks::SharedMarks>().0.release(owner);
            }
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            this.sync_secure_input(window);
            cx.notify();
            if window.is_window_active() && this.loaded_once {
                this.refresh(false, window, cx);
            }
        })
        .detach();
        let mut this = Self {
            repo,
            open_error,
            open_kind,
            path,
            askpass_env,
            settings,
            history: Arc::new(History::default()),
            head: HeadInfo::default(),
            working: Arc::new(WorkingState::default()),
            refs: Vec::new(),
            remotes: Vec::new(),
            stashes: Vec::new(),
            repo_state: RepoState::Clean,
            tracking: Vec::new(),
            unpushed: None,
            worktree_marks_loading: false,
            watch_ready: false,
            rows_for: None,
            first_read_pending: false,
            identity: (None, None),
            conflict_sides: None,
            worktrees: Vec::new(),
            submodules: Vec::new(),
            subtrees: Vec::new(),
            subtrees_head: None,
            subtrees_loading: false,
            subtrees_queued: false,
            lfs: Default::default(),
            submodule_banner_dismissed: None,
            fs_banner_dismissed: None,
            signature: None,
            signers_offer: None,
            selected: 0,
            sidebar_scroll: ScrollHandle::new(),
            commit_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            detail: Detail::Empty,
            detail_generation: 0,
            commit_loading: false,
            commit_load_gen: 0,
            rows: Arc::new(Vec::new()),
            folders: Default::default(),
            file_marks: Default::default(),
            commit_marks: Default::default(),
            expansion: Expansion::default(),
            line_sel: None,
            dragging: false,
            text_sel: None,
            text_dragging: false,
            drag_pos: None,
            autoscrolling: false,
            diff_list_bounds: Rc::default(),
            drag_targets: Default::default(),
            menu_row: None,
            message,
            spell: Default::default(),
            merge_prefill: None,
            prepared: None,
            prepared_dismissed: None,
            amend: false,
            commit_box: Default::default(),
            draft_dirty: false,
            draft_save: None,
            amend_draft: None,
            message_was_blank: Cell::new(true),
            status: None,
            errors: Default::default(),
            status_gen: 0,
            status_fading: 0,
            palette_room: Rc::default(),
            busy: None,
            pending: None,
            secure_input: Default::default(),
            busy_visible: false,
            pending_visible: false,
            working_generation: 0,
            watch_pending: None,
            watch_filter: Default::default(),
            watch_seen: Vec::new(),
            deferred: Default::default(),
            op_cancel: None,
            op_progress: Default::default(),
            op_output: Default::default(),
            busy_generation: 0,
            list_focus: cx.focus_handle(),
            side_focus: cx.focus_handle(),
            side_cursor: None,
            side_cursor_keys: Vec::new(),
            diff_focus: cx.focus_handle(),
            loaded_once: false,
            load_deferred: false,
            worktree_tab: None,
            worktree_tabs: RefCell::new(None),
            refresh_in_flight: false,
            refresh_queued: None,
            history_in_flight: false,
            history_queued: None,
            paths_in_flight: false,
            paths_queued: None,
            collapsed: HashSet::new(),
            branches: Default::default(),
            modal: None,
            ask_queue: Default::default(),
            ask_shown: None,
            replacing_modal: false,
            picker: None,
            wrap_cols: Rc::new(Cell::new(120)),
            note_wrap: fs_paths::NoteWrapper::new(cx),
            commit_fs: Default::default(),
            diff_viewport_h: Rc::new(Cell::new(px(0.))),
            log_scroll: UniformListScrollHandle::new(),
            log_focus: cx.focus_handle(),
            log_seen: Cell::new(0),
            log_view: RefCell::new(None),
            log_view_builds: Cell::new(0),
            highlights: Default::default(),
            hl_cache: Default::default(),
            commit_hl: Default::default(),
            prefetching: Default::default(),
            last_shown_row: 0,
            hl_generation: 0,
            worktree_hl: None,
            cmd_input,
            cmd_history: Vec::new(),
            cmd_history_pos: None,
            settings_open: false,
            settings_section: Default::default(),
            settings_focus: cx.focus_handle(),
            settings_scroll: ScrollHandle::new(),
            nav_candidates: Vec::new(),
            last_merge: Default::default(),
            last_cherry_pick: Default::default(),
            file_tab: None,
            view_anchor: None,
            pending_file: None,
            compare: Default::default(),
            change_search: Default::default(),
            reflog: Default::default(),
            repo_stats: Default::default(),
            header_rows: 0,
            worktree_totals: RefCell::new(None),
            commit_box_h: px(0.),
            commit_editor_bounds: Rc::default(),
            editor_max_rows: EDITOR_MAX_ROWS,
            editor_rows: Default::default(),
            detail_panel_h: px(0.),
            commit_box_collapse: Cell::new(px(0.)),
            message_focused: Cell::new(false),
            history_full: false,
            list_width: Rc::new(Cell::new(640.)),
            row_layout: Cell::new(commit_list::ListLayout::Columns),
            date_col: RefCell::default(),
            panes,
            body_size: Rc::default(),
            commit_graph_hint_shown: false,
            trust_offered: false,
            _subscriptions: subscriptions,
            _watcher: None,
        };
        // A git older than the supported minimum: said once per tab, stays until the next
        // operation.
        if this.repo.is_some()
            && let Some(warning) = gitti_core::caps::GitCaps::get().warning()
        {
            this.set_error(warning);
        }
        this.apply_log_persistence();
        // Closing the tab or the window, quitting: the typed message is kept.
        cx.on_release(|this, cx| this.save_draft_now(cx)).detach();
        if defer_load {
            this.load_deferred = this.repo.is_some();
        } else {
            this.begin_load(window, cx);
        }
        this.list_focus.focus(window, cx);
        this
    }

    /// Reads the repository: draft, watcher, the newest commits, then working state and
    /// the whole history.
    fn begin_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_draft(window, cx);
        self.start_watcher(window, cx);
        self.load_quick_history(window, cx);
        self.first_read(window, cx);
    }

    /// The tab comes to the front: what it shows is what it has read — a working-directory
    /// view built from an older working state is built again (no git; its watcher kept
    /// the state current in the background), and the view is drawn anew.
    pub fn brought_to_front(&mut self, cx: &mut Context<Self>) {
        let stale = matches!(self.detail, Detail::Worktree)
            && !self.rows_for.as_ref().is_some_and(|w| Arc::ptr_eq(w, &self.working));
        if stale {
            self.rebuild_rows();
            self.schedule_highlight(cx);
        }
        cx.notify();
    }

    /// A restored tab's turn to read its repository; whether that started now
    /// ([`AppEvent::FirstLoadDone`] follows).
    pub fn load_if_deferred(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !std::mem::take(&mut self.load_deferred) {
            return false;
        }
        self.begin_load(window, cx);
        true
    }

    /// The first read of the repository is running ([`AppEvent::FirstLoadDone`] follows).
    pub fn first_load_running(&self) -> bool {
        self.repo.is_some() && !self.loaded_once && !self.load_deferred
    }

    /// `waiting` (a restored tab before its turn), `loading`, `loaded` — for tests.
    pub fn script_load_state(&self) -> &'static str {
        match (self.load_deferred, self.loaded_once) {
            (true, _) => "waiting",
            (false, false) => "loading",
            (false, true) => "loaded",
        }
    }

    /// Tab title.
    pub fn title(&self) -> String {
        if let Some(r) = &self.repo {
            return r.name();
        }
        // A folder that is missing keeps its name: the tab says which one comes back.
        self.path
            .as_ref()
            .filter(|_| self.folder_missing())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(no repository)".into())
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// The running git operation's label (quitting asks while there is one).
    pub fn busy_label(&self) -> Option<&str> {
        self.busy.as_deref()
    }

    /// Another tab may have changed the shared settings file.
    pub fn reload_settings(&mut self, cx: &mut Context<Self>) {
        let fresh = Settings::load();
        // gitty or another window may have changed the keyboard layout.
        self.sync_keyboard_layout(cx);
        let rows_changed = fresh.wrap_lines != self.settings.wrap_lines;
        crate::theme::apply_text_antialiasing(fresh.text_antialiasing, cx);
        self.settings = fresh;
        self.apply_log_persistence();
        if rows_changed {
            self.rebuild_rows_keep_top(cx);
        }
        cx.notify();
    }

    pub fn focus_list(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.list_focus.focus(window, cx);
    }

    fn order(&self) -> LogOrder {
        if self.settings.topo_order {
            LogOrder::Topo
        } else {
            LogOrder::Date
        }
    }

    // ------------------------------------------------------------------ loading

    /// Newest commits only, so huge repositories show something right away; the full
    /// history from the first `refresh` replaces it.
    fn load_quick_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const QUICK_ROWS: usize = 500;
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let order = self.order();
        let task = cx
            .background_executor()
            .spawn(async move { repo.history_newest(order, QUICK_ROWS) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Ok(h) = result
                    && !this.history_full
                {
                    this.set_history(h);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Writes git's commit-graph file (much faster history loading), then reloads.
    pub(super) fn write_commit_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_op(
            "Writing commit-graph",
            true,
            window,
            cx,
            |r| r.write_commit_graph(),
            |this, _, _, _| this.set_info("commit-graph written — history loads faster now"),
        );
    }

    /// Reloads working state and refs; with `full` also checks the history, in a job of its
    /// own: the working state shows as soon as it is read.
    fn refresh(&mut self, full: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        // Its turn reads everything anyway.
        if self.load_deferred {
            return;
        }
        if self.refresh_in_flight || self.paths_in_flight {
            self.refresh_queued = Some(self.refresh_queued.unwrap_or(false) || full);
            return;
        }
        self.refresh_in_flight = true;
        // It reads them too.
        self.paths_queued = None;
        self.watch_filter.read_started(full);
        if full {
            self.load_history(false, cx);
        }
        let opts = self.settings.diff_options();
        let generation = self.working_generation;
        let task = cx
            .background_executor()
            .spawn(async move { repo.snapshot(&opts) });
        cx.spawn_in(window, async move |this, cx| {
            let snapshot = task.await;
            this.update_in(cx, |this, window, cx| {
                this.refresh_in_flight = false;
                let first = !this.loaded_once;
                let failed = snapshot.is_err();
                // Rows now show the new state (or the operation failed and was reported).
                if this.busy.is_none() {
                    this.pending = None;
                }
                match snapshot {
                    Ok(mut s) => {
                        if this.working_generation != generation {
                            // An operation changed the working state meanwhile: this
                            // one is older than what is shown. Read again.
                            s.working = (*this.working).clone();
                            s.conflict_sides = this.conflict_sides.clone();
                            this.refresh_queued = Some(this.refresh_queued.unwrap_or(false));
                        }
                        let prepared = s.prepared_message.take();
                        this.commit_box.template = s.commit_template.take();
                        this.commit_box.encoding = std::mem::take(&mut s.commit_encoding);
                        this.errors.succeeded("Refresh");
                        this.apply_snapshot(s, cx);
                        this.sync_prepared_message(prepared, window, cx);
                    }
                    Err(e) => {
                        let text = gitti_core::error_mark::refresh_failed(&format!("{e:#}"));
                        this.op_failed("Refresh", text);
                        if let Some(d) = gitti_core::safe_directory::dubious_ownership(&e) {
                            this.offer_trust_directory(d, window, cx);
                        }
                    }
                }
                if first {
                    this.report_startup_tab(failed, cx);
                    cx.emit(AppEvent::FirstLoadDone);
                }
                if let Some(full) = this.refresh_queued.take() {
                    this.refresh(full, window, cx);
                } else if let Some(paths) = this.paths_queued.take() {
                    this.refresh_paths(paths, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply_snapshot(&mut self, s: RepoSnapshot, cx: &mut Context<Self>) {
        let refs_changed = !same_refs(&self.refs, &s.refs) || self.head.id != s.head.id;
        if refs_changed || self.head.branch != s.head.branch {
            // The open reflog has new entries (`reflog.rs`).
            self.reflog_refs_moved(cx);
        }
        if !self.loaded_once {
            if s.refs.iter().filter(|r| r.kind == RefKind::Tag).count() > 25 {
                self.collapsed.insert("tags".into());
            }
            for r in &s.remotes {
                let n = s
                    .refs
                    .iter()
                    .filter(|x| {
                        x.kind == RefKind::RemoteBranch
                            && x.short_name.starts_with(&format!("{}/", r.name))
                    })
                    .count();
                if n > 25 {
                    self.collapsed.insert(format!("remote:{}", r.name));
                }
            }
        }
        let prev_sel_diff = self
            .line_sel
            .as_ref()
            .and_then(|sel| self.file_diff(sel.src, sel.file).cloned());
        let prev_working = std::mem::replace(&mut self.working, Arc::new(s.working));
        self.head = s.head;
        self.refs = s.refs;
        self.branches.recent = s.recent_checkouts;
        self.update_branch_order();
        self.remotes = s.remotes;
        self.stashes = s.stashes;
        self.repo_state = s.state;
        self.tracking = s.tracking;
        self.unpushed = s.unpushed;
        self.identity = s.identity;
        self.conflict_sides = s.conflict_sides;
        self.worktrees = s.worktrees;
        self.submodules = s.submodules;
        self.sync_side_cursor();
        self.apply_lfs_state(s.lfs, cx);
        // Only when HEAD moved (in-process, in the background).
        self.refresh_subtrees(false, cx);
        // This worktree's marks for every tab; the worktrees no tab has open (in the
        // background).
        self.publish_marks(cx);
        self.refresh_worktree_marks(cx);
        // Refs moved behind our back → the history may have changed too (the first
        // refresh loads it anyway).
        if refs_changed && self.loaded_once {
            self.load_history(false, cx);
        }
        self.loaded_once = true;
        // The working directory is row 0 of every history: its state shows before the
        // history has loaded.
        if matches!(self.detail, Detail::Empty) && self.selected == 0 {
            self.detail = Detail::Worktree;
        }
        self.rebuild_rows_if_worktree(prev_sel_diff, &prev_working);
        self.restore_worktree_view_anchor(cx);
        if matches!(self.detail, Detail::Worktree) {
            self.schedule_highlight(cx);
        }
        self.check_compare_base(cx);
        self.refresh_worktree_compare(cx);
        // `.mailmap` changed: the shown header's names (the list re-reads its rows).
        if s.mailmap_changed && matches!(&self.detail, Detail::Commit(cv) if cv.compare.is_none()) {
            self.show_selected_commit(cx);
        }
    }

    /// (Re)loads the history in the background, apart from the working state: `force` loads
    /// it in full, else [`Repo::history_update`] keeps the shown one when no ref moved and
    /// reuses its rows where it can. One at a time; asked for meanwhile, it runs once more
    /// afterwards.
    fn load_history(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if self.history_in_flight {
            self.history_queued = Some(self.history_queued.unwrap_or(false) || force);
            return;
        }
        self.history_in_flight = true;
        let order = self.order();
        let prev = (!force && self.history_full).then(|| self.history.clone());
        let executor = cx.background_executor().clone();
        let task = cx.background_executor().spawn(async move {
            // Tests hold it back to see the working state come first.
            while HOLD_HISTORY.load(std::sync::atomic::Ordering::Relaxed) {
                executor.timer(std::time::Duration::from_millis(10)).await;
            }
            match prev {
                Some(prev) => repo.history_update(&prev, order),
                None => repo.history(order).map(Some),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.history_in_flight = false;
                match result {
                    // Another order was chosen meanwhile: the queued load brings it.
                    Ok(Some(h)) if h.order() == this.order() => this.apply_history(h, cx),
                    Ok(_) => {}
                    Err(e) => this.op_failed("Refresh", format!("{e:#}")),
                }
                if let Some(force) = this.history_queued.take() {
                    this.load_history(force, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply_history(&mut self, hist: History, cx: &mut Context<Self>) {
        if !hist.used_commit_graph && hist.commits.len() > 30_000 && !self.commit_graph_hint_shown {
            self.commit_graph_hint_shown = true;
            self.set_info(format!(
                "{} commits and no commit-graph file — \"Write Commit-Graph\" in the command palette makes loading much faster",
                hist.commits.len()
            ));
        }
        self.history_full = true;
        if self.set_history(hist) {
            // What was selected is gone: the row that took its place, its detail shown
            // (in view; the list keeps its scroll position when it already is).
            self.show_row(self.selected, cx);
        } else {
            self.renumber_shown_stash();
        }
        self.check_compare_base(cx);
    }

    /// The shown stash moved in the stash list (one above it dropped / popped, another
    /// pushed): its header says its new `stash@{n}`.
    fn renumber_shown_stash(&mut self) {
        let Detail::Commit(cv) = &self.detail else {
            return;
        };
        let Some(c) = self.history.commits.get(self.selected) else {
            return;
        };
        if cv.compare.is_none() && cv.details.id == c.id && cv.stash.is_some() && cv.stash != c.stash {
            self.detail = Detail::Commit(Arc::new(CommitView {
                data: cv.data.clone(),
                stash: c.stash,
                compare: None,
            }));
        }
    }

    /// Shows `hist`. The selection stays on its commit (by id); one that vanished goes to
    /// the commit taking its place ([`gitti_core::reselect::history_row`]) — `true` then:
    /// the caller shows that row's detail.
    fn set_history(&mut self, hist: History) -> bool {
        let prev = std::mem::replace(&mut self.history, Arc::new(hist));
        // Sorting by date reads the new history's commit times.
        self.update_branch_order();
        self.sync_side_cursor();
        let pick = gitti_core::reselect::history_row(&prev, self.selected, &self.history);
        let new_sel = pick.row();
        // New commits above the selected one (or it moved in a new order): keep it in its
        // place on screen instead of letting the list slide under it — unless the list is
        // at its top, where the new commits are what one wants to see.
        if matches!(pick, Reselect::Kept(_))
            && !prev.is_worktree_row(self.selected)
            && new_sel != self.selected
        {
            let state = self.commit_scroll.0.borrow();
            let handle = &state.base_handle;
            let offset = handle.offset();
            if offset.y < px(0.) {
                let rows = new_sel as f32 - self.selected as f32;
                handle.set_offset(point(offset.x, offset.y - self.commit_row_h() * rows));
            }
        }
        if matches!(self.detail, Detail::Empty) {
            self.detail = if self.history.worktree_row {
                Detail::Worktree
            } else {
                Detail::Empty
            };
        }
        self.selected = new_sel.min(self.history.commits.len().saturating_sub(1));
        // Marks of commits the new history no longer lists go.
        let history = self.history.clone();
        self.commit_marks.retain(&history);
        matches!(pick, Reselect::Replaced(_))
    }

    /// New rows for a new working state (`prev` = the one the current rows show). What is
    /// at the top of the view stays there, also when a hunk above it was staged / unstaged.
    fn rebuild_rows_if_worktree(&mut self, prev_sel_diff: Option<FileDiff>, prev: &WorkingState) {
        if !matches!(self.detail, Detail::Worktree) {
            return;
        }
        let anchor = self.worktree_anchor(prev);
        self.rows = Arc::new(self.worktree_rows(self.wrap()));
        self.rows_for = Some(self.working.clone());
        if let Some(a) = anchor {
            self.follow_worktree_anchor(&a, prev);
        }
        let sel = self.line_sel.take();
        self.line_sel = sel
            .zip(prev_sel_diff)
            .and_then(|(sel, prev)| self.remap_selection(sel, &prev));
    }

    /// Keeps a line selection alive across a refresh if its file's diff did not change.
    fn remap_selection(&self, mut sel: LineSel, prev: &FileDiff) -> Option<LineSel> {
        let files = self.work_files(sel.src)?;
        let new_file = files.iter().position(|f| f.diff.as_ref() == Some(prev))?;
        sel.file = new_file;
        let (h, l) = *sel.lines.first()?;
        sel.anchor_row = self.rows.iter().position(|r| {
            *r == DiffRow::Line {
                src: sel.src,
                file: new_file,
                hunk: h,
                line: l,
                part: 0,
            }
        })?;
        Some(sel)
    }

    /// Wrap width in columns, if line wrapping is on.
    fn wrap(&self) -> Option<usize> {
        self.settings
            .wrap_lines
            .then(|| self.wrap_cols.get().max(1))
    }

    fn rebuild_rows(&mut self) {
        let wrap = self.wrap();
        let before = self.rows.clone();
        self.commit_fs = self.shown_commit_fs();
        self.folders = Default::default();
        self.rows_for = matches!(self.detail, Detail::Worktree).then(|| self.working.clone());
        self.rows = Arc::new(match &self.detail {
            Detail::Worktree => self.worktree_rows(wrap),
            Detail::Commit(cv) => {
                // A comparison has its banner above the list instead of a header.
                let header = if cv.compare.is_some() { 0 } else { self.header_rows };
                let mut rows = vec![DiffRow::HeaderSpace; header];
                let built = self.with_note_wrap(|notes| {
                    build_commit_rows_noted(
                        &cv.files,
                        self.file_tab,
                        &self.expansion,
                        wrap,
                        Some(&self.commit_fs),
                        notes,
                    )
                });
                rows.extend(self.change_search_rows(cv, built));
                rows
            }
            _ => Vec::new(),
        });
        // Row indices of a character selection only stay meaningful for the same rows.
        if *before != *self.rows {
            self.text_sel = None;
        }
    }

    /// The commit box was measured at `h` (its natural height).
    fn set_commit_box_h(&mut self, h: Pixels, cx: &mut Context<Self>) {
        self.commit_box_h = h;
        // A font size change also changes the box: fit the editor again.
        self.fit_commit_editor(self.detail_panel_h, cx);
        cx.notify();
    }

    /// Limits the growing message editor so that the commit box takes at most half of the
    /// detail panel (`panel_h`); at least two rows, at most [`EDITOR_MAX_ROWS`].
    fn fit_commit_editor(&mut self, panel_h: Pixels, cx: &mut Context<Self>) {
        self.detail_panel_h = panel_h;
        let line_h = self.settings.message_line_h();
        let editor_h = self
            .commit_editor_bounds
            .get()
            .map_or(px(0.), |b| b.size.height);
        if self.commit_box_h <= px(0.) || editor_h <= px(0.) || line_h <= px(0.) {
            return;
        }
        // Everything in the box but the editor's text lines: identity, buttons, paddings
        // and the editor's own border and padding.
        let rows_now = self.message_rows_shown(editor_h, line_h);
        let chrome = self.commit_box_h - line_h * rows_now as f32;
        let rows = ((panel_h * 0.5 - chrome) / line_h).floor().clamp(2., EDITOR_MAX_ROWS as f32)
            as usize;
        if rows != self.editor_max_rows {
            // The editor takes it over on the next render (`sync_editor_rows`).
            self.editor_max_rows = rows;
            cx.notify();
        }
    }

    /// Left press on the text of line `line` of `area` (laid out as `layout`): a click
    /// starts a character selection (shift: extends the current one), a double click
    /// selects the word, a triple click the line (`text_sel::press`).
    fn press_text(
        &mut self,
        area: crate::text_sel::Area,
        line: usize,
        layout: &TextLayout,
        text: &str,
        ev: &MouseDownEvent,
    ) {
        use crate::text_sel::{TextPos, char_index, press, text_index};
        let pos = TextPos {
            area,
            line,
            byte: text_index(layout, text, ev.position),
        };
        let under = char_index(layout, text, ev.position);
        let sel = press(
            self.text_sel,
            pos,
            under,
            ev.click_count,
            ev.modifiers.shift,
            self.sel_rows(area),
        );
        self.text_sel = Some(sel);
        self.text_dragging = true;
    }

    /// The rows of a selectable text area as displayed, by line index: the commit
    /// header's author / date / committer, the commit message's lines, the diff's rows (a
    /// wrapped line's parts continue it).
    fn sel_rows(
        &self,
        area: crate::text_sel::Area,
    ) -> impl Fn(usize) -> Option<crate::text_sel::Row> + '_ {
        use crate::text_sel::{Area, Row};
        let message: Vec<String> = match (&self.detail, area) {
            (Detail::Commit(cv), Area::Message) => {
                crate::message::style_message(&cv.details.message)
                    .into_iter()
                    .map(|l| l.text)
                    .collect()
            }
            (Detail::Commit(cv), Area::Header) => header_ids::header_lines(&cv.details),
            _ => Vec::new(),
        };
        move |line| match area {
            Area::Header | Area::Message => message.get(line).map(|t| Row::line(t.clone())),
            Area::Diff => self.diff_sel_row(line),
            _ => None,
        }
    }

    /// Diff row `ix` as displayed, if it has selectable text (a hunk line or a revealed
    /// unchanged line; each wrapped part is a row of its own).
    fn diff_sel_row(&self, ix: usize) -> Option<crate::text_sel::Row> {
        let (raw, part) = match *self.rows.get(ix)? {
            DiffRow::Line {
                src,
                file,
                hunk,
                line,
                part,
            } => {
                let l = self.file_diff(src, file)?.hunks.get(hunk)?.lines.get(line)?;
                (l.text.as_str(), part)
            }
            DiffRow::Context {
                src,
                file,
                line,
                part,
            } => {
                let path = self.file_path(src, file)?;
                (self.expansion.context_line(src, &path, line)?, part)
            }
            _ => return None,
        };
        let (text, _, _) = line_part_range(raw, part, self.wrap(), MAX_LINE_CHARS);
        Some(crate::text_sel::Row {
            text,
            continues: part > 0,
        })
    }

    /// Mouse moved anywhere in the window with the button held: extends a character
    /// selection to the nearest message / diff line position.
    /// The mouse moved with the button held during a selection drag (anywhere in the
    /// window): text selections follow it; above / below the diff list it auto-scrolls.
    fn drag_moved(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.text_dragging && !self.dragging {
            return;
        }
        self.drag_pos = Some(pos);
        self.drag_text_sel(pos, cx);
        if !self.autoscrolling && self.drag_overshoot().is_some() {
            self.autoscrolling = true;
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                    let go_on = this
                        .update(cx, |this, cx| this.drag_autoscroll_step(cx))
                        .unwrap_or(false);
                    if !go_on {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// How far (px) the dragging mouse is above (negative) or below the diff list.
    fn drag_overshoot(&self) -> Option<Pixels> {
        let pos = self.drag_pos?;
        let b = self.diff_list_bounds.get()?;
        if pos.y < b.top() {
            Some(pos.y - b.top())
        } else if pos.y > b.bottom() {
            Some(pos.y - b.bottom())
        } else {
            None
        }
    }

    /// One tick of auto-scrolling while a selection is dragged past the diff list's top or
    /// bottom: faster the farther out the mouse is; the selection follows to the edge.
    /// Returns whether to keep ticking.
    fn drag_autoscroll_step(&mut self, cx: &mut Context<Self>) -> bool {
        let over = self
            .drag_overshoot()
            .filter(|_| self.dragging || self.text_dragging);
        let (Some(over), Some(b)) = (over, self.diff_list_bounds.get()) else {
            self.autoscrolling = false;
            return false;
        };
        let speed = (over.abs() * 0.35).clamp(px(2.), px(80.));
        let step = if over < px(0.) { -speed } else { speed };
        let row_h = self.settings.diff_row_h();
        let max = (row_h * self.rows.len() as f32 - b.size.height).max(px(0.));
        let handle = self.diff_scroll.0.borrow().base_handle.clone();
        let y = (-handle.offset().y + step).clamp(px(0.), max);
        handle.set_offset(point(handle.offset().x, -y));
        if self.dragging {
            // The line selection reaches the row at the edge the mouse is beyond.
            let edge = if over < px(0.) { px(0.) } else { b.size.height - px(1.) };
            let row = ((y + edge) / row_h).floor() as usize;
            self.extend_selection(row.min(self.rows.len().saturating_sub(1)));
        }
        if let Some(pos) = self.drag_pos {
            self.drag_text_sel(pos, cx);
        }
        cx.notify();
        true
    }

    fn drag_text_sel(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        use crate::text_sel::{Area, unit_at};
        let Some(mut sel) = self.text_sel.filter(|_| self.text_dragging) else {
            return;
        };
        let Some((at, under)) = self
            .drag_targets
            .hit(pos, |a| matches!(a, Area::Header | Area::Message | Area::Diff))
        else {
            return;
        };
        // A word / line selection extends by whole words / lines.
        let span = sel
            .unit
            .map(|(unit, _, _)| unit_at(unit, at, under, self.sel_rows(at.area)));
        if sel.drag_to(at, span) {
            self.text_sel = Some(sel);
            cx.notify();
        }
    }

    /// The selected characters (header lines, message lines, then diff rows; wrapped rows
    /// joined), else the full text of the selected diff lines.
    fn selected_text(&self) -> Option<String> {
        use crate::text_sel::Area;
        let mut out = String::new();
        // `started`: a line was pushed already (it may have been empty).
        let mut started = false;
        let mut push = |piece: &str, continuation: bool, out: &mut String| {
            if started && !continuation {
                out.push('\n');
            }
            started = true;
            out.push_str(piece);
        };
        if let Some(sel) = self.text_sel.filter(|s| !s.is_empty()) {
            if let Detail::Commit(cv) = &self.detail {
                for (i, l) in header_ids::header_lines(&cv.details).iter().enumerate() {
                    if let Some(r) = sel.span_in(Area::Header, i, l.len()) {
                        push(&l[r], false, &mut out);
                    }
                }
                let lines = crate::message::style_message(&cv.details.message);
                for (i, l) in lines.iter().enumerate() {
                    if let Some(r) = sel.span_in(Area::Message, i, l.text.len()) {
                        push(&l.text[r], false, &mut out);
                    } else if l.text.is_empty() && sel.covers_line(Area::Message, i) {
                        push("", false, &mut out);
                    }
                }
            }
            // The diff rows the selection reaches (hunk lines and revealed lines).
            let (start, end) = sel.ordered();
            if end.area == Area::Diff {
                let first = if start.area == Area::Diff { start.line } else { 0 };
                for ix in first..=end.line {
                    let Some(row) = self.diff_sel_row(ix) else {
                        continue;
                    };
                    if let Some(r) = sel.span_in(Area::Diff, ix, row.text.len()) {
                        push(&row.text[r], row.continues, &mut out);
                    } else if row.text.is_empty() && sel.covers_line(Area::Diff, ix) {
                        push("", row.continues, &mut out);
                    }
                }
            }
            return Some(out);
        }
        let ls = self.line_sel.as_ref()?;
        let f = self.file_diff(ls.src, ls.file)?;
        for (h, l) in &ls.lines {
            if let Some(line) = f.hunks.get(*h).and_then(|hk| hk.lines.get(*l)) {
                push(&line.text, false, &mut out);
            }
        }
        Some(out)
    }

    /// The file and line a diff row stands for (`DiffRow::editor_line`).
    fn row_location(&self, row: usize) -> Option<(String, u32)> {
        let row = *self.rows.get(row)?;
        let (src, file) = row.file_ref()?;
        let line = row.editor_line(self.file_diff(src, file)?);
        Some((self.file_path(src, file)?, line))
    }

    /// Row the keyboard command acts on: the first selected line, else the first file.
    fn diff_cursor_row(&self) -> usize {
        let selected = self.line_sel.as_ref().and_then(|s| {
            self.rows.iter().position(|r| {
                matches!(*r, DiffRow::Line { src, file, hunk, line, .. }
                    if src == s.src && file == s.file && s.lines.contains(&(hunk, line)))
            })
        });
        selected
            .or_else(|| self.rows.iter().position(|r| matches!(r, DiffRow::File { .. })))
            .unwrap_or(0)
    }

    /// Opens the working-tree file of `row` at its line in the configured editor.
    fn open_in_editor(&mut self, row: usize, _: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some((path, line)) = self.row_location(row) else {
            self.set_info("Nothing to open here");
            cx.notify();
            return;
        };
        // A name that is not UTF-8 by its bytes.
        let full = repo.fs_path(&path);
        if !full.exists() {
            self.set_error(format!("{path} does not exist in the working tree"));
            cx.notify();
            return;
        }
        match open_editor(&self.settings.editor_command, &full, line, repo.workdir()) {
            Ok(()) => self.set_info(format!("Opened {path}:{line}")),
            Err(e) => self.set_error(format!("{e:#}")),
        }
        cx.notify();
    }

    fn on_open_in_editor(&mut self, _: &OpenInEditor, w: &mut Window, cx: &mut Context<Self>) {
        let row = self.diff_cursor_row();
        self.open_in_editor(row, w, cx);
    }

    fn on_copy_selection(&mut self, _: &CopySelection, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text().filter(|t| !t.is_empty()) {
            let n = text.lines().count();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.set_info(format!("Copied {n} line(s)"));
            cx.notify();
        }
    }

    /// Called from the diff panel's paint pass when its width changed.
    fn set_wrap_cols(&mut self, cols: usize, cx: &mut Context<Self>) {
        if self.wrap_cols.get() != cols {
            self.wrap_cols.set(cols);
            if self.settings.wrap_lines {
                self.rebuild_rows_keep_top(cx);
                cx.notify();
            }
        }
    }

    pub fn select_row(&mut self, row: usize, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_row(row, cx);
    }

    /// [`Self::select_row`] without a window (a history load that replaced the selection).
    fn show_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if row >= self.history.commits.len() {
            return;
        }
        self.selected = row;
        self.line_sel = None;
        self.commit_scroll
            .scroll_to_item(row, ScrollStrategy::Nearest);
        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if self.show_compare(cx) {
            return;
        }
        if self.history.is_worktree_row(row) {
            self.detail = Detail::Worktree;
            self.expansion = Expansion::default();
            self.rebuild_rows();
            self.schedule_highlight(cx);
            cx.notify();
            return;
        }
        self.show_selected_commit(cx);
    }

    /// Shows the selected commit row: at once when it is cached, else after loading it (one
    /// load at a time — a load that ends looks at the selection again). Prefetches the
    /// neighbours so arrowing through the history hits the cache.
    fn show_selected_commit(&mut self, cx: &mut Context<Self>) {
        self.show_selected_commit_or(None, cx);
    }

    /// [`Self::show_selected_commit`]; `loaded` is what the load that just ended returned
    /// (with its diff options) — a commit too big for the core's cache is only there (it
    /// was looked up in the cache again: an endless "Loading…").
    fn show_selected_commit_or(
        &mut self,
        loaded: Option<(gitti_core::DiffOptions, Arc<gitti_core::commit_cache::CommitData>)>,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        // The reflog view's selected entry has the detail pane (`reflog.rs`).
        if self.reflog_shows_commit(cx) {
            return;
        }
        let row = self.selected;
        let Some(c) = self.history.commits.get(row) else {
            return;
        };
        let (id, stash) = (c.id, c.stash);
        if self.history.is_worktree_row(row) || self.show_compare(cx) {
            return;
        }
        let opts = self.settings.diff_options();
        let loaded = loaded
            .filter(|(o, d)| *o == opts && d.details.id == id)
            .map(|(_, d)| d);
        if let Some(data) = repo.cached_commit(id, &opts).or(loaded) {
            self.detail_generation += 1;
            self.show_commit(
                CommitView {
                    data,
                    stash,
                    compare: None,
                },
                cx,
            );
            self.prefetch_around(row, cx);
            cx.notify();
            return;
        }
        // Keep the old commit visible while loading; avoids flicker when arrowing through.
        if matches!(self.detail, Detail::Worktree | Detail::Empty | Detail::Failed(_)) {
            self.detail = Detail::Loading;
            self.rows = Arc::new(Vec::new());
        }
        cx.notify();
        if self.commit_loading {
            return;
        }
        self.commit_loading = true;
        self.commit_load_gen += 1;
        let generation = self.commit_load_gen;
        let task = cx
            .background_executor()
            .spawn(async move { repo.load_commit(id, stash, &opts) });
        // A slow load (a merge of 1700 files: over a second) shows "Loading…" instead of
        // the previous commit, which would look like the selected one meanwhile.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SLOW_LOAD).await;
            this.update(cx, |this, cx| {
                if this.commit_loading
                    && this.commit_load_gen == generation
                    && matches!(this.detail, Detail::Commit(_))
                {
                    this.detail = Detail::Loading;
                    this.rows = Arc::new(Vec::new());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.commit_loading = false;
                match result {
                    // Shows it if still selected, else loads the new selection.
                    Ok(data) => this.show_selected_commit_or(Some((opts, data)), cx),
                    // Still selected: why it isn't shown (not "Loading…" or the previous
                    // commit). Else the selection moved on: load that one.
                    Err(e) => {
                        let still = this.history.commits.get(this.selected).is_some_and(|c| c.id == id)
                            && !this.history.is_worktree_row(this.selected);
                        if still {
                            let text = format!("{e:#}");
                            this.detail = Detail::Failed(text.clone());
                            this.rows = Arc::new(Vec::new());
                            this.set_error(text);
                            cx.notify();
                        } else {
                            this.show_selected_commit_or(None, cx);
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn show_commit(&mut self, cv: CommitView, cx: &mut Context<Self>) {
        let same = matches!(&self.detail, Detail::Commit(old) if Arc::ptr_eq(&old.data, &cv.data));
        if same {
            return;
        }
        if cv.sig_kind.is_some() {
            self.verify_signature(cv.details.id, self.detail_generation, cx);
        }
        self.expansion = Expansion::default();
        self.file_tab = None;
        self.detail = Detail::Commit(Arc::new(cv));
        self.rebuild_rows();
        self.apply_pending_file(cx);
        self.restore_view_anchor(cx);
        self.schedule_highlight(cx);
    }

    /// "Load anyway": the diff of file `file` of the shown commit, stash or comparison,
    /// left out as too large, loaded in the background; the view keeps its place.
    pub(super) fn load_omitted_file(&mut self, file: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Detail::Commit(cv) = &self.detail else {
            return;
        };
        let Some(path) = cv
            .files
            .get(file)
            .filter(|f| gitti_core::commit_files::loadable(f))
            .map(|f| f.path().to_string())
        else {
            return;
        };
        let (data, compare) = (cv.data.clone(), cv.compare.clone());
        let opts = self.settings.diff_options();
        self.set_info(format!("Loading the diff of {path}…"));
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            match compare {
                Some(c) => repo
                    .load_omitted_compare_file(&c, file, &opts)
                    .map(|c| (c.data.clone(), Some(c))),
                None => repo.load_omitted_file(&data, file, &opts).map(|d| (d, None)),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((data, compare)) => {
                        // Still the same commit / comparison on screen?
                        let same = |cv: &CommitView| match (&cv.compare, &compare) {
                            (None, None) => cv.details.id == data.details.id,
                            (Some(a), Some(b)) => (a.old, a.new) == (b.old, b.new),
                            _ => false,
                        };
                        if let Detail::Commit(cv) = &this.detail
                            && same(cv)
                        {
                            let stash = cv.stash;
                            // Asked for: shown even when long (not auto-collapsed).
                            if let Some(f) = data.files.get(file)
                                && !this.expansion.is_expanded(Src::Commit, f)
                            {
                                this.expansion.toggle(Src::Commit, f.path());
                            }
                            this.detail = Detail::Commit(Arc::new(CommitView {
                                data,
                                stash,
                                compare,
                            }));
                            this.rebuild_rows_keep_top(cx);
                            this.schedule_highlight(cx);
                            this.set_info(format!("Loaded the diff of {path}"));
                        }
                    }
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads (and highlights) the rows next to `row` in the background: three ahead in the
    /// direction the selection moves, one behind — all in one task and one `git show`
    /// (stashes each their own). A commit already being prefetched is skipped.
    fn prefetch_around(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let down = row >= self.last_shown_row;
        self.last_shown_row = row;
        let step = |n: usize| if down { row.checked_add(n) } else { row.checked_sub(n) };
        let back = if down { row.checked_sub(1) } else { row.checked_add(1) };
        let opts = self.settings.diff_options();
        let highlight = self.settings.syntax_highlight;
        let mut todo = Vec::new();
        for r in [step(1), step(2), step(3), back].into_iter().flatten() {
            if self.history.is_worktree_row(r) {
                continue;
            }
            let Some(c) = self.history.commits.get(r) else {
                continue;
            };
            if self.prefetching.lock().expect("prefetching").insert(c.id) {
                todo.push((c.id, c.stash));
            }
        }
        if todo.is_empty() {
            return;
        }
        let prefetching = self.prefetching.clone();
        let (hl_cache, commit_hl) = (self.hl_cache.clone(), self.commit_hl.clone());
        cx.background_executor()
            .spawn(async move {
                let commits: Vec<_> = todo
                    .iter()
                    .filter(|(_, s)| s.is_none())
                    .map(|(id, _)| *id)
                    .collect();
                let _ = repo.load_commits(&commits, &opts);
                for &(id, stash) in &todo {
                    let data = match stash {
                        None => repo.cached_commit(id, &opts),
                        Some(_) => repo.load_commit(id, stash, &opts).ok(),
                    };
                    if let Some(data) = data
                        && highlight
                    {
                        highlighting::prefetch_commit_highlights(
                            &repo, &data, opts, &hl_cache, &commit_hl,
                        );
                    }
                }
                let mut prefetching = prefetching.lock().expect("prefetching");
                for (id, _) in &todo {
                    prefetching.remove(id);
                }
            })
            .detach();
    }

    /// Verifies a commit signature in the background (gpg / ssh-keygen can take a while).
    fn verify_signature(&mut self, id: gitti_core::ObjectId, generation: u64, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.signature = None;
        self.signers_offer = None;
        let others = signers::open_repos();
        let task = cx.background_executor().spawn(async move {
            let info = repo.verify_signature(id).ok().flatten();
            // No allowed signers file set: which ones the user has (no git).
            let offer = info
                .as_ref()
                .filter(|i| gitti_core::allowed_signers::missing_setting(i))
                .and_then(|_| signers::SignersOffer::find(id, &repo, &others));
            (info, offer)
        });
        cx.spawn(async move |this, cx| {
            let (info, offer) = task.await;
            this.update(cx, |this, cx| {
                if this.detail_generation == generation {
                    this.signature = Some((id, info));
                    this.signers_offer = offer;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn set_error(&mut self, text: String) {
        self.status = Some(Status { text, error: true });
        self.status_gen += 1;
    }

    fn set_info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
        });
        self.status_gen += 1;
    }

    /// Runs a repository operation in the background, then refreshes.
    fn run_op<T: Send + 'static>(
        &mut self,
        label: &str,
        full_refresh: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&Repo) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if self.busy.is_some() {
            // A remote op waits for the running one (and then clears its warning
            // like any operation); others are refused, but the attempt still counts as
            // the user's next operation.
            if gitti_core::error_mark::waits_while_busy(label) && self.deferred.len() < 8 {
                let label = label.to_string();
                self.deferred.push_back(Box::new(move |this, w, cx| {
                    this.run_op(&label, full_refresh, w, cx, op, done)
                }));
                return;
            }
            self.refuse_busy(cx);
            return;
        }
        self.mark_busy(label, cx);
        let label = label.to_string();
        let cancel = self.start_cancellable();
        let sink = self.progress_sink(cx);
        let task = cx.background_executor().spawn(async move {
            let result = gitti_core::process::with_cancel(&cancel, || {
                gitti_core::process::with_progress(sink, || op(&repo))
            });
            let stale = (result.as_ref().err())
                .and_then(|e| gitti_core::lock::stale_lock(e, repo.workdir()));
            (result, stale)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (result, stale) = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(v) => {
                        this.errors.succeeded(&label);
                        done(this, v, window, cx)
                    }
                    // Esc on the busy mark: not a failure.
                    Err(_) if this.op_cancelled() => this.set_info(format!("{label} cancelled")),
                    Err(e) => match e.downcast_ref::<gitti_core::push::PullBlock>() {
                        // The upstream went since the last refresh: no git ran.
                        Some(block) => this.pull_blocked(block.clone(), window, cx),
                        // The same for the main Push.
                        None if e.is::<gitti_core::push::PushBlock>() => {
                            if let Some(b) = e.downcast_ref::<gitti_core::push::PushBlock>() {
                                this.push_blocked(b.clone(), window, cx)
                            }
                        }
                        // A refused push: the ways out (pull, push with lease, …).
                        None if e.is::<gitti_core::overwrite::PushRejected>() => {
                            if let Some(r) = e.downcast_ref::<gitti_core::overwrite::PushRejected>() {
                                this.push_rejected(r.clone(), window, cx)
                            }
                        }
                        // Diverged, no strategy: the user chooses one.
                        None if e.is::<gitti_core::pull::DivergentPull>() => {
                            if let Some(d) = e.downcast_ref::<gitti_core::pull::DivergentPull>() {
                                this.pull_diverged(d.clone(), window, cx)
                            }
                        }
                        None => {
                            if e.is::<gitti_core::ops::MessageCommitted>() {
                                // Continue used the box's text before git stopped again.
                                this.message_committed(window, cx);
                            }
                            this.op_failed(&label, format!("{e:#}"));
                            this.offer_unrelated_merge(&e, window, cx);
                            // Nothing to commit / refused for uncommitted changes: the
                            // ways out (`op_stops.rs`).
                            this.offer_ways_out(&e, window, cx);
                            if let Some(lock) = stale {
                                this.offer_stale_lock(&label, lock, window, cx);
                            }
                        }
                    },
                }
                this.watch_pending = None;
                this.refresh(full_refresh, window, cx);
                this.end_busy(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// An operation the user started while another one runs, refused: earlier warnings
    /// go all the same, the status says why nothing happened.
    fn refuse_busy(&mut self, cx: &mut Context<Self>) {
        self.user_op_started();
        self.set_error(format!(
            "Busy ({}) — try again in a moment",
            self.busy.clone().unwrap_or_default()
        ));
        cx.notify();
    }

    /// Sets the busy state; its marks show up only if the operation takes a while.
    fn mark_busy(&mut self, label: &str, cx: &mut Context<Self>) {
        // Every user-triggered operation comes here (`run_op`, `work_op`, clone / init).
        self.user_op_started();
        self.busy = Some(label.to_string());
        self.op_progress.clear();
        self.op_output.open = false;
        self.busy_generation += 1;
        let generation = self.busy_generation;
        for (delay, pending) in [(PENDING_SHOW_DELAY, true), (BUSY_SHOW_DELAY, false)] {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                this.update(cx, |this, cx| {
                    if this.busy.is_some() && this.busy_generation == generation {
                        if pending {
                            this.pending_visible = true;
                        } else {
                            this.busy_visible = true;
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    /// Clears the busy state, then runs what waited for it: file-system changes seen
    /// meanwhile, and a key action pressed meanwhile.
    fn end_busy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = None;
        self.op_cancel = None;
        self.op_progress.clear();
        // The output window goes with its operation (the Git Log has it all now).
        self.op_output.open = false;
        self.busy_visible = false;
        self.pending_visible = false;
        if let Some(change) = self.watch_pending.take() {
            self.on_watch(change, window, cx);
        }
        if let Some(next) = self.deferred.pop_front() {
            next(self, window, cx);
        }
        if self.busy.is_none() {
            cx.emit(AppEvent::Idle);
        }
    }

    /// A stage / unstage / discard / restore: runs `op`, then re-reads the working state of
    /// `paths` only (a folder: everything below it; `None`: all of it, but nothing else)
    /// and highlights those files — all in one
    /// background task, so the result shows up in one go a few ms later, without the full
    /// refresh (refs, remotes, stashes, config, …) an op does otherwise.
    fn work_op(
        &mut self,
        label: &str,
        paths: Option<Vec<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&Repo) -> anyhow::Result<()> + Send + 'static,
    ) {
        self.work_op_noted(label, paths, window, cx, move |r| op(r).map(|()| None));
    }

    /// [`Self::work_op`] whose `op` may say something besides success (`Some(note)`: the
    /// status line shows it, e.g. what a discard kept). Runs with a progress sink (the
    /// status bar shows what a long op reports, e.g. the meter of deleting 10 000
    /// untracked files) and can be cancelled like any operation.
    fn work_op_noted(
        &mut self,
        label: &str,
        paths: Option<Vec<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&Repo) -> anyhow::Result<Option<String>> + Send + 'static,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if self.busy.is_some() {
            self.refuse_busy(cx);
            return;
        }
        self.mark_busy(label, cx);
        let label = label.to_string();
        self.working_generation += 1;
        let prev = self.working.clone();
        let opts = self.settings.diff_options();
        let read_with = opts;
        let cache = self
            .settings
            .syntax_highlight
            .then(|| self.hl_cache.clone());
        let prev_subs = self.submodules.clone();
        let cancel = self.start_cancellable();
        let sink = self.progress_sink(cx);
        let task = cx.background_executor().spawn(async move {
            let done = gitti_core::process::with_cancel(&cancel, || {
                gitti_core::process::with_progress(sink, || op(&repo))
            });
            let stale = (done.as_ref().err())
                .and_then(|e| gitti_core::lock::stale_lock(e, repo.workdir()));
            let index = repo.index_stamp();
            let read_at = gitti_core::watch::read_start();
            let (working, scope) = match &paths {
                Some(p) => match repo.working_state_reread(&opts, &prev, p) {
                    Ok((w, scope)) => (Ok(w), scope),
                    Err(e) => (Err(e), gitti_core::ReadScope::Whole),
                },
                None => (repo.working_state_with(&opts), gitti_core::ReadScope::Whole),
            };
            // A folder stands for the files below it (a restored folder): those are
            // highlighted again.
            let touched = match (&working, &paths) {
                (Ok(w), Some(p)) => Some(gitti_core::touched_files(&prev, w, p)),
                _ => None,
            };
            let styles = match (&working, &touched, cache) {
                (Ok(w), Some(t), Some(cache)) => {
                    highlighting::highlight_paths(&repo, w, t, &cache)
                }
                _ => None,
            };
            // A staged / discarded pointer changes the submodule states (in-process; the
            // nested ones keep their dirty flags).
            let subs = working
                .as_ref()
                .ok()
                .map(|w| repo.submodules_after(&w.status, &prev_subs));
            let read = (scope, touched, read_at);
            (done, working.map(|w| (w, styles)), read, index, subs, stale)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (done, working, (scope, touched, read_at), index, subs, stale) = task.await;
            this.update_in(cx, |this, window, cx| {
                this.working_generation += 1;
                // Only the touched paths were re-read: an index write of anyone else
                // since needs a refresh, ours (this stamp) doesn't — nor our own writes
                // to those files (discard), the folders a discard left empty and deleted,
                // or anything where the re-read took the whole working state.
                this.watch_filter.own_index(index);
                if working.is_ok() {
                    this.watch_filter.reread(&scope, read_at);
                }
                this.pending = None;
                if let Some(subs) = subs {
                    this.submodules = subs;
                    this.sync_side_cursor();
                }
                match done {
                    Ok(note) => {
                        this.errors.succeeded(&label);
                        if let Some(note) = note {
                            this.set_info(note);
                        }
                    }
                    Err(_) if this.op_cancelled() => this.set_info(format!("{label} cancelled")),
                    Err(e) => this.op_failed(&label, format!("{e:#}")),
                }
                if let Some(lock) = stale {
                    this.offer_stale_lock(&label, lock, window, cx);
                }
                match working {
                    Ok((w, styles)) => this.apply_working(w, styles, touched, cx),
                    Err(e) => {
                        this.set_error(format!("{e:#}"));
                        this.refresh(false, window, cx);
                    }
                }
                // A display option changed while it ran (its own reload was dropped).
                if this.settings.diff_options() != read_with {
                    this.reload_working(window, cx);
                }
                this.end_busy(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows a working state read by `work_op`; `styles` are the highlights of `paths`.
    fn apply_working(
        &mut self,
        working: WorkingState,
        styles: Option<highlighting::Highlights>,
        paths: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let prev_sel_diff = self
            .line_sel
            .as_ref()
            .and_then(|sel| self.file_diff(sel.src, sel.file).cloned());
        let prev_working = std::mem::replace(&mut self.working, Arc::new(working));
        if self.working.conflicted.is_empty() {
            self.conflict_sides = None;
        }
        self.publish_marks(cx);
        self.rebuild_rows_if_worktree(prev_sel_diff, &prev_working);
        self.restore_worktree_view_anchor(cx);
        self.refresh_worktree_compare(cx);
        match (styles, paths) {
            (Some(styles), Some(paths)) if matches!(self.detail, Detail::Worktree) => {
                // Drop a running full highlight: it was computed from the old state.
                self.hl_generation += 1;
                self.highlights.retain(|(_, p), _| !paths.contains(p));
                self.highlights.extend(styles);
                self.worktree_hl = Some((self.working.clone(), self.highlights.clone()));
            }
            _ => {
                if matches!(self.detail, Detail::Worktree) {
                    self.schedule_highlight(cx);
                }
            }
        }
    }

    /// `run_op` for git commands whose output is just reported in the status bar.
    fn git_op(
        &mut self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&Repo) -> anyhow::Result<String> + Send + 'static,
    ) {
        let label_done = label.to_string();
        self.run_op(label, true, window, cx, op, move |this, out, _, _| {
            this.set_info(last_line(&out, &format!("{label_done} done")));
        });
    }

    // ------------------------------------------------------------------ staging

    /// The working-state file list behind a diff source (`None` for commit diffs).
    fn work_files(&self, src: Src) -> Option<&[gitti_core::WorkFile]> {
        match src {
            Src::Conflicted => Some(&self.working.conflicted),
            Src::Unstaged => Some(&self.working.unstaged),
            Src::Staged => Some(&self.working.staged),
            Src::Commit => None,
        }
    }

    fn file_diff(&self, src: Src, file: usize) -> Option<&FileDiff> {
        match src {
            Src::Commit => match &self.detail {
                Detail::Commit(cv) => cv.files.get(file),
                _ => None,
            },
            _ => self.work_files(src)?.get(file)?.diff.as_ref(),
        }
    }

    fn file_path(&self, src: Src, file: usize) -> Option<String> {
        match src {
            Src::Commit => self.file_diff(src, file).map(|f| f.path().to_string()),
            _ => self
                .work_files(src)?
                .get(file)
                .map(|f| f.path().to_string()),
        }
    }

    fn stage_file(&mut self, src: Src, file: usize, window: &mut Window, cx: &mut Context<Self>) {
        // A path this file system can't hold: asks first (`fs_paths.rs`).
        if src == Src::Unstaged
            && let Some(path) = self.file_path(src, file)
        {
            return self.guard_fs_stage(vec![path], window, cx, move |this, _, w, cx| {
                this.stage_file_now(src, file, w, cx)
            });
        }
        self.stage_file_now(src, file, window, cx)
    }

    fn stage_file_now(&mut self, src: Src, file: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.file_path(src, file) else {
            return;
        };
        match src {
            Src::Unstaged => self.set_pending(src, file, None, "Staging"),
            Src::Staged => self.set_pending(src, file, None, "Unstaging"),
            _ => {}
        }
        let paths = Some(vec![path.clone()]);
        match src {
            Src::Unstaged => {
                self.work_op("Staging", paths, window, cx, move |r| r.stage_paths(&[&path]))
            }
            Src::Staged => {
                self.work_op("Unstaging", paths, window, cx, move |r| r.unstage_paths(&[&path]))
            }
            Src::Conflicted | Src::Commit => {}
        }
    }

    /// Marks a file (or some of its hunks) as being worked on, unless another operation
    /// is still running (then `run_op` refuses anyway).
    fn set_pending(
        &mut self,
        src: Src,
        file: usize,
        hunks: Option<BTreeSet<usize>>,
        label: &'static str,
    ) {
        if self.busy.is_none() {
            self.pending = Some(Pending {
                src,
                file,
                hunks,
                label,
            });
        }
    }

    /// The running operation's label if it covers this file (`hunk: None`: the whole
    /// file) or hunk.
    pub(super) fn pending_label(
        &self,
        src: Src,
        file: usize,
        hunk: Option<usize>,
    ) -> Option<&'static str> {
        let p = self.pending.as_ref().filter(|_| self.pending_visible)?;
        let covers = match (&p.hunks, hunk) {
            (None, _) => true,
            (Some(hs), Some(h)) => hs.contains(&h),
            (Some(_), None) => false,
        };
        (p.src == src && p.file == file && covers).then_some(p.label)
    }

    /// Opens the merge editor window for a conflicted file.
    pub fn open_merge_editor(
        &mut self,
        file: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(repo), Some(path)) = (self.repo.clone(), self.file_path(Src::Conflicted, file))
        else {
            return;
        };
        let sides = self.conflict_sides.clone().unwrap_or(ConflictSides {
            ours: "HEAD".into(),
            theirs: "incoming".into(),
        });
        let scripted = std::env::var_os("GITTI_SCRIPT").is_some();
        let settings = self.settings.clone();
        let load_path = path.clone();
        let task = cx
            .background_executor()
            .spawn(async move { crate::merge_window::load(repo, &load_path, sides) });
        cx.spawn(async move |this, cx| {
            let session = task.await;
            this.update(cx, |this, cx| {
                let opened = session.and_then(|s| {
                    crate::merge_window::open(s, &path, settings, scripted, cx)
                });
                if let Err(e) = opened {
                    this.set_error(format!("Merge editor: {e:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Resolve a conflicted file: `Some(ours)` takes one side, `None` marks the current
    /// worktree content as resolved (asks first if conflict markers are left).
    pub(super) fn resolve_conflict(
        &mut self,
        file: usize,
        side: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.file_path(Src::Conflicted, file) else {
            return;
        };
        let paths = vec![path.clone()];
        if side.is_some() {
            self.set_pending(Src::Conflicted, file, None, "Resolving");
        }
        match side {
            Some(ours) => self.git_op(
                if ours { "Resolve (ours)" } else { "Resolve (theirs)" },
                window,
                cx,
                move |r| r.resolve_with_side(&paths, ours),
            ),
            None => {
                let go = move |this: &mut Self, w: &mut Window, cx: &mut Context<Self>| {
                    let paths = paths.clone();
                    this.git_op("Mark Resolved", w, cx, move |r| r.mark_resolved(&paths))
                };
                if self
                    .repo
                    .as_ref()
                    .is_some_and(|r| r.has_conflict_markers(&path))
                {
                    self.confirm(
                        format!("{path} still contains conflict markers."),
                        Some("Mark it as resolved anyway? The markers will be staged.".into()),
                        "Mark Resolved",
                        window,
                        cx,
                        go,
                    );
                } else {
                    go(self, window, cx);
                }
            }
        }
    }

    /// Abort for conflicts without an operation (a conflicting `git stash pop` / `apply`):
    /// asks first, listing what goes back to HEAD, then `git reset --merge`.
    pub(super) fn abort_conflicts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working.conflicted.is_empty() {
            return;
        }
        let (title, detail) = gitti_core::ops::abort_conflicts_prompt(
            self.working.conflicted.iter().map(|f| f.path()),
            self.working.staged.iter().map(|f| f.path()),
        );
        self.confirm(title, Some(detail), "Abort", window, cx, |this, w, cx| {
            this.git_op("Abort", w, cx, |r| r.abort_conflicts())
        });
    }

    fn apply_selection(
        &mut self,
        src: Src,
        file: usize,
        sel: Selection,
        mode: PatchMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_not_utf8(src, file, mode, cx) {
            return;
        }
        if self.refuse_lfs(src, file, mode, cx) || self.refuse_fs_path(src, file, mode, cx) {
            return;
        }
        let Some(shown) = self.file_diff(src, file) else {
            return;
        };
        // With "ignore whitespace" the shown diff (`-w`) cannot be applied: transfer the
        // selection onto the exact diff of the file.
        let exact = self
            .work_files(src)
            .and_then(|f| f.get(file))
            .and_then(|w| w.patch_diff.as_ref());
        let (diff, sel) = match exact {
            Some(real) => match gitti_core::patch::map_selection(shown, &sel, real) {
                Some(mapped) => (real.clone(), mapped),
                None => {
                    self.set_error(
                        "Could not match the selection to the exact diff — turn off \"Ignore Whitespace\" to stage these lines"
                            .into(),
                    );
                    cx.notify();
                    return;
                }
            },
            None => (shown.clone(), sel),
        };
        let label = match mode {
            PatchMode::Stage => "Staging",
            PatchMode::Unstage => "Unstaging",
            PatchMode::Discard => "Discarding",
        };
        self.set_pending(src, file, Some(sel.hunks.keys().copied().collect()), label);
        // A partial patch of a new / deleted file touches only this path, too.
        let mut paths = vec![diff.path().to_string()];
        paths.extend(diff.old_path.clone().filter(|p| *p != paths[0]));
        self.work_op(label, Some(paths), window, cx, move |r| {
            r.apply_selection(&diff, &sel, mode)
        });
    }

    /// A file whose name is not UTF-8 and whose bytes aren't known for sure (two names
    /// that show alike) has no hunk / line operations (the patch would name another
    /// file): says so and returns true, before anything runs.
    fn refuse_not_utf8(
        &mut self,
        src: Src,
        file: usize,
        mode: PatchMode,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(diff), Some(repo)) = (self.file_diff(src, file), self.repo.as_ref()) else {
            return false;
        };
        match repo.check_names(diff, mode) {
            Ok(()) => false,
            Err(e) => {
                self.set_error(e.to_string());
                cx.notify();
                true
            }
        }
    }

    /// Stage/unstage the current line selection.
    /// Whether selected lines of section `src` are what s / u act on.
    fn has_line_selection(&self, src: Src) -> bool {
        self.line_sel.as_ref().is_some_and(|s| s.src == src && !s.lines.is_empty())
    }

    fn apply_line_selection(&mut self, want: Src, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sel) = self.line_sel.clone() else {
            return;
        };
        if sel.src != want || sel.lines.is_empty() {
            return;
        }
        let mode = if sel.src == Src::Staged {
            PatchMode::Unstage
        } else {
            PatchMode::Stage
        };
        self.apply_selection(sel.src, sel.file, sel.to_selection(), mode, window, cx);
    }

    fn confirm(
        &mut self,
        message: String,
        detail: Option<String>,
        button: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            detail.as_deref(),
            &[button, "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| then(this, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn discard_file(&mut self, file: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wf) = self.working.unstaged.get(file).cloned() else {
            return;
        };
        // A folder that is a repository of its own: Discard would keep it (as `git
        // clean` does); deleting it is asked for in plain words.
        let nested = self
            .repo
            .as_ref()
            .is_some_and(|r| gitti_core::discard::is_nested_repo(&**r, &wf.entry));
        let (message, detail, button, opts) = if nested {
            (
                format!("Delete the repository folder {}?", wf.path()),
                format!(
                    "{} is a separate git repository (it has its own .git). Deleting it \
                     deletes its files, its history and everything not pushed from it. \
                     This cannot be undone.",
                    wf.path()
                ),
                "Delete Repository",
                DiscardOptions {
                    delete_nested_repos: true,
                },
            )
        } else {
            (
                format!("Discard all changes in {}?", wf.path()),
                // A path this file system can't hold: what Discard can't do (`fs_paths.rs`).
                match self.fs_discard_warning([wf.path()]) {
                    Some(w) => format!("{w}\n\nThis cannot be undone."),
                    None => "This cannot be undone.".to_string(),
                },
                "Discard",
                DiscardOptions::default(),
            )
        };
        self.confirm(message, Some(detail), button, window, cx, move |this, window, cx| {
            this.set_pending(Src::Unstaged, file, None, "Discarding");
            let paths = Some(vec![wf.path().to_string()]);
            this.work_op_noted("Discarding", paths, window, cx, move |r| {
                let report = r.discard_with(&[&wf.entry], opts)?;
                Ok(report.note())
            });
        });
    }

    /// Discards every unstaged change (tracked files restored from the index, untracked
    /// files deleted) after confirmation. Staged changes are kept.
    pub(super) fn discard_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.discard_unstaged(None, window, cx)
    }

    /// Stages the files of a group of the unstaged section (its header's Stage button):
    /// one `git add` and a re-read of those paths, as for a file.
    pub(super) fn stage_group(
        &mut self,
        group: crate::rows::Group,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.defer_while_busy(move |this, w, cx| this.stage_group(group, w, cx)) {
            return;
        }
        let paths = crate::rows::group_paths(&self.working, group);
        if paths.is_empty() {
            return;
        }
        self.guard_fs_stage(paths.clone(), window, cx, move |this, only, w, cx| {
            let paths = only.unwrap_or(paths);
            let list = paths.clone();
            this.work_op("Staging", Some(paths), w, cx, move |r| r.stage_path_list(&list));
        });
    }

    /// [`Self::discard_all`] for one group of the unstaged section (`None` = all of it).
    pub(super) fn discard_unstaged(
        &mut self,
        group: Option<crate::rows::Group>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries: Vec<_> = self
            .working
            .unstaged
            .iter()
            .filter(|w| group.is_none_or(|g| crate::rows::Group::of(w) == g))
            .map(|w| w.entry.clone())
            .filter(|e| !e.conflicted)
            .collect();
        if entries.is_empty() {
            self.set_info("Nothing to discard");
            cx.notify();
            return;
        }
        let n = entries.len();
        let (title, done) = match group {
            None => (
                format!("Discard all unstaged changes in {n} file(s)?"),
                "All unstaged changes discarded",
            ),
            Some(crate::rows::Group::Tracked) => (
                format!("Discard the changes of {n} tracked file(s)?"),
                "Changes of tracked files discarded",
            ),
            Some(crate::rows::Group::Untracked) => (
                format!("Delete {n} untracked file(s)?"),
                "Untracked files deleted",
            ),
        };
        self.discard_entries(entries, title, done, "Discard All", window, cx);
    }

    /// The confirmation of discarding the unstaged changes of `entries` (several files: all,
    /// a group, a folder, marked files): `title` asks, the detail says what happens to each
    /// kind of file; then one discard and a re-read of those paths. `done` starts the status
    /// bar's report, `button` confirms.
    pub(super) fn discard_entries(
        &mut self,
        entries: Vec<gitti_core::status::StatusEntry>,
        title: String,
        done: &'static str,
        button: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plan = match &self.repo {
            Some(r) => DiscardPlan::of(&**r, &entries),
            None => return,
        };
        let fs_warning = self.fs_discard_warning(entries.iter().map(|e| e.path.as_str()));
        let run = move |this: &mut Self, opts: DiscardOptions, window: &mut Window, cx: &mut Context<Self>| {
            let paths = Some(entries.iter().map(|e| e.path.clone()).collect());
            this.work_op_noted("Discarding", paths, window, cx, move |r| {
                let report = r.discard_with(&entries.iter().collect::<Vec<_>>(), opts)?;
                Ok(Some(gitti_core::discard::done_note(done, &report)))
            });
        };
        let detail = match fs_warning {
            Some(w) => format!("{}\n\n{w}", plan.detail()),
            None => plan.detail(),
        };
        if plan.nested.is_empty() {
            self.confirm(title, Some(detail), button, window, cx, move |this, w, cx| {
                run(this, DiscardOptions::default(), w, cx)
            });
            return;
        }
        // Nested repositories: kept unless the box is ticked.
        let spec = modal::ModalSpec::new(title)
            .message(detail)
            .check(plan.nested_check(), false)
            .ok(button)
            .danger();
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let opts = DiscardOptions {
                delete_nested_repos: res.checks.first().copied().unwrap_or(false),
            };
            run(this, opts, w, cx)
        });
    }

    fn toggle_amend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // What was typed before is the draft (or the amend draft), not HEAD's message
        // that may come now.
        self.save_draft_now(cx);
        self.amend = !self.amend;
        if self.amend
            && self.message.read(cx).value().trim().is_empty()
            && let Some(msg) = self.amend_start_message()
        {
            self.message.update(cx, |m, cx| m.set_value(msg, window, cx));
        }
        if !self.amend {
            // An empty box starts again with git's message or the commit template.
            self.sync_box_start(window, cx);
        }
        cx.notify();
    }

    /// Changes settings and stores only the changed fields, so a stale copy (another tab,
    /// another gitti process) never overwrites other settings.
    fn update_settings(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        f(&mut self.settings);
        crate::store::save_changed_settings(&before, &self.settings);
        crate::theme::apply_text_antialiasing(self.settings.text_antialiasing, cx);
        cx.notify();
    }

    // ------------------------------------------------------------------ actions

    fn on_refresh(&mut self, _: &Refresh, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_everything(window, cx);
    }

    /// The user's Refresh: the working state and the whole history read again (the full
    /// refresh's own history check then finds nothing moved).
    pub(super) fn refresh_everything(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_history(true, cx);
        self.refresh(true, window, cx);
    }

    fn on_fetch(&mut self, _: &Fetch, window: &mut Window, cx: &mut Context<Self>) {
        self.git_op("Fetch", window, cx, |r| r.fetch());
    }

    fn on_pull(&mut self, _: &Pull, window: &mut Window, cx: &mut Context<Self>) {
        self.pull_main(window, cx);
    }

    fn on_push(&mut self, _: &Push, window: &mut Window, cx: &mut Context<Self>) {
        self.push_main(window, cx);
    }

    fn on_commit(&mut self, _: &Commit, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(window, cx);
    }

    fn on_stash(&mut self, _: &StashChanges, window: &mut Window, cx: &mut Context<Self>) {
        self.stash_dialog(window, cx);
    }

    fn on_select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        self.select_row(self.selected + 1, window, cx);
    }

    fn on_select_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected > 0 {
            self.select_row(self.selected - 1, window, cx);
        }
    }

    fn on_stage_selection(
        &mut self,
        _: &StageSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stage_selection(window, cx);
    }

    fn on_unstage_selection(
        &mut self,
        _: &UnstageSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.unstage_selection(window, cx);
    }

    pub fn stage_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.defer_while_busy(|this, w, cx| this.stage_selection(w, cx)) {
            return;
        }
        if !self.has_line_selection(Src::Unstaged) && self.marks_action(PatchMode::Stage, window, cx) {
            return;
        }
        self.apply_line_selection(Src::Unstaged, window, cx);
    }

    pub fn unstage_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.defer_while_busy(|this, w, cx| this.unstage_selection(w, cx)) {
            return;
        }
        if !self.has_line_selection(Src::Staged) && self.marks_action(PatchMode::Unstage, window, cx) {
            return;
        }
        self.apply_line_selection(Src::Staged, window, cx);
    }

    /// Stage / unstage everything.
    pub(super) fn stage_everything(&mut self, stage: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.defer_while_busy(move |this, w, cx| this.stage_everything(stage, w, cx)) {
            return;
        }
        if stage {
            // Conflicted files with markers left: ask first, as Mark Resolved does.
            let conflicted: Vec<&str> = self.working.conflicted.iter().map(|f| f.path()).collect();
            let markers = match &self.repo {
                Some(r) if !conflicted.is_empty() => r.with_conflict_markers(&conflicted),
                _ => Vec::new(),
            };
            if !markers.is_empty() {
                let (title, detail, ok) = gitti_core::stage_all::markers_prompt(&markers);
                return self.confirm(title, Some(detail), ok, window, cx, |this, w, cx| {
                    this.stage_everything_now(w, cx)
                });
            }
            self.stage_everything_now(window, cx);
        } else {
            self.work_op("Unstaging", None, window, cx, |r| r.unstage_all());
        }
    }

    /// Stage All, asked already: what the list shows (`gitti_core::stage_all`).
    fn stage_everything_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let untracked = self.working.status.untracked_files;
        let paths: Vec<String> = self.working.unstaged.iter().map(|f| f.path().to_string()).collect();
        self.guard_fs_stage(paths, window, cx, move |this, only, w, cx| match only {
            None => this.work_op_noted("Staging", None, w, cx, move |r| r.stage_all_shown(untracked)),
            Some(list) => {
                let paths = list.clone();
                this.work_op("Staging", Some(paths), w, cx, move |r| r.stage_path_list(&list))
            }
        });
    }

    /// While an operation runs, queues a key action for when it is done (`true`).
    fn defer_while_busy(
        &mut self,
        action: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> bool {
        if self.busy.is_none() {
            return false;
        }
        // Key repeat must not pile up minutes of work.
        if self.deferred.len() < 8 {
            self.deferred.push_back(Box::new(action));
        }
        true
    }

    fn on_toggle_order(&mut self, _: &ToggleOrder, window: &mut Window, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.topo_order = !s.topo_order);
        self.set_info(format!("Commit order: {:?}", self.order()));
        self.reload_history(window, cx);
    }

    /// Reloads only the history (the order changed; nothing else did).
    fn reload_history(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.load_history(false, cx);
    }

    /// Re-reads only the working state (a diff display option changed).
    pub(super) fn reload_working(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        // Newer than any refresh or reload still running.
        self.working_generation += 1;
        let generation = self.working_generation;
        self.watch_filter.read_started(false);
        let opts = self.settings.diff_options();
        let task = cx
            .background_executor()
            .spawn(async move { repo.working_state_with(&opts) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.working_generation != generation {
                    return;
                }
                match result {
                    Ok(w) => this.apply_working(w, None, None, cx),
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn on_zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.zoom(1.0));
    }

    fn on_zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.zoom(-1.0));
    }

    fn on_zoom_diff_in(&mut self, _: &ZoomDiffIn, _: &mut Window, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.zoom_diff(1.0));
    }

    fn on_zoom_diff_out(&mut self, _: &ZoomDiffOut, _: &mut Window, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.zoom_diff(-1.0));
    }

    fn on_reset_zoom(&mut self, _: &ResetZoom, _: &mut Window, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.reset_zoom());
    }

    fn on_focus_message(&mut self, _: &FocusMessage, window: &mut Window, cx: &mut Context<Self>) {
        if !self.history.is_worktree_row(self.selected) {
            self.select_row(0, window, cx);
        }
        self.message.update(cx, |m, cx| m.focus(window, cx));
    }

    fn on_focus_list(&mut self, _: &FocusCommitList, window: &mut Window, cx: &mut Context<Self>) {
        // Esc in the diff: clears marked files first, then hides revealed unchanged lines
        // ("Show Changes Only"), then ends compare mode (the focus stays).
        if self.diff_focus.is_focused(window)
            && (self.clear_marks(cx) || self.esc_changes_only(cx) || self.end_compare(cx))
        {
            return;
        }
        self.list_focus.focus(window, cx);
    }

    fn on_modal_cancel(&mut self, _: &ModalCancel, window: &mut Window, cx: &mut Context<Self>) {
        self.close_top_overlay(window, cx);
    }

    fn on_open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(window, cx);
    }

    fn on_command_palette(
        &mut self,
        _: &CommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_command_palette(window, cx);
    }

    fn on_switch_branch(&mut self, _: &SwitchBranch, window: &mut Window, cx: &mut Context<Self>) {
        self.open_branch_switcher(window, cx);
    }

    fn on_picker_up(&mut self, _: &PickerUp, _: &mut Window, cx: &mut Context<Self>) {
        self.picker_nav(gitti_core::keys::Nav::LineUp, cx);
    }

    fn on_picker_down(&mut self, _: &PickerDown, _: &mut Window, cx: &mut Context<Self>) {
        self.picker_nav(gitti_core::keys::Nav::LineDown, cx);
    }

    fn on_toggle_git_log(&mut self, _: &ToggleGitLog, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_git_log(cx);
        self.leave_hidden_log(window, cx);
    }

    /// The git log panel was hidden while it had the focus: back to the commit list.
    pub(super) fn leave_hidden_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.show_git_log && self.log_focus.is_focused(window) {
            self.list_focus.focus(window, cx);
        }
    }

    fn on_toggle_wrap(&mut self, _: &ToggleWrap, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_wrap(cx);
    }

    fn on_toggle_whitespace(
        &mut self,
        _: &ToggleWhitespace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_whitespace(window, cx);
    }

    fn on_run_git_command(
        &mut self,
        _: &RunGitCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_command_input(window, cx);
    }

    pub(super) fn focus_command_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.show_git_log {
            self.toggle_git_log(cx);
        }
        self.cmd_input.update(cx, |s, cx| s.focus(window, cx));
    }

    fn on_cmd_history_prev(
        &mut self,
        _: &CmdHistoryPrev,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_command_history(-1, window, cx);
    }

    fn on_cmd_history_next(
        &mut self,
        _: &CmdHistoryNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_command_history(1, window, cx);
    }

    fn step_command_history(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        if self.cmd_history.is_empty() {
            return;
        }
        let len = self.cmd_history.len() as i32;
        let pos = match self.cmd_history_pos {
            None if delta < 0 => len - 1,
            None => return,
            Some(p) => p as i32 + delta,
        };
        let (pos, text) = if pos >= len {
            (None, String::new())
        } else {
            let p = pos.max(0) as usize;
            (Some(p), self.cmd_history[p].clone())
        };
        self.cmd_history_pos = pos;
        self.cmd_input
            .update(cx, |s, cx| s.set_value(text, window, cx));
    }

    /// Runs the command typed into the git log panel.
    fn run_typed_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = self.cmd_input.read(cx).value().trim().to_string();
        let args = gitti_core::ops::split_command_line(&line);
        if args.is_empty() {
            return;
        }
        if self.cmd_history.last() != Some(&line) {
            self.cmd_history.push(line.clone());
        }
        self.cmd_history_pos = None;
        self.cmd_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        let label = format!("git {}", args.first().cloned().unwrap_or_default());
        self.run_op(
            &label,
            true,
            window,
            cx,
            move |r| r.run_user_command(&args),
            |this, out, _, _| {
                let lines = out.lines().count();
                if lines > 1 {
                    this.status = Some(Status {
                        text: format!("{} lines of output (click for details)\n{out}", lines),
                        error: false,
                    });
                    this.status_gen += 1;
                } else {
                    this.set_info(if out.is_empty() {
                        "Done".to_string()
                    } else {
                        out
                    });
                }
            },
        );
    }

    /// Mirrors the command log to disk (or stops) according to the settings.
    pub(super) fn apply_log_persistence(&self) {
        if let Some(repo) = &self.repo {
            let path = self
                .settings
                .persist_git_log
                .then(|| crate::theme::log_file_for(repo.workdir()))
                .flatten();
            repo.command_log().set_file(path);
        }
    }

    pub(super) fn toggle_git_log(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.show_git_log = !s.show_git_log);
    }

    /// Shows the git log panel (if hidden) scrolled to the newest entries.
    pub(super) fn show_git_log_newest(&mut self, cx: &mut Context<Self>) {
        if !self.settings.show_git_log {
            self.toggle_git_log(cx);
        }
        let count = self.visible_log_entries().len();
        if count > 0 {
            self.log_scroll
                .scroll_to_item(count - 1, ScrollStrategy::Bottom);
        }
    }

    pub(super) fn toggle_syntax(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.syntax_highlight = !s.syntax_highlight);
        self.schedule_highlight(cx);
    }

    pub(super) fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.wrap_lines = !s.wrap_lines);
        self.rebuild_rows_keep_top(cx);
        let on = self.settings.wrap_lines;
        self.set_info(if on { "Line wrap on" } else { "Line wrap off" });
    }

    pub(super) fn toggle_whitespace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.ignore_whitespace = !s.ignore_whitespace);
        self.line_sel = None;
        let on = self.settings.ignore_whitespace;
        self.set_info(if on {
            "Ignoring whitespace changes (staging keeps whitespace-only changes unstaged)"
        } else {
            "Showing whitespace changes"
        });
        self.keep_view_anchor();
        self.reload_working(window, cx);
        self.reload_detail_keep_view(cx);
    }

    // ------------------------------------------------------------------ line selection

    fn on_line_mouse_down(&mut self, row: usize, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(DiffRow::Line {
            src,
            file,
            hunk,
            line,
            ..
        }) = self.rows.get(row).copied()
        else {
            return;
        };
        let is_change = self
            .file_diff(src, file)
            .and_then(|f| f.hunks.get(hunk)?.lines.get(line))
            .is_some_and(|l| l.is_change());
        // A commit's / stash's lines: for Revert / Apply … in Working Tree (not a
        // comparison's).
        let commit = src == Src::Commit && self.commit_origin().is_some();
        if !is_change || src == Src::Conflicted || (src == Src::Commit && !commit) {
            return;
        }
        let same_file = self
            .line_sel
            .as_ref()
            .is_some_and(|s| s.src == src && s.file == file);
        if ev.modifiers.shift && same_file {
            self.extend_selection(row);
        } else if ev.modifiers.secondary() && same_file {
            let s = self.line_sel.as_mut().expect("same file");
            if !s.lines.remove(&(hunk, line)) {
                s.lines.insert((hunk, line));
            }
            s.anchor_row = row;
        } else {
            let mut lines = BTreeSet::new();
            lines.insert((hunk, line));
            self.line_sel = Some(LineSel {
                src,
                file,
                anchor_row: row,
                lines,
            });
        }
        self.dragging = true;
        cx.notify();
    }

    fn extend_selection(&mut self, row: usize) {
        let Some(mut sel) = self.line_sel.take() else {
            return;
        };
        let rows = self.rows.clone();
        if let Some(f) = self.file_diff(sel.src, sel.file) {
            sel.select_range(&rows, row, |h, l| f.hunks[h].lines[l].is_change());
        }
        self.line_sel = Some(sel);
    }

    // ------------------------------------------------------------------ script hooks

    pub fn script_select_lines(&mut self, a: usize, b: usize, cx: &mut Context<Self>) {
        let down = MouseDownEvent {
            button: MouseButton::Left,
            position: Point::default(),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        };
        self.on_line_mouse_down(a, &down, cx);
        self.dragging = false;
        self.extend_selection(b);
        cx.notify();
    }

    /// Row of the commit whose id starts with `prefix`.
    pub fn script_row_of(&self, prefix: &str) -> Option<usize> {
        self.history
            .commits
            .iter()
            .position(|c| !c.id.is_null() && c.id.to_string().starts_with(prefix))
    }

    pub fn script_discard_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.discard_all(window, cx);
    }

    pub fn script_zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.zoom(delta));
    }

    pub fn script_fonts(&mut self, ui: Option<f32>, diff: Option<f32>, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| {
            if let Some(ui) = ui {
                s.ui_font_size = ui;
            }
            if let Some(diff) = diff {
                s.diff_font_size = diff;
            }
        });
    }

    pub fn script_focus_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.message.update(cx, |m, cx| m.focus(window, cx));
    }

    /// Sizes the layout tests compare against: message line pitch, diff row height and the
    /// commit box shrunk to one line.
    pub fn script_metrics(&self) -> crate::testing::Metrics {
        crate::testing::Metrics {
            message_line_h: f32::from(self.settings.message_line_h()),
            diff_row_h: f32::from(self.settings.diff_row_h()),
            commit_row_h: f32::from(self.commit_row_h()),
            commit_box_min_h: f32::from(self.commit_box_min_h()),
            selected_row: self.selected,
            diff_scroll_y: -f32::from(self.diff_scroll.0.borrow().base_handle.offset().y),
            selected_lines: self.line_sel.as_ref().map_or(0, |s| s.lines.len()),
            text_selected: self.selected_text().map_or(0, |t| t.len()),
            diff_rows: self.rows.iter().map(|r| r.kind_name()).collect(),
        }
    }

    /// Stage (`stage`) or unstage the file at `path`, or only its hunk `hunk` — what the
    /// file / hunk header buttons do.
    pub fn script_stage(
        &mut self,
        stage: bool,
        path: &str,
        hunk: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let src = if stage { Src::Unstaged } else { Src::Staged };
        let Some(file) = self
            .work_files(src)
            .and_then(|fs| fs.iter().position(|f| f.path() == path))
        else {
            self.set_error(format!("script: no {src:?} file {path}"));
            return;
        };
        match hunk {
            Some(h) => {
                let mode = if stage { PatchMode::Stage } else { PatchMode::Unstage };
                self.apply_selection(src, file, Selection::whole_hunk(h), mode, window, cx)
            }
            None => self.stage_file(src, file, window, cx),
        }
    }

    /// One line per working-state file: `U|S|C path hunks`.
    pub fn script_working(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (tag, files) in [
            ("C", &self.working.conflicted),
            ("U", &self.working.unstaged),
            ("S", &self.working.staged),
        ] {
            for f in files {
                let hunks = f.diff.as_ref().map_or(0, |d| d.hunks.len());
                out.push(format!("{tag} {} {hunks}", f.path()));
            }
        }
        out
    }

    /// The summary of the commit list's row `row` (as the list shows it, decoded).
    pub fn script_commit_summary(&self, row: usize) -> Option<String> {
        (row < self.history.commits.len() && !self.history.is_worktree_row(row))
            .then(|| self.history.meta(row).summary.clone())
    }

    /// The `i18n.commitEncoding` note under the commit box (`render_encoding_note`).
    pub fn script_encoding_note(&self, cx: &App) -> Option<String> {
        self.encoding_note(cx)
    }

    /// Rows of the shown history and how it came about (`HistoryLoad`).
    pub fn script_history(&self) -> (usize, String) {
        (self.history.commits.len(), format!("{:?}", self.history.load))
    }

    /// The file watcher's watches are in place (tests).
    pub fn script_watcher_armed(&self) -> bool {
        self._watcher.as_ref().is_some_and(|w| w.is_armed())
    }

    /// The file watcher has reported `path` (relative to the work tree) — at any time
    /// while the probes were on (tests).
    pub fn script_watch_seen(&self, path: &str) -> bool {
        self.watch_seen.iter().any(|p| p.ends_with(path))
    }

    pub fn script_idle(&self) -> bool {
        self.busy.is_none()
            && !self.refresh_in_flight
            && self.refresh_queued.is_none()
            && !self.history_in_flight
            && self.history_queued.is_none()
            && !self.paths_in_flight
            && self.paths_queued.is_none()
            && !self.change_search.busy()
            && !self.worktree_marks_loading
            // The watcher's first report (and the re-read it may ask for) is handled.
            && (self._watcher.is_none() || self.watch_ready)
    }

    pub fn script_workdir(&self) -> PathBuf {
        self.repo
            .as_ref()
            .map(|r| r.workdir().to_path_buf())
            .unwrap_or_default()
    }

    /// The status bar text, if it shows an error.
    pub fn script_error(&self) -> Option<String> {
        self.status.as_ref().filter(|s| s.error).map(|s| s.text.clone())
    }

    pub fn script_git_commands(&self) -> Vec<String> {
        self.repo.as_ref().map_or_else(Vec::new, |r| {
            r.command_log()
                .snapshot()
                .into_iter()
                // git processes only (not work done without git, `in_process`).
                .filter(|e| !e.previous_session && !e.in_process)
                .map(|e| e.args.join(" "))
                .collect()
        })
    }

    pub fn script_run_command(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_command_input(window, cx);
        self.cmd_input
            .update(cx, |s, cx| s.set_value(line.to_string(), window, cx));
        self.run_typed_command(window, cx);
    }

    pub fn script_config_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_config_dialog(gitti_core::ops::ConfigScope::Local, window, cx);
    }

    pub fn script_focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list_focus.focus(window, cx);
    }

    pub fn script_fetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.on_fetch(&Fetch, window, cx);
    }

    pub fn script_set_message(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.replace("\\n", "\n");
        self.message
            .update(cx, |m, cx| m.set_value(text, window, cx));
    }

    /// Script hook: the detail panel's selected text (commit message / diff) as copying
    /// takes it; `None` without a text selection.
    pub fn script_selected_text(&self) -> Option<String> {
        self.text_sel.filter(|s| !s.is_empty())?;
        self.selected_text()
    }

    /// Script hook: where byte `byte` of the commit header's line `line` (`header`: author,
    /// date, committer), of the commit message's line `line` (`message`) or of diff row
    /// `line` (`diff`) was laid out in the last frame.
    pub fn script_text_point(&self, area: &str, line: usize, byte: usize) -> Option<Point<Pixels>> {
        use crate::text_sel::Area;
        let area = match area {
            "header" => Area::Header,
            "message" => Area::Message,
            "diff" => Area::Diff,
            _ => return None,
        };
        self.drag_targets.point(area, line, byte)
    }

    /// Script hook: the commit message editor, a field or the text area of the open
    /// dialog: where its byte `byte` was laid out (inside the character) and its selected
    /// text.
    pub fn script_input(
        &self,
        input: crate::testing::InputBox,
        byte: usize,
        cx: &App,
    ) -> Option<(Option<Point<Pixels>>, String)> {
        use crate::testing::InputBox;
        let area = |a: &Entity<TextareaState>| {
            let a = a.read(cx);
            (a.range_to_bounds(&(byte..byte + 1)), a.selected_value())
        };
        let (bounds, text) = match input {
            InputBox::Message => area(&self.message),
            InputBox::DialogText => area(self.modal.as_ref()?.text_area()?),
            InputBox::DialogField(ix) => {
                let f = self.modal.as_ref()?.field_input(ix)?.read(cx);
                (f.range_to_bounds(&(byte..byte + 1)), f.selected_value())
            }
        };
        Some((bounds.map(|b| b.center()), text.to_string()))
    }

    /// The commit message box's text (tests).
    pub fn script_commit_message(&self, cx: &App) -> String {
        self.message.read(cx).value().to_string()
    }

    pub fn script_toggle(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(DiffRow::File { src, file }) = self.rows.get(row).copied()
            && let Some(path) = self.file_diff(src, file).map(|f| f.path().to_string())
        {
            self.expansion.toggle(src, &path);
            self.rebuild_rows();
            cx.notify();
        }
    }

    pub fn script_dump(&self) {
        if let Some((id, info)) = &self.signature {
            eprintln!("[dump] signature {id}: {info:?}");
        }
        eprintln!(
            "[dump] secure input: {}",
            crate::secure_input::system_enabled()
        );
        eprintln!(
            "[dump] diff scroll offset {:?}, header rows {}, commit box {:?} (collapsed by {:?}), message focused {}",
            self.diff_scroll.0.borrow().base_handle.offset(),
            self.header_rows,
            self.commit_box_h,
            self.commit_box_collapse.get(),
            self.message_focused.get()
        );
        for (i, r) in self.rows.iter().enumerate() {
            let text = match *r {
                DiffRow::Line {
                    src,
                    file,
                    hunk,
                    line,
                    ..
                } => self
                    .file_diff(src, file)
                    .and_then(|f| f.hunks.get(hunk)?.lines.get(line))
                    .map(|l| format!("{:?} {}", l.kind, l.text))
                    .unwrap_or_default(),
                DiffRow::File { src, file } => self.file_path(src, file).unwrap_or_default(),
                _ => String::new(),
            };
            eprintln!("{i:4} {r:?} {text}");
        }
        if let Some(s) = &self.line_sel {
            eprintln!("selection: {s:?}");
        }
        eprintln!("selected row: {}", self.selected);
        if let Some(c) = self.history.commits.get(self.selected) {
            eprintln!("selected commit: {}", c.id);
        }
        if let Some(s) = &self.status {
            eprintln!("status: {}", s.text);
        }
        if let Some(m) = &self.modal {
            eprintln!("modal: {}", m.title);
        }
    }

    // ------------------------------------------------------------------ chrome

    /// Banner while a merge / rebase / cherry-pick is in progress.
    fn render_state_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = cx.palette().clone();
        let conflicts = self.working.conflicted.len();
        if self.repo_state == RepoState::Clean {
            // e.g. `git stash apply` with conflicts: no operation to continue or abort.
            return (conflicts > 0).then(|| {
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .bg(p.warning.opacity(0.18))
                    .border_b_1()
                    .border_color(p.border)
                    .text_color(p.warning)
                    // The text wraps (min_w_0): at big fonts it pushed the buttons out.
                    .child(
                        div().flex_1().min_w_0().child(format!(
                            "{conflicts} conflicted file(s) — take ours / theirs or edit them, then mark them resolved (see Uncommitted changes)."
                        )),
                    )
                    .child(
                        Button::new("show-conflicts")
                            .label("Show")
                            .small()
                            .on_click(cx.listener(|this, _, w, cx| this.select_row(0, w, cx))),
                    )
                    .child(
                        Button::new("op-abort")
                            .probe("op-abort")
                            .label("Abort")
                            .small()
                            .danger()
                            .tooltip("Undo the conflicted changes (git reset --merge); a stash stays")
                            .on_click(cx.listener(|this, _, w, cx| this.abort_conflicts(w, cx))),
                    )
                    .into_any_element()
            });
        }
        let text = if conflicts > 0 {
            format!(
                "{} in progress — {conflicts} conflicted file(s). Resolve, stage, then continue.",
                self.repo_state.label()
            )
        } else {
            format!(
                "{} in progress — all conflicts staged.",
                self.repo_state.label()
            )
        };
        Some(
            h_flex()
                .px_3()
                .py_1()
                .gap_2()
                .items_center()
                .bg(p.warning.opacity(0.18))
                .border_b_1()
                .border_color(p.border)
                .text_color(p.warning)
                .child(div().flex_1().min_w_0().child(text))
                .child(
                    Button::new("op-continue")
                        .probe("op-continue")
                        .label("Continue")
                        .small()
                        .disabled(conflicts > 0)
                        .tooltip(match self.continue_message(cx) {
                            Some(_) => format!("Continue {}", gitti_core::ops::CONTINUE_WITH_BOX),
                            None => "Continue with git's message".to_string(),
                        })
                        .on_click(cx.listener(|this, _, w, cx| this.continue_operation(w, cx))),
                )
                .when(gitti_core::skip::can_skip(self.repo_state), |d| {
                    d.child(
                        Button::new("op-skip")
                            .probe("op-skip")
                            .label("Skip")
                            .small()
                            .tooltip(gitti_core::skip::skip_explain(self.repo_state))
                            .on_click(cx.listener(|this, _, w, cx| this.skip_operation(w, cx))),
                    )
                })
                .child(
                    Button::new("op-abort")
                        .probe("op-abort")
                        .label("Abort")
                        .small()
                        .danger()
                        .on_click(cx.listener(|this, _, w, cx| {
                            this.git_op("Abort", w, cx, |r| r.abort_operation())
                        })),
                )
                .into_any_element(),
        )
    }

    fn render_status_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        // While an operation runs the status bar shows only it (`op_progress.rs`).
        if let Some(bar) = self.render_busy_status(window, cx) {
            return bar;
        }
        let p = cx.palette().clone();
        let (text, color) = match &self.status {
            Some(s) if s.error => (s.text.lines().next().unwrap_or("").to_string(), p.danger),
            Some(s) => (
                s.text.lines().next().unwrap_or("").to_string(),
                p.text_muted,
            ),
            None => (
                format!(
                    "↑↓ navigate · {m}↑↓ / alt-↑↓ follow branch · +shift other way · click/shift/{m}click/drag lines · s stage · u unstage · right-click for actions",
                    m = crate::theme::MOD_KEY
                ),
                p.text_faint,
            ),
        };
        let has_more = self
            .status
            .as_ref()
            .is_some_and(|s| s.text.lines().count() > 1);
        h_flex()
            .id("status")
            .probe("status-bar")
            .h(self.settings.row_h())
            .px_3()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(p.border)
            .bg(p.bg_toolbar)
            .text_size(self.settings.ui_small())
            .text_color(color)
            .cursor_pointer()
            .child(div().flex_1().truncate().child(text))
            .when(has_more, |d| {
                d.child(div().text_color(p.text_muted).child("details…"))
            })
            .map(|d| {
                if has_more && self.status.as_ref().is_some_and(|s| s.error) {
                    d.tooltip(tips::ERROR_DETAILS.tooltip())
                } else if !has_more && !self.settings.show_git_log {
                    d.tooltip(tips::OPEN_GIT_LOG.tooltip())
                } else {
                    d
                }
            })
            // Multi-line output / errors: the whole text; otherwise the git command log
            // (shown at its newest entries — the status line came from one of them).
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(s) = this.status.clone()
                    && s.text.lines().count() > 1
                {
                    this.show_text_modal(
                        if s.error { "Error" } else { "Output" },
                        s.text,
                        window,
                        cx,
                    );
                } else {
                    this.status = None;
                    this.show_git_log_newest(cx);
                }
                cx.notify();
            }))
            .into_any_element()
    }
}

impl Render for GittiApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_status_fade(cx);
        let p = cx.palette().clone();
        let root = v_flex()
            .size_full()
            .relative()
            .bg(p.bg_list)
            .text_color(p.text)
            .text_size(self.settings.ui())
            .key_context("Gitti");
        // First, so an unavailable command says why instead of running.
        let root = self.unavailable_guards(root, cx)
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_fetch))
            .on_action(cx.listener(Self::on_pull))
            .on_action(cx.listener(Self::on_push))
            .on_action(cx.listener(Self::on_commit))
            .on_action(cx.listener(Self::on_stash))
            .on_action(cx.listener(Self::on_stage_selection))
            .on_action(cx.listener(Self::on_copy_selection))
            .on_action(cx.listener(Self::on_open_in_editor))
            .on_action(cx.listener(Self::on_unstage_selection))
            .on_action(cx.listener(Self::on_toggle_order))
            // The commit graph keys act from every pane (`GRAPH_KEY_CONTEXT`).
            .on_action(cx.listener(Self::on_follow_down))
            .on_action(cx.listener(Self::on_follow_up))
            .on_action(cx.listener(Self::on_other_down))
            .on_action(cx.listener(Self::on_other_up))
            .on_action(cx.listener(Self::on_focus_message))
            .on_action(cx.listener(Self::on_focus_list))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_zoom_diff_in))
            .on_action(cx.listener(Self::on_zoom_diff_out))
            .on_action(cx.listener(Self::on_reset_zoom))
            .on_action(cx.listener(Self::on_modal_cancel))
            .on_action(cx.listener(Self::on_command_palette))
            .on_action(cx.listener(Self::on_switch_branch))
            .on_action(cx.listener(Self::on_picker_up))
            .on_action(cx.listener(Self::on_picker_down))
            .on_action(cx.listener(Self::on_toggle_git_log))
            .on_action(cx.listener(Self::on_toggle_wrap))
            .on_action(cx.listener(Self::on_toggle_whitespace))
            .on_action(cx.listener(Self::on_run_git_command))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_cmd_history_prev))
            .on_action(cx.listener(Self::on_cmd_history_next));
        let root = self.key_actions(root, cx);
        let root = self.nav_actions(root, cx);
        let root = self.repo_actions(root, cx);
        let root = self.fs_paths_actions(root, cx);
        let root = self.reflog_actions(root, cx);
        let root = self.navigate_actions(root, cx);
        let root = self.menu_cmd_actions(root, cx);
        let root = self.ext_tool_actions(root, cx);
        let root = self.restore_contains_actions(root, cx);
        let root = self.commit_lines_actions(root, cx);
        let root = self.stash_ui_actions(root, cx);
        let root = self.file_marks_actions(root, cx);
        let root = self.pick_several_actions(root, cx);
        let root = self.list_layout_actions(root, cx);
        let root = self.branch_list_actions(root, cx);
        let root = self.worktree_actions(root, cx);
        let root = self.submodule_actions(root, cx);
        let root = self.subtree_actions(root, cx);
        let root = self.custom_command_actions(root, cx);
        let root = self.spell_actions(root, cx);
        let root = self.toolbar_actions(root, cx);
        let root = self.commit_box_actions(root, cx);
        if self.repo.is_none() {
            return self.render_no_repo(root, window, cx);
        }

        let banner = self.render_state_banner(cx);
        let sub_banner = self.render_submodule_banner(cx);
        let lfs_banner = self.render_lfs_banner(cx);
        let fs_banner = self.render_fs_paths_banner(cx);
        let modal = self.render_modal(window, cx);
        let picker = self.render_picker(cx);
        let settings = self.render_settings(cx);
        let body = self.render_panes(window, cx);
        root.child(self.render_toolbar(window, cx))
            .children(banner)
            .children(sub_banner)
            .children(lfs_banner)
            .children(fs_banner)
            .child(body)
            .child(self.render_status_bar(window, cx))
            .children(settings)
            .children(modal)
            .children(picker)
            .into_any_element()
    }
}

impl GittiApp {
    /// Test hook, see `testing::restart_panes`.
    pub fn script_restart_panes(
        &mut self,
        body_width: f32,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) {
        self.panes = PaneStates::new(cx);
        let (mut body, mut viewport) = self.body_size.get();
        body.width = px(body_width);
        viewport.width = px(viewport_width);
        self.body_size.set((body, viewport));
        cx.notify();
    }

    /// Sidebar | commit list | detail, and the git log below. Sizes start from the stored
    /// fractions of the measured body; dragging stores them again (all tabs share the
    /// resize state, so the arrangement is the same in every repository).
    fn render_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let size = self.body_size.clone();
        let app = cx.entity().downgrade();
        let panes = self.panes.clone();
        let sidebar_shown = self.settings.show_sidebar;
        let measure = canvas(
            move |bounds, window, cx| {
                let new = (bounds.size, window.viewport_size());
                let old = size.replace(new);
                if old.0.width <= px(0.) && new.0.width > px(0.) {
                    // First measurement: lay out the panes with their stored sizes.
                    let app = app.clone();
                    cx.defer(move |cx| {
                        app.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
            },
            // Before the handles see it: does a press start a drag of the sidebar's edge?
            move |bounds, _, window, _| {
                if !sidebar_shown {
                    return;
                }
                let panes = panes.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && e.button == MouseButton::Left {
                        panes.note_press(f32::from(e.position.x - bounds.left()), cx);
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        let container = div().flex_1().min_h_0().relative().child(measure);
        let (body, viewport) = self.body_size.get();
        if body.width <= px(0.) {
            // One frame without panes, so they never start from a guessed size.
            return container.into_any_element();
        }
        // The body was measured in an earlier frame. If the window changed size since (a
        // restored maximized window on Windows draws once at its normal size, then
        // maximizes), the toolbar and bars around the body stayed the same, so the body
        // changed by as much as the window. Panes started from the stale size would keep
        // it: the resize state only rescales on later changes of its own size.
        let now = window.viewport_size();
        let w = f32::from(body.width + now.width - viewport.width);
        let h = f32::from(body.height + now.height - viewport.height);
        let s = &self.settings;
        let side_min = self.sidebar_min_width();
        let mut sidebar = PaneLayout::size(s.pane_sidebar, w, 230.).max(side_min);
        let mut list = PaneLayout::size(s.pane_list, w, 640.);
        if self.settings.show_sidebar {
            if let Some(sizes) = rescale_panes(&self.panes.main, w, cx) {
                (sidebar, list) = (sizes[0], sizes[1]);
                self.panes.keep_list.set(None);
            } else if let Some(sizes) = self.panes.keep_list_width(w, cx) {
                // The sidebar's edge is dragged: the list keeps its width.
                (sidebar, list) = sizes;
            }
        } else if let Some(sizes) = rescale_panes(&self.panes.main_no_sidebar, w, cx) {
            list = sizes[0];
        }
        let app = cx.entity().downgrade();
        let commit_list = resizable_panel().size(px(list)).child(
            div()
                .size_full()
                .probe("commit-list")
                .child(self.render_list_pane(cx)),
        );
        let detail = resizable_panel().child(
            div()
                .size_full()
                .probe("detail")
                .child(self.render_detail(window, cx)),
        );
        // View ▸ Toggle Sidebar: the list keeps its stored share of the whole row.
        let main = if !self.settings.show_sidebar {
            h_resizable("main-no-sidebar")
                .with_state(&self.panes.main_no_sidebar)
                .on_resize(move |state, _, cx| {
                    let sizes: Vec<f32> =
                        state.read(cx).sizes().iter().map(|&p| f32::from(p)).collect();
                    let total: f32 = sizes.iter().sum();
                    if sizes.len() < 2 {
                        return;
                    }
                    app.update(cx, |this, cx| {
                        this.update_settings(cx, |s| {
                            s.pane_list = PaneLayout::fraction(sizes[0], total);
                        })
                    })
                    .ok();
                })
                .child(commit_list)
                .child(detail)
        } else {
            let compact = self.sidebar_compact(sidebar, cx);
            let panes = self.panes.clone();
            h_resizable("main")
                .with_state(&self.panes.main)
            .on_resize(move |state, _, cx| {
                let sizes: Vec<f32> = state.read(cx).sizes().iter().map(|&p| f32::from(p)).collect();
                let total: f32 = sizes.iter().sum();
                let Some((side, list)) = panes.stored_main_sizes(&sizes, total) else {
                    return;
                };
                app.update(cx, |this, cx| {
                    this.update_settings(cx, |s| {
                        s.pane_sidebar = PaneLayout::fraction(side, total);
                        s.pane_list = PaneLayout::fraction(list, total);
                    });
                    // A release before the frame that corrects the last move.
                    cx.notify();
                })
                .ok();
            })
            .child(
                resizable_panel()
                    .size(px(sidebar))
                    .size_range(px(side_min)..px(700.))
                    .child(div().size_full().probe("sidebar").child(self.render_sidebar(window, compact, cx))),
            )
            .child(commit_list)
            .child(detail)
        };
        let body = if self.settings.show_git_log {
            let log = rescale_panes(&self.panes.vertical, h, cx)
                .map_or_else(|| PaneLayout::size(self.settings.pane_log, h, 220.), |s| s[1]);
            let app = cx.entity().downgrade();
            v_resizable("main-vertical")
                .with_state(&self.panes.vertical)
                .on_resize(move |state, _, cx| {
                    let sizes: Vec<f32> =
                        state.read(cx).sizes().iter().map(|&p| f32::from(p)).collect();
                    let total: f32 = sizes.iter().sum();
                    if sizes.len() < 2 {
                        return;
                    }
                    app.update(cx, |this, cx| {
                        this.update_settings(cx, |s| {
                            s.pane_log = PaneLayout::fraction(sizes[1], total);
                        })
                    })
                    .ok();
                })
                .child(resizable_panel().child(main))
                .child(
                    resizable_panel()
                        .size(px(log))
                        .size_range(px(80.)..px(1600.))
                        .child(self.render_git_log(window, cx)),
                )
                .into_any_element()
        } else {
            main.into_any_element()
        };
        container.child(body).into_any_element()
    }
}

/// The pane sizes of a resize group for this frame when its container changed size (the
/// window was resized): the sizes it had, scaled to the new `total` along its axis — as
/// gpui-base's `ResizableState` would rescale them, but only in prepaint, after this
/// frame's layout, so the first frame split the extra space evenly between the panes.
/// The state is cleared, so the panels lay out with these sizes as their initial ones
/// (`flex_none`, the last pane fills) and measure themselves again in prepaint.
fn rescale_panes(state: &Entity<ResizableState>, total: f32, cx: &mut App) -> Option<Vec<f32>> {
    let (container, sizes) = {
        let s = state.read(cx);
        let sizes: Vec<f32> = s.sizes().iter().map(|&p| f32::from(p)).collect();
        (f32::from(s.container_size()), sizes)
    };
    let sum: f32 = sizes.iter().sum();
    if container <= 0. || (container - total).abs() < 0.5 || sizes.len() < 2 || sum <= 0. {
        return None;
    }
    state.update(cx, |s, _| s.clear());
    Some(sizes.iter().map(|size| size / sum * total).collect())
}

fn same_refs(a: &[RefInfo], b: &[RefInfo]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.full_name == y.full_name && x.target == y.target)
}

/// Tests: history loads wait while this is set (`testing::hold_history`).
pub static HOLD_HISTORY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Most rows the commit message editor grows to (fewer in a low panel).
const EDITOR_MAX_ROWS: usize = 12;

pub fn short(id: &gitti_core::ObjectId) -> String {
    id.to_hex_with_len(7).to_string()
}

fn last_line(s: &str, fallback: &str) -> String {
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn format_time(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(secs, 0).single() else {
        return String::new();
    };
    let age = Local::now().signed_duration_since(t);
    if age.num_seconds() < 0 {
        t.format("%Y-%m-%d").to_string()
    } else if age.num_minutes() < 1 {
        "just now".to_string()
    } else if age.num_minutes() < 60 {
        format!("{} min ago", age.num_minutes())
    } else if age.num_hours() < 24 {
        format!("{} h ago", age.num_hours())
    } else if age.num_days() < 7 {
        t.format("%a %H:%M").to_string()
    } else {
        t.format("%Y-%m-%d").to_string()
    }
}

fn format_time_full(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(secs, 0)
        .single()
        .map(|t| t.format("%a, %d %b %Y %H:%M:%S %z").to_string())
        .unwrap_or_default()
}
