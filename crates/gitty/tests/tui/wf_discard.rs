//! Workflow tests of discarding many files, a file that can't be deleted and lines of a
//! read-only file (catalog: `crates/gitti-workflow/WORKFLOWS.md`, DC-04, DC-06, DC-09),
//! driven through gitty: the workflow's fixture (`gitti_workflow::dc`), one headless gitty
//! run with the user's keys, `state` / `dump` / `cmds` checks of what gitty shows and runs,
//! then the repository with the workflow's own `expect` — shared with
//! `tests/ui/wf_discard.rs`.

use crate::common::*;
use crate::support::*;

use gitti_workflow::dc;

/// DC-04: D with 1 000 untracked files in a build folder: the menu counts them and says
/// ignored files stay; Y discards in one go (no `git clean`); nothing left listed, the
/// ignored file stays.
/// Workflow: DC-04
#[gitti_test::test]
fn dc04_discard_thousands_of_untracked_files() {
    use dc::thousands_of_untracked as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "state;cmds;press:D;dump;key:Y;state;cmds");
    let st = states(&out);
    assert_eq!(num(st[0], "unstaged"), wf::UNTRACKED + 1, "{out}");
    let screen = screens(&out)[0];
    assert!(screen.contains(&format!("{} untracked files will be deleted", wf::UNTRACKED)), "{out}");
    assert!(st[1].contains("unstaged=0 staged=0"), "{out}");
    assert!(!st[1].contains("status=\"ERROR"), "{out}");
    let ran = &cmds(&out)[1];
    // One go: one `git restore` for the tracked file, the untracked files deleted
    // in-process (no `git clean`, nothing per file), then the re-read.
    assert_eq!(ran.iter().filter(|c| c.starts_with("restore")).count(), 1, "{ran:?}");
    assert!(!ran.iter().any(|c| c.starts_with("clean")) && ran.len() < 10, "{ran:?}");
    wf::expect(d).unwrap();
}

/// DC-06: D, Y with a file in a read-only folder: the others go, the error names the file,
/// which stays listed.
/// Workflow: DC-06
#[cfg(unix)]
#[gitti_test::test]
fn dc06_a_file_that_cannot_be_deleted() {
    use dc::cannot_delete as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    struct Unlock<'a>(&'a std::path::Path);
    impl Drop for Unlock<'_> {
        fn drop(&mut self) {
            dc::cannot_delete::unlock(self.0);
        }
    }
    let _unlock = Unlock(d);
    if !wf::locked(d) {
        return; // running as root: nothing is read-only
    }
    let out = gitty(d, "press:D;key:Y;state");
    let st = states(&out)[0];
    assert!(st.contains("status=\"ERROR"), "{out}");
    assert!(st.contains("1 file could not be discarded") && st.contains(wf::LOCKED), "{out}");
    assert!(st.contains("unstaged=1 staged=0"), "{out}");
    wf::expect(d).unwrap();
}

/// DC-09: in a read-only file, n j j to the line X, space selects it, d asks, y discards:
/// the file rewritten without it, Y still there, the mode kept.
/// Workflow: DC-09
#[cfg(unix)]
#[gitti_test::test]
fn dc09_discard_lines_of_a_read_only_file() {
    use dc::read_only_file as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, "press:enter;press:n;press:j;press:j;press:space;state;press:d;dump;key:y;state");
    let st = states(&out);
    assert!(st[0].contains("Line { src: Unstaged, file: 0, hunk: 0, line: 1"), "{out}");
    assert_eq!(num(st[0], "sel_lines"), 1, "{out}");
    assert!(screens(&out)[0].contains("Discard Lines"), "{out}");
    assert!(st[1].contains("unstaged=1 staged=0") && !st[1].contains("status=\"ERROR"), "{out}");
    wf::expect(d).unwrap();
}
