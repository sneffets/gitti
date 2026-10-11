//! Workflow tests of tags (catalog: `crates/gitti-workflow/WORKFLOWS.md`, TG), driven
//! through gitty: the workflow's fixture (`gitti_workflow::tg`), one headless gitty run
//! with the user's keys (the commit menu's Create Tag Here…, a tag's sidebar menu),
//! `dump` / `state` / `cmds` checks of what gitty shows — the dialog's refusal, Show Tag's
//! text — then the repository with the workflow's own `expect`, shared with
//! `tests/core/wf_tags.rs` and `tests/ui/wf_tags.rs`.

use crate::common::*;
use crate::support::*;

use gitti_workflow::tg;

/// TG-05: Create Tag Here… (t) on HEAD with `v1`: the dialog says where v1 is and keeps
/// OK disabled — ⏎ runs no git; `v1.1`, not annotated, goes through.
/// Workflow: TG-05
#[gitti_test::test]
fn tg05_an_existing_name_is_refused_in_the_dialog() {
    use tg::existing_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = gitty(
        d,
        &format!(
            "key:j;key:.;key:t;{}dump;cmds;key:enter;cmds;dump;press:backspace;press:backspace;\
             {}press:tab;press:space;key:enter;state",
            presses(wf::TAKEN),
            presses(wf::NEW_NAME)
        ),
    );
    let sc = screens(&out);
    let want = format!("{}{})", wf::REFUSED, &s.init[..7]);
    assert!(sc[0].contains(&want), "v1 not refused: {out}");
    assert!(cmds(&out)[1].is_empty(), "⏎ on a refused name ran git: {out}");
    assert!(sc[1].contains("Create tag at"), "the dialog closed: {out}");
    wf::expect(d, &s).unwrap();
}

/// TG-06: a tag of a tag, an annotated and a lightweight tag of a blob — all listed in
/// TAGS; Show Tag (i) says what each one is and points at.
/// Workflow: TG-06
#[gitti_test::test]
fn tg06_tags_of_tags_and_blobs_listed_and_shown() {
    use tg::nested as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    // The sidebar: TAGS (the 6th row: branches, main, worktrees, the worktree, remotes),
    // expanded: outer, light-blob, inner, blob-tag (newest first; the lightweight one by
    // its name); each one's menu ▸ Show Tag… (i).
    let show = "key:m;key:i;dump;key:esc;";
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "160x45",
        &format!(
            "key:h;key:j;key:j;key:j;key:j;key:j;key:enter;dump;key:j;{show}key:j;{show}key:j;key:j;\
             {show}"
        ),
    );
    let sc = screens(&out);
    assert!(sc[0].contains("TAGS (4)"), "{out}");
    // The sidebar's column: the first 26 characters of a line.
    let sidebar: Vec<String> =
        sc[0].lines().map(|l| l.chars().take(26).collect::<String>().trim().to_string()).collect();
    for tag in wf::ALL {
        assert!(sidebar.iter().any(|l| l == tag), "{tag} not listed: {out}");
    }
    let chain = format!("Points at: tag {} → commit {} init", &s.inner[..10], &s.init[..10]);
    assert!(sc[1].contains("Annotated tag outer") && sc[1].contains(wf::OUTER_MESSAGE), "{out}");
    assert!(sc[1].contains(&chain), "{out}");
    let blob = format!("blob {}", &s.blob[..10]);
    assert!(sc[2].contains("Lightweight tag light-blob") && sc[2].contains(&blob), "{out}");
    assert!(sc[3].contains("Annotated tag blob-tag") && sc[3].contains(&blob), "{out}");
    wf::expect(d, &s).unwrap();
}

/// TG-07: on a detached HEAD, Create Tag Here… (t) with a message of several lines (one
/// starting with `#`, kept): an annotated tag of that commit, HEAD stays detached.
/// Workflow: TG-07
#[gitti_test::test]
fn tg07_tag_a_detached_commit() {
    use tg::detached as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let message: String = wf::MESSAGE.split('\n').map(presses).collect::<Vec<_>>().join("press:enter;");
    let out = gitty(
        d,
        &format!("key:j;key:j;key:.;key:t;{}press:enter;{message}key:ctrl-s;state", presses(wf::NAME)),
    );
    let st = states(&out);
    assert!(st[0].contains(&format!("status=\"Create tag {} — done\"", wf::NAME)), "{out}");
    wf::expect(d, &s).unwrap();
}
