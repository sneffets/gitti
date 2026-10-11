//! Fuzzy picker overlay: command palette (cmd-p / cmd-shift-p) and branch switcher (cmd-b).
//! It drops down under the toolbar's command bar; an entry may open a second picker
//! in its place (Push… → a remote, Switch Branch… → a branch).

use std::ops::Range;
use std::rc::Rc;

use gitti_core::RefKind;
use gitti_core::keys::Nav;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use crate::probe::ProbeExt as _;
use crate::theme::PaletteExt as _;

type RunFn = Rc<dyn Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>)>;

/// Width of the picker's panel.
pub(super) const PICKER_W: Pixels = px(620.);

/// Runs a picker's search for the typed query (instead of filtering fixed items), e.g. in
/// the background ("Go to Commit…"); it hands the results to `set_picker_results`.
pub(super) type SearchFn = Rc<dyn Fn(&mut GittiApp, String, &mut Window, &mut Context<GittiApp>)>;

#[derive(Clone)]
pub(super) struct PickerItem {
    pub label: String,
    /// What the entry acts on / does ("a remote", "git clone"), muted after the label.
    pub detail: String,
    /// Its keyboard shortcut: the right-aligned column of the list, never in the
    /// label. Empty = none; palette entries with an `action` get theirs from the live
    /// bindings (`open_command_palette`).
    pub keys: String,
    /// Colour swatch (graph lane) in front of the label.
    pub color: Option<Hsla>,
    pub run: RunFn,
    /// The menu-bar action this entry stands for (`gitti::Fetch`), for the test that the
    /// palette has every menu command (entries with the menu's label need none).
    pub action: Option<&'static str>,
}

impl PickerItem {
    /// Marks the entry as the palette's version of menu action `a`.
    pub(super) fn act(mut self, a: &dyn Action) -> Self {
        self.action = Some(a.name());
        self
    }
}

/// A palette entry's shortcut text (`shortcuts::hint`), for [`item`]'s right side: it goes
/// into the keys column instead of the description.
pub(super) struct Keys(pub String);

/// The right side of an [`item`]: a description or its keys.
pub(super) enum Right {
    Detail(String),
    Keys(String),
}

impl From<&str> for Right {
    fn from(s: &str) -> Self {
        Right::Detail(s.to_string())
    }
}

impl From<String> for Right {
    fn from(s: String) -> Self {
        Right::Detail(s)
    }
}

impl From<Keys> for Right {
    fn from(k: Keys) -> Self {
        Right::Keys(k.0)
    }
}

pub struct Picker {
    input: Entity<InputState>,
    items: Vec<PickerItem>,
    /// Indices into `items`, best match first.
    filtered: Vec<usize>,
    pub(super) selected: usize,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
    /// Search pickers: the query goes here instead of filtering `items`.
    search: Option<SearchFn>,
    /// A search runs (shown while there are no results yet).
    pub(super) searching: bool,
    /// Shown instead of "No matches" while the query is empty.
    hint: Option<String>,
    /// Search pickers: a line above the results (what was searched, why nothing matched).
    note: Option<String>,
    /// Search pickers: why each item matched (parallel to `items`; empty = none).
    tags: Vec<Option<String>>,
    /// The palette: why an entry's command can't act now (parallel to `items`; empty = all
    /// can). Such an entry is greyed and says the reason instead of running.
    disabled: Vec<Option<&'static str>>,
    /// Set when the running search is outdated (new query, picker closed).
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _search_task: Option<Task<()>>,
    /// The file tab list: drops down from the file tab bar's ▾ (`file_tabs.rs`) instead of
    /// from the command bar.
    pub(super) at_file_tabs: bool,
    /// The command palette: its filter knows synonyms (`palette::synonym_score`),
    /// "new" finds the Create… and Add… commands too.
    synonyms: bool,
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub(super) fn item(
    label: impl Into<String>,
    right: impl Into<Right>,
    run: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> PickerItem {
    let (detail, keys) = match right.into() {
        Right::Detail(d) => (crate::theme::keys_hint(&d), String::new()),
        Right::Keys(k) => (String::new(), k),
    };
    PickerItem {
        label: label.into(),
        detail,
        keys,
        color: None,
        run: Rc::new(run),
        action: None,
    }
}

/// What a palette entry found by a synonym (`palette::synonym_score`) scores less than
/// by its own words: about one word start's worth, so "new" lists "New …" before
/// "Create …" / "Add …".
const SYNONYM_PENALTY: i32 = 100;

/// Subsequence match score (higher is better); `None` if `query` does not match.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text_l: Vec<char> = text.to_lowercase().chars().collect();
    let orig: Vec<char> = text.chars().collect();
    let mut score = 0;
    let mut pos = 0usize;
    let mut prev: Option<usize> = None;
    for qc in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = (pos..text_l.len()).find(|&i| text_l[i] == qc)?;
        score += 1;
        if prev.is_some_and(|p| p + 1 == found) {
            score += 5; // contiguous
        }
        let word_start = found == 0 || !orig[found - 1].is_alphanumeric();
        if word_start {
            score += 8;
        }
        if found == 0 {
            score += 4;
        }
        prev = Some(found);
        pos = found + 1;
    }
    Some(score * 10 - text_l.len() as i32)
}

impl Picker {
    /// Greys entries with their reason (parallel to the items; see `disabled`).
    pub(super) fn set_disabled(&mut self, disabled: Vec<Option<&'static str>>) {
        self.disabled = disabled;
    }

    /// For the key tests (`GittiApp::script_state`).
    pub(super) fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Its query field (where the focus belongs, `overlay_keys.rs`).
    pub(super) fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    pub(super) fn script_state(&self, cx: &App) -> crate::testing::PickerState {
        crate::testing::PickerState {
            labels: self.filtered.iter().map(|&i| self.items[i].label.clone()).collect(),
            actions: self.filtered.iter().map(|&i| self.items[i].action).collect(),
            details: self.filtered.iter().map(|&i| self.items[i].detail.clone()).collect(),
            keys: self.filtered.iter().map(|&i| self.items[i].keys.clone()).collect(),
            tags: self
                .filtered
                .iter()
                .map(|&i| self.tags.get(i).cloned().flatten())
                .collect(),
            disabled: self
                .filtered
                .iter()
                .map(|&i| self.disabled.get(i).copied().flatten().map(str::to_string))
                .collect(),
            note: self.note.clone(),
            searching: self.searching,
            selected: self.selected,
            query: self.input.read(cx).value().to_string(),
        }
    }
}

impl GittiApp {
    pub(super) fn open_picker(
        &mut self,
        placeholder: &str,
        items: Vec<PickerItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_string()));
        let subs =
            vec![cx.subscribe_in(
                &input,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => {
                        this.picker_query_changed(window, cx);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.run_picker_selection(window, cx),
                    _ => {}
                },
            )];
        input.update(cx, |s, cx| s.focus(window, cx));
        let filtered = (0..items.len()).collect();
        self.picker = Some(Picker {
            input,
            items,
            filtered,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subs,
            search: None,
            searching: false,
            hint: None,
            note: None,
            tags: Vec::new(),
            disabled: Vec::new(),
            cancel: Default::default(),
            _search_task: None,
            at_file_tabs: false,
            synonyms: false,
        });
        cx.notify();
    }

    /// A picker whose entries come from `search` for each query (`items` until the first
    /// one); `hint` is shown while nothing is typed.
    pub(super) fn open_search_picker(
        &mut self,
        placeholder: &str,
        hint: &str,
        items: Vec<PickerItem>,
        search: SearchFn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_picker(placeholder, items, window, cx);
        if let Some(p) = self.picker.as_mut() {
            p.search = Some(search);
            p.hint = Some(hint.to_string());
        }
    }

    /// The query changed: filter the items, or start the picker's search.
    pub(super) fn picker_query_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let Some(search) = p.search.clone() else {
            self.refilter_picker(cx);
            self.reveal_nav_focus();
            return;
        };
        // The running search is outdated.
        p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        p._search_task = None;
        p.searching = false;
        let query = p.input.read(cx).value().to_string();
        search(self, query, window, cx);
    }

    /// A search started: it is cancelled by the next query or when the picker closes.
    pub(super) fn picker_search_started(
        &mut self,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        task: Task<()>,
    ) {
        if let Some(p) = self.picker.as_mut() {
            p.cancel = cancel;
            p._search_task = Some(task);
            p.searching = true;
        }
    }

    /// The results of a search picker's query (best first).
    pub(super) fn set_picker_results(&mut self, items: Vec<PickerItem>, cx: &mut Context<Self>) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        p.filtered = (0..items.len()).collect();
        p.items = items;
        p.tags.clear();
        p.disabled.clear();
        p.note = None;
        p.selected = 0;
        p.searching = false;
        p._search_task = None;
        p.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Results of a search picker that may arrive in steps (`done == false`: more
    /// follow): items with why they matched, and the line above them. The selected entry
    /// stays selected when it is still there.
    pub(super) fn set_search_results(
        &mut self,
        items: Vec<PickerItem>,
        tags: Vec<Option<String>>,
        note: Option<String>,
        done: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let was = p
            .filtered
            .get(p.selected)
            .and_then(|&i| p.items.get(i))
            .map(|it| (it.label.clone(), it.detail.clone()));
        let selected = was
            .and_then(|(l, d)| items.iter().position(|it| it.label == l && it.detail == d))
            .unwrap_or(0);
        p.filtered = (0..items.len()).collect();
        p.items = items;
        p.tags = tags;
        p.disabled.clear();
        p.note = note;
        p.searching = !done;
        if done {
            p._search_task = None;
        }
        if selected != p.selected || selected == 0 {
            p.selected = selected;
            p.scroll.scroll_to_item(selected, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    /// A search picker's line above the results only (no new results yet).
    pub(super) fn set_picker_note(&mut self, note: Option<String>, cx: &mut Context<Self>) {
        if let Some(p) = self.picker.as_mut() {
            p.note = note;
            cx.notify();
        }
    }

    /// The entries of a filtering picker opened empty while they load in the background
    /// (its task registered with `picker_search_started`): filtered by what was typed
    /// meanwhile; `note` = the line above them.
    pub(super) fn fill_picker(
        &mut self,
        items: Vec<PickerItem>,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        p.items = items;
        p.disabled.clear();
        p.note = note;
        p.searching = false;
        p._search_task = None;
        self.refilter_picker(cx);
        self.reveal_nav_focus();
        cx.notify();
    }

    /// Script hook: types `query` into the open picker (filters / searches as typing does).
    pub fn script_picker_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.picker.as_ref().map(|p| p.input.clone()) else {
            return;
        };
        input.update(cx, |s, cx| s.set_value(query.to_string(), window, cx));
        self.picker_query_changed(window, cx);
        cx.notify();
    }

    /// Script hook: runs the selected entry of the open picker (as Enter does).
    pub fn script_picker_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_picker_selection(window, cx);
    }

    pub(super) fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self.nav_candidates.clear();
        self.list_focus.focus(window, cx);
        cx.notify();
    }

    fn refilter_picker(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let query = p.input.read(cx).value().to_string();
        let mut scored: Vec<(i32, usize)> = p
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let hay = format!("{} {}", it.label, it.detail);
                let label = if p.synonyms {
                    let penalty = SYNONYM_PENALTY;
                    gitti_core::palette::synonym_score(&query, &it.label, penalty, fuzzy_score)
                } else {
                    fuzzy_score(&query, &it.label)
                };
                label
                    .map(|s| s + 1000)
                    .or_else(|| fuzzy_score(&query, &hay))
                    .map(|s| (s, i))
            })
            .collect();
        if !query.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        p.filtered = scored.into_iter().map(|(_, i)| i).collect();
        p.selected = 0;
        p.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// The list / scroll keys in a picker: ↑ / ↓ wrap at the ends, pages and Home / End
    /// stop there; the list window moves only when the selection leaves it (the wheel
    /// scrolls it without moving the selection).
    pub(super) fn picker_nav(&mut self, nav: Nav, cx: &mut Context<Self>) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let n = p.filtered.len();
        if n == 0 {
            return;
        }
        let page = Self::picker_rows(n).saturating_sub(1).max(1);
        let wrap = matches!(nav, Nav::LineUp | Nav::LineDown);
        p.selected = gitti_core::nav::move_cursor(p.selected, n, nav, page, wrap);
        p.scroll.scroll_to_item(p.selected, ScrollStrategy::Nearest);
        self.reveal_nav_focus();
        cx.notify();
    }

    /// Entries a picker shows at once.
    fn picker_rows(n: usize) -> usize {
        n.clamp(1, 12)
    }

    fn run_picker_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.picker.as_ref() else {
            return;
        };
        let Some(&i) = p.filtered.get(p.selected) else {
            return;
        };
        let run = p.items[i].run.clone();
        self.close_picker(window, cx);
        run(self, window, cx);
    }

    /// Script hook: open the palette, type `query`, run the best match.
    pub fn script_palette(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_command_palette(window, cx);
        if let Some(p) = &self.picker {
            let input = p.input.clone();
            input.update(cx, |s, cx| s.set_value(query.to_string(), window, cx));
        }
        self.refilter_picker(cx);
        self.reveal_nav_focus();
        self.run_picker_selection(window, cx);
    }

    // ------------------------------------------------------------------ contents

    pub(super) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = &self.settings;
        let on_off = |b: bool| if b { "on" } else { "off" };
        // Key hints from the live bindings (`shortcuts.rs`), so they can't drift; they go
        // into the keys column.
        let k = |a: &dyn Action| Keys(crate::shortcuts::hint(a, cx));
        let mut items = vec![
            item("Switch Branch…", k(&crate::SwitchBranch), |t, w, cx| {
                t.open_branch_switcher(w, cx)
            }),
            item("Merge Branch into Current…", k(&crate::MergeBranch), |t, w, cx| {
                t.open_merge_picker(w, cx)
            }),
            item("Fetch", k(&crate::Fetch), |t, w, cx| {
                t.git_op("Fetch", w, cx, |r| r.fetch())
            })
            .act(&crate::Fetch),
            item("Fetch from…", "a remote", |t, w, cx| t.open_fetch_step(w, cx))
                .act(&crate::FetchFrom),
            item("Pull", k(&crate::Pull), |t, w, cx| {
                t.git_op("Pull", w, cx, |r| r.pull_saved())
            })
            .act(&crate::Pull),
            item("Pull from…", "a remote", |t, w, cx| t.open_pull_step(w, cx))
                .act(&crate::PullFrom),
            item("Pull with…", "merge, rebase or fast-forward only", |t, w, cx| {
                t.open_pull_with_step(w, cx)
            })
            .act(&crate::PullWith),
            item("Push", k(&crate::Push), |t, w, cx| t.push_main(w, cx)).act(&crate::Push),
            item("Push…", "a remote, all remotes, options", |t, w, cx| {
                t.open_push_step(w, cx)
            })
            .act(&crate::PushTo),
            item(
                "Reset Remote Branch to Selected Commit…",
                "a push with lease; the local branch stays",
                |t, w, cx| t.reset_remote_selected(w, cx),
            )
            .act(&crate::ResetRemoteBranch),
            item("Commit", k(&crate::Commit), |t, w, cx| t.commit(w, cx)),
            item("Discard All Unstaged Changes…", k(&crate::DiscardAll), |t, w, cx| {
                t.discard_all(w, cx)
            })
            .act(&crate::DiscardAll),
            item("Show Error Details", k(&crate::ErrorDetails), |t, w, cx| {
                if !t.show_error_details(w, cx) {
                    t.set_info("No error to show");
                }
            })
            .act(&crate::ErrorDetails),
            item("Stash Changes…", k(&crate::StashChanges), |t, w, cx| {
                t.stash_dialog(w, cx)
            }),
            item("Stage All", k(&crate::StageAll), |t, w, cx| {
                t.stage_everything(true, w, cx)
            }),
            item("Unstage All", k(&crate::UnstageAll), |t, w, cx| {
                t.stage_everything(false, w, cx)
            }),
            item("Create Branch at HEAD…", k(&crate::CreateBranch), |t, w, cx| {
                t.create_branch_dialog("HEAD".into(), "HEAD".into(), w, cx)
            })
            .act(&crate::CreateBranch),
            item("Create Tag at HEAD…", k(&crate::CreateTag), |t, w, cx| {
                t.create_tag_dialog("HEAD".into(), "HEAD".into(), w, cx)
            })
            .act(&crate::CreateTag),
            item("Add Remote…", k(&crate::AddRemote), |t, w, cx| t.add_remote_dialog(w, cx)),
            item("Go to Working Directory", k(&crate::FocusMessage), |t, w, cx| {
                t.select_row(0, w, cx);
                t.message.update(cx, |m, cx| m.focus(w, cx));
            }),
            item("Settings…", k(&crate::OpenSettings), |t, w, cx| t.open_settings(w, cx)),
            item(
                "Write Commit-Graph (faster history loading)",
                k(&crate::WriteCommitGraph),
                |t, w, cx| t.write_commit_graph(w, cx),
            ),
            item("Open Repository…", k(&crate::OpenRepository), |_, w, cx| {
                w.dispatch_action(Box::new(crate::OpenRepository), cx)
            }),
            item("Close Tab", k(&crate::CloseTab), |_, w, cx| {
                w.dispatch_action(Box::new(crate::CloseTab), cx)
            }),
            item("Refresh", k(&crate::Refresh), |t, w, cx| t.refresh_everything(w, cx)),
            item("Run Git Command…", k(&crate::RunGitCommand), |t, w, cx| {
                t.focus_command_input(w, cx)
            }),
            // The file list (the file tabs' ▾), keys from the action.
            item("Go to File…", "a file of the shown diff", |t, w, cx| {
                t.open_file_list(w, cx)
            })
            .act(&crate::FileList),
            item(
                "Git Settings: This Repository…",
                "user.name, email, pull, push…",
                |t, w, cx| t.open_config_dialog(gitti_core::ops::ConfigScope::Local, w, cx),
            ),
            item("Git Settings: Global…", "~/.gitconfig", |t, w, cx| {
                t.open_config_dialog(gitti_core::ops::ConfigScope::Global, w, cx)
            }),
            item(
                "Edit Repository Config File…",
                ".git/config",
                |t, w, cx| t.edit_config_file(gitti_core::ops::ConfigScope::Local, w, cx),
            ),
            item("Edit Global Config File…", "~/.gitconfig", |t, w, cx| {
                t.edit_config_file(gitti_core::ops::ConfigScope::Global, w, cx)
            }),
            item(
                format!("Toggle Git Command Log ({})", on_off(s.show_git_log)),
                k(&crate::ToggleGitLog),
                |t, _, cx| t.toggle_git_log(cx),
            )
            .act(&crate::ToggleGitLog),
            item(
                format!("Toggle Line Wrap ({})", on_off(s.wrap_lines)),
                k(&crate::ToggleWrap),
                |t, _, cx| t.toggle_wrap(cx),
            )
            .act(&crate::ToggleWrap),
            item(
                format!(
                    "Toggle Syntax Highlighting ({})",
                    on_off(s.syntax_highlight)
                ),
                "",
                |t, _, cx| t.toggle_syntax(cx),
            ),
            item(
                format!("Toggle Spell Checking ({})", on_off(s.spell_check)),
                "",
                |t, _, cx| t.toggle_spell_check(cx),
            ),
            item(
                format!("Toggle Ignore Whitespace ({})", on_off(s.ignore_whitespace)),
                k(&crate::ToggleWhitespace),
                |t, w, cx| t.toggle_whitespace(w, cx),
            )
            .act(&crate::ToggleWhitespace),
            item(
                format!("Highlight Changed Characters Stronger ({} %)", s.intraline_strength),
                "",
                |t, _, cx| t.step_intraline_strength(1, cx),
            ),
            item(
                format!("Highlight Changed Characters Weaker ({} %)", s.intraline_strength),
                "",
                |t, _, cx| t.step_intraline_strength(-1, cx),
            ),
            item(
                format!(
                    "Toggle Commit Order (now {})",
                    if s.topo_order { "topological" } else { "date" }
                ),
                k(&crate::ToggleOrder),
                |t, w, cx| {
                    t.update_settings(cx, |s| s.topo_order = !s.topo_order);
                    t.refresh(true, w, cx);
                },
            )
            .act(&crate::ToggleOrder),
            item("Bigger Font", k(&crate::ZoomIn), |t, _, cx| {
                t.update_settings(cx, |s| s.zoom(1.0))
            }),
            item("Smaller Font", k(&crate::ZoomOut), |t, _, cx| {
                t.update_settings(cx, |s| s.zoom(-1.0))
            }),
            item("Bigger Diff Font", k(&crate::ZoomDiffIn), |t, _, cx| {
                t.update_settings(cx, |s| s.zoom_diff(1.0))
            }),
            item("Smaller Diff Font", k(&crate::ZoomDiffOut), |t, _, cx| {
                t.update_settings(cx, |s| s.zoom_diff(-1.0))
            }),
            item("Reset Font Size", k(&crate::ResetZoom), |t, _, cx| {
                t.update_settings(cx, |s| s.reset_zoom())
            }),
        ];
        // Navigation (the Navigate menu in the palette, too).
        let nav: [(&str, Box<dyn Action>); 10] = [
            ("Next Commit", Box::new(crate::SelectNext)),
            ("Previous Commit", Box::new(crate::SelectPrev)),
            ("Follow Branch Down (Parent)", Box::new(crate::FollowBranchDown)),
            ("Follow Branch Up (Child)", Box::new(crate::FollowBranchUp)),
            ("Other Way Down (Merged Parent)", Box::new(crate::OtherBranchDown)),
            ("Other Way Up (Other Child)", Box::new(crate::OtherBranchUp)),
            ("Next File", Box::new(crate::NextFile)),
            ("Previous File", Box::new(crate::PrevFile)),
            ("Next Hunk", Box::new(crate::NextHunk)),
            ("Previous Hunk", Box::new(crate::PrevHunk)),
        ];
        for (label, action) in nav {
            let name = action.name();
            let mut it = item(label, k(action.as_ref()), move |_, w, cx| {
                w.dispatch_action(action.boxed_clone(), cx)
            });
            it.action = Some(name);
            items.push(it);
        }
        items.extend(self.repo_palette_items(cx));
        items.extend(self.navigate_palette_items());
        items.extend(self.menu_cmd_palette_items());
        items.extend(self.restore_contains_palette_items());
        items.extend(self.stash_palette_items());
        items.extend(self.prepared_palette_items(cx));
        items.extend(self.commit_box_palette_items());
        items.extend(self.list_layout_palette_items());
        items.extend(self.branch_palette_items());
        items.extend(self.worktree_palette_items());
        items.extend(self.submodule_palette_items());
        items.extend(self.subtree_palette_items());
        items.extend(self.signers_palette_items());
        items.extend(self.lfs_palette_items());
        items.extend(self.lfs_download_palette_items());
        items.extend(self.lfs_health_palette_items());
        items.extend(self.fs_paths_palette_items());
        items.extend(self.reflog_palette_items());
        if self.repo_state != gitti_core::ops::RepoState::Clean {
            items.insert(
                0,
                item(
                    format!("Abort {}", self.repo_state.label()),
                    "",
                    |t, w, cx| t.git_op("Abort", w, cx, |r| r.abort_operation()),
                ),
            );
            if gitti_core::skip::can_skip(self.repo_state) {
                items.insert(
                    0,
                    item(gitti_core::skip::SKIP_LABEL, "leave the stopped commit out", |t, w, cx| {
                        t.skip_operation(w, cx)
                    }),
                );
            }
            items.insert(
                0,
                item(
                    format!("Continue {}", self.repo_state.label()),
                    match self.continue_message(cx) {
                        Some(_) => gitti_core::ops::CONTINUE_WITH_BOX,
                        None => "",
                    },
                    |t, w, cx| t.continue_operation(w, cx),
                ),
            );
        } else if !self.working.conflicted.is_empty() {
            // Conflicts without an operation (stash pop / apply), as gitty's palette.
            items.insert(
                0,
                item("Abort (undo the conflicted changes)", "", |t, w, cx| {
                    t.abort_conflicts(w, cx)
                }),
            );
        }
        // Compare mode (⇧C in the panes).
        if self.compare.mode.is_some() {
            items.push(item("End Comparison", k(&crate::EndCompare), |t, _, cx| {
                t.end_compare(cx);
            }));
            items.push(item("Swap Comparison Sides", k(&crate::SwapCompare), |t, _, cx| t.swap_compare(cx)));
        }
        if !self.is_compare_base(self.selected) {
            items.push(
                item("Mark Selected Commit for Comparison", k(&crate::MarkCompare), |t, _, cx| {
                    t.toggle_compare_mark(cx)
                })
                .act(&crate::MarkCompare),
            );
        }
        for st in &self.stashes {
            let stash = st.target();
            items.push(item(
                format!("Apply Stash: {}", st.message),
                st.name.clone(),
                move |t, w, cx| {
                    t.git_op("Stash apply", w, cx, move |r| r.stash_apply(stash, false))
                },
            ));
            items.push(item(
                format!("Pop Stash: {}", st.message),
                st.name.clone(),
                move |t, w, cx| t.git_op("Stash pop", w, cx, move |r| r.stash_apply(stash, true)),
            ));
        }
        // Every entry standing for a menu action shows that action's keys (the column, from
        // the live bindings), also where the entry was made without them.
        for it in &mut items {
            if it.keys.is_empty()
                && let Some(action) = it.action.and_then(|a| cx.build_action(a, None).ok())
            {
                it.keys = crate::shortcuts::hint(action.as_ref(), cx);
            }
        }
        // Custom commands last: their reasons go after the menu commands'.
        let custom_from = items.len();
        let custom_off = self.custom_palette_items(&mut items);
        // What can't act now stays findable, greyed with the reason.
        let mut disabled = self.mark_unavailable(&mut items);
        for (i, why) in custom_off.into_iter().enumerate() {
            if let (Some(why), Some(d)) = (why, disabled.get_mut(custom_from + 2 + i)) {
                *d = Some(why);
            }
        }
        self.open_picker("Type a command…", items, window, cx);
        if let Some(p) = self.picker.as_mut() {
            p.synonyms = true;
            p.disabled = disabled;
        }
    }

    /// Picks the upstream (remote branch) for local `branch`.
    pub(super) fn open_upstream_picker(
        &mut self,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut items: Vec<PickerItem> = self
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::RemoteBranch)
            .map(|r| {
                let (b, up) = (branch.clone(), r.short_name.clone());
                item(up.clone(), "remote branch", move |t, w, cx| {
                    let (b, up) = (b.clone(), up.clone());
                    t.git_op("Set upstream", w, cx, move |r| r.set_upstream(&b, &up))
                })
            })
            .collect();
        // Most likely choice first: <remote>/<same name>.
        items.sort_by_key(|it| !it.label.ends_with(&format!("/{branch}")));
        self.open_picker(&format!("Upstream for {branch}…"), items, window, cx);
    }

    pub(super) fn open_branch_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.head.branch.clone();
        let mut items = Vec::new();
        for r in &self.refs {
            let name = r.short_name.clone();
            match r.kind {
                RefKind::LocalBranch if current.as_deref() != Some(&name) => {
                    items.push(item(name.clone(), "local branch", move |t, w, cx| {
                        t.checkout_ref(RefKind::LocalBranch, name.clone(), w, cx)
                    }))
                }
                RefKind::RemoteBranch => {
                    let local = name
                        .split_once('/')
                        .map(|(_, b)| b.to_string())
                        .unwrap_or(name.clone());
                    let exists = self
                        .refs
                        .iter()
                        .any(|x| x.kind == RefKind::LocalBranch && x.short_name == local);
                    if !exists {
                        items.push(item(
                            name.clone(),
                            "remote branch → new tracking branch",
                            move |t, w, cx| {
                                t.checkout_ref(RefKind::RemoteBranch, name.clone(), w, cx)
                            },
                        ))
                    }
                }
                _ => {}
            }
        }
        items.push(item("Create Branch at HEAD…", "", |t, w, cx| {
            t.create_branch_dialog("HEAD".into(), "HEAD".into(), w, cx)
        }));
        let placeholder = match current {
            Some(b) => format!("Switch from {b} to…"),
            None => "Switch to branch…".into(),
        };
        self.open_picker(&placeholder, items, window, cx);
    }

    /// Branch picker for "Merge Branch into Current…" → merge options dialog.
    pub(super) fn open_merge_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.head.branch.clone();
        let items = self
            .refs
            .iter()
            .filter(|r| match r.kind {
                RefKind::LocalBranch => current.as_deref() != Some(&r.short_name),
                RefKind::RemoteBranch | RefKind::Tag => true,
                _ => false,
            })
            .map(|r| {
                let name = r.short_name.clone();
                let kind = match r.kind {
                    RefKind::LocalBranch => "local branch",
                    RefKind::RemoteBranch => "remote branch",
                    _ => "tag",
                };
                item(name.clone(), kind, move |t, w, cx| {
                    t.merge_dialog(name.clone(), name.clone(), w, cx)
                })
            })
            .collect();
        let placeholder = format!(
            "Merge into {}…",
            current.unwrap_or_else(|| "HEAD".into())
        );
        self.open_picker(&placeholder, items, window, cx);
    }

    // ------------------------------------------------------------------ rendering

    /// The picker's panel under the command bar (`None` while it drops down from the file
    /// tab bar instead, `file_tabs.rs`).
    pub(super) fn render_picker_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.picker.as_ref()?.at_file_tabs {
            return None;
        }
        self.picker_panel(PICKER_W, cx)
    }

    /// The picker's panel (query, note, entries), `width` wide.
    pub(super) fn picker_panel(&self, width: Pixels, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.picker.as_ref()?;
        let p = cx.palette().clone();
        let row_h = self.settings.row_h();
        let mut visible = Self::picker_rows(picker.filtered.len());
        // Under the command bar: no more rows than fit below it (input, note, margins).
        if self.repo.is_some()
            && !picker.at_file_tabs
            && let Some(room) = self.palette_room.get()
        {
            let chrome = row_h * 3. + px(24.);
            let fit = ((room - chrome) / row_h).floor().max(1.) as usize;
            visible = visible.min(fit);
        }
        let list: AnyElement = if picker.filtered.is_empty() {
            let empty_query = picker.input.read(cx).value().trim().is_empty();
            let text = match (&picker.hint, picker.searching) {
                (_, true) => "Searching…".to_string(),
                (Some(hint), false) if empty_query => hint.clone(),
                _ => "No matches".to_string(),
            };
            div()
                .px_3()
                .h(row_h)
                .flex()
                .items_center()
                .text_color(p.text_muted)
                .child(text)
                .into_any_element()
        } else {
            uniform_list(
                "picker-items",
                picker.filtered.len(),
                cx.processor(|this, range: Range<usize>, window, cx| {
                    let p = cx.palette().clone();
                    let row_h = this.settings.row_h();
                    let small = this.settings.ui_small();
                    let Some(picker) = this.picker.as_ref() else {
                        return Vec::new();
                    };
                    // The keys column: as wide as the widest keys of all entries,
                    // so it lines up over the rows; none when no entry has keys.
                    let keys_w = keys_column_width(&picker.items, small, window, cx);
                    range
                        .map(|ix| {
                            let it = &picker.items[picker.filtered[ix]];
                            let tag = picker.tags.get(picker.filtered[ix]).cloned().flatten();
                            let off = picker.disabled.get(picker.filtered[ix]).copied().flatten();
                            let selected = ix == picker.selected;
                            h_flex()
                                .id(("picker-item", ix))
                                .probe(format!("picker-item:{ix}"))
                                .w_full()
                                .h(row_h)
                                .px_3()
                                .gap_3()
                                .items_center()
                                .cursor_pointer()
                                .when(selected, |d| d.bg(p.bg_list_selected))
                                .when(!selected, |d| d.hover(|s| s.bg(p.bg_list_hover)))
                                .when_some(it.color, |d, c| {
                                    d.child(div().flex_none().size(px(12.)).rounded_full().bg(c))
                                })
                                // With a keys column the description follows the label
                                // (so it doesn't read as keys); without, it sits right.
                                .child(
                                    div()
                                        .probe(format!("picker-label:{ix}"))
                                        .when(keys_w.is_none(), |d| d.flex_1())
                                        .when(keys_w.is_some(), |d| d.flex_shrink(1.))
                                        .min_w_0()
                                        .truncate()
                                        .text_color(if off.is_some() {
                                            p.text_faint
                                        } else {
                                            p.text_strong
                                        })
                                        .child(it.label.clone()),
                                )
                                .when_some(tag, |d, tag| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .probe(format!("picker-tag:{ix}"))
                                            .text_size(small)
                                            .text_color(p.accent)
                                            .child(tag),
                                    )
                                })
                                // What it acts on: gives way to the label (shrinks first —
                                // flex shrinking is weighted by width, so a long detail
                                // next to a short label needs a big factor); fainter than
                                // the keys in a list with keys.
                                .when(!it.detail.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .probe(format!("picker-detail:{ix}"))
                                            .flex_shrink(1000.)
                                            .min_w_0()
                                            .truncate()
                                            .text_size(small)
                                            .text_color(if keys_w.is_some() {
                                                p.text_faint
                                            } else {
                                                p.text_muted
                                            })
                                            .child(it.detail.clone()),
                                    )
                                })
                                .when_some(keys_w, |d, w| {
                                    d.child(div().flex_1().min_w_0()).child(
                                        div()
                                            .probe(format!("picker-keys:{ix}"))
                                            .flex_none()
                                            .min_w(w)
                                            .flex()
                                            .justify_end()
                                            .text_size(small)
                                            .text_color(p.text_muted)
                                            .child(it.keys.clone()),
                                    )
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(p) = this.picker.as_mut() {
                                        p.selected = ix;
                                    }
                                    this.run_picker_selection(window, cx);
                                }))
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&picker.scroll)
            .w_full()
            .h(row_h * visible as f32)
            .into_any_element()
        };
        let panel = v_flex()
                        .w(width)
                        .overflow_hidden()
                        .bg(p.bg_modal)
                        .border_1()
                        // The focus colour: in a repository the panel lies over the
                        // toolbar's command bar and takes its focused look.
                        .border_color(p.accent)
                        .shadow_lg()
                        .key_context("GittiPicker")
                        .probe("picker")
                        // The command palette proper (not the file list dropped from the file
                        // tabs' chevron): `layout_check` holds it over the command bar.
                        .when(!picker.at_file_tabs, |d| d.probe("palette"))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().p_2().child(super::text_input(&picker.input, &self.settings)))
                        .when_some(picker.note.clone(), |d, note| {
                            d.child(
                                div()
                                    .probe("picker-note")
                                    .px_3()
                                    .pb_1()
                                    .text_size(self.settings.ui_small())
                                    .text_color(p.text_muted)
                                    .child(note),
                            )
                        })
                        .child(div().probe("picker-list").child(list));
        Some(panel.into_any_element())
    }

    /// The picker over the window: a backdrop (a click on it closes the picker). In a
    /// repository the panel drops down under the toolbar's command bar
    /// (`render_command_bar`); on the start page it sits near the top.
    pub(super) fn render_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.picker.as_ref()?;
        let panel = if self.repo.is_some() {
            None
        } else {
            self.render_picker_panel(cx)
        };
        Some(
            div()
                .id("picker-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .pt(px(48.))
                .bg(hsla(0., 0., 0., 0.12))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_picker(window, cx)),
                )
                .children(panel)
                .into_any_element(),
        )
    }
}

/// The width of the picker's keys column: the widest keys text of `items` in the small UI
/// font (`None` when no entry has keys — branch switchers, searches). Shaped lines are
/// cached by gpui across frames, so this is cheap per render.
fn keys_column_width(items: &[PickerItem], size: Pixels, window: &Window, cx: &App) -> Option<Pixels> {
    use gpui_kit::component::ActiveTheme as _;
    let family = cx.theme().font_family.clone();
    let mut seen = std::collections::HashSet::new();
    items
        .iter()
        .filter(|it| !it.keys.is_empty() && seen.insert(it.keys.as_str()))
        .map(|it| {
            let run = TextRun {
                len: it.keys.len(),
                font: font(family.clone()),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let text = SharedString::from(it.keys.clone());
            f32::from(window.text_system().shape_line(text, size, &[run], None).width)
        })
        .reduce(f32::max)
        .map(|w| px(w.ceil()))
}

impl Picker {
    /// The item under the cursor (an index into the items given to `open_picker`).
    pub(super) fn selected_item(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    /// Selects shown entry `ix` (clamped) and scrolls it into view.
    pub(super) fn select(&mut self, ix: usize) {
        self.selected = ix.min(self.filtered.len().saturating_sub(1));
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Nearest);
    }

    /// For tests: (selected entry, number of entries shown).
    pub(super) fn script_selection(&self) -> (usize, usize) {
        (self.selected, self.filtered.len())
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[gitti_test::test]
    fn fuzzy() {
        assert!(fuzzy_score("fet", "Fetch").is_some());
        assert!(fuzzy_score("sb", "Switch Branch…").is_some());
        assert!(fuzzy_score("xyz", "Fetch").is_none());
        // word starts beat scattered matches
        assert!(fuzzy_score("sb", "Switch Branch…") > fuzzy_score("sb", "Stash Pop: bla bla"));
        assert!(fuzzy_score("push", "Push") > fuzzy_score("push", "Force Push to origin"));
    }
}
