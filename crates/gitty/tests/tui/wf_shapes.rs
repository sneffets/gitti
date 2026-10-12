//! Workflow tests of file shapes (catalog: `crates/gitti-workflow/WORKFLOWS.md`, RN, BN,
//! BG, NU, FS, IG, CM-20), driven through gitty: one repository per shape group
//! (`gitti_workflow::shapes`), headless gitty runs with the user's keys — Go to File (t),
//! space to pick lines or mark files, s / u / d on lines, hunks, files and groups —
//! `state` / `dump` / `cmds` checks of what gitty shows and runs, then the repository with
//! the workflow's own `expect`, shared with `tests/core/wf_shapes.rs` and
//! `tests/ui/wf_shapes.rs`.
//!
//! Every write costs a gitty run its settle time, so each test does at most three; a
//! workflow with more steps is split over two runs (the `expect` of the first step
//! between them).

use std::path::Path;

use crate::common::*;
use crate::support::*;

use gitti_workflow::shapes;

/// One gitty run (120x40).
fn run(d: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    gitty_with(d, config.path(), "120x40", script)
}

/// Go to File (t) `name`, ⏎: the cursor on its first header row.
fn goto(name: &str) -> String {
    format!("press:t;type:{name};key:enter;")
}

/// `key` `n` times, without waiting.
fn times(key: &str, n: usize) -> String {
    format!("press:{key};").repeat(n)
}

/// No error in the status line of `state`.
fn no_error(state: &str, out: &str) {
    assert!(!state.contains("status=\"ERROR"), "{out}");
}

/// RN-01: t old.txt, space marks it, t new.txt, space, s stages both (one `git add`): the
/// staged section shows the rename's halves; a second run's U unstages them.
/// Workflow: RN-01
#[gitti_test::test]
fn rn01_stage_and_unstage_both_halves_of_a_rename() {
    use shapes::renames as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let script = format!("key:enter;{}press:space;{}press:space;key:s;state;dump", goto(wf::OLD), goto(wf::NEW));
    let out = run(d, &script);
    let st = states(&out);
    assert!(st[0].contains("unstaged=2 staged=2"), "{out}");
    no_error(st[0], &out);
    let screen = screens(&out)[0];
    assert!(screen.contains("R new.txt") && screen.contains("D old.txt"), "{out}");
    wf::expect_rn01_staged(d).unwrap();
    let out = run(d, "key:enter;key:U;state");
    let st = states(&out);
    assert!(st[0].contains("unstaged=4 staged=0"), "{out}");
    no_error(st[0], &out);
    wf::expect_rn01_unstaged(d).unwrap();
}

/// RN-02: t final.txt, j j to its first line, space + J × 10 pick the draft's lines and
/// `extra 1`, s stages them; t draft.txt, s stages the deletion: a rename.
/// Workflow: RN-02
#[gitti_test::test]
fn rn02_stage_lines_of_a_renamed_file_then_its_deletion() {
    use shapes::renames as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let script = format!(
        "key:enter;{}press:j;press:j;press:space;{}state;key:s;{}key:s;state",
        goto(wf::FINAL),
        times("J", 10),
        goto(wf::DRAFT)
    );
    let out = run(d, &script);
    let st = states(&out);
    assert_eq!(num(st[0], "sel_lines"), 11, "{out}");
    no_error(st[1], &out);
    wf::expect_rn02(d).unwrap();
}

/// RN-04: j selects the rename commit: its file shows as `a.txt → b.txt`.
/// Workflow: RN-04
#[gitti_test::test]
fn rn04_a_commit_that_renames() {
    use shapes::renames as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, "key:j;state;dump");
    assert!(states(&out)[0].contains(wf::RENAME_SUBJECT), "{out}");
    assert!(screens(&out)[0].contains(wf::SHOWN), "{out}");
}

/// BN-01: bin.dat and new.bin show "Binary file", no lines; the line X of a.txt beside
/// them picked (space) and staged (s).
/// Workflow: BN-01
#[gitti_test::test]
fn bn01_stage_a_line_beside_binary_files() {
    use shapes::binary as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, &format!("key:enter;dump;{}{}press:space;state;key:s;state", goto(wf::TEXT), times("j", 3)));
    let st = states(&out);
    assert_eq!(num(st[0], "sel_lines"), 1, "{out}");
    no_error(st[1], &out);
    let s = screens(&out)[0];
    assert!(s.contains("M bin.dat") && s.contains("? new.bin") && s.contains("Binary file"), "{out}");
    wf::expect_bn01_line(d).unwrap();
}

/// BN-01: bin.dat and new.bin marked (space on their headers) and staged together (s):
/// whole, as git adds them.
/// Workflow: BN-01
#[gitti_test::test]
fn bn01_stage_binary_files() {
    use shapes::binary as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, &format!("key:enter;{}press:space;{}press:space;key:s;state;dump", goto(wf::BIN), goto(wf::NEW_BIN)));
    no_error(states(&out)[0], &out);
    assert!(screens(&out)[0].contains("STAGED FILES (3)"), "bin.dat, new.bin, flip.txt: {out}");
    wf::expect_bn01_binaries(d).unwrap();
}

/// BN-01: a staged bin.dat unstaged (u on its header).
/// Workflow: BN-01
#[gitti_test::test]
fn bn01_unstage_a_binary_file() {
    use shapes::binary as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    gitti_workflow::git::git(d, &["add", wf::BIN]);
    let out = run(d, &format!("key:enter;{}key:u;state", goto(wf::BIN)));
    no_error(states(&out)[0], &out);
    wf::expect_bn01_unstaged(d).unwrap();
}

/// BN-01: bin.dat discarded (d on its header asks, y): the committed bytes again.
/// Workflow: BN-01
#[gitti_test::test]
fn bn01_discard_a_binary_file() {
    use shapes::binary as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, &format!("key:enter;{}press:d;dump;key:y;state", goto(wf::BIN)));
    assert!(screens(&out)[0].contains("Discard"), "asks first: {out}");
    no_error(states(&out)[0], &out);
    wf::expect_bn01_discarded_only(d).unwrap();
}

/// BN-02: flip.txt — binary staged, text again in the working tree — shows "Binary file"
/// in both sections; s on its header stages it: the staged diff is text.
/// Workflow: BN-02
#[gitti_test::test]
fn bn02_text_becomes_binary_and_back() {
    use shapes::binary as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, &format!("key:enter;{}dump;key:s;state;dump", goto(wf::FLIP)));
    let s = screens(&out);
    // The rows under each `M flip.txt` header.
    let under = |screen: &str| -> Vec<String> {
        let lines: Vec<&str> = screen.lines().collect();
        (0..lines.len())
            .filter(|&i| lines[i].contains("M flip.txt"))
            .flat_map(|i| lines[i + 1..(i + 3).min(lines.len())].iter().map(|l| l.to_string()))
            .collect()
    };
    let binary = |rows: &[String]| rows.iter().filter(|l| l.contains("Binary file")).count();
    assert_eq!(binary(&under(s[0])), 2, "binary in both sections: {out}");
    assert_eq!(binary(&under(s[1])), 0, "text once staged: {out}");
    assert!(s[1].contains("+ again"), "{out}");
    no_error(states(&out)[0], &out);
    wf::expect_bn02_staged(d).unwrap();
}

/// BN-02: flip.txt's text staged after its binary version: the staged diff's line
/// `again` picked and unstaged (u).
/// Workflow: BN-02
#[gitti_test::test]
fn bn02_unstage_a_line_of_the_text_again() {
    use shapes::binary as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_flip_staged(d);
    let out = run(d, &format!("key:enter;{}{}press:space;state;key:u;state", goto(wf::FLIP), times("j", 3)));
    let st = states(&out);
    assert!(st[0].contains("Line { src: Staged") && num(st[0], "sel_lines") == 1, "{out}");
    no_error(st[1], &out);
    wf::expect_bn02(d).unwrap();
}

/// BG-01: j selects the merge of 1 700 files: one `git show` of it, its files listed; j
/// to its parent `side` (as big, prefetched as the merge's neighbour) doesn't load the
/// merge again, and `side` once in all.
/// Workflow: BG-01
#[gitti_test::test]
fn bg01_a_huge_merge_loads_once() {
    use shapes::huge_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let (merge, side) = (&s.ids[s.merge], &s.ids[s.side]);
    let out = run(d, "cmds;key:j;cmds;state;dump;key:j;cmds;state");
    let blocks = cmds(&out);
    let loads_of = |id: &str, b: &[String]| b.iter().filter(|c| c.starts_with("show") && c.contains(id)).count();
    let loads = |b: &[String]| loads_of(merge, b);
    assert_eq!(loads(&blocks[0]) + loads(&blocks[1]), 1, "one load: {out}");
    assert_eq!(loads(&blocks[2]), 0, "not loaded again: {out}");
    assert_eq!(blocks.iter().map(|b| loads_of(side, b)).sum::<usize>(), 1, "side once: {out}");
    let st = states(&out);
    assert!(st[0].contains(wf::MERGE_SUBJECT), "{out}");
    assert!(screens(&out)[0].contains(&wf::name(0)), "{out}");
    assert!(st[1].contains("Add many files"), "{out}");
}

/// BG-02: the 200 000-character line's `-` / `+` pair picked (space, J) and staged (s).
/// Workflow: BG-02
#[gitti_test::test]
fn bg02_stage_a_very_long_line() {
    use shapes::long_lines as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    // Header, hunk, `short`, then the pair.
    let out = run(d, &format!("key:enter;{}{}press:space;press:J;state;key:s;state", goto(wf::PATH), times("j", 3)));
    let st = states(&out);
    assert_eq!(num(st[0], "sel_lines"), 2, "{out}");
    no_error(st[1], &out);
    wf::expect_staged(d).unwrap();
}

/// BG-02: the line added after the long ones picked and discarded (d, y).
/// Workflow: BG-02
#[gitti_test::test]
fn bg02_discard_beside_very_long_lines() {
    use shapes::long_lines as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    // Header, hunk, `short`, the pair, `end`, then `+added`.
    let out = run(d, &format!("key:enter;{}{}press:space;state;press:d;key:y;state", goto(wf::PATH), times("j", 6)));
    let st = states(&out);
    assert_eq!(num(st[0], "sel_lines"), 1, "{out}");
    no_error(st[1], &out);
    wf::expect_discarded(d).unwrap();
}

/// BG-03: 5 000 commits — G jumps to the oldest one: listed, loaded in full, its diff
/// shown.
/// Workflow: BG-03
#[gitti_test::test]
fn bg03_a_long_history() {
    use shapes::long_history as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, "key:G;state;dump");
    let st = states(&out);
    assert_eq!(num(st[0], "history_rows"), wf::COUNT + 1, "the worktree row and every commit: {out}");
    assert!(st[0].contains("history_load=Full"), "{out}");
    assert!(st[0].contains(&format!("{}]", wf::subject(1))), "{out}");
    assert!(screens(&out)[0].contains(wf::PATH), "{out}");
    wf::expect(d).unwrap();
}

/// NU-02: UTF-16 shown as text (a `diff` attribute): `ä x` picked and staged, `中 y`
/// discarded (d, y).
/// Workflow: NU-02
#[gitti_test::test]
fn nu02_utf16_stage_and_discard_lines() {
    use shapes::utf16 as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    // Header, hunk, a, b, then the two added lines; after the stage `ä x` is context.
    let script = format!(
        "key:enter;{}dump;{}press:space;state;key:s;{}{}press:space;state;press:d;key:y;state",
        goto(wf::PATH),
        times("j", 4),
        goto(wf::PATH),
        times("j", 5)
    );
    let out = run(d, &script);
    assert!(screens(&out)[0].contains("UTF-16"), "the encoding named: {out}");
    let st = states(&out);
    assert_eq!(num(st[0], "sel_lines"), 1, "{out}");
    assert_eq!(num(st[1], "sel_lines"), 1, "{out}");
    no_error(st[2], &out);
    wf::expect_staged(d).unwrap();
}

/// NU-02: the staged `ä x` picked in the staged diff and unstaged (u).
/// Workflow: NU-02
#[gitti_test::test]
fn nu02_utf16_unstage_a_line() {
    use shapes::utf16 as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_staged(d);
    let out = run(d, &format!("key:enter;{}{}press:space;state;key:u;state", goto(wf::PATH), times("j", 4)));
    let st = states(&out);
    assert!(st[0].contains("Line { src: Staged"), "{out}");
    no_error(st[1], &out);
    wf::expect_unstaged(d).unwrap();
}

/// NU-05: the hunks of two odd names — on unix with a backslash and a newline in them,
/// else with a space and CJK — found with t and staged (n, s): their patches name them.
/// Workflow: NU-05
#[gitti_test::test]
fn nu05_hunks_of_odd_names() {
    use shapes::odd_names as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let (a, b) = if cfg!(unix) { ("back", "line") } else { ("with", "日本") };
    let out = run(d, &format!("key:enter;{}press:n;key:s;{}press:n;key:s;state", goto(a), goto(b)));
    let st = states(&out);
    assert!(st[0].contains("staged=2"), "{out}");
    no_error(st[0], &out);
    let names = wf::tracked();
    let picked: Vec<&str> = names.iter().copied().filter(|n| n.contains(a) || n.contains(b)).collect();
    assert_eq!(picked.len(), 2, "{picked:?}");
    wf::expect_lines_of(d, &picked).unwrap();
}

/// NU-05: every odd name by group — j to the tracked group, s; the untracked group, s
/// (`git add` of their paths).
/// Workflow: NU-05
#[gitti_test::test]
fn nu05_stage_odd_names_by_group() {
    use shapes::odd_names as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let n = wf::tracked().len();
    let out = run(d, "key:enter;press:j;state;key:s;press:g;press:j;state;key:s;state");
    let st = states(&out);
    assert!(st[0].contains("Group(Tracked)"), "{out}");
    assert!(st[1].contains("Group(Untracked)"), "{out}");
    assert!(st[2].contains(&format!("unstaged=0 staged={}", 2 * n)), "{out}");
    no_error(st[2], &out);
    wf::expect_all_staged(d).unwrap();
}

/// NU-05: every odd name staged; U unstages them all.
/// Workflow: NU-05
#[gitti_test::test]
fn nu05_unstage_odd_names() {
    use shapes::odd_names as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    gitti_workflow::git::git(d, &["add", "-A"]);
    wf::expect_all_staged(d).unwrap();
    let out = run(d, "key:enter;key:U;state");
    no_error(states(&out)[0], &out);
    wf::expect_nothing_staged(d).unwrap();
}

/// NU-05: every odd name discarded by group (d on the tracked group, y; on the untracked
/// group, y): a clean tree.
/// Workflow: NU-05
#[gitti_test::test]
fn nu05_discard_odd_names_by_group() {
    use shapes::odd_names as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, "key:enter;press:j;press:d;key:y;press:g;press:j;state;press:d;key:y;state");
    let st = states(&out);
    assert!(st[0].contains("Group(Untracked)"), "{out}");
    assert!(st[1].contains("unstaged=0 staged=0"), "{out}");
    no_error(st[1], &out);
    wf::expect_clean(d).unwrap();
}

/// NU-04 in the index only (unix; git keeps the name whatever the disk takes): the staged
/// `caf\xe9.txt` listed with U+FFFD for the byte, no "not UTF-8" note; ] to its staged
/// header, its line `three` picked (space) and unstaged (u) — the patch names the file by
/// its bytes.
/// Workflow: NU-04
#[cfg(unix)]
#[gitti_test::test]
fn nu04_unstage_a_line_of_a_name_that_is_not_utf8() {
    use shapes::non_utf8_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_index(d);
    let out = run(d, &format!("key:enter;dump;{}press:];press:n;{}press:space;state;key:u;state", goto("caf"), times("j", 4)));
    let screen = screens(&out)[0];
    assert!(screen.contains(wf::SHOWN), "{out}");
    assert!(!screen.contains(gitti_core::patch::NOT_UTF8_NOTE), "{out}");
    let st = states(&out);
    assert!(st[0].contains("Line { src: Staged") && num(st[0], "sel_lines") == 1, "{out}");
    no_error(st[1], &out);
    wf::expect_line_unstaged(d).unwrap();
}

/// NU-04 in the index only: s on the file's header stages it — its deletion —, u on the
/// staged header unstages it; git gets the name's bytes.
/// Workflow: NU-04
#[cfg(unix)]
#[gitti_test::test]
fn nu04_stage_and_unstage_a_name_that_is_not_utf8() {
    use shapes::non_utf8_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_index(d);
    let out = run(d, &format!("key:enter;{}key:s;state", goto("caf")));
    no_error(states(&out)[0], &out);
    wf::expect_deletion_staged(d).unwrap();
    let out = run(d, &format!("key:enter;{}key:u;state", goto("caf")));
    no_error(states(&out)[0], &out);
    wf::expect_deletion_unstaged(d).unwrap();
}

/// NU-04 on disk (where the file system takes such a name: Linux): the untracked
/// `caf\xe9.txt` listed; its line `one` picked (n to the hunk, j, space) and staged (s);
/// then s on its header stages the whole file, u unstages it, d (asks, y) deletes it.
/// Workflow: NU-04
#[cfg(unix)]
#[gitti_test::test]
fn nu04_a_name_that_is_not_utf8_on_disk() {
    use shapes::non_utf8_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    if !wf::fixture(d) {
        return; // this file system refuses such names (macOS)
    }
    let out = run(d, "key:enter;dump;press:n;press:j;press:space;state;key:s;state");
    assert!(screens(&out)[0].contains(wf::SHOWN), "{out}");
    let st = states(&out);
    assert_eq!(num(st[0], "sel_lines"), 1, "{out}");
    no_error(st[1], &out);
    wf::expect_line_staged(d).unwrap();
    let out = run(d, &format!("key:enter;{}key:s;state;{}key:u;state", goto("caf"), goto("caf")));
    let st = states(&out);
    no_error(st[0], &out);
    no_error(st[1], &out);
    wf::expect_unstaged(d).unwrap();
    let out = run(d, &format!("key:enter;{}press:d;key:y;state", goto("caf")));
    no_error(states(&out)[0], &out);
    wf::expect_discarded(d).unwrap();
}

/// NU-06 (unix): the commit that changed `caf\xe9.txt` — a name only commits have, its
/// bytes learned from the commit view — shown (row 2, ⏎): the file listed with U+FFFD;
/// V on its header opens a read-only copy of that version. Then `.` ▸ Restore This
/// Version… names it: r restores it where the disk takes the name (Linux), Esc elsewhere.
/// Workflow: NU-06
#[cfg(unix)]
#[gitti_test::test]
fn nu06_a_commits_file_whose_name_is_not_utf8() {
    use gitti_workflow::nu::history_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = &tmp.path().join("work");
    let s = wf::fixture(d);
    let versions = tmp.path().join("versions");
    let out = gitty_env(
        d,
        &tmp.path().join("config"),
        "120x40",
        &[("VISUAL", ""), ("EDITOR", ""), ("GITTI_VERSIONS_DIR", versions.to_str().unwrap())],
        "key:j;key:j;key:enter;dump;key:];key:V;state",
    );
    assert!(screens(&out)[0].contains(wf::SHOWN), "{out}");
    let st = states(&out);
    assert!(st[0].contains(&format!("Opened a read-only copy of {} at {}", wf::SHOWN, &s.change[..7])), "{out}");
    let copy = versions.join("work").join(&s.change[..10]).join(wf::SHOWN);
    assert_eq!(std::fs::read_to_string(&copy).ok().as_deref(), Some(wf::CHANGED), "{out}");
    let holds = wf::holds(d);
    let then = if holds { "key:r;state" } else { "key:esc;state" };
    let out = run(d, &format!("key:j;key:j;key:enter;key:];key:.;key:enter;dump;{then}"));
    assert!(screens(&out)[0].contains(&format!("Restore {} from {}", wf::SHOWN, &s.change[..7])), "{out}");
    no_error(states(&out)[0], &out);
    if holds {
        wf::expect_restored(d).unwrap();
    } else {
        wf::expect_unchanged(d).unwrap();
    }
}

/// NU-07 (where the disk takes the name: Linux): the line `three` of the edited
/// `caf\xe9.txt` picked (n to the hunk, j ×3, space) ▸ Stash Selected Lines (palette), ⏎.
/// Workflow: NU-07
#[cfg(unix)]
#[gitti_test::test]
fn nu07_stash_lines_of_a_name_that_is_not_utf8() {
    use gitti_workflow::nu::stash_lines_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    if !wf::fixture(d) {
        return; // this file system refuses such names (macOS)
    }
    let out = run(
        d,
        &format!(
            "key:enter;{}press:n;{}press:space;state;press:ctrl-p;type:stash selected lines;press:enter;dump;key:enter;state",
            goto("caf"),
            times("j", 3)
        ),
    );
    let st = states(&out);
    assert_eq!(num(st[0], "sel_lines"), 1, "{out}");
    assert!(screens(&out)[0].contains(&format!("Stash 1 Line of {}", wf::SHOWN)), "{out}");
    no_error(st[1], &out);
    wf::expect_stashed(d).unwrap();
}

/// NU-08 (unix): on the tracked `caf\xe9.txt` (deleted on disk) the file menu (m) ▸ Add
/// to .gitignore… (i) ▸ "This file" (1) `/caf�.txt`, Stop tracking ticked: the pattern
/// written in the name's bytes, the file out of the index.
/// Workflow: NU-08
#[cfg(unix)]
#[gitti_test::test]
fn nu08_ignore_a_name_that_is_not_utf8() {
    use gitti_workflow::nu::ignore_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(
        d,
        &format!("key:enter;{}press:m;press:i;dump;press:1;press:tab;press:tab;press:space;key:enter;state", goto("caf")),
    );
    let screen = screens(&out)[0];
    assert!(screen.contains("Ignore (tracked file)") && screen.contains(&wf::pattern()), "{out}");
    no_error(states(&out)[0], &out);
    wf::expect_ignored(d).unwrap();
}

/// FS-03 (where the disk keeps the names apart): Readme.md → README.md — a deletion and
/// an untracked file; both marked and staged (a rename), then unstaged (U).
/// Workflow: FS-03
#[gitti_test::test]
fn fs03_rename_only_in_case() {
    use shapes::case_and_form as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("repo");
    let (case, _) = wf::fixture(&d);
    if !case {
        return; // a case-insensitive disk keeps one file (macOS, Windows)
    }
    let out = run(&d, &format!("key:enter;{}press:space;{}press:space;key:s;state", goto(wf::BEFORE), goto(wf::AFTER)));
    no_error(states(&out)[0], &out);
    wf::expect_fs03_staged(&d).unwrap();
    let out = run(&d, "key:enter;key:U;state");
    no_error(states(&out)[0], &out);
    wf::expect_fs03_unstaged(&d).unwrap();
}

/// FS-04 (where the disk keeps NFC and NFD apart): the NFD file's hunk staged, the NFC
/// file's added line discarded.
/// Workflow: FS-04
#[gitti_test::test]
fn fs04_nfc_and_nfd_names() {
    use shapes::case_and_form as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("repo");
    let (_, form) = wf::fixture(&d);
    if !form {
        return; // the disk folds the two (macOS)
    }
    // Byte order: the NFD name (`e` + U+0301) before the NFC one (U+00E9).
    let out = run(&d, "key:enter;press:];press:n;key:s;press:g;press:];press:n;press:j;press:j;press:space;press:d;key:y;state");
    no_error(states(&out)[0], &out);
    wf::expect_fs04(&d).unwrap();
}

/// FS-05 / FS-06: the retargeted link, new_link and dangling — shown as their targets,
/// never what they point at, their file rows saying "symlink → target" — and real/x.txt marked (space) and staged (s). Nothing
/// beyond the `alias` symlink is listed.
/// Workflow: FS-05, FS-06
#[cfg(unix)]
#[gitti_test::test]
fn fs05_fs06_stage_symlinks() {
    use shapes::symlinks as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let script = format!(
        "key:enter;dump;{}press:space;{}press:space;{}press:space;{}press:space;key:s;state",
        goto(wf::LINK),
        goto(wf::NEW_LINK),
        goto(wf::DANGLING),
        goto(wf::BEYOND)
    );
    let out = run(d, &script);
    let screen = screens(&out)[0];
    assert!(screen.contains("+ target.txt") && screen.contains("+ nowhere"), "the links' targets: {out}");
    assert!(!screen.contains("alias/"), "nothing beyond the symlink: {out}");
    // The file rows say they are links, with their targets.
    for label in ["symlink → other.txt", "symlink → target.txt", "symlink → nowhere"] {
        assert!(screen.contains(label), "{label:?} on a file row: {out}");
    }
    let st = states(&out);
    assert!(st[0].contains("unstaged=1 staged=4"), "only gone's deletion left: {out}");
    no_error(st[0], &out);
    wf::expect_staged(d).unwrap();
}

/// FS-05: the deleted link's deletion discarded (d on its header, y): a link again.
/// Workflow: FS-05
#[cfg(unix)]
#[gitti_test::test]
fn fs05_discard_a_deleted_symlink() {
    use shapes::symlinks as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, &format!("key:enter;{}press:d;key:y;state", goto(wf::GONE)));
    no_error(states(&out)[0], &out);
    wf::expect_restored(d).unwrap();
}

/// FS-07: a FIFO in the work tree doesn't block the refresh; the hard-linked b, `empty`
/// and the new empty file ("New empty file") marked (space) and staged (s).
/// Workflow: FS-07
#[cfg(unix)]
#[gitti_test::test]
fn fs07_stage_special_files() {
    use shapes::special_files as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let script = format!(
        "key:enter;dump;{}press:space;{}press:space;{}press:space;key:s;state",
        goto(wf::B),
        goto(wf::EMPTY),
        goto(wf::NEW_EMPTY)
    );
    let out = run(d, &script);
    assert!(screens(&out)[0].contains("New empty file"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=1 staged=3"), "only a left: {out}");
    no_error(st[0], &out);
    wf::expect_staged(d).unwrap();
}

/// FS-07: the hunk of `a` — hard-linked with `b` — discarded (n, d, y): `a` as committed,
/// `b` keeps the edit.
/// Workflow: FS-07
#[cfg(unix)]
#[gitti_test::test]
fn fs07_discard_a_hard_linked_file() {
    use shapes::special_files as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, &format!("key:enter;{}press:n;press:d;key:y;state", goto(wf::A)));
    no_error(states(&out)[0], &out);
    wf::expect_discarded(d).unwrap();
}

/// FS-08: b evil ⏎ — the switch is refused, in plain words (which path, why); HEAD stays.
/// Where the disk folds case, cloning a repository with `X.h` and `x.h` says which paths
/// collided.
/// Workflow: FS-08
#[gitti_test::test]
fn fs08_a_dot_git_alias_and_colliding_names() {
    use shapes::git_alias as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("repo");
    wf::fixture(&d);
    let out = run(&d, &format!("press:b;{}key:enter;state;press:e;dump", presses(wf::EVIL)));
    let st = states(&out);
    assert!(st[0].contains("status=\"ERROR ") && st[0].contains(wf::REFUSAL), "{out}");
    assert!(screens(&out)[0].contains(wf::ALIAS), "{out}");
    wf::expect_refused(&d).unwrap();
    if shapes::case_and_form::holds_case(tmp.path()) {
        return;
    }
    let remote = tmp.path().join("pair.git");
    wf::collide_remote(&remote);
    let url = remote.display().to_string();
    let parent = tmp.path().join("clones");
    std::fs::create_dir_all(&parent).unwrap();
    let out = run(
        &d,
        &format!(
            "press:ctrl-p;{}press:enter;type:{url};press:tab;press:ctrl-u;type:{};key:enter;dump",
            presses("clone repo"),
            parent.display()
        ),
    );
    assert!(screens(&out)[0].contains(wf::COLLIDED), "{out}");
}

/// IG-06: listed are exactly the untracked files git shows (a nested `!keep.log`, the
/// excludes file's `*.tmp` ignored); S stages them.
/// Workflow: IG-06
#[gitti_test::test]
fn ig06_negation_and_excludes_file() {
    use shapes::ignore_rules as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("repo");
    wf::fixture(&d);
    let out = run(&d, "key:enter;state;dump;key:S;state");
    let st = states(&out);
    assert!(st[0].contains("unstaged=2 staged=0"), "{out}");
    let screen = screens(&out)[0];
    for f in wf::SHOWN {
        assert!(screen.contains(f), "{f}: {out}");
    }
    for f in wf::HIDDEN {
        assert!(!screen.contains(f), "{f}: {out}");
    }
    assert!(st[1].contains("unstaged=0 staged=2"), "{out}");
    wf::expect(&d).unwrap();
}

/// CM-20: `i18n.commitEncoding = ISO-8859-1` — S stages, c and the typed message: the
/// note row under the box says what Latin-1 lacks and what is written instead; ctrl-s
/// commits it in Latin-1 (with its header), the status line says what was replaced.
/// Workflow: CM-20
#[gitti_test::test]
fn cm20_legacy_encoding() {
    use shapes::legacy_encoding as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("repo");
    wf::fixture(&d);
    let out = gitty_with(
        &d,
        tempfile::tempdir().unwrap().path(),
        "200x45",
        &format!("key:enter;key:S;press:c;type:{};dump;key:ctrl-s;state", wf::TYPED),
    );
    let screen = screens(&out)[0];
    let note = screen.lines().find(|l| l.contains("Not in ISO-8859-1: – → -, „ → \"")).unwrap_or("");
    assert!(!note.is_empty(), "{out}");
    let st = states(&out);
    no_error(st[0], &out);
    assert!(st[0].contains("Message written in ISO-8859-1"), "{out}");
    wf::expect(&d).unwrap();
}
