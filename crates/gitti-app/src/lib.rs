//! gitti — GPUI git GUI. A library so headless UI tests (`tests/ui`) can build the
//! real views; the `gitti` binary (`main.rs`) only calls [`run`].

mod app;
mod app_icon;
mod assets;
mod graph_paint;
mod highlight;
mod menu_height;
mod merge_window;
mod message;
mod rem_scope;
pub mod layout_check;
pub mod probe;
mod rows;
mod script;
mod scroll_guard;
mod secure_input;
mod shell_env;
mod shortcuts;
pub mod spell;
use gitti_core::store;
mod text_sel;
mod theme;
mod tooltip_gate;
mod window_placement;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

actions!(
    gitti,
    [
        Quit,
        Refresh,
        Fetch,
        Pull,
        Push,
        Commit,
        StashChanges,
        SelectNext,
        SelectPrev,
        StageSelection,
        CopySelection,
        OpenInEditor,
        ShowShortcuts,
        ToggleAmend,
        StageAll,
        UnstageAll,
        DiscardAll,
        MoreContext,
        LessContext,
        ErrorDetails,
        SelectFirst,
        SelectLast,
        DiscardSelection,
        NextHunk,
        PrevHunk,
        NextFile,
        PrevFile,
        FileList,
        UnstageSelection,
        ToggleOrder,
        FocusMessage,
        FocusCommitList,
        ZoomIn,
        ZoomOut,
        ZoomDiffIn,
        ZoomDiffOut,
        ResetZoom,
        ModalCancel,
        CommandPalette,
        SwitchBranch,
        ToggleGitLog,
        ToggleWrap,
        ToggleWhitespace,
        PickerUp,
        PickerDown,
        RunGitCommand,
        CmdHistoryPrev,
        CmdHistoryNext,
        OpenSettings,
        FollowBranchDown,
        FollowBranchUp,
        OtherBranchDown,
        OtherBranchUp,
        OpenRepository,
        CloseTab,
        NextTab,
        PrevTab,
        PageUp,
        PageDown,
        ScrollHome,
        ScrollEnd,
        NextPane,
        PrevPane,
        FocusSidebar,
        FocusDiff,
        SideNext,
        SidePrev,
        SideActivate,
        MarkCompare,
        EndCompare,
        SwapCompare,
        LineUp,
        LineDown,
        HalfPageUp,
        HalfPageDown,
        ShowMoreLines,
        ShowWholeFile,
    ]
);

/// The app's entry point (`main.rs` only calls this).
pub fn run() {
    // Started by git/ssh as askpass helper? Then answer the prompt and exit.
    if let Some(code) = gitti_core::askpass::run_client_if_requested() {
        std::process::exit(code);
    }
    // Start-up timings: from here on, after the askpass helper path. Scripted runs
    // (tests, smoke) open off-screen and would only bury the real starts — unless
    // `GITTI_SCRIPT_STARTUP=1` asks for it (to check the recording itself).
    let scripted = std::env::var_os("GITTI_SCRIPT").is_some();
    if !scripted || std::env::var_os("GITTI_SCRIPT_STARTUP").is_some() {
        gitti_core::startup::begin("gitti", env!("CARGO_PKG_VERSION"));
    }
    // Panics go to <config dir>/logs/panic.log (no terminal when started from a launcher).
    gitti_core::panic_log::install("gitti", env!("CARGO_PKG_VERSION"));
    // Command logs from before their redaction go, once per machine.
    gitti_core::redact::purge_unredacted_logs_in_config_dir();
    // Started from Finder / Dock: PATH etc. as in the user's terminal.
    shell_env::import_login_env();
    gitti_core::startup::mark("login environment read");
    // The git version probe (one process start, slow under a virus scanner) runs while
    // the GPU and the window come up; the first tab's `Repo::open` would wait for it.
    std::thread::spawn(|| {
        gitti_core::caps::GitCaps::get();
    });

    let path = std::env::args().nth(1).map(PathBuf::from);
    // Old read-only file versions ("Open This Version in Editor") of earlier sessions.
    std::thread::spawn(|| {
        use gitti_core::version_file::{MAX_AGE, cleanup_copies, copies_root};
        cleanup_copies(&copies_root(), MAX_AGE)
    });

    // Credential prompts from git/ssh are forwarded to the UI through this channel.
    let (ask_tx, ask_rx) = async_channel::unbounded::<app::AskRequest>();
    let askpass = gitti_core::askpass::AskpassServer::start(Arc::new(move |prompt| {
        let (reply, answer) = std::sync::mpsc::channel();
        let asker = prompt.asker.clone();
        ask_tx
            .send_blocking(app::AskRequest { prompt, reply })
            .ok()?;
        // A helper that went away: its dialog closes itself (`GittiApp::show_ask`).
        asker.wait(&answer, || ())
    }));
    let askpass_env = match &askpass {
        Ok(server) => server.env(),
        Err(e) => {
            eprintln!("gitti: askpass unavailable: {e:#}");
            Vec::new()
        }
    };
    gitti_core::startup::mark("askpass ready");

    gpui_kit::application()
        .with_assets(assets::GittiAssets)
        .run(move |cx| {
            gitti_core::startup::mark("GPU / window system ready");
            init(cx);
            if !scripted {
                note_system_keyboard(cx);
            }
            gitti_core::startup::mark("keys, menus, theme");
            // Settings writes are queued on a background thread (`store`): let the last
            // ones land before the process ends.
            cx.on_app_quit(|_| async {
                gitti_core::store::flush();
                // Closed before the start was complete: keep what was measured.
                gitti_core::startup::abandon();
            })
            .detach();
            app_icon::set_dock_icon();

            // Scripted runs (tests, screenshots) must not pop up on the user's screen.
            if scripted {
                probe::enable();
            }
            // `GITTI_SCRIPT_PLACEMENT=1`: a scripted run opens where a normal start would
            // (saved placement, e.g. maximized) — for tests of the restore path.
            let placed = std::env::var_os("GITTI_SCRIPT_PLACEMENT").is_some();
            let (bounds, display_id) = if scripted && !placed {
                // Far outside the visible screen area; still drawn, so screenshots work.
                let b = Bounds::new(point(px(-6000.), px(-6000.)), size(px(1500.), px(950.)));
                (WindowBounds::Windowed(b), None)
            } else {
                window_placement::initial("main", size(px(1500.), px(950.)), cx)
            };
            let options = WindowOptions {
                window_bounds: Some(bounds),
                display_id,
                focus: !scripted,
                titlebar: Some(TitlebarOptions {
                    title: Some("gitti".into()),
                    ..Default::default()
                }),
                app_id: Some(app_icon::APP_ID.into()),
                ..Default::default()
            };
            let (window, view) = gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| {
                    workspace::Workspace::new(
                        path.clone(),
                        askpass_env.clone(),
                        ask_rx.clone(),
                        scripted,
                        window,
                        cx,
                    )
                })
            })
            .expect("failed to open window");
            if !scripted {
                cx.activate(true);
            }
            if gitti_core::startup::is_recording() {
                gitti_core::startup::mark("window open");
                workspace::Workspace::watch_first_frame(window, view.clone(), cx);
            }
            if let Ok(steps) = std::env::var("GITTI_SCRIPT") {
                script::run(steps, window, view, cx);
            }
        });
    drop(askpass);
}

/// Globals, theme, key bindings and menus — everything before the first window. Also used by
/// the headless UI tests.
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    tooltip_gate::init(cx);
    menu_height::init(cx);
    Theme::change(ThemeMode::Dark, None, cx);
    cx.set_global(theme::Palette::dark());
    theme::apply_to_components(&theme::Palette::dark(), cx);

    // The kit's own bindings (text fields, menus), kept when the keyboard layout changes.
    let kit: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().cloned().collect();
    // `secondary` = cmd on macOS, ctrl on Windows/Linux. Written for a US keyboard; the
    // keyboard layout setting moves them (`set_keyboard_layout`).
    let mut us = vec![
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-r", Refresh, None),
        KeyBinding::new("f5", Refresh, None),
        KeyBinding::new("secondary-shift-f", Fetch, None),
        KeyBinding::new("secondary-alt-down", Pull, None),
        KeyBinding::new("secondary-alt-up", Push, None),
        KeyBinding::new("secondary-p", CommandPalette, None),
        KeyBinding::new("secondary-shift-p", CommandPalette, None),
        KeyBinding::new("secondary-b", SwitchBranch, None),
        // ⌘⇧B / Ctrl+Shift+B: create branch.
        KeyBinding::new("secondary-shift-b", CreateBranch, None),
        // Add Worktree… (W for worktree; ⌘W closes the tab, ⌘⇧W is whitespace). gitty:
        // ctrl-alt-w.
        KeyBinding::new("secondary-alt-w", AddWorktree, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        KeyBinding::new("secondary-o", OpenRepository, None),
        KeyBinding::new("secondary-w", CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("secondary-shift-]", NextTab, None),
        KeyBinding::new("secondary-shift-[", PrevTab, None),
        KeyBinding::new("secondary-shift-g", RunGitCommand, None),
        KeyBinding::new("up", CmdHistoryPrev, Some("GittiCommand > Input")),
        KeyBinding::new("down", CmdHistoryNext, Some("GittiCommand > Input")),
        KeyBinding::new("secondary-shift-l", ToggleGitLog, None),
        KeyBinding::new("alt-z", ToggleWrap, None),
        KeyBinding::new("secondary-shift-w", ToggleWhitespace, None),
        KeyBinding::new("escape", ModalCancel, Some("GittiModal > Input")),
        KeyBinding::new("secondary-shift-s", StashChanges, None),
        KeyBinding::new("secondary-enter", Commit, None),
        KeyBinding::new("secondary-t", ToggleOrder, None),
        KeyBinding::new("secondary-l", FocusMessage, None),
        KeyBinding::new("secondary-=", ZoomIn, None),
        KeyBinding::new("secondary-+", ZoomIn, None),
        KeyBinding::new("secondary--", ZoomOut, None),
        KeyBinding::new("secondary-alt-=", ZoomDiffIn, None),
        // German and other layouts have + on its own key.
        KeyBinding::new("secondary-alt-+", ZoomDiffIn, None),
        KeyBinding::new("secondary-alt--", ZoomDiffOut, None),
        // macOS takes ⌥⌘= / ⌥⌘− itself while the Accessibility zoom shortcuts are on;
        // ⌃⌘ always reaches the app (the hints show these on macOS).
        KeyBinding::new("cmd-ctrl-=", ZoomDiffIn, None),
        KeyBinding::new("cmd-ctrl-+", ZoomDiffIn, None),
        KeyBinding::new("cmd-ctrl--", ZoomDiffOut, None),
        KeyBinding::new("secondary-0", ResetZoom, None),
        KeyBinding::new("escape", ModalCancel, Some("GittiModal")),
        KeyBinding::new("escape", FocusCommitList, Some("DiffView")),
        KeyBinding::new("escape", EndCompare, Some("CommitList")),
        KeyBinding::new("escape", EndCompare, Some("Sidebar")),
        KeyBinding::new("secondary-c", CopySelection, Some("DiffView")),
        // Panes and the sidebar by keyboard, as in gitty: tab cycles sidebar → commit
        // list → diff (→ git log while shown); ← / h and → / l / ⏎ move between
        // neighbours. The list / scroll keys of the panes come from `gitti_core::keys`.
        KeyBinding::new("tab", NextPane, Some("Sidebar")),
        KeyBinding::new("tab", NextPane, Some("CommitList")),
        KeyBinding::new("tab", NextPane, Some("DiffView")),
        KeyBinding::new("tab", NextPane, Some("GitLog")),
        KeyBinding::new("shift-tab", PrevPane, Some("Sidebar")),
        KeyBinding::new("shift-tab", PrevPane, Some("CommitList")),
        KeyBinding::new("shift-tab", PrevPane, Some("DiffView")),
        KeyBinding::new("shift-tab", PrevPane, Some("GitLog")),
        KeyBinding::new("enter", SideActivate, Some("Sidebar")),
        KeyBinding::new("space", SideActivate, Some("Sidebar")),
        KeyBinding::new("right", FocusCommitList, Some("Sidebar")),
        KeyBinding::new("l", FocusCommitList, Some("Sidebar")),
        KeyBinding::new("left", FocusSidebar, Some("CommitList")),
        KeyBinding::new("h", FocusSidebar, Some("CommitList")),
        KeyBinding::new("right", FocusDiff, Some("CommitList")),
        KeyBinding::new("l", FocusDiff, Some("CommitList")),
        KeyBinding::new("enter", FocusDiff, Some("CommitList")),
        KeyBinding::new("left", FocusCommitList, Some("DiffView")),
        KeyBinding::new("h", FocusCommitList, Some("DiffView")),
    ];
    // The single-key commands and the list / scroll keys shared with gitty
    // (`gitti_core::keys`).
    us.extend(shared_key_bindings(cx));
    // Before the list keys: a picker's query field takes ⌘Home / ⌘End as its list's ends.
    us.extend(text_end_bindings(cx));
    us.extend(nav_key_bindings());
    us.extend(graph_key_bindings());
    us.extend(merge_window::key_bindings());
    cx.set_global(AppKeys {
        kit,
        us,
        layout: gitti_core::keys::KeyboardLayout::Us,
    });
    cx.on_action(|_: &Quit, cx| cx.quit());
    let saved = gitti_core::store::load_settings::<gitti_core::keys::KeyboardSettings>();
    set_keyboard_layout(saved.keyboard_layout, cx);
}

/// The keyboard layout's one-time hint: the system keyboard is one the layout setting
/// knows, the setting is still US and the hint wasn't shown yet → the first tab says so
/// (`GittiApp::sync_keyboard_layout`).
fn note_system_keyboard(cx: &mut App) {
    use gitti_core::keys::{KeyboardLayout, KeyboardSettings};
    let saved = gitti_core::store::load_settings::<KeyboardSettings>();
    let system = KeyboardLayout::from_system_id(cx.keyboard_layout().id())
        .or_else(|| KeyboardLayout::from_system_id(cx.keyboard_layout().name()));
    if saved.keyboard_layout == KeyboardLayout::Us
        && !saved.keyboard_layout_hint
        && let Some(system) = system
        && system.char_for('[') != '['
    {
        cx.set_global(KeyboardHint(Some(system)));
    }
}

/// The system keyboard layout to mention once (see [`note_system_keyboard`]).
pub(crate) struct KeyboardHint(pub Option<gitti_core::keys::KeyboardLayout>);

impl Global for KeyboardHint {}

/// gitti's key bindings as written (US keyboard) and the kit's, for [`set_keyboard_layout`].
struct AppKeys {
    kit: Vec<KeyBinding>,
    us: Vec<KeyBinding>,
    layout: gitti_core::keys::KeyboardLayout,
}

impl Global for AppKeys {}

/// The keyboard layout the shortcuts follow (`gitti_core::keys::layout`).
pub(crate) fn keyboard_layout(cx: &App) -> gitti_core::keys::KeyboardLayout {
    cx.try_global::<AppKeys>()
        .map(|k| k.layout)
        .unwrap_or_default()
}

/// Binds every key for `layout`: the keys its keyboard can't type move to ones it can
/// (`[` → `ö` on German); the menus, palette, tooltips and shortcut list follow (they read
/// the key map). Doesn't save it (`GittiApp::choose_keyboard_layout` does).
pub(crate) fn set_keyboard_layout(layout: gitti_core::keys::KeyboardLayout, cx: &mut App) {
    let keys = cx.global::<AppKeys>();
    let bindings: Vec<KeyBinding> = keys
        .kit
        .iter()
        .cloned()
        .chain(keys.us.iter().map(|b| remap_binding(b, layout)))
        .collect();
    cx.global_mut::<AppKeys>().layout = layout;
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
    set_menus(cx);
}

/// `b` with its keys as `layout` types them (the same binding when nothing moves).
fn remap_binding(b: &KeyBinding, layout: gitti_core::keys::KeyboardLayout) -> KeyBinding {
    let us: Vec<String> = b.keystrokes().iter().map(|k| k.inner().unparse()).collect();
    let mapped: Vec<String> = us.iter().map(|k| layout.remap(k).into_owned()).collect();
    if mapped == us {
        return b.clone();
    }
    KeyBinding::load(
        &mapped.join(" "),
        b.action().boxed_clone(),
        b.predicate(),
        false,
        b.action_input(),
        &DummyKeyboardMapper,
    )
    .unwrap_or_else(|e| {
        eprintln!("gitti: key {} on {layout:?}: {e}", mapped.join(" "));
        b.clone()
    })
}

/// Entry points for the headless UI tests (`tests/ui`).
pub mod testing {
    use std::path::PathBuf;

    use gpui_kit::*;

    pub use crate::app::SpellTarget;
    pub use crate::probe::Probe;
    pub use crate::workspace::Workspace;

    /// The main window's root view on `repo`, as in a scripted run (no saved tabs, no
    /// window placement, no askpass). Turns the layout probes on and the desktop sandbox
    /// (`gitti_core::desktop::sandbox`: no editor / file manager on the user's screen).
    pub fn workspace(repo: PathBuf, window: &mut Window, cx: &mut App) -> Entity<Workspace> {
        crate::probe::enable();
        gitti_core::desktop::sandbox();
        let (_, ask_rx) = async_channel::unbounded();
        cx.new(|cx| Workspace::new(Some(repo), Vec::new(), ask_rx, true, window, cx))
    }

    /// Tooltip views made visible so far and the visible ones up now; whether a context
    /// menu / dropdown is open (`tooltip_gate.rs`).
    pub fn tooltips(cx: &App) -> (usize, usize, bool) {
        let (shown, live) = crate::tooltip_gate::shown_tooltips(cx);
        (shown, live, crate::tooltip_gate::menu_open(cx))
    }

    /// The labels of the context-menu entries built since the last call (the sidebar menus
    /// against `gitti_core::sidebar_menus`).
    pub fn menu_labels() -> Vec<String> {
        crate::app::take_menu_labels()
    }

    /// Runs one `GITTI_SCRIPT` step (`cmd:arg`) in the main window, synchronously.
    /// `sleep`, `win`, `layout` and `check` are the test's business.
    pub fn step(ws: &Entity<Workspace>, step: &str, window: &mut Window, cx: &mut App) {
        let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
        crate::script::main_window_step(ws, cmd, arg, window, cx);
    }

    /// The probes of the frame drawn last.
    pub fn probes() -> Vec<Probe> {
        crate::probe::snapshot()
    }

    /// Every frame drawn since the last call or [`clear_probes`] (gpui's test mode also
    /// draws on effect flushes, so one update can draw several), the last one last.
    pub fn take_frames() -> Vec<Vec<Probe>> {
        crate::probe::take_frames()
    }

    pub fn clear_probes() {
        crate::probe::clear();
    }

    /// The layout invariants violated by `probes` (empty = fine).
    pub fn check(probes: &[Probe]) -> Vec<String> {
        crate::layout_check::check(probes)
    }

    pub use crate::layout_check::contains;
    /// A list's scrollbar strip: where its thumb is, what a press there does.
    pub use crate::scroll_guard::{HANDLE_BAND, Press, Strip};
    /// The family the diff, git log and file tabs ask for (per OS).
    pub use crate::theme::mono_font;

    /// The UI font family on the theme (gpui-component names the family `.SystemUIFont`
    /// resolves to, when that is an installed one).
    pub fn ui_font_family(cx: &App) -> SharedString {
        use gpui_kit::component::ActiveTheme as _;
        cx.theme().font_family.clone()
    }

    /// Sizes derived from the active tab's settings (see `GittiApp::script_metrics`).
    #[derive(Clone, Debug)]
    pub struct Metrics {
        pub message_line_h: f32,
        pub diff_row_h: f32,
        pub commit_row_h: f32,
        pub commit_box_min_h: f32,
        /// Selected commit list row.
        pub selected_row: usize,
        /// Diff list scroll position (px from the top).
        pub diff_scroll_y: f32,
        /// Lines in the diff line selection; bytes of the selected text (or lines).
        pub selected_lines: usize,
        pub text_selected: usize,
        /// Kind of every diff row (`DiffRow::kind_name`).
        pub diff_rows: Vec<&'static str>,
    }

    /// Scroll state of the active tab (see `GittiApp::script_scroll`); offsets in px from
    /// the top, maxima as of the last layout.
    #[derive(Clone, Debug, Default)]
    pub struct Scroll {
        /// Sidebar offset from the top and largest offset (px).
        pub side_y: f32,
        pub side_max: f32,
        pub commit_y: f32,
        pub commit_max: f32,
        /// Commit list rows; the full history (not the quick first screen) is loaded.
        pub commits: usize,
        pub history_full: bool,
        /// Id of the selected commit (`None` = the worktree row).
        pub selected_id: Option<String>,
        pub diff_y: f32,
        pub diff_max: f32,
        pub diff_viewport_h: f32,
        pub log_y: f32,
        pub log_max: f32,
        pub log_open: bool,
        /// How often the git log panel filtered the command log (once per change).
        pub log_builds: u64,
        /// Git log entries after the last one shown ("N newer below"), and whether the
        /// panel follows new entries (at the newest end).
        pub log_newer: usize,
        pub log_following: bool,
        /// Open text window: (first line = scroll offset in lines, end of the rows shown,
        /// lines).
        pub text_view: Option<(usize, usize, usize)>,
        /// Settings content offset from the top and largest offset (px).
        pub settings_y: f32,
        pub settings_max: f32,
        /// Commit message editor.
        pub message_y: f32,
        /// Open picker: (selected entry, entries shown).
        pub picker: Option<(usize, usize)>,
        /// Collapsed sidebar groups, sorted.
        pub collapsed: Vec<String>,
        pub wrap: bool,
    }

    pub fn scroll(ws: &Entity<Workspace>, cx: &App) -> Scroll {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_scroll(cx)
    }

    /// The source line at the top of the active tab's diff (`view_anchor.rs`): path,
    /// new-version number, wrapped part, its row's top relative to the list's top (px).
    pub fn diff_top_line(ws: &Entity<Workspace>, cx: &App) -> Option<(String, u32, u16, f32)> {
        let app = ws.read(cx).active_app()?;
        app.read(cx).script_top_line()
    }

    /// Identity of every diff row of the active tab (`GittiApp::script_diff_row_ids`).
    pub fn diff_row_ids(ws: &Entity<Workspace>, cx: &App) -> Vec<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_diff_row_ids()
    }

    /// The merge editor window opened last (`mergeeditor:N`).
    pub fn merge_window() -> Option<AnyWindowHandle> {
        crate::merge_window::LAST_WINDOW.with(|w| *w.borrow())
    }

    /// Scroll state of the last merge editor: (first top row, first result line, result line
    /// matching that top row, top at its end).
    pub fn merge_scroll(cx: &App) -> Option<(f32, f32, f32, bool)> {
        let editor = crate::merge_window::LAST_EDITOR.with(|e| e.borrow().clone())?;
        Some(editor.upgrade()?.read(cx).script_scroll_state(cx))
    }

    /// The git commands the active tab ran in this session so far (args joined by spaces),
    /// oldest first — take the length before an action to see what it ran.
    pub fn git_commands(ws: &Entity<Workspace>, cx: &App) -> Vec<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_git_commands()
    }

    pub use crate::app::ChangeSearchState;

    /// The active tab's Search in Changes: open, text, hits, note, scans started.
    pub fn change_search(ws: &Entity<Workspace>, cx: &App) -> ChangeSearchState {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_change_search_state(cx)
    }

    /// The working state the active tab shows: one `U|S|C path hunks` line per file.
    pub fn working(ws: &Entity<Workspace>, cx: &App) -> Vec<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_working()
    }

    /// Rows of the active tab's history and how it was loaded (`HistoryLoad`).
    pub fn history(ws: &Entity<Workspace>, cx: &App) -> (usize, String) {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_history()
    }

    /// Holds history loads back (they wait until released) — to see that the working
    /// state doesn't wait for them.
    pub fn hold_history(hold: bool) {
        crate::app::HOLD_HISTORY.store(hold, std::sync::atomic::Ordering::Relaxed);
    }

    /// The words of the active tab's `target` editor (commit message, a dialog's text area
    /// or field) marked misspelled (spell checking).
    pub fn spell_marks(ws: &Entity<Workspace>, target: SpellTarget, cx: &App) -> Vec<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_spell_marks(target, cx)
    }

    /// Puts `target`'s cursor at byte `offset`, then what its right-click menu offers
    /// there: the misspelled word and its suggestions.
    pub fn spell_at(
        ws: &Entity<Workspace>,
        target: SpellTarget,
        offset: usize,
        cx: &mut App,
    ) -> Option<(String, Vec<String>)> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.update(cx, |a, cx| a.script_spell_cursor(target, offset, cx));
        app.read(cx).script_spell_at_cursor(target, cx)
    }

    /// Chooses suggestion `index` of `target`'s right-click menu at the cursor (dispatches
    /// its action like the menu does); false if there is none.
    pub fn spell_choose(
        ws: &Entity<Workspace>,
        target: SpellTarget,
        index: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let app = ws.read(cx).active_app().expect("a repository tab");
        let Some(action) = app.read(cx).script_spell_choose(target, index, cx) else {
            return false;
        };
        window.dispatch_action(action, cx);
        true
    }

    /// The last right-click menu of a spell-checked editor (taken): its editor and the
    /// misspelled word it offered.
    pub fn take_editor_menu() -> Option<(SpellTarget, Option<String>)> {
        crate::app::take_last_editor_menu()
    }

    /// Puts `text` into the text area of the active tab's open dialog (without confirming).
    pub fn modal_text(ws: &Entity<Workspace>, text: &str, window: &mut Window, cx: &mut App) {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.update(cx, |a, cx| a.script_modal_text(text, window, cx));
    }

    /// The working-directory file tabs of the active tab: labels (`All Changes (n)`, then
    /// `<letter> <path>[ ●|◐]`) and the active tab's path (`None` = all).
    pub fn worktree_tabs(ws: &Entity<Workspace>, cx: &App) -> (Vec<String>, Option<String>) {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_worktree_tabs()
    }

    /// Every shortcut tooltip (`shortcuts::ALL`): its label and the keys it names, read
    /// from the key map `init` bound, and its whole text.
    pub fn tooltip_shortcuts(cx: &App) -> Vec<(&'static str, Vec<String>, String)> {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        crate::shortcuts::ALL
            .iter()
            .map(|t| (t.label, t.keys(&keymap), t.text(cx).to_string()))
            .collect()
    }

    /// The shortcut list (settings ▸ Keyboard Shortcuts) as `keys — what` lines, headings
    /// as `# title` / `## group`.
    pub fn shortcut_list(cx: &App) -> Vec<String> {
        use crate::app::ShortcutLine;
        crate::app::shortcut_lines(cx)
            .into_iter()
            .map(|l| match l {
                ShortcutLine::Heading(t) => format!("# {t}"),
                ShortcutLine::Group(t) => format!("## {t}"),
                ShortcutLine::Keys(k, w) => format!("{k} — {w}"),
            })
            .collect()
    }

    /// A diff context-menu entry as it is labelled: `label` and the keys of the action
    /// `name` in the diff (`shortcuts::menu_label`).
    pub fn diff_menu_label(label: &str, name: &str, cx: &App) -> String {
        let action = cx.build_action(name, None).expect("action");
        crate::shortcuts::menu_label(label, action.as_ref(), "DiffView", cx)
    }

    /// The merge window's key hints (tooltips, status texts), read from its bindings.
    pub fn merge_hints(cx: &App) -> crate::merge_window::Hints {
        (*crate::merge_window::Hints::get(cx)).clone()
    }

    /// The key hint of every bound action as a palette entry shows it (action name, hint);
    /// actions bound only in a context the hint leaves out show an empty one.
    pub fn bound_key_hints(cx: &App) -> Vec<(String, String)> {
        let actions: Vec<Box<dyn Action>> = {
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let mut seen = std::collections::HashSet::new();
            keymap
                .bindings()
                .filter(|b| seen.insert(b.action().name()))
                .map(|b| b.action().boxed_clone())
                .collect()
        };
        actions
            .iter()
            .map(|a| (a.name().to_string(), crate::shortcuts::hint(a.as_ref(), cx)))
            .collect()
    }

    /// The key hint a palette entry for the action `name` (`gitti::Fetch`) shows.
    pub fn key_hint(name: &str, cx: &App) -> String {
        let action = cx.build_action(name, None).expect("action");
        crate::shortcuts::hint(action.as_ref(), cx)
    }

    /// The keyboard layout the shortcuts follow (`de`, …).
    pub fn keyboard_layout(cx: &App) -> &'static str {
        crate::keyboard_layout(cx).id()
    }

    /// Text edge smoothing: the saved setting's id and gpui's live mode (`Debug`).
    pub fn text_antialiasing(cx: &App) -> (&'static str, String) {
        (
            crate::theme::Settings::load().text_antialiasing.id(),
            format!("{:?}", cx.text_rendering_mode()),
        )
    }

    /// Binds the keys for layout `id` (not saved), as choosing it in the settings does.
    pub fn set_keyboard_layout(id: &str, cx: &mut App) {
        let layout = gitti_core::keys::KeyboardLayout::ALL
            .into_iter()
            .find(|l| l.id() == id)
            .expect("layout id");
        crate::set_keyboard_layout(layout, cx);
    }

    /// Keys bound twice in the same context to different actions, as `keys [context]:
    /// action, action` (the kit's bindings included).
    pub fn key_clashes(cx: &App) -> Vec<String> {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let mut seen: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for b in keymap.bindings() {
            let keys: Vec<String> = b.keystrokes().iter().map(|k| k.inner().unparse()).collect();
            let ctx = b.predicate().map(|p| p.to_string()).unwrap_or_default();
            let actions = seen.entry(format!("{} [{ctx}]", keys.join(" "))).or_default();
            let name = b.action().name().to_string();
            if !actions.contains(&name) {
                actions.push(name);
            }
        }
        seen.into_iter()
            .filter(|(_, a)| a.len() > 1)
            .map(|(k, a)| format!("{k}: {}", a.join(", ")))
            .collect()
    }

    /// gitti's own bound keys the current layout can't type with one key (Shift allowed).
    pub fn untypeable_keys(cx: &App) -> Vec<String> {
        let layout = crate::keyboard_layout(cx);
        let mac = cfg!(target_os = "macos");
        let mut out = Vec::new();
        for b in &cx.global::<crate::AppKeys>().us {
            for k in crate::remap_binding(b, layout).keystrokes() {
                let key = &k.inner().key;
                let mut chars = key.chars();
                if let (Some(c), None) = (chars.next(), chars.next())
                    && !c.is_ascii_alphabetic()
                    && !layout.types(c, mac)
                {
                    out.push(k.inner().unparse());
                }
            }
        }
        out
    }

    /// No operation, refresh or reload of the active tab is running or queued (true without
    /// a repository tab).
    pub fn idle(ws: &Entity<Workspace>, cx: &App) -> bool {
        ws.read(cx)
            .active_app()
            .is_none_or(|app| app.read(cx).script_idle())
    }

    /// The active tab's busy area once it shows: `Commit… [meter] | last output line`
    /// (the status bar, `app/op_progress.rs`).
    pub fn busy_text(ws: &Entity<Workspace>, cx: &App) -> Option<String> {
        let app = ws.read(cx).active_app()?;
        app.read(cx).script_busy_text()
    }

    /// Whether the active tab's output window of the running operation is open.
    pub fn busy_output_open(ws: &Entity<Workspace>, cx: &App) -> bool {
        ws.read(cx)
            .active_app()
            .is_some_and(|app| app.read(cx).script_output_open())
    }

    /// Shows the active tab's busy area as if operation `label` ran and had printed
    /// `lines` (`None`: idle again) — see `GittiApp::script_show_progress`.
    pub fn show_progress(ws: &Entity<Workspace>, busy: Option<(&str, &[&str])>, cx: &mut App) {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.update(cx, |app, cx| app.script_show_progress(busy, cx));
    }

    /// The submodules the active tab knows (`path state`, nested after their parent).
    pub fn submodules(ws: &Entity<Workspace>, cx: &App) -> Vec<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_submodules()
    }

    /// The subtrees the active tab knows (`prefix [squash] [removed] update [source]`).
    pub fn subtrees(ws: &Entity<Workspace>, cx: &App) -> Vec<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_subtrees()
    }

    /// The worktrees the active tab knows (`name checkout [current] [main] [locked]
    /// [prunable]`, main first).
    pub fn worktrees(ws: &Entity<Workspace>, cx: &App) -> Vec<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_worktrees()
    }

    /// The open folder picker: its rows (`[>]path on|off|implied|partial [+|-]`),
    /// the field text it would write, the checked folders not in the tree.
    pub fn folder_picker(
        ws: &Entity<Workspace>,
        cx: &App,
    ) -> Option<(Vec<String>, String, Vec<String>)> {
        ws.read(cx).script_app()?.read(cx).script_folder_rows()
    }

    /// The message of the open dialog (active tab or start page).
    pub fn modal_message(ws: &Entity<Workspace>, cx: &App) -> Option<String> {
        ws.read(cx).script_app()?.read(cx).script_modal_message()
    }

    /// The text of the open text window (About, Start-up Timings, …).
    pub fn modal_viewer_text(ws: &Entity<Workspace>, cx: &App) -> Option<String> {
        ws.read(cx).script_app()?.read(cx).script_modal_viewer_text(cx)
    }

    /// The active tab's repository directory.
    pub fn workdir(ws: &Entity<Workspace>, cx: &App) -> PathBuf {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_workdir()
    }

    /// The status bar text when it shows an error.
    pub fn error(ws: &Entity<Workspace>, cx: &App) -> Option<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_error()
    }

    /// Git LFS health: replaces the active tab's last check by `health` (the detection is
    /// core's business; this drives the banner, fixes and push warning).
    pub fn set_lfs_health(
        ws: &Entity<Workspace>,
        health: gitti_core::lfs_health::LfsHealth,
        cx: &mut App,
    ) {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.update(cx, |a, cx| {
            a.set_lfs_health(health);
            cx.notify();
        });
    }

    /// The Git LFS problems the banner shows (`short` each; empty: no banner).
    pub fn lfs_issues(ws: &Entity<Workspace>, cx: &App) -> Vec<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_lfs_issues()
    }

    /// The commit message box's text.
    pub fn commit_message(ws: &Entity<Workspace>, cx: &App) -> String {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_commit_message(cx)
    }

    /// The commit message draft stored for the active tab's worktree (a db read).
    pub fn commit_draft(ws: &Entity<Workspace>, cx: &App) -> Option<String> {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_draft()
    }

    /// Puts the active tab back to its first frame with panes, with the body last measured
    /// at `body_width` in a window `viewport_width` wide (fresh resize states). A real
    /// window gets there when it changes size right after its first frame (Windows
    /// maximizing a restored window); the test platform draws those frames synchronously.
    pub fn restart_panes(
        ws: &Entity<Workspace>,
        body_width: f32,
        viewport_width: f32,
        cx: &mut App,
    ) {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.update(cx, |a, cx| a.script_restart_panes(body_width, viewport_width, cx));
    }

    pub fn metrics(ws: &Entity<Workspace>, cx: &App) -> Metrics {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_metrics()
    }

    /// What the keys of the active tab act on (see `GittiApp::script_state`).
    #[derive(Clone, Debug)]
    pub struct UiState {
        /// `list`, `sidebar`, `diff`, `message`, `command`, `settings`, `picker`, `modal`,
        /// `other`, `none`.
        pub focus: &'static str,
        /// `empty`, `worktree`, `loading`, `commit`, `compare`.
        pub detail: &'static str,
        /// Title of the open in-window dialog.
        pub modal: Option<String>,
        pub picker: Option<PickerState>,
        pub settings_open: bool,
        pub settings_section: String,
        pub show_git_log: bool,
        pub wrap_lines: bool,
        pub ignore_whitespace: bool,
        pub topo_order: bool,
        pub diff_context: u32,
        pub ui_font: f32,
        pub diff_font: f32,
        pub amend: bool,
        /// Commit message editor and git command line contents.
        pub message: String,
        pub cmd_input: String,
        /// Status bar text and whether it is an error.
        pub status: Option<(String, bool)>,
        /// The toolbar's error mark ("⚠ Pull failed").
        pub error_mark: Option<String>,
        pub busy: bool,
        pub branch: Option<String>,
        pub commits: usize,
        /// Id of the selected commit (`None` for the worktree row).
        pub selected_commit: Option<String>,
        /// Paths in the worktree view's unstaged / staged lists.
        pub unstaged: Vec<String>,
        pub staged: Vec<String>,
        pub stashes: usize,
        /// Every diff row: `kind src path [line kind]`, e.g. `line Unstaged big.txt Added`.
        pub rows: Vec<String>,
        /// Lines in the diff line selection.
        pub selected_lines: usize,
        /// Sidebar keyboard cursor (entry index) and the collapsed sidebar groups, sorted.
        pub side_cursor: Option<usize>,
        pub collapsed: Vec<String>,
        /// Compare mode: `base=<id>[ swapped][ shown=<old>..<new>]` (`None` = off).
        pub compare: Option<String>,
    }

    /// The open picker: labels of the shown items (best match first), the selected one,
    /// the typed query.
    #[derive(Clone, Debug)]
    pub struct PickerState {
        pub labels: Vec<String>,
        /// The menu action each shown entry stands for (`gitti::Fetch`).
        pub actions: Vec<Option<&'static str>>,
        /// The muted text after each shown entry's label (what it acts on).
        pub details: Vec<String>,
        /// Each shown entry's keys, the right-aligned column (empty = none).
        pub keys: Vec<String>,
        /// Search pickers: why each shown item matched.
        pub tags: Vec<Option<String>>,
        /// The palette: why each shown entry's command can't act now.
        pub disabled: Vec<Option<String>>,
        /// Search pickers: the line above the results.
        pub note: Option<String>,
        /// A search is still running.
        pub searching: bool,
        pub selected: usize,
        pub query: String,
    }

    pub fn state(ws: &Entity<Workspace>, window: &Window, cx: &App) -> UiState {
        let app = ws.read(cx).active_app().expect("a repository tab");
        app.read(cx).script_state(window, cx)
    }

    /// (number of repository tabs, active tab).
    pub fn tabs(ws: &Entity<Workspace>, cx: &App) -> (usize, usize) {
        ws.read(cx).script_tabs()
    }

    /// The repository tabs' paths in their order.
    pub fn tab_paths(ws: &Entity<Workspace>, cx: &App) -> Vec<PathBuf> {
        ws.read(cx).script_tab_paths()
    }

    /// Restores `repos` as tabs the way a start does, the `active`th of them shown.
    pub fn restore_tabs(
        ws: &Entity<Workspace>,
        repos: &[PathBuf],
        active: usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        ws.update(cx, |w, cx| w.script_restore(repos, active, window, cx));
    }

    /// Per tab `name waiting|loading|loaded` (restored tabs read their repository in turn).
    pub fn tab_loads(ws: &Entity<Workspace>, cx: &App) -> Vec<String> {
        ws.read(cx).script_tab_loads(cx)
    }

    /// State of the last merge editor (see `MergeEditor::script_state`).
    #[derive(Clone, Debug)]
    pub struct MergeState {
        /// Chunk navigated to last.
        pub current: Option<usize>,
        /// Kind of every chunk (`Stable`, `Conflict`, …) and its state (`Unresolved`,
        /// `Taken(Ours)`, …).
        pub chunks: Vec<(String, String)>,
        /// Result only (ours / theirs folded).
        pub single: bool,
        pub status: Option<(String, bool)>,
        /// The ours / theirs panes have the focus.
        pub top_focused: bool,
        pub text: String,
    }

    pub fn merge_state(cx: &mut App) -> Option<MergeState> {
        let window = merge_window()?;
        let editor = crate::merge_window::LAST_EDITOR.with(|e| e.borrow().clone())?.upgrade()?;
        window
            .update(cx, |_, window, cx| editor.read(cx).script_state(window))
            .ok()
    }

    /// Gives the merge editor's ours / theirs panes the focus (as a click into them does).
    pub fn merge_focus_top(cx: &mut App) {
        let (Some(window), Some(editor)) = (
            merge_window(),
            crate::merge_window::LAST_EDITOR.with(|e| e.borrow().clone()),
        ) else {
            return;
        };
        window
            .update(cx, |_, window, cx| {
                editor
                    .update(cx, |e, cx| e.script_focus_top(window, cx))
                    .ok()
            })
            .ok();
    }
}

/// Test entry points of the menu bar and the dialogs.
pub mod testing_menus {
    use gpui_kit::*;

    use crate::workspace::Workspace;

    /// Every menu-bar entry as (menu, label, action name, e.g. `gitti::Fetch`).
    pub fn menu_items() -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for m in crate::menu_bar() {
            for it in m.items {
                if let MenuItem::Action { name, action, .. } = it {
                    out.push((m.name.to_string(), name.to_string(), action.name().to_string()));
                }
            }
        }
        out
    }

    /// The menu bar as it is shown now: (menu, label as shown, action, reason when greyed —
    /// then the label ends with it).
    pub fn live_menu_items() -> Vec<crate::LiveMenuItem> {
        crate::LIVE_MENUS.with(|l| l.borrow().clone())
    }

    /// Runs the action `name` in the main window as its menu item does.
    pub fn dispatch(name: &str, window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
        let action = cx.build_action(name, None)?;
        window.dispatch_action(action, cx);
        Ok(())
    }

    /// Title and field values of the open dialog of the active tab (or the start page).
    pub fn modal(ws: &Entity<Workspace>, cx: &App) -> Option<(String, Vec<String>)> {
        let app = ws.read(cx).script_app()?;
        let app = app.read(cx);
        Some((app.script_modal_title()?, app.script_modal_fields(cx)))
    }

    /// The note shown under field `ix` of the open dialog (a prefilled field's "From the
    /// clipboard").
    pub fn modal_note(ws: &Entity<Workspace>, ix: usize, cx: &App) -> Option<String> {
        ws.read(cx).script_app()?.read(cx).script_modal_note(ix, cx)
    }

    /// The selected range (bytes) of field `ix` of the open dialog.
    pub fn modal_selection(
        ws: &Entity<Workspace>,
        ix: usize,
        cx: &App,
    ) -> Option<std::ops::Range<usize>> {
        ws.read(cx).script_app()?.read(cx).script_modal_selection(ix, cx)
    }

    /// [`modal_live`]'s answer: (text, OK label, why OK is disabled, completion buttons'
    /// values).
    pub type ModalLive = (String, Option<String>, Option<String>, Vec<String>);

    /// What the open dialog says live (`ModalSpec::live`).
    pub fn modal_live(ws: &Entity<Workspace>, cx: &App) -> Option<ModalLive> {
        let l = ws.read(cx).script_app()?.read(cx).script_modal_live()?;
        Some((l.text, l.ok_label, l.blocked, l.fills.into_iter().map(|f| f.1).collect()))
    }

    /// The chosen option of the open dialog (`ModalSpec::options`).
    pub fn modal_option(ws: &Entity<Workspace>, cx: &App) -> Option<usize> {
        ws.read(cx).script_app()?.read(cx).script_modal_option()
    }

    /// Whether the active tab shows a repository (else the start / "not a repository" page).
    pub fn has_repo(ws: &Entity<Workspace>, cx: &App) -> bool {
        ws.read(cx)
            .active_app()
            .is_some_and(|a| a.read(cx).has_repo())
    }

    /// The status line of the active tab or the start page: (text, error).
    pub fn status(ws: &Entity<Workspace>, cx: &App) -> Option<(String, bool)> {
        let app = ws.read(cx).script_app()?;
        app.read(cx).script_status()
    }

    /// Heading and message of the active tab's "not a repository" page (a bare
    /// repository, no git, a plain folder).
    pub fn open_problem(ws: &Entity<Workspace>, cx: &App) -> Option<(String, String)> {
        let app = ws.read(cx).script_app()?;
        app.read(cx).script_open_problem()
    }
}

/// Whether menus are drawn in the window (always on Windows/Linux; `GITTI_MENU_BAR=1` forces
/// it on macOS for testing).
pub fn in_window_menu_bar() -> bool {
    !cfg!(target_os = "macos") || std::env::var_os("GITTI_MENU_BAR").is_some()
}

/// Whether the app draws the window frame itself (title bar drag area, window buttons): the
/// compositor does not (GNOME/Wayland, WSLg). `GITTI_WINDOW_FRAME=1` forces it for testing.
pub fn draws_window_frame(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
        || std::env::var_os("GITTI_WINDOW_FRAME").is_some()
}

// Menu-bar commands; the handlers live in `app/repo_actions.rs`, clone / init in
// `workspace.rs`.
actions!(
    gitti,
    [
        CloneRepository,
        InitRepository,
        OpenInFileManager,
        OpenTerminal,
        AddToGitignore,
        EditGitignore,
        TrackWithLfs,
        EditGitattributes,
        RepoGitSettings,
        GlobalGitSettings,
        EditRepoConfig,
        EditGlobalConfig,
        WriteCommitGraph,
        CreateBranch,
        CreateTag,
        MergeBranch,
        AddRemote,
        ApplyStash,
        PopStash,
        GoToWorkingDirectory,
        About,
        StartupTimings,
        AddWorktree,
        PruneWorktrees,
        RemoveThisWorktree,
        MoveThisWorktree,
        RepairWorktrees,
        AddSubmodule,
        UpdateSubmodules,
        UpdateSubmodulesRemote,
        CheckoutSubmoduleCommit,
        SyncSubmodules,
        RemoveSubmodule,
        ForeachSubmodule,
        AddSubtree,
        PullSubtree,
        MergeIntoSubtree,
        PushSubtree,
        SplitSubtree,
    ]
);

// More menu items; handlers in `app/menu_cmds.rs`, `app/navigate.rs`, Open Recent / Full
// Screen in `workspace.rs`.
actions!(
    gitti,
    [
        OpenRecent,
        ToggleFullScreen,
        ToggleSidebar,
        GoToCommit,
        SearchChanges,
        GoToRef,
        GoToHead,
        RenameCurrentBranch,
        DeleteBranchMenu,
        SetUpstreamMenu,
        RebaseCurrentOnto,
        RevertCommit,
        CherryPickCommit,
        EditCommitMessage,
        GetSigningKeys,
        DropLatestStash,
        DropStash,
        StashIncludingUntracked,
        OpenThisVersion,
        OpenOldVersion,
        StageAllAndCommit,
        CreateEmptyCommit,
    ]
);

// View ▸ Auto / One-Line / Two-Line Commit List; handlers in `app/commit_list.rs`.
actions!(gitti, [CommitListAuto, CommitListOneLine, CommitListTwoLines]);

// Remote commands with a choice: the palette's second step (a remote, or the push options
// dialog); handlers in `app/toolbar.rs`.
actions!(gitti, [PushTo, PullFrom, PullWith, FetchFrom]);

// The action of a greyed menu-bar entry. Nothing handles it, so macOS's menu validation
// shows the entry disabled too (it asks gpui whether the action is handled).
actions!(gitti, [MenuItemUnavailable]);

/// A menu-bar entry as shown: (menu, label, action, reason when greyed).
pub type LiveMenuItem = (String, String, String, Option<String>);

thread_local! {
    /// What the menu bar shows now.
    static LIVE_MENUS: std::cell::RefCell<Vec<LiveMenuItem>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// The shown tab's unavailable commands (the menu bar is built with them).
    static MENU_OFF: std::cell::RefCell<Option<Vec<(&'static str, &'static str)>>> =
        const { std::cell::RefCell::new(None) };
}

/// The menu-bar label of menu command `name` (`Fetch`; for the greyed-entry messages), else
/// `name`.
pub(crate) fn menu_label(name: &str) -> String {
    let full = format!("gitti::{name}");
    menu_bar()
        .into_iter()
        .flat_map(|m| m.items)
        .find_map(|it| match it {
            MenuItem::Action { name, action, .. } if action.name() == full => {
                Some(name.to_string())
            }
            _ => None,
        })
        .unwrap_or_else(|| name.to_string())
}

/// Every menu-bar action under its label reduced for comparing (`palette::normalize`):
/// the palette finds the action of an entry that has the menu's label.
pub(crate) fn menu_actions_by_label() -> Vec<(String, &'static str)> {
    menu_bar()
        .into_iter()
        .flat_map(|m| m.items)
        .filter_map(|it| match it {
            MenuItem::Action { name, action, .. } => {
                Some((gitti_core::palette::normalize(&name), action.name()))
            }
            _ => None,
        })
        .collect()
}

/// The menu bar with the commands of `off` greyed, their reason after the label.
fn menu_bar_with(off: &[(&'static str, &'static str)]) -> Vec<Menu> {
    let mut live = Vec::new();
    let menus = menu_bar()
        .into_iter()
        .map(|m| {
            let items = m
                .items
                .into_iter()
                .map(|it| match it {
                    MenuItem::Action { name, action, os_action, checked, .. } => {
                        let full = action.name();
                        let why = off
                            .iter()
                            .find(|(a, _)| full.strip_prefix("gitti::") == Some(a))
                            .map(|(_, w)| *w);
                        let shown = match why {
                            Some(why) => gitti_core::availability::with_reason(&name, why),
                            None => name.to_string(),
                        };
                        live.push((
                            m.name.to_string(),
                            shown.clone(),
                            full.to_string(),
                            why.map(str::to_string),
                        ));
                        match why {
                            Some(_) => MenuItem::Action {
                                name: shown.into(),
                                action: Box::new(MenuItemUnavailable),
                                os_action: None,
                                checked,
                                disabled: true,
                            },
                            None => MenuItem::Action { name, action, os_action, checked, disabled: false },
                        }
                    }
                    other => other,
                })
                .collect();
            Menu { name: m.name, items, disabled: m.disabled }
        })
        .collect();
    LIVE_MENUS.with(|l| *l.borrow_mut() = live);
    menus
}

/// Records `off`, the shown tab's unavailable commands (the workspace, each frame);
/// `true` when they changed — then the menu bar must be rebuilt ([`refresh_menus`]).
pub(crate) fn set_menu_off(off: &[(&'static str, &'static str)]) -> bool {
    MENU_OFF.with(|m| {
        let mut m = m.borrow_mut();
        if m.as_deref() == Some(off) {
            return false;
        }
        *m = Some(off.to_vec());
        true
    })
}

/// Why menu command `name` (`StageAll`) can't act in the shown tab now (the context menus
/// grey their entries of it with this reason).
pub(crate) fn why_not_now(name: &str) -> Option<&'static str> {
    MENU_OFF.with(|m| {
        m.borrow()
            .as_ref()?
            .iter()
            .find(|(a, _)| *a == name)
            .map(|(_, why)| *why)
    })
}

/// Rebuilds the menu bar with the commands of `off` greyed.
pub(crate) fn refresh_menus(off: &[(&'static str, &'static str)], cx: &mut App) {
    install_menus(menu_bar_with(off), cx);
}

fn menu(name: &str, items: Vec<MenuItem>) -> Menu {
    Menu {
        name: name.to_string().into(),
        items,
        disabled: false,
    }
}

/// The menus after the app menu (macOS) — File … Help. On Windows /
/// Linux `set_menus` adds Settings and Exit to File.
fn menu_bar() -> Vec<Menu> {
    use gitti_core::list_layout::CommitListLayout;
    let file_manager = gitti_core::desktop::file_manager_name();
    vec![
        menu(
            "File",
            vec![
                MenuItem::action("Open Repository…", OpenRepository),
                MenuItem::action("Open Recent…", OpenRecent),
                MenuItem::action("Clone Repository…", CloneRepository),
                MenuItem::action("Init Repository…", InitRepository),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::separator(),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Previous Tab", PrevTab),
            ],
        ),
        menu(
            "View",
            vec![
                MenuItem::action("Command Palette…", CommandPalette),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Toggle Full Screen", ToggleFullScreen),
                MenuItem::action("Git Command Log", ToggleGitLog),
                MenuItem::action("Last Error Details", ErrorDetails),
                MenuItem::action("Run Git Command…", RunGitCommand),
                MenuItem::separator(),
                MenuItem::action("Line Wrap in Diffs", ToggleWrap),
                MenuItem::action("Ignore Whitespace in Diffs", ToggleWhitespace),
                MenuItem::action("More Context Lines", MoreContext),
                MenuItem::action("Fewer Context Lines", LessContext),
                MenuItem::action("Show Whole File", ShowWholeFile),
                MenuItem::separator(),
                MenuItem::action("Bigger Font", ZoomIn),
                MenuItem::action("Smaller Font", ZoomOut),
                MenuItem::action("Bigger Diff Font", ZoomDiffIn),
                MenuItem::action("Smaller Diff Font", ZoomDiffOut),
                MenuItem::action("Reset Font Size", ResetZoom),
                MenuItem::separator(),
                MenuItem::action("Toggle Topo/Date Order", ToggleOrder),
                // (The palette marks the current choice, the menu can't.)
                MenuItem::action(CommitListLayout::Auto.label(), CommitListAuto),
                MenuItem::action(CommitListLayout::OneLine.label(), CommitListOneLine),
                MenuItem::action(CommitListLayout::TwoLines.label(), CommitListTwoLines),
                MenuItem::separator(),
                MenuItem::action("Refresh", Refresh),
            ],
        ),
        menu(
            "Repository",
            vec![
                MenuItem::action("Refresh", Refresh),
                MenuItem::separator(),
                MenuItem::action(format!("Open in {file_manager}"), OpenInFileManager),
                MenuItem::action("Open in Terminal", OpenTerminal),
                MenuItem::separator(),
                MenuItem::action("Add Worktree…", AddWorktree),
                MenuItem::action("Move This Worktree…", MoveThisWorktree),
                MenuItem::action("Remove This Worktree…", RemoveThisWorktree),
                MenuItem::action("Repair Worktrees…", RepairWorktrees),
                MenuItem::action("Prune Worktrees", PruneWorktrees),
                MenuItem::separator(),
                MenuItem::action("Add Submodule…", AddSubmodule),
                MenuItem::action("Update Submodules", UpdateSubmodules),
                MenuItem::action(
                    "Update Submodules to Remote Branches",
                    UpdateSubmodulesRemote,
                ),
                MenuItem::action("Check Out Commit in Submodule…", CheckoutSubmoduleCommit),
                MenuItem::action("Sync Submodule URLs", SyncSubmodules),
                MenuItem::action("Remove Submodule…", RemoveSubmodule),
                MenuItem::action("Run Command in Each Submodule…", ForeachSubmodule),
                MenuItem::separator(),
                MenuItem::action("Add Subtree…", AddSubtree),
                MenuItem::action("Pull Subtree…", PullSubtree),
                MenuItem::action("Merge into Subtree…", MergeIntoSubtree),
                MenuItem::action("Push Subtree…", PushSubtree),
                MenuItem::action("Split Subtree…", SplitSubtree),
                MenuItem::separator(),
                MenuItem::action("Add to .gitignore…", AddToGitignore),
                MenuItem::action("Edit .gitignore…", EditGitignore),
                MenuItem::action("Track with Git LFS…", TrackWithLfs),
                MenuItem::action("Edit .gitattributes…", EditGitattributes),
                MenuItem::separator(),
                MenuItem::action("Git Settings: This Repository…", RepoGitSettings),
                MenuItem::action("Git Settings: Global…", GlobalGitSettings),
                MenuItem::action("Edit Repository Config File…", EditRepoConfig),
                MenuItem::action("Edit Global Config File…", EditGlobalConfig),
                MenuItem::separator(),
                MenuItem::action("Run Git Command…", RunGitCommand),
                MenuItem::action("Write Commit-Graph", WriteCommitGraph),
            ],
        ),
        menu(
            "Branch",
            vec![
                MenuItem::action("Switch Branch…", SwitchBranch),
                MenuItem::action("Create Branch at HEAD…", CreateBranch),
                MenuItem::action("Merge Branch into Current…", MergeBranch),
                MenuItem::action("Rebase Current Branch onto…", RebaseCurrentOnto),
                MenuItem::separator(),
                MenuItem::action("Rename Current Branch…", RenameCurrentBranch),
                MenuItem::action("Set Upstream of Current Branch…", SetUpstreamMenu),
                MenuItem::action("Delete Branch…", DeleteBranchMenu),
            ],
        ),
        menu(
            "Commit",
            vec![
                MenuItem::action("Commit", Commit),
                MenuItem::action("Stage All and Commit", StageAllAndCommit),
                MenuItem::action("Create Empty Commit", CreateEmptyCommit),
                MenuItem::action("Amend Previous Commit", ToggleAmend),
                MenuItem::action("Go to Working Directory", GoToWorkingDirectory),
                MenuItem::separator(),
                MenuItem::action("Stage All", StageAll),
                MenuItem::action("Unstage All", UnstageAll),
                MenuItem::action("Discard All Unstaged…", DiscardAll),
                MenuItem::separator(),
                MenuItem::action("Revert Selected Commit", RevertCommit),
                MenuItem::action("Cherry-pick Selected Commit", CherryPickCommit),
                MenuItem::action("Edit Commit Message…", EditCommitMessage),
                MenuItem::action("Get Signing Keys from GitHub…", GetSigningKeys),
                MenuItem::separator(),
                MenuItem::action("Open This Version in Editor (read-only)", OpenThisVersion),
                MenuItem::action("Open Old Version in Editor (read-only)", OpenOldVersion),
                MenuItem::separator(),
                MenuItem::action("Mark for Comparison", MarkCompare),
                MenuItem::action("Swap Comparison Sides", SwapCompare),
                MenuItem::action("End Comparison", EndCompare),
                MenuItem::separator(),
                MenuItem::action("Create Tag at HEAD…", CreateTag),
            ],
        ),
        menu(
            "Stash",
            vec![
                MenuItem::action("Stash Changes…", StashChanges),
                MenuItem::action("Stash Including Untracked", StashIncludingUntracked),
                MenuItem::separator(),
                MenuItem::action("Apply Latest Stash", ApplyStash),
                MenuItem::action("Pop Latest Stash", PopStash),
                MenuItem::action("Drop Latest Stash…", DropLatestStash),
                MenuItem::action("Drop Stash…", DropStash),
            ],
        ),
        menu(
            "Remote",
            vec![
                MenuItem::action("Fetch", Fetch),
                MenuItem::action("Fetch from…", FetchFrom),
                MenuItem::action("Pull", Pull),
                MenuItem::action("Pull from…", PullFrom),
                MenuItem::action("Pull with…", PullWith),
                MenuItem::action("Push", Push),
                MenuItem::action("Push…", PushTo),
                MenuItem::separator(),
                MenuItem::action("Add Remote…", AddRemote),
            ],
        ),
        menu(
            "Navigate",
            vec![
                MenuItem::action("Go to Commit…", GoToCommit),
                MenuItem::action("Search in Changes…", SearchChanges),
                MenuItem::action("Go to Branch / Tag…", GoToRef),
                MenuItem::action("Go to HEAD", GoToHead),
                MenuItem::action("Go to File…", FileList),
                MenuItem::separator(),
                MenuItem::action("Next Commit", SelectNext),
                MenuItem::action("Previous Commit", SelectPrev),
                MenuItem::separator(),
                MenuItem::action("Follow Branch Down (Parent)", FollowBranchDown),
                MenuItem::action("Follow Branch Up (Child)", FollowBranchUp),
                MenuItem::action("Other Way Down (Merged Parent)", OtherBranchDown),
                MenuItem::action("Other Way Up (Other Child)", OtherBranchUp),
                MenuItem::separator(),
                MenuItem::action("Next File", NextFile),
                MenuItem::action("Previous File", PrevFile),
                MenuItem::action("Next Hunk", NextHunk),
                MenuItem::action("Previous Hunk", PrevHunk),
            ],
        ),
        menu(
            "Help",
            vec![
                MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
                MenuItem::action("Start-up Timings…", StartupTimings),
                MenuItem::action("About gitti", About),
            ],
        ),
    ]
}

/// The titles of the in-window menu bar (`workspace` sizes the bar by them).
pub(crate) fn menu_titles() -> Vec<SharedString> {
    menu_bar().into_iter().map(|m| m.name).collect()
}

/// Application menus: native on macOS, drawn in the window (see `workspace`) elsewhere.
/// Keeps the shown tab's greyed commands (a keyboard layout change rebuilds them).
fn set_menus(cx: &mut App) {
    let off = MENU_OFF.with(|m| m.borrow().clone()).unwrap_or_default();
    install_menus(menu_bar_with(&off), cx);
}

/// Installs `menus` (File … Help) with the app menu (macOS) / Settings and Exit.
fn install_menus(mut menus: Vec<Menu>, cx: &mut App) {
    if !in_window_menu_bar() {
        menus.insert(
            0,
            menu(
                "gitti",
                vec![
                    MenuItem::action("About gitti", About),
                    MenuItem::separator(),
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Quit gitti", Quit),
                ],
            ),
        );
        cx.set_menus(menus);
    } else {
        menus[0].items.extend([
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Exit", Quit),
        ]);
        gpui_kit::component::GlobalState::global_mut(cx)
            .set_app_menus(menus.into_iter().map(Menu::owned).collect());
    }
}

/// Key bindings from the table shared with gitty: `Panes` keys act in the commit list and
/// the diff (never in text fields), the others in their pane. Panics on a command without a
/// gitti action, so a table entry can't silently do nothing.
fn shared_key_bindings(cx: &App) -> Vec<KeyBinding> {
    use gitti_core::keys::{BINDINGS, Scope};
    let mut out = Vec::new();
    for b in BINDINGS {
        // `None`: a global binding (also in text fields).
        let contexts: &[Option<&str>] = match b.scope {
            Scope::Global => &[None],
            Scope::Panes => &[
                Some("CommitList"),
                Some("DiffView"),
                Some("Sidebar"),
                Some("GitLog"),
            ],
            Scope::Commits => &[Some("CommitList")],
            Scope::Diff => &[Some("DiffView")],
        };
        for ctx in contexts {
            let name = format!("gitti::{}", b.command.name());
            let action = cx
                .build_action(&name, None)
                .unwrap_or_else(|e| panic!("shared key {}: no action {name}: {e}", b.key));
            let predicate =
                ctx.map(|c| KeyBindingContextPredicate::parse(c).expect("context").into());
            match KeyBinding::load(b.key, action, predicate, false, None, &DummyKeyboardMapper) {
                Ok(binding) => out.push(binding),
                Err(e) => eprintln!("gitti: shared key {}: {e}", b.key),
            }
        }
    }
    out
}

/// `secondary-home` / `secondary-end` go to the start / end of the text in every text
/// field (the commit message, dialog fields), as Home / End go to a list's ends. gpui-base
/// binds ctrl-home / ctrl-end on Windows / Linux only; this adds ⌘Home / ⌘End on macOS.
fn text_end_bindings(cx: &App) -> Vec<KeyBinding> {
    let predicate = || Some(KeyBindingContextPredicate::parse("Input").expect("context").into());
    [
        ("secondary-home", "input::MoveToStart"),
        ("secondary-end", "input::MoveToEnd"),
        ("secondary-shift-home", "input::SelectToStart"),
        ("secondary-shift-end", "input::SelectToEnd"),
    ]
    .into_iter()
    .filter_map(|(key, name)| {
        let action = cx.build_action(name, None).ok()?;
        KeyBinding::load(key, action, predicate(), false, None, &DummyKeyboardMapper).ok()
    })
    .collect()
}

/// The key contexts of a list / scroll scope of the shared table.
pub(crate) fn nav_contexts(scope: gitti_core::keys::NavScope) -> &'static [&'static str] {
    use gitti_core::keys::NavScope::*;
    match scope {
        Sidebar => &["Sidebar"],
        Commits => &["CommitList"],
        Diff => &["DiffView"],
        Log => &["GitLog"],
        Picker => &["GittiPicker > Input"],
        Menu => &["PopupMenu"],
        // Read-only text dialogs and the settings (shortcut list); neither has a text field.
        Text => &["GittiTextView", "GittiSettings"],
    }
}

/// The gitti action of a list / scroll key; `None` where gitti can't do it (gpui-component's
/// context menus only move a line at a time).
pub(crate) fn nav_action(
    scope: gitti_core::keys::NavScope,
    nav: gitti_core::keys::Nav,
) -> Option<Box<dyn Action>> {
    use gitti_core::keys::{Nav, NavScope};
    use gpui_kit::base::actions as menu;
    Some(match (scope, nav) {
        (NavScope::Menu, Nav::LineUp) => Box::new(menu::SelectUp),
        (NavScope::Menu, Nav::LineDown) => Box::new(menu::SelectDown),
        (NavScope::Menu, Nav::Back) => Box::new(menu::Cancel),
        (NavScope::Menu, _) => return None,
        (NavScope::Commits, Nav::LineUp) => Box::new(SelectPrev),
        (NavScope::Commits, Nav::LineDown) => Box::new(SelectNext),
        (NavScope::Sidebar, Nav::LineUp) => Box::new(SidePrev),
        (NavScope::Sidebar, Nav::LineDown) => Box::new(SideNext),
        (NavScope::Picker, Nav::LineUp) => Box::new(PickerUp),
        (NavScope::Picker, Nav::LineDown) => Box::new(PickerDown),
        (NavScope::Log, Nav::Back) => Box::new(FocusCommitList),
        (_, Nav::Back) => Box::new(ModalCancel),
        // The rest act on whatever has the focus (`app/nav_keys.rs`).
        (_, Nav::LineUp) => Box::new(LineUp),
        (_, Nav::LineDown) => Box::new(LineDown),
        (_, Nav::PageUp) => Box::new(PageUp),
        (_, Nav::PageDown) => Box::new(PageDown),
        (_, Nav::HalfPageUp) => Box::new(HalfPageUp),
        (_, Nav::HalfPageDown) => Box::new(HalfPageDown),
        (_, Nav::First) => Box::new(ScrollHome),
        (_, Nav::Last) => Box::new(ScrollEnd),
    })
}

/// Key bindings from the list / scroll table shared with gitty (`NAV_BINDINGS`).
/// Where the commit graph keys act: anywhere in a repository tab — sidebar, commit
/// list, diff, git log, or no pane focused — but not in a text field (`Input`: the commit
/// message, dialog fields, picker queries, the command line), where they edit, and not
/// inside an overlay (dialog, picker, settings, folder picker, context menu). The handlers
/// sit on the tab's root (`GittiApp::render`), so the action reaches them from every pane.
pub(crate) const GRAPH_KEY_CONTEXT: &str = "Gitti && !Input && !GittiModal && !GittiPicker \
     && !GittiSettings && !GittiFolderPicker && !PopupMenu";

/// The commit graph keys of the table shared with gitty (`gitti_core::keys::GRAPH_BINDINGS`).
fn graph_key_bindings() -> Vec<KeyBinding> {
    use gitti_core::keys::GRAPH_BINDINGS;
    GRAPH_BINDINGS
        .iter()
        .map(|b| {
            let action: Box<dyn Action> = match (b.nav.down, b.nav.other) {
                (true, false) => Box::new(FollowBranchDown),
                (false, false) => Box::new(FollowBranchUp),
                (true, true) => Box::new(OtherBranchDown),
                (false, true) => Box::new(OtherBranchUp),
            };
            let predicate = KeyBindingContextPredicate::parse(GRAPH_KEY_CONTEXT)
                .expect("context")
                .into();
            KeyBinding::load(b.key, action, Some(predicate), false, None, &DummyKeyboardMapper)
                .unwrap_or_else(|e| panic!("graph key {}: {e}", b.key))
        })
        .collect()
}

fn nav_key_bindings() -> Vec<KeyBinding> {
    use gitti_core::keys::NAV_BINDINGS;
    let mut out = Vec::new();
    for b in NAV_BINDINGS {
        let Some(action) = nav_action(b.scope, b.nav) else {
            continue;
        };
        for ctx in nav_contexts(b.scope) {
            let predicate = KeyBindingContextPredicate::parse(ctx).expect("context").into();
            match KeyBinding::load(
                b.key,
                action.boxed_clone(),
                Some(predicate),
                false,
                None,
                &DummyKeyboardMapper,
            ) {
                Ok(binding) => out.push(binding),
                Err(e) => eprintln!("gitti: shared key {}: {e}", b.key),
            }
        }
    }
    out
}
