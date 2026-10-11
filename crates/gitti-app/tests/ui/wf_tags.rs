//! Workflow tests of tags (catalog: `crates/gitti-workflow/WORKFLOWS.md`, TG), driven
//! through gitti's views: each scenario starts from the workflow's fixture
//! (`gitti_workflow::tg`), takes the user's steps (a tag's sidebar menu, the TAGS
//! section's menu, Create Tag at HEAD…, Show Tag…), checks what the app shows (dialog text,
//! the live refusal, the tag window) and then the repository — and the bare remote — with
//! the workflow's own `expect`, the same one the core test (`tests/core/wf_tags.rs`) and
//! the gitty test (`tests/tui/wf_tags.rs`) use.
//!
//! `cargo test --workspace --test ui -- wf_tags[::FILTER]`.

use crate::common;

use std::any::Any;
use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_workflow::tg;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

thread_local! {
    /// The running scenario's fixture ids (its workflow's `Setup`).
    static SETUP: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

fn setup<T: 'static>() -> T {
    let s = SETUP.with(|s| s.borrow_mut().take()).expect("the fixture's ids");
    *s.downcast::<T>().expect("the scenario's own setup")
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[
        ("tg03_push_a_tag_then_all_tags", tg03_push_a_tag_then_all_tags),
        ("tg04_delete_a_tag_on_origin_then_here", tg04_delete_a_tag_on_origin_then_here),
        ("tg05_an_existing_name_is_refused_in_the_dialog", tg05_an_existing_name_is_refused_in_the_dialog),
        ("tg06_tags_of_tags_and_blobs_listed_and_shown", tg06_tags_of_tags_and_blobs_listed_and_shown),
        ("tg07_tag_a_detached_commit", tg07_tag_a_detached_commit),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        let s: Box<dyn Any> = match &name[..4] {
            "tg03" | "tg04" => Box::new(tg::push_and_delete::fixture(&dir)),
            "tg05" => Box::new(tg::existing_name::fixture(&dir)),
            "tg06" => Box::new(tg::nested::fixture(&dir)),
            "tg07" => Box::new(tg::detached::fixture(&dir)),
            other => panic!("no fixture for {other}"),
        };
        SETUP.with(|c| *c.borrow_mut() = Some(s));
        dir
    });
}

/// A workflow's `expect` result as a scenario failure.
fn expect(ui: &mut Ui, result: Result<(), String>) {
    if let Err(e) = result {
        ui.fail(e);
    }
}

/// Opens the context menu of sidebar entry `probe` and chooses `entry`.
fn choose(ui: &mut Ui, probe: &str, entry: &str) -> bool {
    testing::menu_labels();
    ui.click_on(probe, MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == entry) {
        fail!(ui, "{probe}'s menu lacks {entry:?}: {labels:?}");
        return false;
    }
    ui.step(&format!("menu:{entry}"));
    true
}

/// Waits for the running git operation (a push) to end.
fn idle(ui: &mut Ui) {
    ui.wait_real(1500, |ui| ui.is_idle());
}

/// TG-03: v2's menu ▸ Push Tag to origin, then the TAGS section's Push All Tags to origin.
/// Workflow: TG-03
fn tg03_push_a_tag_then_all_tags(ui: &mut Ui) {
    use tg::push_and_delete as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    if !choose(ui, "side:ref-refs/tags/v2", "Push Tag to origin") {
        return;
    }
    idle(ui);
    expect(ui, wf::expect_pushed_one(&s));
    if !choose(ui, "side-section:tags", "Push All Tags to origin") {
        return;
    }
    idle(ui);
    if let Some((t, true)) = ui.status() {
        fail!(ui, "pushing all tags failed: {t}");
    }
    expect(ui, wf::expect_pushed(&dir, &s));
}

/// The native confirmation's (message, detail) — answered with `answer`.
fn answer_prompt(ui: &mut Ui, answer: &str) -> Option<(String, String)> {
    let asked = ui.cx.pending_prompt();
    if asked.is_some() {
        ui.cx.simulate_prompt_answer(answer);
        ui.settle();
        idle(ui);
    }
    asked
}

/// TG-04: v1's menu ▸ Delete Tag on origin… (asks), then ▸ Delete Tag… (asks, saying the
/// remotes keep theirs): gone in both.
/// Workflow: TG-04
fn tg04_delete_a_tag_on_origin_then_here(ui: &mut Ui) {
    use tg::push_and_delete as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    if !choose(ui, "side:ref-refs/tags/v1", "Delete Tag on origin…") {
        return;
    }
    match answer_prompt(ui, "Delete") {
        Some((m, _)) if m == "Delete tag v1 on remote origin?" => {}
        other => return fail!(ui, "Delete Tag on origin… asked {other:?}"),
    }
    expect(ui, wf::expect_deleted_on_origin(&s));
    if !choose(ui, "side:ref-refs/tags/v1", "Delete Tag…") {
        return;
    }
    match answer_prompt(ui, "Delete") {
        Some((m, detail)) if m == "Delete tag v1?" && detail.contains("If origin has v1, it keeps it") => {}
        other => return fail!(ui, "Delete Tag… asked {other:?}"),
    }
    if ui.ui_state().status.is_some_and(|(_, err)| err) {
        fail!(ui, "deleting failed: {:?}", ui.status());
    }
    expect(ui, wf::expect_deleted(&dir, &s));
}

fn live(ui: &mut Ui) -> Option<gitti_app::testing_menus::ModalLive> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| gitti_app::testing_menus::modal_live(&ws, cx))
}

/// TG-05: Create Tag at HEAD… with `v1`: OK disabled, the dialog says where v1 is and
/// what to do; ⏎ does nothing (no git); `v1.1` goes through.
/// Workflow: TG-05
fn tg05_an_existing_name_is_refused_in_the_dialog(ui: &mut Ui) {
    use tg::existing_name as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    ui.step("action:gitti::CreateTag");
    ui.quick(&format!("field:0={}", wf::TAKEN));
    let want = format!("{}{})", wf::REFUSED, &s.init[..7]);
    match live(ui) {
        Some((_, _, Some(why), _)) if why.starts_with(&want) => {}
        other => fail!(ui, "v1 not refused: {other:?}"),
    }
    let n = ui.n_git();
    ui.step("modalok");
    ui.no_git_since(n, "OK on a refused name");
    if ui.modal().is_none() {
        fail!(ui, "the dialog closed on a refused name");
    }
    ui.quick(&format!("field:0={}", wf::NEW_NAME));
    match live(ui) {
        Some((_, _, None, _)) => {}
        other => fail!(ui, "v1.1 refused: {other:?}"),
    }
    ui.step("modalok");
    idle(ui);
    expect(ui, wf::expect(&dir, &s));
}

/// The text of the tag window Show Tag… of `tag` opens (closed again).
fn show_tag(ui: &mut Ui, tag: &str) -> String {
    if !choose(ui, &format!("side:ref-refs/tags/{tag}"), "Show Tag…") {
        return String::new();
    }
    let ws = ui.ws.clone();
    ui.wait_real(1500, |ui| ui.cx.update(|cx| testing::modal_viewer_text(&ws, cx)).is_some());
    let text = ui.cx.update(|cx| testing::modal_viewer_text(&ws, cx)).unwrap_or_default();
    ui.step("cancel");
    text
}

/// TG-06: a tag of a tag, an annotated and a lightweight tag of a blob: all listed;
/// Show Tag says what each is and points at.
/// Workflow: TG-06
fn tg06_tags_of_tags_and_blobs_listed_and_shown(ui: &mut Ui) {
    use tg::nested as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    let probes = ui.frame();
    for tag in wf::ALL {
        if find(&probes, &format!("side:ref-refs/tags/{tag}")).is_none() {
            fail!(ui, "tag {tag} not listed");
        }
    }
    let text = show_tag(ui, wf::OUTER);
    let chain = format!("Points at: tag {} → commit {} init", &s.inner[..10], &s.init[..10]);
    if !(text.starts_with("Annotated tag outer") && text.contains(wf::OUTER_MESSAGE) && text.contains(&chain)) {
        fail!(ui, "Show Tag outer: {text:?}");
    }
    let text = show_tag(ui, "light-blob");
    if !(text.starts_with("Lightweight tag light-blob") && text.contains(&format!("blob {}", &s.blob[..10]))) {
        fail!(ui, "Show Tag light-blob: {text:?}");
    }
    let text = show_tag(ui, wf::BLOB_TAG);
    if !(text.starts_with("Annotated tag blob-tag") && text.contains(&format!("blob {}", &s.blob[..10]))) {
        fail!(ui, "Show Tag blob-tag: {text:?}");
    }
    expect(ui, wf::expect(&dir, &s));
}

/// TG-07: on a detached HEAD, Create Tag at HEAD… with a message of several lines: an
/// annotated tag of that commit, HEAD stays detached.
/// Workflow: TG-07
fn tg07_tag_a_detached_commit(ui: &mut Ui) {
    use tg::detached as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    if ui.ui_state().branch.is_some() {
        fail!(ui, "HEAD is not detached: {:?}", ui.ui_state().branch);
    }
    ui.step("action:gitti::CreateTag");
    ui.quick(&format!("field:0={}", wf::NAME));
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::modal_text(&ws, wf::MESSAGE, window, cx))
        .unwrap();
    ui.step("modalok");
    idle(ui);
    match ui.status() {
        Some((t, false)) if t == format!("Created tag {}", wf::NAME) => {}
        other => fail!(ui, "status after creating the tag: {other:?}"),
    }
    expect(ui, wf::expect(&dir, &s));
}
