//! Headless tests of the menu bar and its dialogs: every menu entry is wired, the
//! new entries open their dialogs (also from the command palette), "Add to .gitignore…"
//! hides the file, Clone / Init open the new repository in a tab — also from the start
//! page when no tab is open and from a folder that is no repository.
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- menus[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::*;
use gitti_app::testing_menus as menus;

thread_local! {
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
}

/// `ui.fail(format!(…))` with the message built first (its arguments may use `ui`).
macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

fn base() -> PathBuf {
    BASE.with(|b| b.borrow().clone())
}

pub fn main() {
    let tmp = common::setup();
    BASE.with(|b| *b.borrow_mut() = tmp.path().to_path_buf());
    // "Open This Version": copies under the test dir, an "editor" that records its args.
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var("GITTI_VERSIONS_DIR", base().join("versions")) };
    fake_editor();
    let tests: &[(&str, Scenario)] = &[
        ("menu_entries", menu_entries),
        ("gitignore_dialog", gitignore_dialog),
        ("clone_and_init", clone_and_init),
        ("go_to_commit", go_to_commit),
        ("go_to_ref_head_file", go_to_ref_head_file),
        ("branch_commit_stash_menus", branch_commit_stash_menus),
        ("open_recent", open_recent),
        ("open_this_version", open_this_version),
        ("worktree_tab_menu", worktree_tab_menu),
        ("merge_unrelated_histories", merge_unrelated_histories),
        ("sidebar_menus", sidebar_menus),
    ];
    common::run(tests, |name| {
        let d = make_repo(&base().join(name));
        if name == "merge_unrelated_histories" {
            // Branch `other`: a root commit of its own, fetched from another repository.
            let src = base().join("unrelated-src");
            git(&base(), &["init", "-q", "-b", "main", "unrelated-src"]);
            git(&src, &["config", "user.name", "Other"]);
            git(&src, &["config", "user.email", "o@example.com"]);
            std::fs::write(src.join("o.txt"), "o\n").unwrap();
            git(&src, &["add", "."]);
            git(&src, &["commit", "-qm", "Other root"]);
            git(&d, &["fetch", "-q", src.to_str().unwrap(), "main:other"]);
            // git merges only with an index equal to HEAD.
            git(&d, &["reset", "-q"]);
        }
        if name == "sidebar_menus" {
            // A bare origin with main and older, a tag, a stash, a linked worktree.
            let origin = base().join("sidebar-origin.git");
            git(&base(), &["init", "-q", "--bare", "-b", "main", "sidebar-origin.git"]);
            git(&d, &["branch", "older", "HEAD~2"]);
            git(&d, &["tag", "v1", "HEAD~1"]);
            git(&d, &["remote", "add", "origin", origin.to_str().unwrap()]);
            git(&d, &["push", "-q", "origin", "main", "older"]);
            git(&d, &["fetch", "-q", "origin"]);
            git(&d, &["stash", "-q"]);
            let wt = base().join("sidebar-wt");
            git(&d, &["worktree", "add", "-q", "-b", "wtbranch", wt.to_str().unwrap()]);
        }
        if name == "go_to_ref_head_file" || name == "branch_commit_stash_menus" {
            git(&d, &["branch", "older", "HEAD~2"]);
            git(&d, &["tag", "v1", "HEAD~1"]);
        }
        d
    });
}

fn fake_editor() {
    install_fake_editor(&base(), &editor_log());
}

fn editor_log() -> PathBuf {
    base().join("editor.log")
}

/// The editor starts detached: wait (real time) for a new record, return the last line.
fn wait_editor_line(before: usize) -> Option<String> {
    for _ in 0..60 {
        let s = std::fs::read_to_string(editor_log()).unwrap_or_default();
        let lines: Vec<&str> = s.lines().collect();
        if lines.len() > before {
            return lines.last().map(|l| l.to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}

fn editor_lines() -> usize {
    std::fs::read_to_string(editor_log())
        .unwrap_or_default()
        .lines()
        .count()
}

fn state(ui: &mut Ui) -> testing::UiState {
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
}

fn picker_labels(ui: &mut Ui) -> Option<Vec<String>> {
    state(ui).picker.map(|p| p.labels)
}

fn selected(ui: &mut Ui) -> Option<String> {
    state(ui).selected_commit
}

fn modal(ui: &mut Ui) -> Option<(String, Vec<String>)> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal(&ws, cx))
}

fn modal_title(ui: &mut Ui) -> Option<String> {
    modal(ui).map(|m| m.0)
}

fn status(ui: &mut Ui) -> Option<(String, bool)> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::status(&ws, cx))
}

fn tabs(ui: &mut Ui) -> (usize, usize) {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::tabs(&ws, cx))
}

fn working(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::working(&ws, cx))
}

fn has_repo(ui: &mut Ui) -> bool {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::has_repo(&ws, cx))
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ------------------------------------------------------------------ scenarios

/// The menu bar has the full menu set and the new entries; the dialog entries open
/// their dialog (menu action and palette alike).
fn menu_entries(ui: &mut Ui) {
    let items = menus::menu_items();
    let has = |menu: &str, label: &str| items.iter().any(|(m, l, _)| m == menu && l == label);
    let fm = gitti_core::desktop::file_manager_name();
    for (menu, label) in [
        ("File", "Open Repository…"),
        ("File", "Clone Repository…"),
        ("File", "Init Repository…"),
        ("Repository", &format!("Open in {fm}") as &str),
        ("Repository", "Open in Terminal"),
        ("Repository", "Add to .gitignore…"),
        ("Repository", "Edit .gitignore…"),
        ("Repository", "Add Worktree…"),
        ("Repository", "Prune Worktrees"),
        ("Repository", "Git Settings: This Repository…"),
        ("Branch", "Create Branch at HEAD…"),
        ("Branch", "Merge Branch into Current…"),
        ("Commit", "Amend Previous Commit"),
        ("Commit", "Create Tag at HEAD…"),
        ("Commit", "Mark for Comparison"),
        ("Commit", "End Comparison"),
        ("Stash", "Pop Latest Stash"),
        ("Remote", "Add Remote…"),
        ("Help", "About gitti"),
        ("File", "Open Recent…"),
        ("View", "Toggle Sidebar"),
        ("View", "Toggle Full Screen"),
        ("Navigate", "Go to Commit…"),
        ("Navigate", "Go to Branch / Tag…"),
        ("Navigate", "Go to HEAD"),
        ("Navigate", "Go to File…"),
        ("Branch", "Rename Current Branch…"),
        ("Branch", "Delete Branch…"),
        ("Branch", "Set Upstream of Current Branch…"),
        ("Branch", "Rebase Current Branch onto…"),
        ("Commit", "Revert Selected Commit"),
        ("Commit", "Cherry-pick Selected Commit"),
        ("Commit", "Edit Commit Message…"),
        ("Commit", "Open This Version in Editor (read-only)"),
        ("Commit", "Open Old Version in Editor (read-only)"),
        ("Stash", "Stash Including Untracked"),
        ("Stash", "Drop Latest Stash…"),
        ("Stash", "Drop Stash…"),
    ] {
        if !has(menu, label) {
            fail!(ui, "no menu entry {menu} ▸ {label}");
        }
    }
    // Menu action → dialog title.
    for (action, title) in [
        ("gitti::CloneRepository", "Clone Repository"),
        ("gitti::InitRepository", "Init Repository"),
        ("gitti::AddToGitignore", "Add to .gitignore"),
        ("gitti::EditGitignore", "Edit .gitignore"),
        ("gitti::CreateBranch", "Create branch at HEAD"),
        ("gitti::CreateTag", "Create tag at HEAD"),
        ("gitti::AddRemote", "Add remote"),
        ("gitti::About", "About gitti"),
        ("gitti::RepoGitSettings", "Git settings"),
        ("gitti::AddWorktree", "Add Worktree"),
    ] {
        ui.step("focuslist");
        ui.step(&format!("action:{action}"));
        match modal_title(ui) {
            Some(t) if t.starts_with(title) => {}
            other => fail!(ui, "{action}: dialog {other:?}, expected {title}…"),
        }
        ui.step("cancel");
    }
    // The same dialogs from the palette.
    for (query, title) in [
        ("clone repository", "Clone Repository"),
        ("init repository", "Init Repository"),
        ("add to gitignore", "Add to .gitignore"),
        ("edit gitignore", "Edit .gitignore"),
        ("about gitti", "About gitti"),
        ("rename current branch", "Rename branch main"),
        ("add worktree", "Add Worktree"),
    ] {
        ui.step(&format!("palette:{query}"));
        match modal_title(ui) {
            Some(t) if t == title => {}
            other => fail!(ui, "palette `{query}`: dialog {other:?}, expected {title}"),
        }
        ui.step("cancel");
    }
    if modal_title(ui).is_some() {
        ui.fail("a dialog stayed open".into());
    }
    // Pickers from the palette.
    for query in ["go to commit", "go to branch / tag", "go to file", "drop stash"] {
        ui.step(&format!("palette:{query}"));
        let open = state(ui).picker.is_some();
        // No stash yet: Drop Stash only says so.
        if open != (query != "drop stash") {
            fail!(ui, "palette `{query}`: picker open {open}");
        }
        ui.step("cancel");
    }
}

/// "Add to .gitignore…": prefilled `/path`, the suggestion buttons fill the (editable)
/// line, OK appends it and the file disappears; a tracked file can be untracked.
fn gitignore_dialog(ui: &mut Ui) {
    let dir = base().join("gitignore_dialog");
    if !working(ui).iter().any(|l| l.contains("new.txt")) {
        fail!(ui, "new.txt not untracked to begin with: {:?}", working(ui));
    }
    ui.step("ignore:new.txt");
    match modal(ui) {
        Some((t, f)) if t == "Add to .gitignore" && f == ["/new.txt"] => {}
        other => fail!(ui, "ignore dialog: {other:?}"),
    }
    // Choices: 0 = this file, 1 = all *.txt files. A real click on the second one.
    let probes = ui.frame();
    let modal_box = find(&probes, "modal").and_then(Probe::visible);
    let choice = find(&probes, "modal-choice:1").and_then(Probe::visible);
    match (modal_box, choice) {
        (Some(m), Some(c)) if testing::contains(&m, &c) => {}
        other => fail!(ui, "choice button not inside the dialog: {other:?}"),
    }
    ui.click_on("modal-choice:1", MouseButton::Left);
    if modal(ui).map(|m| m.1) != Some(vec!["*.txt".to_string()]) {
        let now = modal(ui);
        fail!(ui, "choice did not fill the line: {now:?}");
    }
    // The line stays editable: back to the file itself.
    ui.step("field:0=/new.txt");
    ui.step("modalok");
    let w = working(ui);
    if w.iter().any(|l| l.contains("new.txt")) {
        fail!(ui, "new.txt still shown after ignoring it: {w:?}");
    }
    if !w.iter().any(|l| l.contains(".gitignore")) {
        fail!(ui, "the new .gitignore is not shown: {w:?}");
    }
    let text = std::fs::read_to_string(dir.join(".gitignore")).unwrap_or_default();
    if text != "/new.txt\n" {
        fail!(ui, ".gitignore is {text:?}");
    }
    match status(ui) {
        Some((s, false)) if s.contains("Added /new.txt to .gitignore") => {}
        other => fail!(ui, "status after ignoring: {other:?}"),
    }
    // Same pattern again: no second line.
    ui.step("ignore:.gitignore");
    ui.step("field:0=/new.txt");
    ui.step("modalok");
    let text = std::fs::read_to_string(dir.join(".gitignore")).unwrap_or_default();
    if text != "/new.txt\n" {
        fail!(ui, "duplicate line: .gitignore is {text:?}");
    }

    // A tracked file: ignoring alone keeps it; "stop tracking" removes it from the index.
    ui.step("ignore:big.txt");
    ui.step("checkbox:1");
    ui.step("modalok");
    if !git_out(&dir, &["ls-files", "big.txt"]).is_empty() {
        ui.fail("big.txt still tracked after Stop tracking".into());
    }
    if !dir.join("big.txt").exists() {
        ui.fail("big.txt deleted from disk".into());
    }
    let w = working(ui);
    if !w.iter().any(|l| l.starts_with("S big.txt")) {
        fail!(ui, "removal of big.txt not staged: {w:?}");
    }
}

/// A bare repository with one commit next to the scenario repositories.
fn bare_source(name: &str) -> PathBuf {
    let work = base().join(format!("{name}-work"));
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["config", "user.name", "T"]);
    git(&work, &["config", "user.email", "t@example.com"]);
    git(&work, &["config", "commit.gpgsign", "false"]);
    std::fs::write(work.join("readme.md"), "hi\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-qm", "init"]);
    let bare = base().join(format!("{name}.git"));
    git(
        &base(),
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    bare
}

/// Clone (URL → derived folder name, parent) and Init open the result in a new tab; both
/// also work from the start page (no tab) and Init turns a non-repository tab into one.
fn clone_and_init(ui: &mut Ui) {
    let bare = bare_source("upstream");
    let parent = base().join("clones");
    std::fs::create_dir_all(&parent).unwrap();

    ui.step("focuslist");
    ui.step("action:gitti::CloneRepository");
    ui.step(&format!("field:0={}", bare.display()));
    match modal(ui) {
        Some((t, f)) if t == "Clone Repository" && f.len() == 3 && f[2] == "upstream" => {}
        other => fail!(ui, "clone dialog after typing the URL: {other:?}"),
    }
    // The name follows the URL until the user types one; that one is kept.
    ui.step("field:0=git@example.com:team/other.git");
    if modal(ui).map(|m| m.1[2].clone()).as_deref() != Some("other") {
        let now = modal(ui);
        fail!(ui, "folder name does not follow the URL: {now:?}");
    }
    ui.step("field:2=mine");
    ui.step(&format!("field:0={}/", bare.display()));
    if modal(ui).map(|m| m.1[2].clone()).as_deref() != Some("mine") {
        let now = modal(ui);
        fail!(ui, "derived name overwrote the typed one: {now:?}");
    }
    ui.step(&format!("field:1={}", parent.display()));
    ui.step("modalok");
    let clone = parent.join("mine");
    if !clone.join("readme.md").exists() {
        fail!(ui, "no clone at {}: {:?}", clone.display(), status(ui));
    }
    if tabs(ui) != (2, 1) || !has_repo(ui) {
        let t = tabs(ui);
        fail!(ui, "clone not opened in a new tab: tabs {t:?}");
    }
    let ws = ui.ws.clone();
    let workdir = ui.cx.update(|cx| testing::workdir(&ws, cx));
    if workdir != gitti_core::paths::canonical(&clone) {
        fail!(ui, "active tab is {}", workdir.display());
    }
    // The clone ran through the tab's git runner: it is in its command log.
    ui.step("tab:0");
    if !ui
        .git_commands()
        .iter()
        .any(|c| c.contains("clone --progress"))
    {
        ui.fail("clone not in the command log of the tab it was started from".into());
    }

    // Init into a new folder.
    ui.step("action:gitti::InitRepository");
    let fresh = base().join("fresh");
    ui.step(&format!("field:0={}", fresh.display()));
    ui.step("modalok");
    if !fresh.join(".git").exists() || tabs(ui) != (3, 2) {
        let t = tabs(ui);
        fail!(ui, "init: tabs {t:?}, status {:?}", status(ui));
    }

    // A folder that is no repository: its tab offers Init for itself.
    let plain = base().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    ui.step(&format!("open:{}", plain.display()));
    if tabs(ui) != (4, 3) || has_repo(ui) {
        let t = tabs(ui);
        fail!(ui, "plain folder: tabs {t:?}");
    }
    ui.step("action:gitti::InitRepository");
    let want = gitti_core::paths::canonical(&plain).display().to_string();
    if modal(ui).map(|m| m.1) != Some(vec![want]) {
        let now = modal(ui);
        fail!(ui, "init dialog of a plain folder: {now:?}");
    }
    ui.step("modalok");
    if tabs(ui) != (4, 3) || !has_repo(ui) {
        let t = tabs(ui);
        fail!(
            ui,
            "plain folder after init: tabs {t:?}, repo {}",
            has_repo(ui)
        );
    }

    // A bare repository: said as such (not "Not a git repository"), and Init doesn't
    // offer the bare repository's own folder.
    ui.step(&format!("open:{}", bare.display()));
    if tabs(ui) != (5, 4) || has_repo(ui) {
        let t = tabs(ui);
        fail!(ui, "bare repository: tabs {t:?}");
    }
    let ws = ui.ws.clone();
    match ui.cx.update(|cx| menus::open_problem(&ws, cx)) {
        Some((t, m)) if t == "Bare repository" && m.contains("worktree add") => {}
        other => fail!(ui, "bare repository page: {other:?}"),
    }
    ui.step("action:gitti::InitRepository");
    let bare_dir = gitti_core::paths::canonical(&bare).display().to_string();
    if modal(ui).is_none_or(|m| m.1 == vec![bare_dir]) {
        let now = modal(ui);
        fail!(ui, "init dialog of a bare repository: {now:?}");
    }
    ui.step("key:escape");

    // No tab open: the start page hosts the dialogs.
    for _ in 0..5 {
        ui.step("key:secondary-w");
    }
    if tabs(ui).0 != 0 {
        let t = tabs(ui);
        fail!(ui, "tabs left: {t:?}");
    }
    if !find(&ui.frame(), "start-page").is_some_and(|p| p.visible().is_some()) {
        ui.fail("no start page without tabs".into());
    }
    ui.step("action:gitti::CloneRepository");
    ui.step(&format!("field:0={}", bare.display()));
    ui.step(&format!("field:1={}", parent.display()));
    ui.step("modalok");
    if tabs(ui) != (1, 0) || !has_repo(ui) || !parent.join("upstream").join(".git").exists() {
        let t = tabs(ui);
        fail!(
            ui,
            "clone from the start page: tabs {t:?}, status {:?}",
            status(ui)
        );
    }
}

fn rev(ui: &mut Ui, r: &str) -> String {
    let ws = ui.ws.clone();
    let dir = ui.cx.update(|cx| testing::workdir(&ws, cx));
    git_out(&dir, &["rev-parse", r])
}

/// Go to Commit: a background search over the loaded history by subject, author or hash
/// prefix; choosing a result selects that commit.
fn go_to_commit(ui: &mut Ui) {
    ui.step("focuslist");
    ui.step("action:gitti::GoToCommit");
    if picker_labels(ui) != Some(vec![]) {
        let l = picker_labels(ui);
        fail!(ui, "Go to Commit did not open an empty picker: {l:?}");
    }
    ui.step("pick:add f1");
    match picker_labels(ui) {
        Some(l) if l == ["Add f1"] => {}
        other => fail!(ui, "search `add f1`: {other:?}"),
    }
    ui.step("pickok");
    let want = rev(ui, "HEAD~2");
    if selected(ui).as_deref() != Some(want.as_str()) || state(ui).picker.is_some() {
        let s = selected(ui);
        fail!(ui, "Go to Commit selected {s:?}, expected {want}");
    }
    // Author (all commits) and hash prefix (found first, by its hash).
    ui.step("action:gitti::GoToCommit");
    ui.step("pick:layout test");
    if picker_labels(ui).map(|l| l.len()) != Some(5) {
        let l = picker_labels(ui);
        fail!(ui, "author search: {l:?}");
    }
    let initial = rev(ui, "HEAD~4");
    ui.step(&format!("pick:{}", &initial[..8]));
    match picker_labels(ui) {
        Some(l) if l.first().map(String::as_str) == Some("Initial import") => {}
        other => fail!(ui, "hash search: {other:?}"),
    }
    ui.step("pickok");
    if selected(ui) != Some(initial.clone()) {
        let s = selected(ui);
        fail!(ui, "hash search selected {s:?}");
    }
    // A file name: the commits that changed it, each tagged with the path, a line saying
    // what was searched; choosing one shows that file in the commit's diff.
    ui.step("action:gitti::GoToCommit");
    ui.step("pick:F2.txt");
    let p = state(ui).picker;
    match &p {
        Some(p)
            if p.labels == ["Rework the frobnicator", "Add f2"]
                && p.tags.iter().all(|t| t.as_deref() == Some("f2.txt"))
                && !p.searching
                && p.note.as_deref()
                    == Some(
                        "No commit message or author contains 'f2.txt' — showing commits \
                         that changed f2.txt",
                    ) => {}
        other => fail!(ui, "file search `F2.txt`: {other:?}"),
    }
    ui.step("pickok");
    let head = rev(ui, "HEAD");
    if selected(ui) != Some(head.clone()) {
        let s = selected(ui);
        fail!(ui, "file hit selected {s:?}, expected {head}");
    }
    let rows = state(ui).rows;
    let files: Vec<&String> = rows.iter().filter(|r| r.starts_with("file ")).collect();
    if files.len() != 1 || !files[0].contains("f2.txt") {
        fail!(ui, "file hit shows {files:?}");
    }
    // Same for a commit not loaded yet (the file is chosen when it arrives).
    ui.step("action:gitti::GoToCommit");
    ui.step("pick:f2.txt");
    ui.step("key:down");
    ui.step("pickok");
    let add = rev(ui, "HEAD~1");
    let rows = state(ui).rows;
    let files: Vec<&String> = rows.iter().filter(|r| r.starts_with("file ")).collect();
    if selected(ui) != Some(add.clone()) || files.len() != 1 || !files[0].contains("f2.txt") {
        let s = selected(ui);
        fail!(ui, "second file hit: selected {s:?} (expected {add}), files {files:?}");
    }
    // Nothing found: the picker stays, empty, and says what was searched.
    ui.step("action:gitti::GoToCommit");
    ui.step("pick:no such commit anywhere");
    let p = state(ui).picker;
    match &p {
        Some(p) if p.labels.is_empty() && p.note.as_deref().is_some_and(|n| n.contains("nothing matches 'anywhere'")) => {}
        other => fail!(ui, "no-match search: {other:?}"),
    }
    ui.check("go to commit: nothing found");
    ui.step("cancel");
}

/// Go to Branch / Tag, Go to HEAD, Go to File.
fn go_to_ref_head_file(ui: &mut Ui) {
    ui.step("focuslist");
    ui.step("action:gitti::GoToRef");
    match picker_labels(ui) {
        Some(l) if l.contains(&"older".to_string()) && l.contains(&"v1".to_string()) => {}
        other => fail!(ui, "Go to Branch / Tag: {other:?}"),
    }
    ui.step("pick:v1");
    ui.step("pickok");
    let tag = rev(ui, "v1^{commit}");
    if selected(ui) != Some(tag.clone()) {
        let s = selected(ui);
        fail!(ui, "Go to v1 selected {s:?}, expected {tag}");
    }
    ui.step("action:gitti::GoToHead");
    let head = rev(ui, "HEAD");
    if selected(ui) != Some(head.clone()) {
        let s = selected(ui);
        fail!(ui, "Go to HEAD selected {s:?}");
    }
    // HEAD's commit touches f0..f2: pick f2.txt → only its rows.
    ui.step("action:gitti::GoToFile");
    match picker_labels(ui) {
        Some(l) if l == ["f0.txt", "f1.txt", "f2.txt"] => {}
        other => fail!(ui, "Go to File in the commit: {other:?}"),
    }
    ui.step("pick:f2");
    ui.step("pickok");
    let rows = state(ui).rows;
    let files: Vec<&String> = rows.iter().filter(|r| r.starts_with("file ")).collect();
    if files.len() != 1 || !files[0].contains("f2.txt") {
        fail!(ui, "Go to File shows {files:?}");
    }
    // The working directory: its changed paths, a choice sets the file tab.
    ui.step("select:0");
    ui.step("action:gitti::GoToFile");
    match picker_labels(ui) {
        Some(l) if l == ["big.txt", "new.txt", "small.rs"] => {}
        other => fail!(ui, "Go to File in the working directory: {other:?}"),
    }
    ui.step("pick:small");
    ui.step("pickok");
    let ws = ui.ws.clone();
    let (_, active) = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx));
    if active.as_deref() != Some("small.rs") {
        fail!(ui, "worktree file tab after Go to File: {active:?}");
    }
}

/// Branch / Commit / Stash menu commands reach their dialogs and pickers (the native
/// confirmations are not answerable headless, so the git side is tested in the core).
fn branch_commit_stash_menus(ui: &mut Ui) {
    let dir = base().join("branch_commit_stash_menus");
    ui.step("focuslist");
    ui.step("action:gitti::RenameCurrentBranch");
    match modal(ui) {
        Some((t, f)) if t == "Rename branch main" && f == ["main"] => {}
        other => fail!(ui, "Rename Current Branch: {other:?}"),
    }
    ui.step("field:0=trunk");
    ui.step("modalok");
    if state(ui).branch.as_deref() != Some("trunk") {
        let b = state(ui).branch;
        fail!(ui, "branch after rename: {b:?}");
    }
    ui.step("action:gitti::DeleteBranchMenu");
    if picker_labels(ui) != Some(vec!["older".to_string()]) {
        let l = picker_labels(ui);
        fail!(ui, "Delete Branch picker: {l:?}");
    }
    ui.step("cancel");
    ui.step("action:gitti::RebaseCurrentOnto");
    match picker_labels(ui) {
        Some(l) if l.contains(&"older".into()) && !l.contains(&"trunk".into()) => {}
        other => fail!(ui, "Rebase onto picker: {other:?}"),
    }
    ui.step("cancel");
    ui.step("action:gitti::SetUpstreamMenu");
    if picker_labels(ui).is_none() {
        ui.fail("Set Upstream opened no picker".into());
    }
    ui.step("cancel");
    // The worktree row is selected: the Commit menu explains instead of acting.
    ui.step("select:0");
    let before = ui.git_commands().len();
    ui.step("action:gitti::RevertCommit");
    match status(ui) {
        Some((s, false)) if s.contains("Select a commit") => {}
        other => fail!(ui, "Revert on the worktree row: {other:?}"),
    }
    if ui.git_commands().len() != before {
        let ran = ui.git_commands()[before..].to_vec();
        fail!(ui, "Revert on the worktree row ran git: {ran:?}");
    }
    ui.step("action:gitti::EditCommitMessage");
    if modal_title(ui).is_some() {
        ui.fail("Edit Commit Message opened a dialog for the worktree row".into());
    }
    // Commit selected: Edit Commit Message opens its dialog.
    ui.step("select:1");
    ui.step("action:gitti::EditCommitMessage");
    match modal_title(ui) {
        Some(t) if t.starts_with("Edit message of") => {}
        other => fail!(ui, "Edit Commit Message: {other:?}, status {:?}", status(ui)),
    }
    ui.step("cancel");
    // Stash Including Untracked: new.txt goes into the stash too.
    ui.step("action:gitti::StashIncludingUntracked");
    let stashes = state(ui).stashes;
    if dir.join("new.txt").exists() || stashes != 1 {
        fail!(ui, "Stash Including Untracked: stashes {stashes}, status {:?}", status(ui));
    }
    ui.step("action:gitti::DropStash");
    match picker_labels(ui) {
        Some(l) if l.len() == 1 => {}
        other => fail!(ui, "Drop Stash picker: {other:?}"),
    }
    ui.step("cancel");
    // Toggle Sidebar from the menu: hidden, persisted, back.
    ui.step("action:gitti::ToggleSidebar");
    if find(&ui.frame(), "sidebar").is_some() {
        ui.fail("sidebar still drawn after Toggle Sidebar".into());
    }
    ui.step("action:gitti::ToggleSidebar");
    if find(&ui.frame(), "sidebar").is_none() {
        ui.fail("sidebar not back after the second Toggle Sidebar".into());
    }
}

/// Open Recent lists the repositories opened before (not the current one); choosing one
/// switches to it.
fn open_recent(ui: &mut Ui) {
    let other = make_repo(&base().join("recent-other"));
    ui.step(&format!("open:{}", other.display()));
    if tabs(ui) != (2, 1) {
        let t = tabs(ui);
        fail!(ui, "second repository not opened: {t:?}");
    }
    ui.step("action:gitti::OpenRecent");
    match picker_labels(ui) {
        Some(l) if l.first().map(String::as_str) == Some("open_recent") => {}
        other => fail!(ui, "Open Recent (newest first, current left out): {other:?}"),
    }
    ui.step("pickok");
    if tabs(ui) != (2, 0) {
        let t = tabs(ui);
        fail!(ui, "Open Recent did not switch to the first tab: {t:?}");
    }
    // Choosing it counts as opening it: newest again, the other one next.
    let recent = gitti_core::store::load_recent_repos();
    let want = [
        gitti_core::paths::canonical(&base().join("open_recent")),
        gitti_core::paths::canonical(&other),
    ];
    if recent.get(..2) != Some(&want[..]) {
        fail!(ui, "recent list: {recent:?}, expected {want:?} first");
    }
}

/// "Open This Version" writes the commit's file to a read-only copy named after the
/// commit and opens it in the editor; "Open Old Version" the parent's.
fn open_this_version(ui: &mut Ui) {
    let dir = base().join("open_this_version");
    let versions = base().join("versions").join("open_this_version");
    // The newest commit; the diff cursor is on its first file, f0.txt.
    ui.step("select:1");
    let before = editor_lines();
    ui.step("action:gitti::OpenThisVersion");
    let Some(line) = wait_editor_line(before) else {
        fail!(ui, "no editor started: {:?}", status(ui));
        return;
    };
    let path = PathBuf::from(line.rsplit_once(' ').map_or(line.as_str(), |(p, _)| p));
    let head = git_out(&dir, &["rev-parse", "HEAD"]);
    let want = versions.join(&head[..10]).join("f0.txt");
    if path != want {
        fail!(ui, "copy at {}, expected {}", path.display(), want.display());
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if text != git_out(&dir, &["show", "HEAD:f0.txt"]) + "\n" {
        fail!(ui, "copy content {text:?}");
    }
    if !std::fs::metadata(&path).is_ok_and(|m| m.permissions().readonly()) {
        ui.fail("copy is not read-only".into());
    }
    match status(ui) {
        Some((s, false)) if s.contains("read-only copy of f0.txt") => {}
        other => fail!(ui, "status after Open This Version: {other:?}"),
    }
    // The parent's version.
    let before = editor_lines();
    ui.step("action:gitti::OpenOldVersion");
    let line = wait_editor_line(before).unwrap_or_default();
    let path = PathBuf::from(line.rsplit_once(' ').map_or(line.as_str(), |(p, _)| p));
    let parent = git_out(&dir, &["rev-parse", "HEAD^"]);
    if path != versions.join(&parent[..10]).join("f0.txt")
        || std::fs::read_to_string(&path).unwrap_or_default()
            != git_out(&dir, &["show", "HEAD^:f0.txt"]) + "\n"
    {
        fail!(ui, "old version copy: {}", path.display());
    }
    // The working directory has no commit version: a message, no editor.
    ui.step("select:0");
    let before = editor_lines();
    ui.step("action:gitti::OpenThisVersion");
    match status(ui) {
        Some((s, false)) if s.contains("Open in Editor") => {}
        other => fail!(ui, "Open This Version in the working directory: {other:?}"),
    }
    if wait_editor_line(before).is_some() {
        ui.fail("an editor was started for the working directory".into());
    }
}

/// Right click on a working-directory file tab: its menu has the file actions; "Add to
/// .gitignore…" opens the dialog for that file (menu driven by keys).
fn worktree_tab_menu(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let (labels, _) = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx));
    let Some(ix) = labels.iter().position(|l| l.contains("new.txt")) else {
        fail!(ui, "no tab for new.txt: {labels:?}");
        return;
    };
    ui.click_on(&format!("file-tab:{ix}"), MouseButton::Right);
    // Show Only, Open in Editor, Stage File, Discard Changes…, Add to .gitignore…
    for _ in 0..5 {
        ui.step("key:down");
    }
    ui.step("key:enter");
    match modal(ui) {
        Some((t, f)) if t == "Add to .gitignore" && f == ["/new.txt"] => {}
        other => fail!(ui, "tab menu → Add to .gitignore: {other:?}"),
    }
    ui.step("cancel");
}

/// Merging a branch with its own history. Without the option git refuses; the
/// error offers "Merge anyway (allow unrelated histories)", which merges. The merge
/// dialog's option merges directly and is remembered for the next merge.
fn merge_unrelated_histories(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let dir = ui.cx.update(|cx| testing::workdir(&ws, cx));
    let parents = || git_out(&dir, &["rev-list", "--parents", "-n1", "HEAD"]).split_whitespace().count();
    let merge_dialog = |ui: &mut Ui| {
        ui.step("action:gitti::MergeBranch");
        ui.step("pick:other");
        ui.step("pickok");
        match modal(ui) {
            Some((t, _)) if t == "Merge other into main" => {}
            other => fail!(ui, "merge picker → dialog: {other:?}"),
        }
    };
    merge_dialog(ui);
    ui.step("modalok");
    match modal_title(ui) {
        Some(t) if t == "Unrelated histories" => {}
        other => fail!(ui, "no offer after git refused: {other:?} {:?}", status(ui)),
    }
    if parents() != 2 {
        fail!(ui, "merged before the offer was accepted");
    }
    ui.step("modalok");
    if parents() != 3 || !dir.join("o.txt").exists() {
        fail!(ui, "Merge anyway did not merge: {:?}", status(ui));
    }

    // The option in the dialog (5th checkbox) merges at once and stays checked.
    git(&dir, &["reset", "-q", "--keep", "HEAD~1"]);
    ui.step("action:gitti::Refresh");
    merge_dialog(ui);
    ui.step("checkbox:4");
    ui.step("modalok");
    if modal(ui).is_some() || parents() != 3 {
        fail!(ui, "merge with --allow-unrelated-histories: {:?} {:?}", modal(ui), status(ui));
    }
    git(&dir, &["reset", "-q", "--keep", "HEAD~1"]);
    ui.step("action:gitti::Refresh");
    merge_dialog(ui);
    ui.step("modalok");
    if modal(ui).is_some() || parents() != 3 {
        fail!(ui, "the option was not remembered: {:?} {:?}", modal(ui), status(ui));
    }
}

/// Every sidebar menu (rows and section headers) has the entries of
/// `gitti_core::sidebar_menus` (the same table gitty's menus are checked against);
/// "Create Branch from Stash…" works.
fn sidebar_menus(ui: &mut Ui) {
    use gitti_core::sidebar_menus::{SideMenu, Vars, missing};
    let fm = gitti_core::desktop::file_manager_name().to_string();
    let vars = |name: &str| Vars {
        name: name.into(),
        head: "main".into(),
        remote: "origin".into(),
        branch: name.rsplit('/').next().unwrap_or(name).into(),
        fm: fm.clone(),
    };
    let cases: &[(SideMenu, &str, &str)] = &[
        (SideMenu::CurrentBranch, "side:ref-refs/heads/main", "main"),
        (SideMenu::Branch, "side:ref-refs/heads/older", "older"),
        (SideMenu::RemoteBranch, "side:ref-refs/remotes/origin/older", "origin/older"),
        (SideMenu::Tag, "side:ref-refs/tags/v1", "v1"),
        (SideMenu::Remote, "side:remote:origin", "origin"),
        (SideMenu::Stash, "side:stash-0", "stash@{0}"),
        (SideMenu::Worktree, "side:wt-sidebar-wt", "sidebar-wt"),
        (SideMenu::BranchesSection, "side-section:branches", ""),
        (SideMenu::RemotesSection, "side-section:remotes", ""),
        (SideMenu::TagsSection, "side-section:tags", ""),
        (SideMenu::StashesSection, "side-section:stashes", ""),
        (SideMenu::WorktreesSection, "side-section:worktrees", ""),
    ];
    for &(menu, target, name) in cases {
        testing::menu_labels();
        ui.click_on(target, MouseButton::Right);
        let labels = testing::menu_labels();
        let miss = missing(menu, &vars(name), &labels);
        if !miss.is_empty() {
            fail!(ui, "{menu:?} menu ({target}) lacks {miss:?}; has {labels:?}");
        }
        ui.step("key:escape");
    }
    // Stash ▸ Create Branch from Stash… (Apply, Pop, then it).
    ui.click_on("side:stash-0", MouseButton::Right);
    for _ in 0..3 {
        ui.step("key:down");
    }
    ui.step("key:enter");
    match modal_title(ui) {
        Some(t) if t == "Create branch from stash@{0}" => {}
        other => fail!(ui, "Create Branch from Stash…: {other:?}"),
    }
    ui.step("field:0=from-stash");
    ui.step("modalok");
    let st = state(ui);
    if st.branch.as_deref() != Some("from-stash") || st.stashes != 0 {
        fail!(ui, "after Create Branch from Stash: branch {:?}, stashes {}", st.branch, st.stashes);
    }
}
