//! Operations that stop, are refused or would change more than shown: a merge that stops
//! on a conflict says so in the status bar (not as an error), Stash Changes is refused
//! during it and Stage All asks about the conflict markers; a cherry-pick with nothing to
//! commit asks Skip / Commit Empty, the banner has Skip; a merge refused for an
//! uncommitted change offers Stash and Merge; Stage All leaves untracked files hidden by
//! the config out; a stage from a stale view is refused in plain words; a line-ending-only
//! change has its note row.
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- op_safety[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::*;

thread_local! {
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
}

const MERGE_STOP: &str = "Merge stopped on 1 conflict — resolve it, then Continue (or Abort).";

pub fn main() {
    let tmp = common::setup();
    BASE.with(|b| *b.borrow_mut() = gitti_core::paths::canonical(tmp.path()));
    let tests: &[(&str, Scenario)] = &[
        ("merge_stop_stash_and_stage_all", merge_stop_stash_and_stage_all),
        ("empty_pick_skip", empty_pick_skip),
        ("blocked_merge_stash_and_merge", blocked_merge_stash_and_merge),
        ("hidden_untracked_and_stale_stage", hidden_untracked_and_stale_stage),
    ];
    common::run(tests, |name| make_ops_repo(&BASE.with(|b| b.borrow().join(name))));
}

/// main and `feat` both changed a.txt (merging conflicts); `clean` changed line 1 of
/// b.txt (merges cleanly); `same` has main's change of a.txt (picking it is empty); w.txt
/// has CRLF line endings, LF in the working tree. Checked out: main.
fn make_ops_repo(dir: &Path) -> PathBuf {
    init_repo(dir);
    let mut s = Stream::new();
    let init = s.commit(
        "refs/heads/main",
        "init",
        None,
        &[("a.txt", Some("base\n")), ("b.txt", Some("1\n2\n3\n4\n5\n")), ("w.txt", Some("x\r\ny\r\n"))],
    );
    s.commit("refs/heads/feat", "feat a", Some(init), &[("a.txt", Some("feat\n"))]);
    s.commit("refs/heads/clean", "clean b", Some(init), &[("b.txt", Some("ONE\n2\n3\n4\n5\n"))]);
    s.commit("refs/heads/same", "same a", Some(init), &[("a.txt", Some("main\n"))]);
    s.commit("refs/heads/main", "main a", None, &[("a.txt", Some("main\n"))]);
    s.import(dir);
    git(dir, &["reset", "-q", "--hard"]);
    std::fs::write(dir.join("w.txt"), "x\ny\n").unwrap();
    dir.to_path_buf()
}

fn merge(ui: &mut Ui, branch: &str) {
    ui.quick("action:gitti::MergeBranch");
    ui.quick(&format!("pick:{branch}"));
    ui.quick("pickok");
    ui.step("modalok");
}

/// Merge feat: a stop, said plainly (no error, no error mark); Stash Changes says why it
/// can't; Stage All asks about a.txt's markers (Cancel: nothing staged); the diff menu's
/// stash entries are greyed. The line-ending change of w.txt has its note row.
/// Workflow: EX-04, MG-19, SH-16
fn merge_stop_stash_and_stage_all(ui: &mut Ui) {
    let dir = ui.repo.clone();
    let probes = ui.check("worktree");
    if !probes.iter().any(|p| p.kind() == "eol-note") {
        ui.fail("no line-ending note for w.txt".into());
    }
    merge(ui, "feat");
    let st = ui.ui_state();
    if st.status != Some((MERGE_STOP.to_string(), false)) || st.error_mark.is_some() {
        ui.fail(format!("after the merge stopped: status {:?}, mark {:?}", st.status, st.error_mark));
    }
    if !dir.join(".git/MERGE_HEAD").exists() {
        return ui.fail("no merge in progress".into());
    }
    let n = ui.n_git();
    ui.step("action:gitti::StashChanges");
    match ui.status() {
        Some((s, _)) if s.contains("not available: a merge is in progress") => {}
        other => ui.fail(format!("Stash Changes during the merge: {other:?}")),
    }
    ui.no_git_since(n, "a refused stash");
    ui.step("action:gitti::StageAll");
    if !ui.cx.has_pending_prompt() {
        return ui.fail("Stage All with conflict markers left did not ask".into());
    }
    ui.cx.simulate_prompt_answer("Cancel");
    ui.settle();
    if git_out(&dir, &["ls-files", "-u"]).is_empty() {
        ui.fail("a.txt was marked resolved".into());
    }
    // The diff menu's stash entries (a file, the marked files) are greyed with the reason.
    let mut files: Vec<(usize, String)> = ui
        .frame()
        .iter()
        .filter(|p| p.name.starts_with("diff-row:") && p.name.ends_with(":file") && p.visible().is_some())
        .filter_map(|p| Some((p.name.split(':').nth(1)?.parse().ok()?, p.name.clone())))
        .collect();
    files.sort();
    let Some((_, last)) = files.pop() else {
        return ui.fail("no file header".into());
    };
    ui.click_on_with(&last, MouseButton::Left, Modifiers::secondary_key());
    testing::menu_labels();
    ui.click_on(&last, MouseButton::Right);
    let labels = testing::menu_labels();
    for want in ["Stash This File… (a merge is in progress)", "Stash Marked (1 file)… (a merge is in progress)"] {
        if !labels.iter().any(|l| l == want) {
            ui.fail(format!("diff menu during the merge: no {want:?} in {labels:?}"));
        }
    }
    ui.quick("key:escape");
}

/// Picking `same` stops with nothing to commit (on the command line): the banner offers
/// Skip; Continue asks, Skip This Commit ends the pick.
/// Workflow: CP-09
fn empty_pick_skip(ui: &mut Ui) {
    let dir = ui.repo.clone();
    let out = std::process::Command::new("git").args(["cherry-pick", "same"]).current_dir(&dir).output().unwrap();
    assert!(!out.status.success());
    ui.step("key:f5");
    let probes = ui.check("cherry-pick stopped");
    if find(&probes, "op-skip").is_none() {
        return ui.fail("no Skip in the banner".into());
    }
    ui.click_on("op-continue", MouseButton::Left);
    ui.settle();
    if !ui.cx.has_pending_prompt() {
        let status = ui.status();
        return ui.fail(format!("Continue with nothing to commit did not ask: {:?}", status));
    }
    ui.cx.simulate_prompt_answer("Skip This Commit");
    ui.settle();
    if dir.join(".git/CHERRY_PICK_HEAD").exists() {
        let status = ui.status();
        ui.fail(format!("still picking after Skip: {:?}", status));
    }
}

/// b.txt changed (line 5): merging `clean` is refused; the question's Stash and Merge
/// merges and puts the change back.
/// Workflow: MG-16
fn blocked_merge_stash_and_merge(ui: &mut Ui) {
    let dir = ui.repo.clone();
    std::fs::write(dir.join("b.txt"), "1\n2\n3\n4\nFIVE\n").unwrap();
    ui.step("key:f5");
    merge(ui, "clean");
    if !ui.cx.has_pending_prompt() {
        let status = ui.status();
        return ui.fail(format!("the refused merge did not ask: {:?}", status));
    }
    ui.cx.simulate_prompt_answer("Stash and Merge");
    ui.settle();
    let b = std::fs::read_to_string(dir.join("b.txt")).unwrap();
    if b != "ONE\n2\n3\n4\nFIVE\n" || git_out(&dir, &["log", "-1", "--format=%s"]).trim() != "Merge branch 'clean'" {
        let status = ui.status();
        ui.fail(format!("after Stash and Merge: b.txt {b:?}, status {:?}", status));
    }
}

/// Untracked files hidden by the config stay out of Stage All (said in the status bar);
/// a hunk staged from a diff the file no longer has is refused, nothing staged.
/// Workflow: EX-17, IG-05
fn hidden_untracked_and_stale_stage(ui: &mut Ui) {
    let dir = ui.repo.clone();
    std::fs::write(dir.join("b.txt"), "1\n2\nTHREE\n4\n5\n").unwrap();
    ui.step("key:f5");
    // Changed behind the view's back.
    std::fs::write(dir.join("b.txt"), "1\n2\nTHREX\n4\n5\n").unwrap();
    ui.quick("stagehunk:b.txt,0");
    ui.settle();
    match ui.status() {
        Some((s, true)) if s.starts_with("b.txt changed since it was shown — nothing was staged") => {}
        other => ui.fail(format!("a stale stage: {other:?}")),
    }
    if !git_out(&dir, &["diff", "--cached", "--name-only"]).trim().is_empty() {
        ui.fail("a stale stage staged something".into());
    }
    append_config(&dir, "[status]\n\tshowUntrackedFiles = no\n");
    std::fs::write(dir.join("secret.txt"), "hidden\n").unwrap();
    ui.step("key:f5");
    ui.step("action:gitti::StageAll");
    match ui.status() {
        Some((s, false)) if s == gitti_core::stage_all::HIDDEN_UNTRACKED_NOTE => {}
        other => ui.fail(format!("Stage All with hidden untracked files: {other:?}")),
    }
    if git_out(&dir, &["diff", "--cached", "--name-only"]).contains("secret.txt") {
        ui.fail("a hidden untracked file was staged".into());
    }
}
