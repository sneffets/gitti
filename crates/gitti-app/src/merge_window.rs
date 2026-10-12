//! Merge editor: a separate window for one conflicted file. Ours (top left) and theirs
//! (top right) are aligned chunk by chunk in one list; below is the result, a freely
//! editable code editor holding the file as it will be saved. Each changed chunk has
//! buttons to take ours, theirs, both or the base; unresolved conflicts stay as markers.
//!
//! The result text is the truth: every chunk's place in it is a line range
//! (`regions`) that follows typing, undo and outside edits through
//! `gitti_core::merge::remap_regions`, and a chunk's state (taken side, unresolved, edited
//! by hand) is read off its text. Changes made to the file outside (another editor) are
//! taken in like an edit of our own (undoable), never overwritten.
//!
//! The three-way merge itself is `gitti_core::merge`. The main window keeps working; it
//! picks up the saved file through its fs watcher.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gitti_core::Repo;
use gitti_core::merge::{ChunkKind, Resolution};
use gitti_core::merge_session::{
    ChunkState, DiskChange, MergeSession, Origin, TextEdit, TopRow, line_starts,
};
use gitti_core::ops::ConflictSides;
use gitti_core::store::SharedSettings;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Editor, EditorState, InputEvent,
};
use gpui_kit::component::{Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::text_sel::{
    Area, DragTargets, Row, TextPos, TextSel, char_index, on_window_drag, overlay_selection,
    press, text_index, unit_at,
};
use crate::probe::ProbeExt as _;
use crate::theme::{Palette, PaletteExt as _, Settings, mono_font};

thread_local! {
    /// The most recently opened merge window (for `GITTI_SCRIPT`).
    pub static LAST_WINDOW: RefCell<Option<AnyWindowHandle>> = const { RefCell::new(None) };
    /// Its editor (tests read its scroll state).
    pub static LAST_EDITOR: RefCell<Option<WeakEntity<MergeEditor>>> = const { RefCell::new(None) };
}

// The merge window's keys, the same as in gitty's merge view. Bound in `lib.rs::init` from
// [`key_bindings`]; the hints (tooltips, status texts) read them back ([`Hints`]), so they
// can't drift from the bindings.
actions!(
    merge,
    [
        Save,
        ToggleView,
        NextConflict,
        PrevConflict,
        NextChange,
        PrevChange,
        TakeOurs,
        TakeTheirs,
        TakeOursThenTheirs,
        TakeTheirsThenOurs,
        TakeBase,
        PutBack,
        OpenInEditor,
        CopySelection,
    ]
);

/// Key context of the whole merge window (also while typing in the result).
const WINDOW_CONTEXT: &str = "MergeEditor";
/// Key context of the ours / theirs panes (single keys; the result editor types them).
const SIDES_CONTEXT: &str = "MergeSides";

/// The merge window's key bindings.
pub fn key_bindings() -> Vec<KeyBinding> {
    let w = Some(WINDOW_CONTEXT);
    let s = Some(SIDES_CONTEXT);
    vec![
        KeyBinding::new("secondary-s", Save, w),
        KeyBinding::new("alt-v", ToggleView, w),
        KeyBinding::new("v", ToggleView, s),
        KeyBinding::new("n", NextConflict, s),
        KeyBinding::new("shift-n", PrevConflict, s),
        KeyBinding::new("]", NextChange, s),
        KeyBinding::new("[", PrevChange, s),
        KeyBinding::new("1", TakeOurs, s),
        KeyBinding::new("2", TakeTheirs, s),
        KeyBinding::new("3", TakeOursThenTheirs, s),
        KeyBinding::new("4", TakeTheirsThenOurs, s),
        KeyBinding::new("0", TakeBase, s),
        KeyBinding::new("r", PutBack, s),
        KeyBinding::new("o", OpenInEditor, s),
        KeyBinding::new("secondary-c", CopySelection, s),
    ]
}

/// The key hints of the merge window, read from the live bindings (made once per key
/// map, like `shortcuts::Tip`).
#[derive(Clone, Debug, Default)]
pub struct Hints {
    pub save: String,
    /// ⌥V (anywhere in the window).
    pub view: String,
    pub next_conflict: String,
    pub prev_conflict: String,
    /// `] / [`.
    pub changes: String,
    pub take_ours: String,
    pub take_theirs: String,
    pub take_ours_then_theirs: String,
    pub take_theirs_then_ours: String,
    pub take_base: String,
    pub put_back: String,
    pub open_in_editor: String,
    /// The result editor's undo (gpui-component's `Input` binding).
    pub undo: String,
}

thread_local! {
    static HINTS: RefCell<Option<(KeymapVersion, Rc<Hints>)>> = const { RefCell::new(None) };
}

impl Hints {
    /// The key taking `r` for the current chunk.
    pub fn take(&self, r: Resolution) -> &str {
        match r {
            Resolution::Ours => &self.take_ours,
            Resolution::Theirs => &self.take_theirs,
            Resolution::OursThenTheirs => &self.take_ours_then_theirs,
            Resolution::TheirsThenOurs => &self.take_theirs_then_ours,
            Resolution::Base => &self.take_base,
        }
    }

    pub fn get(cx: &App) -> Rc<Hints> {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let version = keymap.version();
        if let Some(h) = HINTS.with(|h| {
            h.borrow()
                .as_ref()
                .filter(|(v, _)| *v == version)
                .map(|(_, h)| h.clone())
        }) {
            return h;
        }
        let keys = |action: &dyn Action, ctx: &str| {
            crate::shortcuts::binding_texts(&keymap, action, Some(ctx)).join(" / ")
        };
        let side = |action: &dyn Action| keys(action, SIDES_CONTEXT);
        let hints = Rc::new(Hints {
            save: keys(&Save, WINDOW_CONTEXT),
            view: keys(&ToggleView, WINDOW_CONTEXT),
            next_conflict: side(&NextConflict),
            prev_conflict: side(&PrevConflict),
            changes: format!("{} / {}", side(&NextChange), side(&PrevChange)),
            take_ours: side(&TakeOurs),
            take_theirs: side(&TakeTheirs),
            take_ours_then_theirs: side(&TakeOursThenTheirs),
            take_theirs_then_ours: side(&TakeTheirsThenOurs),
            take_base: side(&TakeBase),
            put_back: side(&PutBack),
            open_in_editor: side(&OpenInEditor),
            undo: crate::shortcuts::binding_texts(
                &keymap,
                &gpui_kit::component::input::Undo,
                Some("Input"),
            )
            .into_iter()
            .next()
            .unwrap_or_default(),
        });
        HINTS.with(|h| *h.borrow_mut() = Some((version, hints.clone())));
        hints
    }
}

/// Loads the merge session of `path` (git calls: run it in the background). Errors
/// (binary file, stages missing) are returned so the main window can show them.
pub fn load(repo: Arc<Repo>, path: &str, sides: ConflictSides) -> anyhow::Result<MergeSession> {
    MergeSession::open(repo, path, sides)
}

/// Opens the merge editor window for a loaded `session` of `path`.
pub fn open(
    session: MergeSession,
    path: &str,
    settings: Settings,
    scripted: bool,
    cx: &mut App,
) -> anyhow::Result<()> {
    let (bounds, display_id) = if scripted {
        let b = Bounds::new(point(px(-6000.), px(-6000.)), size(px(1400.), px(900.)));
        (WindowBounds::Windowed(b), None)
    } else {
        crate::window_placement::initial("merge", size(px(1400.), px(900.)), cx)
    };
    let options = WindowOptions {
        window_bounds: Some(bounds),
        display_id,
        focus: !scripted,
        titlebar: Some(TitlebarOptions {
            title: Some(format!("Merge {path}").into()),
            ..Default::default()
        }),
        app_id: Some(crate::app_icon::APP_ID.into()),
        ..Default::default()
    };
    let (window, editor) = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| {
            if !scripted {
                crate::window_placement::track("merge", window, cx);
            }
            MergeEditor::new(session, settings, window, cx)
        })
    })?;
    LAST_WINDOW.with(|w| *w.borrow_mut() = Some(window));
    LAST_EDITOR.with(|e| *e.borrow_mut() = Some(editor.downgrade()));
    Ok(())
}

/// How often the file on disk is compared with what we last read / wrote.
const DISK_POLL: Duration = Duration::from_secs(1);

pub struct MergeEditor {
    /// Document, result text, chunk regions and states (shared with gitty).
    session: MergeSession,
    settings: Settings,
    top_scroll: UniformListScrollHandle,
    /// Focus of the ours / theirs panes (⌘C copies their selection).
    top_focus: FocusHandle,
    /// Character selection in the ours / theirs panes.
    text_sel: Option<TextSel>,
    /// A selection drag started in ours / theirs and the button is still held.
    text_dragging: bool,
    /// The ours / theirs lines laid out last frame, for dragging outside them.
    drag_targets: DragTargets,
    /// The result editor; its text follows `session.text()` (synced on every change).
    editor: Entity<EditorState>,
    /// Window bounds of the result pane as last painted (to place the layers drawn behind
    /// and over the editor).
    pane_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Where the editor's text area starts in the pane when scrolled to the top (measured
    /// after the editor painted; constant, unlike its scrolled bounds).
    text_origin: Rc<Cell<Option<Point<Pixels>>>>,
    /// Chunk index of the conflict navigated to last.
    current: Option<usize>,
    /// Single view: only the result, ours / theirs folded to a bar (shared setting
    /// `merge_single_view`, also gitty's).
    single: bool,
    status: Option<(String, bool)>,
    /// Scroll positions (px from the top: ours | theirs, result) as last synchronized.
    synced: (f32, f32),
    /// The pane whose position `sync_scroll` set last: a change of it in the next frame is
    /// that pane clamping, not the user scrolling it.
    follower: Option<SyncPane>,
    /// Result scroll position (px) set by `sync_scroll` that the editor applies only in its
    /// next layout, and how many more renders to wait for it; the layers drawn around the
    /// text use it meanwhile so they move in the same frame as the text.
    pending_result: Option<(f32, u8)>,
    _subscriptions: Vec<Subscription>,
    _disk_poll: Task<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SyncPane {
    Top,
    Result,
}

impl MergeEditor {
    fn new(
        session: MergeSession,
        settings: Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language =
            crate::highlight::language_for_path(&session.doc.path).unwrap_or("plaintext");
        let text = session.text().to_string();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                // Rows must map 1:1 to lines for the chunk layers (see `render_result`).
                .folding(false)
                .soft_wrap(false)
                .cursor_surrounding_lines(Some(3))
                .default_value(text)
        });
        // Colours from gitti-core's engine, like the diffs (`highlight.rs`).
        editor.update(cx, |e, cx| {
            e.set_highlighter_factory(crate::highlight::input_highlighter_factory(), cx)
        });
        let subscriptions = vec![
            cx.subscribe(&editor, |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.sync_from_editor(cx);
                }
            }),
        ];
        let disk_poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(DISK_POLL).await;
                if this
                    .update_in(cx, |this, window, cx| this.check_disk(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        let mut this = Self {
            session,
            settings,
            top_scroll: UniformListScrollHandle::new(),
            top_focus: cx.focus_handle(),
            text_sel: None,
            text_dragging: false,
            drag_targets: DragTargets::default(),
            editor,
            pane_bounds: Rc::default(),
            text_origin: Rc::default(),
            current: None,
            single: gitti_core::store::load_settings::<SharedSettings>().merge_single_view,
            status: None,
            synced: (0., 0.),
            follower: None,
            pending_result: None,
            _subscriptions: subscriptions,
            _disk_poll: disk_poll,
        };
        let first = this.session.doc.conflicts().next();
        if let Some(first) = first {
            this.go_to(first, window, cx);
        }
        this
    }

    // ------------------------------------------------------------------ model

    /// Picks up edits made in the editor (typing, undo, paste …).
    fn sync_from_editor(&mut self, cx: &mut Context<Self>) {
        let value = self.editor.read(cx).value();
        if value.as_ref() != self.session.text() {
            self.session.set_text(value.to_string());
            cx.notify();
        }
    }

    /// Applies an edit the session already made to its text to the editor (one undo step).
    fn apply_edit(&mut self, edit: TextEdit, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| {
            e.set_selected_range(edit.range, cx);
            e.replace(edit.text, window, cx);
        });
        cx.notify();
    }

    /// Takes `r` for `chunk`; `None` puts back what git would do (conflict markers for a
    /// conflict).
    fn choose(
        &mut self,
        chunk: usize,
        r: Option<Resolution>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_from_editor(cx);
        self.current = Some(chunk);
        self.status = None;
        let edit = self.session.choose(chunk, r);
        self.apply_edit(edit, window, cx);
    }

    /// Scrolls the top list to `chunk` and puts the result's cursor on it.
    fn go_to(&mut self, chunk: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.current = Some(chunk);
        // A few rows of context above the chunk.
        if let Some(i) = self.session.top_row_of(chunk) {
            self.top_scroll
                .scroll_to_item_strict(i.saturating_sub(3), ScrollStrategy::Top);
        }
        // Cursor to the chunk without taking the focus (that stays where the user clicked),
        // and the chunk a few lines below the top.
        let line = self.session.regions()[chunk].start;
        let text = self.session.text();
        let offset = line_starts(text)[line.min(text.lines().count())];
        self.editor.update(cx, |e, cx| {
            e.set_selected_range(offset..offset, cx);
            if let Some(lh) = e.line_height() {
                let y = (lh * line.saturating_sub(3) as f32).max(px(0.));
                e.set_scroll_offset(point(px(0.), -y), cx);
            }
        });
        let _ = window;
        cx.notify();
    }

    /// Synchronized scrolling: the pane the user scrolled leads, the other follows
    /// to the matching position (`MergeSession::result_line_for_top` / `top_row_for_result`,
    /// shared with gitty) — before this frame is laid out, so both move together. Runs on
    /// every render; in single view there is nothing to follow.
    fn sync_scroll(&mut self, cx: &mut Context<Self>) {
        if self.single {
            return;
        }
        let top = -f32::from(self.top_scroll.0.borrow().base_handle.offset().y);
        let actual = -f32::from(self.editor.read(cx).scroll_offset().y);
        self.pending_result = match self.pending_result {
            Some((y, left)) if (actual - y).abs() > 0.5 && left > 0 => Some((y, left - 1)),
            _ => None,
        };
        let result = self.pending_result.map_or(actual, |(y, _)| y);
        let row_h = f32::from(self.settings.diff_row_h());
        let line_h = self.editor.read(cx).line_height().map_or(row_h, f32::from);
        let moved = |now: f32, then: f32| (now - then).abs() > 0.5;
        let follower = self.follower.take();
        let (top_moved, result_moved) = (moved(top, self.synced.0), moved(result, self.synced.1));
        if top_moved && follower != Some(SyncPane::Top) {
            // Clamped here already: a follower clamping itself later would look like the
            // user scrolling it.
            let lines = self.session.text().lines().count() as f32 + 1.;
            let view_h = self.pane_bounds.get().map_or(0., |b| f32::from(b.size.height));
            let max = (lines * line_h - view_h).max(0.);
            let y = (self.session.result_line_for_top(top / row_h) * line_h).clamp(0., max);
            self.editor.update(cx, |e, cx| {
                let x = e.scroll_offset().x;
                e.set_scroll_offset(point(x, px(-y)), cx)
            });
            self.pending_result = Some((y, 2));
            self.synced = (top, y);
            self.follower = Some(SyncPane::Result);
        } else if result_moved && follower != Some(SyncPane::Result) {
            let mut y = self.session.top_row_for_result(result / line_h) * row_h;
            let max = f32::from(self.top_scroll.0.borrow().base_handle.max_offset().y);
            if max > 0. {
                y = y.min(max);
            }
            self.top_scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), px(-y.max(0.))));
            self.synced = (y.max(0.), result);
            self.follower = Some(SyncPane::Top);
        } else {
            // A follower clamped at its end (or nothing moved): take it as it is.
            self.synced = (top, result);
        }
    }

    /// For tests: (first top row, first result line, the result line matching that top row,
    /// top scrolled to its end).
    pub fn script_scroll_state(&self, cx: &App) -> (f32, f32, f32, bool) {
        let handle = self.top_scroll.0.borrow().base_handle.clone();
        let top = -f32::from(handle.offset().y);
        let max = f32::from(handle.max_offset().y);
        let row_h = f32::from(self.settings.diff_row_h());
        let line_h = self.editor.read(cx).line_height().map_or(row_h, f32::from);
        let result = -f32::from(self.editor.read(cx).scroll_offset().y);
        let row = top / row_h;
        (row, result / line_h, self.session.result_line_for_top(row), top >= max - 0.5)
    }

    /// For tests: see `testing::MergeState`.
    pub fn script_state(&self, window: &Window) -> crate::testing::MergeState {
        crate::testing::MergeState {
            current: self.current,
            chunks: (0..self.session.doc.chunks.len())
                .map(|i| {
                    (
                        format!("{:?}", self.session.doc.chunks[i].kind),
                        format!("{:?}", self.session.state(i)),
                    )
                })
                .collect(),
            single: self.single,
            status: self.status.clone(),
            top_focused: self.top_focus.is_focused(window),
            text: self.session.text().to_string(),
            selected: self.selected_text(),
        }
    }

    /// Script hook: where `needle` (+ `offset` bytes) was laid out on the ours / theirs
    /// side in the last frame.
    pub fn script_text_point(&self, ours: bool, needle: &str, offset: usize) -> Option<Point<Pixels>> {
        let area = if ours { Area::Ours } else { Area::Theirs };
        self.drag_targets.point_of(area, needle, offset)
    }

    pub fn script_focus_top(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.top_focus.focus(window, cx);
    }

    /// The result editor's vertical scroll offset as it will be drawn this frame (a
    /// position `sync_scroll` set is applied only in the editor's layout).
    fn result_scroll(&self, cx: &App) -> Pixels {
        match self.pending_result {
            Some((y, _)) => px(-y),
            None => self.editor.read(cx).scroll_offset().y,
        }
    }

    /// The handlers of the merge window's key actions ([`key_bindings`]).
    fn key_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &Save, w, cx| this.save(false, w, cx)))
            .on_action(cx.listener(|this, _: &ToggleView, w, cx| this.toggle_single(w, cx)))
            .on_action(cx.listener(|this, _: &NextConflict, w, cx| this.step_conflict(1, w, cx)))
            .on_action(cx.listener(|this, _: &PrevConflict, w, cx| this.step_conflict(-1, w, cx)))
            .on_action(cx.listener(|this, _: &NextChange, w, cx| this.step_changed(1, w, cx)))
            .on_action(cx.listener(|this, _: &PrevChange, w, cx| this.step_changed(-1, w, cx)))
            .on_action(cx.listener(|this, _: &TakeOurs, w, cx| {
                this.choose_current(Some(Resolution::Ours), w, cx)
            }))
            .on_action(cx.listener(|this, _: &TakeTheirs, w, cx| {
                this.choose_current(Some(Resolution::Theirs), w, cx)
            }))
            .on_action(cx.listener(|this, _: &TakeOursThenTheirs, w, cx| {
                this.choose_current(Some(Resolution::OursThenTheirs), w, cx)
            }))
            .on_action(cx.listener(|this, _: &TakeTheirsThenOurs, w, cx| {
                this.choose_current(Some(Resolution::TheirsThenOurs), w, cx)
            }))
            .on_action(cx.listener(|this, _: &TakeBase, w, cx| {
                this.choose_current(Some(Resolution::Base), w, cx)
            }))
            .on_action(cx.listener(|this, _: &PutBack, w, cx| this.choose_current(None, w, cx)))
            .on_action(cx.listener(|this, _: &OpenInEditor, w, cx| this.open_in_editor(w, cx)))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy_selection(cx)))
    }

    /// `]` / `[`: the next / previous changed chunk (conflicts and one-sided changes).
    fn step_changed(&mut self, dir: i32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = self.session.step_changed(self.current, dir) {
            self.go_to(c, window, cx);
        }
    }

    /// 1 2 3 4 0 / r: resolves the current chunk (a changed one) — `None` = git's version.
    fn choose_current(
        &mut self,
        r: Option<Resolution>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(c) = self.current
            && self.session.doc.chunks[c].kind != ChunkKind::Stable
        {
            self.choose(c, r, window, cx);
        }
    }

    /// ⌥V / the toolbar switch / the folded bar: result only ↔ ours | theirs over it.
    /// Remembered for the next merge editor (gitty's too).
    fn toggle_single(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.single = !self.single;
        // Ours / theirs fold away: with their focus no key would arrive anymore (not even ⌥V
        // back, nor ⌘S); the result editor takes it over.
        if self.single && self.top_focus.is_focused(window) {
            self.editor.update(cx, |e, cx| e.focus(window, cx));
        }
        let before: SharedSettings = gitti_core::store::load_settings();
        let after = SharedSettings {
            merge_single_view: self.single,
            ..before.clone()
        };
        gitti_core::store::save_changed_settings(&before, &after);
        cx.notify();
    }

    /// Next (`dir` = 1) or previous (-1) conflict.
    fn step_conflict(&mut self, dir: i32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.session.step_conflict(self.current, dir) {
            self.go_to(next, window, cx);
        }
    }

    // ------------------------------------------------------------------ disk

    /// Takes in a change of the file made outside since we last read / wrote it. Returns
    /// whether there was one.
    fn check_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.sync_from_editor(cx);
        match self.session.disk_change() {
            DiskChange::None => return false,
            DiskChange::Same => {}
            DiskChange::Deleted => {
                self.status =
                    Some(("The file was deleted on disk — Save writes it again".into(), true));
                cx.notify();
            }
            DiskChange::Foreign(note) => {
                self.status = Some((note, true));
                cx.notify();
            }
            DiskChange::TookIn(edit) => {
                // Replaces only the differing middle, so cursor and scroll stay put.
                self.apply_edit(edit, window, cx);
                self.status = Some((
                    format!(
                        "Took in a change made to the file outside — {} drops it again",
                        Hints::get(cx).undo
                    ),
                    false,
                ));
            }
        }
        true
    }

    fn save(&mut self, mark_resolved: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.check_disk(window, cx) {
            // A change that couldn't be taken in keeps its own note.
            if self.session.foreign().is_none() {
                self.status = Some((
                    "The file changed on disk — took that in first; check and save again".into(),
                    true,
                ));
            }
            cx.notify();
            return;
        }
        let bytes = match self.session.file_bytes() {
            Ok(bytes) => bytes,
            Err(e) => {
                self.status = Some((format!("{e:#}"), true));
                cx.notify();
                return;
            }
        };
        let path = self.session.doc.path.clone();
        let repo = self.session.repo.clone();
        // Known now, so the disk poll doesn't mistake our own write for an outside one.
        let before = self.session.note_written(bytes.clone());
        let task = cx.background_executor().spawn(async move {
            repo.write_worktree_file(&path, &bytes)?;
            if mark_resolved {
                repo.mark_resolved(&[path])?;
            }
            anyhow::Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) if mark_resolved => window.remove_window(),
                Ok(()) => {
                    this.status = Some(("Saved".into(), false));
                    cx.notify();
                }
                Err(e) => {
                    this.session.restore_on_disk(before);
                    this.status = Some((format!("{e:#}"), true));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Writes the current result (unless the file changed outside meanwhile — that is taken
    /// in instead) and opens it in the configured editor at the current conflict. Edits made
    /// there come back through the disk poll.
    fn open_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.check_disk(window, cx)
            && let Err(e) = self.session.write()
        {
            self.status = Some((format!("{e:#}"), true));
            cx.notify();
            return;
        }
        let line = self.session.line_of(self.current) + 1;
        let repo = self.session.repo.clone();
        let full = repo.fs_path(&self.session.doc.path);
        let opened = crate::app::open_editor(
            &self.settings.editor_command,
            &full,
            line,
            self.session.repo.workdir(),
        );
        self.status = Some(match opened {
            Ok(()) => (format!("Saved and opened in the editor (line {line})"), false),
            Err(e) => (format!("{e:#}"), true),
        });
        cx.notify();
    }

    /// The selected characters of the ours / theirs pane; fully selected lines are copied
    /// as they are in the file (tabs, not the displayed spaces).
    fn selected_text(&self) -> Option<String> {
        let sel = self.text_sel.filter(|s| !s.is_empty())?;
        let (start, end) = sel.ordered();
        let mut out: Vec<String> = Vec::new();
        for ix in start.line..=end.line {
            let Some(TopRow::Line { chunk, i }) = self.session.top_rows.get(ix).copied() else {
                continue;
            };
            let c = &self.session.doc.chunks[chunk];
            let side = if sel.anchor.area == Area::Ours { &c.ours } else { &c.theirs };
            let Some(l) = side.get(i) else { continue };
            let shown = expand_tabs(l);
            let whole = start.line < ix && ix < end.line;
            if whole {
                out.push(l.trim_end_matches(['\n', '\r']).to_string());
            } else if let Some(r) = sel.span_in(sel.anchor.area, ix, shown.len()) {
                out.push(if r == (0..shown.len()) {
                    l.trim_end_matches(['\n', '\r']).to_string()
                } else {
                    shown[r].to_string()
                });
            }
        }
        let text = out.join("\n");
        (!text.is_empty()).then_some(text)
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.status = Some(("Copied".into(), false));
            cx.notify();
        }
    }

    // ------------------------------------------------------------------ rendering

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let hints = Hints::get(cx);
        let total = self.session.doc.conflicts().count();
        let unresolved = self.session.unresolved();
        let markers = self.session.has_conflict_markers();
        let state = if total == 0 {
            "No conflicting changes — all chunks merge automatically".to_string()
        } else if unresolved == 0 {
            format!("All {total} conflicts resolved")
        } else {
            format!("{unresolved} of {total} conflicts unresolved")
        };
        let deleted = match (self.session.doc.ours_deleted, self.session.doc.theirs_deleted) {
            (true, _) => Some(format!("Ours ({}) deleted the file", self.session.sides.ours)),
            (_, true) => Some(format!("Theirs ({}) deleted the file", self.session.sides.theirs)),
            _ => None,
        };
        // Texts stay on one line and shrink with an ellipsis: wrapped inside the fixed-height
        // bar their lines ran out of it, over the pane titles.
        h_flex()
            .probe("merge-toolbar")
            .flex_none()
            .h(self.settings.row_h() + px(14.))
            .overflow_hidden()
            .px_3()
            .gap_2()
            .items_center()
            .bg(p.bg_toolbar)
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .probe("merge-bar-text:0")
                    .min_w(px(40.))
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.text_strong)
                    .child(self.session.doc.path.clone()),
            )
            .child(
                div()
                    .probe("merge-bar-text:1")
                    // Short: the path gives way first.
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(if markers { p.warning } else { p.success })
                    .child(state),
            )
            // Not UTF-8: the file is held in this encoding and written back in it.
            .children(self.session.doc.encoding.label().map(|label| {
                div()
                    .probe("merge-encoding")
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(p.text_muted)
                    .child(label)
            }))
            .children(deleted.map(|d| {
                div()
                    .probe("merge-bar-text:2")
                    .min_w_0()
                    .truncate()
                    .text_color(p.warning)
                    .child(d)
            }))
            .child(div().flex_1())
            .child(
                h_flex()
                    .child(
                        Button::new("view-split")
                            .label("Split")
                            .tooltip(format!("Ours | theirs over the result — {}", hints.view))
                            .small()
                            .ghost()
                            .selected(!self.single)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.single {
                                    this.toggle_single(window, cx)
                                }
                            })),
                    )
                    .child(
                        Button::new("view-single")
                            .label("Single")
                            .tooltip(format!(
                                "Only the result, edited as one file — {}",
                                hints.view
                            ))
                            .small()
                            .ghost()
                            .selected(self.single)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if !this.single {
                                    this.toggle_single(window, cx)
                                }
                            })),
                    ),
            )
            .children(self.status.clone().map(|(text, err)| {
                div()
                    .probe("merge-bar-text:3")
                    .min_w_0()
                    .truncate()
                    .text_color(if err { p.danger } else { p.text_muted })
                    .child(text)
            }))
            .child(
                Button::new("prev-conflict")
                    .label("↑ Previous")
                    .tooltip(format!(
                        "Previous conflict — {} in the ours / theirs panes",
                        hints.prev_conflict
                    ))
                    .small()
                    .ghost()
                    .disabled(total == 0)
                    .on_click(cx.listener(|this, _, w, cx| this.step_conflict(-1, w, cx))),
            )
            .child(
                Button::new("next-conflict")
                    .label("↓ Next Conflict")
                    .tooltip(format!(
                        "{} in the ours / theirs panes; {} go through all changed chunks, \
                         {} {} {} {} {} take ours / theirs / both / base, {} puts back git's \
                         version",
                        hints.next_conflict,
                        hints.changes,
                        hints.take_ours,
                        hints.take_theirs,
                        hints.take_ours_then_theirs,
                        hints.take_theirs_then_ours,
                        hints.take_base,
                        hints.put_back,
                    ))
                    .small()
                    .ghost()
                    .disabled(total == 0)
                    .on_click(cx.listener(|this, _, w, cx| this.step_conflict(1, w, cx))),
            )
            .child(
                Button::new("open-editor")
                    .label("Open in Editor")
                    .tooltip(format!("{} in the ours / theirs panes", hints.open_in_editor))
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, w, cx| this.open_in_editor(w, cx))),
            )
            .child(
                Button::new("save")
                    .label("Save")
                    .tooltip(hints.save.clone())
                    .small()
                    .on_click(cx.listener(|this, _, w, cx| this.save(false, w, cx))),
            )
            .child(
                Button::new("save-resolve")
                    .probe("merge-save-resolve")
                    .label("Save & Mark Resolved")
                    .small()
                    .primary()
                    .disabled(markers)
                    .on_click(cx.listener(|this, _, w, cx| this.save(true, w, cx))),
            )
    }

    fn pane_title(&self, text: String, color: Hsla, p: &Palette) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .px_3()
            .py_1()
            .truncate()
            .bg(p.bg_section)
            .text_color(color)
            .font_weight(FontWeight::BOLD)
            .child(text)
    }

    /// The single view's bar standing in for ours | theirs; a click brings them back.
    fn render_folded_sides(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let total = self.session.doc.conflicts().count();
        let open = self.session.unresolved();
        h_flex()
            .id("folded-sides")
            .probe("merge-folded")
            .flex_none()
            .h(self.settings.row_h() + px(10.))
            .overflow_hidden()
            .px_3()
            .gap_3()
            .items_center()
            .bg(p.bg_section)
            .border_b_1()
            .border_color(p.border)
            .cursor_pointer()
            .hover(|s| s.bg(p.bg_hunk_header))
            .child(div().text_color(p.text_muted).child("▸"))
            .child(
                div()
                    .probe("merge-bar-text:4")
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.lanes[0])
                    .child(format!("Ours — {}", self.session.sides.ours)),
            )
            .child(div().text_color(p.text_faint).child("│"))
            .child(
                div()
                    .probe("merge-bar-text:5")
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.lanes[1])
                    .child(format!("Theirs — {}", self.session.sides.theirs)),
            )
            .when(total > 0, |el| {
                el.child(
                    div()
                        .probe("merge-bar-text:6")
                        .min_w_0()
                        .truncate()
                        .text_color(if open > 0 { p.warning } else { p.success })
                        .child(format!("{open} of {total} conflicts unresolved")),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .probe("merge-bar-text:7")
                    .min_w_0()
                    .truncate()
                    .text_color(p.text_faint)
                    .child(format!(
                        "Single view — click or {} to show the sides",
                        Hints::get(cx).view
                    )),
            )
            .on_click(cx.listener(|this, _, window, cx| this.toggle_single(window, cx)))
    }

    fn render_top(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        // A selection follows the mouse anywhere in the window while the button is held,
        // clamped to its side; the lines register again during this frame's layout.
        self.drag_targets.clear();
        let app = cx.entity().downgrade();
        let drag = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let up_app = app.clone();
                on_window_drag(
                    window,
                    move |pos, _, cx| {
                        app.update(cx, |this, cx| this.drag_text_sel(pos, cx)).ok();
                    },
                    move |_, cx| {
                        up_app.update(cx, |this, _| this.text_dragging = false).ok();
                    },
                )
            },
        )
        .absolute()
        .size_0();
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .border_b_1()
                    .border_color(p.border)
                    .child(self.pane_title(
                        format!("Ours — {}", self.session.sides.ours),
                        p.lanes[0],
                        &p,
                    ))
                    .child(div().w(px(1.)).h_full().bg(p.border))
                    .child(self.pane_title(
                        format!("Theirs — {}", self.session.sides.theirs),
                        p.lanes[1],
                        &p,
                    )),
            )
            .child(
                div()
                    .id("merge-top-pane")
                    .probe("merge-top-pane")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(drag)
                    .track_focus(&self.top_focus)
                    .key_context(SIDES_CONTEXT)
                    .child(
                        uniform_list(
                            "merge-top",
                            self.session.top_rows.len(),
                            cx.processor(|this, range: Range<usize>, _, cx| {
                                range.map(|ix| this.render_top_row(ix, cx)).collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.top_scroll)
                        .size_full(),
                    )
                    // Thumb drags, track clicks do nothing, no row.
                    .children(crate::scroll_guard::scrollbar(
                        "merge-top",
                        &self.top_scroll,
                        false,
                    )),
            )
    }

    /// Mouse moved anywhere in the window with the button held: extends the selection to
    /// the nearest position on the side it started on.
    fn drag_text_sel(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mut sel) = self.text_sel.filter(|_| self.text_dragging) else {
            return;
        };
        let area = sel.anchor.area;
        let Some((at, under)) = self.drag_targets.hit(pos, |a| a == area) else {
            return;
        };
        // A word / line selection extends by whole words / lines.
        let span = sel
            .unit
            .map(|(unit, _, _)| unit_at(unit, at, under, self.side_rows(area)));
        if sel.drag_to(at, span) {
            self.text_sel = Some(sel);
            cx.notify();
        }
    }

    /// The rows of side `area` (`Ours` / `Theirs`) as displayed, by top row index: the
    /// side's line of a chunk row (tabs expanded), nothing for other rows.
    fn side_rows(&self, area: Area) -> impl Fn(usize) -> Option<Row> + '_ {
        move |ix| {
            let TopRow::Line { chunk, i } = *self.session.top_rows.get(ix)? else {
                return None;
            };
            let c = &self.session.doc.chunks[chunk];
            let side = if area == Area::Ours { &c.ours } else { &c.theirs };
            side.get(i).map(|l| Row::line(expand_tabs(l)))
        }
    }

    /// Background of a side's line `i` in a chunk.
    fn side_bg(&self, chunk: usize, ours: bool, i: usize, p: &Palette) -> Option<Hsla> {
        let c = &self.session.doc.chunks[chunk];
        let changed = match c.kind {
            ChunkKind::Stable => return None,
            ChunkKind::Ours => ours,
            ChunkKind::Theirs => !ours,
            ChunkKind::Both | ChunkKind::Conflict => true,
        };
        if !changed {
            return None;
        }
        let taken = match self.session.state(chunk) {
            ChunkState::Unresolved => return Some(p.warning.opacity(0.22)),
            ChunkState::Edited => return Some(p.lanes[4].opacity(0.18)),
            // Identical on both sides: taking either takes both.
            ChunkState::Taken(Resolution::Ours | Resolution::Theirs)
                if c.kind == ChunkKind::Both =>
            {
                true
            }
            ChunkState::Taken(Resolution::Base) => false,
            ChunkState::Taken(Resolution::Ours) => ours,
            ChunkState::Taken(Resolution::Theirs) => !ours,
            // Both changes: a base line the other side changed is left out.
            ChunkState::Taken(r @ (Resolution::OursThenTheirs | Resolution::TheirsThenOurs)) => {
                self.session.side_line_kept(chunk, r, ours, i)
            }
        };
        Some(if taken {
            p.bg_added
        } else {
            p.bg_removed.opacity(0.45)
        })
    }

    fn render_top_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let row_h = self.settings.diff_row_h();
        let base = h_flex().id(("top", ix)).h(row_h).w_full().overflow_hidden();
        match self.session.top_rows[ix] {
            TopRow::Header(ci) => {
                let c = &self.session.doc.chunks[ci];
                let current = self.current == Some(ci);
                let kind = match c.kind {
                    ChunkKind::Ours => "changed in ours",
                    ChunkKind::Theirs => "changed in theirs",
                    ChunkKind::Both => "same change on both sides",
                    ChunkKind::Conflict => "conflict",
                    ChunkKind::Stable => "",
                };
                let state = self.session.state(ci);
                let note = match state {
                    ChunkState::Edited => Some("edited by hand"),
                    ChunkState::Unresolved => Some("unresolved"),
                    ChunkState::Taken(_) => None,
                };
                let taken = |r| self.session.is_taken(ci, r);
                let same = c.kind == ChunkKind::Both;
                let hints = Hints::get(cx);
                let btn = |id: &'static str, label: &'static str, r: Resolution, active: bool| {
                    Button::new((id, ix))
                        .probe(format!("merge-{id}:{ix}"))
                        .label(label)
                        .tooltip(format!(
                            "{}{} in the ours / theirs panes (for the current chunk)",
                            both_tip(r),
                            hints.take(r)
                        ))
                        .xsmall()
                        .when(!active, |b| b.ghost())
                        .when(active, |b| b.primary())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            // Clicking the active choice again puts back git's version.
                            let r = (!active).then_some(r);
                            this.choose(ci, r, window, cx)
                        }))
                };
                let half = |el: Stateful<Div>| {
                    el.flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .h_full()
                        .px_2()
                        .gap_2()
                        .flex()
                        .items_center()
                        .bg(if current { p.bg_file_header } else { p.bg_hunk_header })
                        .text_size(self.settings.ui_small())
                        .text_color(p.text_muted)
                };
                base.child(
                    half(div().id(("th-o", ix)))
                        .child(btn(
                            "take-ours",
                            "Take Ours ▶",
                            Resolution::Ours,
                            taken(Resolution::Ours) || (same && taken(Resolution::Theirs)),
                        ))
                        .child(div().min_w_0().truncate().child(kind))
                        .children(
                            note.map(|n| div().min_w_0().truncate().text_color(p.warning).child(n)),
                        ),
                )
                .child(div().w(px(1.)).h_full().bg(p.border))
                .child(
                    half(div().id(("th-t", ix)))
                        .when(!same, |d| {
                            d.child(btn(
                                "ours-theirs",
                                "Ours + Theirs",
                                Resolution::OursThenTheirs,
                                taken(Resolution::OursThenTheirs),
                            ))
                            .child(btn(
                                "theirs-ours",
                                "Theirs + Ours",
                                Resolution::TheirsThenOurs,
                                taken(Resolution::TheirsThenOurs),
                            ))
                        })
                        .child(btn(
                            "base",
                            "Base",
                            Resolution::Base,
                            taken(Resolution::Base) && c.base != c.ours && c.base != c.theirs,
                        ))
                        .child(div().flex_1())
                        .child(btn(
                            "take-theirs",
                            "◀ Take Theirs",
                            Resolution::Theirs,
                            taken(Resolution::Theirs) || (same && taken(Resolution::Ours)),
                        )),
                )
                .into_any_element()
            }
            TopRow::Line { chunk, i } => {
                let c = &self.session.doc.chunks[chunk];
                let (so, st) = self.session.line_start[chunk];
                let cell = |lines: &[String], start: usize, ours: bool| {
                    let gutter = div()
                        .w(px(self.settings.diff_font_size * 3.4))
                        .flex_none()
                        .pr_2()
                        .text_right()
                        .text_color(p.text_faint);
                    let Some(l) = lines.get(i) else {
                        return h_flex().flex_1().min_w_0().h_full().bg(p.bg_hunk_gap);
                    };
                    let area = if ours { Area::Ours } else { Area::Theirs };
                    let text: SharedString = expand_tabs(l).into();
                    let sel = self.text_sel.and_then(|s| s.span_in(area, ix, text.len()));
                    let styles = overlay_selection(Vec::new(), sel, p.text_selection, text.len());
                    let styled = StyledText::new(text.clone()).with_highlights(styles);
                    let layout = styled.layout().clone();
                    let target = self.drag_targets.register(area, ix, layout.clone(), text.clone());
                    let side = if ours { "ours" } else { "theirs" };
                    // A line longer than its half is clipped there: unclipped it ran
                    // on under the other side's text, both drawn over each other.
                    h_flex()
                        .probe(format!("merge-cell:{ix}:{side}"))
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .items_center()
                        .when_some(self.side_bg(chunk, ours, i, &p), |d, bg| d.bg(bg))
                        .child(gutter.child((start + i).to_string()))
                        .child(
                            div()
                                .probe(format!("merge-text:{ix}:{side}"))
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .cursor(CursorStyle::IBeam)
                                .text_color(p.text)
                                .child(styled),
                        )
                        .child(target)
                        // Drag selects characters on this side (shift-click extends), see
                        // `drag_text_sel`.
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                this.top_focus.focus(window, cx);
                                this.text_dragging = true;
                                let pos = TextPos {
                                    area,
                                    line: ix,
                                    byte: text_index(&layout, &text, ev.position),
                                };
                                let under = char_index(&layout, &text, ev.position);
                                let extend = ev.modifiers.shift
                                    && this.text_sel.is_some_and(|s| s.anchor.area == area);
                                let sel = press(
                                    this.text_sel,
                                    pos,
                                    under,
                                    ev.click_count,
                                    extend,
                                    this.side_rows(area),
                                );
                                this.text_sel = Some(sel);
                                cx.notify();
                            }),
                        )
                };
                base.font_family(mono_font())
                    .text_size(self.settings.diff())
                    .child(cell(&c.ours, so, true))
                    .child(div().w(px(1.)).h_full().bg(p.border))
                    .child(cell(&c.theirs, st, false))
                    .into_any_element()
            }
        }
    }

    fn render_result(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let (under, over) = self.result_layers(cx);
        let pane_bounds = self.pane_bounds.clone();
        // The editor may scroll during its own layout (following the cursor, a deferred
        // offset); then the layers above were placed for the old offset — draw again.
        let editor = self.editor.clone();
        let editor2 = self.editor.clone();
        let drawn_for = self.result_scroll(cx);
        let text_origin = self.text_origin.clone();
        let first_layout = self.text_origin.get().is_none();
        v_flex()
            .flex_1()
            .min_h_0()
            .border_t_2()
            .border_color(p.border)
            .child(
                h_flex()
                    .flex_none()
                    .border_b_1()
                    .border_color(p.border)
                    .child(self.pane_title(
                        "Result — editable, saved as the file".into(),
                        p.text_strong,
                        &p,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .probe("merge-result-pane")
                    .child(
                        canvas(
                            move |bounds, _, _| pane_bounds.set(Some(bounds)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children(under)
                    .child(
                        div().size_full().child(
                            // Same font, size and row height as the ours / theirs rows
                            // (the editor sets its own code font otherwise; only a style
                            // on the editor itself refines over that).
                            Editor::new(&self.editor)
                                .h(relative(1.))
                                .font_family(mono_font())
                                .text_size(self.settings.diff())
                                .line_height(self.settings.diff_row_h())
                                .appearance(false)
                                .bordered(false),
                        ),
                    )
                    .children(over)
                    .child(
                        canvas(
                            move |_, window, cx| {
                                if first_layout || editor.read(cx).scroll_offset().y != drawn_for {
                                    window.refresh();
                                }
                            },
                            move |bounds, _, _, cx| {
                                // `bounds` sits at the pane's origin (absolute, size 0).
                                let e = editor2.read(cx);
                                if let Some(t) = e.text_bounds() {
                                    let s = e.scroll_offset();
                                    text_origin.set(Some(point(
                                        t.origin.x - bounds.origin.x - s.x,
                                        t.origin.y - bounds.origin.y - s.y,
                                    )));
                                }
                            },
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_0(),
                    ),
            )
    }

    /// What is drawn around the editor's text, positioned from the editor's current scroll
    /// offset and line height (so it moves in the same frame as the text): behind it the
    /// full-width line backgrounds by origin; over it the origin stripes at the text's left
    /// edge, a frame around the current chunk and each changed chunk's button bar.
    fn result_layers(&self, cx: &mut Context<Self>) -> (Vec<AnyElement>, Vec<AnyElement>) {
        let (mut under, mut over) = (Vec::new(), Vec::new());
        let editor = self.editor.read(cx);
        let (Some(pane), Some(text), Some(lh)) = (
            self.pane_bounds.get(),
            self.text_origin.get(),
            editor.line_height(),
        ) else {
            return (under, over);
        };
        let scroll = self.result_scroll(cx);
        let left = text.x;
        let top = text.y + scroll;
        let y = |line: usize| top + lh * line as f32;
        // Only what can be seen (a few rows of slack for the bars).
        let first = ((-scroll) / lh).floor().max(0.) as usize;
        let last = first + (pane.size.height / lh).ceil() as usize + 1;
        let visible = |r: &Range<usize>| r.start <= last && r.end + 1 >= first;
        let p = cx.palette().clone();

        for (lines, origin) in self.session.runs().iter().filter(|(r, _)| visible(r)) {
            let (bg, stripe) = match origin {
                Origin::Ours => (Some(p.bg_added.opacity(0.6)), p.lanes[0]),
                Origin::Theirs => (Some(p.bg_added.opacity(0.6)), p.lanes[1]),
                Origin::Both => (Some(p.bg_added.opacity(0.6)), p.success),
                Origin::Base => (None, p.text_faint),
                Origin::Marker => (Some(p.warning.opacity(0.22)), p.warning),
                Origin::Edited => (Some(p.lanes[4].opacity(0.2)), p.lanes[4]),
            };
            let h = lh * lines.len() as f32;
            if let Some(bg) = bg {
                under.push(
                    div()
                        .absolute()
                        .left(left)
                        .right_0()
                        .top(y(lines.start))
                        .h(h)
                        .bg(bg)
                        .into_any_element(),
                );
            }
            over.push(
                div()
                    .absolute()
                    .left(left - px(4.))
                    .w(px(3.))
                    .top(y(lines.start))
                    .h(h)
                    .bg(stripe)
                    .into_any_element(),
            );
        }
        if let Some(c) = self.current
            && let r = &self.session.regions()[c]
            && visible(r)
        {
            over.push(
                div()
                    .absolute()
                    .left(left - px(5.))
                    .right(px(1.))
                    .top(y(r.start))
                    .h((lh * r.len() as f32).max(px(2.)))
                    .border_1()
                    .border_color(p.accent)
                    .into_any_element(),
            );
        }
        for ci in (0..self.session.doc.chunks.len()).filter(|&ci| {
            self.session.doc.chunks[ci].kind != ChunkKind::Stable
                && visible(&self.session.regions()[ci])
        }) {
            over.push(self.result_bar(ci, y(self.session.regions()[ci].start), lh, cx));
        }
        (under, over)
    }

    /// The take-buttons of chunk `ci`, at the right end of its first result line.
    fn result_bar(&self, ci: usize, y: Pixels, lh: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let c = &self.session.doc.chunks[ci];
        let state = self.session.state(ci);
        let label = match state {
            ChunkState::Unresolved => "Unresolved conflict".to_string(),
            ChunkState::Edited => "Edited by hand".into(),
            ChunkState::Taken(Resolution::Ours) if c.kind == ChunkKind::Both => {
                "Same change on both sides".into()
            }
            ChunkState::Taken(Resolution::Ours) => format!("Ours ({})", self.session.sides.ours),
            ChunkState::Taken(Resolution::Theirs) => {
                format!("Theirs ({})", self.session.sides.theirs)
            }
            ChunkState::Taken(r @ (Resolution::OursThenTheirs | Resolution::TheirsThenOurs)) => {
                capitalized(self.session.both_description(ci, r))
            }
            ChunkState::Taken(Resolution::Base) => "Base (neither change)".into(),
        };
        let auto = match c.auto_resolution() {
            Some(r) => state == ChunkState::Taken(r),
            None => state == ChunkState::Unresolved,
        };
        let btn = |id: &'static str, text: &'static str, r: Option<Resolution>| {
            let active = r.is_some_and(|r| self.session.is_taken(ci, r));
            Button::new((id, ci))
                .label(text)
                .xsmall()
                .when(!active, |b| b.ghost())
                .when(active, |b| b.primary())
                .on_click(cx.listener(move |this, _, window, cx| this.choose(ci, r, window, cx)))
        };
        h_flex()
            .id(("result-bar", ci))
            .absolute()
            .right(px(14.))
            .top(y)
            .h(lh)
            .px_1()
            .gap_1()
            .items_center()
            .bg(if self.current == Some(ci) { p.bg_file_header } else { p.bg_hunk_header })
            .text_size(self.settings.ui_small())
            .child(
                div()
                    .px_1()
                    .text_color(match state {
                        ChunkState::Unresolved => p.warning,
                        ChunkState::Edited => p.lanes[4],
                        ChunkState::Taken(_) => p.text,
                    })
                    .child(label),
            )
            .when(auto && c.kind != ChunkKind::Conflict, |d| {
                d.child(div().text_color(p.text_faint).child("(automatic)"))
            })
            .child(btn("r-ours", "Ours", Some(Resolution::Ours)))
            .child(btn("r-theirs", "Theirs", Some(Resolution::Theirs)))
            .when(c.kind != ChunkKind::Both, |d| {
                d.child(btn("r-ot", "Ours + Theirs", Some(Resolution::OursThenTheirs)))
                    .child(btn("r-to", "Theirs + Ours", Some(Resolution::TheirsThenOurs)))
            })
            .child(btn("r-base", "Base", Some(Resolution::Base)))
            .when(!auto, |d| d.child(btn("r-reset", "Reset", None)))
            .into_any_element()
    }
}

/// What "Ours + Theirs" / "Theirs + Ours" do, for their tooltips ("" for the others).
fn both_tip(r: Resolution) -> &'static str {
    match r {
        Resolution::OursThenTheirs => {
            "Both changes — ours first where both add lines at the same place; where both \
             changed the same lines, all of ours, then all of theirs. "
        }
        Resolution::TheirsThenOurs => {
            "Both changes — theirs first where both add lines at the same place; where both \
             changed the same lines, all of theirs, then all of ours. "
        }
        _ => "",
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

fn expand_tabs(s: &str) -> String {
    s.trim_end_matches(['\n', '\r'])
        .replace('\t', "    ")
        .chars()
        .take(crate::app::MAX_LINE_CHARS)
        .collect()
}

impl Render for MergeEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_scroll(cx);
        crate::theme::sync_rem_size(&self.settings, window, cx);
        let p = cx.palette().clone();
        v_flex()
            .size_full()
            .bg(p.bg_diff)
            .text_color(p.text)
            .text_size(self.settings.ui())
            // ⌘S / ⌥V wherever the focus is (the result editor has no binding for them).
            .key_context(WINDOW_CONTEXT)
            .map(|el| self.key_actions(el, cx))
            // No tooltips after the pointer left the window.
            .child(crate::tooltip_gate::watcher())
            .child(self.render_toolbar(cx))
            .map(|el| {
                if self.single {
                    el.child(self.render_folded_sides(cx))
                } else {
                    el.child(self.render_top(cx))
                }
            })
            .child(self.render_result(cx))
    }
}
