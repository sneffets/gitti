//! Workflow tests across branches and of rebasing (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, CP-14 / CP-15, RB, RL-03 / RL-04), driven
//! through gitty: the workflow's fixture (`gitti_workflow::cp`, `gitti_workflow::rb`,
//! `gitti_workflow::rl`), one headless gitty run with the user's keys — the palette's
//! Merge Branch into Current… and Rebase Current Branch onto…, a conflicted file's resolve
//! menu, the commit box, Continue / Abort from the palette, the reflog's entry menu —
//! `state` / `dump` checks of what gitty shows, then the repository with the workflow's
//! own `expect`, shared with `tests/core/wf_history.rs` and `tests/ui/wf_history.rs`.

use std::path::Path;

use crate::common::*;
use crate::support::*;

use gitti_core::ops::RepoState;
use gitti_core::rebase_note::{finished, left_out};
use gitti_core::stopped::stop_line;
use gitti_workflow::cp::{pick_resolved_then_merge, pick_revert_then_merge};
use gitti_workflow::rb::{self, Base, Opts, Side};
use gitti_workflow::rl::undo_amend;

/// gitty on `dir` with `script` (160×45), the global git config shut out.
fn run(dir: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    gitty_env(dir, config.path(), "160x45", &[("GIT_CONFIG_GLOBAL", "/dev/null")], script)
}

/// A palette command run by its label.
fn palette(label: &str) -> String {
    format!("press:ctrl-p;{}key:enter;", presses(label))
}

/// The palette's Merge Branch into Current…, `feat` picked, the Merge dialog's defaults.
fn merge_feat() -> String {
    format!("{}{}key:enter;key:enter;", palette("Merge Branch into Current"), presses("feat"))
}

/// The palette's Rebase Current Branch onto…, `main` picked.
fn rebase_onto_main() -> String {
    format!("{}{}key:enter;", palette("Rebase Current Branch onto"), presses("main"))
}

/// To the working directory row, into its diff, ⏎ on the (first) conflicted file: its
/// resolve menu.
const RESOLVE_MENU: &str = "press:g;press:enter;press:j;press:enter;";

/// The commit box: c, to the end of its first line, `text` typed, Esc.
fn append_to_box(text: &str) -> String {
    format!("press:c;press:ctrl-home;press:end;{}press:esc;", presses(text))
}

/// HEAD's reflog (palette), down to `entry`, ⏎: its menu.
fn reflog_entry_menu(entry: usize) -> String {
    format!("{}wait;{}reflog;key:enter;", palette("Reflog (Recover"), "key:down;".repeat(entry + 1))
}

/// The status text of a `state` line (its quotes unescaped).
fn status_of(state: &str) -> String {
    let text = state.split(" status=\"").nth(1).and_then(|s| s.split("\" commit_offset=").next());
    text.unwrap_or("").replace("\\\"", "\"")
}

fn no_error(state: &str, out: &str) {
    assert!(!state.contains("status=\"ERROR"), "{out}");
}

/// CP-14, the merge: the pick's own resolution on `main`; Merge feat stops on `a.txt`
/// again, said plainly.
/// Workflow: CP-14
#[gitti_test::test]
fn cp14_merge_stops_on_the_resolved_line() {
    use pick_resolved_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(d, &format!("{}state", merge_feat()));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=merging conflicted=1"), "{out}");
    assert!(status_of(st).ends_with(&stop_line(RepoState::Merging, 1)), "{out}");
    wf::expect_stopped(d, &s).unwrap();
}

/// CP-14, the resolution (the merge stopped): the resolve menu's Take ours (o) keeps the
/// pick's resolution, staged; the merge still waits.
/// Workflow: CP-14
#[gitti_test::test]
fn cp14_keep_the_resolution() {
    use pick_resolved_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_stopped(d);
    let out = run(d, &format!("{RESOLVE_MENU}key:o;state"));
    assert!(states(&out)[0].contains("staged=1 repo_state=merging conflicted=0"), "{out}");
    let e = gitti_workflow::Expect::new(d);
    assert_eq!(e.index("a.txt"), Some(gitti_workflow::cp::a(Some(wf::A2_RESOLVED)).into_bytes()), "{out}");
}

/// CP-14, the end (the resolution kept): the palette's Continue makes the merge commit.
/// Workflow: CP-14
#[gitti_test::test]
fn cp14_continue_makes_the_merge() {
    use pick_resolved_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture_resolved(d);
    let out = run(d, &format!("{}state", palette("Continue Merging")));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    wf::expect_merged(d, &s).unwrap();
}

/// CP-15: the pick reverted on `main`; Merge feat goes through; the merge commit (j) shows
/// the fix coming back in its diff.
/// Workflow: CP-15
#[gitti_test::test]
fn cp15_pick_revert_then_merge_brings_the_change_back() {
    use pick_revert_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(d, &format!("{}state;press:g;press:j;wait;dump", merge_feat()));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    let screen = screens(&out)[0];
    assert!(screen.contains(gitti_workflow::cp::MERGE), "the merge commit shown: {out}");
    assert!(screen.contains(&format!("+ {}", gitti_workflow::cp::FIXED)), "the fix in its diff: {out}");
    wf::expect(d, &s).unwrap();
}

/// RB-01: the palette's Rebase Current Branch onto… main: `feat` on top of `main`; the
/// status line says so.
/// Workflow: RB-01
#[gitti_test::test]
fn rb01_rebase_onto_main_from_the_palette() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rb::fixture(d, Base::Apart);
    // TODO(open point RB-01 / RB-03): gitty starts the rebase at once, gitti's picker asks
    // first — whether a rebase asks is open.
    let out = run(d, &format!("{}state", rebase_onto_main()));
    let st = states(&out)[0];
    assert!(status_of(st).ends_with(&finished(Some("feat"), "main", 2)), "{out}");
    let replayed = [(s.feat_a.as_str(), rb::FEAT_A), (s.feat_b.as_str(), rb::FEAT_B)];
    rb::expect_rebased(d, &s, &replayed, &rb::rebased_files(&[]), &[], "RB-01").unwrap();
}

/// RB-02 (stopped at `Feat a`, resolved to its side): git's message in the box, edited;
/// the palette's Continue commits it with that message and replays the rest.
/// Workflow: RB-02
#[gitti_test::test]
fn rb02_continue_with_the_edited_message() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rb::fixture_with(d, Base::Clash, Opts { stops: true, resolve: Some(Side::Commit), ..Opts::default() });
    let edit = &rb::EDITED[rb::FEAT_A.len()..];
    let out = run(d, &format!("{}{}state", append_to_box(edit), palette("Continue Rebasing")));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    let replayed = [(s.feat_a.as_str(), rb::EDITED), (s.feat_b.as_str(), rb::FEAT_B)];
    rb::expect_rebased(d, &s, &replayed, &rb::rebased_files(&[]), &[], "RB-02").unwrap();
}

/// RB-03: the rebase stops on `a.txt` (said plainly, the banner up); the palette's Abort:
/// `feat` exactly as before.
/// Workflow: RB-03
#[gitti_test::test]
fn rb03_rebase_stops_then_abort() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rb::fixture(d, Base::Clash);
    let out = run(d, &format!("{}state;dump;{}state", rebase_onto_main(), palette("Abort Rebasing")));
    let st = states(&out);
    assert!(st[0].contains("repo_state=rebasing conflicted=1"), "{out}");
    assert!(status_of(st[0]).ends_with(&stop_line(RepoState::Rebasing, 1)), "{out}");
    assert!(screens(&out)[0].contains("REBASING"), "the banner: {out}");
    assert!(st[1].contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st[1], &out);
    rb::expect_unchanged(d, &s, &[], "RB-03 after Abort").unwrap();
}

/// RB-04, first half (stopped at `Feat a`, resolved): the edited message, Continue —
/// `Feat b` conflicts too: the stop said, the box emptied (its text went into the first
/// commit, the box shows git's message of the next one).
/// Workflow: RB-04
#[gitti_test::test]
fn rb04_the_next_commit_conflicts_too() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    rb::fixture_with(d, Base::ClashTwice, Opts { stops: true, resolve: Some(Side::Commit), ..Opts::default() });
    let edit = &rb::EDITED[rb::FEAT_A.len()..];
    let out = run(d, &format!("{}{}state;dump", append_to_box(edit), palette("Continue Rebasing")));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=rebasing conflicted=1"), "{out}");
    assert!(status_of(st).ends_with(&stop_line(RepoState::Rebasing, 1)), "{out}");
    // The box (the line under the detail's title) holds git's message of the next stop.
    let box_line = screens(&out)[0].lines().skip_while(|l| !l.contains("Working Directory")).nth(1);
    assert!(box_line.is_some_and(|l| l.trim_end().ends_with(&format!(" {}", rb::FEAT_B))), "the used message left the box: {out}");
    let first = gitti_workflow::Expect::new(d).subjects("HEAD", 1);
    assert_eq!(first, [rb::EDITED], "{out}");
}

/// RB-04, second half (stopped again at `Feat b`): Take theirs (t) on `b.txt`, the
/// palette's Continue finishes.
/// Workflow: RB-04
#[gitti_test::test]
fn rb04_resolve_the_second_and_finish() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let o = Opts { stops: true, resolve: Some(Side::Commit), then_continue: true, ..Opts::default() };
    let s = rb::fixture_with(d, Base::ClashTwice, o);
    let out = run(d, &format!("{RESOLVE_MENU}key:t;{}state", palette("Continue Rebasing")));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=0"), "{out}");
    no_error(st, &out);
    let replayed = [(s.feat_a.as_str(), rb::FEAT_A), (s.feat_b.as_str(), rb::FEAT_B)];
    rb::expect_rebased(d, &s, &replayed, &rb::rebased_files(&[]), &[], "RB-04").unwrap();
}

/// RB-05 (stopped at `Feat a`, resolved to `main`'s side): the palette's Continue — git
/// drops the commit without a word; the status line names it.
/// Workflow: RB-05
#[gitti_test::test]
fn rb05_resolved_to_the_base_is_left_out_and_said() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rb::fixture_with(d, Base::Clash, Opts { stops: true, resolve: Some(Side::Base), ..Opts::default() });
    let out = run(d, &format!("{}state", palette("Continue Rebasing")));
    let st = states(&out)[0];
    let note = left_out("main", &[(s.feat_a[..7].to_string(), rb::FEAT_A.into())]).unwrap();
    assert!(status_of(st).ends_with(&format!("{} {note}", finished(Some("feat"), "main", 1))), "{out}");
    assert!(st.contains("repo_state=clean"), "{out}");
    let files = rb::rebased_files(&[("a.txt", 2, rb::A2_MAIN)]);
    rb::expect_rebased(d, &s, &[(&s.feat_b, rb::FEAT_B)], &files, &[], "RB-05").unwrap();
}

/// RB-06: `Feat a` was picked onto `main` before; the rebase replays `Feat b` only and
/// says it left `Feat a` out.
/// Workflow: RB-06
#[gitti_test::test]
fn rb06_a_commit_picked_before_is_left_out() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rb::fixture(d, Base::Picked);
    let out = run(d, &format!("{}state", rebase_onto_main()));
    let note = left_out("main", &[(s.feat_a[..7].to_string(), rb::FEAT_A.into())]).unwrap();
    assert!(status_of(states(&out)[0]).ends_with(&format!("{} {note}", finished(Some("feat"), "main", 1))), "{out}");
    rb::expect_rebased(d, &s, &[(&s.feat_b, rb::FEAT_B)], &rb::rebased_files(&[]), &[], "RB-06").unwrap();
}

/// RB-07: `a.txt` changed locally; the rebase is refused, naming the file and offering
/// the ways out; Esc: nothing changed.
/// Workflow: RB-07
#[gitti_test::test]
fn rb07_rebase_with_a_dirty_tree_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let local = rb::file("a", &[(2, rb::A2_FEAT), (9, "a 9 local")]);
    let s = rb::fixture_with(d, Base::Apart, Opts { dirty: Some(("a.txt", &local)), ..Opts::default() });
    let out = run(d, &format!("{}dump;key:esc;state", rebase_onto_main()));
    let sc = screens(&out)[0];
    assert!(sc.contains("Rebase refused: uncommitted changes in the way"), "{out}");
    assert!(sc.contains("Stash and Rebase") && sc.contains("Discard and Rebase"), "{out}");
    assert!(sc.contains("a.txt"), "names the file: {out}");
    rb::expect_unchanged(d, &s, &[" M a.txt"], "RB-07 refused").unwrap();
    assert_eq!(gitti_workflow::Expect::new(d).file("a.txt"), Some(local.into_bytes()));
}

/// RB-08 (`rebase --apply` stopped, resolved): the box shows the patch's message; edited,
/// the palette's Continue is refused in plain words; the box keeps the text; the rebase
/// still waits.
/// Workflow: RB-08
#[gitti_test::test]
fn rb08_the_apply_backend_keeps_the_box_text() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let o = Opts { stops: true, apply: true, resolve: Some(Side::Commit), ..Opts::default() };
    let s = rb::fixture_with(d, Base::Clash, o);
    let edit = &rb::EDITED[rb::FEAT_A.len()..];
    let out = run(d, &format!("{}{}state;dump", append_to_box(edit), palette("Continue Rebasing")));
    let st = states(&out)[0];
    assert!(st.contains("status=\"ERROR") && status_of(st).contains("rebase --apply"), "{out}");
    assert!(st.contains("repo_state=rebasing"), "{out}");
    assert!(screens(&out)[0].contains(rb::EDITED), "the box keeps the text: {out}");
    rb::expect_stopped(d, &s, &[], "RB-08 after the refusal").unwrap();
}

/// RL-03 (`main` amended): HEAD's reflog, the entry before the amend ▸ Reset main to
/// here — soft (S): the commit before the amend back, what the amend added staged.
/// Workflow: RL-03
#[gitti_test::test]
fn rl03_undo_an_amend_from_the_reflog() {
    use undo_amend as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(d, &format!("{}dump;key:S;state", reflog_entry_menu(wf::BEFORE_AMEND_ENTRY)));
    assert!(out.contains(&format!("selected=Some({}) shown={}", wf::BEFORE_AMEND_ENTRY, &s.before[..7])), "{out}");
    assert!(screens(&out)[0].contains("Reset main to here — soft"), "{out}");
    let st = states(&out)[0];
    assert!(status_of(st).contains(&format!("main is at {} now — soft", &s.before[..7])), "{out}");
    assert!(st.contains("unstaged=0 staged=1"), "{out}");
    wf::expect(d, &s).unwrap();
}

/// RL-04 (`feat` rebased onto `main` by git): HEAD's reflog, the entry of `feat`'s old
/// tip ▸ Reset feat to here — hard (H), the question answered: `feat` as before.
/// Workflow: RL-04
#[gitti_test::test]
fn rl04_undo_a_rebase_from_the_reflog() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = rb::fixture_with(d, Base::Apart, Opts { rebased: true, ..Opts::default() });
    let entry = rb::OLD_TIP_AFTER_REBASE;
    assert_eq!(rb::old_tip_entry(d, &s), Some(entry));
    let out = run(d, &format!("{}key:H;dump;key:enter;state", reflog_entry_menu(entry)));
    assert!(out.contains(&format!("selected=Some({entry}) shown={}", &s.feat_b[..7])), "{out}");
    assert!(screens(&out)[0].contains(&format!("Hard reset feat to {}?", &s.feat_b[..7])), "{out}");
    no_error(states(&out)[0], &out);
    rb::expect_unchanged(d, &s, &[], "RL-04 after the reset").unwrap();
}
