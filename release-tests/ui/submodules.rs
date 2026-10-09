//! Headless tests of the submodule UI: the SUBMODULES sidebar section (states,
//! nested entries, double click opens a tab), the "out of date" banner and its Update, a
//! pointer change in the working-directory diff as summary + commit rows (staged as one
//! unit, no git call of its own), layout checks of both across font sizes, and nothing of
//! it in a repository without submodules; Add Submodule… there. Stage 2: the nested
//! submodule's dirty state (refresh only, no git on navigation), Update to Remote Branch,
//! the diff menu's submodule part, Remove Submodule… with its warning about local work.
//! Check Out Commit in Submodule… (history picker without git calls, detached
//! checkout, pointer change staged + committed records it, the warning before losing work).
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- submodules[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::*;
use gitti_app::testing_menus as menus;

thread_local! {
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
}

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
    // Local clones as submodules (`git submodule add/update` of a path) need this; commits
    // inside a cloned submodule need an identity even without a global git config. gitti's
    // own git calls drop `GIT_CONFIG_COUNT` (`LOCAL_REPO_ENV`), so the setting also goes
    // into a global config file of the test.
    let global = gitti_core::paths::canonical(tmp.path()).join("gitconfig");
    std::fs::write(&global, "[protocol \"file\"]\n\tallow = always\n").unwrap();
    // SAFETY: still single-threaded.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
        for who in ["AUTHOR", "COMMITTER"] {
            std::env::set_var(format!("GIT_{who}_NAME"), "T");
            std::env::set_var(format!("GIT_{who}_EMAIL"), "t@example.com");
        }
    }
    BASE.with(|b| *b.borrow_mut() = gitti_core::paths::canonical(tmp.path()));
    let tests: &[(&str, Scenario)] = &[
        ("sidebar_banner_update", sidebar_banner_update),
        ("pointer_diff_rows", pointer_diff_rows),
        ("no_submodules", no_submodules),
        ("stage2_nested_remote_remove", stage2_nested_remote_remove),
        ("sidebar_menus", sidebar_menus),
        ("checkout_commit", checkout_commit),
    ];
    common::run(tests, |name| {
        if name == "no_submodules" {
            make_repo(&base().join(name))
        } else {
            make_super(&base().join(name))
        }
    });
}

fn commit_file(p: &Path, name: &str, text: &str, msg: &str) {
    std::fs::write(p.join(name), text).unwrap();
    git(p, &["add", name]);
    git(p, &["commit", "-qm", msg]);
}

fn init(p: &Path) {
    std::fs::create_dir_all(p).unwrap();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.name", "T"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "commit.gpgsign", "false"]);
}

/// `<dir>/super` with submodule `libs/lib` (which has `vendor/inner`), all checked out.
fn make_super(dir: &Path) -> PathBuf {
    let inner = dir.join("inner");
    init(&inner);
    commit_file(&inner, "i.txt", "i\n", "inner one");
    let lib = dir.join("lib");
    init(&lib);
    commit_file(&lib, "l.txt", "1\n", "lib one");
    git(&lib, &["submodule", "add", "-q", inner.to_str().unwrap(), "vendor/inner"]);
    git(&lib, &["commit", "-qm", "lib two: inner"]);
    let sup = dir.join("super");
    init(&sup);
    commit_file(&sup, "s.txt", "s\n", "super one");
    git(&sup, &["submodule", "add", "-q", lib.to_str().unwrap(), "libs/lib"]);
    git(&sup, &["submodule", "update", "--init", "--recursive", "-q"]);
    git(&sup, &["commit", "-qm", "add lib"]);
    sup
}

fn submodules(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::submodules(&ws, cx))
}

fn tabs(ui: &mut Ui) -> (usize, usize) {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::tabs(&ws, cx))
}

fn workdir(ui: &mut Ui) -> PathBuf {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::workdir(&ws, cx))
}

fn working(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::working(&ws, cx))
}

fn visible(p: &[Probe], name: &str) -> bool {
    find(p, name).is_some_and(|r| r.visible().is_some())
}

fn dclick_on(ui: &mut Ui, target: &str) {
    let p = ui.frame();
    let Some(r) = find(&p, target).and_then(Probe::visible) else {
        fail!(ui, "dclick: no visible {target}");
        return;
    };
    let c = r.center();
    ui.step(&format!("dclick:{},{}", f32::from(c.x), f32::from(c.y)));
}

/// Every frame of a few font sizes passes the layout invariants (`ctx` names the state).
fn font_sweep(ui: &mut Ui, ctx: &str, must_show: &[&str]) {
    for (f_ui, f_diff) in [(13., 13.), (16., 15.), (22., 20.), (28., 32.)] {
        ui.step(&format!("fonts:{f_ui},{f_diff}"));
        let p = ui.check(&format!("{ctx} at {f_ui}/{f_diff}"));
        for name in must_show {
            if !visible(&p, name) {
                fail!(ui, "{ctx} at {f_ui}/{f_diff}: no visible {name}");
            }
        }
    }
    ui.step("fonts:13,13");
}

// ------------------------------------------------------------------ scenarios

/// The section lists both submodules (nested one indented after its parent) as up to
/// date; checking the submodule out at an older commit shows the banner, whose Update
/// brings it back (and the banner goes); a double click opens the submodule in a tab.
fn sidebar_banner_update(ui: &mut Ui) {
    let sup = ui.repo.clone();
    let want = ["libs/lib up to date", "libs/lib/vendor/inner up to date"];
    if submodules(ui) != want {
        fail!(ui, "submodules at the start: {:?}", submodules(ui));
    }
    let p = ui.check("clean");
    for name in [
        "side-section:submodules",
        "side:sub-libs/lib",
        "side:sub-libs/lib/vendor/inner",
        "sub-mark:libs/lib",
    ] {
        if !visible(&p, name) {
            fail!(ui, "clean: no visible {name}");
        }
    }
    if visible(&p, "submodule-banner") {
        ui.fail("a banner while everything is up to date".into());
    }
    // Nested: indented further than its parent.
    let x = |p: &[Probe], n: &str| find(p, n).and_then(Probe::visible).map(|r| r.origin.x);
    let (outer, inner) = (
        x(&p, "sub-mark:libs/lib"),
        x(&p, "sub-mark:libs/lib/vendor/inner"),
    );
    if !matches!((outer, inner), (Some(a), Some(b)) if b > a) {
        fail!(ui, "nested entry not indented: {outer:?} {inner:?}");
    }

    // Behind the recorded commit (as after a pull that moved the pointer).
    git(&sup.join("libs/lib"), &["checkout", "-q", "HEAD~1"]);
    ui.step("key:secondary-r");
    if submodules(ui).first().map(String::as_str) != Some("libs/lib out of date") {
        fail!(ui, "after moving it back: {:?}", submodules(ui));
    }
    font_sweep(ui, "out of date", &["submodule-banner", "submodule-update"]);
    ui.click_on("submodule-update", MouseButton::Left);
    if submodules(ui) != want {
        fail!(ui, "after Update: {:?}", submodules(ui));
    }
    if visible(&ui.frame(), "submodule-banner") {
        ui.fail("the banner stays after Update".into());
    }
    if !ui.git_commands().iter().any(|c| c.contains("submodule update --init --recursive")) {
        fail!(ui, "no submodule update ran: {:?}", ui.git_commands());
    }

    // Double click: a new tab on the submodule's folder.
    dclick_on(ui, "side:sub-libs/lib");
    if tabs(ui) != (2, 1) || workdir(ui) != sup.join("libs/lib") {
        let (t, w) = (tabs(ui), workdir(ui));
        fail!(ui, "not opened in a tab: {t:?} {}", w.display());
    }
    // Its own sidebar lists its submodule.
    if submodules(ui) != ["vendor/inner up to date"] {
        fail!(ui, "submodule tab: {:?}", submodules(ui));
    }
}

/// Two new commits in the submodule: the working-directory diff shows the summary and
/// both commit subjects as `submodule` rows (no hunk / line rows), staging the file moves
/// it as a whole and runs no submodule command; the layout holds at every font size.
fn pointer_diff_rows(ui: &mut Ui) {
    let sup = ui.repo.clone();
    let sub = sup.join("libs/lib");
    commit_file(&sub, "l.txt", "1\n2\n", "lib three");
    commit_file(&sub, "l.txt", "1\n2\n3\n", "lib four");
    ui.step("key:secondary-r");
    ui.step("select:0");
    let rows = |p: &[Probe]| -> Vec<String> {
        p.iter()
            .filter(|q| q.name.starts_with("diff-row:"))
            .filter_map(|q| q.name.rsplit(':').next().map(str::to_string))
            .collect()
    };
    let p = ui.check("pointer change");
    let kinds = rows(&p);
    let subs = kinds.iter().filter(|k| *k == "submodule").count();
    if subs != 3 || kinds.iter().any(|k| k == "hunk" || k == "line") {
        fail!(ui, "diff rows: {kinds:?}");
    }
    if submodules(ui).first().map(String::as_str) != Some("libs/lib modified") {
        fail!(ui, "state with new commits: {:?}", submodules(ui));
    }
    font_sweep(ui, "pointer rows", &["side:sub-libs/lib"]);

    let before = ui.git_commands().len();
    ui.step("stagefile:libs/lib");
    let after: Vec<String> = ui.git_commands()[before..].to_vec();
    if working(ui) != ["S libs/lib 1"] {
        fail!(ui, "after staging: {:?}", working(ui));
    }
    if after.iter().any(|c| c.split_whitespace().any(|w| w == "submodule")) {
        fail!(ui, "staging ran submodule commands: {after:?}");
    }
    if submodules(ui).first().map(String::as_str) != Some("libs/lib up to date") {
        fail!(ui, "state after staging the pointer: {:?}", submodules(ui));
    }
    let p = ui.check("staged pointer");
    if rows(&p).iter().filter(|k| *k == "submodule").count() != 3 {
        fail!(ui, "staged rows: {:?}", rows(&p));
    }
}

/// No section, no banner and no submodule work at all without submodules.
fn no_submodules(ui: &mut Ui) {
    let p = ui.check("plain repository");
    if visible(&p, "side-section:submodules") || visible(&p, "submodule-banner") {
        ui.fail("submodule UI without submodules".into());
    }
    if !submodules(ui).is_empty() {
        fail!(ui, "submodules: {:?}", submodules(ui));
    }
    ui.step("key:secondary-r");
    if ui.git_commands().iter().any(|c| c.split_whitespace().any(|w| w == "submodule")) {
        fail!(ui, "submodule commands: {:?}", ui.git_commands());
    }

    // Repository ▸ Add Submodule… (here via the palette): the dialog clones and stages it,
    // the section appears.
    let src = base().join("add-src");
    init(&src);
    commit_file(&src, "a.txt", "a\n", "add src one");
    ui.step("palette:Add Submodule…");
    ui.step(&format!("field:0={}", src.display()));
    ui.step("field:1=deps/addme");
    ui.step("modalok");
    if submodules(ui) != ["deps/addme up to date"] {
        fail!(ui, "after Add Submodule: {:?}", submodules(ui));
    }
    let w = working(ui);
    if !w.iter().any(|l| l.contains(".gitmodules")) || !w.iter().any(|l| l.contains("deps/addme")) {
        fail!(ui, "not staged: {w:?}");
    }
    if !visible(&ui.frame(), "side-section:submodules") {
        ui.fail("no SUBMODULES section after Add Submodule".into());
    }
}

fn modal_title(ui: &mut Ui) -> Option<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal(&ws, cx)).map(|m| m.0)
}

fn modal_message(ui: &mut Ui) -> String {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default()
}

/// Stage 2: a dirty file in the nested submodule shows on both entries after a refresh,
/// and navigating runs no git; Update to Remote Branch follows lib's main (a new pointer
/// to commit); the diff's menu on the pointer change ends with the submodule's actions
/// (Remove Submodule… last), whose dialog names the local work that gets lost; removing
/// stages `.gitmodules` + the pointer and the section goes.
fn stage2_nested_remote_remove(ui: &mut Ui) {
    let sup = ui.repo.clone();
    let lib = sup.parent().unwrap().join("lib");
    let inner = sup.join("libs/lib/vendor/inner");
    std::fs::write(inner.join("i.txt"), "changed\n").unwrap();
    ui.step("key:secondary-r");
    let want = ["libs/lib modified", "libs/lib/vendor/inner modified"];
    if submodules(ui) != want {
        fail!(ui, "nested dirty: {:?}", submodules(ui));
    }
    font_sweep(ui, "nested dirty", &["sub-mark:libs/lib/vendor/inner"]);
    // Navigation reads no state (the nested state came with the refresh; a commit's diff
    // is all it loads).
    ui.step("focuslist");
    let before = ui.git_commands().len();
    for k in ["down", "down", "up", "up"] {
        ui.step(&format!("key:{k}"));
    }
    let nav: Vec<String> = ui.git_commands()[before..].to_vec();
    if nav.iter().any(|c| c.split_whitespace().any(|w| w == "status" || w == "submodule")) {
        fail!(ui, "navigation read the state: {nav:?}");
    }
    git(&inner, &["checkout", "--", "i.txt"]);

    // Update to Remote Branch: lib's new commit comes in, the pointer shows as a change.
    commit_file(&lib, "l.txt", "1\nupstream\n", "lib upstream");
    ui.step("submodule:remote,libs/lib");
    if !ui
        .git_commands()
        .iter()
        .any(|c| c.contains("submodule update --init --recursive --remote -- libs/lib"))
    {
        fail!(ui, "no remote update ran: {:?}", ui.git_commands());
    }
    if submodules(ui).first().map(String::as_str) != Some("libs/lib modified") {
        fail!(ui, "after Update to Remote Branch: {:?}", submodules(ui));
    }
    if !working(ui).iter().any(|l| l.contains("libs/lib")) {
        fail!(ui, "the new pointer isn't a change: {:?}", working(ui));
    }

    // Local work for the warning: an untracked file in the submodule.
    std::fs::write(sup.join("libs/lib/notes.txt"), "n\n").unwrap();
    ui.step("key:secondary-r");
    ui.step("select:0");
    let p = ui.frame();
    let Some(row) = p
        .iter()
        .find(|q| q.name.starts_with("diff-row:") && q.name.ends_with(":submodule"))
        .map(|q| q.name.clone())
    else {
        fail!(ui, "no submodule diff row");
        return;
    };
    ui.click_on(&row, MouseButton::Right);
    // The last entry of the diff's menu: the submodule's Remove Submodule….
    ui.step("key:up");
    ui.step("key:enter");
    if modal_title(ui).as_deref() != Some("Remove Submodule") {
        fail!(ui, "diff menu → last entry: {:?}", modal_title(ui));
    }
    let msg = modal_message(ui);
    for want in [
        "Removes submodule libs/lib",
        ".git/modules/libs/lib",
        "Nested submodules go with it: libs/lib/vendor/inner",
        "This local work is lost",
        "libs/lib: untracked files",
    ] {
        if !msg.contains(want) {
            fail!(ui, "remove dialog lacks {want:?}: {msg}");
        }
    }
    ui.check("remove dialog");
    ui.step("modalok");
    if !submodules(ui).is_empty() {
        fail!(ui, "after Remove: {:?}", submodules(ui));
    }
    if sup.join("libs/lib").exists() || sup.join(".git/modules/libs/lib").exists() {
        fail!(ui, "submodule files left behind");
    }
    let w = working(ui);
    if !w.iter().any(|l| l.starts_with("S .gitmodules")) || !w.iter().any(|l| l.starts_with("S libs/lib")) {
        fail!(ui, "removal not staged: {w:?}");
    }
    if visible(&ui.frame(), "side-section:submodules") {
        ui.fail("SUBMODULES section after removing the last one".into());
    }
}

/// The submodule row's and the SUBMODULES header's menus have the entries of
/// `gitti_core::sidebar_menus`.
fn sidebar_menus(ui: &mut Ui) {
    use gitti_core::sidebar_menus::{SideMenu, Vars, missing};
    let vars = Vars {
        name: "libs/lib".into(),
        fm: gitti_core::desktop::file_manager_name().to_string(),
        ..Default::default()
    };
    for (menu, target) in [
        (SideMenu::Submodule, "side:sub-libs/lib"),
        (SideMenu::SubmodulesSection, "side-section:submodules"),
    ] {
        testing::menu_labels();
        ui.click_on(target, MouseButton::Right);
        let labels = testing::menu_labels();
        let miss = missing(menu, &vars, &labels);
        if !miss.is_empty() {
            fail!(ui, "{menu:?} menu lacks {miss:?}; has {labels:?}");
        }
        ui.step("key:escape");
    }
}

fn picker(ui: &mut Ui) -> Option<testing::PickerState> {
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
        .picker
}

fn head(p: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(p)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// The submodule's menu has "Check Out Commit in Submodule…": a picker over its
/// history (newest first, what is recorded / checked out marked; read in-process — no git
/// call), filtered by typing; an older commit checked out detached is "out of date"
/// without the banner offering to undo it; staging + committing the pointer records it
/// (the sidebar shows recorded → checked out until then); a detached commit nothing else
/// has makes it ask first.
fn checkout_commit(ui: &mut Ui) {
    let sup = ui.repo.clone();
    let sub = sup.join("libs/lib");
    commit_file(&sub, "l.txt", "1\n2\n", "lib three");
    commit_file(&sub, "l.txt", "1\n2\n3\n", "lib four");
    let three = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD~1"])
            .current_dir(&sub)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    ui.step("key:secondary-r");
    font_sweep(ui, "recorded vs checked out", &["side:sub-libs/lib"]);

    // From the sidebar row's menu (Update, Update to Remote Branch, Check Out Commit…).
    let before = ui.git_commands().len();
    ui.click_on("side:sub-libs/lib", MouseButton::Right);
    for k in ["down", "down", "down", "enter"] {
        ui.step(&format!("key:{k}"));
    }
    let Some(pk) = picker(ui) else {
        fail!(ui, "no picker from the submodule menu");
        return;
    };
    if pk.labels != ["lib four", "lib three", "lib two: inner", "lib one"] {
        fail!(ui, "history picker: {:?}", pk.labels);
    }
    if !pk.details[0].contains("checked out · main") || !pk.details[2].contains("recorded · origin/main") {
        fail!(ui, "picker marks: {:?}", pk.details);
    }
    if !pk.note.as_deref().unwrap_or("").starts_with("4 commits of libs/lib") {
        fail!(ui, "picker note: {:?}", pk.note);
    }
    let read: Vec<String> = ui.git_commands()[before..].to_vec();
    if !read.is_empty() {
        fail!(ui, "the history picker ran git: {read:?}");
    }
    ui.check("history picker");

    // An older commit: detached there; out of date, but no banner offering Update.
    ui.step("pick:lib one");
    ui.step("pickok");
    if !ui.git_commands().iter().any(|c| c.contains("checkout --detach")) {
        fail!(ui, "no checkout ran: {:?}", ui.git_commands());
    }
    if submodules(ui).first().map(String::as_str) != Some("libs/lib out of date") {
        fail!(ui, "after checking out lib one: {:?}", submodules(ui));
    }
    if visible(&ui.frame(), "submodule-banner") {
        ui.fail("the banner offers to undo the chosen commit".into());
    }
    // From the palette (one submodule checked out: its history directly), lib three.
    ui.step("palette:Check Out Commit in Submodule");
    ui.step("pick:three");
    if picker(ui).map(|p| p.labels.first().cloned()) != Some(Some("lib three".into())) {
        fail!(ui, "palette → picker filtered: {:?}", picker(ui));
    }
    ui.step("pickok");
    if head(&sub) != three {
        fail!(ui, "lib three not checked out: {}", head(&sub));
    }
    if !working(ui).iter().any(|l| l.starts_with("U libs/lib")) {
        fail!(ui, "no pointer change: {:?}", working(ui));
    }
    // Staged and committed: the superproject records lib three.
    ui.step("stagefile:libs/lib");
    ui.step("msg:pin lib to lib three");
    ui.step("commit");
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD:libs/lib"])
        .current_dir(&sup)
        .output()
        .unwrap();
    if String::from_utf8_lossy(&out.stdout).trim() != three {
        fail!(ui, "the commit doesn't record lib three: {}", String::from_utf8_lossy(&out.stdout));
    }
    if submodules(ui).first().map(String::as_str) != Some("libs/lib up to date") {
        fail!(ui, "after the commit: {:?}", submodules(ui));
    }

    // Work on the detached HEAD that nothing else has: it asks first.
    commit_file(&sub, "d.txt", "d\n", "detached work");
    ui.step("key:secondary-r");
    ui.step("submodule:checkout,libs/lib");
    ui.step("pick:lib four");
    ui.step("pickok");
    if modal_title(ui).as_deref() != Some("Check Out Commit in Submodule") {
        fail!(ui, "no warning: {:?}", modal_title(ui));
    }
    if !modal_message(ui).contains("1 commit on no branch") {
        fail!(ui, "warning text: {}", modal_message(ui));
    }
    ui.check("checkout warning");
    ui.step("modalok");
    if submodules(ui).first().map(String::as_str) != Some("libs/lib modified") {
        fail!(ui, "after Check Out Anyway: {:?}", submodules(ui));
    }
}
