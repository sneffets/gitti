//! Navigate menu ("Go to …"): Go to Commit (background search over the
//! loaded history, `gitti_core::commit_search`: hash / message / author / file or path —
//! each row says why it matched, a line above them what was searched), Go to Branch / Tag,
//! Go to HEAD, Go to File (file tabs of the shown diff) — and File ▸ Open Recent. All
//! pickers open at once; the only work off the UI thread is the commit search, cancelled
//! by the next query (its `git log` killed), with file hits shown while it runs.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gitti_core::RefKind;
use gitti_core::commit_search::{CommitHit, CommitQuery, MatchReason, SearchResult};
use gpui_kit::*;

use super::picker::PickerItem;
use super::{Detail, GittiApp, format_time, short};
use crate::{GoToCommit, GoToHead, GoToRef, SearchChanges};

/// Go to Commit shows at most this many matches.
pub(super) const GOTO_COMMIT_LIMIT: usize = 200;

fn entry(
    label: impl Into<String>,
    detail: impl Into<String>,
    run: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> PickerItem {
    PickerItem {
        label: label.into(),
        detail: detail.into(),
        keys: String::new(),
        color: None,
        run: Rc::new(run),
        action: None,
    }
}

impl GittiApp {
    /// Registers the Navigate menu actions on the app's root element.
    pub(super) fn navigate_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &GoToCommit, w, cx| this.open_goto_commit(w, cx)))
            .on_action(cx.listener(|this, _: &GoToRef, w, cx| this.open_goto_ref(w, cx)))
            .on_action(cx.listener(|this, _: &GoToHead, w, cx| this.goto_head(w, cx)))
            .on_action(cx.listener(|this, _: &SearchChanges, w, cx| {
                this.open_change_search(w, cx)
            }))
    }

    /// Palette entries of this module.
    pub(super) fn navigate_palette_items(&self) -> Vec<PickerItem> {
        vec![
            // Its key (⌘G / Ctrl+G) goes to the keys column via the action.
            entry("Go to Commit…", "hash, message, author or file", |t, w, cx| {
                t.open_goto_commit(w, cx)
            })
            .act(&GoToCommit),
            entry(
                "Search in Changes…",
                "commits whose changed lines have the words (experimental)",
                |t, w, cx| t.open_change_search(w, cx),
            )
            .act(&SearchChanges),
            entry("Go to Branch / Tag…", "", |t, w, cx| t.open_goto_ref(w, cx)),
            entry("Go to HEAD", "", |t, w, cx| t.goto_head(w, cx)),
            entry("Open Recent…", "repositories opened before", |t, w, cx| {
                t.open_recent_picker(w, cx)
            }),
        ]
    }

    /// Selects the commit `id` in the list (centred) and focuses the list; a status
    /// message when it is not loaded.
    pub(super) fn goto_commit_id(
        &mut self,
        id: gitti_core::ObjectId,
        what: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.history.row_of(&id) {
            Some(row) => {
                self.select_row(row, window, cx);
                self.commit_scroll
                    .scroll_to_item(row, ScrollStrategy::Center);
                self.list_focus.focus(window, cx);
            }
            None => self.set_error(format!(
                "{what} ({}) is not in the loaded history",
                short(&id)
            )),
        }
        cx.notify();
    }

    /// Selects commit `id` and, once its diff is shown, the file `path` in it (a file hit
    /// of Go to Commit).
    fn goto_commit_file(
        &mut self,
        id: gitti_core::ObjectId,
        path: Option<String>,
        what: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_file = path.map(|p| (id, p));
        self.goto_commit_id(id, what, window, cx);
        // Already shown (cached, or the selected one): no `show_commit` follows.
        if matches!(&self.detail, Detail::Commit(cv) if cv.details.id == id && cv.compare.is_none())
        {
            self.apply_pending_file(cx);
        }
    }

    /// The shown commit is the one a file hit asked for: show that file (by its path in
    /// the commit, or its old path). Any other commit drops the request.
    pub(super) fn apply_pending_file(&mut self, cx: &mut Context<Self>) {
        let Some((id, path)) = self.pending_file.take() else {
            return;
        };
        let Detail::Commit(cv) = &self.detail else {
            return;
        };
        if cv.details.id != id {
            return;
        }
        let i = cv
            .files
            .iter()
            .position(|f| f.path() == path)
            .or_else(|| {
                cv.files
                    .iter()
                    .position(|f| f.old_path.as_deref() == Some(path.as_str()))
            });
        if let Some(i) = i {
            self.show_commit_file(i, cx);
        }
    }

    // ------------------------------------------------------------------ Go to Commit

    pub(super) fn open_goto_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        let search: super::picker::SearchFn = Rc::new(|this, query, window, cx| {
            this.search_commits(query, window, cx)
        });
        self.open_search_picker(
            "Go to commit: hash, message, author or file…",
            "Type a hash prefix, words of the message, an author — or a file name / path \
             (foo.rs, src/app/) for the commits that changed it",
            Vec::new(),
            search,
            window,
            cx,
        );
    }

    /// Searches the loaded history in the background (`History::search_smart`); file hits
    /// arrive while `git log` runs, then the final results replace them.
    fn search_commits(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let q = CommitQuery::parse(&query);
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if q.is_empty() {
            self.set_picker_results(Vec::new(), cx);
            return;
        }
        let git = repo.git().clone();
        let history = self.history.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (tx, rx) = async_channel::unbounded::<SearchResult>();
        let job = cx.background_executor().spawn(async move {
            let done = history.search_smart(&git, &q, GOTO_COMMIT_LIMIT, &flag, &mut |early| {
                let _ = tx.send_blocking(early.clone());
            });
            // `None`: cancelled by a newer query (nothing more to show).
            if let Some(r) = done {
                let _ = tx.send_blocking(r);
            }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(r) = rx.recv().await {
                let done = r.done;
                let ok = this.update(cx, |this, cx| this.show_commit_search(r, cx));
                if done || ok.is_err() {
                    break;
                }
            }
            job.await;
        });
        self.picker_search_started(cancel, task);
    }

    /// Search results (early or final) into the picker.
    fn show_commit_search(&mut self, r: SearchResult, cx: &mut Context<Self>) {
        let note = r.note();
        if !r.done && r.hits.is_empty() {
            self.set_picker_note(note, cx);
            return;
        }
        let tags = r.hits.iter().map(reason_tag).collect();
        let items = r.hits.into_iter().map(commit_item).collect();
        self.set_search_results(items, tags, note, r.done, cx);
    }

    // ------------------------------------------------------------------ Go to Branch / Tag

    pub(super) fn open_goto_ref(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let head = self.head.branch.clone();
        let mut items: Vec<PickerItem> = self
            .refs
            .iter()
            .filter(|r| {
                matches!(
                    r.kind,
                    RefKind::LocalBranch | RefKind::RemoteBranch | RefKind::Tag
                )
            })
            .map(|r| {
                let kind = match r.kind {
                    RefKind::LocalBranch if head.as_deref() == Some(&r.short_name) => {
                        "current branch"
                    }
                    RefKind::LocalBranch => "branch",
                    RefKind::RemoteBranch => "remote branch",
                    _ => "tag",
                };
                let (id, name) = (r.target, r.short_name.clone());
                entry(
                    r.short_name.clone(),
                    format!("{kind} · {}", short(&r.target)),
                    move |t, w, cx| t.goto_commit_id(id, &name, w, cx),
                )
            })
            .collect();
        // Local branches first, then remote branches, then tags (stable within a kind).
        let rank = |d: &str| {
            if d.starts_with("current") || d.starts_with("branch") {
                0
            } else if d.starts_with("remote") {
                1
            } else {
                2
            }
        };
        items.sort_by_key(|it| rank(&it.detail));
        self.open_picker("Go to branch or tag…", items, window, cx);
    }

    // ------------------------------------------------------------------ Go to HEAD

    pub(super) fn goto_head(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.head.id {
            Some(id) => {
                let what = self.head.branch.clone().unwrap_or_else(|| "HEAD".into());
                self.goto_commit_id(id, &what, window, cx)
            }
            None => {
                self.set_info("HEAD has no commit yet");
                cx.notify();
            }
        }
    }

    // ------------------------------------------------------------------ Go to File

    /// Commit view: file tab `i` (as clicking it does; the file is shown expanded).
    pub(super) fn show_commit_file(&mut self, i: usize, cx: &mut Context<Self>) {
        let Detail::Commit(cv) = &self.detail else {
            return;
        };
        let Some(f) = cv.files.get(i) else {
            return;
        };
        let path = f.path().to_string();
        if !self.expansion.is_expanded(crate::rows::Src::Commit, f) {
            self.expansion.toggle(crate::rows::Src::Commit, &path);
        }
        // The file opens as changes (no unchanged lines revealed before).
        self.expansion.hide_all_context();
        self.file_tab = Some(i);
        self.rebuild_rows();
        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    // ------------------------------------------------------------------ Open Recent

    /// The recently opened repositories (newest first); choosing one opens it in a tab (or
    /// switches to its tab). Gone ones are dropped from the list when chosen.
    pub fn open_recent_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.repo.as_ref().map(|r| r.workdir().to_path_buf());
        let items: Vec<PickerItem> = crate::store::load_recent_repos()
            .into_iter()
            .filter(|p| Some(p) != current.as_ref())
            .map(|path| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let detail = path.display().to_string();
                entry(name, detail, move |t, _, cx| {
                    if path.is_dir() {
                        cx.emit(super::repo_actions::AppEvent::OpenRepository(path.clone()));
                    } else {
                        crate::store::forget_recent_repo(&path);
                        t.set_error(format!("{} is gone (removed from the list)", path.display()));
                        cx.notify();
                    }
                })
            })
            .collect();
        if items.is_empty() {
            self.set_info("No recent repositories yet");
            cx.notify();
            return;
        }
        self.open_picker("Open recent repository…", items, window, cx);
    }
}

/// A Go to Commit result: subject, then short hash · author · date; a file hit selects
/// that file in the commit's diff.
fn commit_item(h: CommitHit) -> PickerItem {
    let id = h.id;
    let label = if h.summary.is_empty() {
        short(&h.id)
    } else {
        h.summary.clone()
    };
    let what = short(&h.id);
    let path = h.path().map(str::to_string);
    entry(
        label,
        format!("{} · {} · {}", short(&h.id), h.author, format_time(h.time)),
        move |t, w, cx| t.goto_commit_file(id, path.clone(), &what, w, cx),
    )
}

/// Longest reason shown in a result row (the end is kept: the file name matters).
const TAG_MAX_CHARS: usize = 36;

/// Why a Go to Commit result matched, for its row — `None` for a plain subject match.
fn reason_tag(h: &CommitHit) -> Option<String> {
    if h.reasons.iter().all(|r| *r == MatchReason::Subject) {
        return None;
    }
    let tag = h.reason_label();
    let n = tag.chars().count();
    Some(if n > TAG_MAX_CHARS {
        let tail: String = tag.chars().skip(n - (TAG_MAX_CHARS - 1)).collect();
        format!("…{tail}")
    } else {
        tag
    })
}
