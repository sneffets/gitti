//! Key and mouse handling. Order: open modal → commit message editor → focused pane →
//! global keys.

use gitti_core::patch::PatchMode;
use gitti_core::rows::{DiffRow, Src};
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::app::{App, Detail, Focus};
use crate::modal::{Modal, SubmitText};
use gitti_core::keys::{self, Command, Scope};
use gitti_core::store::{self, PaneLayout};

use crate::app::Divider;
use crate::sidebar::SideItem;

enum Outcome {
    Keep,
    Close,
    Run(crate::modal::Action),
}

impl App {
    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                self.follow = true;
                self.on_key(k);
            }
            Event::Mouse(m) => self.on_mouse(m),
            Event::FocusGained => {
                self.merge_check_disk();
                self.reload_settings();
                self.refresh(false);
            }
            Event::Paste(text) => self.on_paste(&text),
            _ => {}
        }
    }

    fn on_paste(&mut self, text: &str) {
        let searching = self.search_query().is_some();
        match &mut self.modal {
            Some(Modal::Input { fields, focus, .. }) => {
                if let Some(f) = fields.get_mut(*focus) {
                    f.paste(text);
                }
            }
            Some(m @ Modal::Picker { .. }) => {
                if let Modal::Picker { query, .. } = m {
                    query.insert_str(text);
                }
                m.refilter();
            }
            Some(Modal::Ask { input, .. }) => input.insert_str(text.trim_end_matches('\n')),
            Some(Modal::Edit { edit, .. }) => edit.insert_str(text),
            Some(_) => {}
            None if self.merge.is_some() => self.merge_paste(text),
            None if self.focus == Focus::Message => {
                self.message.insert_str(text);
                self.message_edited();
            }
            None => {}
        }
        if searching {
            self.start_commit_search();
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.modal.is_some() {
            self.modal_key(key);
            return;
        }
        // The running operation's output window takes its keys (`app/op_output.rs`):
        // Esc closes it first.
        if self.op_output.is_some() && self.op_output_key(key) {
            return;
        }
        // Esc while an operation's busy label shows cancels it (`app/op_cancel.rs`).
        if key.code == KeyCode::Esc && self.cancel_op() {
            return;
        }
        // `v` while it shows: its whole output, live.
        if key.code == KeyCode::Char('v')
            && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && self.focus != Focus::Message
            && self.merge.is_none()
            && self.busy_name().is_some()
        {
            self.toggle_op_output();
            return;
        }
        // Info messages go away with the next key; errors stay until esc (or a new one).
        if self
            .status
            .as_ref()
            .is_some_and(|s| !s.error || key.code == KeyCode::Esc)
        {
            self.status = None;
        }
        if self.merge.is_some() {
            self.merge_key(key);
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        // Keys that act everywhere, also in the message editor (ctrl-g: Go to Commit).
        if let Some(cmd) = global_command(&key) {
            self.run_command(cmd);
            return;
        }
        // Search in Changes: its query line has the keys.
        if self.change.editing {
            if self.change.open && self.focus == Focus::Commits {
                self.change_search_key(key);
                return;
            }
            self.change.editing = false;
        }
        if self.focus == Focus::Message {
            match key.code {
                KeyCode::Esc => self.focus = Focus::Detail,
                KeyCode::Char('s') if ctrl => self.commit(),
                KeyCode::Enter if ctrl || alt => self.commit(),
                KeyCode::Tab | KeyCode::BackTab => self.focus = Focus::Detail,
                KeyCode::Char('c') if ctrl => self.request_quit(),
                // Earlier messages in an untouched box (`app/commit_box.rs`); else the
                // cursor moves.
                KeyCode::Up if !ctrl && !alt && self.recall_key(true) => {}
                KeyCode::Down if !ctrl && !alt && self.recall_key(false) => {}
                _ => {
                    let before = self.message.text();
                    self.message.handle_key(key);
                    if self.message.text() != before {
                        self.message_edited();
                    }
                }
            }
            return;
        }
        // The commit graph keys act in every pane (`keys::GRAPH_BINDINGS`).
        if let Some(mv) = graph_move(&key) {
            self.graph_move(mv);
            return;
        }
        // Keys shared with gitti (`gitti_core::keys`) come first.
        if let KeyCode::F(n) = key.code {
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            let name = keys::named_key(&format!("f{n}"), ctrl, alt, shift);
            if let Some(cmd) = keys::lookup(&name, Scope::Panes) {
                self.run_command(cmd);
                return;
            }
        }
        if let KeyCode::Char(ch) = key.code {
            let scope = match self.focus {
                Focus::Commits => Scope::Commits,
                Focus::Detail => Scope::Diff,
                _ => Scope::Panes,
            };
            // On another keyboard layout the bracket keys sit elsewhere (`crate::kb`).
            let key = keys::key_name(ch, ctrl, alt);
            // A greyed menu entry that is this key's command says why instead.
            if !ctrl && !alt && self.keybar_disabled_key(ch) {
                return;
            }
            if let Some(cmd) = keys::lookup_in(crate::kb::layout(), &key, scope) {
                self.run_command(cmd);
                return;
            }
            // The context key bar (`app/keybar.rs`): `o` pages it, a free key of
            // the focused thing's menu runs that entry.
            if !ctrl && !alt {
                if ch == gitti_core::keybar::MORE_KEY {
                    self.keybar_more();
                    return;
                }
                if self.keybar_key(ch) {
                    return;
                }
            }
        }
        let handled = match self.focus {
            Focus::Sidebar => self.sidebar_key(key),
            Focus::Commits => self.commits_key(key),
            Focus::Detail => self.detail_key(key),
            Focus::Log => self.log_key(key),
            Focus::Message => false,
        };
        if handled {
            return;
        }
        match (key.code, ctrl) {
            // gitty-only extras; the shared keys are in `gitti_core::keys`.
            (KeyCode::Char('c'), true) | (KeyCode::Char('q'), false) => self.request_quit(),
            (KeyCode::F(1), _) => self.palette(),
            (KeyCode::Tab, _) => self.cycle_focus(true),
            (KeyCode::BackTab, _) => self.cycle_focus(false),
            (KeyCode::F(5), _) | (KeyCode::Char('r'), true) => self.run_command(Command::Refresh),
            (KeyCode::Char('b'), true) => self.run_command(Command::SwitchBranch),
            // gitti: ⌘⌥W / Ctrl+Alt+W.
            (KeyCode::Char('w'), true) if alt => self.add_worktree_dialog(),
            (KeyCode::Char('l'), true) => self.focus_message(),
            (KeyCode::Char('s'), true) => self.commit(),
            (KeyCode::Char(c), _) if c == crate::kb::ch('=') => self.change_context(1),
            (KeyCode::Char('m'), false) | (KeyCode::Menu, _) => self.context_menu(),
            (KeyCode::Char(c), false) if c == crate::kb::ch('.') => {
                self.context_menu()
            }
            _ => {}
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        // The git log panel joins the cycle while it is shown.
        // A hidden sidebar (H) is left out; h / ← still reach it.
        let mut order = vec![Focus::Sidebar, Focus::Commits, Focus::Detail];
        if !self.show_sidebar {
            order.retain(|f| *f != Focus::Sidebar);
        }
        if self.show_log {
            order.push(Focus::Log);
        }
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(1);
        let n = order.len();
        self.focus = order[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }];
    }

    fn page(&self, area: Rect) -> usize {
        (area.height as usize).saturating_sub(1).max(1)
    }

    // ------------------------------------------------------------------ panes

    fn sidebar_key(&mut self, key: KeyEvent) -> bool {
        let n = self.sidebar.len();
        if n == 0 {
            return false;
        }
        let page = self.page(self.areas.sidebar);
        let c = self.side_cursor;
        let to = match key.code {
            KeyCode::Down | KeyCode::Char('j') => (c + 1).min(n - 1),
            KeyCode::Up | KeyCode::Char('k') => c.saturating_sub(1),
            KeyCode::PageDown => (c + page).min(n - 1),
            KeyCode::PageUp => c.saturating_sub(page),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => n - 1,
            KeyCode::Right => {
                self.focus = Focus::Commits;
                return true;
            }
            // ctrl-l is the global "commit message" key.
            KeyCode::Char('l') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.focus = Focus::Commits;
                return true;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate_side_item();
                return true;
            }
            _ => return false,
        };
        self.side_cursor = to;
        self.side_jump();
        true
    }

    /// Selects the sidebar entry's commit in the list.
    fn side_jump(&mut self) {
        // A worktree's mark says what it means (gitti: its tooltip).
        let info = match self.sidebar.get(self.side_cursor) {
            Some(SideItem::Worktree { index, name }) => self
                .worktree_marks(*index)
                .filter(|m| m.mark().is_some())
                .map(|m| format!("{name}: {}", m.describe().replace('\n', " · "))),
            _ => None,
        };
        if let Some(info) = info {
            self.set_info(info);
        }
        if let Some(row) = self
            .sidebar
            .get(self.side_cursor)
            .and_then(|it| it.target_row(self))
        {
            self.select_row(row);
            self.commit_offset = row.saturating_sub(self.areas.commit_rows() / 3);
        }
    }

    fn activate_side_item(&mut self) {
        let Some(item) = self.sidebar.get(self.side_cursor).cloned() else {
            return;
        };
        if let Some(key) = item.collapse_key() {
            if !self.collapsed.remove(&key) {
                self.collapsed.insert(key);
            }
            self.rebuild_sidebar();
            self.refresh_worktree_marks();
            return;
        }
        match item {
            SideItem::Branch { name, current, .. } => {
                if current {
                    self.set_info(format!("{name} is already checked out"));
                } else {
                    self.checkout(name);
                }
            }
            SideItem::RemoteBranch { full, .. } => {
                self.switch_to(gitti_core::switch::SwitchTarget::Remote(full))
            }
            SideItem::Submodule { path, .. } => self.open_submodule(&path),
            // ⏎: Pull (the subtree's everyday action); its menu has more.
            SideItem::Subtree { index, .. } => {
                if let Some(s) = self.subtrees.get(index).filter(|s| s.present) {
                    let req = gitti_core::subtree::Request::for_subtree(
                        gitti_core::subtree::Action::Pull,
                        s,
                    );
                    self.subtree_dialog(req);
                }
            }
            _ => {
                self.side_jump();
                self.focus = Focus::Commits;
            }
        }
    }

    fn commits_key(&mut self, key: KeyEvent) -> bool {
        if self.change.open && self.change_hits_key(key) {
            return true;
        }
        let n = self.history.commits.len();
        if n == 0 {
            return false;
        }
        // Commits per page: two-line rows show half as many.
        let page = self.areas.commit_rows().saturating_sub(1).max(1);
        let c = self.selected;
        let to = match key.code {
            KeyCode::Down | KeyCode::Char('j') => (c + 1).min(n - 1),
            KeyCode::Up | KeyCode::Char('k') => c.saturating_sub(1),
            KeyCode::PageDown => (c + page).min(n - 1),
            KeyCode::PageUp => c.saturating_sub(page),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                (c + page / 2).min(n - 1)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                c.saturating_sub(page / 2)
            }
            KeyCode::Home => 0,
            KeyCode::End => n - 1,
            KeyCode::Enter | KeyCode::Right => {
                self.focus = Focus::Detail;
                return true;
            }
            // ctrl-l is the global "commit message" key.
            KeyCode::Char('l') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.focus = Focus::Detail;
                return true;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.focus = Focus::Sidebar;
                return true;
            }
            KeyCode::Esc => return self.end_compare(),
            _ => return false,
        };
        self.select_row(to);
        true
    }

    /// Moves the commit selection along the graph (alt-↓ ↑, alt-shift-↓ ↑, ctrl too): the
    /// first parent / the child on the lane, or the other one — a chooser when there are
    /// several (as in gitti); the focus stays.
    fn graph_move(&mut self, mv: keys::GraphMove) {
        if self.history.commits.is_empty() {
            return;
        }
        let targets = if mv.down {
            self.history.parent_rows(self.selected)
        } else {
            self.history.child_rows(self.selected)
        };
        match mv.target(&targets) {
            keys::GraphTarget::None(why) => self.set_info(why),
            keys::GraphTarget::Row(r) => self.select_row(r),
            keys::GraphTarget::Choose => self.graph_chooser(&targets, mv.down),
        }
    }

    /// Picks one of several parents / children: all of them, the branch-continuing one
    /// first (marked), the cursor on the first alternative; typing its number finds it.
    fn graph_chooser(&mut self, targets: &[usize], down: bool) {
        let items = targets
            .iter()
            .enumerate()
            .map(|(n, &row)| {
                let id = self.history.commits.at(row).id;
                let refs = self
                    .history
                    .refs_by_commit
                    .get(&id)
                    .map(|r| r.iter().map(|r| r.short_name.as_str()).collect::<Vec<_>>().join(", "))
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("[{s}] "))
                    .unwrap_or_default();
                let meta = self.history.meta(row);
                let summary = if self.history.is_worktree_row(row) {
                    "Working directory"
                } else {
                    meta.summary.as_str()
                };
                let main = match (n, down) {
                    (0, true) => "first parent · ",
                    (0, false) => "continues branch · ",
                    _ => "",
                };
                let hint = format!("{main}{} · {}", id.to_hex_with_len(7), meta.author);
                let mut it = crate::modal::PickItem::new(
                    format!("{} {refs}{summary}", n + 1),
                    hint,
                    move |app: &mut App| app.select_row(row),
                );
                it.lane = self.history.graph.color(row);
                it
            })
            .collect();
        let title = if down { "Which parent?" } else { "Which child?" };
        let mut m = Modal::picker(title, items);
        if let Modal::Picker { cursor, note, graph, .. } = &mut m {
            *cursor = 1;
            *note = Some("↑↓ ⏎, or type its number".into());
            *graph = targets.to_vec();
        }
        self.modal = Some(m);
    }

    /// The graph chooser's candidates and the one under its cursor (commit rows).
    pub fn nav_marks(&self) -> Option<(&[usize], Option<usize>)> {
        let Some(Modal::Picker { graph, filtered, cursor, .. }) = &self.modal else {
            return None;
        };
        if graph.is_empty() {
            return None;
        }
        let focus = filtered.get(*cursor).and_then(|&i| graph.get(i)).copied();
        Some((graph, focus))
    }

    fn detail_key(&mut self, key: KeyEvent) -> bool {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.page(self.areas.detail) as isize;
        let n = self.rows.len();
        let wt = matches!(self.detail, Detail::Worktree);
        match key.code {
            KeyCode::Down if shift => self.extend_selection(1),
            KeyCode::Up if shift => self.extend_selection(-1),
            KeyCode::Char('J') => self.extend_selection(1),
            KeyCode::Char('K') => self.extend_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_diff_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_diff_cursor(-1),
            KeyCode::PageDown => {
                self.move_diff_cursor(page);
                self.diff_offset += page as usize;
            }
            KeyCode::PageUp => {
                self.move_diff_cursor(-page);
                self.diff_offset = self.diff_offset.saturating_sub(page as usize);
            }
            KeyCode::Char('d') if ctrl => self.move_diff_cursor(page / 2),
            KeyCode::Char('u') if ctrl => self.move_diff_cursor(-page / 2),
            KeyCode::Home | KeyCode::Char('g') => self.diff_cursor = 0,
            KeyCode::End | KeyCode::Char('G') => self.diff_cursor = n.saturating_sub(1),
            // ctrl-l is the global "commit message" key.
            KeyCode::Right | KeyCode::Char('l') if !ctrl => {
                if !self.opts.wrap {
                    self.hscroll += 8;
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if self.hscroll == 0 {
                    self.focus = Focus::Commits;
                } else {
                    self.hscroll = self.hscroll.saturating_sub(8);
                }
            }
            KeyCode::Char(' ') if wt => self.toggle_line(),
            KeyCode::Enter if Some(self.diff_cursor) == self.signature_header_line() => {
                self.show_signature()
            }
            KeyCode::Enter if self.lfs_download_row(self.diff_cursor) => self.lfs_download(),
            // A nested repository's / an untracked folder's row: what it is.
            KeyCode::Enter if self.no_preview_at(self.diff_cursor) => {
                if let Some(&DiffRow::Note { src, file }) = self.rows.get(self.diff_cursor) {
                    self.no_preview_menu(src, file);
                }
            }
            KeyCode::Enter if self.not_loaded_at(self.diff_cursor).is_some() => {
                if let Some(file) = self.not_loaded_at(self.diff_cursor) {
                    self.load_omitted_file(file);
                }
            }
            KeyCode::Enter => match self.rows.get(self.diff_cursor) {
                Some(
                    DiffRow::File {
                        src: Src::Conflicted,
                        file,
                    }
                    | DiffRow::Note {
                        src: Src::Conflicted,
                        file,
                    },
                ) => self.resolve_menu(*file),
                Some(&DiffRow::PathNote { src, file, .. }) => self.path_note_menu(src, file),
                Some(DiffRow::File { .. } | DiffRow::Note { .. } | DiffRow::Submodule { .. }) => {
                    self.toggle_file()
                }
                Some(DiffRow::Group(_)) => self.toggle_group(),
                Some(&DiffRow::Expand { src, file, gap }) => {
                    self.show_more_lines(src, file, Some(gap), gitti_core::context::Reveal::More)
                }
                Some(DiffRow::Hunk { .. } | DiffRow::Line { .. }) if wt => self.toggle_hunk(),
                Some(DiffRow::Hunk { .. } | DiffRow::Line { .. }) => self.toggle_file(),
                _ => {}
            },
            // Esc: clears the line selection, hides revealed unchanged lines ("Show
            // Changes Only"), ends compare mode, then back to the list.
            KeyCode::Esc => {
                if self.line_sel.take().is_none()
                    && !self.show_changes_only(None)
                    && !self.end_compare()
                {
                    self.focus = Focus::Commits;
                }
            }
            _ => return false,
        }
        true
    }

    /// The git log panel has no cursor: the keys scroll it. g / home = oldest entry,
    /// G / end = newest (and follow new ones again).
    fn log_key(&mut self, key: KeyEvent) -> bool {
        let page = self.log_rows().saturating_sub(1).max(1) as isize;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.scroll_log(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_log(-1),
            KeyCode::PageDown => self.scroll_log(page),
            KeyCode::PageUp => self.scroll_log(-page),
            KeyCode::Char('d') if ctrl => self.scroll_log(page / 2),
            KeyCode::Char('u') if ctrl => self.scroll_log(-page / 2),
            KeyCode::Home | KeyCode::Char('g') => self.move_log(|_, _| 0),
            KeyCode::End | KeyCode::Char('G') => self.move_log(|_, max| max),
            KeyCode::Esc => self.focus = Focus::Commits,
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------------ modals

    fn modal_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(self.modal, Some(Modal::Ask { .. })) {
            let Some(Modal::Ask { input, .. }) = &mut self.modal else {
                return;
            };
            match key.code {
                KeyCode::Esc => self.answer_ask(None),
                KeyCode::Enter => {
                    let text = input.text();
                    self.answer_ask(Some(text));
                }
                KeyCode::Char('c') if ctrl => self.answer_ask(None),
                _ => {
                    input.handle_key(key);
                }
            }
            return;
        }
        // Go to Commit searches per query (instead of filtering fixed entries).
        let search_before = self.search_query();
        // ctrl-g (Go to Commit) from a list or text window: it takes the window's place.
        // Dialogs with fields and questions keep the key (closing them would lose input).
        if matches!(
            self.modal,
            Some(Modal::Picker { .. } | Modal::Menu { .. } | Modal::Text { .. } | Modal::Help { .. })
        ) && let Some(cmd) = global_command(&key)
        {
            self.modal = None;
            if search_before.is_some() {
                self.stop_commit_search();
            }
            self.run_command(cmd);
            return;
        }
        let Some(mut modal) = self.modal.take() else {
            return;
        };
        // Enter does nothing while the dialog says why it can't work (`Modal::with_live`).
        let live = modal.live_now();
        let blocked = live.as_ref().is_some_and(|l| l.blocked.is_some());
        let fills = live.map(|l| l.fills).unwrap_or_default();
        let outcome = match &mut modal {
            Modal::Input {
                fields,
                checks,
                focus,
                submit,
                pick,
                extra,
                ..
            } => {
                let total = fields.len() + checks.len();
                match key.code {
                    KeyCode::Esc => Outcome::Close,
                    // The dialog's further action (`Modal::with_extra`).
                    KeyCode::Char(c) if ctrl && extra.as_ref().is_some_and(|x| x.key == c) => {
                        match extra.take() {
                            Some(x) => Outcome::Run(x.f),
                            None => Outcome::Keep,
                        }
                    }
                    // The field's chooser (`Modal::with_pick`).
                    KeyCode::Char(c)
                        if ctrl
                            && c == crate::modal::PICK_KEY
                            && pick.as_ref().is_some_and(|p| p.field == *focus) =>
                    {
                        let values: Vec<String> = fields.iter().map(|f| f.edit.text()).collect();
                        let flags: Vec<bool> = checks.iter().map(|c| c.on).collect();
                        match pick.take() {
                            Some(p) => Outcome::Run(Box::new(move |app: &mut App| (p.f)(app, values, flags))),
                            None => Outcome::Keep,
                        }
                    }
                    KeyCode::Enter if blocked => Outcome::Keep,
                    KeyCode::Enter => {
                        let values: Vec<String> = fields.iter().map(|f| f.edit.text()).collect();
                        let flags: Vec<bool> = checks.iter().map(|c| c.on).collect();
                        match submit.take() {
                            Some(s) => {
                                Outcome::Run(Box::new(move |app: &mut App| s(app, values, flags)))
                            }
                            None => Outcome::Close,
                        }
                    }
                    // The next of the field's fill values (`Live::fills`).
                    KeyCode::Char(c) if ctrl && c == crate::modal::FILL_KEY => {
                        if let Some(f) = fields.get_mut(*focus)
                            && let Some(v) = crate::modal::next_fill(&fills, *focus, &f.edit.text())
                        {
                            f.edit.set_text(v);
                            f.fresh = false;
                        }
                        Outcome::Keep
                    }
                    KeyCode::Tab | KeyCode::Down => {
                        *focus = (*focus + 1) % total.max(1);
                        Outcome::Keep
                    }
                    KeyCode::BackTab | KeyCode::Up => {
                        *focus = (*focus + total.max(1) - 1) % total.max(1);
                        Outcome::Keep
                    }
                    KeyCode::Char(' ') if *focus >= fields.len() => {
                        if let Some(c) = checks.get_mut(*focus - fields.len()) {
                            c.on = !c.on;
                        }
                        Outcome::Keep
                    }
                    _ => {
                        if let Some(f) = fields.get_mut(*focus) {
                            f.key(key);
                        }
                        Outcome::Keep
                    }
                }
            }
            Modal::Confirm { action, .. } => match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => match action.take() {
                    Some(a) => Outcome::Run(a),
                    None => Outcome::Close,
                },
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
                    Outcome::Close
                }
                _ => Outcome::Keep,
            },
            Modal::Menu { items, cursor, .. } => {
                let step = |cursor: &mut usize, down: bool, items: &[crate::modal::MenuItem]| {
                    let n = items.len();
                    let mut c = *cursor;
                    for _ in 0..n {
                        c = if down { (c + 1) % n } else { (c + n - 1) % n };
                        if items[c].action.is_some() {
                            break;
                        }
                    }
                    *cursor = c;
                };
                // An item's own key wins over the menu's close / move keys (the
                // branch menu's "Merge … into" is `m`, which also closes menus; a
                // submodule's "Check Out Commit…" is `k`).
                let item_key = match key.code {
                    KeyCode::Char(ch) if !ctrl => items
                        .iter()
                        .position(|i| i.key == Some(ch) && i.action.is_some()),
                    _ => None,
                };
                match key.code {
                    _ if item_key.is_some() => {
                        match item_key.and_then(|ix| items[ix].action.take()) {
                            Some(a) => Outcome::Run(a),
                            None => Outcome::Keep,
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('m') | KeyCode::Left => {
                        Outcome::Close
                    }
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                        step(cursor, true, items);
                        Outcome::Keep
                    }
                    KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                        step(cursor, false, items);
                        Outcome::Keep
                    }
                    KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown => {
                        // The nearest selectable item at the target (separators are not).
                        let n = items.len();
                        let page = (self.areas.modal_list.height as usize).saturating_sub(1).max(1);
                        let (target, down) = match key.code {
                            KeyCode::Home => (0, true),
                            KeyCode::End => (n.saturating_sub(1), false),
                            KeyCode::PageUp => (cursor.saturating_sub(page), false),
                            _ => ((*cursor + page).min(n.saturating_sub(1)), true),
                        };
                        let ok = |i: &usize| items[*i].action.is_some();
                        let found = if n == 0 {
                            None
                        } else if down {
                            (target..n).find(ok).or_else(|| (0..target).rev().find(ok))
                        } else {
                            (0..=target).rev().find(ok).or_else(|| (target..n).find(ok))
                        };
                        if let Some(i) = found {
                            *cursor = i;
                        }
                        Outcome::Keep
                    }
                    KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') => {
                        match items.get_mut(*cursor).and_then(|i| i.action.take()) {
                            Some(a) => Outcome::Run(a),
                            None => Outcome::Keep,
                        }
                    }
                    _ => Outcome::Keep,
                }
            }
            Modal::Picker {
                query,
                items,
                filtered,
                cursor,
                ..
            } => match key.code {
                KeyCode::Esc => Outcome::Close,
                KeyCode::Down | KeyCode::Tab => {
                    *cursor = (*cursor + 1).min(filtered.len().saturating_sub(1));
                    Outcome::Keep
                }
                KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
                    *cursor = (*cursor + 1).min(filtered.len().saturating_sub(1));
                    Outcome::Keep
                }
                KeyCode::Up | KeyCode::BackTab => {
                    *cursor = cursor.saturating_sub(1);
                    Outcome::Keep
                }
                KeyCode::Char('p') | KeyCode::Char('k') if ctrl => {
                    *cursor = cursor.saturating_sub(1);
                    Outcome::Keep
                }
                KeyCode::PageDown => {
                    *cursor = (*cursor + 10).min(filtered.len().saturating_sub(1));
                    Outcome::Keep
                }
                KeyCode::PageUp => {
                    *cursor = cursor.saturating_sub(10);
                    Outcome::Keep
                }
                // As in gitti: Home / End (also with ctrl) = first / last entry.
                KeyCode::Home => {
                    *cursor = 0;
                    Outcome::Keep
                }
                KeyCode::End => {
                    *cursor = filtered.len().saturating_sub(1);
                    Outcome::Keep
                }
                KeyCode::Enter => match filtered
                    .get(*cursor)
                    .and_then(|&i| items.get_mut(i))
                    .and_then(|i| i.action.take())
                {
                    Some(a) => Outcome::Run(a),
                    None => Outcome::Keep,
                },
                _ => {
                    if query.handle_key(key) {
                        modal.refilter();
                    }
                    Outcome::Keep
                }
            },
            Modal::Text { scroll, body, .. } => {
                // The window may be narrower (more wrapped lines); drawing clamps to the
                // last line it can show.
                let max = crate::ui::text_modal_lines(body, 10).len();
                text_scroll(scroll, max, key)
            }
            Modal::Edit { edit, submit, .. } => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let alt = key.modifiers.contains(KeyModifiers::ALT);
                match key.code {
                    KeyCode::Esc => Outcome::Close,
                    KeyCode::Char('s') if ctrl => save_edit(edit, submit),
                    KeyCode::Enter if ctrl || alt => save_edit(edit, submit),
                    _ => {
                        edit.handle_key(key);
                        Outcome::Keep
                    }
                }
            }
            Modal::Help { scroll } => {
                let max = crate::ui::text_modal_lines(&crate::ui::help_text(), 10).len();
                text_scroll(scroll, max, key)
            }
            Modal::Ask { .. } => Outcome::Keep,
        };
        match outcome {
            Outcome::Keep => {
                self.modal = Some(modal);
                if search_before.is_some() && self.search_query() != search_before {
                    self.start_commit_search();
                }
            }
            Outcome::Close => {}
            Outcome::Run(action) => action(self),
        }
        if search_before.is_some() && self.search_query().is_none() {
            self.stop_commit_search();
        }
    }

    // ------------------------------------------------------------------ mouse

    fn on_mouse(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        if let Some(modal) = &mut self.modal {
            // Click on a menu / picker entry runs it; anything else outside closes.
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                let list = self.areas.modal_list;
                if list.contains(pos) {
                    let i = (m.row - list.y) as usize;
                    // `offset` is the window as last drawn.
                    let action = match modal {
                        Modal::Menu { items, offset, .. } => items
                            .get_mut(*offset + i)
                            .and_then(|it| it.action.take()),
                        Modal::Picker {
                            items,
                            filtered,
                            offset,
                            ..
                        } => filtered
                            .get(*offset + i)
                            .and_then(|&k| items.get_mut(k))
                            .and_then(|it| it.action.take()),
                        _ => None,
                    };
                    if let Some(a) = action {
                        self.modal = None;
                        a(self);
                    }
                }
                return;
            }
            // The wheel scrolls the list / text window without moving its cursor.
            let d: isize = match m.kind {
                MouseEventKind::ScrollDown => 3,
                MouseEventKind::ScrollUp => -3,
                _ => return,
            };
            let h = self.areas.modal_list.height as usize;
            let scroll = |off: &mut usize, len: usize| {
                let max = len.saturating_sub(h) as isize;
                *off = (*off as isize + d).clamp(0, max.max(0)) as usize;
            };
            match modal {
                Modal::Menu { items, offset, .. } => scroll(offset, items.len()),
                Modal::Picker {
                    filtered, offset, ..
                } => scroll(offset, filtered.len()),
                // Drawing clamps to the last line.
                Modal::Text { scroll: s, .. } | Modal::Help { scroll: s } => {
                    *s = s.saturating_add_signed(d)
                }
                _ => {}
            }
            return;
        }
        let a = self.areas;
        // The running operation's output window: the wheel scrolls it, a click
        // outside closes it (the status line's own click toggles it below).
        if self.op_output.is_some() {
            let inside = a.op_output.contains(pos);
            match m.kind {
                MouseEventKind::ScrollDown if inside => return self.scroll_op_output(3),
                MouseEventKind::ScrollUp if inside => return self.scroll_op_output(-3),
                MouseEventKind::Down(_) if inside => return,
                MouseEventKind::Down(_) if !a.status.contains(pos) => {
                    self.op_output = None;
                    return;
                }
                _ => {}
            }
        }
        // The wheel over the git log panel scrolls it (also next to the merge editor).
        if a.log.contains(pos) {
            match m.kind {
                MouseEventKind::ScrollDown => return self.scroll_log(3),
                MouseEventKind::ScrollUp => return self.scroll_log(-3),
                _ => {}
            }
        }
        if self.merge.is_some() {
            match m.kind {
                MouseEventKind::ScrollDown => self.merge_mouse(m.column, m.row, Some(3)),
                MouseEventKind::ScrollUp => self.merge_mouse(m.column, m.row, Some(-3)),
                MouseEventKind::Down(MouseButton::Left) => self.merge_mouse(m.column, m.row, None),
                _ => {}
            }
            return;
        }
        if self.drag_divider(&m) {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d: isize = if m.kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                let scroll = |off: &mut usize, len: usize, h: u16| {
                    let max = len.saturating_sub(h as usize) as isize;
                    *off = (*off as isize + d).clamp(0, max.max(0)) as usize;
                };
                if a.message.contains(pos) {
                    self.message_view.wheel(d);
                } else if a.commits.contains(pos) {
                    let shown = u16::try_from(a.commit_rows()).unwrap_or(u16::MAX);
                    scroll(&mut self.commit_offset, self.history.commits.len(), shown);
                } else if a.detail.contains(pos) {
                    scroll(&mut self.diff_offset, self.rows.len(), a.detail.height);
                } else if a.sidebar.contains(pos) {
                    scroll(&mut self.side_offset, self.sidebar.len(), a.sidebar.height);
                }
            }
            MouseEventKind::ScrollRight if a.detail.contains(pos) => self.hscroll += 4,
            MouseEventKind::ScrollLeft if a.detail.contains(pos) => {
                self.hscroll = self.hscroll.saturating_sub(4)
            }
            MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) => {
                let right = button == MouseButton::Right;
                let shift = m.modifiers.contains(KeyModifiers::SHIFT);
                if a.prepared_use.contains(pos) && a.message.width > 0 {
                    self.use_prepared_message();
                } else if a.prepared_keep.contains(pos) && a.message.width > 0 {
                    self.keep_my_message();
                } else if a.sign_off.contains(pos) && a.message.width > 0 {
                    self.toggle_sign_off();
                } else if a.message.contains(pos) {
                    self.focus = Focus::Message;
                } else if a.log.contains(pos) {
                    self.focus = Focus::Log;
                } else if a.error_mark.contains(pos) {
                    self.show_error_details();
                } else if a.repo_marks.contains(pos) {
                    self.explain_repo_marks();
                } else if a.status.contains(pos) && self.busy_name().is_some() {
                    // A running operation: "esc cancel" cancels, elsewhere the
                    // output window opens / closes.
                    if a.busy_cancel.contains(pos) {
                        self.cancel_op();
                    } else {
                        self.toggle_op_output();
                    }
                } else if a.keybar_more.contains(pos) {
                    self.keybar_more();
                } else if a.status.contains(pos) {
                    // The whole error, otherwise the git log panel at its newest entries
                    // (as in gitti).
                    if let Some(full) = self.status.as_ref().and_then(|s| s.full.clone()) {
                        self.modal = Some(Modal::text("Error", full));
                    } else if !self.show_log {
                        self.toggle_log();
                    } else {
                        self.log_top = None;
                    }
                } else if a.commits.contains(pos) && self.change.active() {
                    self.change.editing = false;
                    self.select_change_hit(self.change.offset + (m.row - a.commits.y) as usize);
                } else if a.commits.contains(pos) {
                    self.focus = Focus::Commits;
                    let row = self.commit_offset + a.commit_at(m.row);
                    if row < self.history.commits.len() {
                        // A click on "+N" says which refs it hides (gitti: its tooltip).
                        let more = (!right)
                            .then(|| self.badge_more.iter().find(|(r, _)| r.contains(pos)))
                            .flatten()
                            .map(|(_, hidden)| hidden.clone());
                        // A right click on a ref's badge: that ref's menu (as in gitti).
                        let badge = right
                            .then(|| self.badge_refs.iter().find(|(r, _)| r.contains(pos)))
                            .flatten()
                            .map(|(_, r)| r.clone());
                        self.select_row(row);
                        if let Some(r) = badge {
                            self.ref_menu(&r);
                        } else if right {
                            self.context_menu();
                        } else if let Some(hidden) = more {
                            self.set_info(hidden);
                        }
                    }
                } else if a.sidebar.contains(pos) {
                    self.focus = Focus::Sidebar;
                    let row = self.side_offset + (m.row - a.sidebar.y) as usize;
                    if row < self.sidebar.len() {
                        let again = row == self.side_cursor;
                        self.side_cursor = row;
                        if right {
                            // Selects like a click (the ref's commit), then the menu.
                            self.side_jump();
                            self.context_menu();
                        } else if again || self.sidebar[row].collapse_key().is_some() {
                            self.activate_side_item();
                        } else {
                            self.side_jump();
                        }
                    }
                } else if a.detail.contains(pos) {
                    self.focus = Focus::Detail;
                    let row = self.diff_offset + (m.row - a.detail.y) as usize;
                    if row >= self.rows.len() {
                        return;
                    }
                    if right {
                        self.diff_cursor = row;
                        self.context_menu();
                    } else if shift {
                        self.extend_to(row);
                    } else {
                        self.diff_cursor = row;
                        match self.rows[row] {
                            DiffRow::HeaderSpace if Some(row) == self.signature_header_line() => {
                                self.show_signature()
                            }
                            DiffRow::File { .. } => self.toggle_file(),
                            DiffRow::Group(_) => self.toggle_group(),
                            DiffRow::Expand { src, file, gap } => self.show_more_lines(
                                src,
                                file,
                                Some(gap),
                                gitti_core::context::Reveal::More,
                            ),
                            DiffRow::Line { .. } if matches!(self.detail, Detail::Worktree) => {
                                self.toggle_line()
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Shift-click: selects from the selection anchor to `row`.
    fn extend_to(&mut self, row: usize) {
        let delta = row as isize - self.diff_cursor as isize;
        if delta != 0 {
            self.extend_selection(delta);
        }
    }
}

/// The command of a key that acts everywhere (`Scope::Global` of the shared table: ctrl-g).
/// The graph move of an arrow key with alt / ctrl (and shift), as the shared table names it.
fn graph_move(key: &KeyEvent) -> Option<keys::GraphMove> {
    let name = match key.code {
        KeyCode::Up => "up",
        KeyCode::Down => "down",
        _ => return None,
    };
    let m = key.modifiers;
    // ⌘ arrives only through the kitty keyboard protocol and when the terminal doesn't
    // keep the key for itself (Ghostty / iTerm2 / Terminal use ⌘↑ ⌘⇧↑): as gitti's ⌘.
    keys::graph_lookup(&keys::named_key(
        name,
        m.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER),
        m.contains(KeyModifiers::ALT),
        m.contains(KeyModifiers::SHIFT),
    ))
}

fn global_command(key: &KeyEvent) -> Option<Command> {
    let KeyCode::Char(ch) = key.code else {
        return None;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    keys::lookup_in(crate::kb::layout(), &keys::key_name(ch, ctrl, alt), Scope::Global)
}

fn text_scroll(scroll: &mut usize, max: usize, key: KeyEvent) -> Outcome {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter | KeyCode::Char('?') => Outcome::Close,
        KeyCode::Down | KeyCode::Char('j') => {
            *scroll = (*scroll + 1).min(max);
            Outcome::Keep
        }
        KeyCode::Up | KeyCode::Char('k') => {
            *scroll = scroll.saturating_sub(1);
            Outcome::Keep
        }
        KeyCode::PageDown | KeyCode::Char(' ') => {
            *scroll = (*scroll + 20).min(max);
            Outcome::Keep
        }
        KeyCode::PageUp => {
            *scroll = scroll.saturating_sub(20);
            Outcome::Keep
        }
        KeyCode::Home | KeyCode::Char('g') => {
            *scroll = 0;
            Outcome::Keep
        }
        // Drawing clamps to the last line the window can show.
        KeyCode::End | KeyCode::Char('G') => {
            *scroll = max;
            Outcome::Keep
        }
        _ => Outcome::Keep,
    }
}

fn save_edit(edit: &crate::edit::TextEdit, submit: &mut Option<SubmitText>) -> Outcome {
    let text = edit.text();
    match submit.take() {
        Some(s) => Outcome::Run(Box::new(move |app: &mut App| s(app, text))),
        None => Outcome::Close,
    }
}

impl App {
    /// Runs a command of the shared key table (`gitti_core::keys`).
    pub fn run_command(&mut self, cmd: Command) {
        let wt = matches!(self.detail, crate::app::Detail::Worktree);
        match cmd {
            Command::Fetch => self.fetch(),
            Command::Pull => self.pull(),
            Command::Push => self.push(),
            Command::SwitchBranch => self.switch_branch_picker(),
            Command::CommandPalette => self.palette(),
            Command::ShowShortcuts => self.modal = Some(Modal::Help { scroll: 0 }),
            Command::Refresh => {
                self.set_info("Refreshing…");
                self.refresh_everything();
            }
            Command::StashChanges => self.stash_dialog(),
            Command::RunGitCommand => self.git_command_dialog(),
            Command::FocusMessage => self.focus_message(),
            Command::ToggleAmend => self.toggle_amend(),
            Command::StageAll => self.stage_all(),
            Command::UnstageAll => self.unstage_all(),
            Command::DiscardAll => self.discard_all(),
            Command::ToggleWhitespace => self.toggle_whitespace(),
            Command::ToggleWrap => self.toggle_wrap(),
            Command::MoreContext => self.change_context(1),
            Command::LessContext => self.change_context(-1),
            Command::ToggleOrder => self.toggle_order(),
            Command::ToggleGitLog => self.toggle_log(),
            Command::ErrorDetails => {
                self.show_error_details();
            }
            Command::SearchChanges => self.open_change_search(),
            Command::SelectFirst if self.change.active() => self.select_change_hit(0),
            Command::SelectLast if self.change.active() => {
                let n = self.change.session.hits().len();
                self.select_change_hit(n.saturating_sub(1))
            }
            Command::SelectFirst => self.select_row(0),
            Command::SelectLast => self.select_row(self.history.commits.len().saturating_sub(1)),
            Command::StageSelection if wt => self.patch_action(PatchMode::Stage),
            Command::UnstageSelection if wt => self.patch_action(PatchMode::Unstage),
            Command::DiscardSelection if wt => self.patch_action(PatchMode::Discard),
            Command::StageSelection | Command::UnstageSelection | Command::DiscardSelection => {
                self.set_info(if self.shown_compare().is_some() {
                    "A comparison is read-only — staging works in the working directory (row 0)"
                } else {
                    "Staging works in the working directory (row 0)"
                })
            }
            Command::CopySelection => self.copy_selection(),
            Command::OpenInEditor => self.open_in_editor(),
            Command::NextHunk => self.jump_diff(true, |r| matches!(r, DiffRow::Hunk { .. })),
            Command::PrevHunk => self.jump_diff(false, |r| matches!(r, DiffRow::Hunk { .. })),
            Command::NextFile => self.jump_diff(true, |r| matches!(r, DiffRow::File { .. })),
            Command::PrevFile => self.jump_diff(false, |r| matches!(r, DiffRow::File { .. })),
            Command::FileList => self.file_picker(),
            Command::MarkCompare => self.toggle_compare_mark(),
            Command::ShowMoreLines => self.show_more_lines_key(false),
            Command::ShowWholeFile => self.show_more_lines_key(true),
            Command::ToggleSidebar => self.toggle_sidebar(),
            Command::GoToCommit => self.goto_commit_picker(),
            Command::Quit => self.request_quit(),
        }
        self.follow = true;
    }
}

impl App {
    /// Dragging a pane separator (or the git log's title bar) with the mouse; the sizes
    /// are saved as fractions when released (`PaneLayout`, shared with gitti).
    fn drag_divider(&mut self, m: &MouseEvent) -> bool {
        let a = self.areas;
        let in_body = m.row > 0 && m.row < a.status.y;
        let width = f32::from(a.body.width);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if in_body => {
                self.dragging = if a.sep_sidebar == Some(m.column) {
                    Some(Divider::Sidebar)
                } else if a.sep_list == Some(m.column) {
                    Some(Divider::List)
                } else if a.log.height > 0 && m.row == a.log.y {
                    Some(Divider::Log)
                } else {
                    None
                };
                self.dragging.is_some()
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.dragging {
                Some(Divider::Sidebar) => {
                    let w = m.column.saturating_sub(a.body.x).max(12);
                    self.layout.pane_sidebar = PaneLayout::fraction(f32::from(w), width);
                    true
                }
                Some(Divider::List) => {
                    let w = f32::from(m.column.saturating_sub(a.commits.x).max(20));
                    let mut fraction = PaneLayout::fraction(w, width);
                    if a.sep_sidebar.is_none() && (0. ..1.).contains(&self.layout.pane_sidebar) {
                        // Sidebar hidden: undo `list_fraction`'s scaling.
                        fraction *= 1. - self.layout.pane_sidebar;
                    }
                    self.layout.pane_list = fraction;
                    true
                }
                Some(Divider::Log) => {
                    // The log reaches from the dragged row down to the status bar.
                    let panes_h = a.status.y.saturating_sub(a.body.y);
                    let h = a.status.y.saturating_sub(m.row).clamp(3, panes_h.saturating_sub(3).max(3));
                    self.layout.pane_log = PaneLayout::fraction(f32::from(h), f32::from(panes_h));
                    true
                }
                None => false,
            },
            MouseEventKind::Up(MouseButton::Left) if self.dragging.is_some() => {
                self.dragging = None;
                let saved: PaneLayout = store::load_settings();
                store::save_changed_settings(&saved, &self.layout);
                true
            }
            _ => false,
        }
    }
}
