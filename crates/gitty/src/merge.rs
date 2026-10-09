//! Merge editor for one conflicted file (T6), the terminal sibling of gitti's merge window
//! on the same model (`gitti_core::merge_session`): ours | theirs aligned chunk by chunk on
//! top, the result below — or, in the single view (`v`), only the result with the sides
//! folded to a bar. Chunks are taken with single keys; the result is edited in place (`i`,
//! edit mode) or in the external editor (`o`; changes come back through the fs watcher
//! like any outside edit, taken in and undoable here).

use gitti_core::merge::{ChunkKind, Resolution};
use gitti_core::merge_session::{ChunkState, DiskChange, MergeSession, Origin, TopRow};
use gitti_core::rows::expand_line;
use gitti_core::syntax::{self, Category, LineCategories};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::{After, App};
use crate::edit::TextEdit;
use crate::modal::Modal;
use crate::theme::{Palette, blend};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MergePane {
    /// The aligned ours | theirs rows.
    Sides,
    Result,
}

pub struct MergeView {
    pub session: MergeSession,
    /// The chunk the keys act on (a conflict at first).
    pub current: Option<usize>,
    pub focus: MergePane,
    pub top_offset: usize,
    pub result_offset: usize,
    pub hscroll: usize,
    /// Result texts before / after own edits (`u` / `U`).
    undo: Vec<String>,
    redo: Vec<String>,
    /// Syntax colours per line of ours / theirs (whole file, by line number) and the
    /// result (`None` when highlighting is off or the language unknown).
    ours_hl: Option<Vec<LineCategories>>,
    theirs_hl: Option<Vec<LineCategories>>,
    result_hl: Option<Vec<LineCategories>>,
    lang: Option<&'static str>,
    /// Edit mode: the result as an editable buffer (kept equal to the session's text).
    pub edit: Option<TextEdit>,
    /// The current run of typing already pushed its undo snapshot.
    burst: bool,
    /// Scroll the result to the edit cursor on the next frame.
    follow_cursor: bool,
    /// Areas of the last frame (mouse hit testing, paging); `sides_area` is the folded bar
    /// in the single view. `text_x`: first column of the result text.
    pub sides_area: Rect,
    pub result_area: Rect,
    text_x: u16,
    /// `top_offset` / `result_offset` as last synchronized (see `draw`).
    synced: (usize, usize),
}

/// Rows of context kept above a chunk navigated to.
const CONTEXT_ROWS: usize = 3;

impl MergeView {
    pub fn new(session: MergeSession, syntax_on: bool) -> Self {
        let lang = syntax_on
            .then(|| syntax::language_for_path(&session.doc.path))
            .flatten();
        let side_text = |ours: bool| -> String {
            session
                .doc
                .chunks
                .iter()
                .flat_map(|c| if ours { &c.ours } else { &c.theirs })
                .map(String::as_str)
                .collect()
        };
        let hl = |text: String| lang.and_then(|l| syntax::highlight(l, &text));
        let mut this = Self {
            ours_hl: hl(side_text(true)),
            theirs_hl: hl(side_text(false)),
            result_hl: None,
            lang,
            current: None,
            focus: MergePane::Sides,
            top_offset: 0,
            result_offset: 0,
            hscroll: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            edit: None,
            burst: false,
            follow_cursor: false,
            sides_area: Rect::default(),
            result_area: Rect::default(),
            text_x: 0,
            synced: (0, 0),
            session,
        };
        this.text_changed();
        let first = this
            .session
            .doc
            .conflicts()
            .next()
            .or_else(|| this.session.step_changed(None, 1));
        if let Some(c) = first {
            this.go_to(c);
        }
        this
    }

    /// After the session's text changed: the edit buffer follows (cursor kept), colours.
    fn text_changed(&mut self) {
        if let Some(e) = &mut self.edit
            && e.text() != self.session.text()
        {
            e.replace_text(self.session.text());
            self.burst = false;
        }
        self.result_hl = self
            .lang
            .and_then(|l| syntax::highlight(l, self.session.text()));
    }

    /// Makes `chunk` current and scrolls both panes to it.
    pub fn go_to(&mut self, chunk: usize) {
        self.current = Some(chunk);
        if let Some(row) = self.session.top_row_of(chunk) {
            self.top_offset = row.saturating_sub(CONTEXT_ROWS);
        }
        self.result_offset = self.session.regions()[chunk]
            .start
            .saturating_sub(CONTEXT_ROWS);
    }

    /// Next / previous changed chunk (conflicts and the automatically merged ones).
    fn step_changed(&mut self, dir: i32) {
        if let Some(c) = self.session.step_changed(self.current, dir) {
            self.go_to(c);
        }
    }

    fn choose(&mut self, r: Option<Resolution>) -> bool {
        let Some(chunk) = self.current else {
            return false;
        };
        if self.session.doc.chunks[chunk].kind == ChunkKind::Stable {
            return false;
        }
        let before = self.session.text().to_string();
        self.session.choose(chunk, r);
        if self.session.text() != before {
            self.undo.push(before);
            self.redo.clear();
            self.text_changed();
        }
        true
    }

    fn undo_redo(&mut self, undo: bool) -> bool {
        let (from, to) = if undo {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some(text) = from.pop() else {
            return false;
        };
        to.push(self.session.text().to_string());
        self.session.set_text(text);
        self.text_changed();
        true
    }

    /// Takes in a change of the file made outside (`None` = nothing changed).
    pub fn check_disk(&mut self) -> Option<(String, bool)> {
        let before = self.session.text().to_string();
        match self.session.disk_change() {
            DiskChange::None | DiskChange::Same => None,
            DiskChange::Deleted => Some((
                "The file was deleted on disk — s writes it again".into(),
                true,
            )),
            DiskChange::TookIn(_) => {
                self.undo.push(before);
                self.redo.clear();
                self.text_changed();
                Some((
                    "Took in a change made to the file outside — u drops it again".into(),
                    false,
                ))
            }
        }
    }

    /// Enters edit mode with the cursor at the start of the current chunk.
    pub fn start_edit(&mut self) {
        let mut e = TextEdit::multi();
        e.set_text(self.session.text());
        let row = self
            .current
            .map_or(self.result_offset, |c| self.session.regions()[c].start);
        e.set_position(row, 0);
        self.edit = Some(e);
        self.burst = false;
        self.focus = MergePane::Result;
        self.follow_cursor = true;
    }

    /// A key in edit mode (esc, ctrl-s etc. are handled by the caller). Returns a status
    /// text if there is something to say.
    fn edit_key(&mut self, key: KeyEvent) -> Option<&'static str> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.page(MergePane::Result).unsigned_abs();
        self.follow_cursor = true;
        match key.code {
            KeyCode::Char('z') if ctrl => {
                return (!self.undo_redo(true)).then_some("Nothing to undo");
            }
            KeyCode::Char('y') if ctrl => {
                return (!self.undo_redo(false)).then_some("Nothing to redo");
            }
            KeyCode::Char(c @ ('n' | 'p')) if ctrl => {
                let dir = if c == 'n' { 1 } else { -1 };
                let Some(chunk) = self.session.step_conflict(self.current, dir) else {
                    return Some("No conflicts");
                };
                self.go_to(chunk);
                let row = self.session.regions()[chunk].start;
                if let Some(e) = &mut self.edit {
                    e.set_position(row, 0);
                }
                self.burst = false;
                return None;
            }
            _ => {}
        }
        let e = self.edit.as_mut()?;
        let (row, col) = e.position();
        match key.code {
            KeyCode::PageDown => e.set_position(row + page, col),
            KeyCode::PageUp => e.set_position(row.saturating_sub(page), col),
            KeyCode::Home if ctrl => e.set_position(0, 0),
            KeyCode::End if ctrl => e.set_position(usize::MAX, usize::MAX),
            _ => {
                let before = e.text();
                if key.code == KeyCode::Tab {
                    e.insert_str("\t");
                } else {
                    e.handle_key(key);
                }
                let after = e.text();
                if after != before {
                    self.edited(before, after);
                    return None;
                }
            }
        }
        // Only moved: the next change starts a new undo step.
        self.burst = false;
        self.current_from_cursor();
        None
    }

    /// Pastes `text` at the edit cursor.
    pub fn paste(&mut self, text: &str) {
        let Some(e) = self.edit.as_mut() else { return };
        let before = e.text();
        e.insert_str(text);
        let after = e.text();
        self.burst = false;
        self.edited(before, after);
        self.burst = false;
    }

    /// The buffer changed from `before` to `after`: one undo step per run of typing.
    fn edited(&mut self, before: String, after: String) {
        if !self.burst {
            self.undo.push(before);
            self.redo.clear();
            self.burst = true;
        }
        self.session.set_text(after);
        self.result_hl = self
            .lang
            .and_then(|l| syntax::highlight(l, self.session.text()));
        self.current_from_cursor();
        self.follow_cursor = true;
    }

    /// The chunk under the edit cursor becomes current (so 1 / 2 … act on it after esc).
    fn current_from_cursor(&mut self) {
        let Some(e) = &self.edit else { return };
        if let Some(c) = self.session.chunk_at_line(e.position().0)
            && self.session.doc.chunks[c].kind != ChunkKind::Stable
        {
            self.current = Some(c);
        }
    }

    /// Edit mode: puts the cursor where the result was clicked.
    fn click_result(&mut self, x: u16, y: u16) {
        let Some(e) = self.edit.as_mut() else { return };
        let row = self.result_offset + y.saturating_sub(self.result_area.y + 1) as usize;
        let dcol = x.saturating_sub(self.text_x) as usize + self.hscroll;
        let line = self.session.text().split('\n').nth(row).unwrap_or("");
        e.set_position(row, char_col_at(line, dcol));
        self.burst = false;
        self.current_from_cursor();
    }

    /// Unsaved changes (the result differs from the file as last read / written).
    pub fn dirty(&self) -> bool {
        self.session.is_modified()
    }

    fn scroll(&mut self, pane: MergePane, delta: isize) {
        self.follow_cursor = false;
        let (offset, len) = match pane {
            MergePane::Sides => (&mut self.top_offset, self.session.top_rows.len()),
            MergePane::Result => (&mut self.result_offset, self.session.text().lines().count()),
        };
        *offset = offset.saturating_add_signed(delta).min(len.saturating_sub(1));
    }

    fn page(&self, pane: MergePane) -> isize {
        let area = match pane {
            MergePane::Sides => self.sides_area,
            MergePane::Result => self.result_area,
        };
        area.height.saturating_sub(2).max(1) as isize
    }

    /// The chunk shown in row `y` of a pane (mouse clicks).
    fn chunk_at(&self, pane: MergePane, y: u16) -> Option<usize> {
        let area = match pane {
            MergePane::Sides => self.sides_area,
            MergePane::Result => self.result_area,
        };
        let row = y.checked_sub(area.y + 1)? as usize;
        match pane {
            MergePane::Sides => match self.session.top_rows.get(self.top_offset + row)? {
                TopRow::Header(c) | TopRow::Line { chunk: c, .. } => Some(*c),
            },
            MergePane::Result => self.session.chunk_at_line(self.result_offset + row),
        }
    }
}

/// The char index in `line` at display column `dcol` (tabs as four columns).
fn char_col_at(line: &str, dcol: usize) -> usize {
    let mut w = 0;
    for (i, ch) in line.chars().enumerate() {
        let cw = if ch == '\t' { 4 } else { ch.to_string().width() };
        if w + cw > dcol {
            return i;
        }
        w += cw;
    }
    line.chars().count()
}

// ---------------------------------------------------------------------- app glue

impl App {
    /// Opens the merge editor for conflicted file `path`.
    pub fn open_merge_editor(&mut self, path: String) {
        let sides = self.conflict_sides.clone().unwrap_or_else(|| {
            gitti_core::ops::ConflictSides {
                ours: "ours".into(),
                theirs: "theirs".into(),
            }
        });
        // Reads the conflict stages (git): in the background, shown when loaded.
        let repo = self.repo.clone();
        self.start_load(
            move || MergeSession::open(repo, &path, sides),
            crate::app::Msg::MergeLoaded,
        );
    }

    /// The merge session loaded by [`App::open_merge_editor`].
    pub fn merge_loaded(&mut self, session: anyhow::Result<MergeSession>) {
        match session {
            Ok(s) => {
                self.merge = Some(MergeView::new(s, self.opts.syntax));
                self.status = None;
            }
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// Closes the merge editor; asks first when the result isn't saved.
    fn close_merge(&mut self) {
        if self.merge.as_ref().is_some_and(MergeView::dirty) {
            self.modal = Some(Modal::confirm(
                "Close Merge Editor",
                "The merge result is not saved. Close anyway?",
                "Close",
                |app| app.merge = None,
            ));
        } else {
            self.merge = None;
        }
    }

    /// Picks up outside edits of the file being merged (fs watcher, focus, editor return).
    pub fn merge_check_disk(&mut self) {
        if let Some(status) = self.merge.as_mut().and_then(MergeView::check_disk) {
            match status {
                (text, true) => self.set_error(text),
                (text, false) => self.set_info(text),
            }
        }
    }

    fn save_merge(&mut self, mark_resolved: bool) {
        let Some(m) = self.merge.as_mut() else {
            return;
        };
        if let Some((_, false)) = m.check_disk() {
            self.set_error(
                "The file changed on disk — took that in first; check and save again".into(),
            );
            return;
        }
        if mark_resolved && m.session.has_conflict_markers() {
            self.set_error("Conflict markers remain — resolve them first (n: next)".into());
            return;
        }
        if let Err(e) = m.session.write() {
            self.set_error(format!("{e:#}"));
            return;
        }
        let path = m.session.doc.path.clone();
        if mark_resolved {
            self.merge = None;
            self.run_op("Mark resolved", false, After::Nothing, move |r| {
                r.mark_resolved(&[path])
            });
        } else {
            self.set_info(format!("Saved {path}"));
        }
    }

    /// `o`: saves the result (unless the file changed outside — that is taken in instead)
    /// and opens it in the editor at the current chunk.
    fn merge_open_in_editor(&mut self) {
        let Some(m) = self.merge.as_mut() else {
            return;
        };
        let took_in = m.check_disk();
        let written = if took_in.is_some() { Ok(()) } else { m.session.write() };
        let line = m.session.line_of(m.current) + 1;
        let path = m.session.doc.path.clone();
        if let Err(e) = written {
            self.set_error(format!("{e:#}"));
            return;
        }
        self.open_path_in_editor(&path, line);
        if let Some((text, _)) = took_in {
            self.set_info(text);
        }
    }

    /// ctrl-c: quits gitty, asking first when the merge result isn't saved.
    fn quit_from_merge(&mut self) {
        if self.merge.as_ref().is_some_and(MergeView::dirty) {
            self.modal = Some(Modal::confirm(
                "Quit",
                "The merge result is not saved. Quit anyway?",
                "Quit",
                |app| app.quit = true,
            ));
        } else {
            self.quit = true;
        }
    }

    /// `v` / alt-v: result only (sides folded to a bar) ↔ ours | theirs over the result.
    /// Remembered as the shared setting `merge_single_view`.
    pub fn toggle_merge_single(&mut self) {
        self.merge_single = !self.merge_single;
        if let Some(m) = &mut self.merge
            && self.merge_single
        {
            m.focus = MergePane::Result;
        }
    }

    /// Pasted text goes into the result while editing it.
    pub fn merge_paste(&mut self, text: &str) {
        if let Some(m) = &mut self.merge {
            m.paste(text);
        }
    }

    /// Keys while the merge editor is open (after modals, before everything else).
    pub fn merge_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        // Both modes.
        match key.code {
            KeyCode::Char('c') if ctrl => return self.quit_from_merge(),
            KeyCode::Char('s') if ctrl => return self.save_merge(false),
            KeyCode::Char('v') if alt => return self.toggle_merge_single(),
            _ => {}
        }
        if let Some(m) = self.merge.as_mut()
            && m.edit.is_some()
        {
            if key.code == KeyCode::Esc {
                m.edit = None;
                m.burst = false;
            } else if let Some(info) = m.edit_key(key) {
                self.set_info(info);
            }
            return;
        }
        // Keys that reach beyond the view.
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return self.close_merge(),
            KeyCode::Char('v') => return self.toggle_merge_single(),
            KeyCode::Char('s') => return self.save_merge(false),
            KeyCode::Char('S') => return self.save_merge(true),
            KeyCode::Char('o') => return self.merge_open_in_editor(),
            KeyCode::Char(c) if c == crate::kb::ch('?') => {
                let help = crate::kb::text(MERGE_HELP);
                self.modal = Some(Modal::text("Merge Editor Keys", help));
                return;
            }
            _ => {}
        }
        let Some(m) = self.merge.as_mut() else {
            return;
        };
        if self.merge_single {
            m.focus = MergePane::Result;
        }
        let pane = m.focus;
        let info = match key.code {
            KeyCode::Char('i') | KeyCode::Enter => {
                m.start_edit();
                None
            }
            KeyCode::Tab | KeyCode::BackTab if self.merge_single => {
                Some("Single view — v shows ours | theirs")
            }
            KeyCode::Tab | KeyCode::BackTab => {
                m.focus = match m.focus {
                    MergePane::Sides => MergePane::Result,
                    MergePane::Result => MergePane::Sides,
                };
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                m.scroll(pane, 1);
                None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                m.scroll(pane, -1);
                None
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                m.scroll(pane, m.page(pane));
                None
            }
            KeyCode::PageUp => {
                m.scroll(pane, -m.page(pane));
                None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                m.scroll(pane, isize::MIN / 2);
                None
            }
            KeyCode::End | KeyCode::Char('G') => {
                m.scroll(pane, isize::MAX / 2);
                None
            }
            KeyCode::Left | KeyCode::Char('h') => {
                m.hscroll = m.hscroll.saturating_sub(8);
                None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                m.hscroll += 8;
                None
            }
            KeyCode::Char(c @ ('n' | 'N')) => {
                match m.session.step_conflict(m.current, if c == 'n' { 1 } else { -1 }) {
                    Some(c) => {
                        m.go_to(c);
                        None
                    }
                    None => Some("No conflicts — ] / [ go through the changed chunks"),
                }
            }
            KeyCode::Char(c) if c == crate::kb::ch(']') => {
                m.step_changed(1);
                None
            }
            KeyCode::Char(c) if c == crate::kb::ch('[') => {
                m.step_changed(-1);
                None
            }
            KeyCode::Char(c @ ('0'..='4' | 'r')) if !ctrl => {
                let r = match c {
                    '1' => Some(Resolution::Ours),
                    '2' => Some(Resolution::Theirs),
                    '3' => Some(Resolution::OursThenTheirs),
                    '4' => Some(Resolution::TheirsThenOurs),
                    '0' => Some(Resolution::Base),
                    _ => None,
                };
                (!m.choose(r)).then_some("No changed chunk here — n / ] go to the next one")
            }
            KeyCode::Char('u') => (!m.undo_redo(true)).then_some("Nothing to undo"),
            KeyCode::Char('U') | KeyCode::Char('r') => {
                (!m.undo_redo(false)).then_some("Nothing to redo")
            }
            _ => None,
        };
        if let Some(info) = info {
            // The hints name `]` / `[`: as the keyboard layout types them.
            self.set_info(crate::kb::text(info).into_owned());
        }
    }

    /// Mouse in the merge editor: wheel scrolls the pane under it, a click makes the chunk
    /// under it current and focuses that pane.
    pub fn merge_mouse(&mut self, x: u16, y: u16, wheel: Option<isize>) {
        let Some(m) = self.merge.as_mut() else {
            return;
        };
        let pos = Position::new(x, y);
        if self.merge_single && m.sides_area.contains(pos) {
            // The folded sides bar.
            if wheel.is_none() {
                self.toggle_merge_single();
            }
            return;
        }
        let pane = if m.sides_area.contains(pos) {
            MergePane::Sides
        } else if m.result_area.contains(pos) {
            MergePane::Result
        } else {
            return;
        };
        match wheel {
            Some(d) => m.scroll(pane, d),
            None if pane == MergePane::Result && m.edit.is_some() => m.click_result(x, y),
            None => {
                m.focus = pane;
                if let Some(c) = m.chunk_at(pane, y) {
                    m.current = Some(c);
                }
            }
        }
    }
}

pub const MERGE_HELP: &str = "\
Merge editor — ours | theirs on top, the result below

  i / ⏎         edit the result here (esc ends editing; ctrl-z / ctrl-y undo / redo,
                ctrl-n / ctrl-p next / previous conflict, ctrl-s save)
  v / alt-v     single view: only the result, ours | theirs folded to a bar (click it
                or v to bring them back); remembered, also by gitti

  n / N         next / previous conflict
  ] / [         next / previous changed chunk
  1 / 2         take ours / theirs for the current chunk
  3 / 4         take ours then theirs / theirs then ours
  0             take the base (neither change)
  r             put back git's version (conflict markers)
  u / U ctrl-r  undo / redo
  o             save and open the result in the editor (edits come back here)
  s / ctrl-s    save
  S             save and mark resolved (closes the merge editor)
  tab           switch pane (↑↓ PgUp PgDn g G scroll it, ←→ pan)
  click         make the chunk under the mouse current
  ctrl-c        quit gitty (asks if the result isn't saved)
  esc / q       close";

// ---------------------------------------------------------------------- drawing

/// Background of a side's line in a chunk (as in gitti): taken teal, dropped red,
/// unresolved yellow, edited by hand purple.
fn side_bg(m: &MergeView, chunk: usize, ours: bool, p: &Palette) -> Option<Color> {
    let c = &m.session.doc.chunks[chunk];
    let changed = match c.kind {
        ChunkKind::Stable => return None,
        ChunkKind::Ours => ours,
        ChunkKind::Theirs => !ours,
        ChunkKind::Both | ChunkKind::Conflict => true,
    };
    if !changed {
        return None;
    }
    let taken = match m.session.state(chunk) {
        ChunkState::Unresolved => return Some(blend(p.warning, p.bg_diff, 0.22)),
        ChunkState::Edited => return Some(blend(p.lanes[4], p.bg_diff, 0.18)),
        // Identical on both sides: taking either takes both.
        ChunkState::Taken(Resolution::Ours | Resolution::Theirs) if c.kind == ChunkKind::Both => {
            true
        }
        ChunkState::Taken(Resolution::Base) => false,
        ChunkState::Taken(Resolution::Ours) => ours,
        ChunkState::Taken(Resolution::Theirs) => !ours,
        ChunkState::Taken(Resolution::OursThenTheirs | Resolution::TheirsThenOurs) => true,
    };
    Some(if taken {
        p.bg_added
    } else {
        blend(p.bg_removed, p.bg_diff, 0.45)
    })
}

fn state_label(m: &MergeView, chunk: usize) -> String {
    let c = &m.session.doc.chunks[chunk];
    let (ours, theirs) = m.session.labels();
    match m.session.state(chunk) {
        ChunkState::Unresolved => "unresolved".into(),
        ChunkState::Edited => "edited by hand".into(),
        ChunkState::Taken(Resolution::Ours) if c.kind == ChunkKind::Both => {
            "same change on both sides".into()
        }
        ChunkState::Taken(Resolution::Ours) => format!("ours ({ours})"),
        ChunkState::Taken(Resolution::Theirs) => format!("theirs ({theirs})"),
        ChunkState::Taken(Resolution::OursThenTheirs) => "ours, then theirs".into(),
        ChunkState::Taken(Resolution::TheirsThenOurs) => "theirs, then ours".into(),
        ChunkState::Taken(Resolution::Base) => "base (neither change)".into(),
    }
}

fn kind_label(kind: ChunkKind) -> &'static str {
    match kind {
        ChunkKind::Stable => "",
        ChunkKind::Ours => "Changed in ours",
        ChunkKind::Theirs => "Changed in theirs",
        ChunkKind::Both => "Changed alike",
        ChunkKind::Conflict => "Conflict",
    }
}

/// Spans of one line of text: tabs expanded, panned by `hscroll`, syntax colours.
fn text_spans(
    raw: &str,
    cats: Option<&LineCategories>,
    hscroll: usize,
    fg: Color,
    p: &Palette,
) -> Vec<Span<'static>> {
    let raw = raw.trim_end_matches(['\n', '\r']);
    let text: String = expand_line(raw).chars().skip(hscroll).collect();
    let cats = cats
        .map(|c| syntax::segment_ranges(raw, c, hscroll, usize::MAX))
        .unwrap_or_default();
    let mut spans = Vec::new();
    let mut at = 0;
    for (r, cat) in cats {
        let (Some(gap), Some(piece)) = (text.get(at..r.start), text.get(r.clone())) else {
            break;
        };
        spans.push(Span::styled(gap.to_string(), Style::new().fg(fg)));
        let mut style = Style::new().fg(p.syntax(cat).unwrap_or(fg));
        if cat == Category::Comment {
            style = style.add_modifier(Modifier::ITALIC);
        }
        spans.push(Span::styled(piece.to_string(), style));
        at = r.end;
    }
    spans.push(Span::styled(text.get(at..).unwrap_or("").to_string(), Style::new().fg(fg)));
    spans
}

fn put(buf: &mut Buffer, x: u16, y: u16, w: u16, spans: Vec<Span<'_>>) -> u16 {
    if w == 0 {
        return x;
    }
    buf.set_line(x, y, &Line::from(spans), w).0
}

fn title(buf: &mut Buffer, area: Rect, text: String, extra: String, focused: bool, p: &Palette) {
    let row = Rect::new(area.x, area.y, area.width, 1);
    let bg = if focused {
        blend(p.accent, p.border, 0.6)
    } else {
        p.bg_section
    };
    buf.set_style(row, Style::new().bg(bg));
    let style = if focused {
        Style::new().fg(p.accent_text).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(p.text_muted)
    };
    let end = put(buf, area.x + 1, area.y, area.width.saturating_sub(1), vec![
        Span::styled(text, style),
    ]);
    put(
        buf,
        end + 2,
        area.y,
        (area.x + area.width).saturating_sub(end + 2),
        vec![Span::styled(
            extra,
            Style::new().fg(if focused { p.accent_text } else { p.text_faint }),
        )],
    );
}

/// Draws the merge editor into `area` (the panes' place); `single` = only the result,
/// the sides folded to a bar. Returns where the terminal cursor goes (edit mode).
pub fn draw(
    buf: &mut Buffer,
    area: Rect,
    m: &mut MergeView,
    single: bool,
    p: &Palette,
) -> Option<Position> {
    buf.set_style(area, Style::new().bg(p.bg_diff).fg(p.text));
    let top_h = if single {
        1.min(area.height)
    } else {
        (area.height / 2).max(3).min(area.height)
    };
    let sides = Rect::new(area.x, area.y, area.width, top_h);
    let result = Rect::new(area.x, area.y + top_h, area.width, area.height - top_h);
    m.sides_area = sides;
    m.result_area = result;
    // Synchronized scrolling (mapping shared with gitti): ours | theirs scrolled →
    // the result follows before it is drawn; the result moved (scrolled, or following the
    // edit cursor while drawn) → ours | theirs follow before they are drawn. Followers are
    // set within their range, so clamping never looks like the user scrolling them.
    if !single && m.top_offset != m.synced.0 {
        let line = m.session.result_line_for_top(m.top_offset as f32).round() as usize;
        let lines = m.session.text().lines().count() + usize::from(m.edit.is_some());
        let h = result.height.saturating_sub(1) as usize;
        m.result_offset = line.min(lines.saturating_sub(h.min(lines)));
        m.synced = (m.top_offset, m.result_offset);
    }
    let cursor = draw_result(buf, result, m, p);
    if single {
        draw_folded_sides(buf, sides, m, p);
    } else {
        if m.result_offset != m.synced.1 {
            let row = m.session.top_row_for_result(m.result_offset as f32).round() as usize;
            let rows = m.session.top_rows.len();
            let h = sides.height.saturating_sub(1) as usize;
            m.top_offset = row.min(rows.saturating_sub(h.min(rows)));
        }
        draw_sides(buf, sides, m, p);
    }
    m.synced = (m.top_offset, m.result_offset);
    cursor
}

/// The single view's bar standing in for ours | theirs: who is merged, how far, how to
/// get the sides back.
fn draw_folded_sides(buf: &mut Buffer, area: Rect, m: &MergeView, p: &Palette) {
    if area.height == 0 {
        return;
    }
    buf.set_style(area, Style::new().bg(p.bg_section));
    let (ours, theirs) = m.session.labels();
    let total = m.session.doc.conflicts().count();
    let mut spans = vec![
        Span::styled(" ▸ ", Style::new().fg(p.text_muted)),
        Span::styled(format!("Ours — {ours}"), Style::new().fg(p.lanes[0])),
        Span::styled("  │  ", Style::new().fg(p.text_faint)),
        Span::styled(format!("Theirs — {theirs}"), Style::new().fg(p.lanes[1])),
        Span::styled("   ", Style::new()),
    ];
    if total > 0 {
        let open = m.session.unresolved();
        spans.push(Span::styled(
            format!("{open} of {total} conflicts unresolved"),
            Style::new().fg(if open > 0 { p.warning } else { p.success }),
        ));
    }
    spans.push(Span::styled(
        "   single view — v or click here shows the sides",
        Style::new().fg(p.text_faint),
    ));
    put(buf, area.x, area.y, area.width, spans);
}

fn draw_sides(buf: &mut Buffer, area: Rect, m: &mut MergeView, p: &Palette) {
    if area.height < 2 {
        return;
    }
    let focused = m.focus == MergePane::Sides;
    let half = area.width.saturating_sub(1) / 2;
    let left = Rect::new(area.x, area.y, half, area.height);
    let right = Rect::new(area.x + half + 1, area.y, area.width - half - 1, area.height);
    let (ours, theirs) = m.session.labels();
    let total = m.session.doc.conflicts().count();
    let state = if total == 0 {
        "no conflicts — all chunks merge automatically".to_string()
    } else {
        format!("{} of {total} conflicts unresolved", m.session.unresolved())
    };
    let deleted = match (m.session.doc.ours_deleted, m.session.doc.theirs_deleted) {
        (true, _) => " · ours deleted the file",
        (_, true) => " · theirs deleted the file",
        _ => "",
    };
    title(buf, left, format!("Ours — {ours}"), format!("{state}{deleted}"), focused, p);
    title(buf, right, format!("Theirs — {theirs}"), String::new(), focused, p);
    buf.set_style(
        Rect::new(area.x + half, area.y + 1, 1, area.height - 1),
        Style::new().bg(p.border),
    );

    let rows = &m.session.top_rows;
    let h = (area.height - 1) as usize;
    m.top_offset = m.top_offset.min(rows.len().saturating_sub(h.min(rows.len())));
    // Line number width over both sides.
    let max_no = m
        .session
        .line_start
        .last()
        .zip(m.session.doc.chunks.last())
        .map_or(1, |(&(o, t), c)| (o + c.ours.len()).max(t + c.theirs.len()));
    let gutter = max_no.to_string().len() as u16 + 1;
    for (i, row) in rows.iter().skip(m.top_offset).take(h).enumerate() {
        let y = area.y + 1 + i as u16;
        match *row {
            TopRow::Header(ci) => {
                let c = &m.session.doc.chunks[ci];
                let current = m.current == Some(ci);
                let bg = if current { p.bg_file_header } else { p.bg_hunk_header };
                buf.set_style(Rect::new(area.x, y, area.width, 1), Style::new().bg(bg));
                let state = m.session.state(ci);
                let tone = match state {
                    ChunkState::Unresolved => p.warning,
                    ChunkState::Edited => p.lanes[4],
                    ChunkState::Taken(_) => p.success,
                };
                let mut spans = vec![
                    Span::styled(
                        if current { "▶ " } else { "  " },
                        Style::new().fg(p.accent),
                    ),
                    Span::styled(
                        kind_label(c.kind),
                        Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" · ", Style::new().fg(p.text_faint)),
                    Span::styled(state_label(m, ci), Style::new().fg(tone)),
                ];
                if current {
                    spans.push(Span::styled(
                        "   1 ours · 2 theirs · 3 ours+theirs · 4 theirs+ours · 0 base · r reset",
                        Style::new().fg(p.text_faint),
                    ));
                }
                put(buf, area.x, y, area.width, spans);
            }
            TopRow::Line { chunk, i } => {
                let c = &m.session.doc.chunks[chunk];
                let (so, st) = m.session.line_start[chunk];
                for (ours, half_area, lines, start, hl) in [
                    (true, left, &c.ours, so, &m.ours_hl),
                    (false, right, &c.theirs, st, &m.theirs_hl),
                ] {
                    let row = Rect::new(half_area.x, y, half_area.width, 1);
                    let Some(l) = lines.get(i) else {
                        // Filler for the shorter side.
                        buf.set_style(row, Style::new().bg(p.bg_gutter));
                        continue;
                    };
                    let bg = side_bg(m, chunk, ours, p);
                    if let Some(bg) = bg {
                        buf.set_style(row, Style::new().bg(bg));
                    }
                    let no = start + i;
                    let gx = put(
                        buf,
                        row.x,
                        y,
                        gutter,
                        vec![Span::styled(
                            format!("{no:>w$} ", w = gutter as usize - 1),
                            Style::new().fg(p.text_faint),
                        )],
                    );
                    let cats = hl.as_ref().and_then(|h| h.get(no - 1));
                    let fg = if bg.is_some() { p.text_strong } else { p.text };
                    put(
                        buf,
                        gx,
                        y,
                        (row.x + row.width).saturating_sub(gx),
                        text_spans(l, cats, m.hscroll, fg, p),
                    );
                }
            }
        }
    }
}

fn draw_result(buf: &mut Buffer, area: Rect, m: &mut MergeView, p: &Palette) -> Option<Position> {
    if area.height < 2 {
        return None;
    }
    let focused = m.focus == MergePane::Result;
    let markers = m.session.has_conflict_markers();
    let extra = if m.edit.is_some() {
        format!(
            "EDITING — esc done · ctrl-s save · ctrl-z undo · ctrl-n next conflict{}",
            if m.dirty() { " · modified" } else { "" }
        )
    } else {
        format!(
            "{}{} · i edit",
            if m.dirty() { "modified · s save" } else { "saved" },
            if markers {
                ""
            } else {
                " · S save & mark resolved"
            }
        )
    };
    title(
        buf,
        area,
        format!("Result — {}", m.session.doc.path),
        extra,
        focused,
        p,
    );
    let text = m.session.text();
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    // While editing, the empty line after a final newline is a place for the cursor too.
    if m.edit.is_some() && (text.is_empty() || text.ends_with('\n')) {
        lines.push("");
    }
    let h = (area.height - 1) as usize;
    let gutter = lines.len().max(1).to_string().len() as u16 + 1;
    m.text_x = area.x + gutter + 1;
    let text_w = area.width.saturating_sub(gutter + 1) as usize;
    // Edit cursor: line and display column; the view follows it after edit keys.
    let cursor = m.edit.as_ref().map(|e| {
        let (row, col) = e.position();
        let line = e.lines().get(row).map_or("", String::as_str);
        let prefix: String = line.chars().take(col).collect();
        (row, expand_line(&prefix).width())
    });
    if let Some((row, dcol)) = cursor
        && m.follow_cursor
    {
        if row < m.result_offset {
            m.result_offset = row;
        } else if row >= m.result_offset + h {
            m.result_offset = row + 1 - h;
        }
        if dcol < m.hscroll {
            m.hscroll = dcol.saturating_sub(8);
        } else if text_w > 0 && dcol >= m.hscroll + text_w {
            m.hscroll = dcol + 8 - text_w.min(dcol + 8);
        }
        m.follow_cursor = false;
    }
    m.result_offset = m.result_offset.min(lines.len().saturating_sub(h.min(lines.len())));
    let current = m.current.map(|c| m.session.regions()[c].clone());
    for (i, l) in lines.iter().enumerate().skip(m.result_offset).take(h) {
        let y = area.y + 1 + (i - m.result_offset) as u16;
        let row = Rect::new(area.x, y, area.width, 1);
        let origin = m.session.origin_of(i);
        let bg = match origin {
            Some(Origin::Ours | Origin::Theirs | Origin::Both) => Some(p.bg_added),
            Some(Origin::Marker) => Some(blend(p.warning, p.bg_diff, 0.22)),
            Some(Origin::Edited) => Some(blend(p.lanes[4], p.bg_diff, 0.2)),
            Some(Origin::Base) | None => None,
        };
        if let Some(bg) = bg {
            buf.set_style(row, Style::new().bg(bg));
        }
        // Stripe: where the line comes from; the current chunk gets an accent gutter.
        let in_current = current.as_ref().is_some_and(|r| r.contains(&i));
        let stripe = match origin {
            Some(Origin::Ours) => Some(p.lanes[0]),
            Some(Origin::Theirs) => Some(p.lanes[1]),
            Some(Origin::Both) => Some(p.success),
            Some(Origin::Marker) => Some(p.warning),
            Some(Origin::Edited) => Some(p.lanes[4]),
            _ => None,
        };
        let no_style = if in_current {
            Style::new().fg(p.accent_text).bg(p.accent)
        } else {
            Style::new().fg(p.text_faint)
        };
        let mut x = put(
            buf,
            area.x,
            y,
            gutter,
            vec![Span::styled(
                format!("{:>w$} ", i + 1, w = gutter as usize - 1),
                no_style,
            )],
        );
        x = put(
            buf,
            x,
            y,
            1,
            vec![Span::styled(
                if stripe.is_some() { "▌" } else { " " },
                Style::new().fg(stripe.unwrap_or(p.text_faint)),
            )],
        );
        let cats = m.result_hl.as_ref().and_then(|h| h.get(i));
        let fg = if bg.is_some() { p.text_strong } else { p.text };
        put(
            buf,
            x,
            y,
            (area.x + area.width).saturating_sub(x),
            text_spans(l, cats, m.hscroll, fg, p),
        );
    }
    // An empty current chunk (resolved to no lines): a thin marker where it sits.
    if let Some(r) = current
        && r.is_empty()
        && r.start >= m.result_offset
        && r.start < m.result_offset + h
    {
        let y = area.y + 1 + (r.start - m.result_offset) as u16;
        buf.set_style(Rect::new(area.x, y, gutter, 1), Style::new().fg(p.accent));
        put(buf, area.x, y, 1, vec![Span::styled("▔", Style::new().fg(p.accent))]);
    }
    let (row, dcol) = cursor?;
    let visible_row = row.checked_sub(m.result_offset).filter(|&r| r < h)?;
    let visible_col = dcol.checked_sub(m.hscroll).filter(|&c| c < text_w)?;
    Some(Position::new(
        m.text_x + visible_col as u16,
        area.y + 1 + visible_row as u16,
    ))
}
