//! Workflow tests of line endings, byte order marks and encodings changed by another
//! program (catalog: `crates/gitti-workflow/WORKFLOWS.md`, EX / NU), driven through gitty:
//! the workflow's fixture (`gitti_workflow::ex`, `nu`), one headless gitty run with the
//! user's keys (space picks lines, s / u / d on lines and hunks, the palette's Renormalize
//! Line Endings…), `state` / `dump` checks of what gitty shows — the note that says why
//! lines changed, the encoding label, the status line — then the repository with the
//! workflow's own `expect`, shared with `tests/core/wf_eol.rs` and `tests/ui/wf_eol.rs`.

use crate::common::*;
use crate::support::*;

use gitti_workflow::{ex, nu};

/// One gitty run, wide enough for the notes (160x45).
fn run(d: &std::path::Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    gitty_with(d, config.path(), "160x45", script)
}

/// `j` `n` times.
fn down(n: usize) -> String {
    "press:j;".repeat(n)
}

/// EX-02: the note says every line changed its ending; the last line's `-` / `+` pair
/// picked with space and staged (s), the first line's picked and discarded (d, y).
/// Workflow: EX-02
#[gitti_test::test]
fn ex02_whole_file_to_crlf() {
    use ex::lf_to_crlf as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    // n: the hunk; its lines are the N `-` lines, then the N `+` lines.
    let n = wf::N;
    let script = format!(
        "press:enter;dump;press:n;{}press:space;{}press:space;state;key:s;state;\
         press:g;press:n;press:j;press:space;{}press:space;state;press:d;key:y;state",
        down(n),
        down(n),
        down(n - 1),
    );
    let out = run(d, &script);
    assert!(screens(&out)[0].contains(wf::NOTE), "{out}");
    let st = states(&out);
    let last = format!("hunk: 0, line: {}", 2 * n - 1);
    assert!(st[0].contains(&last) && st[0].contains("sel_lines=2"), "{out}");
    assert!(st[1].contains("unstaged=1 staged=1"), "{out}");
    let first_added = format!("hunk: 0, line: {}", n - 1);
    assert!(st[2].contains(&first_added) && st[2].contains("sel_lines=2"), "{out}");
    wf::expect(d).unwrap();
}

/// EX-03: two one-line hunks, said; s on the first hunk stages it alone.
/// Workflow: EX-03
#[gitti_test::test]
fn ex03_crlf_on_some_lines() {
    use ex::crlf_on_some_lines as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, "press:enter;dump;press:n;key:s;state");
    assert!(screens(&out)[0].contains(wf::NOTE), "{out}");
    assert_eq!(screens(&out)[0].lines().filter(|l| l.contains("@@ -")).count(), 2, "two hunks: {out}");
    assert!(states(&out)[0].contains("unstaged=1 staged=1"), "{out}");
    wf::expect(d).unwrap();
}

/// EX-04: every line changed, said; S (Stage All) stores LF — the staged diff says it too.
/// Workflow: EX-04
#[gitti_test::test]
fn ex04_crlf_file_saved_with_lf() {
    use ex::crlf_to_lf as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, "press:enter;dump;key:S;state;dump");
    assert!(screens(&out)[0].contains(wf::NOTE), "{out}");
    assert!(states(&out)[0].contains("unstaged=0 staged=1"), "{out}");
    assert!(screens(&out)[1].contains(wf::NOTE), "the staged diff's note: {out}");
    wf::expect(d).unwrap();
}

/// EX-05: `core.autocrlf=true` — only the edits show, no note; S stages the file, u on
/// the staged second hunk unstages it, d on the unstaged hunk discards it (CRLF back).
/// Workflow: EX-05
#[gitti_test::test]
fn ex05_autocrlf_stage_unstage_discard() {
    use ex::autocrlf as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let script = "press:enter;dump;key:S;state;press:g;press:n;press:n;state;key:u;state;\
                  press:g;press:n;state;press:d;key:y;state";
    let out = run(d, script);
    let screen = screens(&out)[0];
    assert!(screen.contains("M a.txt  -2 +2"), "the two edits only: {out}");
    assert!(!screen.contains("line ending"), "no line-ending note with autocrlf: {out}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=1"), "{out}");
    assert!(st[1].contains("Hunk { src: Staged, file: 0, hunk: 1 }"), "{out}");
    assert!(st[2].contains("unstaged=1 staged=1"), "{out}");
    assert!(st[3].contains("Hunk { src: Unstaged, file: 0, hunk: 0 }"), "{out}");
    assert!(st[4].contains("unstaged=0 staged=1"), "{out}");
    wf::expect(d).unwrap();
}

/// EX-06: `eol=crlf` — line 3's pair picked (space, j, space) and staged; d on line 17's
/// hunk discards it.
/// Workflow: EX-06
#[gitti_test::test]
fn ex06_eol_attribute_stage_and_discard() {
    use ex::eol_attribute as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    // Hunk 0: `a 1`, `a 2` (context), `-a 3`, `+a 3 changed`, …
    let script = format!(
        "press:enter;press:n;{}press:space;press:j;press:space;state;key:s;state;\
         press:g;press:n;state;press:d;key:y;state",
        down(3)
    );
    let out = run(d, &script);
    let st = states(&out);
    assert!(st[0].contains("hunk: 0, line: 3") && st[0].contains("sel_lines=2"), "{out}");
    assert!(st[1].contains("unstaged=1 staged=1"), "{out}");
    assert!(st[2].contains("Hunk { src: Unstaged, file: 0, hunk: 0 }"), "{out}");
    assert!(st[3].contains("unstaged=0 staged=1"), "{out}");
    wf::expect(d).unwrap();
}

/// EX-07: the palette's Renormalize Line Endings… — its dialog says what it does and
/// names the file with unstaged changes, left out by default; ⏎ renormalizes, the status
/// says what was staged, the staged diff why. `.gitattributes`' menu (.) offers it too;
/// there with the box ticked (space) `d.txt` follows.
/// Workflow: EX-07
#[gitti_test::test]
fn ex07_renormalize_after_attributes() {
    use ex::renormalize as wf;
    use gitti_core::renormalize::{MENU_LABEL, TITLE};
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    wf::expect_before(d).unwrap();
    let script = format!(
        "press:ctrl-p;{}key:enter;dump;key:enter;state;dump",
        presses("renormalize")
    );
    let out = run(d, &script);
    let dialog = screens(&out)[0];
    for want in [TITLE, "--renormalize)", "    d.txt", "☐ Include that file (stages its changes too)"] {
        assert!(dialog.contains(want), "the dialog doesn't say {want:?}: {out}");
    }
    assert!(states(&out)[0].contains(wf::DONE), "{out}");
    assert!(screens(&out)[1].contains(wf::NOTE), "the staged diff's note: {out}");
    wf::expect_renormalized(d).unwrap();
    // `.gitattributes`' menu (] ]: its file row, after d.txt's): r opens the dialog,
    // space ticks the box, ⏎ renormalizes d.txt too.
    let out = run(d, "press:enter;press:];press:];state;press:.;dump;key:r;press:space;key:enter;state");
    assert!(states(&out)[0].contains("File { src: Unstaged, file: 1 }"), "{out}");
    assert!(screens(&out)[0].contains(&format!("{MENU_LABEL}      r")), "{out}");
    let done = "Renormalized line endings: 1 file staged (d.txt) — commit to record it";
    assert!(states(&out)[1].contains(done), "{out}");
    wf::expect_renormalized_all(d).unwrap();
}

/// EX-08: both notes; s on `a.txt`'s hunk stages the mark, d on `b.txt`'s writes it back.
/// Workflow: EX-08
#[gitti_test::test]
fn ex08_bom_added_and_removed() {
    use ex::bom as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, "press:enter;dump;press:n;key:s;state;press:g;press:n;state;press:d;key:y;state");
    let screen = screens(&out)[0];
    assert!(screen.contains(wf::NOTE_ADDED) && screen.contains(wf::NOTE_REMOVED), "{out}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=1 staged=1"), "{out}");
    assert!(st[2].contains("unstaged=0 staged=1"), "{out}");
    wf::expect(d).unwrap();
}

/// EX-09: the file row names Windows-1252; line 1's pair picked and staged byte for byte.
/// Workflow: EX-09
#[gitti_test::test]
fn ex09_code_page_switch() {
    use ex::code_page as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, "press:enter;dump;press:n;press:j;press:space;press:j;press:space;state;key:s;state");
    let screen = screens(&out)[0];
    assert!(screen.lines().any(|l| l.contains(wf::FILE) && l.contains(wf::LABEL)), "{out}");
    assert!(screen.contains("Preis: 5 €") && screen.contains("Grüße"), "decoded: {out}");
    let st = states(&out);
    assert!(st[0].contains("sel_lines=2"), "{out}");
    assert!(st[1].contains("unstaged=1 staged=1"), "{out}");
    wf::expect(d).unwrap();
}

/// EX-11: s on `a.txt`'s second hunk (the added last newline) stages it alone; d on
/// `b.txt`'s hunk (the dropped newline) brings it back.
/// Workflow: EX-11
#[gitti_test::test]
fn ex11_trailing_newline() {
    use ex::trailing_newline as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(d, "press:enter;press:n;press:n;state;key:s;state;press:g;press:n;press:n;state;press:d;key:y;state");
    let st = states(&out);
    assert!(st[0].contains("Hunk { src: Unstaged, file: 0, hunk: 1 }"), "{out}");
    assert!(st[1].contains("unstaged=2 staged=1"), "{out}");
    assert!(st[2].contains("Hunk { src: Unstaged, file: 1, hunk: 0 }"), "{out}");
    assert!(st[3].contains("unstaged=1 staged=1"), "{out}");
    wf::expect(d).unwrap();
}

/// NU-03: `working-tree-encoding` — d on `t.txt`'s added line writes UTF-16 back; the
/// untracked `n.txt`'s line `ö` picked and staged as UTF-8.
/// Workflow: NU-03
#[gitti_test::test]
fn nu03_working_tree_encoding() {
    use nu::working_tree_encoding as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = run(
        d,
        "press:enter;press:n;press:j;press:j;press:space;state;press:d;key:y;state;\
         press:g;press:n;press:j;press:j;press:space;state;key:s;state",
    );
    let st = states(&out);
    assert!(st[0].contains("sel_lines=1"), "{out}");
    assert!(st[2].contains("sel_lines=1"), "{out}");
    assert!(st[3].contains("staged=1"), "{out}");
    wf::expect(d).unwrap();
}
