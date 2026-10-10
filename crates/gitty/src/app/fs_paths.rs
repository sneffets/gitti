//! Paths this work tree's file system can't hold, in gitty (as gitti's
//! `app/fs_paths.rs`; logic in `gitti_core::fs_paths`): such files carry a "⚠ collides
//! with xt_TCPMSS.h" mark on their file header and explanation rows under it
//! (`DiffRow::PathNote`; Enter there opens the whole explanation with "Hide this path").
//! A toolbar badge says "N PATHS CAN'T BE CHECKED OUT HERE — ctrl-p: File System"; the
//! palette's "Paths This File System Can't Hold…" explains it, hides them (skip-worktree)
//! and hides the badge; "Hide Paths This File System Can't Hold" and "Show Hidden Paths
//! Again" (the undo) are there too, greyed while there is nothing to do, and the file's
//! detail menu (`m`) has its explanation and "Hide this path". The explanation rows wrap
//! at the pane's width; file list entries say why too. Staging such a file asks
//! first, Discard says what it can't do, lines of such a file are not staged / discarded.

use gitti_core::fs_paths::{PathIssue, STAGE_THEM_TOO, discard_warning, partial_refusal, stage_question};
use gitti_core::patch::PatchMode;
use gitti_core::rows::Src;

use super::{App, MenuItem, Modal, PickItem, work_files};

/// Width of the explanation lines in the menus.
const NOTE_WIDTH: usize = 72;

impl App {
    /// The problem of a working-directory file's path on this file system.
    pub(crate) fn fs_issue_of(&self, src: Src, file: usize) -> Option<&PathIssue> {
        let path = work_files(&self.working, src).get(file)?.path();
        self.working.fs_paths.issue(path)
    }

    fn fs_issues_among<'a>(&'a self, paths: impl IntoIterator<Item = &'a str>) -> Vec<&'a PathIssue> {
        paths
            .into_iter()
            .filter_map(|p| self.working.fs_paths.issue(p))
            .filter(|i| i.problem.cannot_hold())
            .collect()
    }

    fn fs_badge_key(&self) -> Vec<String> {
        self.working.fs_paths.shown().map(|i| i.path.clone()).collect()
    }

    /// The toolbar badge (`None`: nothing in the way, or hidden until the paths change).
    pub(crate) fn fs_paths_badge(&self) -> Option<String> {
        let short = self.working.fs_paths.short_note()?;
        (self.fs_badge_dismissed.as_ref() != Some(&self.fs_badge_key())).then_some(short)
    }

    pub(crate) fn fs_hidable(&self) -> Vec<String> {
        self.working.fs_paths.hidable(&self.working)
    }

    fn hide_fs_paths(&mut self, paths: Vec<String>) {
        self.run_op("Hide paths", true, super::After::Nothing, move |r| r.hide_fs_paths(&paths));
    }

    fn show_hidden_fs_paths(&mut self) {
        self.run_op("Show hidden paths", true, super::After::Nothing, |r| r.show_hidden_fs_paths());
    }

    /// "Paths This File System Can't Hold…": the note, hiding them, the undo, hiding the
    /// badge.
    pub(crate) fn fs_paths_menu(&mut self) {
        let report = self.working.fs_paths.clone();
        let mut items = match report.note() {
            Some(note) => MenuItem::notes(&note, NOTE_WIDTH),
            None => MenuItem::notes("No paths in the way on this file system.", NOTE_WIDTH),
        };
        items.push(MenuItem::separator());
        let hidable = self.fs_hidable();
        if !hidable.is_empty() {
            let n = hidable.len();
            items.push(
                MenuItem::new(
                    format!(
                        "Hide the {} that show as changed (skip-worktree)",
                        gitti_core::discard::count(n, "path")
                    ),
                    move |app: &mut App| app.hide_fs_paths(hidable),
                )
                .key('h'),
            );
        }
        let hidden = report.hidden().count();
        if hidden > 0 {
            items.push(
                MenuItem::new(
                    format!("Show the {} hidden again", gitti_core::discard::count(hidden, "path")),
                    |app: &mut App| app.show_hidden_fs_paths(),
                )
                .key('u'),
            );
        }
        if report.note().is_some() {
            items.push(
                MenuItem::new("Hide this note until the paths change", |app: &mut App| {
                    app.fs_badge_dismissed = Some(app.fs_badge_key());
                })
                .key('x'),
            );
        }
        self.modal = Some(Modal::menu("File System", items));
    }

    /// Enter on an explanation row: the whole explanation, "Hide this path".
    pub(crate) fn path_note_menu(&mut self, src: Src, file: usize) {
        let Some(issue) = self.fs_issue_of(src, file).cloned() else {
            return;
        };
        let mut items = Vec::new();
        for line in issue.problem.lines(&issue.path, &self.working.fs_paths.rules) {
            items.extend(MenuItem::notes(&line, NOTE_WIDTH));
        }
        if src == Src::Unstaged && issue.problem.cannot_hold() {
            items.push(MenuItem::separator());
            let path = issue.path.clone();
            items.push(
                MenuItem::new("Hide this path (skip-worktree)", move |app: &mut App| {
                    app.hide_fs_paths(vec![path])
                })
                .key('h'),
            );
        }
        self.modal = Some(Modal::menu(issue.path, items));
    }

    /// Palette entries: the note, hide, show again — always listed, greyed with the reason
    /// while there is nothing to do (`unavailable.rs`), like gitti's Repository menu.
    pub(crate) fn fs_paths_palette_items(&self) -> Vec<PickItem> {
        let report = &self.working.fs_paths;
        let count = |n| gitti_core::discard::count(n, "path");
        let hidable = self.fs_hidable();
        let hidden = report.hidden().count();
        let note = report
            .short_note()
            .unwrap_or_else(|| format!("{} in the way, {hidden} hidden", count(0)));
        vec![
            PickItem::new("Paths This File System Can't Hold…", note, |app: &mut App| app.fs_paths_menu()),
            PickItem::new(
                "Hide Paths This File System Can't Hold",
                format!("{} showing as changed — skip-worktree", count(hidable.len())),
                |app: &mut App| {
                    let paths = app.fs_hidable();
                    app.hide_fs_paths(paths)
                },
            ),
            PickItem::new(
                "Show Hidden Paths Again",
                format!("{} this file system can't hold — no-skip-worktree", count(hidden)),
                |app: &mut App| app.show_hidden_fs_paths(),
            ),
        ]
    }

    /// The detail menu's entries on the header of a file this file system can't hold
    /// (`m`): the whole explanation, hiding it.
    pub(crate) fn fs_path_menu_items(&self, src: Src, file: usize) -> Vec<MenuItem> {
        if self.fs_issue_of(src, file).is_none() {
            return Vec::new();
        }
        let mut items = vec![MenuItem::new("Why it can't be checked out here…", move |app: &mut App| {
            app.path_note_menu(src, file)
        })];
        if let Some(issue) = self.fs_issue_of(src, file).filter(|i| src == Src::Unstaged && i.problem.cannot_hold()) {
            let path = issue.path.clone();
            items.push(MenuItem::new("Hide this path (skip-worktree)", move |app: &mut App| {
                app.hide_fs_paths(vec![path])
            }));
        }
        items
    }

    /// Whether diff row `row` is the note of a nested repository / an untracked folder
    /// entry ([`Self::no_preview_menu`] on Enter).
    pub(crate) fn no_preview_at(&self, row: usize) -> bool {
        let Some(&gitti_core::rows::DiffRow::Note { src, file }) = self.rows.get(row) else {
            return false;
        };
        work_files(&self.working, src)
            .get(file)
            .is_some_and(|wf| gitti_core::discard::no_preview_note(&self.working, wf).is_some())
    }

    /// Enter on the row of a nested repository / an untracked folder entry: what it is,
    /// what Discard does with it (`true`: there was one).
    pub(crate) fn no_preview_menu(&mut self, src: Src, file: usize) -> bool {
        let Some(wf) = work_files(&self.working, src).get(file) else {
            return false;
        };
        let Some((_, full)) = gitti_core::discard::no_preview_note(&self.working, wf) else {
            return false;
        };
        let title = wf.path().to_string();
        self.modal = Some(Modal::menu(title, MenuItem::notes(full, NOTE_WIDTH)));
        true
    }

    /// Lines of a file this file system can't hold are not staged / discarded: says why.
    pub(super) fn refuse_fs_path(&mut self, src: Src, file: usize, mode: PatchMode) -> bool {
        let Some(text) = self.fs_issue_of(src, file).and_then(|i| partial_refusal(i, mode)) else {
            return false;
        };
        self.set_error(text);
        true
    }

    /// Staging `paths`: asks first when some can't be held on this file system (one file:
    /// stage anyway; several: the others, or all), then `stage` with the paths to stage
    /// (`None`: all of them).
    pub(super) fn guard_fs_stage(
        &mut self,
        paths: Vec<String>,
        stage: impl FnOnce(&mut App, Option<Vec<String>>) + 'static,
    ) {
        let issues = self.fs_issues_among(paths.iter().map(String::as_str));
        if issues.is_empty() {
            return stage(self, None);
        }
        let (title, text) = stage_question(&issues);
        if issues.len() == 1 {
            self.modal = Some(Modal::confirm(title, text, "Stage anyway", move |app| stage(app, None)));
            return;
        }
        let skip: Vec<String> = issues.iter().map(|i| i.path.clone()).collect();
        let stage = std::rc::Rc::new(std::cell::RefCell::new(Some(stage)));
        let again = stage.clone();
        let mut items = MenuItem::notes(&text, NOTE_WIDTH);
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("Stage the others", move |app: &mut App| {
                let rest: Vec<String> = paths.into_iter().filter(|p| !skip.contains(p)).collect();
                if rest.is_empty() {
                    app.set_info("Nothing staged");
                } else if let Some(stage) = stage.borrow_mut().take() {
                    stage(app, Some(rest))
                }
            })
            .key('s'),
        );
        items.push(
            MenuItem::new(STAGE_THEM_TOO, move |app: &mut App| {
                if let Some(stage) = again.borrow_mut().take() {
                    stage(app, None)
                }
            })
            .key('a'),
        );
        self.modal = Some(Modal::menu(title, items));
    }

    /// What Discard of `paths` can't do on this file system, for its question.
    pub(super) fn fs_discard_warning<'a>(&'a self, paths: impl IntoIterator<Item = &'a str>) -> Option<String> {
        discard_warning(&self.fs_issues_among(paths))
    }
}
