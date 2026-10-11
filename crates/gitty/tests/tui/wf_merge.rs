//! Workflow tests of merging (catalog: `crates/gitti-workflow/WORKFLOWS.md`, MG, RN-05),
//! driven through gitty: the workflow's fixture (`gitti_workflow::mg`), one headless gitty
//! run with the user's keys (the palette's Merge Branch into Current… and the Merge dialog,
//! a branch's menu, a conflicted file's resolve menu, Continue / Abort from the palette),
//! `state` / `dump` checks of what gitty shows, then the repository with the workflow's own
//! `expect` — shared with `tests/core/wf_merge.rs` and `tests/ui/wf_merge.rs`.

use std::path::{Path, PathBuf};

use crate::common::*;
use crate::support::*;

use gitti_workflow::mg;

/// gitty on `dir` with `script` (120×40), the global git config shut out.
fn run(dir: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    gitty_env(dir, config.path(), "120x40", &[("GIT_CONFIG_GLOBAL", "/dev/null")], script)
}

/// A fresh folder for a fixture: `<tmp>/repo` (a fixture's linked worktree lands next to
/// it).
fn repo_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    (tmp, dir)
}

/// The palette's Merge Branch into Current…, `branch` picked: the Merge dialog (dumped);
/// then `dialog` (keys in the dialog: the message, tab / space for the checkboxes) and ⏎.
fn merge(branch: &str, dialog: &str) -> String {
    format!(
        "press:ctrl-p;{}key:enter;{}key:enter;dump;{dialog}key:enter;",
        presses("Merge Branch into Current"),
        presses(branch)
    )
}

/// Tab to the Merge dialog's checkbox `n` (0 = --no-ff) and tick it.
fn tick(n: usize) -> String {
    format!("{}press:space;", "press:tab;".repeat(n + 1))
}

/// A palette command run by its label.
fn palette(label: &str) -> String {
    format!("press:ctrl-p;{}key:enter;", presses(label))
}

/// No error in the status line.
fn no_error(state: &str, out: &str) {
    assert!(!state.contains("status=\"ERROR"), "{out}");
}

/// The status line's text of a `state` line.
fn status_of(state: &str) -> &str {
    state.split(" status=\"").nth(1).and_then(|s| s.split("\" commit_offset=").next()).unwrap_or("")
}

/// The working directory's first conflicted file: ⏎ (the diff), j (its header).
const FIRST_CONFLICT: &str = "press:enter;press:j;";

/// MG-02: the Merge dialog (its options listed), a message typed, --no-ff ticked: a merge
/// commit with that message.
/// Workflow: MG-02
#[gitti_test::test]
fn mg02_no_ff_merge_with_a_message() {
    use mg::ff as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, &format!("{}state", merge(wf::TARGET, &format!("{}{}", presses(wf::MESSAGE), tick(0)))));
    let screen = screens(&out)[0];
    assert!(screen.contains("Merge feat into main"), "{out}");
    for label in gitti_core::ops::MERGE_OPTION_LABELS {
        let short = label.split(" — ").next().unwrap();
        assert!(screen.contains(short), "{short} not in the dialog: {out}");
    }
    no_error(states(&out)[0], &out);
    wf::expect_merge_commit(&d, &s).unwrap();
}

/// MG-03: the Merge dialog with --squash: f.txt staged, git's squash message in the box,
/// no merge in progress; ctrl-s commits it as a one-parent commit.
/// Workflow: MG-03
#[gitti_test::test]
fn mg03_squash_merge_then_commit() {
    use mg::diverged as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, &format!("{}state;dump;press:c;key:ctrl-s;state", merge(wf::TARGET, &tick(2))));
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=1 repo_state=clean"), "{out}");
    no_error(st[0], &out);
    assert!(screens(&out)[1].contains(wf::SQUASHED), "git's squash message in the box: {out}");
    no_error(st[1], &out);
    wf::expect_squash_committed(&d, &s).unwrap();
}

/// MG-04: feat's branch menu (m) ▸ Merge feat into main… (M), --ff-only ticked: refused in
/// plain words, nothing changed.
/// Workflow: MG-04
#[gitti_test::test]
fn mg04_fast_forward_only_on_diverged_branches() {
    use mg::diverged as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, &format!("press:backtab;press:g;press:j;press:m;dump;press:M;{}key:enter;state", tick(1)));
    assert!(screens(&out)[0].contains("Merge feat into main…"), "{out}");
    let st = states(&out)[0];
    let want = gitti_core::ops::not_fast_forward("main", wf::TARGET);
    assert!(st.contains("status=\"ERROR ") && status_of(st).ends_with(&want), "{out}");
    wf::expect_unchanged(&d, &s).unwrap();
}

/// MG-05: the Merge dialog with the default options on diverged branches: a merge commit.
/// Workflow: MG-05
#[gitti_test::test]
fn mg05_clean_three_way_merge() {
    use mg::diverged as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, &format!("{}state", merge(wf::TARGET, "")));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    wf::expect_merged(&d, &s).unwrap();
}

/// MG-07 (first half): a.txt and b.txt conflicted; a.txt's resolve menu ▸ Take Ours —
/// main (o), then b.txt's (now the first) ▸ Take Theirs — feat (t).
/// Workflow: MG-07
#[gitti_test::test]
fn mg07_take_a_side_per_file() {
    use mg::sides as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    wf::expect_stopped(&d, &s).unwrap();
    let out = run(&d, &format!("{FIRST_CONFLICT}press:enter;dump;key:o;state;press:enter;key:t;state"));
    let screen = screens(&out)[0];
    assert!(screen.contains("Take Ours — main") && screen.contains("Take Theirs — feat"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("repo_state=merging conflicted=1"), "{out}");
    assert!(st[1].contains("repo_state=merging conflicted=0"), "{out}");
    no_error(st[1], &out);
    wf::expect_resolved(&d, &s).unwrap();
}

/// MG-07 (second half): both resolved; the palette's Continue Merging concludes the merge.
/// Workflow: MG-07
#[gitti_test::test]
fn mg07_continue_after_taking_sides() {
    use mg::sides as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture_resolved(&d);
    wf::expect_resolved(&d, &s).unwrap();
    let out = run(&d, &format!("{}state", palette("Continue Merging")));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    wf::expect_concluded(&d, &s).unwrap();
}

/// MG-09 (first half): both sides renamed r.txt: feat-name.txt, main-name.txt and r.txt
/// conflicted, each with its kind; keep feat-name.txt (k), delete main-name.txt (d).
/// Workflow: MG-09
#[gitti_test::test]
fn mg09_both_renamed_keep_theirs_name() {
    use mg::rename_rename as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    wf::expect_stopped(&d, &s).unwrap();
    let out = run(&d, &format!("{FIRST_CONFLICT}dump;press:enter;key:k;press:enter;key:d;state"));
    let screen = screens(&out)[0];
    assert!(screen.contains("CONFLICTED FILES (3)"), "{out}");
    assert!(screen.contains("only in feat") && screen.contains("only in main"), "{out}");
    assert!(screen.contains("deleted on both sides"), "{out}");
    let st = states(&out)[0];
    assert!(st.contains("repo_state=merging conflicted=1"), "{out}");
    no_error(st, &out);
    wf::expect_kept(&d, &s).unwrap();
}

/// MG-09 (second half): feat's name kept, main's deleted; r.txt's menu explains it was
/// deleted on both sides, d deletes it; Continue Merging: feat's name only.
/// Workflow: MG-09
#[gitti_test::test]
fn mg09_delete_the_old_name_and_continue() {
    use mg::rename_rename as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture_kept(&d);
    wf::expect_kept(&d, &s).unwrap();
    let out = run(&d, &format!("{FIRST_CONFLICT}press:enter;dump;key:d;{}state", palette("Continue Merging")));
    assert!(screens(&out)[0].contains("Delete the File — as both did"), "{out}");
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    wf::expect_concluded(&d, &s).unwrap();
}

/// MG-10: the palette's Abort Merging: no merge, main's files back.
/// Workflow: MG-10
#[gitti_test::test]
fn mg10_abort_a_conflicting_merge() {
    use mg::sides as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, &format!("{}state", palette("Abort Merging")));
    let st = states(&out)[0];
    assert!(st.contains("unstaged=0 staged=0 repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    wf::expect_aborted(&d, &s).unwrap();
}

/// MG-12: other has its own root: git refuses the merge, gitty offers Merge anyway; ⏎
/// merges both histories.
/// Workflow: MG-12
#[gitti_test::test]
fn mg12_unrelated_histories_merge_anyway() {
    use mg::unrelated as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, &format!("{}dump;key:enter;state", merge(wf::TARGET, "")));
    let screen = screens(&out)[1];
    assert!(screen.contains("Unrelated histories") && screen.contains("Merge anyway"), "{out}");
    no_error(states(&out)[0], &out);
    wf::expect_merged(&d, &s).unwrap();
}

/// MG-17: notes.txt changed; the merge stops on a.txt (said as a stop, not an error), the
/// change listed; Abort Merging keeps it.
/// Workflow: MG-17
#[gitti_test::test]
fn mg17_dirty_tree_survives_merge_and_abort() {
    use mg::dirty_apart as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, &format!("{}state;{}state", merge(wf::TARGET, ""), palette("Abort Merging")));
    let st = states(&out);
    assert!(st[0].contains("unstaged=1 staged=0 repo_state=merging conflicted=1"), "{out}");
    assert!(status_of(st[0]).contains("Merge stopped on 1 conflict"), "{out}");
    assert!(st[1].contains("unstaged=1 staged=0 repo_state=clean conflicted=0"), "{out}");
    no_error(st[1], &out);
    wf::expect_aborted(&d, &s).unwrap();
}

/// MG-20: gitty on the linked worktree, where the merge stopped: merging, a.txt
/// conflicted; Take Theirs (t), Continue Merging: concluded there, the main worktree
/// untouched.
/// Workflow: MG-20
#[gitti_test::test]
fn mg20_merge_conflict_in_a_linked_worktree() {
    use mg::worktree as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    wf::expect_stopped(&d, &s).unwrap();
    let out = run(&s.linked, &format!("state;{FIRST_CONFLICT}press:enter;key:t;{}state;dump", palette("Continue Merging")));
    let st = states(&out);
    assert!(st[0].contains("repo_state=merging conflicted=1"), "{out}");
    assert!(st[1].contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st[1], &out);
    assert!(screens(&out)[0].contains("⎇ topic"), "{out}");
    wf::expect_concluded(&d, &s).unwrap();
}

/// MG-21: both sides moved a submodule's pointer: its header says submodule, its rows ours
/// → theirs; its resolve menu explains and offers neither merge editor nor external merge
/// tool; Take Theirs (t) records feat's commit, Continue Merging.
/// Workflow: MG-21
#[gitti_test::test]
fn mg21_submodule_pointer_conflict_take_theirs() {
    use mg::submodule as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    wf::expect_stopped(&d, &s).unwrap();
    let out = run(&d, &format!("{FIRST_CONFLICT}dump;press:enter;dump;key:t;state;{}state", palette("Continue Merging")));
    let sc = screens(&out);
    assert!(sc[0].contains("submodule, moved on both sides"), "{out}");
    assert!(sc[0].contains(&format!("Submodule {}: {} → {}", wf::PATH, &s.lib_main[..7], &s.lib_feat[..7])), "{out}");
    assert!(sc[0].contains(wf::LIB_FEAT) && sc[0].contains(wf::LIB_MAIN), "{out}");
    assert!(sc[1].contains("Take Theirs — feat") && sc[1].contains("no merge editor"), "{out}");
    assert!(!sc[1].contains("Open in Merge Editor") && !sc[1].contains("External Merge Tool"), "{out}");
    let st = states(&out);
    assert!(status_of(st[0]).contains("Recorded their commit"), "{out}");
    assert!(st[1].contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st[1], &out);
    wf::expect_concluded(&d, &s).unwrap();
}

/// MG-22: a binary conflict: its header says binary, its row the binary note; its resolve
/// menu explains and offers no merge editor; Take Theirs (t), Continue Merging.
/// Workflow: MG-22
#[gitti_test::test]
fn mg22_binary_conflict_take_theirs() {
    use mg::binary as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    wf::expect_stopped(&d, &s).unwrap();
    let out = run(&d, &format!("{FIRST_CONFLICT}dump;press:enter;dump;key:t;{}state", palette("Continue Merging")));
    let sc = screens(&out);
    assert!(sc[0].contains("binary, changed on both sides"), "{out}");
    assert!(sc[0].contains("Binary file changed on both sides"), "{out}");
    assert!(sc[1].contains("Take Theirs — feat") && sc[1].contains("no merge editor"), "{out}");
    assert!(!sc[1].contains("Open in Merge Editor"), "{out}");
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    wf::expect_concluded(&d, &s).unwrap();
}

/// MG-27: a text file the nested .gitattributes marks `binary`: shown as a binary
/// conflict (header, note); its resolve menu says why — .gitattributes — and offers no
/// merge editor; Take Ours (o), Continue Merging.
/// Workflow: MG-27
#[gitti_test::test]
fn mg27_conflict_in_a_file_gitattributes_marks_binary() {
    use mg::binary_attr as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    wf::expect_stopped(&d, &s).unwrap();
    let out = run(&d, &format!("{FIRST_CONFLICT}dump;press:enter;dump;key:o;{}state", palette("Continue Merging")));
    let sc = screens(&out);
    // The header is cut at 120 columns.
    assert!(sc[0].contains("data/table.csv  -0 +0  binary, changed on both"), "{out}");
    assert!(sc[0].contains("Binary file changed on both sides"), "{out}");
    assert!(sc[1].contains("Take Ours — main") && sc[1].contains(".gitattributes"), "{out}");
    assert!(!sc[1].contains("Open in Merge Editor"), "{out}");
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    wf::expect_concluded(&d, &s).unwrap();
}

/// RN-05: the Merge dialog on feat, which renamed a.txt main edited: merged without a
/// conflict, the edit in b.txt.
/// Workflow: RN-05
#[gitti_test::test]
fn rn05_merge_across_a_rename() {
    use mg::across_rename as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, &format!("{}state", merge(wf::TARGET, "")));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    wf::expect_merged(&d, &s).unwrap();
}
