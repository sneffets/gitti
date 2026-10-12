//! Window root: one [`GittiApp`] per repository tab, the tab bar and — on Windows/Linux,
//! where gpui has no native menu bar — the application menu bar. Where the compositor draws no
//! window frame (GNOME/Wayland, WSLg: client-side decorations) the tab bar doubles as title
//! bar: its empty space moves the window and it carries minimize/maximize/close buttons.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{AppMenuBar, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppEvent, AskRequest, GittiApp, PaneStates};
use crate::probe::ProbeExt as _;
use crate::theme::{PaletteExt as _, Settings};
use crate::{CloneRepository, CloseTab, InitRepository, NextTab, OpenRepository, PrevTab};
use crate::{OpenRecent, ToggleFullScreen};

struct Tab {
    /// Repository work dir (or the path the user gave, if it is no repository).
    path: PathBuf,
    app: Entity<GittiApp>,
}

/// A repository tab being dragged to a new place; also its drag image.
#[derive(Clone)]
struct DraggedTab {
    ix: usize,
    title: SharedString,
    text_size: Pixels,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        div()
            .px_3()
            .py_1()
            .bg(p.bg_list)
            .border_1()
            .border_color(p.accent)
            .shadow_md()
            .text_size(self.text_size)
            .text_color(p.text_strong)
            .child(self.title.clone())
    }
}

/// Where the active tab is after the tab at `from` moved to `to` (the others shift).
fn index_after_move(ix: usize, from: usize, to: usize) -> usize {
    if ix == from {
        to
    } else if from < ix && ix <= to {
        ix - 1
    } else if to <= ix && ix < from {
        ix + 1
    } else {
        ix
    }
}

pub struct Workspace {
    tabs: Vec<Tab>,
    active: usize,
    askpass_env: Vec<(String, String)>,
    /// Pane resize state shared by all tabs.
    panes: PaneStates,
    menu_bar: Option<Entity<AppMenuBar>>,
    /// The menu bar's width in rems for its UI font family (`menu_bar_rem`).
    menu_bar_rems: std::cell::RefCell<Option<(SharedString, f32)>>,
    focus: FocusHandle,
    /// Left button went down on the title bar's drag area; the next mouse move starts a
    /// window move (not the press itself, so double clicks still arrive).
    drag_pending: bool,
    /// Scripted runs don't load the saved tab list, and save it only with a config dir of
    /// their own (tests).
    scripted: bool,
    /// The open tabs as the start found them saved; saves the session's tabs — none when
    /// the saved ones could not be read, so they survive for the next start.
    saved_tabs: crate::store::SavedTabs,
    /// The start page while no tab is open: a repository-less app that hosts the Clone /
    /// Init dialogs.
    start: Option<Entity<GittiApp>>,
    /// Restored tabs still to read their repository, in turn after the shown one.
    load_queue: std::collections::VecDeque<WeakEntity<GittiApp>>,
    /// The restored tab reading its repository now ([`AppEvent::FirstLoadDone`] ends it).
    loading: Option<EntityId>,
    /// Esc closes the active tab's dialog / picker even with the focus behind it.
    _overlay_keys: Subscription,
    /// The user chose to quit (or close the window) once the running git operations are
    /// done ([`QuitWhileBusy`]); every tab going idle checks ([`AppEvent::Idle`]).
    quit_pending: Option<QuitTarget>,
    /// How often a waiting quit fired (tests: the test platform's quit does nothing).
    quits_fired: usize,
}

/// What a quit waiting for git ends: the app (⌘Q, File ▸ Exit) or the window (its close
/// button, which quits gitti on Windows / Linux).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitTarget {
    App,
    Window,
}

/// The answer to quitting while git runs (`gitti_core::ops::quit_while_busy_prompt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitWhileBusy {
    /// Wait for the operations, then quit.
    WhenDone,
    /// Cancel them (git removes its locks), then quit once they ended.
    Stop,
    Cancel,
}

/// Asks what to do while `running` git operations run. Quitting at once is not offered:
/// git would lose its output pipes mid-way.
fn ask_quit_while_busy(
    running: &[String],
    window: &mut Window,
    cx: &mut App,
) -> impl std::future::Future<Output = QuitWhileBusy> + 'static {
    use gitti_core::ops::{QUIT_WHEN_DONE, STOP_AND_QUIT};
    let (title, detail) = gitti_core::ops::quit_while_busy_prompt(running);
    let buttons = [QUIT_WHEN_DONE, STOP_AND_QUIT, "Cancel"];
    let answer = window.prompt(PromptLevel::Warning, &title, Some(&detail), &buttons, cx);
    async move {
        match answer.await {
            Ok(0) => QuitWhileBusy::WhenDone,
            Ok(1) => QuitWhileBusy::Stop,
            _ => QuitWhileBusy::Cancel,
        }
    }
}

/// Closing a tab while its git operation runs asks first, as quitting does; `false`
/// closes it right away.
const ASK_BEFORE_CLOSING_A_BUSY_TAB: bool = true;

/// The repository work dir containing `path`, if it is inside one.
fn repo_root(path: &Path) -> Option<PathBuf> {
    gitti_core::Repo::open(path)
        .ok()
        .map(|r| r.workdir().to_path_buf())
}

impl Workspace {
    pub fn new(
        path: Option<PathBuf>,
        askpass_env: Vec<(String, String)>,
        ask_rx: async_channel::Receiver<AskRequest>,
        scripted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let menu_bar = crate::in_window_menu_bar().then(|| AppMenuBar::new(cx));
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            askpass_env,
            panes: PaneStates::new(cx),
            menu_bar,
            menu_bar_rems: Default::default(),
            focus: cx.focus_handle(),
            drag_pending: false,
            scripted,
            saved_tabs: Default::default(),
            start: None,
            load_queue: Default::default(),
            loading: None,
            _overlay_keys: crate::app::intercept_overlay_keys(cx.weak_entity(), window, cx),
            quit_pending: None,
            quits_fired: 0,
        };
        // Scripted runs (tests) show exactly the given repository.
        if !scripted {
            this.saved_tabs = crate::store::SavedTabs::load();
        }
        let repos = std::mem::take(&mut this.saved_tabs.repos);
        this.restore_tabs(&repos, window, cx);
        this.active = this.saved_tabs.active.min(this.tabs.len().saturating_sub(1));
        // Started with GIT_DIR / GIT_WORK_TREE: the folder they are meant for opens (and
        // says why not, if they name another repository) — never silently the saved tabs.
        let launch_env = gitti_core::open_check::LaunchEnv::current().is_some();
        let requested = path.or_else(|| {
            (this.tabs.is_empty() || launch_env)
                .then(|| std::env::current_dir().ok())
                .flatten()
        });
        if let Some(p) = requested {
            this.open(p, window, cx);
        } else {
            this.activate(this.active, window, cx);
        }
        this.start_restored_loads(window, cx);
        if let Some(why) = this.saved_tabs.unreadable.clone()
            && let Some(app) = this.script_app()
        {
            let text = format!(
                "The tabs open last time could not be read ({why}). They stay saved as they \
                 are: this session's tabs are not saved."
            );
            app.update(cx, |a, cx| a.show_startup_notice(text, cx));
        }
        this.listen_askpass(ask_rx, window, cx);
        if !scripted {
            crate::window_placement::track("main", window, cx);
        }
        // Start-up timings: the start is complete once these tabs are shown (the
        // first frame comes later, so this never completes it); usable once the shown one is.
        let repos = this.tabs.iter().filter(|t| t.app.read(cx).has_repo()).count();
        if let Some(app) = this.active_app().filter(|a| a.read(cx).has_repo()) {
            gitti_core::startup::active_tab(&app.read(cx).title());
        }
        let _ = gitti_core::startup::expect_tabs(repos);
        gitti_core::startup::mark("settings, tabs created");
        // Closing the window (it quits gitti on Windows / Linux) while git runs asks first.
        let ws = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(ws) = ws.upgrade() else { return true };
            let running = ws.read(cx).running_ops(cx);
            if running.is_empty() {
                return true;
            }
            let answer = ask_quit_while_busy(&running, window, cx);
            window
                .spawn(cx, async move |cx| {
                    let answer = answer.await;
                    cx.update(|window, cx| {
                        ws.update(cx, |ws, cx| {
                            ws.answer_quit(answer, QuitTarget::Window, window, cx)
                        })
                    })
                    .ok();
                })
                .detach();
            false
        });
        this
    }

    /// Start-up timings: reports the first frame on screen. A next-frame callback
    /// runs at the start of a frame, so the second one runs once the first frame has been
    /// drawn and presented.
    pub fn watch_first_frame(window: AnyWindowHandle, view: Entity<Self>, cx: &mut App) {
        let _ = window.update(cx, |_, window, _| {
            window.on_next_frame(move |window, _| {
                window.on_next_frame(move |_, cx| {
                    if let Some(record) = gitti_core::startup::first_frame() {
                        view.update(cx, |ws, cx| ws.startup_finished(&record, cx));
                    }
                });
            });
        });
    }

    /// The start is complete: a slow one says so once, in the active tab's status line.
    fn startup_finished(
        &mut self,
        record: &gitti_core::startup::StartupRecord,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = record.slow_notice(crate::app::STARTUP_TIMINGS_WHERE) else {
            return;
        };
        if let Some(app) = self.active_app().or_else(|| self.start.clone()) {
            app.update(cx, |a, cx| a.show_startup_notice(text, cx));
        }
    }

    /// The labels of the git operations running in the tabs (quitting asks).
    pub fn running_ops(&self, cx: &App) -> Vec<String> {
        self.tabs
            .iter()
            .filter_map(|t| t.app.read(cx).busy_label().map(str::to_string))
            .collect()
    }

    /// Quit (⌘Q, File ▸ Exit) while a git operation runs: asks first — wait for it, stop
    /// it, or cancel the quit (also a quit already waiting).
    fn on_quit(&mut self, _: &crate::Quit, window: &mut Window, cx: &mut Context<Self>) {
        let running = self.running_ops(cx);
        if running.is_empty() {
            cx.quit();
            return;
        }
        let answer = ask_quit_while_busy(&running, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            this.update_in(cx, |this, window, cx| {
                this.answer_quit(answer, QuitTarget::App, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// The answer to [`ask_quit_while_busy`]: Quit When Done waits, Stop and Quit cancels
    /// the operations first; both quit once every tab is idle. Cancel also drops a quit
    /// that was waiting.
    fn answer_quit(
        &mut self,
        answer: QuitWhileBusy,
        target: QuitTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if answer == QuitWhileBusy::Cancel {
            self.quit_pending = None;
            return;
        }
        // A quit of the app outranks closing the window.
        if self.quit_pending != Some(QuitTarget::App) {
            self.quit_pending = Some(target);
        }
        let apps: Vec<_> = self.tabs.iter().map(|t| t.app.clone()).collect();
        for app in apps {
            app.update(cx, |a, cx| match answer {
                QuitWhileBusy::Stop => a.stop_for_quit(cx),
                _ => a.note_quit_when_done(cx),
            });
        }
        // The operations may have ended while the question was open.
        self.quit_if_idle(window, cx);
    }

    /// A quit waiting for git ([`Self::answer_quit`]): quits once no tab runs one.
    fn quit_if_idle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.quit_pending else { return };
        if !self.running_ops(cx).is_empty() {
            return;
        }
        self.quit_pending = None;
        self.quits_fired += 1;
        match target {
            QuitTarget::App => cx.quit(),
            QuitTarget::Window => window.remove_window(),
        }
    }

    /// Tests: a quit is waiting for git to finish.
    pub fn quit_pending(&self) -> bool {
        self.quit_pending.is_some()
    }

    /// Tests: how often a waiting quit fired.
    pub fn quits_fired(&self) -> usize {
        self.quits_fired
    }

    /// The app of the active tab.
    pub fn active_app(&self) -> Option<Entity<GittiApp>> {
        self.tabs.get(self.active).map(|t| t.app.clone())
    }

    /// The menu bar greys what the shown tab (or the start page) can't do now; it
    /// is rebuilt only when that changed.
    fn sync_menu_availability(&self, cx: &mut Context<Self>) {
        crate::app::set_tab_count(self.tabs.len());
        let off = match self.script_app() {
            Some(app) => app.read(cx).unavailable_commands(),
            None => gitti_core::availability::unavailable(&gitti_core::availability::RepoFacts {
                tabs: self.tabs.len(),
                ..Default::default()
            }),
        };
        if crate::set_menu_off(&off) {
            let bar = self.menu_bar.clone();
            cx.defer(move |cx| {
                crate::refresh_menus(&off, cx);
                if let Some(bar) = bar {
                    bar.update(cx, |b, cx| b.reload(cx));
                }
            });
        }
    }

    /// The active tab's app, else the start page's (script steps, dialogs).
    pub fn script_app(&self) -> Option<Entity<GittiApp>> {
        self.active_app().or_else(|| self.start.clone())
    }

    fn new_app(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Entity<GittiApp> {
        let env = self.askpass_env.clone();
        let panes = self.panes.clone();
        let app = cx.new(|cx| GittiApp::new(path, env, panes, window, cx));
        self.watch_app(&app, window, cx);
        app
    }

    /// Redraw on the app's changes; open the repositories it asks for (clone / init).
    fn watch_app(&self, app: &Entity<GittiApp>, window: &mut Window, cx: &mut Context<Self>) {
        cx.observe(app, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(app, window, |this, app, ev: &AppEvent, window, cx| match ev {
            AppEvent::FirstLoadDone => {
                if this.loading == Some(app.entity_id()) {
                    this.load_next_tab(window, cx);
                }
            }
            AppEvent::OpenRepository(path) => this.open(path.clone(), window, cx),
            AppEvent::OpenRepositoryNoted(path, note) => {
                this.open(path.clone(), window, cx);
                if let Some(app) = this.active_app() {
                    let note = note.clone();
                    app.update(cx, |a, cx| a.show_notice("Cloned", note, window, cx));
                }
            }
            AppEvent::OpenRepositoryRefused(path, note, refused) => {
                this.open(path.clone(), window, cx);
                if let Some(app) = this.active_app() {
                    let retry = crate::app::RetryUpdate::Clone { note: note.clone() };
                    let refused = refused.clone();
                    app.update(cx, |a, cx| a.submodules_refused(refused, retry, window, cx));
                }
            }
            AppEvent::RepositoryRemoved(path) => {
                let path = gitti_core::paths::canonical(path);
                while let Some(ix) = this.tabs.iter().position(|t| t.path == path) {
                    this.close(ix, window, cx);
                }
            }
            AppEvent::RepositoryMoved { from, to } => this.retarget(from, to, window, cx),
            AppEvent::StartupDone(record) => this.startup_finished(record, cx),
            AppEvent::Idle => this.quit_if_idle(window, cx),
            AppEvent::SignersChanged => {
                for t in &this.tabs {
                    if t.app != *app {
                        t.app.update(cx, |a, cx| a.reverify_shown_signature(cx));
                    }
                }
            }
            AppEvent::RemoveWorktree { main, wt, branch } => {
                this.open(main.clone(), window, cx);
                if let Some(app) = this.active_app() {
                    let (wt, branch) = (wt.clone(), branch.clone());
                    app.update(cx, |a, cx| a.run_remove_worktree(wt, false, branch, window, cx));
                }
            }
        })
        .detach();
    }

    /// Tabs on `from` (a worktree that moved) show `to` instead, in their place.
    fn retarget(&mut self, from: &Path, to: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let from = gitti_core::paths::canonical(from);
        let to = gitti_core::paths::canonical(to);
        let mut active_moved = false;
        for ix in 0..self.tabs.len() {
            if self.tabs[ix].path == from {
                let app = self.new_app(to.clone(), window, cx);
                let old = std::mem::replace(
                    &mut self.tabs[ix],
                    Tab {
                        path: to.clone(),
                        app,
                    },
                );
                self.gone_while_loading(old.app.entity_id(), window, cx);
                active_moved |= ix == self.active;
            }
        }
        if active_moved {
            self.activate(self.active, window, cx);
        } else {
            self.save();
            cx.notify();
        }
    }

    /// The saved tabs, not reading their repositories yet ([`Self::start_restored_loads`]);
    /// the settings are read once for all of them. A folder that isn't there (a drive not
    /// connected) keeps its tab, which says so ([`gitti_core::OpenError::FolderMissing`])
    /// and reads it once it is back: dropped, the tab was gone for good with the next save.
    fn restore_tabs(&mut self, repos: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if repos.is_empty() {
            return;
        }
        let settings = Settings::load();
        for path in repos {
            let (env, panes) = (self.askpass_env.clone(), self.panes.clone());
            let settings = settings.clone();
            let app = cx.new(|cx| {
                GittiApp::restored(path.clone(), env, panes, settings, window, cx)
            });
            self.watch_app(&app, window, cx);
            self.tabs.push(Tab { path: path.clone(), app });
        }
    }

    /// The shown tab reads its repository first (`activate`), the other restored tabs one
    /// at a time after it: all at once, their git processes (~10 per tab) slowed the shown
    /// one down — much more so under a virus scanner.
    fn start_restored_loads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = self
            .active_app()
            .filter(|a| a.read(cx).first_load_running())
            .map(|a| a.entity_id());
        self.load_queue = self.tabs.iter().map(|t| t.app.downgrade()).collect();
        if self.loading.is_none() {
            self.load_next_tab(window, cx);
        }
    }

    /// The next restored tab whose turn it is starts reading its repository.
    fn load_next_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = None;
        while let Some(app) = self.load_queue.pop_front() {
            let Some(app) = app.upgrade() else { continue };
            if app.update(cx, |a, cx| a.load_if_deferred(window, cx)) {
                self.loading = Some(app.entity_id());
                return;
            }
        }
    }

    fn add_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let app = self.new_app(path.clone(), window, cx);
        self.tabs.push(Tab { path, app });
        self.tabs.len() - 1
    }

    /// Opens `path` in a new tab, or switches to the tab that already shows its repository
    /// (a tab on the same folder that was no repository yet loads it now).
    pub fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let root = repo_root(&path);
        if let Some(root) = &root
            && self.records_recent()
        {
            crate::store::note_recent_repo(root);
        }
        let path = root.unwrap_or_else(|| gitti_core::paths::canonical(&path));
        let ix = match self.tabs.iter().position(|t| t.path == path) {
            Some(ix) => {
                if !self.tabs[ix].app.read(cx).has_repo() {
                    self.tabs[ix].app = self.new_app(path, window, cx);
                }
                ix
            }
            None => self.add_tab(path, window, cx),
        };
        self.activate(ix, window, cx);
    }

    /// The app that shows a dialog now: the active tab's, else the start page's.
    fn dialog_app(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<GittiApp> {
        if let Some(app) = self.active_app() {
            return app;
        }
        if let Some(app) = self.start.clone() {
            return app;
        }
        let env = self.askpass_env.clone();
        let panes = self.panes.clone();
        let app = cx.new(|cx| GittiApp::start_page(env, panes, window, cx));
        self.watch_app(&app, window, cx);
        self.start = Some(app.clone());
        app
    }

    /// (number of tabs, active tab) for tests.
    pub fn script_tabs(&self) -> (usize, usize) {
        (self.tabs.len(), self.active)
    }

    /// Restores `repos` after the open tabs as a start does, the `active`th of them shown
    /// (tests; scripted runs restore no saved tabs).
    pub fn script_restore(
        &mut self,
        repos: &[PathBuf],
        active: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let first = self.tabs.len();
        self.restore_tabs(repos, window, cx);
        self.activate(first + active, window, cx);
        self.start_restored_loads(window, cx);
    }

    /// Per tab: folder name and whether it waits for its turn / is reading / has read its
    /// repository (`waiting` / `loading` / `loaded`), for tests.
    pub fn script_tab_loads(&self, cx: &App) -> Vec<String> {
        self.tabs
            .iter()
            .map(|t| format!("{} {}", t.app.read(cx).title(), t.app.read(cx).script_load_state()))
            .collect()
    }

    /// Per tab: its mark as text ("●", "○", "") and what they mean, for tests.
    pub fn script_tab_marks(&self, cx: &App) -> Vec<(String, String)> {
        self.tabs
            .iter()
            .map(|t| {
                let m = t.app.read(cx).tab_marks(cx);
                (m.glyphs().to_string(), m.describe())
            })
            .collect()
    }

    /// Repository tab `ix`'s tooltip: its folder, then what its mark means, the glyph
    /// first ("● Uncommitted changes in 2 files: …", "○ 1 commit not pushed to …"; both
    /// lines when both hold). Built when the tooltip shows, so it names the current marks.
    pub fn tab_tooltip(&self, ix: usize, cx: &App) -> String {
        let Some(tab) = self.tabs.get(ix) else {
            return String::new();
        };
        let path = tab.path.display().to_string();
        let legend = tab.app.read(cx).tab_marks(cx).legend();
        if legend.is_empty() { path } else { format!("{path}\n{legend}") }
    }

    /// Per tab: its tooltip ([`Self::tab_tooltip`]), for tests.
    pub fn script_tab_tooltips(&self, cx: &App) -> Vec<String> {
        (0..self.tabs.len()).map(|ix| self.tab_tooltip(ix, cx)).collect()
    }

    /// Tab `ix`'s app, for tests.
    pub fn script_tab_app(&self, ix: usize) -> Option<Entity<GittiApp>> {
        self.tabs.get(ix).map(|t| t.app.clone())
    }

    /// Per tab: its WORKTREES section's marks, for tests.
    pub fn script_sidebar_marks(&self, cx: &App) -> Vec<Vec<String>> {
        self.tabs.iter().map(|t| t.app.read(cx).script_worktree_marks(cx)).collect()
    }

    /// The tabs' paths in their order, for tests.
    pub fn script_tab_paths(&self) -> Vec<PathBuf> {
        self.tabs.iter().map(|t| t.path.clone()).collect()
    }

    /// Moves the tab at `from` to `to` (drag and drop); the active tab stays active.
    /// The order is saved with the open tabs.
    fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let n = self.tabs.len();
        if from == to || from >= n || to >= n {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = index_after_move(self.active, from, to);
        self.save();
        cx.notify();
    }

    pub fn script_activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.activate(ix, window, cx);
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            self.active = 0;
            window.set_window_title("gitti");
            self.dialog_app(window, cx);
            self.focus.focus(window, cx);
            self.save();
            cx.notify();
            return;
        }
        self.active = ix.min(self.tabs.len() - 1);
        // A tab whose folder was missing (a drive not connected) reads it once it is back.
        let tab = &self.tabs[self.active];
        if tab.app.read(cx).folder_missing() && tab.path.exists() {
            let path = tab.path.clone();
            self.tabs[self.active].app = self.new_app(path, window, cx);
        }
        let app = self.tabs[self.active].app.clone();
        app.update(cx, |a, cx| {
            a.reload_settings(cx);
            // A restored tab shown before its turn reads its repository now.
            a.load_if_deferred(window, cx);
            a.brought_to_front(cx);
            a.focus_list(window, cx);
        });
        let title = format!("{} — gitti", app.read(cx).title());
        window.set_window_title(&title);
        self.save();
        cx.notify();
    }

    /// The user closes tab `ix` (⌘W, its ×, a middle click): while its git operation
    /// runs, asks first ([`ASK_BEFORE_CLOSING_A_BUSY_TAB`]).
    fn request_close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        let running = tab.app.read(cx).busy_label().map(str::to_string);
        let Some(label) = running.filter(|_| ASK_BEFORE_CLOSING_A_BUSY_TAB) else {
            self.close(ix, window, cx);
            return;
        };
        let id = tab.app.entity_id();
        let (title, detail) = gitti_core::ops::close_tab_while_busy_prompt(&label);
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &["Close Anyway", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| {
                    // The tabs may have moved meanwhile.
                    if let Some(ix) = this.tabs.iter().position(|t| t.app.entity_id() == id) {
                        this.close(ix, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        let closed = self.tabs.remove(ix);
        let next = if self.active > ix || self.active >= self.tabs.len() {
            self.active.saturating_sub(1)
        } else {
            self.active
        };
        self.activate(next, window, cx);
        self.gone_while_loading(closed.app.entity_id(), window, cx);
    }

    /// A tab's app is gone (closed, replaced): if it was reading its repository in turn,
    /// the next restored tab goes on.
    fn gone_while_loading(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading == Some(id) {
            self.load_next_tab(window, cx);
        }
    }

    fn save(&self) {
        crate::app::set_open_repos(self.tabs.iter().map(|t| t.path.clone()).collect());
        if self.scripted && std::env::var_os("GITTI_CONFIG_DIR").is_none() {
            return;
        }
        let repos: Vec<PathBuf> = self.tabs.iter().map(|t| t.path.clone()).collect();
        self.saved_tabs.save(&repos, self.active);
    }

    /// Opened repositories go to the recent list (File ▸ Open Recent) — in scripted runs
    /// only with a config dir of their own (tests), never into the user's list.
    fn records_recent(&self) -> bool {
        !self.scripted || std::env::var_os("GITTI_CONFIG_DIR").is_some()
    }

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Repository".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = answer.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| this.open(path, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Credential prompts go to the tab running a git operation (else the active one).
    fn listen_askpass(
        &mut self,
        rx: async_channel::Receiver<AskRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(req) = rx.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    let busy = this.tabs.iter().position(|t| t.app.read(cx).is_busy());
                    let ix = busy.unwrap_or(this.active);
                    if ix != this.active {
                        this.activate(ix, window, cx);
                    }
                    // A clone from the start page (no tab open).
                    let target = this.tabs.get(ix).map(|t| t.app.clone()).or_else(|| this.start.clone());
                    match target {
                        Some(app) => app.update(cx, |a, cx| a.enqueue_ask(req, window, cx)),
                        // No tab: nobody can answer.
                        None => {
                            let _ = req.reply.send(None);
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // ------------------------------------------------------------------ actions

    fn on_open(&mut self, _: &OpenRepository, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_open(window, cx);
    }

    fn on_clone(&mut self, _: &CloneRepository, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.dialog_app(window, cx);
        app.update(cx, |a, cx| a.clone_dialog(window, cx));
    }

    fn on_init(&mut self, _: &InitRepository, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.dialog_app(window, cx);
        app.update(cx, |a, cx| a.init_dialog(window, cx));
    }

    fn on_open_recent(&mut self, _: &OpenRecent, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.dialog_app(window, cx);
        app.update(cx, |a, cx| a.open_recent_picker(window, cx));
    }

    fn on_fullscreen(&mut self, _: &ToggleFullScreen, window: &mut Window, _: &mut Context<Self>) {
        window.toggle_fullscreen();
    }

    fn on_close(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(self.active, window, cx);
    }

    fn on_next(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            self.activate((self.active + 1) % self.tabs.len(), window, cx);
        }
    }

    fn on_prev(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            let n = self.tabs.len();
            self.activate((self.active + n - 1) % n, window, cx);
        }
    }

    // ------------------------------------------------------------------ rendering

    fn render_tab_bar(
        &self,
        settings: &Settings,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = cx.palette().clone();
        let h = settings.row_h() + px(6.);
        let text_size = settings.ui();
        // Menu bar, "+" and ▾ share the bar with the tabs and window buttons.
        let bar_rem = self.menu_bar_rem(settings, window, cx);
        let tabs = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let active = ix == self.active;
            let title = tab.app.read(cx).title();
            let busy = tab.app.read(cx).busy_label().map(str::to_string);
            let marks = tab.app.read(cx).tab_marks(cx);
            let dragged = DraggedTab {
                ix,
                title: title.clone().into(),
                text_size,
            };
            h_flex()
                .id(("repo-tab", ix))
                .probe(format!("repo-tab:{ix}"))
                .h_full()
                .min_w(px(settings.ui_font_size * 7.))
                .max_w(px(settings.ui_font_size * 18.))
                .pl_3()
                .pr_1()
                .gap_2()
                .items_center()
                .border_r_1()
                .border_color(p.border)
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(p.bg_list)
                        .text_color(p.text_strong)
                        .border_t_2()
                        .border_color(p.accent)
                })
                .when(!active, |d| {
                    d.text_color(p.text_muted)
                        .hover(|s| s.bg(p.bg_list_hover).text_color(p.text))
                })
                // The marks follow the name right away (it truncates first); the spacer
                // keeps × at the right end.
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .probe(format!("repo-tab-title:{ix}"))
                        .child(title),
                )
                .children(marks.element(ix, settings.ui_font_size, &p))
                .child(div().flex_1())
                // Not a circle: those are the marks.
                .when_some(busy, |d, label| {
                    d.child(
                        div()
                            .id(("tab-busy", ix))
                            .flex_none()
                            .text_color(p.warning)
                            .child(Icon::new(IconName::Loader).small())
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(format!("{label}…"))
                                    .build(window, cx)
                            }),
                    )
                })
                .child(
                    div()
                        .id(("close-tab", ix))
                        .px_1()
                        .text_color(p.text_faint)
                        .hover(|s| s.bg(p.bg_list_selected).text_color(p.text_strong))
                        .child("×")
                        // ⌘W closes the active tab.
                        .when(active, |d| d.tooltip(crate::shortcuts::CLOSE_TAB.tooltip()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.request_close(ix, window, cx);
                        })),
                )
                // Read when it shows: the path and what the tab's mark means right then.
                .tooltip({
                    let ws = cx.entity().downgrade();
                    move |window, cx| {
                        let text = ws.upgrade().map(|ws| ws.read(cx).tab_tooltip(ix, cx)).unwrap_or_default();
                        gpui_kit::component::tooltip::Tooltip::new(text).build(window, cx)
                    }
                })
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| this.request_close(ix, window, cx)),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.activate(ix, window, cx)))
                // Drag and drop reorders: the dragged tab takes the place of the one it is
                // dropped on.
                .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<DraggedTab>({
                    let accent = p.accent;
                    move |s, _, _, _| s.bg(accent.opacity(0.25))
                })
                .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| this.move_tab(d.ix, ix, cx)))
        });
        h_flex()
            .h(h)
            .flex_none()
            .w_full()
            .probe("tab-bar")
            .items_center()
            .bg(p.bg_section)
            .border_b_1()
            .border_color(p.border)
            .text_size(settings.ui())
            .when_some(self.menu_bar.clone(), |d, bar| {
                d.child(crate::rem_scope::rem_scope(
                    bar_rem,
                    div()
                        .flex_none()
                        .h_full()
                        .px_1()
                        .border_r_1()
                        .border_color(p.border)
                        .probe("menu-bar")
                        .child(bar),
                ))
            })
            .child(
                h_flex()
                    .id("repo-tabs")
                    .h_full()
                    // A narrow window with a big UI font (Linux: menu bar + ▾ + window
                    // controls in the same bar) squeezed the strip to a sliver; the drag
                    // area shrinks first, the strip keeps a usable part of a tab.
                    .min_w(px(48.))
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .child(crate::rem_scope::rem_scope(
                bar_rem,
                div().flex_none().probe("open-repo").child(
                    Button::new("open-repo")
                        .label("+")
                        .small()
                        .ghost()
                        .tooltip(crate::shortcuts::OPEN_REPO.text(cx))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.prompt_open(window, cx)),
                        ),
                ),
            ))
            .map(|d| {
                if crate::draws_window_frame(window) {
                    d.child(self.render_drag_area(cx))
                        .children(self.render_tab_list_menu(bar_rem, false, cx))
                        .child(self.render_window_controls(h, window, cx))
                } else {
                    d.child(div().flex_1())
                        .children(self.render_tab_list_menu(bar_rem, true, cx))
                }
            })
    }

    /// The ▾ at the right edge of the tab bar (probe `repo-tabs-menu`): a menu of every
    /// open repository, the active one checked; a click switches to it — also the way to
    /// tabs scrolled out of sight. On the menu bar's rem like "+" (at big fonts it took the
    /// last repository tab's room in a narrow window); `padded` away from the window edge
    /// when no window buttons follow.
    fn render_tab_list_menu(
        &self,
        bar_rem: Pixels,
        padded: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // One tab has nothing to pick from — and a narrow window with a big UI font needs
        // the room for that tab (Linux: menu bar, tabs and window controls share the bar).
        if self.tabs.len() < 2 {
            return None;
        }
        let ws = cx.entity().downgrade();
        let menu = div()
                .flex_none()
                .when(padded, |d| d.px_1())
                .probe("repo-tabs-menu")
                .child(
                    Button::new("repo-tabs-menu")
                        .icon(IconName::ChevronDown)
                        .small()
                        .ghost()
                        .tooltip("All open repositories")
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
                            Self::tab_list_menu(menu, &ws, cx)
                        }),
                );
        Some(crate::rem_scope::rem_scope(bar_rem, menu).into_any_element())
    }

    fn tab_list_menu(menu: PopupMenu, ws: &WeakEntity<Self>, cx: &App) -> PopupMenu {
        let Some(this) = ws.upgrade() else {
            return menu;
        };
        let this = this.read(cx);
        let mut menu = menu;
        for (ix, label) in this.tab_list_labels(cx).into_iter().enumerate() {
            let ws = ws.clone();
            menu = menu.item(
                PopupMenuItem::new(label)
                    .checked(ix == this.active)
                    .on_click(move |_, window, cx| {
                        ws.update(cx, |ws, cx| ws.activate(ix, window, cx)).ok();
                    }),
            );
        }
        menu
    }

    /// The ▾ list's entries: each tab's name (with its path when two share it), then its
    /// mark and what it means ("● uncommitted changes", "○ unpushed commits").
    pub fn tab_list_labels(&self, cx: &App) -> Vec<String> {
        let titles: Vec<String> = self.tabs.iter().map(|t| t.app.read(cx).title()).collect();
        self.tabs
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                // Two tabs of the same name (worktrees, clones) tell their paths.
                let mut label = if titles.iter().filter(|t| **t == titles[ix]).count() > 1 {
                    format!("{} — {}", titles[ix], tab.path.display())
                } else {
                    titles[ix].clone()
                };
                if let Some(mark) = tab.app.read(cx).tab_marks(cx).mark() {
                    label = format!("{label}  {} {}", mark.glyph(), mark.meaning());
                }
                label
            })
            .collect()
    }

    /// The menu bar's (and "+"'s, ▾'s) rem: the window's (UI font), but at most what lets the bar
    /// leave room for a repository tab, "+", the drag area, ▾ and the window buttons —
    /// never below gpui's 16 px, the size it always had before. Its width is linear in the
    /// rem: gpui-component draws each menu as a small compact button (text 0.875 rem,
    /// padding 2 × 0.375 rem, 0.25 rem gaps, 0.25 rem padding each side here).
    fn menu_bar_rem(&self, settings: &Settings, window: &Window, cx: &App) -> Pixels {
        use gpui_kit::component::ActiveTheme as _;
        let rem = settings.rem();
        if rem <= px(16.) || self.menu_bar.is_none() {
            return rem;
        }
        let family = cx.theme().font_family.clone();
        let cached = self.menu_bar_rems.borrow().clone();
        let rems = match cached {
            Some((f, r)) if f == family => r,
            _ => {
                let titles = crate::menu_titles();
                let probe = px(100.);
                let text: f32 = titles
                    .iter()
                    .map(|t| {
                        let run = TextRun {
                            len: t.len(),
                            font: font(family.clone()),
                            color: Hsla::default(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        f32::from(window.text_system().shape_line(t.clone(), probe, &[run], None).width)
                            / f32::from(probe)
                    })
                    .sum();
                let n = titles.len() as f32;
                let r = 0.875 * text + n * 0.75 + (n - 1.).max(0.) * 0.25 + 0.5;
                *self.menu_bar_rems.borrow_mut() = Some((family, r));
                r
            }
        };
        // Tab (part of it), "+", drag area, ▾ and the three window buttons.
        let room = window.viewport_size().width - px(354.);
        rem.min((room / rems).max(px(16.)))
    }

    /// Empty title bar space: drag moves the window, double click toggles maximize, right
    /// click opens the compositor's window menu.
    fn render_drag_area(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("window-drag")
            .flex_1()
            // Always something to grab the window by: the repository tabs shrink first
            // (their strip scrolls).
            .min_w(px(32.))
            .h_full()
            .probe("window-drag")
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag_pending = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag_pending = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.drag_pending = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if std::mem::take(&mut this.drag_pending) {
                    window.start_window_move();
                }
            }))
            .on_mouse_down(MouseButton::Right, |ev, window, _| {
                window.show_window_menu(ev.position)
            })
            .on_click(|ev, window, _| {
                if ev.click_count() == 2 {
                    window.zoom_window();
                }
            })
    }

    /// Minimize / maximize-restore / close, as far as the window manager supports them.
    fn render_window_controls(
        &self,
        h: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = cx.palette().clone();
        let supported = window.window_controls();
        let button = |id: &'static str, icon: IconName, close: bool| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                // The icon keeps its size at big fonts; wider buttons pushed close out of a
                // small window (800 px at 28 pt: menu bar 537 + 3 × 81).
                .w((h * 1.4).min(px(46.)))
                .h_full()
                .text_color(p.text_muted)
                .hover(|s| {
                    if close {
                        s.bg(p.danger).text_color(gpui_kit::white())
                    } else {
                        s.bg(p.bg_list_hover).text_color(p.text_strong)
                    }
                })
                // Keep the press away from the drag area and the window's own handlers.
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .probe(id)
                .child(Icon::new(icon).small())
        };
        let (max_id, max_icon) = if window.is_maximized() {
            ("window-restore", IconName::WindowRestore)
        } else {
            ("window-maximize", IconName::WindowMaximize)
        };
        h_flex()
            .flex_none()
            .h_full()
            .probe("window-controls")
            .when(supported.minimize, |d| {
                d.child(
                    button("window-minimize", IconName::WindowMinimize, false)
                        .on_click(|_, window, _| window.minimize_window()),
                )
            })
            .when(supported.maximize, |d| {
                d.child(
                    button(max_id, max_icon, false)
                        .on_click(|_, window, _| window.zoom_window()),
                )
            })
            .child(
                button("window-close", IconName::WindowClose, true)
                    .on_click(|_, window, _| window.remove_window()),
            )
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.bg_diff)
            .child(
                v_flex()
                    .gap_3()
                    .items_center()
                    .text_color(p.text_muted)
                    .child("No repository open")
                    .child(
                        Button::new("open-repo-empty")
                            .label(format!("Open Repository…  ({}O)", crate::theme::MOD_KEY))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.prompt_open(window, cx)),
                            ),
                    ),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The window root renders once per frame: a frame boundary for the layout probes.
        crate::probe::begin_frame();
        crate::menu_height::track_window(window, cx);
        let p = cx.palette().clone();
        let active = self.active_app();
        let settings = active
            .as_ref()
            .map(|a| a.read(cx).settings().clone())
            .unwrap_or_default();
        crate::theme::sync_rem_size(&settings, window, cx);
        self.sync_menu_availability(cx);
        v_flex()
            .size_full()
            .bg(p.bg_list)
            .text_color(p.text)
            .text_size(settings.ui())
            .track_focus(&self.focus)
            .key_context("Workspace")
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_clone))
            .on_action(cx.listener(Self::on_init))
            .on_action(cx.listener(Self::on_open_recent))
            .on_action(cx.listener(Self::on_fullscreen))
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            .on_action(cx.listener(Self::on_quit))
            // No tooltips after the pointer left the window.
            .child(crate::tooltip_gate::watcher())
            .child(
                v_flex()
                    .size_full()
                    // A click on something that takes no focus itself (status bar, toolbar,
                    // tab bar, headers) keeps the focus where it was: gpui would give it to
                    // this root, outside the tab's key context, and the tab's shortcuts
                    // (⌘⇧L, menu actions, …) would stop working. With nothing focused at
                    // all, the commit list gets it.
                    .when(active.is_some(), |d| {
                        d.on_any_mouse_down(cx.listener(|this, _, window, cx| {
                            window.prevent_default();
                            if window.focused(cx).is_none_or(|f| f == this.focus)
                                && let Some(app) = this.active_app()
                            {
                                app.update(cx, |a, cx| a.focus_list(window, cx));
                            }
                        }))
                    })
                    .child(self.render_tab_bar(&settings, window, cx))
                    .map(|d| match active.or_else(|| self.start.clone()) {
                        Some(app) => d.child(div().flex_1().min_h_0().child(app)),
                        None => d.child(self.render_empty(cx)),
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::index_after_move;

    #[gitti_test::test]
    fn active_tab_follows_a_move() {
        // Tabs a b c d, a moved to the end: b c d a.
        assert_eq!(index_after_move(0, 0, 3), 3);
        assert_eq!(index_after_move(1, 0, 3), 0);
        assert_eq!(index_after_move(3, 0, 3), 2);
        // d moved to the front: d a b c.
        assert_eq!(index_after_move(3, 3, 0), 0);
        assert_eq!(index_after_move(0, 3, 0), 1);
        assert_eq!(index_after_move(2, 3, 0), 3);
        // Outside the moved range nothing shifts.
        assert_eq!(index_after_move(3, 0, 2), 3);
        assert_eq!(index_after_move(0, 1, 3), 0);
    }
}
