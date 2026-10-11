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
    static IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
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
    // A private global git config (the user's own isn't read): git's default object
    // format comes from it (`init_object_format`). Fixtures write their identity locally.
    let global = tmp.path().join("gitconfig");
    std::fs::write(&global, "[init]\n\tdefaultObjectFormat = sha1\n").unwrap();
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", &global) };
    // "Open This Version": copies under the test dir, an "editor" that records its args.
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var("GITTI_VERSIONS_DIR", base().join("versions")) };
    fake_editor();
    let tests: &[(&str, Scenario)] = &[
        ("go_to_ref_head_file", go_to_ref_head_file),
        ("worktree_tab_menu", worktree_tab_menu),
        ("menu_entries", menu_entries),
        ("gitignore_dialog", gitignore_dialog),
        ("terminal_chooser", terminal_chooser),
        ("discard_all_nested_repository", discard_all_nested_repository),
        ("switch_blocked_ways", switch_blocked_ways),
        ("folder_rule_files", folder_rule_files),
        ("clone_and_init", clone_and_init),
        ("clone_from_clipboard", clone_from_clipboard),
        ("init_object_format", init_object_format),
        ("go_to_commit", go_to_commit),
        ("search_in_changes", search_in_changes),
        ("branch_commit_stash_menus", branch_commit_stash_menus),
        ("stash_dialogs", stash_dialogs),
        ("open_recent", open_recent),
        ("open_this_version", open_this_version),
        ("restore_and_containing", restore_and_containing),
        ("merge_unrelated_histories", merge_unrelated_histories),
        ("pull_diverged", pull_diverged),
        ("push_refused_ways_out", push_refused_ways_out),
        ("sidebar_menus", sidebar_menus),
        ("tag_message_and_show_tag", tag_message_and_show_tag),
        ("startup_timings", startup_timings),
        ("about_and_licences", about_and_licences),
        ("unavailable_commands", unavailable_commands),
        ("detached_head_greys_branch_commands", detached_head_greys_branch_commands),
        ("fresh_repository_greys_what_needs_a_commit", fresh_repository_greys_what_needs_a_commit),
    ];
    common::run(tests, |name| {
        let d = base().join(name);
        if name == "pull_diverged" || name == "push_refused_ways_out" {
            diverged_repo(&d);
            return d;
        }
        if name == "fresh_repository_greys_what_needs_a_commit" {
            init_repo(&d);
            return d;
        }
        let ids = make_repo_ids(&d);
        if name == "discard_all_nested_repository" {
            // A cloned project inside the repository: an untracked folder with its own .git.
            git(&d, &["init", "-q", "nested"]);
            std::fs::write(d.join("nested/n.txt"), "n\n").unwrap();
        }
        if name == "switch_blocked_ways" {
            // Branch `feat` (HEAD's commit + x.txt); an untracked x.txt here blocks it.
            let mut s = Stream::new();
            s.commit_on("refs/heads/feat", "Add x", &ids[5], &[("x.txt", Some("theirs\n"))]);
            s.import(&d);
            std::fs::write(d.join("x.txt"), "ours\n").unwrap();
        }
        if name == "detached_head_greys_branch_commands" {
            // HEAD on the commit itself, written as git would (no git process).
            std::fs::write(d.join(".git/HEAD"), format!("{}\n", ids[5])).unwrap();
        }
        if name == "merge_unrelated_histories" {
            // Branch `other`: a root commit of its own.
            let mut s = Stream::new();
            s.commit_by("refs/heads/other", "Other <o@example.com>", "Other root", None, &[("o.txt", Some("o\n"))]);
            s.import(&d);
            // git merges only with an index equal to HEAD.
            git(&d, &["reset", "-q"]);
        }
        if name == "sidebar_menus" {
            // A bare origin (sharing the objects) with main and older, a tag, a stash, a
            // linked worktree.
            let origin = base().join("sidebar-origin.git");
            git(&base(), &["init", "-q", "--bare", "-b", "main", "sidebar-origin.git"]);
            std::fs::create_dir_all(origin.join("objects/info")).unwrap();
            std::fs::write(origin.join("objects/info/alternates"), format!("{}\n", d.join(".git/objects").display())).unwrap();
            for (name, id) in [("main", &ids[5]), ("older", &ids[3])] {
                std::fs::write(origin.join(format!("refs/heads/{name}")), format!("{id}\n")).unwrap();
                write_ref(&d, &format!("refs/remotes/origin/{name}"), id);
            }
            append_config(
                &d,
                &format!("[remote \"origin\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n", origin.display()),
            );
            write_ref(&d, "refs/heads/older", &ids[3]);
            write_ref(&d, "refs/tags/v1", &ids[4]);
            git(&d, &["stash", "-q"]);
            let wt = base().join("sidebar-wt");
            git(&d, &["worktree", "add", "-q", "-b", "wtbranch", wt.to_str().unwrap()]);
        }
        if name == "go_to_ref_head_file" || name == "branch_commit_stash_menus" {
            write_ref(&d, "refs/heads/older", &ids[3]);
            write_ref(&d, "refs/tags/v1", &ids[4]);
        }
        IDS.with(|i| *i.borrow_mut() = ids);
        d
    });
}

fn fake_editor() {
    install_fake_editor(&base(), &editor_log());
}

fn editor_log() -> PathBuf {
    base().join("editor.log")
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

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ------------------------------------------------------------------ scenarios

fn rev(ui: &mut Ui, r: &str) -> String {
    let ws = ui.ws.clone();
    let dir = ui.cx.update(|cx| testing::workdir(&ws, cx));
    git_out(&dir, &["rev-parse", r])
}

/// Go to Branch / Tag, Go to HEAD, Go to File (the file list; a commit with one
/// file has no file tab bar, the list still shows — under the command bar).
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
    ui.step("action:gitti::FileList");
    match picker_labels(ui) {
        Some(l) if l == ["All Files (3)", "f0.txt", "f1.txt", "f2.txt"] => {}
        other => fail!(ui, "Go to File in the commit: {other:?}"),
    }
    ui.step("pick:f2");
    ui.step("pickok");
    let rows = state(ui).rows;
    let files: Vec<&String> = rows.iter().filter(|r| r.starts_with("file ")).collect();
    if files.len() != 1 || !files[0].contains("f2.txt") {
        fail!(ui, "Go to File shows {files:?}");
    }
    // "Add f2": one file, no file tab bar — the list is drawn anyway.
    ui.step("select:2");
    ui.step("action:gitti::FileList");
    match picker_labels(ui) {
        Some(l) if l == ["All Files (1)", "f2.txt"] => {}
        other => fail!(ui, "Go to File in a one-file commit: {other:?}"),
    }
    let p = ui.check("go to file, one-file commit");
    if find(&p, "file-tabs").is_some() || find(&p, "palette").is_none() {
        fail!(ui, "one-file commit: the file list isn't drawn under the command bar");
    }
    ui.step("action:gitti::ModalCancel");
    // The working directory: its changed paths, a choice sets the file tab.
    ui.step("select:0");
    ui.step("action:gitti::FileList");
    match picker_labels(ui) {
        Some(l) if l == ["All Changes (3)", "big.txt", "new.txt", "small.rs"] => {}
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
    // Show Only, Open in Editor, Stage File, Discard Changes…, Stash File…, Add to
    // .gitignore…
    for _ in 0..6 {
        ui.step("key:down");
    }
    ui.step("key:enter");
    match modal(ui) {
        // One pattern line per target: .gitignore, .git/info/exclude.
        Some((t, f)) if t == "Add to .gitignore" && f == ["/new.txt", "/new.txt"] => {}
        other => fail!(ui, "tab menu → Add to .gitignore: {other:?}"),
    }
    ui.step("cancel");
}


/// Help ▸ Start-up Timings… shows the kept starts (newest first, with the explanation) from
/// `<config dir>/logs/startup-times.json`, without running git.
fn startup_timings(ui: &mut Ui) {
    use gitti_core::startup::{Phase, StartupRecord, TabTiming, append_at, file};
    let path = file().expect("config dir");
    let record = |version: &str, before_main_ms: u64| StartupRecord {
        app: "gitti".into(),
        version: version.into(),
        at_ms: 1_791_300_000_000,
        before_main_ms: Some(before_main_ms),
        before_main_cpu_ms: Some(40),
        exe_bytes: Some(82_400_000),
        exe_modified_ms: Some(1_791_200_000_000),
        phases: vec![Phase { name: "window open".into(), ms: 570 }],
        tabs: vec![TabTiming { repo: "gitti".into(), ms: 2510, git_calls: 31, git_ms: 2100, failed: false }],
        active_tab: None,
        complete: true,
    };
    let _ = std::fs::remove_file(&path);
    append_at(&path, &record("9.9.1", 30)).unwrap();
    append_at(&path, &record("9.9.2", 12_800)).unwrap();
    let n = ui.git_commands().len();
    ui.quick("focuslist");
    ui.quick("action:gitti::StartupTimings");
    if modal_title(ui).as_deref() != Some("Start-up Timings") {
        fail!(ui, "Start-up Timings: dialog {:?}", modal_title(ui));
    }
    let ws = ui.ws.clone();
    let text = ui.cx.update(|cx| testing::modal_viewer_text(&ws, cx)).unwrap_or_default();
    let newest = text.find("gitti 9.9.2 — usable after 15.31 s — slow");
    let older = text.find("gitti 9.9.1 — usable after 2.54 s");
    if !matches!((newest, older), (Some(a), Some(b)) if a < b) {
        fail!(ui, "Start-up Timings text (newest first):\n{text}");
    }
    for part in ["program loaded", "+12.80 s  (CPU 0.04 s)", "gitti shown", "31 git calls before", "What the steps mean"] {
        if !text.contains(part) {
            fail!(ui, "Start-up Timings text lacks `{part}`:\n{text}");
        }
    }
    let ran = ui.git_commands()[n..].to_vec();
    if !ran.is_empty() {
        fail!(ui, "Start-up Timings ran git: {ran:?}");
    }
    ui.quick("cancel");
}

/// Help ▸ About: the build commit first, licence and third-party summary; Copy Version
/// copies the version line and leaves the window open; Third-Party Licences… opens the
/// list of the components compiled into gitti on this platform, with their texts.
fn about_and_licences(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let viewer = |ui: &mut Ui| ui.cx.update(|cx| testing::modal_viewer_text(&ws, cx)).unwrap_or_default();
    let (version, build) = (env!("CARGO_PKG_VERSION"), env!("GITTI_BUILD"));
    let n = ui.git_commands().len();
    ui.quick("focuslist");
    ui.quick("action:gitti::About");
    let text = viewer(ui);
    if !text.starts_with(&format!("gitti {version}\nBuilt from git commit {build}\n")) {
        fail!(ui, "About doesn't start with the version and build commit:\n{text}");
    }
    let os = gitti_core::third_party::os_name(std::env::consts::OS);
    for part in [
        "GNU General Public License",
        "WITHOUT ANY WARRANTY",
        "Source code: https://github.com/sneffets/gitti",
        "Report a security issue (privately): ",
        &format!("\nThird-party components\ngitti on {os} contains "),
        "Their licences: the Third-Party Licences… button below",
        "\nRuns git ",
        "\nSettings and logs: ",
    ] {
        if !text.contains(part) {
            fail!(ui, "About lacks `{part}`:\n{text}");
        }
    }
    ui.cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
    ui.click_on("modal-copy", MouseButton::Left);
    ui.settle();
    let copied = ui.cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
    if copied != gitti_core::about::version_line("gitti", version, build) {
        fail!(ui, "Copy Version copied {copied:?}");
    }
    if modal_title(ui).as_deref() != Some("About gitti") {
        fail!(ui, "Copy Version closed About: {:?}", modal_title(ui));
    }
    ui.click_on("modal-extra", MouseButton::Left);
    ui.settle();
    if modal_title(ui).as_deref() != Some("Third-Party Licences") {
        fail!(ui, "Third-Party Licences…: dialog {:?}", modal_title(ui));
    }
    let list = viewer(ui);
    let head = format!("Third-party licences of gitti on {os}\n\ngitti on {os} contains ");
    if !list.starts_with(&head) || !list.contains("\n  Lucide icons — ISC AND MIT [") || !list.contains("\nLicence texts\n\n[1] used by ") {
        fail!(ui, "licence list:\n{}", &list[..list.len().min(2000)]);
    }
    let ran = ui.git_commands()[n..].to_vec();
    if !ran.is_empty() {
        fail!(ui, "About / licences ran git: {ran:?}");
    }
    ui.quick("cancel");
}

// ------------------------------------------------------------------ lean scenarios

fn modal_title(ui: &mut Ui) -> Option<String> {
    modal(ui).map(|m| m.0)
}

fn status(ui: &mut Ui) -> Option<(String, bool)> {
    ui.status()
}

fn has_repo(ui: &mut Ui) -> bool {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::has_repo(&ws, cx))
}

/// The editor starts detached: wait (real time) for a new record, return the last line.
fn wait_editor_line(before: usize) -> Option<String> {
    for _ in 0..100 {
        let s = std::fs::read_to_string(editor_log()).unwrap_or_default();
        let lines: Vec<&str> = s.lines().collect();
        if lines.len() > before {
            return lines.last().map(|l| l.to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

fn editor_lines() -> usize {
    std::fs::read_to_string(editor_log()).unwrap_or_default().lines().count()
}

/// The menu bar has the full menu set and the new entries; the dialog entries open their
/// dialog (menu action and palette alike), the pickers too.
fn menu_entries(ui: &mut Ui) {
    let items = menus::menu_items();
    let has = |menu: &str, label: &str| items.iter().any(|(m, l, _)| m == menu && l == label);
    let fm = gitti_core::desktop::file_manager_name();
    for (menu, label) in [
        ("File", "Open Repository…"),
        ("File", "Clone Repository…"),
        ("File", "Init Repository…"),
        ("File", "Open Recent…"),
        ("Repository", &format!("Open in {fm}") as &str),
        ("Repository", "Open in Terminal"),
        ("View", "Toggle Main Worktree When It Is the Only One"),
        ("Repository", "Add to .gitignore…"),
        ("Repository", "Edit .gitignore…"),
        ("Repository", "Add Worktree…"),
        ("Repository", "Prune Worktrees"),
        ("Repository", "Git Settings: This Repository…"),
        ("Branch", "Create Branch at HEAD…"),
        ("Branch", "Merge Branch into Current…"),
        ("Branch", "Rename Current Branch…"),
        ("Branch", "Delete Branch…"),
        ("Branch", "Set Upstream of Current Branch…"),
        ("Branch", "Rebase Current Branch onto…"),
        ("Commit", "Amend Previous Commit"),
        ("Commit", "Create Tag at HEAD…"),
        ("Commit", "Mark for Comparison"),
        ("Commit", "End Comparison"),
        ("Commit", "Revert Selected Commit"),
        ("Commit", "Cherry-pick Selected Commit"),
        ("Commit", "Edit Commit Message…"),
        ("Commit", "Open This Version in Editor (read-only)"),
        ("Commit", "Open Old Version in Editor (read-only)"),
        ("Stash", "Pop Latest Stash"),
        ("Stash", "Stash Including Untracked"),
        ("Stash", "Drop Latest Stash…"),
        ("Stash", "Drop Stash…"),
        ("Remote", "Add Remote…"),
        ("View", "Toggle Sidebar"),
        ("View", "Toggle Full Screen"),
        ("Navigate", "Go to Commit…"),
        ("Navigate", "Go to Branch / Tag…"),
        ("Navigate", "Go to HEAD"),
        ("Navigate", "Go to File…"),
        ("Help", "About gitti"),
        ("Help", "Start-up Timings…"),
        ("Help", "Third-Party Licences…"),
    ] {
        if !has(menu, label) {
            fail!(ui, "no menu entry {menu} ▸ {label}");
        }
    }
    for (action, title) in [
        ("gitti::CloneRepository", "Clone Repository"),
        ("gitti::InitRepository", "Init Repository"),
        ("gitti::AddToGitignore", "Add to .gitignore"),
        ("gitti::EditGitignore", "Edit .gitignore"),
        ("gitti::CreateBranch", "Create branch at HEAD"),
        ("gitti::CreateTag", "Create tag at HEAD"),
        ("gitti::AddRemote", "Add remote"),
        ("gitti::About", "About gitti"),
        ("gitti::ThirdPartyLicences", "Third-Party Licences"),
        ("gitti::StartupTimings", "Start-up Timings"),
        ("gitti::RepoGitSettings", "Git settings"),
        ("gitti::AddWorktree", "Add Worktree"),
    ] {
        ui.quick("focuslist");
        ui.quick(&format!("action:{action}"));
        match modal_title(ui) {
            Some(t) if t.starts_with(title) => {}
            other => fail!(ui, "{action}: dialog {other:?}, expected {title}…"),
        }
        ui.quick("cancel");
    }
    for (query, title) in [
        ("clone repository", "Clone Repository"),
        ("init repository", "Init Repository"),
        ("add to gitignore", "Add to .gitignore"),
        ("edit gitignore", "Edit .gitignore"),
        ("about gitti", "About gitti"),
        ("third-party licences", "Third-Party Licences"),
        ("start-up timings", "Start-up Timings"),
        ("rename current branch", "Rename branch main"),
        ("add worktree", "Add Worktree"),
    ] {
        ui.quick(&format!("palette:{query}"));
        match modal_title(ui) {
            Some(t) if t == title => {}
            other => fail!(ui, "palette `{query}`: dialog {other:?}, expected {title}"),
        }
        ui.quick("cancel");
    }
    if modal_title(ui).is_some() {
        ui.fail("a dialog stayed open".into());
    }
    // The palette knows synonyms — either word finds every such command, the typed word's
    // own commands first.
    for (query, first, want) in [
        (
            "new",
            "",
            &["Create Branch at HEAD…", "Create Tag at HEAD…", "Add Remote…", "Add Worktree…"][..],
        ),
        ("create", "Create", &["Add Remote…", "Add Submodule…"][..]),
        ("remove", "Remove", &["Delete Branch…", "Drop Stash…"][..]),
        ("checkout", "", &["Switch Branch…"][..]),
    ] {
        ui.quick("action:gitti::CommandPalette");
        ui.quick(&format!("pick:{query}"));
        let labels = picker_labels(ui).unwrap_or_default();
        let missing: Vec<_> = want.iter().filter(|w| !labels.iter().any(|l| l == *w)).collect();
        if !missing.is_empty() || !labels.first().is_some_and(|l| l.starts_with(first)) {
            fail!(ui, "palette `{query}`: {missing:?} missing or not `{first}…` first: {labels:?}");
        }
        ui.quick("cancel");
    }
    for query in ["go to commit", "go to branch / tag", "go to file", "drop stash"] {
        ui.quick(&format!("palette:{query}"));
        let open = state(ui).picker.is_some();
        // No stash yet: Drop Stash only says so.
        if open != (query != "drop stash") {
            fail!(ui, "palette `{query}`: picker open {open}");
        }
        ui.quick("cancel");
    }
}

/// Settings ▸ Diff ▸ Terminal: the chooser's "Automatic" button empties the command; a
/// typed command says live what it runs and warns when its program is missing; Save
/// stores it (the shared setting) and the row warns too; Open in Terminal (sandboxed: not
/// started) names it in the status line; the palette's "Choose Terminal…" opens the same
/// chooser, back to Automatic. No git runs for any of it.
fn terminal_chooser(ui: &mut Ui) {
    use gitti_core::store::{SharedSettings, load_settings};
    let live = |ui: &mut Ui| {
        let ws = ui.ws.clone();
        ui.cx.update(|cx| menus::modal_live(&ws, cx)).map(|l| l.0).unwrap_or_default()
    };
    let n = ui.n_git();
    // The row is the section's last: scrolled into view (a wheel now and then doesn't
    // reach the list in the first frame of the settings).
    let reveal = |ui: &mut Ui, name: &str| {
        for _ in 0..3 {
            if find(&ui.frame(), name).and_then(Probe::visible).is_some() {
                return;
            }
            ui.wheel("settings-body", 2000.);
        }
    };
    ui.step("settings:diff");
    reveal(ui, "setting:terminal");
    ui.click_on("setting:terminal", MouseButton::Left);
    match modal(ui) {
        Some((t, f)) if t == "Terminal" && f == [""] => {}
        other => fail!(ui, "terminal chooser: {other:?}"),
    }
    let text = live(ui);
    if !text.starts_with("Automatic") {
        fail!(ui, "the empty command should say what Automatic runs: {text:?}");
    }
    let custom = "gitti-no-such-term --dir {dir}";
    ui.quick(&format!("field:0={custom}"));
    let text = live(ui);
    if !text.contains(&format!("Custom — runs {custom}")) || !text.contains("is not installed") {
        fail!(ui, "a missing program should be named: {text:?}");
    }
    // The first button is Automatic: it empties the field.
    ui.step("choice:0");
    if modal(ui).map(|m| m.1) != Some(vec![String::new()]) {
        let now = modal(ui);
        fail!(ui, "Automatic did not empty the command: {now:?}");
    }
    ui.quick(&format!("field:0={custom}"));
    ui.step("modalok");
    let stored = load_settings::<SharedSettings>().terminal_command;
    if stored != custom {
        fail!(ui, "stored terminal command {stored:?}");
    }
    reveal(ui, "setting:terminal-warning");
    let p = ui.frame();
    match find(&p, "setting:terminal-warning").and_then(Probe::visible) {
        Some(_) => {}
        None => fail!(ui, "the settings row doesn't warn about the missing terminal"),
    }
    ui.step("key:escape");
    ui.step("action:gitti::OpenTerminal");
    match status(ui) {
        Some((s, false)) if s.starts_with("Opened gitti-no-such-term in ") => {}
        other => fail!(ui, "status after Open in Terminal: {other:?}"),
    }
    // Back to Automatic from the palette.
    ui.step("palette:Choose Terminal");
    if modal_title(ui).as_deref() != Some("Terminal") {
        let now = modal(ui);
        fail!(ui, "palette ▸ Choose Terminal… opened {now:?}");
    }
    ui.step("choice:0");
    ui.step("modalok");
    let stored = load_settings::<SharedSettings>().terminal_command;
    if !stored.is_empty() {
        fail!(ui, "Automatic stored {stored:?}");
    }
    ui.no_git_since(n, "the terminal chooser and Open in Terminal");
}

/// "Add to .gitignore…" is prefilled `/path`, the suggestion buttons fill the (editable)
/// line, OK appends it and the file disappears; the same pattern twice gives one line; a
/// tracked file can be untracked (staged removal, kept on disk).
fn gitignore_dialog(ui: &mut Ui) {
    let dir = base().join("gitignore_dialog");
    ui.quick("ignore:new.txt");
    match modal(ui) {
        Some((t, f)) if t == "Add to .gitignore" && f == ["/new.txt", "/new.txt"] => {}
        other => fail!(ui, "ignore dialog: {other:?}"),
    }
    let probes = ui.frame();
    match (find(&probes, "modal").and_then(Probe::visible), find(&probes, "modal-choice:1").and_then(Probe::visible)) {
        (Some(m), Some(c)) if testing::contains(&m, &c) => {}
        other => fail!(ui, "choice button not inside the dialog: {other:?}"),
    }
    ui.click_on("modal-choice:1", MouseButton::Left);
    if modal(ui).map(|m| m.1[0].clone()) != Some("*.txt".to_string()) {
        let now = modal(ui);
        fail!(ui, "choice did not fill the line: {now:?}");
    }
    ui.quick("field:0=/new.txt");
    ui.step("modalok");
    let w = ui.working();
    if w.iter().any(|l| l.contains("new.txt")) || !w.iter().any(|l| l.contains(".gitignore")) {
        fail!(ui, "after ignoring new.txt: {w:?}");
    }
    let text = std::fs::read_to_string(dir.join(".gitignore")).unwrap_or_default();
    if text != "/new.txt\n" {
        fail!(ui, ".gitignore is {text:?}");
    }
    match status(ui) {
        Some((s, false)) if s.contains("Added /new.txt to .gitignore") => {}
        other => fail!(ui, "status after ignoring: {other:?}"),
    }
    ui.quick("ignore:.gitignore");
    ui.quick("field:0=/new.txt");
    ui.step("modalok");
    let text = std::fs::read_to_string(dir.join(".gitignore")).unwrap_or_default();
    if text != "/new.txt\n" {
        fail!(ui, "duplicate line: .gitignore is {text:?}");
    }
    ui.quick("ignore:big.txt");
    ui.quick("checkbox:0");
    ui.step("modalok");
    if !dir.join("big.txt").exists() {
        ui.fail("big.txt deleted from disk".into());
    }
    let w = ui.working();
    if !w.iter().any(|l| l.starts_with("S big.txt")) {
        fail!(ui, "removal of big.txt not staged: {w:?}");
    }
}

/// Discard All with an untracked folder that is a repository of its own: the dialog says
/// it is kept and offers a box to delete it too; unticked, everything else goes and the
/// status line names what was kept (before, `git clean` skipped it silently and it stayed
/// listed as if Discard had worked only partly); ticked, it goes too.
fn discard_all_nested_repository(ui: &mut Ui) {
    let dir = base().join("discard_all_nested_repository");
    ui.quick("discardall");
    match modal_title(ui) {
        Some(t) if t.starts_with("Discard all unstaged changes in 3 file(s)?") => {}
        other => fail!(ui, "Discard All with a nested repository: {other:?}"),
    }
    let ws = ui.ws.clone();
    let message = ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default();
    if !message.contains("nested/") || !message.contains("separate git repository") {
        fail!(ui, "the dialog does not name the nested repository: {message:?}");
    }
    ui.step("modalok");
    ui.settle();
    if !dir.join("nested/n.txt").exists() || dir.join("new.txt").exists() {
        ui.fail("unticked: nested/ must stay, new.txt must go".into());
    }
    match status(ui) {
        Some((s, false)) if s.contains("nested/ was kept") => {}
        other => fail!(ui, "status after Discard All: {other:?}"),
    }
    let w = ui.working();
    if w.iter().any(|l| l.starts_with("U ") && !l.contains("nested/")) {
        fail!(ui, "left besides nested/: {w:?}");
    }
    // Its diff pane says what it is instead of "no preview".
    if !ui.check("nested/ left").iter().any(|p| p.kind() == "no-preview") {
        ui.fail("nested/: no note saying it is a repository of its own".into());
    }
    ui.quick("discardall");
    ui.quick("checkbox:0");
    ui.step("modalok");
    ui.settle();
    if dir.join("nested").exists() {
        ui.fail("ticked: nested/ must be deleted".into());
    }
    let w = ui.working();
    if w.iter().any(|l| l.starts_with("U ")) {
        fail!(ui, "left after deleting nested/: {w:?}");
    }
}

/// A switch git refuses (an untracked x.txt that `feat` has): the dialog says what blocks
/// it and offers the three ways out, each explained as it is chosen; Delete & Switch
/// deletes only that file and switches.
fn switch_blocked_ways(ui: &mut Ui) {
    let dir = base().join("switch_blocked_ways");
    ui.quick("palette:Switch Branch");
    ui.quick("pick:feat");
    ui.step("pickok");
    ui.settle();
    if modal_title(ui).as_deref() != Some("Switch to feat?") {
        let t = modal_title(ui);
        fail!(ui, "no dialog for the blocked switch: {t:?}");
        return;
    }
    let ws = ui.ws.clone();
    let message = ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default();
    if !message.contains("x.txt") || !message.contains("would be replaced") {
        fail!(ui, "the dialog does not say what blocks: {message:?}");
    }
    let live = |ui: &mut Ui| ui.cx.update(|cx| menus::modal_live(&ws, cx)).unwrap_or_default();
    let first = live(ui);
    if !first.0.starts_with("Stash & Switch:") || first.1.as_deref() != Some("Stash & Switch") {
        fail!(ui, "first way: {first:?}");
    }
    ui.quick("option:2");
    let force = live(ui);
    if !force.0.contains("--discard-changes") || force.1.as_deref() != Some("Force Switch") {
        fail!(ui, "third way: {force:?}");
    }
    ui.quick("option:1");
    ui.step("modalok");
    ui.settle();
    let head = git_out(&dir, &["branch", "--show-current"]);
    if head.trim() != "feat" {
        fail!(ui, "Delete & Switch did not switch: {head:?}");
    }
    if std::fs::read_to_string(dir.join("x.txt")).unwrap_or_default() != "theirs\n" {
        ui.fail("x.txt is not feat's version".into());
    }
    if !dir.join("new.txt").exists() {
        ui.fail("an untracked file not in the way was deleted".into());
    }
}

/// A file in a folder offers that folder's own .gitignore / .gitattributes (and every
/// folder above it that has one), the nearest existing one preselected, each target with
/// its own pattern line relative to its folder; OK writes into the chosen file. "Track
/// with Git LFS…" opens after its git-lfs check (or says that git-lfs is missing).
fn folder_rule_files(ui: &mut Ui) {
    let dir = base().join("folder_rule_files");
    std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
    std::fs::write(dir.join("sub/deep/x.log"), "x").unwrap();
    std::fs::write(dir.join("sub/.gitignore"), "").unwrap();
    std::fs::write(dir.join("sub/.gitattributes"), "").unwrap();
    ui.quick("ignore:sub/deep/x.log");
    let want = ["/sub/deep/x.log", "/deep/x.log", "/x.log", "/sub/deep/x.log"];
    match modal(ui) {
        Some((t, f)) if t == "Add to .gitignore" && f == want => {}
        other => fail!(ui, "nested ignore dialog: {other:?}"),
    }
    let ws = ui.ws.clone();
    let option = ui.cx.update(|cx| menus::modal_option(&ws, cx));
    if option != Some(1) {
        fail!(ui, "sub/.gitignore (exists) preselected: {option:?}");
    }
    let probes = ui.frame();
    let shown = |name: &str| find(&probes, name).and_then(Probe::visible).is_some();
    if !shown("modal-field:1") || shown("modal-field:0") || shown("modal-field:2") {
        fail!(ui, "only the chosen target's pattern line is shown");
    }
    ui.quick("option:2");
    ui.quick("field:2=*.log");
    ui.step("modalok");
    let text = std::fs::read_to_string(dir.join("sub/deep/.gitignore")).unwrap_or_default();
    if text != "*.log\n" {
        fail!(ui, "sub/deep/.gitignore is {text:?}");
    }
    if ui.working().iter().any(|l| l.contains("x.log")) {
        fail!(ui, "x.log still listed: {:?}", ui.working());
    }

    ui.quick("lfstrack:sub/deep/x.psd");
    match modal(ui) {
        Some((t, _)) if t == "Git LFS is not installed" => {}
        Some((t, f)) if t == "Track with Git LFS" => {
            if f != ["/sub/deep/x.psd", "/deep/x.psd", "/x.psd"] {
                fail!(ui, "track fields: {f:?}");
            }
            let option = ui.cx.update(|cx| menus::modal_option(&ws, cx));
            if option != Some(1) {
                fail!(ui, "sub/.gitattributes (exists) preselected: {option:?}");
            }
            // The long "Lockable" label wraps inside the dialog.
            let probes = ui.frame();
            let m = find(&probes, "modal").and_then(Probe::visible);
            let c = find(&probes, "modal-check-label:1").and_then(Probe::visible);
            match (m, c) {
                (Some(m), Some(c)) if testing::contains(&m, &c) => {}
                other => fail!(ui, "checkbox row not inside the dialog: {other:?}"),
            }
        }
        other => fail!(ui, "track dialog: {other:?}"),
    }
    // Never OK here: without git-lfs set up it would run `git lfs install` (global).
    ui.step("cancel");
}

/// A bare repository with one commit next to the scenario repositories (two processes).
fn bare_source(name: &str) -> PathBuf {
    let bare = base().join(format!("{name}.git"));
    git(&base(), &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
    let mut s = Stream::new();
    s.commit("refs/heads/main", "init", None, &[("readme.md", Some("hi\n"))]);
    s.import(&bare);
    bare
}

/// The Clone dialog takes a repository location from the clipboard as if it was pasted:
/// a forge's page URL cut back to the repository, selected (typing replaces it), the folder
/// name derived, "From the clipboard" under it until it is changed; a local repository's
/// path too; plain text not. Add Submodule… the same with URLs (`gh repo clone` too), not
/// with a local path.
fn clone_from_clipboard(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let note = |ui: &mut Ui| ui.cx.update(|cx| menus::modal_note(&ws, 0, cx));
    let selection = |ui: &mut Ui| ui.cx.update(|cx| menus::modal_selection(&ws, 0, cx));
    ui.quick("focuslist");
    ui.quick("clip:https://github.com/octo/project/blob/main/README.md");
    ui.quick("action:gitti::CloneRepository");
    let url = "https://github.com/octo/project";
    match modal(ui) {
        Some((t, f)) if t == "Clone Repository" && f[0] == url && f[2] == "project" => {}
        other => fail!(ui, "clone dialog from the clipboard: {other:?}"),
    }
    if note(ui).as_deref() != Some(gitti_core::clone_source::CLIPBOARD_NOTE) {
        let n = note(ui);
        fail!(ui, "no clipboard note under the URL: {n:?}");
    }
    if selection(ui) != Some(0..url.len()) {
        let s = selection(ui);
        fail!(ui, "the prefilled URL is not selected: {s:?}");
    }
    // Typing replaces it; the folder name follows, the note goes.
    ui.type_text("git@example.com:team/other.git");
    match modal(ui) {
        Some((_, f)) if f[0] == "git@example.com:team/other.git" && f[2] == "other" => {}
        other => fail!(ui, "typing over the clipboard URL: {other:?}"),
    }
    if let Some(n) = note(ui) {
        fail!(ui, "clipboard note after typing: {n:?}");
    }
    ui.quick("cancel");
    // A local repository: its path, the folder named after it.
    let repo = ui.workdir();
    ui.quick(&format!("clip:  {}\n", repo.display()));
    ui.quick("action:gitti::CloneRepository");
    let name = repo.file_name().unwrap().to_string_lossy().to_string();
    match modal(ui) {
        Some((_, f)) if f[0] == repo.display().to_string() && f[2] == name => {}
        other => fail!(ui, "clone dialog with a repository path on the clipboard: {other:?}"),
    }
    ui.quick("cancel");
    // Plain text: an empty dialog, no note.
    ui.quick("clip:see https://github.com/octo/project for details");
    ui.quick("action:gitti::CloneRepository");
    match modal(ui) {
        Some((_, f)) if f[0].is_empty() && f[2].is_empty() => {}
        other => fail!(ui, "clone dialog with plain text on the clipboard: {other:?}"),
    }
    if let Some(n) = note(ui) {
        fail!(ui, "clipboard note without a URL: {n:?}");
    }
    ui.quick("cancel");
    // Add Submodule: a URL as in Clone, the path derived; a local repository's path not.
    ui.quick("clip:gh repo clone octo/widgets");
    ui.quick("action:gitti::AddSubmodule");
    let url = "https://github.com/octo/widgets.git";
    match modal(ui) {
        Some((t, f)) if t == "Add Submodule" && f[0] == url && f[1] == "widgets" => {}
        other => fail!(ui, "add submodule from the clipboard: {other:?}"),
    }
    if note(ui).as_deref() != Some(gitti_core::clone_source::CLIPBOARD_NOTE)
        || selection(ui) != Some(0..url.len())
    {
        let (n, s) = (note(ui), selection(ui));
        fail!(ui, "add submodule: clipboard URL without note / selection: {n:?} {s:?}");
    }
    ui.quick("cancel");
    ui.quick(&format!("clip:{}", repo.display()));
    ui.quick("action:gitti::AddSubmodule");
    match modal(ui) {
        Some((t, f)) if t == "Add Submodule" && f[0].is_empty() && f[1].is_empty() => {}
        other => fail!(ui, "add submodule with a local path on the clipboard: {other:?}"),
    }
    ui.quick("cancel");
}

/// Init offers the SHA-256 object format, preset from git's default (SHA-1 from
/// `init.defaultObjectFormat` in the suite's global config), and says what it means; the
/// checkbox ticked creates a SHA-256 repository (`--object-format=sha256`), left alone a
/// SHA-1 one without the option (git's default applies).
fn init_object_format(ui: &mut Ui) {
    ui.quick("action:gitti::InitRepository");
    let ws = ui.ws.clone();
    let message = ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default();
    for words in [
        "runs git init there",
        "more secure hash",
        "GitHub can't host SHA-256 repositories",
        "can't be changed later",
        "Preset to git's default: SHA-1 (init.defaultObjectFormat in your git config).",
    ] {
        if !message.contains(words) {
            fail!(ui, "Init dialog message without {words:?}: {message}");
        }
    }
    if find(&ui.frame(), "modal-check:0").is_none_or(|p| p.visible().is_none()) {
        ui.fail("no SHA-256 checkbox in the Init dialog".into());
    }
    let (new, old) = (base().join("init-sha256"), base().join("init-sha1"));
    ui.quick(&format!("field:0={}", new.display()));
    ui.quick("checkbox:0");
    ui.step("modalok");
    // Back to the first tab (the new repository opened in its own): both inits there.
    ui.step("tab:0");
    ui.quick("action:gitti::InitRepository");
    ui.quick(&format!("field:0={}", old.display()));
    ui.step("modalok");
    for (dir, want) in [(&new, "sha256"), (&old, "sha1")] {
        let have = git_out(dir, &["rev-parse", "--show-object-format"]);
        if have != want {
            fail!(ui, "{}: object format {have:?}, wanted {want}: {:?}", dir.display(), status(ui));
        }
    }
    ui.step("tab:0");
    let inits: Vec<String> = ui
        .git_commands()
        .into_iter()
        .filter(|c| c.split(' ').next() == Some("init"))
        .collect();
    let flags: Vec<bool> = inits.iter().map(|c| c.contains("--object-format=sha256")).collect();
    if flags != [true, false] || inits.iter().any(|c| c.contains("--object-format=sha1")) {
        fail!(ui, "git init commands: {inits:?}");
    }
}

/// Clone (URL → derived folder name that follows the URL until one is typed; parent) and
/// Init open the result in a new tab, the clone in the command log of the tab it was
/// started from; Init turns a folder that is no repository into one; a bare repository is
/// said as such; with no tab open the start page hosts the dialogs.
fn clone_and_init(ui: &mut Ui) {
    let bare = bare_source("upstream");
    let parent = base().join("clones");
    std::fs::create_dir_all(&parent).unwrap();
    ui.quick("focuslist");
    ui.quick("action:gitti::CloneRepository");
    ui.quick(&format!("field:0={}", bare.display()));
    match modal(ui) {
        Some((t, f)) if t == "Clone Repository" && f.len() == 3 && f[2] == "upstream" => {}
        other => fail!(ui, "clone dialog after typing the URL: {other:?}"),
    }
    ui.quick("field:0=git@example.com:team/other.git");
    if modal(ui).map(|m| m.1[2].clone()).as_deref() != Some("other") {
        let now = modal(ui);
        fail!(ui, "folder name does not follow the URL: {now:?}");
    }
    ui.quick("field:2=mine");
    ui.quick(&format!("field:0={}/", bare.display()));
    if modal(ui).map(|m| m.1[2].clone()).as_deref() != Some("mine") {
        let now = modal(ui);
        fail!(ui, "derived name overwrote the typed one: {now:?}");
    }
    ui.quick(&format!("field:1={}", parent.display()));
    ui.step("modalok");
    let clone = parent.join("mine");
    if !clone.join("readme.md").exists() {
        fail!(ui, "no clone at {}: {:?}", clone.display(), status(ui));
    }
    if ui.tabs() != (2, 1) || !has_repo(ui) || ui.workdir() != gitti_core::paths::canonical(&clone) {
        let (t, w) = (ui.tabs(), ui.workdir());
        fail!(ui, "clone not opened in a new tab: tabs {t:?}, {}", w.display());
    }
    ui.step("tab:0");
    if !ui.git_commands().iter().any(|c| c.contains("clone --progress")) {
        ui.fail("clone not in the command log of the tab it was started from".into());
    }
    ui.quick("action:gitti::InitRepository");
    let fresh = base().join("fresh");
    ui.quick(&format!("field:0={}", fresh.display()));
    ui.step("modalok");
    if !fresh.join(".git").exists() || ui.tabs() != (3, 2) {
        let t = ui.tabs();
        fail!(ui, "init: tabs {t:?}, status {:?}", status(ui));
    }
    let plain = base().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    ui.step(&format!("open:{}", plain.display()));
    if ui.tabs() != (4, 3) || has_repo(ui) {
        let t = ui.tabs();
        fail!(ui, "plain folder: tabs {t:?}");
    }
    ui.quick("action:gitti::InitRepository");
    let want = gitti_core::paths::canonical(&plain).display().to_string();
    if modal(ui).map(|m| m.1) != Some(vec![want]) {
        let now = modal(ui);
        fail!(ui, "init dialog of a plain folder: {now:?}");
    }
    ui.step("modalok");
    if ui.tabs() != (4, 3) || !has_repo(ui) {
        let t = ui.tabs();
        fail!(ui, "plain folder after init: tabs {t:?}, repo {}", has_repo(ui));
    }
    ui.step(&format!("open:{}", bare.display()));
    if ui.tabs() != (5, 4) || has_repo(ui) {
        let t = ui.tabs();
        fail!(ui, "bare repository: tabs {t:?}");
    }
    let ws = ui.ws.clone();
    match ui.cx.update(|cx| menus::open_problem(&ws, cx)) {
        Some((t, m)) if t == "Bare repository" && m.contains("worktree add") => {}
        other => fail!(ui, "bare repository page: {other:?}"),
    }
    ui.quick("action:gitti::InitRepository");
    let bare_dir = gitti_core::paths::canonical(&bare).display().to_string();
    if modal(ui).is_none_or(|m| m.1 == vec![bare_dir]) {
        let now = modal(ui);
        fail!(ui, "init dialog of a bare repository: {now:?}");
    }
    ui.quick("key:escape");
    for _ in 0..5 {
        ui.quick("key:secondary-w");
    }
    if ui.tabs().0 != 0 {
        let t = ui.tabs();
        fail!(ui, "tabs left: {t:?}");
    }
    if !find(&ui.frame(), "start-page").is_some_and(|p| p.visible().is_some()) {
        ui.fail("no start page without tabs".into());
    }
    ui.quick("action:gitti::CloneRepository");
    ui.quick(&format!("field:0={}", bare.display()));
    ui.quick(&format!("field:1={}", parent.display()));
    ui.step("modalok");
    if ui.tabs() != (1, 0) || !has_repo(ui) || !parent.join("upstream").join(".git").exists() {
        let t = ui.tabs();
        fail!(ui, "clone from the start page: tabs {t:?}, status {:?}", status(ui));
    }
}

/// Go to Commit searches the loaded history by subject, author or hash prefix; a file name
/// finds the commits that changed it (tagged with the path, a note says what was searched)
/// and shows that file — also for a commit not loaded yet; nothing found keeps the picker
/// with a note.
fn go_to_commit(ui: &mut Ui) {
    let ids = ids_of(ui);
    ui.quick("focuslist");
    ui.quick("action:gitti::GoToCommit");
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
    if selected(ui).as_deref() != Some(ids[3].as_str()) || state(ui).picker.is_some() {
        let s = selected(ui);
        fail!(ui, "Go to Commit selected {s:?}, expected {}", ids[3]);
    }
    ui.quick("action:gitti::GoToCommit");
    ui.step("pick:layout test");
    if picker_labels(ui).map(|l| l.len()) != Some(5) {
        let l = picker_labels(ui);
        fail!(ui, "author search: {l:?}");
    }
    ui.step(&format!("pick:{}", &ids[1][..8]));
    match picker_labels(ui) {
        Some(l) if l.first().map(String::as_str) == Some("Initial import") => {}
        other => fail!(ui, "hash search: {other:?}"),
    }
    ui.step("pickok");
    if selected(ui).as_deref() != Some(ids[1].as_str()) {
        let s = selected(ui);
        fail!(ui, "hash search selected {s:?}");
    }
    ui.quick("action:gitti::GoToCommit");
    ui.step("pick:F2.txt");
    match &state(ui).picker {
        Some(p)
            if p.labels == ["Rework the frobnicator", "Add f2"]
                && p.tags.iter().all(|t| t.as_deref() == Some("f2.txt"))
                && !p.searching
                && p.note.as_deref()
                    == Some("No commit message or author contains 'f2.txt' — showing commits that changed f2.txt") => {}
        other => fail!(ui, "file search `F2.txt`: {other:?}"),
    }
    ui.step("pickok");
    let files = |ui: &mut Ui| state(ui).rows.into_iter().filter(|r| r.starts_with("file ")).collect::<Vec<_>>();
    let f = files(ui);
    if selected(ui).as_deref() != Some(ids[5].as_str()) || f.len() != 1 || !f[0].contains("f2.txt") {
        let s = selected(ui);
        fail!(ui, "file hit selected {s:?}, files {f:?}");
    }
    ui.quick("action:gitti::GoToCommit");
    ui.step("pick:f2.txt");
    ui.quick("key:down");
    ui.step("pickok");
    let f = files(ui);
    if selected(ui).as_deref() != Some(ids[4].as_str()) || f.len() != 1 || !f[0].contains("f2.txt") {
        let s = selected(ui);
        fail!(ui, "second file hit: selected {s:?} (expected {}), files {f:?}", ids[4]);
    }
    ui.quick("action:gitti::GoToCommit");
    ui.step("pick:no such commit anywhere");
    match &state(ui).picker {
        Some(p) if p.labels.is_empty() && p.note.as_deref().is_some_and(|n| n.contains("nothing matches 'anywhere'")) => {}
        other => fail!(ui, "no-match search: {other:?}"),
    }
    ui.check("go to commit: nothing found");
    ui.quick("cancel");
}

/// The fixture's commit ids (1 = "Initial import" … 5 = HEAD).
fn changes(ui: &mut Ui) -> testing::ChangeSearchState {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::change_search(&ws, cx))
}

/// Search in Changes — typing runs one scan per query (debounced), not one per keystroke;
/// the hits are the commits whose changed lines have all the words, the best match first,
/// each with why; selecting one limits the diff to the matching files / hunks ("All hunks"
/// shows the rest); strict needs the word inside the changed part.
fn search_in_changes(ui: &mut Ui) {
    let ids = ids_of(ui);
    ui.quick("focuslist");
    ui.quick("action:gitti::SearchChanges");
    if !changes(ui).open || state(ui).focus == "list" {
        let c = changes(ui);
        fail!(ui, "Search in Changes did not open with the focus in its field: {c:?}");
    }
    let before = ui.git_commands().len();
    for text in ["l", "li", "lin", "line", "line 2", "line 2 v"] {
        ui.quick(&format!("changes:{text}"));
    }
    if changes(ui).jobs != 0 {
        let c = changes(ui);
        fail!(ui, "a scan started while typing: {c:?}");
    }
    ui.settle();
    let c = changes(ui);
    let scans = ui.git_commands()[before..]
        .iter()
        .filter(|c| c.contains("diff-tree --stdin"))
        .count();
    let mut hits = c.hits.clone();
    hits.sort();
    if c.jobs != 1 || scans != 1 || hits != ["Add f0", "Add f1", "Add f2", "Rework the frobnicator"] {
        fail!(ui, "one scan for the typed query: {scans} diff-trees, {c:?}");
    }
    if c.hits[0] != "Rework the frobnicator" || c.reasons[0] != "all 3 words in one line" {
        fail!(ui, "best match first, with why: {c:?}");
    }
    let files = |ui: &mut Ui| {
        state(ui)
            .rows
            .into_iter()
            .filter(|r| r.starts_with("file "))
            .collect::<Vec<_>>()
    };
    ui.quick("changes:line 7 w1");
    ui.settle();
    ui.step("changehit:0");
    let f = files(ui);
    if selected(ui).as_deref() != Some(ids[5].as_str()) || f.len() != 1 || !f[0].ends_with("f1.txt") {
        let s = selected(ui);
        let c = changes(ui);
        fail!(ui, "hit 0 selected {s:?} (ids {ids:?}), files {f:?} (only f1.txt has `line 7 w1`): {c:?}");
    }
    // The matching hunk only: f1.txt has one hunk anyway; its lines are there.
    if !state(ui).rows.iter().any(|r| r.starts_with("line ") && r.contains("f1.txt")) {
        fail!(ui, "no lines of f1.txt shown");
    }
    ui.step("changetoggle:all");
    let f = files(ui);
    if f.len() != 3 {
        fail!(ui, "All hunks: files {f:?}");
    }
    ui.step("changetoggle:all");
    // Strict: `line 7` did not change in the rework (only v → w did); the commits adding
    // the files (and big.txt) added it.
    ui.quick("changes:line 7");
    ui.step("changetoggle:strict");
    let c = changes(ui);
    let mut hits = c.hits.clone();
    hits.sort();
    if !c.strict || hits != ["Add f0", "Add f1", "Add f2", "Initial import"] {
        fail!(ui, "strict `line 7`: {c:?}");
    }
    if !find(&ui.frame(), "change-search-note").is_some_and(|p| p.visible().is_some()) {
        ui.fail("no note line in the search bar".into());
    }
    if !find(&ui.frame(), "change-experimental").is_some_and(|p| p.visible().is_some()) {
        ui.fail("the search bar doesn't say it is experimental".into());
    }
}

fn ids_of(_: &mut Ui) -> Vec<String> {
    IDS.with(|i| i.borrow().clone())
}

/// Rename Current Branch, Delete Branch / Rebase onto / Set Upstream pickers; with the
/// worktree row the Commit menu explains instead of acting (no git); Edit Commit Message on
/// a commit; Stash Including Untracked takes the untracked file too, Drop Stash lists it;
/// Toggle Sidebar from the menu.
fn branch_commit_stash_menus(ui: &mut Ui) {
    let dir = base().join("branch_commit_stash_menus");
    ui.quick("focuslist");
    ui.quick("action:gitti::RenameCurrentBranch");
    match modal(ui) {
        Some((t, f)) if t == "Rename branch main" && f == ["main"] => {}
        other => fail!(ui, "Rename Current Branch: {other:?}"),
    }
    ui.quick("field:0=trunk");
    ui.step("modalok");
    if state(ui).branch.as_deref() != Some("trunk") {
        let b = state(ui).branch;
        fail!(ui, "branch after rename: {b:?}");
    }
    ui.quick("action:gitti::DeleteBranchMenu");
    if picker_labels(ui) != Some(vec!["older".to_string()]) {
        let l = picker_labels(ui);
        fail!(ui, "Delete Branch picker: {l:?}");
    }
    ui.quick("cancel");
    ui.quick("action:gitti::RebaseCurrentOnto");
    match picker_labels(ui) {
        Some(l) if l.contains(&"older".into()) && !l.contains(&"trunk".into()) => {}
        other => fail!(ui, "Rebase onto picker: {other:?}"),
    }
    ui.quick("cancel");
    ui.quick("action:gitti::SetUpstreamMenu");
    if picker_labels(ui).is_none() {
        ui.fail("Set Upstream opened no picker".into());
    }
    ui.quick("cancel");
    ui.step("select:0");
    let n = ui.n_git();
    ui.quick("action:gitti::RevertCommit");
    match status(ui) {
        // The core's reason and next step.
        Some((s, false)) if s.contains("select a commit first") => {}
        other => fail!(ui, "Revert on the worktree row: {other:?}"),
    }
    ui.no_git_since(n, "Revert on the worktree row");
    ui.quick("action:gitti::EditCommitMessage");
    if modal_title(ui).is_some() {
        ui.fail("Edit Commit Message opened a dialog for the worktree row".into());
    }
    ui.step("select:1");
    ui.quick("action:gitti::EditCommitMessage");
    match modal_title(ui) {
        Some(t) if t.starts_with("Edit message of") => {}
        other => fail!(ui, "Edit Commit Message: {other:?}, status {:?}", status(ui)),
    }
    ui.quick("cancel");
    ui.step("action:gitti::StashIncludingUntracked");
    let stashes = state(ui).stashes;
    if dir.join("new.txt").exists() || stashes != 1 {
        fail!(ui, "Stash Including Untracked: stashes {stashes}, status {:?}", status(ui));
    }
    ui.quick("action:gitti::DropStash");
    match picker_labels(ui) {
        Some(l) if l.len() == 1 => {}
        other => fail!(ui, "Drop Stash picker: {other:?}"),
    }
    ui.quick("cancel");
    ui.quick("action:gitti::ToggleSidebar");
    if find(&ui.frame(), "sidebar").is_some() {
        ui.fail("sidebar still drawn after Toggle Sidebar".into());
    }
    ui.quick("action:gitti::ToggleSidebar");
    if find(&ui.frame(), "sidebar").is_none() {
        ui.fail("sidebar not back after the second Toggle Sidebar".into());
    }
}

/// The stash dialogs: Stash Changes… says what it does and refuses `--staged` with `-u`;
/// "Stash Selected Lines or File…" stashes two selected lines of big.txt (the rest of the
/// file stays), then the file at the cursor; "Rename Stash…" renames the first one.
fn stash_dialogs(ui: &mut Ui) {
    let dir = base().join("stash_dialogs");
    let ws = ui.ws.clone();
    let live = |ui: &mut Ui| ui.cx.update(|cx| menus::modal_live(&ws, cx)).unwrap_or_default();
    ui.step("select:0");
    ui.quick("action:gitti::StashChanges");
    match live(ui) {
        (text, Some(ok), None, _) if ok == "Stash" && text.contains("1 untracked file stays") => {}
        other => fail!(ui, "Stash Changes… says: {other:?}"),
    }
    // -u with --staged: OK is off, with the reason.
    ui.quick("check:0");
    ui.quick("check:3");
    match live(ui) {
        (_, _, Some(why), _) if why.contains("--staged") => {}
        other => fail!(ui, "--staged with -u: {other:?}"),
    }
    ui.quick("cancel");
    let s = ui.ui_state();
    let Some(line) = s.rows.iter().position(|r| r == "line Unstaged big.txt Removed") else {
        fail!(ui, "no removed line in big.txt: {:?}", s.rows);
        return;
    };
    ui.quick(&format!("lines:{line}-{}", line + 1));
    ui.quick("action:gitti::StashSelection");
    match modal(ui) {
        Some((t, _)) if t == "Stash 2 Lines of big.txt" => {}
        other => fail!(ui, "Stash Selected Lines: {other:?}"),
    }
    ui.quick("field:0=two lines");
    ui.step("modalok");
    let big = std::fs::read_to_string(dir.join("big.txt")).unwrap_or_default();
    if big.contains("line 3 changed") || !big.contains("line 28 changed") {
        fail!(ui, "big.txt after stashing its first change: {:?}, status {:?}", &big[..80.min(big.len())], status(ui));
    }
    if git_out(&dir, &["stash", "list", "--format=%gs"]) != "On main: two lines" {
        fail!(ui, "stash list: {}", git_out(&dir, &["stash", "list"]));
    }
    // No lines selected: the file at the diff cursor.
    ui.quick("action:gitti::StashSelection");
    match modal(ui) {
        Some((t, _)) if t.starts_with("Stash ") && !t.contains("Lines") => {}
        other => fail!(ui, "Stash File: {other:?}"),
    }
    ui.step("modalok");
    if state(ui).stashes != 2 {
        fail!(ui, "second stash: {:?}", status(ui));
    }
    ui.quick("action:gitti::RenameStash");
    match picker_labels(ui) {
        Some(l) if l.len() == 2 => {}
        other => fail!(ui, "Rename Stash picker: {other:?}"),
    }
    ui.step("pick:two lines");
    ui.step("pickok");
    match modal(ui) {
        Some((t, f)) if t == "Rename stash@{1}" && f == ["two lines"] => {}
        other => fail!(ui, "Rename dialog: {other:?}"),
    }
    ui.quick("field:0=renamed");
    ui.step("modalok");
    let list = git_out(&dir, &["stash", "list", "--format=%gs"]);
    if list.lines().nth(1) != Some("On main: renamed") {
        fail!(ui, "after the rename: {list}");
    }
}

/// Open Recent lists the repositories opened before (not the current one, newest first);
/// choosing one switches to it and counts as opening it.
fn open_recent(ui: &mut Ui) {
    let other = base().join("recent-other");
    init_repo(&other);
    ui.step(&format!("open:{}", other.display()));
    if ui.tabs() != (2, 1) {
        let t = ui.tabs();
        fail!(ui, "second repository not opened: {t:?}");
    }
    ui.quick("action:gitti::OpenRecent");
    match picker_labels(ui) {
        Some(l) if l.first().map(String::as_str) == Some("open_recent") => {}
        other => fail!(ui, "Open Recent (newest first, current left out): {other:?}"),
    }
    ui.step("pickok");
    if ui.tabs() != (2, 0) {
        let t = ui.tabs();
        fail!(ui, "Open Recent did not switch to the first tab: {t:?}");
    }
    let recent = gitti_core::store::load_recent_repos();
    let want = [
        gitti_core::paths::canonical(&base().join("open_recent")),
        gitti_core::paths::canonical(&other),
    ];
    if recent.get(..2) != Some(&want[..]) {
        fail!(ui, "recent list: {recent:?}, expected {want:?} first");
    }
}

/// "Open This Version" writes the commit's file to a read-only copy named after the commit
/// and opens it in the editor; "Open Old Version" the parent's; the working directory has
/// no commit version: a message, no editor.
fn open_this_version(ui: &mut Ui) {
    let ids = ids_of(ui);
    let versions = base().join("versions").join("open_this_version");
    ui.step("select:1");
    let path_of = |line: &str| PathBuf::from(line.rsplit_once(' ').map_or(line, |(p, _)| p));
    let numbered = |tag: &str| -> String { (0..30).map(|i| format!("line {i} {tag}\n")).collect() };
    let before = editor_lines();
    ui.quick("action:gitti::OpenThisVersion");
    let Some(line) = wait_editor_line(before) else {
        fail!(ui, "no editor started: {:?}", status(ui));
        return;
    };
    let path = path_of(&line);
    let want = versions.join(&ids[5][..10]).join("f0.txt");
    if path != want {
        fail!(ui, "copy at {}, expected {}", path.display(), want.display());
    }
    if std::fs::read_to_string(&path).unwrap_or_default() != numbered("w0") {
        ui.fail("copy content is not HEAD's f0.txt".into());
    }
    if !std::fs::metadata(&path).is_ok_and(|m| m.permissions().readonly()) {
        ui.fail("copy is not read-only".into());
    }
    match status(ui) {
        Some((s, false)) if s.contains("read-only copy of f0.txt") => {}
        other => fail!(ui, "status after Open This Version: {other:?}"),
    }
    let before = editor_lines();
    ui.quick("action:gitti::OpenOldVersion");
    let path = path_of(&wait_editor_line(before).unwrap_or_default());
    if path != versions.join(&ids[4][..10]).join("f0.txt") || std::fs::read_to_string(&path).unwrap_or_default() != numbered("v0") {
        fail!(ui, "old version copy: {}", path.display());
    }
    ui.step("select:0");
    let before = editor_lines();
    ui.quick("action:gitti::OpenThisVersion");
    match status(ui) {
        Some((s, false)) if s.contains("Open in Editor") => {}
        other => fail!(ui, "Open This Version in the working directory: {other:?}"),
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    if editor_lines() != before {
        ui.fail("an editor was started for the working directory".into());
    }
}

/// Commit ▸ "Restore File or Folder from Selected Commit…": the path and the version
/// before the commit, then a dialog saying what it writes; over a local change OK waits
/// for the check box. Commit ▸ "Branches and Tags Containing Selected Commit…" lists
/// them from the loaded history without git.
fn restore_and_containing(ui: &mut Ui) {
    let ids = ids_of(ui);
    let dir = ui.workdir();
    let numbered = |tag: &str| -> String { (0..30).map(|i| format!("line {i} {tag}\n")).collect() };
    let live = |ui: &mut Ui| {
        let ws = ui.ws.clone();
        ui.cx.update(|cx| menus::modal_live(&ws, cx))
    };
    ui.step("select:1");
    // The commit's menu and its file's menu in the diff offer them.
    testing::menu_labels();
    ui.click_on("commit-row:1", MouseButton::Right);
    let labels = testing::menu_labels();
    for want in [gitti_core::containing::MENU_LABEL, gitti_core::restore::PICK_LABEL] {
        if !labels.iter().any(|l| l == want) {
            fail!(ui, "commit menu lacks {want}: {labels:?}");
        }
    }
    ui.quick("key:escape");
    let file_row = ui
        .frame()
        .into_iter()
        .find(|p| p.name.starts_with("diff-row:") && p.name.ends_with(":file") && p.visible().is_some())
        .map(|p| p.name);
    if let Some(row) = file_row {
        testing::menu_labels();
        ui.click_on(&row, MouseButton::Right);
        let labels = testing::menu_labels();
        for want in [gitti_core::restore::THIS_LABEL, gitti_core::restore::BEFORE_LABEL] {
            if !labels.iter().any(|l| l == want) {
                fail!(ui, "file menu lacks {want}: {labels:?}");
            }
        }
        ui.quick("key:escape");
    } else {
        ui.fail("no file row of the commit shown".into());
    }
    ui.quick("action:gitti::RestoreFromCommit");
    match modal_title(ui) {
        Some(t) if t == format!("Restore from {}", &ids[5][..7]) => {}
        other => fail!(ui, "path dialog: {other:?} {:?}", status(ui)),
    }
    ui.quick("field:0=f0.txt");
    ui.quick("option:1");
    ui.step("modalok");
    match modal_title(ui) {
        Some(t) if t == format!("Restore f0.txt from {}", &ids[4][..7]) => {}
        other => fail!(ui, "restore dialog: {other:?} {:?}", status(ui)),
    }
    match live(ui) {
        Some((text, _, None, _)) if text.contains("No uncommitted changes are overwritten") => {}
        other => fail!(ui, "restore dialog says: {other:?}"),
    }
    ui.step("modalok");
    if std::fs::read_to_string(dir.join("f0.txt")).unwrap_or_default() != numbered("v0") {
        ui.fail("f0.txt is not the version before the commit".into());
    }
    match status(ui) {
        Some((s, false)) if s.starts_with("Restored f0.txt from") => {}
        other => fail!(ui, "status after the restore: {other:?}"),
    }
    // Back to the commit's version over that (now uncommitted) change: OK waits for the
    // check box; with the index too.
    ui.quick("action:gitti::RestoreFromCommit");
    ui.quick("field:0=f0.txt");
    ui.step("modalok");
    match live(ui) {
        Some((text, _, Some(_), _)) if text.contains("  f0.txt (modified)") => {}
        other => fail!(ui, "lost change not asked for: {other:?}"),
    }
    ui.quick("check:0");
    ui.quick("option:1");
    match live(ui) {
        Some((_, Some(ok), None, _)) if ok == "Restore and Stage" => {}
        other => fail!(ui, "after ticking the box: {other:?}"),
    }
    ui.step("modalok");
    if std::fs::read_to_string(dir.join("f0.txt")).unwrap_or_default() != numbered("w0") {
        ui.fail("f0.txt is not the commit's version".into());
    }

    let n = ui.n_git();
    ui.step("action:gitti::ShowContaining");
    ui.no_git_since(n, "Branches and Tags Containing");
    let ws = ui.ws.clone();
    let text = ui.cx.update(|cx| testing::modal_viewer_text(&ws, cx));
    match (modal_title(ui), text) {
        (Some(t), Some(text)) if t.starts_with("Branches and tags containing") && text.contains("  main (checked out)") => {}
        other => fail!(ui, "containing window: {other:?}"),
    }
    ui.quick("modalok");
}

/// Merging a branch with its own history: git refuses, the error offers "Merge anyway
/// (allow unrelated histories)", which merges; the dialog's option merges directly and is
/// remembered for the next merge.
fn merge_unrelated_histories(ui: &mut Ui) {
    let dir = ui.workdir();
    let parents = || git_out(&dir, &["rev-list", "--parents", "-n1", "HEAD"]).split_whitespace().count();
    let merge_dialog = |ui: &mut Ui| {
        ui.quick("action:gitti::MergeBranch");
        ui.quick("pick:other");
        ui.quick("pickok");
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
    ui.step("modalok");
    if parents() != 3 || !dir.join("o.txt").exists() {
        fail!(ui, "Merge anyway did not merge: {:?}", status(ui));
    }
    // The option in the dialog (5th checkbox) merges at once and stays checked.
    git(&dir, &["reset", "-q", "--keep", "HEAD~1"]);
    ui.step("action:gitti::Refresh");
    merge_dialog(ui);
    ui.quick("checkbox:4");
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

/// `d` on `main` with a commit of its own ("Mine"); a bare `origin` (its upstream, sharing
/// the objects) has another one on the same base ("Theirs"). A clean worktree. Three git
/// processes.
fn diverged_repo(d: &Path) {
    init_repo(d);
    let mut s = Stream::new();
    let one = s.commit("refs/heads/main", "Base", None, &[("a.txt", Some("a\n"))]);
    let theirs = s.commit("refs/fixture/theirs", "Theirs", Some(one), &[("t.txt", Some("t\n"))]);
    s.reset("refs/fixture/theirs", None);
    let mine = s.commit("refs/heads/main", "Mine", Some(one), &[("m.txt", Some("m\n"))]);
    s.reset("refs/remotes/origin/main", Some(one));
    let ids = s.import(d);
    read_tree(d, &ids[mine]);
    let name = format!("{}-origin.git", d.file_name().unwrap().to_string_lossy());
    let origin = base().join(&name);
    git(&base(), &["init", "-q", "--bare", "-b", "main", &name]);
    std::fs::create_dir_all(origin.join("objects/info")).unwrap();
    let objects = d.join(".git/objects");
    std::fs::write(origin.join("objects/info/alternates"), format!("{}\n", objects.display())).unwrap();
    std::fs::write(origin.join("refs/heads/main"), format!("{}\n", ids[theirs])).unwrap();
    append_config(
        d,
        &format!(
            "[remote \"origin\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
             [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n",
            origin.display().to_string().replace('\\', "/")
        ),
    );
}

/// Pull on a branch that diverged, no pull strategy configured: git refuses, the
/// dialog offers Merge / Rebase / Fast-forward only with "Remember for this repository";
/// Merge + Remember merges what the refused pull fetched (no second fetch) and is
/// stored in gitti's settings (not the git config). "Pull with…" offers to forget
/// it, then pulls with the strategy picked for this pull.
fn pull_diverged(ui: &mut Ui) {
    use gitti_core::pull::{PullStrategy, saved_strategy};
    let dir = ui.workdir();
    // A global / system pull.rebase or pull.ff decides; git never asks then.
    let configured = git_out(&dir, &["config", "--get-regexp", r"^pull\.(rebase|ff)$"]);
    if !configured.is_empty() {
        println!("    pull_diverged: skipped, the git config says how to pull ({configured})");
        return;
    }
    let parents = || git_out(&dir, &["rev-list", "--parents", "-n1", "HEAD"]).split_whitespace().count() - 1;
    ui.step("action:gitti::Pull");
    match modal(ui) {
        Some((t, _)) if t == "Pull — branches have diverged" => {}
        other => fail!(ui, "no strategy dialog after git refused: {other:?} {:?}", status(ui)),
    }
    match status(ui) {
        Some((s, false)) if s.contains("main and origin/main have diverged") => {}
        other => fail!(ui, "status while asking: {other:?}"),
    }
    if parents() != 1 {
        ui.fail("the refused pull changed HEAD".into());
    }
    ui.quick("option:0");
    ui.quick("checkbox:0");
    let pulls = |ui: &mut Ui| ui.git_commands().iter().filter(|c| c.starts_with("pull")).count();
    let before = pulls(ui);
    ui.step("modalok");
    if modal(ui).is_some() || parents() != 2 || !dir.join("t.txt").exists() {
        fail!(ui, "Merge did not merge: {:?} {:?}", modal(ui), status(ui));
    }
    if pulls(ui) != before {
        let cmds = ui.git_commands();
        ui.fail(format!("the choice fetched again: {cmds:?}"));
    }
    if saved_strategy(&dir) != Some(PullStrategy::Merge) {
        fail!(ui, "not remembered: {:?}", saved_strategy(&dir));
    }
    if !git_out(&dir, &["config", "--get-regexp", r"^pull\."]).is_empty() {
        ui.fail("the choice went into the git config".into());
    }
    // Pull with…: forget the remembered strategy, then pull once with --rebase.
    ui.step("action:gitti::PullWith");
    let forget = "Forget the remembered strategy (Merge)".to_string();
    match picker_labels(ui) {
        Some(l) if l.contains(&forget) && l.contains(&"Rebase".to_string()) => {}
        other => fail!(ui, "Pull with…: {other:?}"),
    }
    ui.step("pick:forget");
    ui.step("pickok");
    if saved_strategy(&dir).is_some() {
        fail!(ui, "not forgotten: {:?}", saved_strategy(&dir));
    }
    ui.step("action:gitti::PullWith");
    if picker_labels(ui).is_some_and(|l| l.iter().any(|x| x.starts_with("Forget"))) {
        ui.fail("Forget offered with nothing remembered".into());
    }
    ui.step("pick:rebase");
    ui.step("pickok");
    let cmds = ui.git_commands();
    if !cmds.iter().any(|c| c == "pull --rebase") {
        let st = status(ui);
        ui.fail(format!("Pull with… Rebase: {cmds:?} {st:?}"));
    }
}

/// A push origin refuses (someone pushed, not fetched): the dialog says so in plain words
/// and offers Pull (merge / rebase) or Fetch, Then Decide…; that one fetches and lists
/// what origin/main loses (their commit, its author), then pushes with the lease pinned
/// to it. After an amend, Push…'s Options with "Force with lease" lists what origin/main
/// loses before it pushes. Remote ▸ Reset Remote Branch to Selected Commit… moves origin's
/// main back to Base (the upstream first in its picker); the local branch stays.
fn push_refused_ways_out(ui: &mut Ui) {
    let dir = ui.workdir();
    let rev = |r: &str| git_out(&dir, &["rev-parse", r]).trim().to_string();
    let (mine, base) = (rev("main"), rev("main~1"));
    ui.step("action:gitti::Push");
    match modal(ui) {
        Some((t, _)) if t == "origin/main has commits main doesn't" => {}
        other => fail!(ui, "no ways-out dialog after the refusal: {other:?} {:?}", status(ui)),
    }
    match status(ui) {
        Some((s, false)) if s.contains("pull them, or Push with Lease") => {}
        other => fail!(ui, "status while asking: {other:?}"),
    }
    let ws = ui.ws.clone();
    let message = ui.cx.update(|cx| gitti_app::testing::modal_message(&ws, cx)).unwrap_or_default();
    for want in ["git only lets a push add commits", "Pull (merge):", "Fetch origin, Then Decide…:"] {
        if !message.contains(want) {
            fail!(ui, "dialog text lacks {want:?}: {message}");
        }
    }
    // Fetch, Then Decide.
    ui.quick("option:2");
    ui.step("modalok");
    match modal(ui) {
        Some((t, _)) if t == "Overwrite origin/main with main?" => {}
        other => fail!(ui, "no overwrite confirmation after the fetch: {other:?} {:?}", status(ui)),
    }
    let message = ui.cx.update(|cx| gitti_app::testing::modal_message(&ws, cx)).unwrap_or_default();
    if !message.contains("origin/main loses 1 commit") || !message.contains("Theirs") {
        fail!(ui, "what origin/main loses: {message}");
    }
    let theirs = rev("origin/main");
    ui.step("modalok");
    let origin = dir.with_file_name("push_refused_ways_out-origin.git");
    let on_origin = || git_out(&origin, &["rev-parse", "main"]).trim().to_string();
    if on_origin() != mine {
        fail!(ui, "not pushed: {:?} {:?}", ui.git_commands(), status(ui));
    }
    let lease = format!("push --force-with-lease=refs/heads/main:{theirs} origin refs/heads/main:refs/heads/main");
    if !ui.git_commands().contains(&lease) {
        fail!(ui, "lease not pinned to what was listed: {:?}", ui.git_commands());
    }

    // Amended after the push: Push…'s Options with "Force with lease" confirms with the
    // same list first (checks: origin, set upstream, force, …), then pushes.
    git(&dir, &["commit", "--amend", "-qm", "Mine, amended"]);
    ui.step("key:f5");
    ui.step("action:gitti::PushTo");
    ui.quick("pick:options");
    ui.step("pickok");
    ui.quick("checkbox:2");
    ui.step("modalok");
    match modal(ui) {
        Some((t, _)) if t == "Overwrite origin/main with main?" => {}
        other => fail!(ui, "no overwrite confirmation for Force with lease: {other:?} {:?}", status(ui)),
    }
    let message = ui.cx.update(|cx| gitti_app::testing::modal_message(&ws, cx)).unwrap_or_default();
    if !message.contains("origin/main loses 1 commit") || !message.contains("amended or rebased") {
        fail!(ui, "what Force with lease loses: {message}");
    }
    ui.step("modalok");
    let mine = rev("main");
    if on_origin() != mine {
        fail!(ui, "not force-pushed: {:?} {:?}", ui.git_commands(), status(ui));
    }

    // Reset origin/main to Base from the menu bar.
    ui.step(&format!("select:{base}"));
    ui.step("action:gitti::ResetRemoteBranch");
    let want = format!("Reset origin/main to {}…", &base[..7]);
    match picker_labels(ui) {
        Some(l) if l.first() == Some(&want) => {}
        other => fail!(ui, "remote branches: {other:?} {:?}", status(ui)),
    }
    ui.step("pickok");
    match modal(ui) {
        Some((t, _)) if t == format!("Reset origin/main to {}?", &base[..7]) => {}
        other => fail!(ui, "no reset confirmation: {other:?} {:?}", status(ui)),
    }
    ui.step("modalok");
    if on_origin() != base || rev("main") != mine {
        fail!(ui, "reset: origin {} local {}: {:?}", on_origin(), rev("main"), status(ui));
    }
}

/// Every sidebar menu (rows and section headers) has the entries of
/// `gitti_core::sidebar_menus` (the table gitty's menus are checked against); "Create
/// Branch from Stash…" works.
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
        ui.quick("key:escape");
    }
    ui.click_on("side:stash-0", MouseButton::Right);
    for _ in 0..3 {
        ui.quick("key:down");
    }
    ui.quick("key:enter");
    match modal_title(ui) {
        Some(t) if t == "Create branch from stash@{0}" => {}
        other => fail!(ui, "Create Branch from Stash…: {other:?}"),
    }
    ui.quick("field:0=from-stash");
    ui.step("modalok");
    let st = state(ui);
    if st.branch.as_deref() != Some("from-stash") || st.stashes != 0 {
        fail!(ui, "after Create Branch from Stash: branch {:?}, stashes {}", st.branch, st.stashes);
    }
}

/// The menu bar's entry for `action` (`gitti::PopStash`) as shown now: (label, reason).
fn live_entry(action: &str) -> Option<(String, Option<String>)> {
    menus::live_menu_items()
        .into_iter()
        .find(|(_, _, a, _)| a == action)
        .map(|(_, label, _, why)| (label, why))
}

/// Each `(action, reason)`: the menu bar shows it greyed as "Label (reason)" (`None`:
/// available, the plain label).
fn expect_menu(ui: &mut Ui, want: &[(&str, Option<&str>)]) {
    for &(action, why) in want {
        match live_entry(action) {
            Some((label, got)) if got.as_deref() == why => {
                if let Some(why) = why
                    && !label.ends_with(&format!("({why})"))
                {
                    fail!(ui, "{action}: label {label:?} without its reason {why:?}");
                }
            }
            other => fail!(ui, "{action}: menu entry {other:?}, expected reason {why:?}"),
        }
    }
}

/// The palette's best match for `query` is `label`, greyed with `why`; ⏎ on it runs no
/// git, closes the palette and says why in the status line.
fn expect_palette_refusal(ui: &mut Ui, query: &str, label: &str, why: &str) {
    ui.quick("action:gitti::CommandPalette");
    ui.quick(&format!("pick:{query}"));
    let Some(p) = state(ui).picker else {
        return fail!(ui, "palette `{query}`: no palette");
    };
    let first = p.labels.first().map(String::as_str);
    let off = p.disabled.first().cloned().flatten();
    if first != Some(label) || off.as_deref() != Some(why) || p.details.first().map(String::as_str) != Some(why) {
        fail!(ui, "palette `{query}`: first {first:?}, greyed {off:?}, details {:?}", p.details.first());
    }
    let n = ui.n_git();
    ui.quick("pickok");
    ui.no_git_since(n, &format!("palette ⏎ on the greyed {label}"));
    let want = gitti_core::availability::refusal(label, why);
    match status(ui) {
        Some((t, false)) if t == want => {}
        other => fail!(ui, "palette `{query}`: status {other:?}, expected {want:?}"),
    }
    if state(ui).picker.is_some() {
        fail!(ui, "palette `{query}`: still open after ⏎ on a greyed entry");
    }
}

/// What can't act now is greyed with the reason — the menu bar, the palette (⏎ says why), a
/// context menu — and its action says why instead of doing nothing; no git. The fixture:
/// changes staged and unstaged, no remote, no stash, one branch.
fn unavailable_commands(ui: &mut Ui) {
    use gitti_core::availability::*;
    ui.quick("focuslist");
    expect_menu(
        ui,
        &[
            ("gitti::PopStash", Some(NO_STASHES)),
            ("gitti::DropStash", Some(NO_STASHES)),
            ("gitti::Fetch", Some(NO_REMOTES)),
            ("gitti::Push", Some(NO_REMOTES)),
            ("gitti::DeleteBranchMenu", Some(NO_OTHER_BRANCH)),
            ("gitti::EndCompare", Some(NOT_COMPARING)),
            ("gitti::RemoveThisWorktree", Some(MAIN_WORKTREE)),
            ("gitti::RevertCommit", Some(WORKING_DIR_SELECTED)),
            ("gitti::NextTab", Some(ONE_TAB)),
            ("gitti::Commit", None),
            ("gitti::StageAll", None),
            ("gitti::UnstageAll", None),
            ("gitti::StashChanges", None),
            ("gitti::CreateBranch", None),
            ("gitti::RenameCurrentBranch", None),
        ],
    );
    // A key / menu action of a greyed command says why.
    let n = ui.n_git();
    ui.quick("action:gitti::PopStash");
    ui.no_git_since(n, "Pop Latest Stash without stashes");
    match status(ui) {
        Some((t, false)) if t == refusal("Pop Latest Stash", NO_STASHES) => {}
        other => fail!(ui, "Pop Latest Stash: status {other:?}"),
    }
    expect_palette_refusal(ui, "pop latest stash", "Pop Latest Stash", NO_STASHES);
    expect_palette_refusal(ui, "fetch from", "Fetch from…", NO_REMOTES);
    // A commit selected: the commit commands are on (the menu follows the selection).
    ui.step("select:2");
    expect_menu(ui, &[("gitti::RevertCommit", None), ("gitti::CherryPickCommit", None)]);
    ui.step("select:0");
    // The stashes section's menu greys what needs a stash.
    testing::menu_labels();
    ui.click_on("side-section:stashes", MouseButton::Right);
    let labels = testing::menu_labels();
    for want in ["Pop Latest Stash (no stashes)", "Apply Latest Stash (no stashes)", "Stash Changes…"] {
        if !labels.iter().any(|l| l == want) {
            fail!(ui, "stashes section menu: no {want:?} in {labels:?}");
        }
    }
    ui.quick("key:escape");
}

/// A detached HEAD greys the current branch's commands with "HEAD is detached".
fn detached_head_greys_branch_commands(ui: &mut Ui) {
    use gitti_core::availability::*;
    ui.quick("focuslist");
    expect_menu(
        ui,
        &[
            ("gitti::RenameCurrentBranch", Some(DETACHED)),
            ("gitti::SetUpstreamMenu", Some(DETACHED)),
            ("gitti::RebaseCurrentOnto", Some(DETACHED)),
            ("gitti::CreateBranch", None),
            ("gitti::MergeBranch", None),
        ],
    );
    expect_palette_refusal(
        ui,
        "rename current branch",
        "Rename Current Branch…",
        DETACHED,
    );
}

/// A fresh `git init` greys what needs a commit with "no commits yet".
fn fresh_repository_greys_what_needs_a_commit(ui: &mut Ui) {
    use gitti_core::availability::*;
    ui.quick("focuslist");
    expect_menu(
        ui,
        &[
            ("gitti::CreateBranch", Some(UNBORN)),
            ("gitti::CreateTag", Some(UNBORN)),
            ("gitti::GoToHead", Some(UNBORN)),
            ("gitti::AddWorktree", Some(UNBORN)),
            ("gitti::Commit", None),
            ("gitti::AddRemote", None),
        ],
    );
    let n = ui.n_git();
    ui.quick("action:gitti::CreateBranch");
    ui.no_git_since(n, "Create Branch without a commit");
    if modal_title(ui).is_some() {
        fail!(ui, "Create Branch at HEAD… opened a dialog without a commit");
    }
    match status(ui) {
        Some((t, false)) if t == refusal("Create Branch at HEAD…", UNBORN) => {}
        other => fail!(ui, "Create Branch: status {other:?}"),
    }
}

/// Create Tag's message is a multi-line text area, kept as typed (`#` lines too): an
/// annotated tag; without one a lightweight tag. The tag's sidebar menu starts with Show Tag: its tagger and message; a
/// lightweight tag says it is one.
fn tag_message_and_show_tag(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let dir = ui.workdir();
    ui.step("action:gitti::CreateTag");
    ui.quick("field:0=v9");
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            testing::modal_text(&ws, "Line one\n\n# kept\nthird", window, cx)
        })
        .unwrap();
    ui.step("modalok");
    ui.wait_real(1500, |ui| ui.is_idle());
    let msg = git_out(&dir, &["for-each-ref", "--format=%(contents)", "refs/tags/v9"]);
    if msg != "Line one\n\n# kept\nthird" {
        fail!(ui, "tag message {msg:?}");
    }
    // No message: a lightweight tag.
    ui.step("action:gitti::CreateTag");
    ui.quick("field:0=light9");
    ui.step("modalok");
    ui.wait_real(1500, |ui| ui.is_idle());
    if git_out(&dir, &["cat-file", "-t", "light9"]) != "commit" {
        fail!(ui, "light9 is not a lightweight tag");
    }
    for (tag, want) in [("v9", "Annotated tag v9"), ("light9", "Lightweight tag light9")] {
        ui.click_on(&format!("side:ref-refs/tags/{tag}"), MouseButton::Right);
        ui.quick("key:down");
        ui.quick("key:enter");
        ui.wait_real(1500, |ui| {
            let ws = ui.ws.clone();
            ui.cx.update(|cx| testing::modal_viewer_text(&ws, cx)).is_some()
        });
        let text = ui.cx.update(|cx| testing::modal_viewer_text(&ws, cx)).unwrap_or_default();
        if !text.starts_with(want) || (tag == "v9" && !(text.contains("Tagger:") && text.contains("# kept"))) {
            fail!(ui, "Show Tag {tag}: {text:?}");
        }
        ui.step("cancel");
    }
}
