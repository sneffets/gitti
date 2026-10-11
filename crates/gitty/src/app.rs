//! Application state, background jobs and git actions. Rendering lives in `ui.rs`, key
//! and mouse handling in `input.rs`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use anyhow::Result;
use gitti_core::commit_cache::CommitData;
use gitti_core::diff::FileDiff;
use gitti_core::keys::Command;
use gitti_core::ops::{ConflictSides, RemoteInfo, RepoState, ResetMode, StashInfo, Tracking};
use gitti_core::ops::{UnrelatedHistories, UnrelatedRetry};
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::rows::{DiffRow, Expansion, LineSel, Src};
use gitti_core::signature::SignatureInfo;
use gitti_core::store::{self, PaneLayout, SharedSettings};
use gitti_core::syntax::LineCategories;
use gitti_core::discard::{DiscardOptions, DiscardPlan};
use gitti_core::switch::{SwitchBlock, SwitchOutcome, SwitchTarget, SwitchWay};
use gitti_core::pull::{DivergentPull, PullStrategy};
use gitti_core::push::{PullBlock, PushBlock, pull_block, push_block};
use gitti_core::watch::Change;
use gitti_core::{
    DiffOptions, HeadInfo, History, LogOrder, ObjectId, RefInfo, RefKind, Repo, WorkFile,
    WorkingState,
};
use ratatui::crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::edit::TextEdit;
use crate::modal::{MenuItem, Modal, PickItem};
use crate::sidebar::{self, SideItem};
use crate::theme::Palette;

mod branch_cmds;
mod change_search;
mod commit_box;
mod commit_flow;
mod compare;
mod context_lines;
mod custom_commands;
mod ext_tools;
mod file_list;
mod file_marks;
mod from_commit;
mod fs_paths;
mod keybar;
mod lfs;
mod lfs_fetch;
mod lfs_health;
mod op_cancel;
mod op_output;
mod overwrite;
pub mod reflog;
mod remote_cmds;
mod repo_cmds;
mod search;
mod sidebar_menu;
mod signers;
mod signing_keys;
mod stash_cmds;
mod submodules;
mod subtrees;
mod terminal;
mod unavailable;
mod view_anchor;
mod watching;
mod worktrees;
pub use repo_cmds::reopen;

pub enum Msg {
    Term(Event),
    QuickHistory(Result<History>),
    /// A refresh's result and the working-state generation it started at.
    Refreshed(Box<Result<Snapshot>>, u64),
    /// "Load anyway" of a file left out as too large: the commit (or stash) with
    /// it, or the comparison with it.
    FileLoaded(usize, Result<(Arc<CommitData>, Option<Arc<gitti_core::compare::CompareData>>)>),
    /// The history after a ref change or an order toggle (`None`: the shown one still is
    /// it).
    HistoryLoaded(Result<Option<History>>),
    Detail(u64, Result<CommitView>),
    /// The commit load of this generation has run [`SLOW_LOAD`].
    DetailSlow(u64),
    /// Syntax highlighting of the shown diff (generation, per file).
    Highlights(u64, HashMap<(Src, String), FileHighlight>),
    /// Signature verification of a commit (`None`: verification failed to run), and the
    /// allowed signers files to offer when none is set (`app/signers.rs`).
    Signature(ObjectId, Box<(Option<SignatureInfo>, Option<signers::SignersOffer>)>),
    /// "Use … for This Repository / All Repositories": the `git config` ran.
    SignersSet(Result<String, String>),
    /// "Open in Terminal" finished: the terminal's name, or why none opened.
    TerminalOpened(Result<String, String>, std::path::PathBuf),
    /// "Get Signing Keys from GitHub…": a lookup or an import finished.
    SigningKeys(Box<signing_keys::KeysMsg>),
    OpDone {
        label: String,
        result: Result<String>,
        full: bool,
        after: After,
        /// The `*.lock` file it failed on, when no git holds it any more.
        stale: Option<gitti_core::lock::StaleLock>,
    },
    /// A stage / unstage / discard finished: its result, the working state re-read for
    /// `paths` (`None`: all of it) and those files' highlights.
    WorkDone {
        label: String,
        /// `Some(note)`: what the status line says besides success.
        result: Result<Option<String>>,
        working: Result<WorkingState>,
        highlights: Option<HashMap<(Src, String), FileHighlight>>,
        paths: Option<Vec<String>>,
        /// The index as the operation left it (see `WatchFilter::own_index`).
        index: Option<gitti_core::watch::IndexStamp>,
        stale: Option<gitti_core::lock::StaleLock>,
        /// When the re-read of `paths` started.
        read_at: std::time::SystemTime,
    },
    /// Redraw (time-based display like the delayed busy label).
    Tick,
    /// The commit message draft's write may be due (`app/commit_flow.rs`).
    DraftDue,
    /// The first read waited this long for the file watcher's first report: it goes
    /// ahead without (`app/watching.rs`).
    FirstReadDue,
    /// The commit message draft stored for this worktree, read at start (and Sign Off).
    Draft(Option<gitti_core::store::Draft>, Option<gitti_core::store::AmendDraft>, bool),
    /// The user's own earlier commit messages, read at this HEAD (`app/commit_box.rs`).
    OwnMessages(Option<String>, Vec<String>, commit_box::OwnFor),
    /// Only the working state, re-read after a display option changed (generation).
    Working(u64, Result<WorkingState>),
    /// The merge editor's session for a conflicted file, loaded in the background.
    MergeLoaded(Result<gitti_core::merge_session::MergeSession>),
    /// "Edit Commit Message": why the commit can't be reworded, else its message.
    RewordChecked(ObjectId, Result<(Option<String>, String)>),
    /// What overwriting a remote branch does (`app/overwrite.rs`), read in the background.
    OverwritePlan(Box<Result<gitti_core::overwrite::Overwrite>>),
    /// Push…'s force with lease: what each remote loses (`app/remote_cmds.rs`).
    ForcePushPlan(Box<Result<gitti_core::overwrite::ForcePush>>),
    Ask(gitti_core::askpass::Prompt, Sender<Option<String>>),
    /// The helper of a shown or queued prompt went away: its dialog closes.
    AskGone,
    Fs(Change),
    /// A path-limited re-read after an outside edit (`app/watching.rs`).
    PathsRead(Box<watching::PathsRead>),
    /// A live stderr line of the running operation (progress meters, hook output;
    /// clone's progress), shown after its busy label (`App::op_progress`).
    Progress(String),
    /// A file's new version for "show more lines" (`context_lines.rs`).
    ContextText(Box<context_lines::ContextText>),
    /// Go to Commit: early or final results of the query of that generation.
    CommitSearch(u64, Box<gitti_core::commit_search::SearchResult>),
    /// Search in Changes: an event of the scan of that generation, its end, and
    /// the debounce timer (`app/change_search.rs`).
    ChangeScan(u64, Box<gitti_core::change_search::ScanEvent>),
    ChangeScanDone(u64),
    ChangeSearchDue,
    /// "Check Out Commit in Submodule…": the history of the submodule at that path.
    SubmoduleHistory(String, Result<Vec<gitti_core::submodule::SubmoduleCommit>>),
    /// The other worktrees' marks for the sidebar (`app/worktrees.rs`).
    WorktreeMarks(Vec<(std::path::PathBuf, gitti_core::tab_marks::Marks)>),
    /// The subtrees of HEAD's history, read for that HEAD (`app/subtrees.rs`).
    Subtrees(Option<ObjectId>, Result<Vec<gitti_core::subtree::Subtree>>),
    /// The Git LFS health check's result (`app/lfs_health.rs`).
    LfsHealth(Box<gitti_core::lfs_health::LfsHealth>),
    /// The repository's Git LFS locks (`app/lfs.rs`).
    LfsLocks(Result<Vec<gitti_core::lfs::LfsLock>>),
    /// "Track with Git LFS…" for this path: git-lfs as found (`app/lfs.rs`).
    LfsSetup(String, gitti_core::lfs_track::LfsSetup),
    /// The repository location on the clipboard for the open Clone / Add Submodule dialog
    /// (its title; `app/repo_cmds.rs`).
    CloneClipboard(&'static str, Option<String>),
    /// A background load's result, applied to the app (`App::start_load` with a closure:
    /// "Branches and Tags Containing…", a restore's plan — `app/from_commit.rs`).
    Apply(Box<dyn FnOnce(&mut App) + Send>),
}

/// What to do after an operation succeeded.
pub enum After {
    Nothing,
    ClearMessage,
    /// Show the command output in a text window.
    ShowOutput(String),
    /// A repository was created there (clone / init): offer to switch to it.
    OfferOpen(std::path::PathBuf),
    /// Restart gitty in this repository (its own worktree was removed).
    Reopen(std::path::PathBuf),
    /// The output's last line is its summary (pushes): the status shows that one.
    Summary,
}

/// Syntax categories of the old and new version of a file (index = line number - 1).
#[derive(Clone, Default)]
pub struct FileHighlight {
    pub old: Option<Arc<Vec<LineCategories>>>,
    pub new: Option<Arc<Vec<LineCategories>>>,
}

/// Highlighted file texts by hash of (language, text), so refreshes don't re-parse.
type HighlightCache = Arc<std::sync::Mutex<HashMap<u64, Arc<Vec<LineCategories>>>>>;

/// A draggable pane separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    Sidebar,
    List,
    /// The git log's title bar (its height).
    Log,
}

/// An op that ends in a question instead of an error: returned as the op's error and turned
/// into a dialog in `OpDone`.
#[derive(Debug, Clone)]
pub enum FollowUp {
    /// git refused to switch because of untracked files / local changes.
    Switch(SwitchTarget, SwitchBlock),
    /// `branch -d` refused: not fully merged.
    ForceDelete(String),
    /// `worktree remove` refused (local changes / untracked files): Force Remove?
    WorktreeRefused {
        wt: gitti_core::worktree::Worktree,
        branch: Option<String>,
        message: String,
        changes: Vec<String>,
    },
    /// A split without a branch: its commit — create a branch on it?
    SplitDone {
        prefix: String,
        id: ObjectId,
        message: String,
    },
    /// The worktree is removed, `branch -d` refused: Force Delete (naming `lost`)?
    UnmergedBranch {
        branch: String,
        lost: Vec<String>,
        more: usize,
        removed: std::path::PathBuf,
        /// gitty ran in the removed worktree: restart in the main one afterwards.
        reopen: Option<std::path::PathBuf>,
    },
    /// Fetch, Then Decide (a refused push): what a push with lease overwrites now.
    Overwrite(Box<gitti_core::overwrite::Overwrite>),
    /// git refused to clone submodules from local paths (Update Submodules, a clone's
    /// `--recurse-submodules`): allow it for one update? (`app/submodules.rs`)
    SubmodulesRefused(Box<gitti_core::submodule::add::FileRefused>, submodules::RetryUpdate),
    /// No external diff / merge tool set up: the set-up dialog, then the tool
    /// (`app/ext_tools.rs`).
    ChooseTool(Box<gitti_core::tool_setup::ToolChoices>, Option<ext_tools::ToolJob>),
    /// A terminal diff / merge tool: it runs with the terminal handed over
    /// (`run_tool_in_terminal`, `app/ext_tools.rs`).
    ToolInTerminal(Box<gitti_core::ext_tool::TerminalRun>),
}

impl std::fmt::Display for FollowUp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FollowUp::Switch(t, _) => write!(f, "switching to {} is blocked", t.name()),
            FollowUp::ForceDelete(n) => write!(f, "{n} is not fully merged"),
            FollowUp::WorktreeRefused { wt, .. } => write!(f, "{} has changes", wt.name()),
            FollowUp::UnmergedBranch { branch, .. } => write!(f, "{branch} is not fully merged"),
            FollowUp::SplitDone { message, .. } => write!(f, "{message}"),
            FollowUp::Overwrite(o) => write!(f, "{}", o.title()),
            FollowUp::SubmodulesRefused(r, _) => write!(f, "{r}"),
            FollowUp::ChooseTool(..) => write!(f, "choose an external diff / merge tool"),
            FollowUp::ToolInTerminal(run) => write!(f, "{} runs in the terminal", run.command_line()),
        }
    }
}

impl std::error::Error for FollowUp {}

pub type Snapshot = gitti_core::RepoSnapshot;

/// A key action that came while an operation ran; it runs when that one is done, against
/// the state then shown (so `s` pressed twice stages two hunks).
type Deferred = Box<dyn FnOnce(&mut App) + Send>;

/// The busy label shows only for operations that take longer (staging takes ~25 ms; a
/// label for one frame would just flicker).
pub const BUSY_SHOW_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

/// A commit load running this long replaces the previous commit with "Loading…" (a merge
/// of 1700 files takes over a second; meanwhile the old one looked like the selected one).
/// Width of the explanation lines in the Discard All menu.
const DISCARD_NOTE_WIDTH: usize = 64;

/// Width of the explanation lines in the blocked-switch menu.
const SWITCH_NOTE_WIDTH: usize = 64;

const SLOW_LOAD: std::time::Duration = std::time::Duration::from_millis(300);

/// A shown commit: the cached, shared data (derefs to [`CommitData`]: `details`, `files`,
/// `sig_kind` — verified separately, see `App::signature`) plus how it was reached.
pub struct CommitView {
    pub id: ObjectId,
    pub data: Arc<CommitData>,
    pub stash: Option<usize>,
    /// Compare mode: `data` is the diff between two versions, not a commit (no header,
    /// read-only; `app/compare.rs`).
    pub compare: Option<Arc<gitti_core::compare::CompareData>>,
}

impl std::ops::Deref for CommitView {
    type Target = CommitData;
    fn deref(&self) -> &CommitData {
        &self.data
    }
}

pub enum Detail {
    Empty,
    Loading,
    Worktree,
    Commit(Arc<CommitView>),
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Sidebar,
    Commits,
    Detail,
    Message,
    /// The git command log panel (while shown): the keys scroll it.
    Log,
}

// The git log panel's position while scrolled back (pure, in `gitti_core::nav`).
pub use gitti_core::nav::{LogTop, log_first};

pub struct StatusLine {
    pub text: String,
    pub error: bool,
    /// Full text (error output), shown with `e`.
    pub full: Option<String>,
}

/// Display options (toggled at runtime, not persisted).
#[derive(Clone, Copy)]
pub struct ViewOpts {
    pub ignore_whitespace: bool,
    pub context: u32,
    pub topo: bool,
    pub wrap: bool,
    pub syntax: bool,
    /// Intra-line diff marks in percent (`SharedSettings::intraline_strength`), 0 = off.
    pub intraline_strength: u32,
    /// Commit list rows: Auto (one line, author from 60 columns), one line, two lines
    /// (shared with gitti).
    pub list_layout: gitti_core::list_layout::CommitListLayout,
}

impl ViewOpts {
    pub fn diff_options(&self) -> DiffOptions {
        DiffOptions {
            ignore_whitespace: self.ignore_whitespace,
            context_lines: self.context,
        }
    }
}

/// Screen areas of the last frame, for mouse hit testing and paging.
#[derive(Default, Clone, Copy)]
pub struct Areas {
    /// Columns of the separators after the sidebar / the commit list (drag handles).
    pub sep_sidebar: Option<u16>,
    pub sep_list: Option<u16>,
    /// The panes row (sidebar, commits, detail) between toolbar and log.
    pub body: Rect,
    pub sidebar: Rect,
    pub commits: Rect,
    /// Text rows per commit in the commit list (1, or 2 with two-line rows); 0
    /// before the first frame counts as 1 (see [`Areas::commit_rows`]).
    pub commit_row_h: u16,
    pub detail: Rect,
    pub message: Rect,
    /// The "Use It" / "Keep Mine" of the note under the message box (git's prepared
    /// message next to the user's own text); empty without the note.
    pub prepared_use: Rect,
    pub prepared_keep: Rect,
    /// The message box's "☐ sign off".
    pub sign_off: Rect,
    pub log: Rect,
    pub status: Rect,
    /// Item list of an open menu / picker.
    pub modal_list: Rect,
    /// The top line's error mark; a click opens its details.
    pub error_mark: Rect,
    /// The marks after the repository name (● uncommitted, ○ not pushed); a click says
    /// what they mean in the status line.
    pub repo_marks: Rect,
    /// The status line's "esc cancel" of a running operation; a click cancels.
    pub busy_cancel: Rect,
    /// The status line's "o more k/n"; a click pages the key bar.
    pub keybar_more: Rect,
    /// The running operation's output window.
    pub op_output: Rect,
    /// The first cell a pane drew outside its own area in the last frame (pane, x, y, its
    /// character): clipped away, reported by the script's `check`.
    pub spill: Option<(&'static str, u16, u16, char)>,
}

impl Areas {
    /// Text rows per commit in the commit list.
    pub fn commit_row_h(&self) -> u16 {
        self.commit_row_h.max(1)
    }

    /// Commits the list shows whole (at least one).
    pub fn commit_rows(&self) -> usize {
        (self.commits.height / self.commit_row_h()).max(1) as usize
    }

    /// The commit (offset-relative) at screen row `y` of the list.
    pub fn commit_at(&self, y: u16) -> usize {
        (y.saturating_sub(self.commits.y) / self.commit_row_h()) as usize
    }
}

pub struct App {
    pub repo: Arc<Repo>,
    tx: Sender<Msg>,
    pub pal: Palette,

    pub history: Arc<History>,
    pub history_full: bool,
    pub head: HeadInfo,
    pub working: Arc<WorkingState>,
    pub refs: Vec<RefInfo>,
    pub remotes: Vec<RemoteInfo>,
    pub stashes: Vec<StashInfo>,
    /// All worktrees (main first), for the worktree marks in the commit list.
    pub worktrees: Vec<gitti_core::worktree::Worktree>,
    /// Every submodule with its state (`app/submodules.rs`; empty without `.gitmodules`).
    pub submodules: Vec<gitti_core::submodule::Submodule>,
    /// The subtrees of HEAD's history (`app/subtrees.rs`), the HEAD they were read for,
    /// a read running / asked for again, a subtree operation ran (read them again).
    pub subtrees: Vec<gitti_core::subtree::Subtree>,
    subtrees_head: Option<Option<ObjectId>>,
    subtrees_loading: bool,
    subtrees_queued: bool,
    subtrees_dirty: bool,
    /// Git LFS: the repository's state, the notice shown, the locks (`app/lfs.rs`).
    pub lfs: lfs::LfsUi,
    /// The paths the "can't be checked out here" badge was hidden for (`app/fs_paths.rs`).
    pub fs_badge_dismissed: Option<Vec<String>>,
    /// The shown commit's (comparison's, stash's) paths this file system can't hold,
    /// against the index's paths (`app/fs_paths.rs`; set with the rows).
    pub commit_fs: Arc<gitti_core::fs_paths::PathReport>,
    pub repo_state: RepoState,
    pub tracking: Vec<Tracking>,
    /// The current branch's commits that are not pushed (the ○ after the repository name).
    pub unpushed: Option<gitti_core::tab_marks::Unpushed>,
    /// Every worktree's marks by folder (`app/worktrees.rs`): gitty's own published after
    /// each read of the working state, the others read in the background — the top line
    /// and the WORKTREES entries both read it. A read of the others runs.
    pub wt_marks: gitti_core::tab_marks::MarksStore,
    worktree_marks_loading: bool,
    /// The file watcher's first report arrived (watches in place; with a recursive OS
    /// watch it asks for one more read) — or there is no watcher. Scripted steps wait for
    /// it.
    pub watch_ready: bool,
    /// The first full read waits for the watcher's first report (`app/watching.rs`).
    pub(super) first_read_pending: bool,
    pub identity: (Option<String>, Option<String>),
    pub conflict_sides: Option<ConflictSides>,
    pub loaded: bool,

    pub focus: Focus,
    pub sidebar: Vec<SideItem>,
    pub side_cursor: usize,
    pub side_offset: usize,
    pub collapsed: HashSet<String>,
    /// The sidebar's branch order, pins, recent checkouts (`app/branch_cmds.rs`).
    pub branches: branch_cmds::BranchState,

    pub selected: usize,
    pub commit_offset: usize,

    pub detail: Detail,
    detail_gen: u64,
    detail_running: bool,
    detail_pending: bool,
    /// Commit header lines (drawn over the `HeaderSpace` rows).
    pub header: Vec<Line<'static>>,
    /// The detail pane's width the header's "Refs" lines wrap at (set by the renderer;
    /// 0 until the first frame).
    pub header_cols: u16,
    /// The commit list's "+N" badges as last drawn: where, and what they hide (one line,
    /// `ref_badges::ListBadges::hidden_summary`) — a click says it, as gitti's tooltip.
    pub badge_more: Vec<(Rect, String)>,
    /// The commit list's ref badges as last drawn (the shown part of each): a right click
    /// opens that ref's menu, as on its sidebar entry.
    pub badge_refs: Vec<(Rect, RefInfo)>,
    /// Commits being prefetched (`prefetch_around`).
    prefetching: Arc<std::sync::Mutex<std::collections::HashSet<ObjectId>>>,
    /// Row of the commit shown before, for the prefetch direction.
    last_shown_row: usize,
    /// Compare mode (marked base, prefetches).
    pub compare: compare::CompareState,
    /// Go to Commit (`search.rs`).
    pub search: search::SearchState,
    /// Search in Changes.
    pub change: change_search::ChangeSearchTui,
    /// The reflog view in place of the commit list (`app/reflog.rs`).
    pub reflog: reflog::ReflogTui,
    /// Verification result for the shown commit (`(id, None)` = failed to run).
    pub signature: Option<(ObjectId, Option<SignatureInfo>)>,
    /// No allowed signers file set for the shown SSH signature: the files to offer.
    signers_offer: Option<signers::SignersOffer>,
    /// A GitHub signing-key lookup or import is running (`app/signing_keys.rs`).
    pub keys_job: bool,
    /// The shared setting `terminal_command` ("Open in Terminal"; `app/terminal.rs`).
    pub terminal_command: String,
    /// A terminal is being started (detection, macOS `open`).
    pub terminal_job: bool,
    pub rows: Vec<DiffRow>,
    /// The folder rows of `rows` (tree view of the working directory; empty when flat).
    pub folders: gitti_core::rows::Folders,
    /// Files marked for an action on several (space on a file / folder row).
    pub marks: gitti_core::rows::FileMarks,
    /// The working directory's changes as a tree (setting `changes_as_tree`).
    pub changes_tree: bool,
    /// Marked files still to open in a terminal editor, one after the other.
    pub open_queue: Vec<String>,
    pub expansion: Expansion,
    /// Line totals of the working-directory sections / groups, summed when the rows
    /// are rebuilt (not per frame).
    pub worktree_totals: gitti_core::rows::WorktreeTotals,
    pub diff_cursor: usize,
    pub diff_offset: usize,
    /// The diff's place across a view toggle (`app/view_anchor.rs`).
    view_anchor: Option<view_anchor::PendingAnchor>,
    anchor_ids: u64,
    pub hscroll: usize,
    /// Columns available for diff text when wrapping (set by the renderer).
    pub wrap_cols: Option<usize>,
    /// Columns of the explanation rows of paths this file system can't hold (set by the
    /// renderer; they always wrap).
    pub note_cols: Option<usize>,
    pub line_sel: Option<LineSel>,

    pub message: TextEdit,
    /// The message box's rows and scroll (`message_view.rs`).
    pub message_view: crate::message_view::MessageView,
    /// The message git left for the next commit (`MERGE_MSG` / `SQUASH_MSG`) as put into
    /// the message box, so an untouched copy can follow it and go with it
    /// (`commit_flow::sync_prepared_message`).
    merge_prefill: Option<String>,
    /// That message as of the last refresh (`None`: git left none).
    prepared: Option<String>,
    /// The prepared message the user answered "Keep Mine" for: its note stays hidden.
    prepared_dismissed: Option<String>,
    pub amend: bool,
    /// The message draft's pending write (`app/commit_flow.rs`).
    draft: commit_flow::DraftState,
    /// Template, Sign Off, earlier messages (`app/commit_box.rs`).
    pub commit_box: commit_box::CommitBox,

    pub busy: Option<(String, Instant)>,
    pub status: Option<StatusLine>,
    /// Failed operations not looked at yet: "⚠ Pull failed" in the top line.
    pub errors: gitti_core::error_mark::ErrorMarks,
    pub modal: Option<Modal>,
    asks: VecDeque<(gitti_core::askpass::Prompt, Sender<Option<String>>)>,
    refresh_in_flight: bool,
    refresh_queued: Option<bool>,
    /// A path-limited re-read of an outside edit is running (`app/watching.rs`);
    /// refreshes wait for it.
    paths_in_flight: bool,
    /// Paths changed meanwhile, re-read after it or the running refresh.
    paths_queued: Option<Vec<String>>,
    /// Bumped whenever an operation changes the working state; a refresh that started
    /// before keeps its working-state part out (it would show the old state again).
    working_gen: u64,
    /// File-system changes seen while an operation ran.
    watch_pending: Option<Change>,
    /// Which watcher changes are already covered by what we read (see `WatchFilter`).
    watch_filter: gitti_core::watch::WatchFilter,
    /// Working-state reloads and other loads running (`Msg::Working`, …).
    loads_in_flight: u32,
    /// The history loads on its own, beside the working state.
    history_in_flight: bool,
    /// A history load asked for while one runs (`Some(force)`).
    history_queued: Option<bool>,
    /// The diff options the running `work_op` reads with.
    work_opts: DiffOptions,
    deferred: VecDeque<Deferred>,
    /// Cancels the running operation's git (`op_cancel.rs`; `None`: not cancellable).
    op_cancel: Option<gitti_core::process::CancelToken>,
    /// Live output of the running operation: its git commands stream their stderr
    /// (`gitti_core::process::with_progress` in `run_op`, `Msg::Progress`); the progress
    /// meter and the last hook / git line follow the busy label (gitti: the toolbar's
    /// busy mark, `app/op_progress.rs`).
    op_progress: gitti_core::progress::OpProgress,
    /// The running operation's output window (`app/op_output.rs`; `None`: closed).
    pub op_output: Option<op_output::OutputWin>,
    /// The status line's context key hints and their page (`app/keybar.rs`).
    pub keybar: keybar::KeybarState,
    /// git refused the repository (another user owns it, `safe.directory`) and the trust
    /// dialog was offered: once, not on every refresh.
    trust_offered: bool,

    pub opts: ViewOpts,
    pub show_log: bool,
    /// The sidebar is shown (`H`, palette "Toggle Sidebar"; the setting shared with
    /// gitti). Hidden it still shows while it has the focus (h / ← from the commit list).
    pub show_sidebar: bool,
    /// The sidebar lists the main worktree also without linked ones (palette toggle; the
    /// setting `show_main_worktree_alone` shared with gitti).
    pub main_worktree_alone: bool,
    pub log_reads: bool,
    /// The git log panel scrolled back (`None`: it follows the newest entries).
    pub log_top: Option<LogTop>,
    /// Last saved / loaded shared settings (`gitti.db`, same rows as gitti).
    pub settings: SharedSettings,
    /// Syntax highlighting of the shown diff.
    pub highlights: HashMap<(Src, String), FileHighlight>,
    hl_gen: u64,
    hl_running: bool,
    hl_cache: HighlightCache,
    /// Pane sizes set by dragging the separators (persisted, shared with gitti).
    pub layout: PaneLayout,
    /// The separator being dragged.
    pub dragging: Option<Divider>,
    /// The merge editor (T6), shown instead of the panes while open.
    pub merge: Option<crate::merge::MergeView>,
    /// Merge editor shows only the result (shared setting `merge_single_view`).
    pub merge_single: bool,
    /// A terminal editor to run with the terminal handed over (the main loop does it).
    pub run_in_terminal: Option<(Vec<String>, std::path::PathBuf)>,
    /// A terminal diff / merge tool to run with the terminal handed over (the main loop
    /// does it, then [`App::terminal_tool_done`]).
    pub run_tool_in_terminal: Option<Box<gitti_core::ext_tool::TerminalRun>>,
    pub cmd_history: Vec<String>,
    /// Switch to this repository: the main loop restarts gitty in it (one repository per
    /// process; `repo_cmds::reopen`).
    pub reopen: Option<std::path::PathBuf>,

    pub areas: Areas,
    /// Scroll the focused list to its cursor on the next frame (keyboard moves; the
    /// mouse wheel scrolls without moving the cursor).
    pub follow: bool,
    pub quit: bool,
    /// Quit was answered "Quit When Done" / "Stop and Quit": gitty quits once no
    /// operation runs (`commit_flow::request_quit`).
    pub quit_when_idle: bool,
}

pub fn spawn<T: Send + 'static>(
    tx: &Sender<Msg>,
    job: impl FnOnce() -> T + Send + 'static,
    wrap: impl FnOnce(T) -> Msg + Send + 'static,
) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(wrap(job()));
    });
}

/// A file to highlight: (key, language, old version, new version, diff).
type HighlightJob = (
    (Src, String),
    &'static str,
    Option<(gitti_core::blobs::Version, String)>,
    Option<(gitti_core::blobs::Version, String)>,
    FileDiff,
);

fn push_job(
    jobs: &mut Vec<HighlightJob>,
    src: Src,
    d: &FileDiff,
    old: Option<gitti_core::blobs::Version>,
    new: Option<gitti_core::blobs::Version>,
) {
    use gitti_core::diff::FileChange;
    // An LFS file's versions in git are pointers, not the lines shown.
    if jobs.len() >= 300 || d.binary || d.hunks.is_empty() || d.lfs.is_some() {
        return;
    }
    let Some(lang) = gitti_core::syntax::language_for_path(d.path()) else {
        return;
    };
    let old_path = d.old_path.clone().unwrap_or_else(|| d.path().to_string());
    let old = old.filter(|_| d.change != FileChange::Added).map(|v| (v, old_path));
    let new = new
        .filter(|_| d.change != FileChange::Deleted)
        .map(|v| (v, d.path().to_string()));
    jobs.push(((src, d.path().to_string()), lang, old, new, d.clone()));
}

/// Jobs for the working-directory view (only files at `only`'s paths when given).
fn worktree_jobs(working: &WorkingState, only: Option<&HashSet<&str>>) -> Vec<HighlightJob> {
    use gitti_core::blobs::Version;
    let mut jobs = Vec::new();
    let wanted = |f: &&WorkFile| only.is_none_or(|o| o.contains(f.path()));
    let head = || Some(Version::Rev("HEAD".into()));
    for f in working.conflicted.iter().filter(wanted) {
        if let Some(d) = &f.diff {
            push_job(&mut jobs, Src::Conflicted, d, head(), Some(Version::Worktree));
        }
    }
    for f in working.unstaged.iter().filter(wanted) {
        if let Some(d) = &f.diff {
            let old = (!f.entry.untracked).then_some(Version::Index);
            push_job(&mut jobs, Src::Unstaged, d, old, Some(Version::Worktree));
        }
    }
    for f in working.staged.iter().filter(wanted) {
        if let Some(d) = &f.diff {
            push_job(&mut jobs, Src::Staged, d, head(), Some(Version::Index));
        }
    }
    jobs
}

/// Reads the file versions of `jobs` in one batch and highlights them (memoised per text).
fn compute_highlights(
    repo: &Repo,
    jobs: Vec<HighlightJob>,
    cache: &HighlightCache,
) -> HashMap<(Src, String), FileHighlight> {
    use gitti_core::blobs::hunk_texts;
    use std::hash::{Hash, Hasher};
    let mut requests = Vec::new();
    for (_, _, old, new, _) in &jobs {
        requests.extend(old.clone());
        requests.extend(new.clone());
    }
    let texts = repo.read_versions(&requests).unwrap_or_default();
    let mut texts = texts.into_iter();
    let mut out = HashMap::new();
    for (key, lang, old, new, diff) in jobs {
        let old_text = old.as_ref().and_then(|_| texts.next().flatten());
        let new_text = new.as_ref().and_then(|_| texts.next().flatten());
        let (fallback_old, fallback_new) = hunk_texts(&diff);
        let side = |text: Option<String>, fallback: String| {
            let text = text.unwrap_or(fallback);
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (lang, &text).hash(&mut h);
            let k = h.finish();
            if let Some(hit) = cache.lock().ok()?.get(&k) {
                return Some(hit.clone());
            }
            let lines = Arc::new(gitti_core::syntax::highlight(lang, &text)?);
            let mut c = cache.lock().ok()?;
            if c.len() > 400 {
                c.clear();
            }
            c.insert(k, lines.clone());
            Some(lines)
        };
        let hl = FileHighlight {
            old: old.is_some().then(|| side(old_text, fallback_old)).flatten(),
            new: new.is_some().then(|| side(new_text, fallback_new)).flatten(),
        };
        out.insert(key, hl);
    }
    out
}

pub fn work_files(ws: &WorkingState, src: Src) -> &[WorkFile] {
    match src {
        Src::Conflicted => &ws.conflicted,
        Src::Unstaged => &ws.unstaged,
        Src::Staged => &ws.staged,
        Src::Commit => &[],
    }
}

impl App {
    pub fn new(repo: Arc<Repo>, tx: Sender<Msg>) -> Self {
        let settings: SharedSettings = store::load_settings();
        crate::kb::reload();
        let mut collapsed = HashSet::new();
        collapsed.insert("tags".to_string());
        Self {
            repo,
            tx,
            pal: Palette::dark(),
            history: Arc::default(),
            history_full: false,
            head: HeadInfo::default(),
            working: Arc::default(),
            refs: Vec::new(),
            remotes: Vec::new(),
            stashes: Vec::new(),
            worktrees: Vec::new(),
            submodules: Vec::new(),
            subtrees: Vec::new(),
            subtrees_head: None,
            subtrees_loading: false,
            subtrees_queued: false,
            subtrees_dirty: false,
            lfs: Default::default(),
            fs_badge_dismissed: None,
            commit_fs: Default::default(),
            repo_state: RepoState::Clean,
            tracking: Vec::new(),
            unpushed: None,
            wt_marks: Default::default(),
            worktree_marks_loading: false,
            watch_ready: true,
            first_read_pending: false,
            identity: (None, None),
            conflict_sides: None,
            loaded: false,
            focus: Focus::Commits,
            sidebar: Vec::new(),
            side_cursor: 0,
            side_offset: 0,
            collapsed,
            branches: branch_cmds::BranchState::new(&settings),
            selected: 0,
            commit_offset: 0,
            detail: Detail::Empty,
            detail_gen: 0,
            detail_running: false,
            detail_pending: false,
            header: Vec::new(),
            signature: None,
            signers_offer: None,
            keys_job: false,
            terminal_command: settings.terminal_command.clone(),
            terminal_job: false,
            prefetching: Default::default(),
            last_shown_row: 0,
            compare: Default::default(),
            search: Default::default(),
            change: Default::default(),
            reflog: Default::default(),
            rows: Vec::new(),
            folders: Default::default(),
            marks: Default::default(),
            changes_tree: settings.changes_as_tree,
            open_queue: Vec::new(),
            expansion: Expansion::default(),
            worktree_totals: Default::default(),
            diff_cursor: 0,
            diff_offset: 0,
            view_anchor: None,
            anchor_ids: 0,
            hscroll: 0,
            wrap_cols: None,
            note_cols: None,
            header_cols: 0,
            badge_more: Vec::new(),
            badge_refs: Vec::new(),
            line_sel: None,
            message: TextEdit::multi(),
            message_view: Default::default(),
            merge_prefill: None,
            prepared: None,
            prepared_dismissed: None,
            amend: false,
            draft: Default::default(),
            commit_box: Default::default(),
            busy: None,
            status: None,
            errors: Default::default(),
            modal: None,
            asks: VecDeque::new(),
            refresh_in_flight: false,
            refresh_queued: None,
            paths_in_flight: false,
            paths_queued: None,
            working_gen: 0,
            work_opts: DiffOptions::default(),
            loads_in_flight: 0,
            history_in_flight: false,
            history_queued: None,
            watch_pending: None,
            watch_filter: Default::default(),
            deferred: VecDeque::new(),
            op_cancel: None,
            op_progress: Default::default(),
            op_output: None,
            keybar: keybar::KeybarState::default(),
            trust_offered: false,
            opts: ViewOpts {
                ignore_whitespace: settings.ignore_whitespace,
                context: settings.diff_context,
                topo: settings.topo_order,
                wrap: settings.wrap_lines,
                syntax: settings.syntax_highlight,
                intraline_strength: settings.intraline_strength,
                list_layout: settings.commit_list_layout,
            },
            show_log: settings.show_git_log,
            show_sidebar: settings.show_sidebar,
            main_worktree_alone: settings.show_main_worktree_alone,
            log_reads: settings.log_show_reads,
            log_top: None,
            merge_single: settings.merge_single_view,
            settings,
            merge: None,
            run_in_terminal: None,
            run_tool_in_terminal: None,
            layout: store::load_settings(),
            highlights: HashMap::new(),
            hl_gen: 0,
            hl_running: false,
            hl_cache: Default::default(),
            dragging: None,
            cmd_history: Vec::new(),
            reopen: None,
            areas: Areas::default(),
            follow: true,
            quit: false,
            quit_when_idle: false,
        }
    }

    fn order(&self) -> LogOrder {
        if self.opts.topo {
            LogOrder::Topo
        } else {
            LogOrder::Date
        }
    }

    /// First screen: the newest commits, then the full refresh.
    pub fn start(&mut self) {
        let repo = self.repo.clone();
        let order = self.order();
        spawn(
            &self.tx,
            move || repo.history_newest(order, 500),
            Msg::QuickHistory,
        );
        self.first_read();
        self.load_draft();
        // A git older than the supported minimum: said once, stays until the next action.
        if let Some(warning) = gitti_core::caps::GitCaps::get().warning() {
            self.set_error(warning);
        }
    }

    pub fn handle(&mut self, msg: Msg) {
        self.handle_msg(msg);
        self.save_settings();
        if self.quit_when_idle && self.busy.is_none() {
            self.quit = true;
        }
    }

    /// The view options as shared settings (see `store::SharedSettings`).
    fn current_settings(&self) -> SharedSettings {
        SharedSettings {
            topo_order: self.opts.topo,
            wrap_lines: self.opts.wrap,
            ignore_whitespace: self.opts.ignore_whitespace,
            diff_context: self.opts.context,
            show_git_log: self.show_log,
            log_show_reads: self.log_reads,
            editor_command: self.settings.editor_command.clone(),
            syntax_highlight: self.opts.syntax,
            merge_single_view: self.merge_single,
            terminal_command: self.terminal_command.clone(),
            show_sidebar: self.show_sidebar,
            spell_check: self.settings.spell_check,
            intraline_strength: self.opts.intraline_strength,
            commit_list_layout: self.opts.list_layout,
            show_main_worktree_alone: self.main_worktree_alone,
            branch_sort: self.branches.sort,
            remote_branch_sort: self.branches.remote_sort,
            changes_as_tree: self.changes_tree,
        }
    }

    /// Writes changed view options to `gitti.db` (shared with gitti, per field).
    pub fn save_settings(&mut self) {
        let now = self.current_settings();
        if now != self.settings {
            store::save_changed_settings(&self.settings, &now);
            self.settings = now;
        }
    }

    /// Takes over settings changed elsewhere (gitti, another gitty) — on focus gain.
    pub fn reload_settings(&mut self) {
        // gitti may have changed the keyboard layout (keys and help read it live).
        crate::kb::reload();
        let loaded: SharedSettings = store::load_settings();
        if loaded == self.settings {
            return;
        }
        let diffs_changed = loaded.ignore_whitespace != self.opts.ignore_whitespace
            || loaded.diff_context != self.opts.context;
        let order_changed = loaded.topo_order != self.opts.topo;
        let wrap_changed = loaded.wrap_lines != self.opts.wrap;
        let syntax_changed = loaded.syntax_highlight != self.opts.syntax;
        self.apply_settings(&loaded);
        self.settings = loaded;
        if diffs_changed {
            self.line_sel = None;
            self.reload_diffs();
        } else if wrap_changed {
            self.rebuild_rows();
        }
        if order_changed {
            self.refresh(true);
        }
        if syntax_changed {
            self.schedule_highlight();
        }
    }

    fn apply_settings(&mut self, s: &SharedSettings) {
        self.opts = ViewOpts {
            ignore_whitespace: s.ignore_whitespace,
            context: s.diff_context,
            topo: s.topo_order,
            wrap: s.wrap_lines,
            syntax: s.syntax_highlight,
            intraline_strength: s.intraline_strength,
            list_layout: s.commit_list_layout,
        };
        self.show_log = s.show_git_log;
        self.show_sidebar = s.show_sidebar;
        if self.main_worktree_alone != s.show_main_worktree_alone {
            self.main_worktree_alone = s.show_main_worktree_alone;
            self.rebuild_sidebar();
        }
        if (self.branches.sort, self.branches.remote_sort) != (s.branch_sort, s.remote_branch_sort) {
            self.branches.sort = s.branch_sort;
            self.branches.remote_sort = s.remote_branch_sort;
            self.rebuild_sidebar();
        }
        if self.changes_tree != s.changes_as_tree {
            self.changes_tree = s.changes_as_tree;
            self.rebuild_rows();
        }
        self.log_reads = s.log_show_reads;
        self.merge_single = s.merge_single_view;
        self.terminal_command = s.terminal_command.clone();
    }

    fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Term(ev) => self.on_event(ev),
            Msg::QuickHistory(h) => {
                if let Ok(h) = h
                    && !self.history_full
                {
                    self.set_history(h);
                }
            }
            Msg::Refreshed(snapshot, generation) => {
                self.refresh_in_flight = false;
                let failed = snapshot.is_err();
                match *snapshot {
                    Ok(mut s) => {
                        if generation != self.working_gen {
                            // An operation changed the working state meanwhile: this one
                            // is older than what is shown. Read again.
                            s.working = (*self.working).clone();
                            s.conflict_sides = self.conflict_sides.clone();
                            self.refresh_queued = Some(self.refresh_queued.unwrap_or(false));
                        }
                        self.errors.succeeded("Refresh");
                        self.apply_snapshot(s)
                    }
                    Err(e) => {
                        let text = gitti_core::error_mark::refresh_failed(&format!("{e:#}"));
                        self.errors.failed("Refresh", text.clone());
                        self.set_error(text);
                        if let Some(d) = gitti_core::safe_directory::dubious_ownership(&e) {
                            self.offer_trust_directory(d);
                        }
                    }
                }
                // Start-up timings: the first read counts, later ones are ignored.
                if let Some(record) =
                    gitti_core::startup::tab_ready(&self.repo.name(), self.repo.command_log(), failed)
                {
                    self.startup_finished(&record);
                }
                if let Some(full) = self.refresh_queued.take() {
                    self.refresh(full);
                } else if let Some(paths) = self.paths_queued.take() {
                    self.refresh_paths(paths);
                }
            }
            Msg::Detail(generation, result) => {
                self.detail_running = false;
                if generation == self.detail_gen {
                    match result {
                        Ok(cv) => {
                            self.show_commit(cv);
                            self.prefetch_around(self.selected);
                        }
                        Err(e) => self.detail = Detail::Error(format!("{e:#}")),
                    }
                }
                if std::mem::take(&mut self.detail_pending) {
                    self.load_detail();
                }
            }
            Msg::DetailSlow(generation) => {
                if self.detail_running
                    && generation == self.detail_gen
                    && matches!(self.detail, Detail::Commit(_))
                {
                    self.detail = Detail::Loading;
                    self.rows.clear();
                }
            }
            Msg::Highlights(generation, map) => {
                if generation == self.hl_gen {
                    self.highlights = map;
                    self.hl_running = false;
                }
            }
            Msg::Signature(id, result) => {
                if let Detail::Commit(cv) = &self.detail
                    && cv.id == id
                {
                    let (info, offer) = *result;
                    self.signature = Some((id, info));
                    self.signers_offer = offer;
                    let cv = cv.clone();
                    self.header = crate::ui::commit_header(self, &cv);
                }
            }
            Msg::SigningKeys(m) => self.signing_keys_msg(*m),
            Msg::SignersSet(result) => self.signers_set(result),
            Msg::TerminalOpened(result, dir) => self.terminal_opened(result, dir),
            Msg::OpDone {
                label,
                result,
                full,
                after,
                stale,
            } => {
                self.busy = None;
                // The output window goes with its operation.
                self.op_output = None;
                let cancelled = self.op_cancel.take().is_some_and(|t| t.is_cancelled());
                // Set false by a follow-up that changed nothing yet.
                let mut refresh = true;
                match result {
                    Ok(out) => {
                        let summary = matches!(after, After::Summary);
                        match after {
                            After::Nothing | After::Summary => {}
                            After::ClearMessage => {
                                self.message = TextEdit::multi();
                                self.merge_prefill = None;
                                self.amend = false;
                                self.committed();
                            }
                            After::ShowOutput(title) => {
                                if !out.trim().is_empty() {
                                    self.modal = Some(Modal::text(title, out.clone()));
                                }
                            }
                            After::OfferOpen(path) => {
                                self.offer_open(path, out.split_once("\n\n").map(|(_, n)| n))
                            }
                            After::Reopen(path) => self.reopen = Some(path),
                        }
                        self.errors.succeeded(&label);
                        let first = if summary {
                            out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("")
                        } else {
                            out.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
                        };
                        self.set_info(if summary && !first.is_empty() {
                            first.trim().to_string()
                        } else if first.is_empty() {
                            format!("{label} — done")
                        } else {
                            format!("{label} — {}", first.trim())
                        });
                    }
                    // Esc while busy: not a failure.
                    Err(_) if cancelled => self.set_info(format!("{label} cancelled")),
                    Err(e) => {
                        if let Some(f) = e.downcast_ref::<FollowUp>() {
                            // The external tool's set-up dialog / terminal run: nothing
                            // changed yet (the tool's end refreshes).
                            refresh = !matches!(f, FollowUp::ChooseTool(..) | FollowUp::ToolInTerminal(_));
                            self.follow_up(f.clone());
                        } else if let Some(block) = e.downcast_ref::<PullBlock>() {
                            // The upstream went since the last refresh: no git ran.
                            self.pull_blocked(block.clone());
                        } else if let Some(block) = e.downcast_ref::<PushBlock>() {
                            // The same for the main push.
                            self.push_blocked(block.clone());
                        } else if let Some(r) =
                            e.downcast_ref::<gitti_core::overwrite::PushRejected>()
                        {
                            // A refused push: the ways out (pull, push with lease, …).
                            self.push_rejected(r.clone());
                        } else if let Some(d) = e.downcast_ref::<DivergentPull>() {
                            // Diverged, no strategy: the user chooses one.
                            self.pull_diverged(d.clone());
                        } else {
                            if matches!(after, After::ClearMessage)
                                && e.is::<gitti_core::ops::MessageCommitted>()
                            {
                                // Continue used the box's text before git stopped again.
                                self.message = TextEdit::multi();
                                self.merge_prefill = None;
                                self.committed();
                            }
                            self.errors.failed(&label, format!("{e:#}"));
                            self.set_error(format!("{label} failed: {e:#}"));
                            if let Some(u) = e.downcast_ref::<UnrelatedHistories>() {
                                self.offer_unrelated_merge(u.clone());
                            }
                            if let Some(lock) = stale {
                                self.offer_stale_lock(&label, lock);
                            }
                        }
                    }
                }
                if refresh {
                    self.refresh(full);
                }
                self.run_deferred();
            }
            Msg::WorkDone {
                label,
                result,
                working,
                highlights,
                paths,
                index,
                stale,
                read_at,
            } => {
                self.busy = None;
                self.op_output = None;
                let cancelled = self.op_cancel.take().is_some_and(|t| t.is_cancelled());
                self.working_gen += 1;
                self.watch_filter.own_index(index);
                // Our own writes to those files (discard) are read already.
                if let (Some(p), Ok(_)) = (&paths, &working) {
                    self.watch_filter.paths_read(p, read_at);
                }
                match result {
                    Ok(note) => {
                        self.errors.succeeded(&label);
                        if let Some(note) = note {
                            self.set_info(note);
                        }
                    }
                    Err(_) if cancelled => self.set_info(format!("{label} cancelled")),
                    Err(e) => {
                        self.errors.failed(&label, format!("{e:#}"));
                        self.set_error(format!("{label} failed: {e:#}"));
                    }
                }
                if let Some(lock) = stale {
                    self.offer_stale_lock(&label, lock);
                }
                match working {
                    Ok(w) => {
                        self.apply_working(w, highlights, paths);
                        self.refresh_submodule_states();
                    }
                    Err(e) => {
                        self.set_error(format!("{e:#}"));
                        self.refresh(false);
                    }
                }
                // A display option changed while it ran (its own reload was dropped).
                if self.opts.diff_options() != self.work_opts {
                    self.reload_working();
                }
                self.run_deferred();
            }
            Msg::Tick => {}
            Msg::DraftDue => self.draft_due(),
            Msg::FirstReadDue => self.first_read_due(),
            Msg::Draft(draft, amend, sign_off) => {
                self.commit_box.sign_off = sign_off;
                self.restore_draft(draft, amend)
            }
            Msg::OwnMessages(head, own, then) => self.own_messages_read(head, own, then),
            Msg::CommitSearch(generation, r) => self.on_commit_search(generation, *r),
            Msg::ChangeScan(generation, ev) => self.on_change_scan(generation, *ev),
            Msg::ChangeScanDone(generation) => self.on_change_scan_done(generation),
            Msg::ChangeSearchDue => self.change_search_due(),
            Msg::Subtrees(head, result) => self.on_subtrees(head, result),
            Msg::WorktreeMarks(marks) => self.on_worktree_marks(marks),
            Msg::SubmoduleHistory(path, result) => {
                self.loads_in_flight -= 1;
                self.on_submodule_history(&path, result);
            }
            Msg::LfsHealth(health) => {
                self.loads_in_flight -= 1;
                self.lfs_health_read(*health);
            }
            Msg::LfsLocks(result) => {
                self.loads_in_flight -= 1;
                self.lfs_locks_read(result);
            }
            Msg::LfsSetup(path, setup) => {
                self.loads_in_flight -= 1;
                self.lfs_setup_read(path, setup);
            }
            Msg::CloneClipboard(title, url) => self.clone_clipboard(title, url),
            Msg::Working(generation, result) => {
                self.loads_in_flight -= 1;
                if generation == self.working_gen {
                    match result {
                        Ok(w) => self.apply_working(w, None, None),
                        Err(e) => self.set_error(format!("{e:#}")),
                    }
                }
            }
            Msg::ContextText(t) => {
                self.loads_in_flight -= 1;
                self.context_text_loaded(*t);
            }
            Msg::MergeLoaded(session) => {
                self.loads_in_flight -= 1;
                self.merge_loaded(session);
            }
            Msg::RewordChecked(id, checked) => {
                self.loads_in_flight -= 1;
                self.reword_checked(id, checked);
            }
            Msg::OverwritePlan(plan) => {
                self.loads_in_flight -= 1;
                self.overwrite_planned(*plan);
            }
            Msg::Apply(f) => {
                self.loads_in_flight -= 1;
                f(self);
            }
            Msg::ForcePushPlan(plan) => {
                self.loads_in_flight -= 1;
                self.force_push_planned(*plan);
            }
            Msg::FileLoaded(file, result) => {
                self.loads_in_flight -= 1;
                match result {
                    Ok((data, compare)) => self.show_loaded_file(file, data, compare),
                    Err(e) => self.set_error(format!("{e:#}")),
                }
            }
            Msg::HistoryLoaded(result) => {
                self.history_in_flight = false;
                match result {
                    // Another order was chosen meanwhile: the queued load brings it.
                    Ok(Some(h)) if h.order() == self.order() => {
                        self.history_full = true;
                        self.set_history(h);
                        self.check_compare_base();
                    }
                    Ok(_) => {}
                    Err(e) => self.set_error(format!("loading history: {e:#}")),
                }
                if let Some(force) = self.history_queued.take() {
                    self.load_history(force);
                }
            }
            Msg::Progress(line) => {
                let cancelled = self.op_cancel.as_ref().is_some_and(|t| t.is_cancelled());
                if self.busy.is_some() && !cancelled {
                    self.op_progress.feed(&line);
                }
            }
            Msg::Ask(prompt, reply) => {
                // Gone already, or its operation is being cancelled (its ssh / hooks may
                // ask once more before they are stopped): refused right away.
                let cancelling = self.busy.is_some()
                    && self.op_cancel.as_ref().is_some_and(|t| t.is_cancelled());
                if prompt.asker.is_gone() || cancelling {
                    let _ = reply.send(None);
                } else {
                    self.asks.push_back((prompt, reply));
                    self.next_ask();
                }
            }
            Msg::AskGone => self.drop_gone_asks(),
            Msg::Fs(change) => self.on_fs(change),
            Msg::PathsRead(r) => self.on_paths_read(*r),
        }
    }

    /// No background job pending (scripted runs wait for this).
    pub fn is_idle(&self) -> bool {
        !self.refresh_in_flight
            && self.refresh_queued.is_none()
            && !self.history_in_flight
            && self.history_queued.is_none()
            && !self.paths_in_flight
            && self.paths_queued.is_none()
            && self.loads_in_flight == 0
            && self.watch_ready
            && self.busy.is_none()
            && !self.detail_running
            && !self.detail_pending
            && !self.signature_pending()
            && !self.keys_job
            && !self.terminal_job
            && !self.hl_running
            && self.prefetching.lock().is_ok_and(|p| p.is_empty())
            && self.compare.idle()
            && !self.search.running
            && !self.change.busy()
            && !self.reflog.busy()
            && (self.history_full || self.status.as_ref().is_some_and(|s| s.error))
    }

    /// One-line summary for scripted runs.
    pub fn debug_state(&self) -> String {
        let sel = match self.selected_id() {
            Some(id) => format!(
                "{} {}",
                &id.to_string()[..7],
                self.history.meta(self.selected).summary
            ),
            None => "worktree".into(),
        };
        let row = self
            .rows
            .get(self.diff_cursor)
            .map(|r| format!("{r:?}"))
            .unwrap_or_default();
        let lines = self.line_sel.as_ref().map(|s| s.lines.len()).unwrap_or(0);
        let status = self
            .status
            .as_ref()
            .map(|s| format!("{}{}", if s.error { "ERROR " } else { "" }, s.text))
            .unwrap_or_default();
        let log_first = log_first(self.log_top.as_ref(), &self.log_entries(), self.log_rows());
        let list = match &self.modal {
            Some(Modal::Menu { cursor, offset, .. } | Modal::Picker { cursor, offset, .. }) => {
                format!(" list_cursor={cursor} list_offset={offset}")
            }
            Some(Modal::Text { scroll, .. } | Modal::Help { scroll }) => {
                format!(" text_scroll={scroll}")
            }
            _ => String::new(),
        };
        format!(
            "focus={:?} selected={} [{sel}] cursor={} {row} sel_lines={lines} marked={} unstaged={} staged={} \
             repo_state={} conflicted={} history_rows={} history_load={:?} status={status:?} \
             commit_offset={} diff_offset={} hscroll={} side_cursor={} side_offset={} \
             log_first={log_first} log_follow={} top={}{list}{}{}{}",
            self.focus,
            self.selected,
            self.diff_cursor,
            self.marks.len(),
            self.working.unstaged.len(),
            self.working.staged.len(),
            format!("{:?}", self.repo_state).to_lowercase(),
            self.working.conflicted.len(),
            self.history.commits.len(),
            self.history.load,
            self.commit_offset,
            self.diff_offset,
            self.hscroll,
            self.side_cursor,
            self.side_offset,
            self.log_top.is_none(),
            self.top_line_debug(),
            self.compare_debug(),
            self.errors
                .text()
                .map(|t| format!(" error_mark={t:?}"))
                .unwrap_or_default(),
            self.modal
                .as_ref()
                .map(|m| format!(" modal={}", m.debug_name()))
                .unwrap_or_default(),
        )
    }

    // ------------------------------------------------------------------ status

    /// The recorded start is complete: a slow one says so once.
    pub fn startup_finished(&mut self, record: &gitti_core::startup::StartupRecord) {
        if let Some(text) = record.slow_notice("ctrl-p ▸ Start-up Timings…") {
            self.set_info(text);
        }
    }

    /// The mark after the repository name: ● uncommitted changes, else ○ commits not
    /// pushed (`gitti_core::tab_marks::mark`, from what the refreshes read anyway).
    pub fn repo_mark(&self) -> Option<gitti_core::tab_marks::Mark> {
        self.own_marks_shown().mark()
    }

    /// gitty's own worktree's marks as the store has them (the same entry its WORKTREES
    /// section shows).
    fn own_marks_shown(&self) -> gitti_core::tab_marks::Marks {
        self.wt_marks
            .get(self.repo.workdir())
            .cloned()
            .unwrap_or_else(|| self.own_marks())
    }

    /// The marks of gitty's own working state.
    pub(crate) fn own_marks(&self) -> gitti_core::tab_marks::Marks {
        gitti_core::tab_marks::Marks::of(&self.working.status, self.unpushed.as_ref())
    }

    /// Puts gitty's own marks into the store (after each read of the working state).
    fn publish_marks(&mut self) {
        let marks = self.own_marks();
        let workdir = self.repo.workdir().to_path_buf();
        self.wt_marks.publish(&workdir, 0, marks);
    }

    /// What the marks mean, in the status line (a click on them).
    pub fn explain_repo_marks(&mut self) {
        let text = self.own_marks_shown().describe().replace('\n', " · ");
        if !text.is_empty() {
            self.set_info(text);
        }
    }

    pub fn set_info(&mut self, text: impl Into<String>) {
        self.status = Some(StatusLine {
            text: text.into(),
            error: false,
            full: None,
        });
    }

    pub fn set_error(&mut self, full: String) {
        self.status = Some(StatusLine {
            text: summarize_error(&full),
            error: true,
            full: Some(full),
        });
    }

    // ------------------------------------------------------------------ loading

    /// Reloads working state and refs; with `full` also checks the history, in a job of
    /// its own: the working state shows as soon as it is read.
    pub fn refresh(&mut self, full: bool) {
        if self.refresh_in_flight || self.paths_in_flight {
            self.refresh_queued = Some(self.refresh_queued.unwrap_or(false) || full);
            return;
        }
        self.refresh_in_flight = true;
        // It reads them too.
        self.paths_queued = None;
        self.watch_filter.read_started(full);
        if full {
            self.load_history(false);
        }
        let repo = self.repo.clone();
        let opts = self.opts.diff_options();
        let generation = self.working_gen;
        spawn(
            &self.tx,
            move || repo.snapshot(&opts),
            move |s| Msg::Refreshed(Box::new(s), generation),
        );
    }

    /// The user's Refresh: the working state and the whole history read again (the full
    /// refresh's own history check then finds nothing moved).
    pub fn refresh_everything(&mut self) {
        self.load_history(true);
        self.refresh(true);
    }

    /// (Re)loads the history in the background, apart from the working state:
    /// `force` loads it in full, else `Repo::history_update` keeps the shown one when no
    /// ref moved and reuses its rows where it can. One at a time; asked for meanwhile, it
    /// runs once more afterwards.
    pub fn load_history(&mut self, force: bool) {
        if self.history_in_flight {
            self.history_queued = Some(self.history_queued.unwrap_or(false) || force);
            return;
        }
        self.history_in_flight = true;
        let repo = self.repo.clone();
        let order = self.order();
        let prev = (!force && self.history_full).then(|| self.history.clone());
        spawn(
            &self.tx,
            move || match prev {
                Some(prev) => repo.history_update(&prev, order),
                None => repo.history(order).map(Some),
            },
            Msg::HistoryLoaded,
        );
    }

    fn apply_snapshot(&mut self, s: Snapshot) {
        // Identify the line selection's file by its diff, to keep it if unchanged.
        let prev_sel_diff = self.line_sel.as_ref().and_then(|sel| {
            work_files(&self.working, sel.src)
                .get(sel.file)
                .and_then(|f| f.diff.clone())
        });
        let at = self.cursor_file();
        let regroup = self.regroup_anchor(&s.working);
        let prev = std::mem::replace(&mut self.working, Arc::new(s.working));
        // Refs moved behind our back → the history may have changed too (the first
        // refresh loads it anyway).
        let same_refs = self.refs.len() == s.refs.len()
            && self
                .refs
                .iter()
                .zip(&s.refs)
                .all(|(a, b)| a.full_name == b.full_name && a.target == b.target);
        if self.loaded && (!same_refs || self.head.id != s.head.id) {
            self.load_history(false);
        }
        if !same_refs || self.head.id != s.head.id || self.head.branch != s.head.branch {
            // The open reflog has new entries (`app/reflog.rs`).
            self.reflog_refs_moved();
        }
        self.head = s.head;
        self.refs = s.refs;
        self.branches.recent = s.recent_checkouts;
        self.remotes = s.remotes;
        self.stashes = s.stashes;
        self.worktrees = s.worktrees;
        self.submodules = s.submodules;
        self.apply_lfs_state(s.lfs);
        self.repo_state = s.state;
        self.tracking = s.tracking;
        self.unpushed = s.unpushed;
        self.identity = s.identity;
        self.conflict_sides = s.conflict_sides;
        self.commit_box.template = s.commit_template;
        self.sync_prepared_message(s.prepared_message);
        self.loaded = true;
        // The header's badges: the current branch, branches in other worktrees.
        self.rewrap_header();
        // Only when HEAD moved, or after a subtree operation (in the background).
        let force = self.subtrees_dirty && self.busy.is_none();
        if force {
            self.subtrees_dirty = false;
        }
        self.refresh_subtrees(force);
        // gitty's own marks for the top line and its WORKTREES entry; the other
        // worktrees' (in the background).
        self.publish_marks();
        self.refresh_worktree_marks();
        // The working directory is row 0 of every history: its state shows before the
        // history has loaded.
        if matches!(self.detail, Detail::Empty) && self.selected == 0 {
            self.detail = Detail::Worktree;
        }
        self.rebuild_sidebar();
        self.check_compare_base();
        self.refresh_worktree_compare();
        if matches!(self.detail, Detail::Worktree) {
            self.rebuild_rows();
            self.apply_regroup(regroup);
            self.schedule_highlight();
            self.follow_cursor_file(&prev, at);
            let sel = self.line_sel.take();
            self.line_sel = sel
                .zip(prev_sel_diff)
                .and_then(|(sel, prev)| self.remap_selection(sel, &prev));
        }
    }

    /// The first / last untracked file adds / removes the group headers of the unstaged
    /// section above the rows on screen: before showing `next`, the place to keep
    /// by content then (with the offset and cursor now).
    fn regroup_anchor(
        &mut self,
        next: &WorkingState,
    ) -> Option<(view_anchor::PendingAnchor, usize, usize)> {
        let regroup = matches!(self.detail, Detail::Worktree)
            && gitti_core::rows::grouped(&self.working) != gitti_core::rows::grouped(next);
        let (offset, cursor) = (self.diff_offset, self.diff_cursor);
        regroup.then(|| self.view_anchor_now()).flatten().map(|a| (a, offset, cursor))
    }

    /// After rebuilding the rows: the top line of [`Self::regroup_anchor`] back on the top
    /// row, the cursor moved with it (only header rows came or went above it).
    fn apply_regroup(&mut self, regroup: Option<(view_anchor::PendingAnchor, usize, usize)>) {
        let Some((anchor, offset, cursor)) = regroup else {
            return;
        };
        self.apply_view_anchor(anchor);
        let moved = self.diff_offset as isize - offset as isize;
        let last = self.rows.len().saturating_sub(1) as isize;
        self.diff_cursor = (cursor as isize + moved).clamp(0, last.max(0)) as usize;
    }

    fn remap_selection(&self, mut sel: LineSel, prev: &FileDiff) -> Option<LineSel> {
        let files = work_files(&self.working, sel.src);
        let file = files.iter().position(|f| f.diff.as_ref() == Some(prev))?;
        sel.file = file;
        let (h, l) = *sel.lines.first()?;
        sel.anchor_row = self.rows.iter().position(|r| {
            *r == DiffRow::Line {
                src: sel.src,
                file,
                hunk: h,
                line: l,
                part: 0,
            }
        })?;
        Some(sel)
    }

    fn selected_id(&self) -> Option<ObjectId> {
        if self.history.is_worktree_row(self.selected) {
            return None;
        }
        self.history.commits.get(self.selected).map(|c| c.id)
    }

    fn set_history(&mut self, h: History) {
        let prev = self.selected_id();
        let was_worktree =
            self.history.is_worktree_row(self.selected) || self.history.commits.is_empty();
        let first = self.history.commits.is_empty();
        self.history = Arc::new(h);
        let row = match prev {
            Some(id) => self.history.row_of(&id),
            None if was_worktree => Some(0),
            None => None,
        }
        .unwrap_or(0)
        .min(self.history.commits.len().saturating_sub(1));
        if first || row != self.selected || prev.is_none() {
            self.selected = row;
            // A commit that is still there keeps its loaded diff.
            if prev.is_none() || prev != self.selected_id() || first {
                self.load_detail();
            }
        }
        // Its refs may have changed (the history's `refs_by_commit`); sorting by date
        // reads its commit times.
        self.rewrap_header();
        self.rebuild_sidebar();
    }

    /// Selects commit list row `row` and loads its detail.
    pub fn select_row(&mut self, row: usize) {
        let n = self.history.commits.len();
        if n == 0 {
            return;
        }
        let row = row.min(n - 1);
        if row != self.selected || matches!(self.detail, Detail::Empty) {
            self.selected = row;
            self.load_detail();
        }
    }

    pub fn load_detail(&mut self) {
        // The reflog view's selected entry has the detail pane (`app/reflog.rs`).
        if self.reflog_shows_commit() {
            return;
        }
        if self.load_compare() {
            return;
        }
        let row = self.selected;
        if self.history.is_worktree_row(row) {
            self.detail_gen += 1;
            let was_worktree = matches!(self.detail, Detail::Worktree);
            self.detail = Detail::Worktree;
            self.header.clear();
            if !was_worktree {
                self.expansion.hide_all_context();
                self.diff_cursor = 0;
                self.diff_offset = 0;
                self.line_sel = None;
            }
            self.rebuild_rows();
            return;
        }
        let Some(c) = self.history.commits.get(row) else {
            self.detail = Detail::Empty;
            self.rows.clear();
            return;
        };
        let (id, stash) = (c.id, c.stash);
        let opts = self.opts.diff_options();
        if let Some(data) = self.repo.cached_commit(id, &opts) {
            // Prefetched or seen before: shown in this frame.
            self.detail_gen += 1;
            self.line_sel = None;
            self.show_commit(CommitView {
                id,
                data,
                stash,
                compare: None,
            });
            self.prefetch_around(row);
            return;
        }
        if self.detail_running {
            self.detail_pending = true;
            return;
        }
        self.detail_gen += 1;
        self.detail_running = true;
        // Commit → commit: keep showing the previous one until the new one is there (no
        // blank frame and no full repaint in between while zapping through the list).
        if !matches!(self.detail, Detail::Commit(_)) {
            self.detail = Detail::Loading;
            self.rows.clear();
        }
        self.line_sel = None;
        let generation = self.detail_gen;
        spawn(
            &self.tx,
            || std::thread::sleep(SLOW_LOAD),
            move |_| Msg::DetailSlow(generation),
        );
        let repo = self.repo.clone();
        spawn(
            &self.tx,
            move || {
                let data = repo.load_commit(id, stash, &opts)?;
                anyhow::Ok(CommitView {
                    id,
                    data,
                    stash,
                    compare: None,
                })
            },
            move |r| Msg::Detail(generation, r),
        );
    }

    /// Loads the commits next to `row` into the core's commit cache in the background: three
    /// ahead in the direction the selection moves, one behind (skips ones in flight).
    fn prefetch_around(&mut self, row: usize) {
        if self.compare.mode.is_some() {
            self.prefetch_compare_around(row);
            return;
        }
        let down = row >= self.last_shown_row;
        self.last_shown_row = row;
        let step = |n: usize| if down { row.checked_add(n) } else { row.checked_sub(n) };
        let back = if down { row.checked_sub(1) } else { row.checked_add(1) };
        let opts = self.opts.diff_options();
        let mut todo = Vec::new();
        for r in [step(1), step(2), step(3), back].into_iter().flatten() {
            if self.history.is_worktree_row(r) {
                continue;
            }
            let Some(c) = self.history.commits.get(r) else {
                continue;
            };
            if self.repo.cached_commit(c.id, &opts).is_none()
                && self.prefetching.lock().expect("prefetching").insert(c.id)
            {
                todo.push((c.id, c.stash));
            }
        }
        if todo.is_empty() {
            return;
        }
        // One thread and one `git show` for all of them (stashes each their own).
        let (repo, prefetching) = (self.repo.clone(), self.prefetching.clone());
        std::thread::spawn(move || {
            let commits: Vec<_> = todo
                .iter()
                .filter(|(_, s)| s.is_none())
                .map(|(id, _)| *id)
                .collect();
            let _ = repo.load_commits(&commits, &opts);
            for &(id, stash) in todo.iter().filter(|(_, s)| s.is_some()) {
                let _ = repo.load_commit(id, stash, &opts);
            }
            let mut prefetching = prefetching.lock().expect("prefetching");
            for (id, _) in &todo {
                prefetching.remove(id);
            }
        });
    }

    /// Header line index of the "Signature" row of the shown commit.
    pub fn signature_header_line(&self) -> Option<usize> {
        match &self.detail {
            Detail::Commit(cv) if cv.sig_kind.is_some() => Some(crate::ui::SIGNATURE_LINE),
            _ => None,
        }
    }

    fn signature_pending(&self) -> bool {
        match &self.detail {
            Detail::Commit(cv) if cv.sig_kind.is_some() => {
                self.signature.as_ref().is_none_or(|(id, _)| *id != cv.id)
            }
            _ => false,
        }
    }

    /// A commit detail load is running (the shown commit may be the previous one).
    pub fn detail_loading(&self) -> bool {
        self.detail_running || self.detail_pending
    }

    /// Details window of the shown commit's signature.
    pub fn show_signature(&mut self) {
        if let Some((_, Some(info))) = &self.signature {
            let mut text = info.details();
            match self.signers_files_known() {
                Some(n) if n > 0 => text.push_str(&format!(
                    "\nYou have an allowed signers file to check it with: m on the Signature line → {}",
                    signers::USE_LABEL.trim_end_matches('…')
                )),
                Some(_) => text.push_str(&format!(
                    "\nHow to set up an allowed signers file: m on the Signature line → {}",
                    gitti_core::allowed_signers::HELP_LABEL
                )),
                None => {}
            }
            if self.own_signers_offered().is_some() {
                text.push_str(&format!(
                    "\nAn allowed signers file with your own key: m on the Signature line → {}",
                    gitti_core::allowed_signers::OWN_LABEL.trim_end_matches('…')
                ));
            }
            if self.signing_keys_offered() {
                text.push_str(&format!(
                    "\nThe signer's keys may be on GitHub: m on the Signature line → {}",
                    gitti_core::signing_keys::ACTION_LABEL
                ));
            }
            self.modal = Some(Modal::text("Signature", text));
        }
    }

    fn show_commit(&mut self, cv: CommitView) {
        let pair = |v: &CommitView| v.compare.as_ref().map(|c| (c.old, c.new));
        let same =
            matches!(&self.detail, Detail::Commit(old) if old.id == cv.id && pair(old) == pair(&cv));
        if cv.sig_kind.is_some() && self.signature.as_ref().is_none_or(|(id, _)| *id != cv.id) {
            self.signature = None;
            self.signers_offer = None;
            self.spawn_verify(cv.id);
        }
        let cv = Arc::new(cv);
        self.header = crate::ui::commit_header(self, &cv);
        let new_data = !matches!(&self.detail, Detail::Commit(old) if Arc::ptr_eq(&old.data, &cv.data));
        self.detail = Detail::Commit(cv);
        if !same {
            // Another commit / comparison opens as changes (no Show Whole File left).
            self.expansion.hide_all_context();
            self.diff_cursor = 0;
            self.diff_offset = 0;
            self.hscroll = 0;
        }
        self.rebuild_rows();
        self.apply_pending_file();
        self.restore_view_anchor();
        if new_data {
            // The previous commit's colours must not land on this one's lines.
            self.highlights.clear();
            self.schedule_highlight();
        }
    }

    /// "Load anyway": the diff of file `file` of the shown commit, stash or
    /// comparison, left out as too large, loaded in the background
    /// (`Msg::FileLoaded`).
    pub fn load_omitted_file(&mut self, file: usize) {
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
        let (repo, data, opts) = (self.repo.clone(), cv.data.clone(), self.opts.diff_options());
        let compare = cv.compare.clone();
        self.set_info(format!("Loading the diff of {path}…"));
        self.loads_in_flight += 1;
        spawn(
            &self.tx,
            move || match compare {
                Some(c) => repo
                    .load_omitted_compare_file(&c, file, &opts)
                    .map(|c| (c.data.clone(), Some(c))),
                None => repo.load_omitted_file(&data, file, &opts).map(|d| (d, None)),
            },
            move |r| Msg::FileLoaded(file, r),
        );
    }

    /// The shown commit (or comparison) with a file loaded: the rows above it stay, so
    /// does the view.
    fn show_loaded_file(
        &mut self,
        file: usize,
        data: Arc<CommitData>,
        compare: Option<Arc<gitti_core::compare::CompareData>>,
    ) {
        let Detail::Commit(cv) = &self.detail else {
            return;
        };
        let same = match (&cv.compare, &compare) {
            (None, None) => cv.id == data.details.id,
            (Some(a), Some(b)) => (a.old, a.new) == (b.old, b.new),
            _ => false,
        };
        if !same {
            return;
        }
        // Asked for: shown even when long (not auto-collapsed).
        if let Some(f) = data.files.get(file)
            && !self.expansion.is_expanded(Src::Commit, f)
        {
            self.expansion.toggle(Src::Commit, f.path());
        }
        let cv = Arc::new(CommitView {
            id: cv.id,
            data,
            stash: cv.stash,
            compare,
        });
        self.header = crate::ui::commit_header(self, &cv);
        self.detail = Detail::Commit(cv);
        self.rebuild_rows();
        self.schedule_highlight();
        self.set_info("Loaded");
    }

    /// Whether row `row` of the diff is a file whose diff wasn't loaded.
    pub fn not_loaded_at(&self, row: usize) -> Option<usize> {
        match self.rows.get(row) {
            Some(&DiffRow::Note { src, file }) => self
                .file_diff(src, file)
                .is_some_and(gitti_core::commit_files::loadable)
                .then_some(file),
            _ => None,
        }
    }

    /// Highlights the files of the shown diff in the background (`Msg::Highlights`).
    pub fn schedule_highlight(&mut self) {
        use gitti_core::blobs::Version;
        self.hl_gen += 1;
        self.hl_running = false;
        if !self.opts.syntax {
            self.highlights.clear();
            return;
        }
        let jobs = match &self.detail {
            Detail::Worktree => worktree_jobs(&self.working, None),
            Detail::Commit(cv) if cv.compare.is_some() => {
                let c = cv.compare.as_ref().expect("compare");
                let version = |side| match side {
                    gitti_core::compare::CompareSide::Commit(id) => Version::Rev(id.to_string()),
                    gitti_core::compare::CompareSide::Worktree => Version::Worktree,
                };
                let mut jobs = Vec::new();
                for d in c.files() {
                    push_job(&mut jobs, Src::Commit, d, Some(version(c.old)), Some(version(c.new)));
                }
                jobs
            }
            Detail::Commit(cv) => {
                let mut jobs = Vec::new();
                let id = cv.details.id.to_string();
                let parent = cv.details.parents.first().map(|p| p.to_string());
                for d in &cv.files {
                    push_job(
                        &mut jobs,
                        Src::Commit,
                        d,
                        parent.clone().map(Version::Rev),
                        Some(Version::Rev(id.clone())),
                    );
                }
                jobs
            }
            _ => return,
        };
        let (repo, cache, generation) = (self.repo.clone(), self.hl_cache.clone(), self.hl_gen);
        self.hl_running = true;
        spawn(
            &self.tx,
            move || compute_highlights(&repo, jobs, &cache),
            move |map| Msg::Highlights(generation, map),
        );
    }

    /// Syntax categories of a diff line (removed lines from the old file, others the new).
    pub fn line_categories(
        &self,
        src: Src,
        path: &str,
        kind: gitti_core::diff::LineKind,
        old_no: Option<u32>,
        new_no: Option<u32>,
    ) -> Option<&LineCategories> {
        use gitti_core::diff::LineKind;
        let hl = self.highlights.get(&(src, path.to_string()))?;
        let (side, no) = match kind {
            LineKind::Removed => (hl.old.as_ref()?, old_no?),
            _ => (hl.new.as_ref()?, new_no?),
        };
        side.get(no.checked_sub(1)? as usize)
    }

    /// Reloads diffs after a display option changed: the shown commit, and the working
    /// state only (nothing else depends on the options).
    pub fn reload_diffs(&mut self) {
        if matches!(self.detail, Detail::Commit(_) | Detail::Error(_)) {
            self.load_detail();
        }
        self.reload_working();
    }

    fn reload_working(&mut self) {
        // Newer than any refresh or reload still running.
        self.working_gen += 1;
        let generation = self.working_gen;
        let repo = self.repo.clone();
        let opts = self.opts.diff_options();
        self.loads_in_flight += 1;
        self.watch_filter.read_started(false);
        spawn(
            &self.tx,
            move || repo.working_state_with(&opts),
            move |r| Msg::Working(generation, r),
        );
    }

    /// The commit header again: at the pane's new width ([`Self::header_cols`]), or after
    /// the refs, the current branch or the worktrees changed (a new tag, a deleted
    /// branch); when its "Refs" lines take another number of lines, the rows follow (the
    /// view kept).
    pub fn rewrap_header(&mut self) {
        let Detail::Commit(cv) = &self.detail else {
            return;
        };
        let cv = cv.clone();
        let header = crate::ui::commit_header(self, &cv);
        let resized = header.len() != self.header.len();
        self.header = header;
        if resized {
            self.rebuild_rows_keep_view();
        }
    }

    pub fn rebuild_rows(&mut self) {
        let wrap = if self.opts.wrap { self.wrap_cols } else { None };
        let notes = match self.note_cols {
            Some(cols) => gitti_core::rows::NoteWrap::Cols(cols),
            None => gitti_core::rows::NoteWrap::None,
        };
        self.commit_fs = self.shown_commit_fs();
        self.folders = Default::default();
        self.rows = match &self.detail {
            Detail::Worktree => {
                self.worktree_totals = gitti_core::rows::WorktreeTotals::of(&self.working);
                self.marks.retain(&self.working);
                let (rows, folders) = gitti_core::rows::build_worktree_view(
                    &self.working,
                    None,
                    &self.expansion,
                    wrap,
                    &notes,
                    self.changes_tree,
                );
                self.folders = folders;
                rows
            }
            Detail::Commit(cv) => {
                let mut rows = vec![DiffRow::HeaderSpace; self.header.len()];
                rows.extend(self.change_search_rows(
                    cv,
                    gitti_core::rows::build_commit_rows_noted(
                        &cv.files,
                        None,
                        &self.expansion,
                        wrap,
                        Some(&self.commit_fs),
                        &notes,
                    ),
                ));
                rows
            }
            _ => Vec::new(),
        };
        self.diff_cursor = self.diff_cursor.min(self.rows.len().saturating_sub(1));
    }

    pub fn rebuild_sidebar(&mut self) {
        self.update_branch_order();
        self.sidebar = sidebar::build(self);
        self.side_cursor = self.side_cursor.min(self.sidebar.len().saturating_sub(1));
    }

    // ------------------------------------------------------------------ helpers

    pub fn file_diff(&self, src: Src, file: usize) -> Option<&FileDiff> {
        match src {
            Src::Commit => match &self.detail {
                Detail::Commit(cv) => cv.files.get(file),
                _ => None,
            },
            _ => work_files(&self.working, src).get(file)?.diff.as_ref(),
        }
    }

    pub fn is_change(&self, src: Src, file: usize, hunk: usize, line: usize) -> bool {
        self.file_diff(src, file)
            .and_then(|f| f.hunks.get(hunk))
            .and_then(|h| h.lines.get(line))
            .is_some_and(|l| l.is_change())
    }

    pub fn current_branch(&self) -> Option<&str> {
        self.head.branch.as_deref()
    }

    pub fn tracking_of(&self, branch: &str) -> Option<&Tracking> {
        self.tracking.iter().find(|t| t.branch == branch)
    }

    /// Row of the commit a ref (full name) points at.
    /// The commit ref `short` of `kind` points at.
    pub fn ref_target(&self, kind: RefKind, short: &str) -> Option<ObjectId> {
        self.refs
            .iter()
            .find(|r| r.kind == kind && r.short_name == short)
            .map(|r| r.target)
    }

    pub fn row_of_ref(&self, kind: RefKind, short: &str) -> Option<usize> {
        let r = self
            .refs
            .iter()
            .find(|r| r.kind == kind && r.short_name == short)?;
        self.history.row_of(&r.target)
    }

    pub fn row_of_stash(&self, index: usize) -> Option<usize> {
        self.history
            .commits
            .iter()
            .position(|c| c.stash == Some(index))
    }

    // ------------------------------------------------------------------ operations

    /// Runs `op` on a background thread; the result is reported and the state refreshed.
    pub fn run_op(
        &mut self,
        label: &str,
        full: bool,
        after: After,
        op: impl FnOnce(&Repo) -> Result<String> + Send + 'static,
    ) {
        if self.busy.is_some() {
            // A remote op waits for the running one (and then clears its warning
            // like any operation); others are refused, the attempt still counts as the
            // user's next operation.
            if gitti_core::error_mark::waits_while_busy(label) && self.deferred.len() < 8 {
                let label = label.to_string();
                self.deferred
                    .push_back(Box::new(move |app: &mut App| app.run_op(&label, full, after, op)));
                return;
            }
            self.refuse_busy();
            return;
        }
        self.set_busy(label);
        let repo = self.repo.clone();
        let label = label.to_string();
        let cancel = gitti_core::process::CancelToken::new();
        self.op_cancel = Some(cancel.clone());
        let tx = self.tx.clone();
        let sink: gitti_core::process::ProgressSink = std::sync::Arc::new(move |line: &str| {
            let _ = tx.send(Msg::Progress(line.to_string()));
        });
        spawn(
            &self.tx,
            move || {
                let result = gitti_core::process::with_cancel(&cancel, || {
                    gitti_core::process::with_progress(sink, || op(&repo))
                });
                let stale = (result.as_ref().err())
                    .and_then(|e| gitti_core::lock::stale_lock(e, repo.workdir()));
                (result, stale)
            },
            move |(result, stale)| Msg::OpDone {
                label,
                result,
                full,
                after,
                stale,
            },
        );
    }

    /// Marks an operation as running; the label shows after [`BUSY_SHOW_DELAY`]. Every
    /// user-triggered operation comes here (`run_op`, `work_op`): earlier warnings — the
    /// top line's "⚠ … failed" and an error in the status line — go (`e` / Last
    /// Error Details still shows them; background refreshes don't count).
    fn set_busy(&mut self, label: &str) {
        self.dismiss_warnings();
        self.busy = Some((format!("{label}…"), Instant::now()));
        self.op_progress.clear();
        self.op_output = None;
        spawn(
            &self.tx,
            || std::thread::sleep(BUSY_SHOW_DELAY + std::time::Duration::from_millis(5)),
            |_| Msg::Tick,
        );
    }

    /// An operation the user started while another one runs, refused: earlier warnings
    /// go all the same, the status says why nothing happened.
    fn refuse_busy(&mut self) {
        self.dismiss_warnings();
        if let Some((b, _)) = &self.busy {
            self.set_error(format!("Busy: {b}"));
        }
    }

    /// The user starts an operation: the top line's mark and an error in the status line
    /// go (their details stay for `e`).
    pub(crate) fn dismiss_warnings(&mut self) {
        let status_error = self
            .status
            .as_ref()
            .filter(|s| s.error)
            .map(|s| s.full.clone().unwrap_or_else(|| s.text.clone()));
        self.errors.dismiss(status_error.as_deref());
        if status_error.is_some() {
            self.status = None;
        }
    }

    /// The busy label, once the operation has run long enough to show it, with its live
    /// output: `Fetch… Receiving objects 45% · 1.2 MiB — From ../remote`.
    pub fn busy_label(&self) -> Option<String> {
        let (label, _) = self
            .busy
            .as_ref()
            .filter(|(_, since)| since.elapsed() >= BUSY_SHOW_DELAY)?;
        Some(match self.op_progress.summary() {
            Some(s) => format!("{label} {s}"),
            None => label.clone(),
        })
    }

    /// While an operation runs, queues a key action for when it is done (`true`).
    pub fn defer_while_busy(&mut self, action: impl FnOnce(&mut App) + Send + 'static) -> bool {
        if self.busy.is_none() {
            return false;
        }
        // Key repeat must not pile up minutes of work.
        if self.deferred.len() < 8 {
            self.deferred.push_back(Box::new(action));
        }
        true
    }

    fn run_deferred(&mut self) {
        if self.busy.is_none()
            && let Some(change) = self.watch_pending.take()
        {
            self.on_watch(change);
        }
        if self.busy.is_none()
            && let Some(next) = self.deferred.pop_front()
        {
            next(self);
        }
    }

    /// A stage / unstage / discard: runs `op`, then re-reads the working state of `paths`
    /// only (`None`: all of it, but nothing else) and highlights those files — in one
    /// background job, so the result shows a few ms later in one go, without the full
    /// refresh (refs, remotes, stashes, config, …) other operations do.
    pub fn work_op(
        &mut self,
        label: &str,
        paths: Option<Vec<String>>,
        op: impl FnOnce(&Repo) -> Result<()> + Send + 'static,
    ) {
        self.work_op_noted(label, paths, move |r| op(r).map(|()| None));
    }

    /// [`Self::work_op`] whose `op` may say something besides success (`Some(note)`: the
    /// status line shows it, e.g. what a discard kept). Runs with a progress sink (the
    /// status line shows what a long op reports, e.g. the meter of deleting 10 000
    /// untracked files) and can be cancelled like any operation.
    pub fn work_op_noted(
        &mut self,
        label: &str,
        paths: Option<Vec<String>>,
        op: impl FnOnce(&Repo) -> Result<Option<String>> + Send + 'static,
    ) {
        if self.busy.is_some() {
            self.refuse_busy();
            return;
        }
        self.set_busy(label);
        self.working_gen += 1;
        let repo = self.repo.clone();
        let label = label.to_string();
        let prev = self.working.clone();
        let opts = self.opts.diff_options();
        self.work_opts = opts;
        let cache = self.opts.syntax.then(|| self.hl_cache.clone());
        let cancel = gitti_core::process::CancelToken::new();
        self.op_cancel = Some(cancel.clone());
        let tx = self.tx.clone();
        let sink: gitti_core::process::ProgressSink = std::sync::Arc::new(move |line: &str| {
            let _ = tx.send(Msg::Progress(line.to_string()));
        });
        spawn(
            &self.tx,
            move || {
                let result = gitti_core::process::with_cancel(&cancel, || {
                    gitti_core::process::with_progress(sink, || op(&repo))
                });
                let stale = (result.as_ref().err())
                    .and_then(|e| gitti_core::lock::stale_lock(e, repo.workdir()));
                let index = repo.index_stamp();
                let read_at = std::time::SystemTime::now();
                let working = match &paths {
                    Some(p) => repo.working_state_after(&opts, &prev, p),
                    None => repo.working_state_with(&opts),
                };
                let highlights = match (&working, &paths, cache) {
                    (Ok(w), Some(p), Some(cache)) => {
                        let only: HashSet<&str> = p.iter().map(String::as_str).collect();
                        Some(compute_highlights(&repo, worktree_jobs(w, Some(&only)), &cache))
                    }
                    _ => None,
                };
                (label, result, working, highlights, paths, (index, stale), read_at)
            },
            |(label, result, working, highlights, paths, (index, stale), read_at)| Msg::WorkDone {
                label,
                result,
                working,
                highlights,
                paths,
                index,
                stale,
                read_at,
            },
        );
    }

    /// The working-directory file the diff cursor is in: (section, path).
    fn cursor_file(&self) -> Option<(Src, String)> {
        if !matches!(self.detail, Detail::Worktree) {
            return None;
        }
        let (src, file) = self.rows.get(self.diff_cursor)?.file_ref()?;
        Some((src, work_files(&self.working, src).get(file)?.path().to_string()))
    }

    /// After a working-state change (`prev` = the state before, `at` = the cursor's file
    /// then): when that file left its section as a whole (staged, unstaged, discarded), the
    /// cursor goes to the next file of the section — else the previous one, else the top —
    /// shown at the top, instead of whatever row now has the old index.
    fn follow_cursor_file(&mut self, prev: &WorkingState, at: Option<(Src, String)>) {
        let Some((src, path)) = at else {
            return;
        };
        if gitti_core::rows::has_work_file(&self.working, src, &path) {
            return;
        }
        let row = gitti_core::rows::neighbour_file_row(&self.rows, prev, &self.working, src, &path)
            .unwrap_or(0);
        self.diff_cursor = row;
        self.diff_offset = row;
    }

    /// Shows a working state read by [`App::work_op`]; `highlights` are those of `paths`.
    fn apply_working(
        &mut self,
        working: WorkingState,
        highlights: Option<HashMap<(Src, String), FileHighlight>>,
        paths: Option<Vec<String>>,
    ) {
        let prev_sel_diff = self.line_sel.as_ref().and_then(|sel| {
            work_files(&self.working, sel.src)
                .get(sel.file)
                .and_then(|f| f.diff.clone())
        });
        let at = self.cursor_file();
        let regroup = self.regroup_anchor(&working);
        let prev = std::mem::replace(&mut self.working, Arc::new(working));
        self.publish_marks();
        if self.working.conflicted.is_empty() {
            self.conflict_sides = None;
        }
        self.rebuild_sidebar();
        self.refresh_worktree_compare();
        if !matches!(self.detail, Detail::Worktree) {
            return;
        }
        self.rebuild_rows();
        self.apply_regroup(regroup);
        self.follow_cursor_file(&prev, at);
        self.restore_view_anchor();
        let sel = self.line_sel.take();
        self.line_sel = sel
            .zip(prev_sel_diff)
            .and_then(|(sel, prev)| self.remap_selection(sel, &prev));
        match (highlights, paths) {
            (Some(hl), Some(paths)) => {
                // Drop a running full highlight: it was computed from the old state.
                self.hl_gen += 1;
                self.hl_running = false;
                self.highlights.retain(|(_, p), _| !paths.contains(p));
                self.highlights.extend(hl);
            }
            _ => self.schedule_highlight(),
        }
    }

    /// A quick local git write that changes the files at `paths` (`None`: maybe all).
    pub fn git_op(
        &mut self,
        label: &str,
        paths: Option<Vec<String>>,
        op: impl FnOnce(&Repo) -> Result<()> + Send + 'static,
    ) {
        self.work_op(label, paths, op);
    }

    pub fn fetch(&mut self) {
        self.run_op("Fetch", true, After::Nothing, |r| r.fetch());
    }

    /// The plain Pull (`p`, palette, the current branch's menu): without an upstream (or
    /// with a detached HEAD) no git runs at all — "Pull from…" opens instead
    /// (before, `git pull` fetched first, asking for a passphrase, and failed only then).
    /// Decided from the last refresh; `Repo::pull` checks once more if that is stale.
    pub fn pull(&mut self) {
        let has_upstream = self
            .current_branch()
            .and_then(|b| self.tracking_of(b))
            .is_some_and(|t| t.upstream.is_some());
        match pull_block(self.current_branch(), has_upstream) {
            None => self.run_op("Pull", true, After::Nothing, |r| r.pull_saved()),
            Some(block) => self.pull_blocked(block),
        }
    }

    /// The plain Pull can't run: says why; without an upstream "Pull from…" opens.
    pub(crate) fn pull_blocked(&mut self, block: PullBlock) {
        // A user operation all the same: earlier warnings go.
        self.dismiss_warnings();
        match block {
            PullBlock::Detached => self.set_error(block.to_string()),
            PullBlock::NoUpstream { .. } => {
                self.set_info(block.to_string());
                self.pull_step();
            }
        }
    }

    /// The branches diverged and no pull strategy is configured or remembered:
    /// Merge / Rebase / Fast-forward only, each once or remembered for this repository
    /// (upper-case key; gitti's settings, not the git config), then the pull again.
    pub(crate) fn pull_diverged(&mut self, d: DivergentPull) {
        self.dismiss_warnings();
        self.set_info(d.to_string());
        let mut items = Vec::new();
        for remember in [false, true] {
            if remember {
                items.push(MenuItem::separator());
            }
            for strategy in PullStrategy::ALL {
                let key = strategy_key(strategy);
                let label = if remember {
                    format!("{}, remember for this repository", strategy.label())
                } else {
                    format!("{} ({})", strategy.label(), strategy.flag())
                };
                let d = d.clone();
                let item = MenuItem::new(label, move |app: &mut App| {
                    app.run_op("Pull", true, After::Nothing, move |r| {
                        if remember {
                            gitti_core::pull::save_strategy(r.workdir(), strategy);
                        }
                        d.retry(r, strategy)
                    })
                });
                items.push(item.key(if remember { key.to_ascii_uppercase() } else { key }));
            }
        }
        self.modal = Some(Modal::menu(
            format!("{} and {} diverged — pull how?", d.branch, d.upstream),
            items,
        ));
    }

    /// "Pull with…" (palette): the plain Pull with a strategy chosen for it — once
    /// (n / r / f) or remembered for this repository (N / R / F) — or the remembered one
    /// forgotten (x; the next divergent pull asks again). Without an upstream as the
    /// plain Pull.
    pub fn pull_with_menu(&mut self) {
        let has_upstream = self
            .current_branch()
            .and_then(|b| self.tracking_of(b))
            .is_some_and(|t| t.upstream.is_some());
        if let Some(block) = pull_block(self.current_branch(), has_upstream) {
            self.pull_blocked(block);
            return;
        }
        let saved = gitti_core::pull::saved_strategy(self.repo.workdir());
        let mut items = Vec::new();
        for remember in [false, true] {
            if remember {
                items.push(MenuItem::separator());
            }
            for strategy in PullStrategy::ALL {
                let label = if remember {
                    format!("{}, remember for this repository", strategy.label())
                } else {
                    format!("{} ({})", strategy.label(), strategy.flag())
                };
                let item = MenuItem::new(label, move |app: &mut App| {
                    app.run_op("Pull", true, After::Nothing, move |r| {
                        if remember {
                            gitti_core::pull::save_strategy(r.workdir(), strategy);
                        }
                        r.pull_chosen(strategy)
                    })
                });
                let key = strategy_key(strategy);
                items.push(item.key(if remember { key.to_ascii_uppercase() } else { key }));
            }
        }
        if let Some(s) = saved {
            items.push(MenuItem::separator());
            let workdir = self.repo.workdir().to_path_buf();
            items.push(
                MenuItem::new(
                    format!("Forget the remembered strategy ({})", s.label()),
                    move |app: &mut App| {
                        gitti_core::pull::forget_strategy(&workdir);
                        app.set_info("Pull strategy forgotten — the next divergent pull asks again");
                    },
                )
                .key('x'),
            );
        }
        self.modal = Some(Modal::menu(gitti_core::pull::pull_with_title(saved), items));
    }

    /// The main push (`P`, palette, the current branch's menu): the current branch to its
    /// upstream (`Repo::push_current`, shared with gitti). Without one no git runs — "Push
    /// to…" opens with the remotes (the chosen one becomes the upstream) and Set
    /// Upstream… (before, it pushed `-u` to the default remote, asking for a
    /// passphrase before the user chose anything). Decided from the last refresh, as
    /// Pull; `Repo::push_current` checks once more if that is stale.
    pub fn push(&mut self) {
        let tracking = self.current_branch().and_then(|b| self.tracking_of(b));
        let has_upstream = tracking.is_some_and(|t| t.upstream.is_some());
        // Behind its upstream (amended / rebased after the push, or someone pushed): git
        // would refuse — the ways out right away, read locally (no network).
        let behind = has_upstream && tracking.is_some_and(|t| t.behind > 0);
        match push_block(self.current_branch(), has_upstream, !self.remotes.is_empty()) {
            None if behind => self.run_op("Push", true, After::Summary, |r| r.push_diverged()),
            None => self.run_push_op("Push", After::Summary, false, |r| r.push_current()),
            Some(block) => self.push_blocked(block),
        }
    }

    /// The main push can't run: says why; without an upstream "Push to…" opens.
    pub(crate) fn push_blocked(&mut self, block: PushBlock) {
        // A user operation all the same: earlier warnings go.
        self.dismiss_warnings();
        match block {
            PushBlock::Detached | PushBlock::NoRemote => self.set_error(block.to_string()),
            PushBlock::NoUpstream { .. } => {
                self.set_info(block.to_string());
                self.push_step();
            }
        }
    }

    /// Details of the failed operations (clears the top line's mark), else of the error
    /// in the status line, else the ones shown or dismissed last. `false` when
    /// there is none.
    pub fn show_error_details(&mut self) -> bool {
        if let Some((title, body)) = self.errors.take_details() {
            self.modal = Some(Modal::text(title, body));
            return true;
        }
        if let Some(full) = self.status.as_ref().and_then(|s| s.full.clone()) {
            self.modal = Some(Modal::text("Error", full));
            return true;
        }
        if let Some((title, body)) = self.errors.last_details() {
            self.modal = Some(Modal::text(title, body));
            return true;
        }
        false
    }

    pub fn toggle_amend(&mut self) {
        // What was typed before is the draft (or the amend draft), not HEAD's message
        // that may come now.
        self.save_draft_now();
        self.amend = !self.amend;
        if self.amend && self.message.is_empty() {
            // The amend message edited before for this commit, else HEAD's.
            match self.amend_draft_for_head() {
                Some(m) => self.message.set_text(&m),
                None => match self.repo.head_message() {
                    Ok(m) => self.message.set_text(m.trim_end()),
                    Err(e) => self.set_error(format!("{e:#}")),
                },
            }
        } else if !self.amend
            && self.repo.head_message().ok().as_deref().map(str::trim_end)
                == Some(self.message.text().as_str())
        {
            self.message = TextEdit::multi();
        }
        if !self.amend {
            // An empty box starts again with git's message or the commit template.
            self.sync_box_start();
        }
    }

    pub fn stage_all(&mut self) {
        if self.defer_while_busy(|app| app.stage_all()) {
            return;
        }
        // Paths this file system can't hold: asks first (`app/fs_paths.rs`).
        let paths: Vec<String> = self.working.unstaged.iter().map(|f| f.path().to_string()).collect();
        self.guard_fs_stage(paths, |app, only| match only {
            None => app.git_op("Stage all", None, |r| r.stage_all()),
            Some(list) => {
                let paths = list.clone();
                app.git_op("Stage all", Some(paths), move |r| r.stage_path_list(&list))
            }
        });
    }

    pub fn unstage_all(&mut self) {
        if self.defer_while_busy(|app| app.unstage_all()) {
            return;
        }
        self.git_op("Unstage all", None, |r| r.unstage_all());
    }

    pub fn discard_all(&mut self) {
        self.discard_unstaged(None)
    }

    /// [`Self::discard_all`] for one group of the unstaged section (`None` = all of it).
    pub fn discard_unstaged(&mut self, group: Option<gitti_core::rows::Group>) {
        let entries: Vec<_> = self
            .working
            .unstaged
            .iter()
            .filter(|f| group.is_none_or(|g| gitti_core::rows::Group::of(f) == g))
            .filter(|f| !f.entry.conflicted)
            .map(|f| f.entry.clone())
            .collect();
        if entries.is_empty() {
            return;
        }
        let n = entries.len();
        let (title, question, done) = match group {
            None => (
                "Discard All",
                format!("Discard all unstaged changes in {n} files?"),
                "All unstaged changes discarded",
            ),
            Some(gitti_core::rows::Group::Tracked) => (
                "Discard Tracked",
                format!("Discard the changes of {n} tracked files?"),
                "Changes of tracked files discarded",
            ),
            Some(gitti_core::rows::Group::Untracked) => (
                "Delete Untracked",
                format!("Delete {n} untracked files?"),
                "Untracked files deleted",
            ),
        };
        self.discard_entries(entries, "Discard all", title, question, done);
    }

    /// The confirmation of discarding the unstaged changes of `entries` (several files: a
    /// group, a folder, marked files): `question`, what happens to each kind of file, then
    /// one discard; `done` starts the status line's report.
    pub(crate) fn discard_entries(
        &mut self,
        entries: Vec<gitti_core::status::StatusEntry>,
        label: &'static str,
        title: &'static str,
        question: String,
        done: &'static str,
    ) {
        let plan = DiscardPlan::of(self.repo.workdir(), &entries);
        let fs_warning = self.fs_discard_warning(entries.iter().map(|e| e.path.as_str()));
        let run = move |app: &mut App, opts: DiscardOptions| {
            let paths = Some(entries.iter().map(|e| e.path.clone()).collect());
            app.work_op_noted(label, paths, move |r| {
                let report = r.discard_with(&entries.iter().collect::<Vec<_>>(), opts)?;
                Ok(Some(gitti_core::discard::done_note(done, &report)))
            })
        };
        let mut body = format!("{question}\n{}", plan.detail());
        if let Some(w) = fs_warning {
            body = format!("{body}\n{w}");
        }
        if plan.nested.is_empty() {
            self.modal = Some(Modal::confirm(title, body, "Discard", move |app| {
                run(app, DiscardOptions::default())
            }));
            return;
        }
        // Nested repositories: kept, unless the second entry is chosen.
        let run = std::rc::Rc::new(std::cell::RefCell::new(Some(run)));
        let again = run.clone();
        let mut items: Vec<MenuItem> = body
            .lines()
            .flat_map(|l| MenuItem::notes(l, DISCARD_NOTE_WIDTH))
            .collect();
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("Discard, keep the nested repositories", move |app: &mut App| {
                if let Some(run) = run.borrow_mut().take() {
                    run(app, DiscardOptions::default())
                }
            })
            .key('d'),
        );
        items.push(
            MenuItem::new(plan.nested_check().replacen("Also delete", "Discard and delete", 1), move |app: &mut App| {
                if let Some(run) = again.borrow_mut().take() {
                    run(app, DiscardOptions { delete_nested_repos: true })
                }
            })
            .key('x'),
        );
        self.modal = Some(Modal::menu(title, items));
    }

    /// Stage / unstage / discard, driven by the line selection or the diff cursor.
    pub fn patch_action(&mut self, mode: PatchMode) {
        if !matches!(self.detail, Detail::Worktree) {
            return;
        }
        if self.defer_while_busy(move |app| app.patch_action(mode)) {
            return;
        }
        let want = match mode {
            PatchMode::Unstage => Src::Staged,
            _ => Src::Unstaged,
        };
        if let Some(sel) = self.line_sel.clone()
            && sel.src == want
            && !sel.lines.is_empty()
        {
            self.apply_selection(sel.src, sel.file, sel.to_selection(), mode);
            return;
        }
        // Marked files: the action is theirs.
        if !self.marks.is_empty() {
            return self.marks_action(mode);
        }
        let row = self.rows.get(self.diff_cursor).copied();
        match row {
            Some(DiffRow::Folder { .. }) => self.folder_action(mode),
            Some(DiffRow::Group(group)) if want == Src::Unstaged => match mode {
                PatchMode::Discard => self.discard_unstaged(Some(group)),
                _ => self.stage_group(group),
            },
            Some(DiffRow::Section(src)) if src == want => match mode {
                PatchMode::Stage => self.stage_all(),
                PatchMode::Unstage => self.unstage_all(),
                PatchMode::Discard => self.discard_all(),
            },
            Some(DiffRow::Section(Src::Conflicted))
            | Some(DiffRow::File {
                src: Src::Conflicted,
                ..
            }) if mode == PatchMode::Stage => {
                if let Some(DiffRow::File { file, .. }) = row {
                    self.resolve_menu(file);
                }
            }
            // A submodule pointer change is staged / unstaged / discarded as a whole.
            Some(
                DiffRow::File { src, file }
                | DiffRow::Note { src, file }
                | DiffRow::PathNote { src, file, .. }
                | DiffRow::Submodule { src, file, .. },
            ) if src == want => self.file_action(src, file, mode),
            Some(
                DiffRow::Hunk { src, file, hunk }
                | DiffRow::Line {
                    src, file, hunk, ..
                },
            ) if src == want => self.apply_selection(src, file, Selection::whole_hunk(hunk), mode),
            _ => self.set_info(match mode {
                PatchMode::Stage => "Move the cursor to an unstaged file, hunk or line to stage it",
                PatchMode::Unstage => {
                    "Move the cursor to a staged file, hunk or line to unstage it"
                }
                PatchMode::Discard => {
                    "Move the cursor to an unstaged file, hunk or line to discard it"
                }
            }),
        }
    }

    pub fn file_action(&mut self, src: Src, file: usize, mode: PatchMode) {
        let Some(wf) = work_files(&self.working, src).get(file) else {
            return;
        };
        let entry = wf.entry.clone();
        let path = entry.path.clone();
        match mode {
            PatchMode::Stage => self.guard_fs_stage(vec![path.clone()], move |app, _| {
                app.git_op("Stage file", Some(vec![path.clone()]), move |r| {
                    r.stage_paths(&[&path])
                })
            }),
            PatchMode::Unstage => {
                self.git_op("Unstage file", Some(vec![path.clone()]), move |r| {
                    r.unstage_paths(&[&path])
                })
            }
            PatchMode::Discard => {
                // A folder that is a repository of its own: Discard would keep it (as
                // `git clean` does); deleting it is asked for in plain words.
                let nested = gitti_core::discard::is_nested_repo(self.repo.workdir(), &entry);
                let (title, what, button) = if nested {
                    (
                        "Delete Repository Folder",
                        format!(
                            "{path} is a separate git repository (it has its own .git). \
                             Delete it? That deletes its files, its history and everything \
                             not pushed from it. This cannot be undone."
                        ),
                        "Delete Repository",
                    )
                } else if entry.untracked {
                    ("Discard File", format!("Delete the untracked file {path}?"), "Discard")
                } else {
                    let mut what = format!("Discard all unstaged changes in {path}?");
                    // A path this file system can't hold: what Discard can't do.
                    if let Some(w) = self.fs_discard_warning([path.as_str()]) {
                        what = format!("{what}\n{w}");
                    }
                    ("Discard File", what, "Discard")
                };
                let opts = DiscardOptions {
                    delete_nested_repos: nested,
                };
                self.modal = Some(Modal::confirm(title, what, button, move |app| {
                    let paths = Some(vec![entry.path.clone()]);
                    app.work_op_noted("Discard file", paths, move |r| {
                        Ok(r.discard_with(&[&entry], opts)?.note())
                    })
                }));
            }
        }
    }

    fn apply_selection(&mut self, src: Src, file: usize, sel: Selection, mode: PatchMode) {
        // An LFS file: its index entry is the pointer — whole file only.
        if self.refuse_lfs(src, file, mode) || self.refuse_fs_path(src, file, mode) {
            return;
        }
        let Some(shown) = self.file_diff(src, file) else {
            return;
        };
        // A file name that is not UTF-8: the patch would name another file.
        if let Err(e) = gitti_core::patch::check_utf8(shown, mode) {
            self.set_error(e.to_string());
            return;
        }
        // With "ignore whitespace" the shown diff (`-w`) cannot be applied: transfer the
        // selection onto the exact diff.
        let exact = work_files(&self.working, src)
            .get(file)
            .and_then(|w| w.patch_diff.as_ref());
        let (diff, sel) = match exact {
            Some(real) => match gitti_core::patch::map_selection(shown, &sel, real) {
                Some(mapped) => (real.clone(), mapped),
                None => {
                    self.set_error(
                        "Could not match the selection to the exact diff — turn off ignore whitespace (W)"
                            .into(),
                    );
                    return;
                }
            },
            None => (shown.clone(), sel),
        };
        let (label, confirm) = match mode {
            PatchMode::Stage => ("Stage", false),
            PatchMode::Unstage => ("Unstage", false),
            PatchMode::Discard => ("Discard", true),
        };
        let run = move |app: &mut App| {
            app.line_sel = None;
            let mut paths = vec![diff.path().to_string()];
            paths.extend(diff.old_path.clone().filter(|p| *p != paths[0]));
            app.git_op(label, Some(paths), move |r| r.apply_selection(&diff, &sel, mode));
        };
        if confirm {
            self.modal = Some(Modal::confirm(
                "Discard Lines",
                "Discard the selected changes from the working directory?\nThis cannot be undone.",
                "Discard",
                run,
            ));
        } else {
            run(self);
        }
    }

    pub fn resolve_menu(&mut self, file: usize) {
        if let Some((title, items)) = self.resolve_menu_spec(file) {
            self.modal = Some(Modal::menu(title, items));
        }
    }

    /// The sides' names of the current conflicts ("ours" / "theirs" until known).
    pub fn conflict_sides_or_default(&self) -> ConflictSides {
        self.conflict_sides.clone().unwrap_or_else(|| ConflictSides {
            ours: "ours".into(),
            theirs: "theirs".into(),
        })
    }

    /// The menu of conflicted file `file` (`None`: no such file): its kind's explanation and
    /// choices ([`gitti_core::conflict::ConflictKind::choices`]) — merge editor / ours /
    /// theirs for a content conflict, keep / delete for a file one side deleted.
    fn resolve_menu_spec(&self, file: usize) -> Option<(String, Vec<MenuItem>)> {
        use gitti_core::conflict::{ConflictAction as A, ConflictKind};
        let wf = self.working.conflicted.get(file)?;
        let path = wf.entry.path.clone();
        let kind = ConflictKind::of(&wf.entry).unwrap_or(ConflictKind::BothModified);
        let sides = self.conflict_sides_or_default();
        let mut items = Vec::new();
        if !kind.is_content() {
            items.extend(MenuItem::notes(&kind.explain(&sides), 60));
            items.push(MenuItem::separator());
        }
        for c in kind.choices(&sides) {
            let p = path.clone();
            let item = match c.action {
                A::MergeEditor => {
                    MenuItem::new("Open in Merge Editor", move |app| app.open_merge_editor(p))
                        .key('e')
                }
                A::MergeTool => MenuItem::new("Open in External Merge Tool", move |app| {
                    app.open_external_merge_tool(p)
                })
                .key('x'),
                A::MarkResolved => {
                    let shown = path.clone();
                    MenuItem::new("Mark Resolved", move |app| {
                    let mark = move |app: &mut App| {
                        app.run_op("Mark resolved", false, After::Nothing, move |r| {
                            r.mark_resolved(&[p])
                        })
                    };
                    let path = shown.clone();
                    if app.repo.has_conflict_markers(&path) {
                        app.modal = Some(Modal::confirm(
                            "Mark Resolved",
                            format!(
                                "{path} still contains conflict markers. Mark it resolved anyway?"
                            ),
                            "Mark Resolved",
                            mark,
                        ));
                    } else {
                        mark(app);
                    }
                    })
                    .key('r')
                }
                action => {
                    let (label, key) = match action {
                        A::TakeOurs => ("Take ours", 'o'),
                        A::TakeTheirs => ("Take theirs", 't'),
                        A::Keep => ("Keep conflicted file", 'k'),
                        A::Delete => ("Delete conflicted file", 'd'),
                        _ => ("Restore base version", 'b'),
                    };
                    MenuItem::new(c.label, move |app| {
                        app.run_op(label, false, After::Nothing, move |r| {
                            r.resolve_conflict(&[p], action)
                        })
                    })
                    .key(key)
                }
            };
            items.push(item);
        }
        Some((format!("Resolve {path}"), items))
    }

    // ------------------------------------------------------------------ line selection

    /// Toggles the change line under the diff cursor.
    pub fn toggle_line(&mut self) {
        let Some(DiffRow::Line {
            src,
            file,
            hunk,
            line,
            ..
        }) = self.rows.get(self.diff_cursor).copied()
        else {
            return;
        };
        if !matches!(src, Src::Unstaged | Src::Staged) || !self.is_change(src, file, hunk, line) {
            return;
        }
        let row = self.diff_cursor;
        let sel = match &mut self.line_sel {
            Some(sel) if sel.src == src && sel.file == file => sel,
            _ => self.line_sel.insert(LineSel {
                src,
                file,
                anchor_row: row,
                lines: Default::default(),
            }),
        };
        if !sel.lines.remove(&(hunk, line)) {
            sel.lines.insert((hunk, line));
        }
        sel.anchor_row = row;
        if sel.lines.is_empty() {
            self.line_sel = None;
        }
    }

    /// Selects every change line of the hunk under the cursor (or clears it if all are
    /// selected already).
    pub fn toggle_hunk(&mut self) {
        let Some(
            DiffRow::Hunk { src, file, hunk }
            | DiffRow::Line {
                src, file, hunk, ..
            },
        ) = self.rows.get(self.diff_cursor).copied()
        else {
            return;
        };
        if !matches!(src, Src::Unstaged | Src::Staged) {
            return;
        }
        let Some(h) = self.file_diff(src, file).and_then(|f| f.hunks.get(hunk)) else {
            return;
        };
        let changes: Vec<(usize, usize)> = h
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_change())
            .map(|(i, _)| (hunk, i))
            .collect();
        let row = self.diff_cursor;
        let sel = match &mut self.line_sel {
            Some(sel) if sel.src == src && sel.file == file => sel,
            _ => self.line_sel.insert(LineSel {
                src,
                file,
                anchor_row: row,
                lines: Default::default(),
            }),
        };
        if changes.iter().all(|c| sel.lines.contains(c)) {
            for c in &changes {
                sel.lines.remove(c);
            }
        } else {
            sel.lines.extend(changes);
        }
        if sel.lines.is_empty() {
            self.line_sel = None;
        }
    }

    /// Moves the cursor by `delta` rows, extending the line selection from its anchor.
    pub fn extend_selection(&mut self, delta: isize) {
        let from = self.diff_cursor;
        let Some(DiffRow::Line { src, file, .. }) = self.rows.get(from).copied() else {
            self.move_diff_cursor(delta);
            return;
        };
        if !matches!(src, Src::Unstaged | Src::Staged) {
            self.move_diff_cursor(delta);
            return;
        }
        if !self
            .line_sel
            .as_ref()
            .is_some_and(|s| s.src == src && s.file == file)
        {
            self.line_sel = Some(LineSel {
                src,
                file,
                anchor_row: from,
                lines: Default::default(),
            });
        }
        let to = (from as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize;
        // Stay inside the file.
        if !matches!(self.rows.get(to), Some(DiffRow::Line { src: s, file: f, .. } | DiffRow::Hunk { src: s, file: f, .. }) if *s == src && *f == file)
        {
            return;
        }
        self.diff_cursor = to;
        let working = self.working.clone();
        let is_change = |h: usize, l: usize| {
            work_files(&working, src)
                .get(file)
                .and_then(|f| f.diff.as_ref())
                .and_then(|d| d.hunks.get(h))
                .and_then(|hk| hk.lines.get(l))
                .is_some_and(|l| l.is_change())
        };
        if let Some(sel) = self.line_sel.as_mut() {
            sel.select_range(&self.rows, to, is_change);
        }
    }

    pub fn move_diff_cursor(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let max = self.rows.len() as isize - 1;
        let mut c = (self.diff_cursor as isize + delta).clamp(0, max);
        // Skip over rows that carry no information when stepping single rows.
        if delta.abs() == 1 {
            while (0..=max).contains(&c)
                && matches!(self.rows[c as usize], DiffRow::Spacer | DiffRow::HunkGap)
            {
                c += delta.signum();
            }
            c = c.clamp(0, max);
        }
        self.diff_cursor = c as usize;
    }

    /// Jumps to the next / previous row matching `pred`.
    pub fn jump_diff(&mut self, forward: bool, pred: impl Fn(&DiffRow) -> bool) {
        let n = self.rows.len();
        let found = if forward {
            (self.diff_cursor + 1..n).find(|&i| pred(&self.rows[i]))
        } else {
            (0..self.diff_cursor).rev().find(|&i| pred(&self.rows[i]))
        };
        if let Some(i) = found {
            self.diff_cursor = i;
            // Show the jumped-to header at the top.
            self.diff_offset = i;
        }
    }

    /// Collapses / expands the file under the cursor.
    pub fn toggle_file(&mut self) {
        let Some(
            DiffRow::File { src, file }
            | DiffRow::Note { src, file }
            | DiffRow::Submodule { src, file, .. }
            | DiffRow::Hunk { src, file, .. }
            | DiffRow::Line { src, file, .. },
        ) = self.rows.get(self.diff_cursor).copied()
        else {
            return;
        };
        let Some(path) = self
            .file_diff(src, file)
            .map(|f| f.path().to_string())
            .or_else(|| {
                work_files(&self.working, src)
                    .get(file)
                    .map(|w| w.path().to_string())
            })
        else {
            return;
        };
        self.expansion.toggle(src, &path);
        self.rebuild_rows();
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| *r == DiffRow::File { src, file })
        {
            self.diff_cursor = i;
        }
    }

    /// Collapses / expands the group of the unstaged section under the cursor.
    pub fn toggle_group(&mut self) {
        let Some(DiffRow::Group(group)) = self.rows.get(self.diff_cursor).copied() else {
            return;
        };
        self.expansion.toggle_group(group);
        self.rebuild_rows();
        if let Some(i) = self.rows.iter().position(|r| *r == DiffRow::Group(group)) {
            self.diff_cursor = i;
        }
    }

    /// Stages the files of a group of the unstaged section: one `git add`, those paths
    /// re-read.
    pub fn stage_group(&mut self, group: gitti_core::rows::Group) {
        if self.defer_while_busy(move |app| app.stage_group(group)) {
            return;
        }
        let paths = gitti_core::rows::group_paths(&self.working, group);
        if paths.is_empty() {
            return;
        }
        self.guard_fs_stage(paths.clone(), move |app, only| {
            let paths = only.unwrap_or(paths);
            let list = paths.clone();
            app.git_op("Stage group", Some(paths), move |r| r.stage_path_list(&list));
        });
    }

    pub fn set_all_expanded(&mut self, expanded: bool) {
        self.expansion.set_all(expanded);
        self.rebuild_rows();
    }

    // ------------------------------------------------------------------ askpass

    fn next_ask(&mut self) {
        if matches!(self.modal, Some(Modal::Ask { .. })) {
            return;
        }
        while let Some((prompt, reply)) = self.asks.pop_front() {
            if prompt.asker.is_gone() {
                let _ = reply.send(None);
                continue;
            }
            let kind = prompt.kind();
            self.modal = Some(Modal::Ask {
                secret: kind == gitti_core::askpass::PromptKind::Secret,
                input: TextEdit::single(""),
                // The prompt, which git command asked, and why it asks again.
                prompt: prompt.message(),
                asker: prompt.asker,
                reply: Some(reply),
            });
            return;
        }
    }

    /// Enter answers; Esc / ctrl-c (`None`) stops the whole operation, not only this
    /// question — ssh would move on to the next key or a password, and the next ssh
    /// connection of the operation asks again. The prompts already waiting are refused.
    pub fn answer_ask(&mut self, answer: Option<String>) {
        let Some(Modal::Ask { reply, .. }) = self.modal.take() else {
            return;
        };
        let cancelled = answer.is_none();
        if let Some(reply) = reply {
            let _ = reply.send(answer);
        }
        if cancelled {
            for (_, reply) in self.asks.drain(..) {
                let _ = reply.send(None);
            }
            self.stop_op_for_ask();
        }
        self.next_ask();
    }

    /// A helper went away (its command is over): its dialog closes without stopping
    /// anything, the queued prompts of gone helpers are dropped.
    fn drop_gone_asks(&mut self) {
        self.asks.retain(|(prompt, _)| !prompt.asker.is_gone());
        if let Some(Modal::Ask { asker, .. }) = &self.modal
            && asker.is_gone()
        {
            self.modal = None;
            self.next_ask();
        }
    }

    // ------------------------------------------------------------------ pickers & menus

    pub fn switch_branch_picker(&mut self) {
        let current = self.current_branch().map(str::to_string);
        let mut items = Vec::new();
        for r in self.refs.iter().filter(|r| r.kind == RefKind::LocalBranch) {
            let name = r.short_name.clone();
            let mut it = PickItem::new(name.clone(), "", move |app: &mut App| app.checkout(name));
            if current.as_deref() == Some(r.short_name.as_str()) {
                it.hint = "current".into();
                it.strong = true;
            }
            items.push(it);
        }
        for r in self.refs.iter().filter(|r| r.kind == RefKind::RemoteBranch) {
            if r.short_name.ends_with("/HEAD") {
                continue;
            }
            let name = r.short_name.clone();
            items.push(PickItem::new(
                name.clone(),
                "remote",
                move |app: &mut App| app.switch_to(SwitchTarget::Remote(name)),
            ));
        }
        self.modal = Some(Modal::picker("Switch Branch", items));
    }

    pub fn checkout(&mut self, branch: String) {
        self.switch_to(SwitchTarget::Local(branch));
    }

    /// Switches; a refusal because of untracked files / local changes becomes a question
    /// (stash them aside and switch) instead of an error.
    pub fn switch_to(&mut self, target: SwitchTarget) {
        let label = format!("Checkout {}", target.name());
        self.run_op(&label, true, After::Nothing, move |r| {
            match r.switch(&target)? {
                SwitchOutcome::Done(done) => Ok(done.status_line()),
                SwitchOutcome::Blocked(block) => Err(FollowUp::Switch(target, block).into()),
                // TODO: offer to open that worktree (gitti does).
                SwitchOutcome::InOtherWorktree(path) => Err(anyhow::anyhow!(
                    "{} is checked out in the worktree at {}",
                    target.name(),
                    path.display()
                )),
            }
        });
    }

    /// git refused a merge / pull because the histories share no commit: offer to merge
    /// anyway (`--allow-unrelated-histories`).
    fn offer_unrelated_merge(&mut self, u: UnrelatedHistories) {
        let label = match u.retry {
            UnrelatedRetry::Merge { .. } => "Merge",
            UnrelatedRetry::Pull { .. } => "Pull",
        };
        self.modal = Some(Modal::confirm(
            "Unrelated histories",
            u.describe(),
            "Merge anyway (allow unrelated histories)",
            move |app| app.run_op(label, true, After::Nothing, move |r| u.retry(r)),
        ));
    }

    pub fn delete_branch(&mut self, name: String) {
        self.run_op("Delete branch", true, After::Nothing, move |r| {
            r.delete_branch_if_merged(&name)?
                .ok_or_else(|| FollowUp::ForceDelete(name).into())
        });
    }

    fn follow_up(&mut self, f: FollowUp) {
        match f {
            FollowUp::SplitDone { prefix, id, message } => self.split_done(&prefix, id, &message),
            FollowUp::Overwrite(plan) => self.overwrite_confirm(*plan),
            FollowUp::SubmodulesRefused(refused, retry) => self.submodules_refused(*refused, retry),
            FollowUp::ChooseTool(choices, job) => self.tool_dialog(*choices, job),
            FollowUp::ToolInTerminal(run) => self.tool_to_terminal(*run),
            FollowUp::Switch(target, block) => {
                // What blocks, then one entry per way out, each explained.
                let name = target.name().to_string();
                let mut items: Vec<MenuItem> = block
                    .describe(&name)
                    .lines()
                    .flat_map(|l| {
                        let indent = l.starts_with("    ");
                        let mut notes = MenuItem::notes(l, SWITCH_NOTE_WIDTH);
                        if indent && let Some(n) = notes.first_mut() {
                            n.label = format!("  {}", n.label);
                        }
                        notes
                    })
                    .collect();
                for (way, key) in SwitchWay::ALL.into_iter().zip(['s', 'd', 'f']) {
                    items.push(MenuItem::separator());
                    items.extend(MenuItem::notes(&way.explain(&block, &name), SWITCH_NOTE_WIDTH));
                    let (target, block) = (target.clone(), block.clone());
                    let label = format!("Checkout {}", target.name());
                    items.push(
                        MenuItem::new(way.label(&block), move |app: &mut App| {
                            app.run_op(&label, true, After::Nothing, move |r| {
                                r.switch_resolving(&target, &block, way)
                            })
                        })
                        .key(key),
                    );
                }
                self.modal = Some(Modal::menu(format!("Switch to {name}?"), items));
            }
            FollowUp::ForceDelete(name) => {
                self.modal = Some(Modal::confirm(
                    "Branch not fully merged",
                    format!(
                        "{name} is not fully merged into its upstream or HEAD. Delete it \
                         anyway? Its commits stay reachable via the reflog for a while."
                    ),
                    "Force Delete",
                    move |app| {
                        app.run_op("Delete branch", true, After::Nothing, move |r| {
                            r.delete_branch(&name, true)
                        })
                    },
                ));
            }
            f @ (FollowUp::WorktreeRefused { .. } | FollowUp::UnmergedBranch { .. }) => {
                self.worktree_follow_up(f)
            }
        }
    }

    pub fn git_command_dialog(&mut self) {
        let mut m = Modal::input("Run git command", vec![("git", "")], |app, v| {
            let line = v[0].trim().to_string();
            if line.is_empty() {
                return;
            }
            app.cmd_history.retain(|l| *l != line);
            app.cmd_history.push(line.clone());
            let args = gitti_core::ops::split_command_line(&line);
            app.run_op(
                &format!("git {line}"),
                true,
                After::ShowOutput(format!("git {line}")),
                move |r| r.run_user_command(&args),
            );
        });
        if let Modal::Input { fields, .. } = &mut m
            && let Some(last) = self.cmd_history.last()
        {
            fields[0].edit = TextEdit::single(last);
        }
        self.modal = Some(m);
    }

    /// "Stash…": all changes (`stash_cmds::stash_paths_dialog`).
    pub fn stash_dialog(&mut self) {
        self.stash_paths_dialog(Vec::new());
    }

    pub fn create_branch_dialog(&mut self, start: String) {
        let title = format!("Create branch at {}", short_rev(&start));
        self.modal = Some(Modal::input_checks(
            title,
            vec![("Branch name", "")],
            vec![("Check out", true)],
            move |app, v, c| {
                let name = v[0].trim().to_string();
                if name.is_empty() {
                    return;
                }
                let checkout = c[0];
                app.run_op(
                    &format!("Create branch {name}"),
                    true,
                    After::Nothing,
                    move |r| r.create_branch(&name, &start, checkout),
                );
            },
        ));
    }

    /// Create Tag: the name, and whether to write a message next (multi-line, in its own
    /// editor): with one an annotated tag (`git tag -a`), without a lightweight one.
    pub fn create_tag_dialog(&mut self, rev: String) {
        let title = format!("Create tag at {}", short_rev(&rev));
        self.modal = Some(Modal::input_checks(
            title,
            vec![("Tag name", "")],
            vec![("Annotated — write a message next (git tag -a)", true)],
            move |app, v, checks| {
                let name = v[0].trim().to_string();
                if name.is_empty() {
                    return;
                }
                let create = move |app: &mut App, name: String, msg: String| {
                    app.run_op(&format!("Create tag {name}"), true, After::Nothing, move |r| {
                        r.create_tag(&name, &rev, (!msg.trim().is_empty()).then_some(msg.as_str()))
                    });
                };
                if !checks[0] {
                    return create(app, name, String::new());
                }
                app.modal = Some(Modal::edit(
                    format!("Message of tag {name}"),
                    "the tag's message (empty: a lightweight tag)",
                    "",
                    move |app, text| create(app, name, text),
                ));
            },
        ));
    }

    /// Show Tag: tag `name`'s details (lightweight or annotated: tagger, date, message,
    /// verified signature) in a text window, read in the background.
    pub fn show_tag(&mut self, name: String) {
        self.run_op("Show tag", false, After::ShowOutput(format!("Tag {name}")), move |r| {
            Ok(r.tag_details(&name)?.describe())
        });
    }

    /// The action menu of the focused item (right click / `m`).
    pub fn context_menu(&mut self) {
        if let Some((title, items)) = self.context_menu_spec() {
            self.modal = Some(Modal::menu(title, items));
        }
    }

    /// The menu of ref `r` (a right click on its badge in the commit list): its sidebar
    /// entry's, custom commands included. No git.
    pub fn ref_menu(&mut self, r: &RefInfo) {
        let item = match r.kind {
            RefKind::LocalBranch => SideItem::Branch {
                current: self.current_branch() == Some(r.short_name.as_str()),
                tracking: self.tracking_of(&r.short_name).cloned(),
                name: r.short_name.clone(),
                pinned: self.is_pinned(&r.full_name),
            },
            RefKind::RemoteBranch => SideItem::RemoteBranch {
                pinned: self.is_pinned(&r.full_name),
                full: r.short_name.clone(),
                short: self
                    .remotes
                    .iter()
                    .find_map(|rm| r.short_name.strip_prefix(&format!("{}/", rm.name)))
                    .unwrap_or(&r.short_name)
                    .to_string(),
            },
            RefKind::Tag => SideItem::Tag {
                name: r.short_name.clone(),
            },
            _ => return self.context_menu(),
        };
        let items = self.side_menu_items(&item);
        self.modal = Some(Modal::menu(item.title(), items));
    }

    /// The focused thing's context menu (title, items) — what `m` opens and the key bar
    /// lists. Builds closures only: no git.
    /// The focused thing's context menu, every entry with a key ([`crate::modal::assign_keys`],
    /// as `Modal::menu` gives them): the key bar shows what the menu takes.
    pub fn context_menu_spec(&self) -> Option<(String, Vec<MenuItem>)> {
        let (title, mut items) = self.context_menu_items()?;
        crate::modal::assign_keys(&mut items);
        Some((title, items))
    }

    fn context_menu_items(&self) -> Option<(String, Vec<MenuItem>)> {
        match self.focus {
            Focus::Sidebar => {
                let item = self.sidebar.get(self.side_cursor)?;
                Some((item.title(), self.side_menu_items(item)))
            }
            Focus::Commits => self.commit_menu_spec(),
            Focus::Detail | Focus::Message => Some(self.detail_menu_spec()),
            Focus::Log => Some(self.log_menu_spec()),
        }
    }

    fn log_menu_spec(&self) -> (String, Vec<MenuItem>) {
        let reads = if self.log_reads { "on" } else { "off" };
        let mut items = vec![
            MenuItem::new(format!("Show Queries: {reads}"), |app: &mut App| {
                app.log_reads = !app.log_reads
            })
            .key('r'),
            MenuItem::new("Hide Log", |app: &mut App| app.toggle_log())
                .same_as('L', Command::ToggleGitLog),
        ];
        // Web links of the newest outputs (a push's "Create a pull request: https://…").
        let links = self.log_links(3);
        if !links.is_empty() {
            items.push(MenuItem::separator());
        }
        for (i, url) in links.into_iter().enumerate() {
            let u = url.clone();
            let open = MenuItem::new(format!("Open Link {url}"), move |app: &mut App| {
                match gitti_core::desktop::open_url(&u) {
                    Ok(()) => app.set_info(format!("Opened {u}")),
                    Err(e) => app.set_error(format!("Cannot open {u}: {e:#}")),
                }
            });
            let u = url.clone();
            let copy = MenuItem::new(format!("Copy Link {url}"), move |app: &mut App| app.copy(u));
            // o / y for the newest link.
            items.push(if i == 0 { open.key('o') } else { open });
            items.push(if i == 0 { copy.key('y') } else { copy });
        }
        ("Git Command Log".to_string(), items)
    }

    /// The web links in the shown log entries' output, newest first, at most `max`.
    pub fn log_links(&self, max: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for e in self.log_entries().iter().rev() {
            if !e.output.contains("://") {
                continue;
            }
            for url in gitti_core::links::find_links(&e.output) {
                if out.len() < max && !out.contains(&url) {
                    out.push(url);
                }
            }
        }
        out
    }

    // ------------------------------------------------------------------ git log panel

    /// The entries the git log panel shows (changing commands, or all with `log_reads`).
    pub fn log_entries(&self) -> Vec<Arc<gitti_core::git_cli::LogEntry>> {
        // Shared, not copied: up to 1000 entries with up to 16 KiB output each.
        let mut entries = self.repo.command_log().shared();
        entries.retain(|e| self.log_reads || !e.read_only || e.user);
        entries
    }

    /// Entry rows of the git log panel (its height without the title row).
    pub fn log_rows(&self) -> usize {
        (self.areas.log.height as usize).saturating_sub(1)
    }

    /// Hides / shows the sidebar (persisted, shared with gitti); hidden while focused, the
    /// focus moves to the commit list.
    pub fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        if !self.show_sidebar && self.focus == Focus::Sidebar {
            self.focus = Focus::Commits;
        }
        self.set_info(if self.show_sidebar {
            "Sidebar shown (H hides it)"
        } else {
            "Sidebar hidden (H shows it)"
        });
    }

    /// Shows / hides the git log panel; shown again it starts at the newest entries.
    pub fn toggle_log(&mut self) {
        self.show_log = !self.show_log;
        self.log_top = None;
        if !self.show_log && self.focus == Focus::Log {
            self.focus = Focus::Commits;
        }
    }

    /// Scrolls the git log panel by `delta` rows. Scrolling back stops following the
    /// newest entries; reaching the end follows them again. Runs no git.
    pub fn scroll_log(&mut self, delta: isize) {
        self.move_log(|first, _| first.saturating_add_signed(delta));
    }

    /// Moves the git log panel's first shown entry to `to(first, max)` (clamped).
    pub fn move_log(&mut self, to: impl FnOnce(usize, usize) -> usize) {
        let entries = self.log_entries();
        let h = self.log_rows();
        let max = entries.len().saturating_sub(h);
        let first = to(log_first(self.log_top.as_ref(), &entries, h), max).min(max);
        self.log_top = (first < max).then(|| LogTop::of(&entries[first]));
    }

    fn commit_menu_spec(&self) -> Option<(String, Vec<MenuItem>)> {
        let row = self.selected;
        if self.history.is_worktree_row(row) {
            let mut items = self.worktree_items();
            items.push(MenuItem::separator());
            items.extend(self.compare_menu_items());
            return Some(("Working Directory".to_string(), items));
        }
        let c = self.history.commits.get(row)?;
        let sha = c.id.to_string();
        let short = sha[..7.min(sha.len())].to_string();
        let merge = c.parents.len() > 1;
        if let Some(n) = c.stash {
            return Some((format!("stash@{{{n}}}"), sidebar_menu::stash_items(n)));
        }
        let head = self.current_branch().unwrap_or("HEAD").to_string();
        // Why the entries acting on the commit can't (HEAD itself, an operation git
        // left half-done).
        let busy_op = gitti_core::availability::in_progress(self.repo_state);
        let head_or_busy = (self.head.id == Some(c.id))
            .then_some(gitti_core::availability::IS_HEAD)
            .or(busy_op);
        let mut items = Vec::new();
        for r in self
            .history
            .refs_by_commit
            .get(&c.id)
            .into_iter()
            .flatten()
            .filter(|r| r.kind == RefKind::LocalBranch)
        {
            if Some(r.short_name.as_str()) != self.current_branch() {
                let b = r.short_name.clone();
                items.push(MenuItem::new(format!("Checkout {b}"), move |app| {
                    app.checkout(b)
                }));
            }
        }
        let s = sha.clone();
        items.push(MenuItem::new(
            format!("Checkout {short} (detached)"),
            move |app| app.switch_to(SwitchTarget::Detached(s)),
        ));
        items.extend(self.compare_menu_items());
        items.push(MenuItem::separator());
        let s = sha.clone();
        items.push(
            MenuItem::new("Create Branch Here…", move |app| {
                app.create_branch_dialog(s)
            })
            .key('b'),
        );
        let s = sha.clone();
        items.push(MenuItem::new("Create Tag Here…", move |app| app.create_tag_dialog(s)).key('t'));
        items.push(MenuItem::separator());
        let s = sha.clone();
        items.push(
            MenuItem::new(format!("Cherry-pick {short}"), move |app| {
                app.run_op("Cherry-pick", true, After::Nothing, move |r| {
                    r.cherry_pick(&s, merge)
                })
            })
            .key('c')
            .unless(head_or_busy),
        );
        let (s, sh) = (sha.clone(), short.clone());
        items.push(
            MenuItem::new("Cherry-pick with Options…", move |app| {
                app.cherry_pick_options(s, sh, merge)
            })
            .key('x')
            .unless(head_or_busy),
        );
        let s = sha.clone();
        items.push(
            MenuItem::new(format!("Revert {short}"), move |app| {
                app.run_op("Revert", true, After::Nothing, move |r| r.revert(&s, merge))
            })
            .key('v')
            .unless(busy_op),
        );
        let s = sha.clone();
        items.push(
            MenuItem::new(format!("Merge {short} into {head}"), move |app| {
                app.run_op("Merge", true, After::Nothing, move |r| r.merge(&s))
            })
            .unless(head_or_busy),
        );
        let s = sha.clone();
        items.push(
            MenuItem::new(format!("Rebase {head} onto {short}"), move |app| {
                app.run_op("Rebase", true, After::Nothing, move |r| r.rebase(&s))
            })
            .unless(head_or_busy),
        );
        for (label, mode) in [
            ("soft", ResetMode::Soft),
            ("mixed", ResetMode::Mixed),
            ("hard", ResetMode::Hard),
        ] {
            let s = sha.clone();
            let reset = move |app: &mut App| {
                app.run_op(
                    &format!("Reset ({label})"),
                    true,
                    After::Nothing,
                    move |r| r.reset(&s, mode),
                )
            };
            if mode == ResetMode::Hard {
                let short = short.clone();
                let head = head.clone();
                items.push(MenuItem::new(format!("Reset {head} to here — hard"), move |app| {
                    app.modal = Some(Modal::confirm(
                        "Reset Hard",
                        format!("Reset {head} to {short} and discard all uncommitted changes?\nThis cannot be undone."),
                        "Reset",
                        reset,
                    ));
                }));
            } else {
                items.push(MenuItem::new(
                    format!("Reset {head} to here — {label}"),
                    reset,
                ));
            }
        }
        // The remote branch moves (push with lease); the local one stays.
        items.extend(self.remote_reset_items(&sha));
        items.push(MenuItem::separator());
        let s = sha.clone();
        items.push(
            MenuItem::new("Edit Commit Message…", move |app| app.edit_commit_message(&s)).key('e'),
        );
        items.extend(self.restore_contains_items(c.id));
        items.push(MenuItem::separator());
        items.extend(self.subtree_ref_items(&sha, ObjectId::from_hex(sha.as_bytes()).ok(), None));
        items.push(MenuItem::separator());
        let s = sha.clone();
        items.push(MenuItem::new("Copy SHA", move |app| app.copy(s)).key('y'));
        // Read when chosen (the key bar builds this menu for every frame).
        items.push(MenuItem::new("Copy Subject", move |app| {
            let subject = app.history.meta(row).summary.clone();
            app.copy(subject)
        }));
        items.extend(self.signing_key_items(Some(&sha)));
        items.extend(self.signers_items(Some(&sha)));
        items.extend(self.custom_menu_items(
            gitti_core::custom_command::Menu::Commit,
            custom_commands::Given {
                commit: Some(sha.clone()),
                ..Default::default()
            },
        ));
        Some((format!("Commit {short}"), items))
    }

    fn worktree_items(&self) -> Vec<MenuItem> {
        vec![
            MenuItem::new("Stage All", |app| app.stage_all())
                .same_as('S', Command::StageAll)
                .unless(self.why_not("StageAll")),
            MenuItem::new("Unstage All", |app| app.unstage_all())
                .same_as('U', Command::UnstageAll)
                .unless(self.why_not("UnstageAll")),
            MenuItem::new("Discard All Unstaged…", |app| app.discard_all())
                .same_as('D', Command::DiscardAll)
                .unless(self.why_not("DiscardAll")),
            MenuItem::separator(),
            MenuItem::new("Stash…", |app| app.stash_dialog())
                .same_as('z', Command::StashChanges)
                .unless(self.why_not("StashChanges")),
            MenuItem::new("Commit", |app| app.commit()),
            MenuItem::new("Stage All and Commit", |app| {
                app.commit_kind(gitti_core::ops::CommitKind::StageAll)
            })
            .unless(self.why_not("StageAllAndCommit")),
            MenuItem::new(gitti_core::ops::STAGE_TRACKED_LABEL, |app| {
                app.commit_kind(gitti_core::ops::CommitKind::StageTracked)
            })
            .unless(self.why_not("StageTrackedAndCommit")),
            MenuItem::new("Create Empty Commit", |app| {
                app.commit_kind(gitti_core::ops::CommitKind::Empty)
            }),
            MenuItem::new(
                if self.amend {
                    "Amend: off"
                } else {
                    "Amend previous commit"
                },
                |app| app.toggle_amend(),
            )
            .same_as('A', Command::ToggleAmend),
            MenuItem::new(
                if self.commit_box.sign_off { "Sign off: off" } else { "Sign off commits" },
                |app| app.toggle_sign_off(),
            ),
            MenuItem::new(gitti_core::commit_message::RECENT_LABEL, |app| {
                app.open_recent_messages()
            }),
        ]
    }

    fn detail_menu_spec(&self) -> (String, Vec<MenuItem>) {
        let mut items = Vec::new();
        // On the header's commit id or parents line: copy the id(s).
        let ids = self.header_id_entries();
        if !ids.is_empty() {
            for (label, text) in ids {
                items.push(MenuItem::new(label, move |app| app.copy(text)));
            }
            items.push(MenuItem::separator());
        }
        // On the header's Signature line, its details, the GitHub keys and an allowed
        // signers file.
        if Some(self.diff_cursor) == self.signature_header_line() {
            items.extend(self.signing_key_items(None));
            items.extend(self.signers_items(None));
            if !items.is_empty() {
                items.push(MenuItem::separator());
            }
        }
        let row = self.rows.get(self.diff_cursor).copied();
        match row {
            Some(
                DiffRow::File {
                    src: Src::Conflicted,
                    file,
                }
                | DiffRow::Note {
                    src: Src::Conflicted,
                    file,
                },
            ) => {
                if let Some(spec) = self.resolve_menu_spec(file) {
                    return spec;
                }
            }
            Some(
                DiffRow::File { src, file }
                | DiffRow::Note { src, file }
                | DiffRow::Submodule { src, file, .. }
                | DiffRow::Hunk { src, file, .. }
                | DiffRow::Line { src, file, .. },
            ) => {
                let path = self
                    .file_diff(src, file)
                    .map(|f| f.path().to_string())
                    .or_else(|| {
                        work_files(&self.working, src)
                            .get(file)
                            .map(|w| w.path().to_string())
                    })
                    .unwrap_or_default();
                match src {
                    Src::Unstaged => {
                        items.push(MenuItem::new("Stage File", move |app| {
                            app.file_action(src, file, PatchMode::Stage)
                        }));
                        items.push(MenuItem::new("Discard File…", move |app| {
                            app.file_action(src, file, PatchMode::Discard)
                        }));
                    }
                    Src::Staged => items.push(MenuItem::new("Unstage File", move |app| {
                        app.file_action(src, file, PatchMode::Unstage)
                    })),
                    _ => {}
                }
                items.extend(self.stash_row_items(row));
                if matches!(src, Src::Unstaged | Src::Staged) {
                    let tracked = !work_files(&self.working, src)
                        .get(file)
                        .is_some_and(|w| w.entry.untracked);
                    let p = path.clone();
                    items.push(
                        MenuItem::new("Add to .gitignore…", move |app| app.ignore_menu(&p, tracked))
                            .key('i'),
                    );
                    let p = path.clone();
                    items.push(
                        MenuItem::new("Track with Git LFS…", move |app| app.lfs_track(p))
                            .key('L'),
                    );
                    // A path this file system can't hold: why, hide it.
                    items.extend(self.fs_path_menu_items(src, file));
                }
                let sub = if self.file_diff(src, file).is_some_and(|f| f.submodule.is_some()) {
                    self.submodule_file_items(&path)
                } else {
                    Vec::new()
                };
                items.extend(self.restore_file_items(src, file));
                items.extend(self.lfs_menu_items());
                items.extend(self.external_diff_item(src, file));
                items.push(MenuItem::new("Copy Path", move |app| app.copy(path)));
                // The file's custom commands (the diff cursor's file).
                items.extend(self.custom_menu_items(
                    gitti_core::custom_command::Menu::File,
                    custom_commands::Given::default(),
                ));
                items.push(MenuItem::separator());
                if !sub.is_empty() {
                    items.extend(sub);
                    items.push(MenuItem::separator());
                }
            }
            Some(DiffRow::Group(_)) => {
                items.extend(self.stash_row_items(row));
                items.push(MenuItem::separator());
            }
            _ => {}
        }
        items.extend(self.marks_menu_items());
        if matches!(self.detail, Detail::Worktree) {
            items.extend(self.worktree_items());
            items.push(MenuItem::separator());
        }
        items.extend(self.lfs_download_menu_items());
        items.push(
            MenuItem::new("Show Whole File", |app| app.show_more_lines_key(true))
                .same_as('X', Command::ShowWholeFile),
        );
        // The way back while unchanged lines of the file are shown (esc: every file).
        if let Some(file) = self.revealed_file_at_cursor() {
            items.push(MenuItem::new("Show Changes Only", move |app| {
                app.show_changes_only(Some(file));
            }));
        }
        if matches!(self.detail, Detail::Worktree) {
            items.push(self.tree_menu_item());
        }
        items.push(MenuItem::new("Expand All Files", |app| {
            app.set_all_expanded(true)
        }));
        items.push(MenuItem::new("Collapse All Files", |app| {
            app.set_all_expanded(false)
        }));
        items.push(
            MenuItem::new(
                format!("Ignore Whitespace: {}", on_off(self.opts.ignore_whitespace)),
                |app| app.toggle_whitespace(),
            )
            .same_as('W', Command::ToggleWhitespace),
        );
        items.push(
            MenuItem::new(format!("Wrap Lines: {}", on_off(self.opts.wrap)), |app| {
                app.toggle_wrap()
            })
            .same_as('w', Command::ToggleWrap),
        );
        items.push(MenuItem::new(
            format!("Syntax Highlighting: {}", on_off(self.opts.syntax)),
            |app| app.toggle_syntax(),
        ));
        if self.compare.mode.is_some() {
            items.push(MenuItem::separator());
            items.extend(self.compare_menu_items());
        }
        ("Diff".to_string(), items)
    }

    /// All commands, for the command palette.
    pub fn palette(&mut self) {
        // Keys in their own column, from the shared key table where it has them.
        use gitti_core::keys::Command as KC;
        let mut items = vec![
            PickItem::cmd("Fetch", KC::Fetch, |app: &mut App| app.fetch()),
            PickItem::cmd("Pull", KC::Pull, |app: &mut App| app.pull()),
            PickItem::cmd("Push", KC::Push, |app: &mut App| app.push()),
            PickItem::cmd("Switch Branch…", KC::SwitchBranch, |app: &mut App| {
                app.switch_branch_picker()
            }),
            PickItem::new("Create Branch at HEAD…", "", |app: &mut App| {
                app.create_branch_dialog("HEAD".into())
            }),
            PickItem::cmd("Stage All", KC::StageAll, |app: &mut App| app.stage_all()),
            PickItem::cmd("Unstage All", KC::UnstageAll, |app: &mut App| app.unstage_all()),
            PickItem::cmd("Discard All Unstaged…", KC::DiscardAll, |app: &mut App| {
                app.discard_all()
            }),
            PickItem::new("Commit", "", |app: &mut App| app.commit()).keys("ctrl-s"),
            PickItem::cmd("Edit Commit Message", KC::FocusMessage, |app: &mut App| {
                app.focus_message()
            }),
            PickItem::cmd("Toggle Amend", KC::ToggleAmend, |app: &mut App| {
                app.toggle_amend()
            }),
            PickItem::cmd("Stash…", KC::StashChanges, |app: &mut App| app.stash_dialog()),
            PickItem::cmd("Run Git Command…", KC::RunGitCommand, |app: &mut App| {
                app.git_command_dialog()
            }),
            PickItem::cmd("Toggle Git Command Log", KC::ToggleGitLog, |app: &mut App| {
                app.toggle_log()
            }),
            PickItem::new("Toggle Queries in Git Log", "", |app: &mut App| {
                app.log_reads = !app.log_reads
            }),
            PickItem::cmd(
                format!("Ignore Whitespace: {}", on_off(self.opts.ignore_whitespace)),
                KC::ToggleWhitespace,
                |app: &mut App| app.toggle_whitespace(),
            ),
            PickItem::cmd(
                format!("Wrap Lines: {}", on_off(self.opts.wrap)),
                KC::ToggleWrap,
                |app: &mut App| app.toggle_wrap(),
            ),
            PickItem::new(
                format!("Syntax Highlighting: {}", on_off(self.opts.syntax)),
                "",
                |app: &mut App| app.toggle_syntax(),
            ),
            PickItem::cmd(
                format!("More Context Lines ({})", self.opts.context),
                KC::MoreContext,
                |app: &mut App| app.change_context(1),
            ),
            PickItem::cmd(
                format!("Fewer Context Lines ({})", self.opts.context),
                KC::LessContext,
                |app: &mut App| app.change_context(-1),
            ),
            PickItem::new(
                format!(
                    "Highlight Changed Characters Stronger ({} %)",
                    self.opts.intraline_strength
                ),
                "",
                |app: &mut App| app.step_intraline_strength(1),
            ),
            PickItem::new(
                format!(
                    "Highlight Changed Characters Weaker ({} %)",
                    self.opts.intraline_strength
                ),
                "",
                |app: &mut App| app.step_intraline_strength(-1),
            ),
            PickItem::cmd(
                if self.opts.topo {
                    "History Order: Date"
                } else {
                    "History Order: Topological"
                },
                KC::ToggleOrder,
                |app: &mut App| app.toggle_order(),
            ),
        ];
        // The commit list's rows (gitti: also in the View menu).
        items.extend(gitti_core::list_layout::CommitListLayout::ALL.into_iter().map(|l| {
            PickItem::new(l.palette_label(self.opts.list_layout), l.description(), move |app: &mut App| {
                app.set_list_layout(l)
            })
        }));
        // The sidebar's branch order and the multi-branch deletes (`app/branch_cmds.rs`).
        items.extend(self.branch_palette_items());
        items.extend([
            PickItem::new("Expand All Files", "", |app: &mut App| {
                app.set_all_expanded(true)
            }),
            PickItem::new("Collapse All Files", "", |app: &mut App| {
                app.set_all_expanded(false)
            }),
            PickItem::cmd("Show Whole File", KC::ShowWholeFile, |app: &mut App| {
                app.show_more_lines_key(true)
            }),
            PickItem::new("Show Changes Only", "esc in the diff", |app: &mut App| {
                if !app.show_changes_only(None) {
                    app.set_info("No unchanged lines are shown");
                }
            }),
            // See `app/commit_flow.rs`; below the view toggles, which the palette's first
            // page shows.
            PickItem::new("Stage All and Commit", "git add -A, then commit", |app: &mut App| {
                app.commit_kind(gitti_core::ops::CommitKind::StageAll)
            }),
            PickItem::new(
                gitti_core::ops::STAGE_TRACKED_LABEL,
                "git add -u, then commit (untracked files stay out)",
                |app: &mut App| app.commit_kind(gitti_core::ops::CommitKind::StageTracked),
            ),
            PickItem::new("Create Empty Commit", "a commit without changes", |app: &mut App| {
                app.commit_kind(gitti_core::ops::CommitKind::Empty)
            }),
            PickItem::cmd("Refresh", KC::Refresh, |app: &mut App| app.refresh_everything()),
            PickItem::new("Write Commit-Graph", "", |app: &mut App| {
                app.run_op("Write commit-graph", true, After::Nothing, |r| {
                    r.write_commit_graph()
                })
            }),
            PickItem::cmd("Keyboard Shortcuts", KC::ShowShortcuts, |app: &mut App| {
                app.modal = Some(Modal::Help { scroll: 0 })
            }),
            // Shortcuts that need AltGr on the chosen layout move to typeable keys.
            PickItem::new(
                format!("Keyboard Layout…: {}", crate::kb::layout().name()),
                "",
                |app: &mut App| app.keyboard_layout_picker(),
            ),
            PickItem::cmd(
                format!("Toggle Sidebar (show / hide): {}", on_off(self.show_sidebar)),
                KC::ToggleSidebar,
                |app: &mut App| app.toggle_sidebar(),
            ),
            PickItem::new("Quit", "", |app: &mut App| app.request_quit())
                .keys(format!("q / {}", gitti_core::palette::command_keys(KC::Quit))),
            // The file list, also in gitti's View menu.
            PickItem::cmd("Go to File…", KC::FileList, |app: &mut App| app.file_picker()),
        ]);
        items.push(PickItem::new("Go to Commit…", "ctrl-g", |app: &mut App| {
            app.goto_commit_picker()
        }));
        // Also `m` on the Signature line and the commit's menu (`K`).
        items.push(PickItem::new(
            gitti_core::signing_keys::ACTION_LABEL,
            "for the shown commit's signature",
            |app: &mut App| app.get_signing_keys(None),
        ));
        items.push(self.signers_palette_item());
        items.extend(self.change_search_palette_items());
        items.extend(self.reflog_palette_items());
        items.extend(self.repo_palette_items());
        items.extend(self.ext_tool_palette_items());
        items.extend(self.terminal_palette_items());
        items.extend(self.submodule_palette_items());
        items.extend(self.lfs_palette_items());
        items.extend(self.lfs_download_palette_items());
        items.extend(self.lfs_health_palette_items());
        items.extend(self.fs_paths_palette_items());
        items.extend(self.worktree_palette_items());
        items.extend(self.menu_palette_items());
        items.extend(self.restore_contains_palette_items());
        items.extend(self.prepared_palette_items());
        items.extend(self.commit_box_palette_items());
        if self.repo_state != RepoState::Clean || !self.working.conflicted.is_empty() {
            let clean = self.repo_state == RepoState::Clean;
            let label = if clean {
                "(undo the conflicted changes)"
            } else {
                self.repo_state.label()
            };
            items.insert(
                0,
                PickItem::new(format!("Abort {label}"), "", |app: &mut App| {
                    if app.repo_state != RepoState::Clean {
                        app.run_op("Abort", true, After::Nothing, |r| r.abort_operation());
                        return;
                    }
                    // Conflicts without an operation (stash pop / apply): ask first.
                    let (title, body) = gitti_core::ops::abort_conflicts_prompt(
                        app.working.conflicted.iter().map(|f| f.path()),
                        app.working.staged.iter().map(|f| f.path()),
                    );
                    app.modal = Some(Modal::confirm(title, body, "Abort", |app: &mut App| {
                        app.run_op("Abort", true, After::Nothing, |r| r.abort_conflicts())
                    }));
                }),
            );
            if !clean {
                items.insert(
                    0,
                    PickItem::new(
                        format!("Continue {label}"),
                        match self.continue_message() {
                            Some(_) => gitti_core::ops::CONTINUE_WITH_BOX,
                            None => "",
                        },
                        |app: &mut App| app.continue_operation(),
                    ),
                );
            }
        }
        for s in self.stashes.iter().take(9) {
            let i = s.index;
            items.push(PickItem::new(
                format!("Apply {} — {}", s.name, s.message),
                "",
                move |app: &mut App| {
                    app.run_op("Apply stash", true, After::Nothing, move |r| {
                        r.stash_apply(i, false)
                    })
                },
            ));
        }
        items.extend(self.compare_palette_items());
        items.extend(self.stash_palette_items());
        items.extend(self.file_marks_palette_items());
        // Last: on an equal match the everyday commands come first ("merge").
        items.extend(self.subtree_palette_items());
        items.extend(self.custom_palette_items());
        // What can't act now stays findable, greyed with the reason.
        let items = self.mark_unavailable(items);
        self.modal = Some(Modal::palette("Command Palette", items));
    }

    pub fn focus_message(&mut self) {
        if !self.history.is_worktree_row(self.selected) {
            self.select_row(0);
        }
        self.focus = Focus::Message;
    }

    pub fn toggle_whitespace(&mut self) {
        self.opts.ignore_whitespace = !self.opts.ignore_whitespace;
        self.line_sel = None;
        self.keep_view_anchor();
        self.reload_diffs();
    }

    pub fn toggle_syntax(&mut self) {
        self.opts.syntax = !self.opts.syntax;
        self.schedule_highlight();
    }

    /// Palette "… Commit List" (stored, shared with gitti); the next frame lays the rows
    /// out and keeps the selected commit in view.
    pub fn set_list_layout(&mut self, l: gitti_core::list_layout::CommitListLayout) {
        self.opts.list_layout = l;
        self.follow = true;
        self.set_info(l.chosen_message());
    }

    pub fn toggle_wrap(&mut self) {
        self.opts.wrap = !self.opts.wrap;
        self.hscroll = 0;
        self.rebuild_rows_keep_view();
    }

    /// Intra-line mark strength up / down by 10 % (0 = off, 100 = strongest).
    pub fn step_intraline_strength(&mut self, steps: i32) {
        let n = (self.opts.intraline_strength as i32 + 10 * steps).clamp(0, 100) as u32;
        self.opts.intraline_strength = n;
        self.set_info(if n == 0 {
            "Changed characters: not highlighted".to_string()
        } else {
            format!("Changed characters: highlighted at {n} %")
        });
    }

    pub fn change_context(&mut self, delta: i32) {
        self.opts.context = (self.opts.context as i32 + delta).clamp(0, 50) as u32;
        self.set_info(format!("Context lines: {}", self.opts.context));
        self.keep_view_anchor();
        self.reload_diffs();
    }

    pub fn toggle_order(&mut self) {
        self.opts.topo = !self.opts.topo;
        self.load_history(false);
    }

    /// "Edit Commit Message…": amend for HEAD, a non-interactive reword rebase for older,
    /// unpushed commits (`Repo::reword`).
    pub fn edit_commit_message(&mut self, sha: &str) {
        let Ok(id) = ObjectId::from_hex(sha.as_bytes()) else {
            return;
        };
        // `branch -r --contains` & co. can take a while in big repositories.
        let repo = self.repo.clone();
        self.start_load(
            move || {
                repo.reword_blocker(id)
                    .and_then(|why| Ok((why, repo.commit_details(id)?.message)))
            },
            move |checked| Msg::RewordChecked(id, checked),
        );
    }

    /// Runs `job` in the background; `is_idle` waits for it (scripted runs).
    pub fn start_load<T: Send + 'static>(
        &mut self,
        job: impl FnOnce() -> T + Send + 'static,
        wrap: impl FnOnce(T) -> Msg + Send + 'static,
    ) {
        self.loads_in_flight += 1;
        spawn(&self.tx, job, wrap);
    }

    fn reword_checked(&mut self, id: ObjectId, checked: Result<(Option<String>, String)>) {
        match checked {
            Ok((Some(why), _)) => {
                self.set_error(format!("Can't edit this commit message: {why}"))
            }
            Ok((None, message)) => {
                let short = id.to_hex_with_len(7).to_string();
                self.modal = Some(Modal::edit(
                    format!("Edit message of {short}"),
                    "rewrites the commit and the ones after it",
                    message.trim_end(),
                    move |app, text| {
                        if text.trim().is_empty() {
                            app.set_error("The message must not be empty".into());
                            return;
                        }
                        app.run_op("Edit commit message", true, After::Nothing, move |r| {
                            r.reword(id, &text)
                        });
                    },
                ));
            }
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// `o` in the diff: opens the working-tree file of the cursor row at its line in the
    /// editor (setting `editor_command`, else `$VISUAL` / `$EDITOR`, else the system default).
    pub fn open_in_editor(&mut self) {
        let Some(row) = self.rows.get(self.diff_cursor).copied() else {
            return;
        };
        let Some((src, file)) = row.file_ref() else {
            self.set_info("Nothing to open here");
            return;
        };
        let Some(f) = self.file_diff(src, file) else {
            return;
        };
        let (path, line) = (f.path().to_string(), row.editor_line(f));
        self.open_path_in_editor(&path, line);
    }

    /// Opens working-tree file `path` at `line` (1-based) in the editor; terminal editors
    /// run with the terminal handed over (the main loop does it).
    pub fn open_path_in_editor(&mut self, path: &str, line: u32) {
        let workdir = self.repo.workdir().to_path_buf();
        let full = gitti_core::paths::join_repo_path(&workdir, path);
        if !full.exists() {
            self.set_error(format!("{path} does not exist in the working tree"));
            return;
        }
        let mut template = self.settings.editor_command.clone();
        if template.trim().is_empty() {
            // A set but empty `$VISUAL` counts as unset (`VISUAL= gitty` must not hide
            // `$EDITOR` behind the system default application).
            let var = |name| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
            template = var("VISUAL").or_else(|| var("EDITOR")).unwrap_or_default();
        }
        match gitti_core::editor::editor_argv(&template, &full, line) {
            Ok(argv) if gitti_core::editor::is_terminal_editor(&argv) => {
                self.run_in_terminal = Some((argv, workdir));
            }
            Ok(argv) => match gitti_core::editor::spawn_editor(&argv, &workdir) {
                Ok(()) => self.set_info(format!("Opened {path}:{line}")),
                Err(e) => self.set_error(format!("{e:#}")),
            },
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// A terminal editor (`run_in_terminal`, run by the main loop — or the script runner)
    /// returned: report how, re-read the merge editor's file and the repository.
    pub fn terminal_editor_done(
        &mut self,
        program: &str,
        status: std::io::Result<std::process::ExitStatus>,
    ) {
        match status {
            Ok(s) if s.success() => self.set_info(format!("{program} finished")),
            Ok(s) => self.set_error(format!("{program} exited with {s}")),
            Err(e) => self.set_error(format!("cannot start {program}: {e}")),
        }
        self.merge_check_disk();
        self.refresh(false);
        self.open_next();
    }

    /// The copy entries (label, text) of the header line under the detail cursor: the
    /// commit id's line its id, the parents line each parent, the author line the author
    /// (whole, name, email), the date line the date and a different committer
    /// (`gitti_core::header_ids`).
    fn header_id_entries(&self) -> Vec<(String, String)> {
        use gitti_core::header_ids::{HeaderId, copy_entries, person_entries};
        let Detail::Commit(cv) = &self.detail else {
            return Vec::new();
        };
        if cv.compare.is_some() || !matches!(self.rows.get(self.diff_cursor), Some(DiffRow::HeaderSpace)) {
            return Vec::new();
        }
        let parents = &cv.details.parents;
        let d = &cv.details;
        if self.diff_cursor == crate::ui::ID_LINE {
            copy_entries(&cv.details.id, HeaderId::Commit, parents.len())
        } else if self.diff_cursor == crate::ui::AUTHOR_LINE {
            person_entries("Author", &d.author, &d.author_email)
        } else if self.diff_cursor == crate::ui::DATE_LINE {
            let mut v = vec![("Copy Date".to_string(), crate::ui::header_date(d))];
            if d.committer != d.author || d.committer_email != d.author_email {
                v.extend(person_entries("Committer", &d.committer, &d.committer_email));
            }
            v
        } else if Some(self.diff_cursor) == crate::ui::parents_line(cv) {
            parents
                .iter()
                .enumerate()
                .flat_map(|(n, id)| copy_entries(id, HeaderId::Parent(n), parents.len()))
                .collect()
        } else {
            Vec::new()
        }
    }

    /// `y` in the diff: the selected lines, else the line under the cursor; on the commit
    /// header's id line the commit id, on its parents line the parent ids (space-separated),
    /// on its author line "Name <email>", on its date line the date, elsewhere in the
    /// header the commit message; on a file header its path.
    pub fn copy_selection(&mut self) {
        let line_text = |app: &App, src: Src, file: usize, hunk: usize, line: usize| {
            app.file_diff(src, file)
                .and_then(|f| f.hunks.get(hunk)?.lines.get(line))
                .map(|l| l.text.clone())
        };
        let text = if let Some(sel) = &self.line_sel {
            let lines: Vec<String> = sel
                .lines
                .iter()
                .filter_map(|&(h, l)| line_text(self, sel.src, sel.file, h, l))
                .collect();
            Some(lines.join("\n"))
        } else {
            match self.rows.get(self.diff_cursor) {
                Some(&DiffRow::Line {
                    src,
                    file,
                    hunk,
                    line,
                    ..
                }) => line_text(self, src, file, hunk, line),
                Some(DiffRow::HeaderSpace) => match &self.detail {
                    Detail::Commit(cv) if cv.compare.is_none() && self.diff_cursor == crate::ui::ID_LINE => {
                        Some(cv.details.id.to_string())
                    }
                    Detail::Commit(cv) if cv.compare.is_none() && self.diff_cursor == crate::ui::AUTHOR_LINE => {
                        Some(format!("{} <{}>", cv.details.author, cv.details.author_email))
                    }
                    Detail::Commit(cv) if cv.compare.is_none() && self.diff_cursor == crate::ui::DATE_LINE => {
                        Some(crate::ui::header_date(&cv.details))
                    }
                    Detail::Commit(cv) if Some(self.diff_cursor) == crate::ui::parents_line(cv) => Some(
                        cv.details
                            .parents
                            .iter()
                            .map(|id| id.to_string())
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    Detail::Commit(cv) => Some(cv.details.message.trim_end().to_string()),
                    _ => None,
                },
                Some(&DiffRow::File { src, file }) => {
                    self.file_diff(src, file).map(|f| f.path().to_string())
                }
                _ => None,
            }
        };
        match text {
            Some(t) if !t.is_empty() => self.copy(t),
            _ => self.set_info("Nothing to copy here"),
        }
    }

    /// Copies to the clipboard: `pbcopy` / `wl-copy` / `xclip` / `xsel` / `clip`, else
    /// OSC 52 (`clipboard.rs`).
    pub fn copy(&mut self, text: String) {
        use std::io::Write;
        // Scripted runs (tests) must not overwrite the user's clipboard: report only.
        if std::env::var_os("GITTY_SCRIPT").is_some() {
            self.set_info(format!("Copied {}", ellipsize(&text, 50)));
            Self::set_script_clipboard(&text);
            return;
        }
        if crate::clipboard::copy_with(&text, crate::clipboard::tools(), None).is_some() {
            self.set_info(format!("Copied {}", ellipsize(&text, 50)));
            return;
        }
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", crate::clipboard::osc52(&text));
        let _ = out.flush();
        self.set_info(format!("Copied {} (OSC 52)", ellipsize(&text, 50)));
    }
}

/// `2026-10-06 09:12:03` in local time (ms since the epoch; the Start-up Timings window).
pub fn local_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn short_rev(rev: &str) -> &str {
    if gitti_core::object_format::is_full_hex(rev) {
        &rev[..7]
    } else {
        rev
    }
}

pub fn ellipsize(s: &str, max: usize) -> String {
    let s = s.lines().next().unwrap_or("");
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

/// The line of a git error that says what went wrong.
pub fn summarize_error(full: &str) -> String {
    let lines: Vec<&str> = full
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let key = lines.iter().find(|l| {
        let l = l.to_lowercase();
        l.starts_with("error")
            || l.starts_with("fatal")
            || l.contains("rejected")
            || l.starts_with("conflict")
    });
    let head = lines.first().copied().unwrap_or("");
    match key {
        Some(k) if *k != head => format!("{} {k}", head.trim_end_matches(':')),
        _ => lines.iter().take(2).copied().collect::<Vec<_>>().join(" "),
    }
}

/// The key of a pull strategy in gitty's menus (upper case: remember it).
fn strategy_key(strategy: PullStrategy) -> char {
    match strategy {
        PullStrategy::Merge => 'n',
        PullStrategy::Rebase => 'r',
        PullStrategy::FastForwardOnly => 'f',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn error_summary_picks_meaningful_line() {
        let s = summarize_error(
            "git push failed (1):\nTo server:x\n ! [rejected] main -> main (fetch first)\nerror: failed to push",
        );
        assert_eq!(
            s,
            "git push failed (1) ! [rejected] main -> main (fetch first)"
        );
    }
}

