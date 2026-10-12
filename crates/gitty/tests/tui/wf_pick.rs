//! Workflow tests of cherry-pick and revert (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! CP / RV), driven through gitty: the workflow's fixture (`gitti_workflow::cp::picks`,
//! `gitti_workflow::rv`), one headless gitty run with the user's keys — the commit menu's
//! Cherry-pick / Revert entries, space marking several commits and Cherry-pick / Revert
//! Marked Commits…, the conflicted file's resolve menu, the palette's Continue / Abort /
//! Skip, the reflog — `state` / `dump` checks of what gitty shows, then the repository
//! with the workflow's own `expect`, shared with `tests/core/wf_pick.rs` and
//! `tests/ui/wf_pick.rs`.

use crate::common::*;
use crate::menus::dialog_text;
use crate::support::*;

use gitti_workflow::cp::picks::{self as wf, row};
use gitti_workflow::rv;

/// To commit list row `n` (row 0: the working directory, selected at the start): down
/// from the top, or up from the bottom (G) when that is shorter — every row passed loads
/// its commit.
fn to_row(n: usize) -> String {
    if n > row::INIT / 2 {
        format!("press:G;{}", "press:k;".repeat(row::INIT - n))
    } else {
        "press:j;".repeat(n)
    }
}

/// `.` on commit list row `n`: its menu.
fn menu_on(n: usize) -> String {
    format!("{}press:.;", to_row(n))
}

/// `ctrl-p`, `query`, ⏎: a palette command.
fn palette(query: &str) -> String {
    format!("press:ctrl-p;{}key:enter;", presses(query))
}

/// To the working directory row, into its diff, ⏎ on the (first) conflicted file: its
/// resolve menu.
const RESOLVE_MENU: &str = "press:g;press:enter;press:j;press:enter;";

/// The status text of a `state` line.
fn status_of(state: &str) -> &str {
    state.split(" status=\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("")
}

fn run(d: &std::path::Path, script: &str) -> String {
    gitty_with(d, tempfile::tempdir().unwrap().path(), "160x45", script)
}

/// CP-01: `Fix a`'s menu ▸ Cherry-pick (c): a copy on `main`.
/// Workflow: CP-01
#[gitti_test::test]
fn cp01_pick_one_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(d, &format!("{}dump;key:c;state", menu_on(row::FIX)));
    assert!(screens(&out)[0].contains(&format!("Cherry-pick {}", &s.fix[..7])), "{out}");
    let st = states(&out);
    assert!(st[0].contains("repo_state=clean conflicted=0") && !st[0].contains("ERROR"), "{out}");
    wf::expect_picked(d, &s, &[(&s.fix, wf::FIX)], false, &[("a.txt", 2, wf::FIXED_A2)], "CP-01").unwrap();
}

/// CP-09: `Fix a` is on `main` already (picked before); ▸ Cherry-pick again stops with
/// nothing to commit — the question offers Skip / Commit Empty; s skips.
/// Workflow: CP-09
#[gitti_test::test]
fn cp09_pick_it_again_then_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture_fix_picked(d);
    let out = run(d, &format!("{}key:c;dump;key:s;state", menu_on(row::FIX)));
    let sc = screens(&out);
    assert!(sc[0].contains("Nothing to commit") && sc[0].contains("Commit Empty"), "{out}");
    let st = states(&out);
    assert!(status_of(st[0]).contains("Skip — done") && st[0].contains("repo_state=clean"), "{out}");
    wf::expect_picked_once(d, &s).unwrap();
}

/// CP-02: ▸ Cherry-pick with Options… (x), the first choice (-x): the message names the
/// original.
/// Workflow: CP-02
#[gitti_test::test]
fn cp02_pick_with_record_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(d, &format!("{}key:x;items;key:enter;state", menu_on(row::FIX)));
    assert!(items(&out).first().is_some_and(|i| i.starts_with("-x")), "{out}");
    assert!(!states(&out)[0].contains("ERROR"), "{out}");
    wf::expect_picked(d, &s, &[(&s.fix, wf::FIX)], true, &[("a.txt", 2, wf::FIXED_A2)], "CP-02").unwrap();
}

/// CP-04, the resolution (the pick of `Clash c` stopped on `c.txt` — the pick itself
/// through the menu: [`cp05_conflicting_pick_stops_then_abort`]): the conflicted file's
/// resolve menu ▸ Take theirs (t): resolved and staged, the pick still waits.
/// Workflow: CP-04
#[gitti_test::test]
fn cp04_take_theirs() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_stopped(d, "", &["cherry-pick", "clash~1"]);
    let out = run(d, &format!("{RESOLVE_MENU}items;key:t;state"));
    assert!(items(&out).iter().any(|i| i.starts_with("Take Theirs")), "{out}");
    let st = states(&out);
    assert!(st[0].contains("staged=1 repo_state=cherrypicking conflicted=0"), "{out}");
    let e = gitti_workflow::Expect::new(d);
    assert_eq!(e.index("c.txt"), Some(wf::file("c", &[(8, wf::C8_CLASH)]).into_bytes()), "{out}");
}

/// CP-04, the end (resolved to the picked side): the palette's Continue commits it with
/// the picked author and message.
/// Workflow: CP-04
#[gitti_test::test]
fn cp04_continue_commits_the_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture_resolved(d, "", &["cherry-pick", "clash~1"], false);
    let out = run(d, &format!("{}state", palette("continue cherry")));
    let st = states(&out);
    assert!(st[0].contains("repo_state=clean conflicted=0") && !st[0].contains("ERROR"), "{out}");
    wf::expect_picked(d, &s, &[(&s.clash_c, wf::CLASH_C)], false, &[("c.txt", 8, wf::C8_CLASH)], "CP-04").unwrap();
}

/// CP-05: `Clash c` ▸ Cherry-pick stops on `c.txt` (said plainly, the banner up); the
/// palette's Abort: everything as before.
/// Workflow: CP-05
#[gitti_test::test]
fn cp05_conflicting_pick_stops_then_abort() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(d, &format!("{}key:c;state;dump;{}state", menu_on(row::CLASH_C), palette("abort cherry")));
    let st = states(&out);
    assert!(st[0].contains("repo_state=cherrypicking conflicted=1") && !st[0].contains("ERROR"), "{out}");
    let stop = "Cherry-pick stopped on 1 conflict — resolve it, then Continue (or Skip / Abort).";
    assert!(status_of(st[0]).ends_with(stop), "{out}");
    assert!(screens(&out)[0].contains("CHERRY-PICKING"), "the banner: {out}");
    assert!(st[1].contains("repo_state=clean conflicted=0") && !st[1].contains("ERROR"), "{out}");
    wf::expect_unchanged(d, &s, "CP-05 after Abort").unwrap();
}

/// CP-06: the merge commit on `side` ▸ Cherry-pick: one ordinary commit with what the
/// merge brought against its first parent.
/// Workflow: CP-06
#[gitti_test::test]
fn cp06_pick_a_merge_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(d, &format!("{}dump;key:c;state", menu_on(row::MERGE_SIDE)));
    assert!(screens(&out)[0].contains(&format!("Cherry-pick {}", &s.merge_side[..7])), "{out}");
    assert!(!states(&out)[0].contains("ERROR"), "{out}");
    wf::expect_merge_picked(d, &s).unwrap();
}

/// CP-08: space marks `Change e`, `Change b`, `Fix a` (✓ on their rows); the palette's
/// Cherry-pick Marked Commits… asks — title, oldest first, the commits in that order —
/// and ⏎ picks them in one go; the marks are gone.
/// Workflow: CP-08
#[gitti_test::test]
fn cp08_pick_three_marked_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(
        d,
        &format!(
            "{}key:space;press:j;key:space;press:j;key:space;state;dump;{}dump;key:enter;state",
            to_row(row::CHANGE_E),
            palette("cherry-pick marked")
        ),
    );
    let st = states(&out);
    assert_eq!(num(st[0], "commit_marks"), 3, "{out}");
    let sc = screens(&out);
    assert_eq!(sc[0].lines().filter(|l| l.contains("│✓")).count(), 3, "three rows marked: {out}");
    assert!(sc[1].contains("┌ Cherry-pick 3 Commits onto main"), "{out}");
    let text = dialog_text(sc[1]);
    assert!(text.contains("oldest first"), "{text}");
    let (f, b, e) = (
        text.find(&format!("1. {} {}", &s.fix[..7], wf::FIX)),
        text.find(&format!("2. {} {}", &s.change_b[..7], wf::CHANGE_B)),
        text.find(&format!("3. {} {}", &s.change_e[..7], wf::CHANGE_E)),
    );
    assert!(f.is_some() && b > f && e > b, "the order: {text}");
    assert_eq!(status_of(st[1]), "Cherry-pick — Picked 3 commits onto main, oldest first.", "{out}");
    assert_eq!(num(st[1], "commit_marks"), 0, "{out}");
    wf::expect_picked(
        d,
        &s,
        &[(&s.fix, wf::FIX), (&s.change_b, wf::CHANGE_B), (&s.change_e, wf::CHANGE_E)],
        false,
        &[("a.txt", 2, wf::FIXED_A2), ("b.txt", 5, wf::B5), ("e.txt", 3, wf::E3)],
        "CP-08",
    )
    .unwrap();
}

/// CP-07, first half: `clash`'s three commits marked (space); the commit menu offers
/// Cherry-pick / Revert 3 Marked Commits…; the palette's entry ⏎: git stops at the
/// second — where, and that one more follows, in the status.
/// Workflow: CP-07
#[gitti_test::test]
fn cp07_marked_sequence_stops_at_the_second() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(
        d,
        &format!(
            "{}key:space;press:j;key:space;press:j;key:space;press:.;items;key:esc;{}key:enter;state",
            to_row(row::CLASH_E),
            palette("cherry-pick marked"),
        ),
    );
    assert!(items(&out).contains(&"Cherry-pick 3 Marked Commits…"), "{out}");
    assert!(items(&out).contains(&"Revert 3 Marked Commits…"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("repo_state=cherrypicking conflicted=1"), "{out}");
    let at = format!("Cherry-pick stopped at commit 2 of 3 ({}) on 1 conflict", &s.clash_c[..7]);
    assert!(status_of(st[0]).contains(&at) && status_of(st[0]).ends_with("the 1 commit after it."), "{out}");
    let e = gitti_workflow::Expect::new(d);
    assert_eq!(e.subjects("HEAD", 2), [wf::CLASH_A, wf::MAIN_A_BACK], "the first one picked");
}

/// CP-07, second half (the sequence stopped at `Clash c`, resolved to its side): the
/// palette's Continue picks the third too.
/// Workflow: CP-07
#[gitti_test::test]
fn cp07_resolve_and_continue_picks_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture_resolved(d, "", &["cherry-pick", "clash~2", "clash~1", "clash"], false);
    let out = run(d, &format!("{}state", palette("continue cherry")));
    assert!(states(&out)[0].contains("repo_state=clean conflicted=0"), "{out}");
    wf::expect_picked(
        d,
        &s,
        &[(&s.clash_a, wf::CLASH_A), (&s.clash_c, wf::CLASH_C), (&s.clash_e, wf::CLASH_E)],
        false,
        &[("a.txt", 9, wf::A9), ("c.txt", 8, wf::C8_CLASH), ("e.txt", 6, wf::E6)],
        "CP-07",
    )
    .unwrap();
}

/// CP-10: the pick of `Clash c` stopped, its conflict resolved to HEAD's side; the
/// palette's Continue says there is nothing left of the commit and offers Skip; s ends
/// the pick, `main` as before.
/// Workflow: CP-10
#[gitti_test::test]
fn cp10_conflict_resolved_to_head_then_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture_resolved(d, "", &["cherry-pick", "clash~1"], true);
    let out = run(d, &format!("{}dump;key:s;state", palette("continue cherry")));
    let sc = screens(&out);
    assert!(sc[0].contains("Nothing to commit") && sc[0].contains("Skip This Commit"), "{out}");
    assert!(states(&out)[0].contains("repo_state=clean conflicted=0"), "{out}");
    wf::expect_unchanged(d, &s, "CP-10 after Skip").unwrap();
}

/// CP-11: `core.commentChar = ;`; the pick of `Clash c` with -x stopped on `c.txt`,
/// resolved to its side; the palette's Continue: no "; Conflicts:" lines, the origin line
/// kept.
/// Workflow: CP-11
#[gitti_test::test]
fn cp11_conflicting_pick_with_record_origin_and_comment_char() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture_resolved(d, "[core]\n\tcommentChar = \";\"\n", &["cherry-pick", "-x", "clash~1"], false);
    let out = run(d, &format!("{}state", palette("continue cherry")));
    assert!(states(&out)[0].contains("repo_state=clean"), "{out}");
    wf::expect_picked(d, &s, &[(&s.clash_c, wf::CLASH_C)], true, &[("c.txt", 8, wf::C8_CLASH)], "CP-11").unwrap();
}

/// CP-16: `a.txt` changed locally; ▸ Cherry-pick `Fix a` is refused in plain words with the
/// ways out (stash / discard and pick); Esc: nothing changed.
/// Workflow: CP-16
#[gitti_test::test]
fn cp16_pick_into_a_dirty_tree_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let dirty = wf::file("a", &[(2, "a 2 local")]);
    let s = wf::fixture_dirty(d, "a.txt", &dirty);
    let out = run(d, &format!("{}key:c;dump;key:esc;state", menu_on(row::FIX)));
    let sc = screens(&out);
    assert!(sc[0].contains("Cherry-pick refused: uncommitted changes in the way"), "{out}");
    assert!(sc[0].contains("Stash and Cherry-pick") && sc[0].contains("Discard and Cherry-pick"), "{out}");
    assert!(sc[0].contains("a.txt"), "names the file: {out}");
    wf::expect_refused(d, &s, "a.txt", &dirty).unwrap();
}

/// CP-17: HEAD's menu has Cherry-pick greyed with the reason; HEAD marked with another
/// commit, the palette's Cherry-pick Marked Commits… is greyed with its reason and ⏎ says
/// it — no git runs.
/// Workflow: CP-17
#[gitti_test::test]
fn cp17_pick_head_is_greyed() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(
        d,
        &format!(
            "{}dump;key:esc;key:space;press:j;key:space;cmds;press:ctrl-p;{}items;key:enter;cmds;state;key:esc;state",
            menu_on(row::MAIN),
            presses("cherry-pick marked")
        ),
    );
    let head = format!("Cherry-pick {} (the selected commit is HEAD)", &s.main[..7]);
    assert!(screens(&out)[0].contains(&head), "{out}");
    let why = gitti_core::availability::HEAD_MARKED;
    assert!(out.contains(why), "the palette's reason: {out}");
    assert_eq!(cmds(&out)[1], Vec::<String>::new(), "no git: {out}");
    let st = states(&out);
    assert!(status_of(st[0]).contains(why) && num(st[0], "commit_marks") == 2, "{out}");
    assert_eq!(num(st[1], "commit_marks"), 0, "Esc clears the marks: {out}");
    wf::expect_unchanged(d, &s, "CP-17").unwrap();
}

/// CP-18: the palette's Reflog, the entry of the lost commit, its menu ▸ Cherry-pick (c).
/// Workflow: CP-18
#[gitti_test::test]
fn cp18_pick_a_lost_commit_from_the_reflog() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let entry = wf::lost_entry(d, &s).expect("the lost commit in HEAD's reflog");
    let out = run(
        d,
        &format!("{}wait;{}reflog;press:enter;key:c;state", palette("Reflog (Recover"), "press:down;".repeat(entry + 1)),
    );
    assert!(out.contains(&format!("selected=Some({entry})")) && out.contains("— Lost work [not in history]"), "{out}");
    assert!(!states(&out)[0].contains("ERROR"), "{out}");
    wf::expect_lost_picked(d, &s).unwrap();
}

/// CP-19, the save: `Change b` and `Fix a` marked (space); the marked commit's menu
/// offers Save 2 Marked Commits as Patches…; the palette's Save as Patch… with "All in
/// one file" on writes the mbox; Copy as Patch says what it copied.
/// Workflow: CP-19
#[gitti_test::test]
fn cp19_save_two_marked_commits_as_one_patch_file() {
    use gitti_workflow::cp::carry_by_patch as carry;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    carry::fixture(d);
    let file = carry::patch_file(d).display().to_string();
    // Rows: 1 `Change c` (main), 2 `Change b`, 3 `Fix a` (feat), 4 `init`.
    let out = run(
        d,
        &format!(
            "press:j;press:j;key:space;press:j;key:space;state;press:.;dump;press:esc;\
             {}press:ctrl-u;type:{file};press:tab;press:space;dump;key:enter;state;{}state",
            palette("save as pa"),
            palette("copy as pa"),
        ),
    );
    let st = states(&out);
    let sc = screens(&out);
    assert_eq!(num(st[0], "commit_marks"), 2, "{out}");
    assert!(sc[0].contains("Save 2 Marked Commits as Patches…"), "{out}");
    let text = dialog_text(sc[1]);
    assert!(text.contains("Writes 2 patches into the one file"), "{text}");
    assert!(status_of(st[1]).contains("Saved 2 commits as patches into"), "{out}");
    carry::expect_saved(d).unwrap();
    assert!(status_of(st[2]).starts_with("Copied 2 commits as patches"), "{out}");
}

/// CP-19, the apply: the saved file; the palette's Apply Patch File… previews it (two
/// commits, applies cleanly) and ⏎ applies them on `main` with `git am`.
/// Workflow: CP-19
#[gitti_test::test]
fn cp19_apply_the_patch_file() {
    use gitti_workflow::cp::carry_by_patch as carry;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = carry::fixture_saved(d);
    let file = carry::patch_file(d).display().to_string();
    let out = run(d, &format!("{}type:{file};key:enter;dump;key:enter;state", palette("apply pat")));
    let sc = screens(&out);
    let text = dialog_text(sc[0]);
    assert!(sc[0].contains("Apply cp19-feat.patch") && text.contains("2 commits made by git format-patch"), "{out}");
    assert!(text.contains("It applies cleanly."), "{text}");
    assert!(status_of(states(&out)[0]).contains("Applied 2 patches as 2 new commits on main"), "{out}");
    carry::expect_applied(d, &s).unwrap();
}

/// RV-01, RV-05: `Change c again` ▸ Revert (v); then the new HEAD's ▸ Revert: c:8 back
/// and forth.
/// Workflow: RV-01, RV-05
#[gitti_test::test]
fn rv01_revert_then_revert_the_revert() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    rv::fixture(d);
    let out = run(d, &format!("{}key:v;state;press:g;press:j;press:.;key:v;state", menu_on(row::MAIN_C_AGAIN)));
    let st = states(&out);
    assert!(st[0].contains("repo_state=clean") && !st[0].contains("ERROR"), "{out}");
    assert!(st[1].contains("repo_state=clean") && !st[1].contains("ERROR"), "{out}");
    let revert = gitti_workflow::Expect::new(d).log("HEAD", 2).pop().expect("the revert").id;
    rv::expect_reapplied(d, &revert).unwrap();
}

/// RV-02: the merge of `topic` ▸ Revert: against its first parent.
/// Workflow: RV-02
#[gitti_test::test]
fn rv02_revert_a_merge_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rv::fixture(d);
    let out = run(d, &format!("{}key:v;state", menu_on(row::MERGE_TOPIC)));
    assert!(!states(&out)[0].contains("ERROR"), "{out}");
    rv::expect_merge_reverted(d, &s).unwrap();
}

/// RV-03: the revert of `Change c` stopped on `c.txt`, resolved to its side; the box's
/// subject edited (", resolved"); the palette's Continue commits with it.
/// Workflow: RV-03
#[gitti_test::test]
fn rv03_revert_conflict_continue_with_an_edited_message() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rv::fixture_resolved(d, "", &["revert", "--no-edit", "main~4"], false);
    let out = run(
        d,
        &format!(
            "press:c;press:ctrl-home;press:end;{}press:esc;{}state",
            presses(", resolved"),
            palette("continue revert")
        ),
    );
    assert!(states(&out)[0].contains("repo_state=clean"), "{out}");
    let message = format!("Revert \"{}\", resolved\n\nThis reverts commit {}.", rv::MAIN_C, s.main_c);
    rv::expect_resolved(d, &s, &message).unwrap();
}

/// RV-04: `Change c` ▸ Revert stops on `c.txt`; the palette's Abort.
/// Workflow: RV-04
#[gitti_test::test]
fn rv04_revert_conflict_abort() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rv::fixture(d);
    let out = run(d, &format!("{}key:v;state;{}state", menu_on(row::MAIN_C), palette("abort revert")));
    let st = states(&out);
    assert!(st[0].contains("repo_state=reverting conflicted=1"), "{out}");
    assert!(status_of(st[0]).ends_with("Revert stopped on 1 conflict — resolve it, then Continue (or Skip / Abort)."), "{out}");
    assert!(st[1].contains("repo_state=clean"), "{out}");
    rv::expect_unchanged(d, &s, "RV-04 after Abort").unwrap();
}

/// RV-06: `Change a` ▸ Revert: undone already — the status says nothing was changed (no
/// error, nothing in progress).
/// Workflow: RV-06
#[gitti_test::test]
fn rv06_revert_a_change_already_undone() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rv::fixture(d);
    let out = run(d, &format!("{}key:v;state", menu_on(row::MAIN_A)));
    let st = states(&out);
    let said = gitti_core::skip::nothing_to_revert(&s.main_a[..7], "main");
    assert!(status_of(st[0]).ends_with(&said) && !st[0].contains("ERROR"), "{out}");
    assert!(st[0].contains("repo_state=clean"), "{out}");
    rv::expect_unchanged(d, &s, "RV-06").unwrap();
}

/// RV-07: `Change c again`, the merge and `Change c` marked; the palette's Revert Marked
/// Commits… asks (newest first) and reverts them in one go.
/// Workflow: RV-07
#[gitti_test::test]
fn rv07_revert_three_marked_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rv::fixture(d);
    let marks = format!("{}key:space;press:j;key:space;press:j;press:j;key:space;", to_row(row::MAIN_C_AGAIN));
    let out = run(d, &format!("{marks}{}dump;key:enter;state", palette("revert marked")));
    let sc = screens(&out);
    assert!(sc[0].contains("┌ Revert 3 Commits on main"), "{out}");
    assert!(dialog_text(sc[0]).contains("newest first"), "{out}");
    assert_eq!(status_of(states(&out)[0]), "Revert — Reverted 3 commits on main, newest first.", "{out}");
    rv::expect_several_reverted(d, &s).unwrap();
}



/// RV-08: `Tune f` (row 2) shown; `n` to its hunk, space on `-f 4` (four lines down) and
/// on `+F 4` (three more); the diff menu (m) ▸ Revert These Lines in Working Tree: line 4
/// back in the working tree only, said so.
/// Workflow: RV-08
#[gitti_test::test]
fn rv08_revert_lines_in_the_working_tree() {
    use gitti_core::commit_lines as cl;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rv::lines::fixture(d);
    let lines = "key:j;".repeat(4) + "key:space;" + &"key:j;".repeat(3) + "key:space;";
    // The menu: Restore This Version…, Restore Version Before…, then the revert entries.
    let out = run(d, &format!("press:j;key:j;key:l;key:n;{lines}state;key:m;items;press:down;press:down;key:enter;state"));
    let menu = &item_lists(&out)[0];
    for want in [cl::REVERT_LINES, cl::REVERT_HUNK, cl::REVERT_FILE, cl::APPLY_LINES, cl::APPLY_HUNK, cl::APPLY_FILE] {
        assert!(menu.contains(&want), "the diff menu lacks {want:?}: {out}");
    }
    assert_eq!(menu.get(2).copied(), Some(cl::REVERT_LINES), "{out}");
    let st = states(&out);
    assert!(st[1].contains(&format!("Reverted 2 lines of f.txt (commit {})", &s.tune_f[..7])), "{out}");
    assert!(st[1].contains("stage and commit to keep it") && !st[1].contains("ERROR"), "{out}");
    rv::lines::expect_reverted(d, &s).unwrap();
}
