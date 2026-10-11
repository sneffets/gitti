//! Workflow tests, the pilots (catalog: `crates/gitti-workflow/WORKFLOWS.md`), driven
//! through gitty: the workflow's fixture (`gitti_workflow`), one headless gitty run with
//! the user's keys, `state` / `dump` checks of what gitty shows, then the repository with
//! the workflow's own `expect` — shared with `tests/core/wf_pilot.rs` and
//! `tests/ui/wf_pilot.rs`.

use crate::common::*;
use crate::support::*;

use gitti_workflow::{cp, st};

/// ST-13, the first commit: n / s stage the first hunk of `a.txt`; g ] ] n j j j j to the
/// first added line of `b.txt`, space selects it, s stages it; c, the message, alt-⏎
/// commits.
#[gitti_test::test]
fn st13_split_a_change_into_two_commits() {
    use st::split_into_two_commits as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let script = format!(
        "press:enter;press:n;state;key:s;state;press:g;press:];press:];press:n;press:j;press:j;press:j;press:j;\
         press:space;state;key:s;state;press:c;{}key:alt-enter;state",
        presses(wf::FIRST),
    );
    let out = gitty(d, &script);
    let st = states(&out);
    assert!(st[0].contains("Hunk { src: Unstaged, file: 0, hunk: 0 }"), "{out}");
    assert!(st[1].contains("unstaged=2 staged=1"), "{out}");
    assert!(st[2].contains("Line { src: Unstaged, file: 1, hunk: 0, line: 3"), "the first added line of b.txt: {out}");
    assert_eq!(num(st[2], "sel_lines"), 1, "{out}");
    assert!(st[3].contains("unstaged=2 staged=2"), "{out}");
    assert!(st[4].contains("unstaged=2 staged=0 repo_state=clean"), "after the first commit: {out}");
    wf::expect_first(d).unwrap();
}

/// ST-13, the second commit (from the state after the first): S stages the rest; c, the
/// message, alt-⏎ commits.
#[gitti_test::test]
fn st13_split_a_change_second_commit() {
    use st::split_into_two_commits as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_first_committed(d);
    wf::expect_first(d).unwrap();
    let out = gitty(d, &format!("key:S;state;press:c;{}key:alt-enter;state", presses(wf::SECOND)));
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=2"), "{out}");
    assert!(st[1].contains("unstaged=0 staged=0 repo_state=clean"), "at the end: {out}");
    wf::expect(d).unwrap();
}

/// CP-12: j to `Fix a`, . opens the commit menu, c cherry-picks; ctrl-p "merge", ⏎,
/// the picker filtered to `feat`, ⏎ merges.
#[gitti_test::test]
fn cp12_pick_then_merge_the_source_branch() {
    use cp::pick_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let script = format!(
        "press:j;press:j;press:j;state;press:.;dump;key:c;state;press:ctrl-p;{}key:enter;{}key:enter;state",
        presses("merge"),
        presses("feat"),
    );
    let out = gitty(d, &script);
    let st = states(&out);
    assert!(st[0].contains(&format!("[{} {}]", &s.fix[..7], cp::FIX)), "the commit to pick: {out}");
    assert!(screens(&out)[0].contains(&format!("Cherry-pick {}", &s.fix[..7])), "{out}");
    assert!(st[1].contains("repo_state=clean conflicted=0"), "{out}");
    assert!(!st[1].contains("status=\"ERROR"), "{out}");
    assert!(st[2].contains("repo_state=clean conflicted=0"), "{out}");
    wf::expect(d, &s).unwrap();
}

/// CP-13: the merge of `feat` stops: gitty shows the merge in progress with one
/// conflicted file and `b.txt` staged.
#[gitti_test::test]
fn cp13_pick_edit_then_merge_conflicts_there() {
    use cp::pick_edit_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let script = format!("press:ctrl-p;{}key:enter;{}key:enter;state;dump", presses("merge"), presses("feat"));
    let out = gitty(d, &script);
    let st = states(&out);
    assert!(st[0].contains("staged=1 repo_state=merging conflicted=1"), "{out}");
    // TODO (catalog: suspected bug "merge conflict reported without git's message"): the
    // status line should name the conflict; today it is "ERROR Merge failed: git merge
    // --no-edit feat: git ended with exit code 1, without a message" (git prints the
    // CONFLICT lines on stdout). Assert it once that is fixed.
    wf::expect(d, &s).unwrap();
}
