//! Workflow tests of staging and discarding (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! ST, DC, BG-04), driven through gitty: the workflow's fixture (`gitti_workflow::{st, dc,
//! bg}`), one headless gitty run with the user's keys, `state` / `dump` / `cmds` checks of
//! what gitty shows and runs, then the repository with the workflow's own `expect` —
//! shared with `tests/core/wf_staging.rs` and `tests/ui/wf_staging.rs`.

use crate::common::*;
use crate::support::*;

use gitti_workflow::{bg, dc, st};

/// ST-03: n n to the second staged hunk, j × 5 to `new 2`, space selects it, u unstages
/// it; the first hunk and `new 1` stay staged.
/// Workflow: ST-03
#[gitti_test::test]
fn st03_unstage_lines_beside_a_staged_hunk() {
    use st::unstage_lines_beside_a_staged_hunk as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;press:n;press:n;press:j;press:j;press:j;press:j;press:j;press:space;state;key:u;state");
    let st = states(&out);
    assert!(st[0].contains("Line { src: Staged, file: 0, hunk: 1, line: 4"), "{out}");
    assert_eq!(num(st[0], "sel_lines"), 1, "{out}");
    assert!(st[0].contains("unstaged=0 staged=1"), "{out}");
    assert!(st[1].contains("unstaged=1 staged=1"), "{out}");
    assert!(!st[1].contains("status=\"ERROR"), "{out}");
    wf::expect(d).unwrap();
}

/// ST-04: n to the untracked file's hunk, j j to `n 2`, space + J select `n 2` and `n 3`,
/// s stages them.
/// Workflow: ST-04
#[gitti_test::test]
fn st04_partial_stage_of_a_new_file() {
    use st::partial_stage_of_a_new_file as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;press:n;press:j;press:j;press:space;press:J;state;key:s;state");
    let st = states(&out);
    assert!(st[0].contains("Line { src: Unstaged, file: 0, hunk: 0, line: 2"), "{out}");
    assert_eq!(num(st[0], "sel_lines"), 2, "{out}");
    assert!(st[1].contains("unstaged=1 staged=1"), "{out}");
    assert!(!st[1].contains("status=\"ERROR"), "{out}");
    wf::expect(d).unwrap();
}

/// ST-05: n to the staged new file's hunk, j j, space + J select `n 2` and `n 3`, u
/// unstages them.
/// Workflow: ST-05
#[gitti_test::test]
fn st05_partial_unstage_of_an_added_file() {
    use st::partial_unstage_of_an_added_file as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;press:n;press:j;press:j;press:space;press:J;state;key:u;state");
    let st = states(&out);
    assert!(st[0].contains("Line { src: Staged, file: 0, hunk: 0, line: 2"), "{out}");
    assert_eq!(num(st[0], "sel_lines"), 2, "{out}");
    assert!(st[1].contains("unstaged=1 staged=1"), "{out}");
    assert!(!st[1].contains("status=\"ERROR"), "{out}");
    wf::expect(d).unwrap();
}

/// ST-06: n to the deleted file's hunk, j j, space + J select `-g 2` and `-g 3`, s
/// stages them.
/// Workflow: ST-06
#[gitti_test::test]
fn st06_partial_stage_of_a_deletion() {
    use st::partial_stage_of_a_deletion as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;press:n;press:j;press:j;press:space;press:J;state;key:s;state");
    let st = states(&out);
    assert!(st[0].contains("Line { src: Unstaged, file: 0, hunk: 0, line: 2"), "{out}");
    assert_eq!(num(st[0], "sel_lines"), 2, "{out}");
    assert!(st[1].contains("unstaged=1 staged=1"), "{out}");
    assert!(!st[1].contains("status=\"ERROR"), "{out}");
    wf::expect(d).unwrap();
}

/// ST-10: W hides whitespace changes — the hunk shows only line 3's change; n, s stages
/// the hunk.
/// Workflow: ST-10
#[gitti_test::test]
fn st10_stage_ignoring_whitespace() {
    use st::stage_ignoring_whitespace as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;key:W;press:n;dump;key:s;state");
    let screen = screens(&out)[0];
    assert!(screen.contains("line THREE") && !screen.contains("line 5  "), "{out}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=1 staged=1"), "{out}");
    assert!(!st[0].contains("status=\"ERROR"), "{out}");
    wf::expect(d).unwrap();
}

/// ST-12: - - - to no context lines (two hunks), n, s stages the first.
/// Workflow: ST-12
#[gitti_test::test]
fn st12_stage_with_minimal_context() {
    use st::stage_with_minimal_context as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;key:-;key:-;key:-;state;press:n;press:n;state;press:g;press:n;key:s;state");
    let st = states(&out);
    assert!(st[0].contains("status=\"Context lines: 0\""), "{out}");
    assert!(st[1].contains("Hunk { src: Unstaged, file: 0, hunk: 1 }"), "two hunks: {out}");
    assert!(st[2].contains("unstaged=1 staged=1"), "{out}");
    assert!(!st[2].contains("status=\"ERROR"), "{out}");
    wf::expect(d).unwrap();
}

/// DC-02: n n to the second hunk; d asks, y discards it.
/// Workflow: DC-02
#[gitti_test::test]
fn dc02_discard_a_hunk() {
    use dc::discard_a_hunk as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;press:n;press:n;state;press:d;dump;key:y;state");
    let st = states(&out);
    assert!(st[0].contains("Hunk { src: Unstaged, file: 0, hunk: 1 }"), "{out}");
    assert!(screens(&out)[0].contains("Discard"), "asks first: {out}");
    assert!(st[1].contains("unstaged=1 staged=0"), "{out}");
    assert!(!st[1].contains("status=\"ERROR"), "{out}");
    wf::expect(d).unwrap();
}

/// DC-08: ] to the deleted file's header, d asks, y brings it back; then the untracked
/// file's header, d, y deletes it.
/// Workflow: DC-08
#[gitti_test::test]
fn dc08_discard_a_deleted_and_an_untracked_file() {
    use dc::discard_whole_files as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;press:];state;press:d;dump;key:y;state;press:g;press:];state;press:d;dump;key:y;state");
    let st = states(&out);
    let sc = screens(&out);
    assert!(st[0].contains("File { src: Unstaged, file: 0 }"), "{out}");
    assert!(sc[0].contains("gone.txt"), "{out}");
    // gone.txt is back: only new.txt is left.
    assert!(st[1].contains("unstaged=1 staged=0"), "{out}");
    assert!(st[2].contains("File { src: Unstaged, file: 0 }"), "{out}");
    assert!(sc[1].contains("new.txt"), "{out}");
    assert!(st[3].contains("unstaged=0 staged=0"), "{out}");
    assert!(!st[3].contains("status=\"ERROR"), "{out}");
    wf::expect(d).unwrap();
}

/// BG-04: j to the Working Directory group, ⏎ folds it, j to Untracked Files (250), s
/// stages the group: two `git add` (200 + 50 paths) and a path-limited re-read.
/// Workflow: BG-04
#[gitti_test::test]
fn bg04_stage_a_group_of_250_paths() {
    use bg::stage_a_big_group as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;press:j;press:enter;press:j;state;cmds;key:s;state;cmds");
    let st = states(&out);
    assert!(st[0].contains("Group(Untracked)"), "{out}");
    assert_eq!(num(st[0], "unstaged"), wf::COUNT + 1, "{out}");
    assert!(st[1].contains("unstaged=1 staged=250"), "{out}");
    assert!(!st[1].contains("status=\"ERROR"), "{out}");
    let blocks = cmds(&out);
    let adds: Vec<&String> = blocks[1].iter().filter(|c| c.starts_with("add -A -- ")).collect();
    assert_eq!(adds.len(), 2, "chunks of 200: {out}");
    wf::expect(d).unwrap();
}
