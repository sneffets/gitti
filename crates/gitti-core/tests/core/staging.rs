//! Partial staging against real temporary repositories.

use std::fs;
use std::path::Path;

use gitti_core::diff::FileDiff;
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::{Repo, WorkingState};

struct Fixture {
    _dir: tempfile::TempDir,
    repo: Repo,
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    sh(p, &["init", "-q", "-b", "main"]);
    crate::test_config(p);
    for (name, content) in files {
        fs::write(p.join(name), content).unwrap();
    }
    if !files.is_empty() {
        sh(p, &["add", "-A"]);
        sh(p, &["commit", "-q", "-m", "init"]);
    }
    let repo = Repo::open(p).unwrap();
    Fixture { _dir: dir, repo }
}

impl Fixture {
    fn write(&self, name: &str, content: &str) {
        fs::write(self.repo.workdir().join(name), content).unwrap();
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.repo.workdir().join(name)).unwrap()
    }
    fn index(&self, name: &str) -> String {
        sh(self.repo.workdir(), &["show", &format!(":{name}")])
    }
    fn state(&self) -> WorkingState {
        self.repo.working_state().unwrap()
    }
    fn unstaged(&self, name: &str) -> FileDiff {
        let s = self.state();
        s.unstaged
            .iter()
            .find(|f| f.path() == name)
            .unwrap()
            .diff
            .clone()
            .unwrap()
    }
    fn staged(&self, name: &str) -> FileDiff {
        let s = self.state();
        s.staged
            .iter()
            .find(|f| f.path() == name)
            .unwrap()
            .diff
            .clone()
            .unwrap()
    }
}

/// Indices of the change lines in hunk `h` whose text equals one of `texts`.
fn pick(f: &FileDiff, h: usize, texts: &[&str]) -> Vec<usize> {
    f.hunks[h]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_change() && texts.contains(&l.text.as_str()))
        .map(|(i, _)| i)
        .collect()
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

/// Workflow: ST-01
#[gitti_test::test]
fn stage_one_of_two_hunks() {
    let fx = fixture(&[("a.txt", &numbered(30))]);
    let changed = numbered(30)
        .replace("line 2\n", "line two\n")
        .replace("line 25\n", "line 25!\n");
    fx.write("a.txt", &changed);
    let d = fx.unstaged("a.txt");
    assert_eq!(d.hunks.len(), 2);
    fx.repo
        .apply_selection(&d, &Selection::whole_hunk(1), PatchMode::Stage)
        .unwrap();
    let idx = fx.index("a.txt");
    assert!(idx.contains("line 25!\n") && idx.contains("line 2\n"));
    // second hunk is now staged, first still unstaged
    assert_eq!(fx.unstaged("a.txt").hunks.len(), 1);
    assert_eq!(fx.staged("a.txt").hunks.len(), 1);
}

/// Workflow: ST-03
#[gitti_test::test]
fn unstage_lines_in_second_hunk_with_first_hunk_staged() {
    let fx = fixture(&[("a.txt", &numbered(40))]);
    let changed = numbered(40)
        .replace("line 3\n", "")
        .replace("line 4\n", "")
        .replace("line 30\n", "line 30\nX\nY\n");
    fx.write("a.txt", &changed);
    fx.repo.stage_paths(&["a.txt"]).unwrap();
    let s = fx.staged("a.txt");
    assert_eq!(s.hunks.len(), 2);
    let sel = Selection::lines(1, pick(&s, 1, &["Y"]));
    fx.repo
        .apply_selection(&s, &sel, PatchMode::Unstage)
        .unwrap();
    let expected = numbered(40)
        .replace("line 3\n", "")
        .replace("line 4\n", "")
        .replace("line 30\n", "line 30\nX\n");
    assert_eq!(fx.index("a.txt"), expected);
}

/// Workflow: ST-04
#[gitti_test::test]
fn partial_stage_of_untracked_file() {
    let fx = fixture(&[("keep.txt", "k\n")]);
    fx.write("new.txt", "one\ntwo\nthree\n");
    let d = fx.unstaged("new.txt");
    let sel = Selection::lines(0, pick(&d, 0, &["one", "three"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("new.txt"), "one\nthree\n");
    // remaining unstaged diff adds "two"
    let d = fx.unstaged("new.txt");
    assert_eq!(d.added_removed(), (1, 0));
}

/// Workflow: ST-05
#[gitti_test::test]
fn partial_unstage_of_added_file() {
    let fx = fixture(&[("keep.txt", "k\n")]);
    fx.write("new.txt", "one\ntwo\nthree\n");
    fx.repo.stage_paths(&["new.txt"]).unwrap();
    let s = fx.staged("new.txt");
    let sel = Selection::lines(0, pick(&s, 0, &["two"]));
    fx.repo
        .apply_selection(&s, &sel, PatchMode::Unstage)
        .unwrap();
    assert_eq!(fx.index("new.txt"), "one\nthree\n");
}

/// Workflow: ST-06
#[gitti_test::test]
fn partial_stage_of_deleted_file() {
    let fx = fixture(&[("gone.txt", "one\ntwo\nthree\n")]);
    fs::remove_file(fx.repo.workdir().join("gone.txt")).unwrap();
    let d = fx.unstaged("gone.txt");
    let sel = Selection::lines(0, pick(&d, 0, &["two"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("gone.txt"), "one\nthree\n");
    // whole-hunk stage of the rest deletes it from the index
    let d = fx.unstaged("gone.txt");
    fx.repo
        .apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage)
        .unwrap();
    let st = fx.state();
    assert!(
        st.staged
            .iter()
            .any(|f| f.path() == "gone.txt" && f.entry.index == Some('D'))
    );
}

/// Workflow: DC-01
#[gitti_test::test]
fn discard_single_line() {
    let fx = fixture(&[("a.txt", "a\nb\nc\n")]);
    fx.write("a.txt", "a\nX\nb\nY\nc\n");
    let d = fx.unstaged("a.txt");
    let sel = Selection::lines(0, pick(&d, 0, &["X"]));
    fx.repo
        .apply_selection(&d, &sel, PatchMode::Discard)
        .unwrap();
    assert_eq!(fx.read("a.txt"), "a\nb\nY\nc\n");
}

/// Workflow: EX-11
#[gitti_test::test]
fn no_newline_at_eof() {
    let fx = fixture(&[("a.txt", "a\nb")]);
    fx.write("a.txt", "a\nb\nc");
    let d = fx.unstaged("a.txt");
    fx.repo
        .apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage)
        .unwrap();
    assert_eq!(fx.index("a.txt"), "a\nb\nc");
}

/// Workflow: ST-13, CM-01
#[gitti_test::test]
fn commit_via_cli() {
    let fx = fixture(&[("a.txt", "1\n")]);
    fx.write("a.txt", "2\n");
    fx.repo.stage_all().unwrap();
    fx.repo.commit("second\n\nbody", false).unwrap();
    assert_eq!(fx.repo.head_message().unwrap().trim(), "second\n\nbody");
    assert!(fx.state().staged.is_empty());
}

#[gitti_test::test]
fn ignore_whitespace_and_command_log() {
    let fx = fixture(&[("a.txt", "a\nb\n")]);
    fx.write("a.txt", "a  \nb\n");
    let r = &fx.repo;
    assert_eq!(r.working_state().unwrap().unstaged.len(), 1);
    let ws = r
        .working_state_with(&gitti_core::DiffOptions {
            ignore_whitespace: true,
            ..Default::default()
        })
        .unwrap();
    // file is still listed (status), but its whitespace-only diff has no hunks
    assert!(
        ws.unstaged[0]
            .diff
            .as_ref()
            .is_none_or(|d| d.hunks.is_empty())
    );
    r.stage_all().unwrap();
    let log = r.command_log().snapshot();
    let add = log
        .iter()
        .find(|e| e.args.first().map(String::as_str) == Some("add"))
        .expect("add logged");
    assert!(!add.read_only);
    assert!(log.iter().any(|e| e.read_only && e.args[0] == "status"));
}

#[gitti_test::test]
fn config_list_scope() {
    use gitti_core::ops::ConfigScope;
    let fx = fixture(&[("a.txt", "1\n")]);
    fx.repo
        .config_set(ConfigScope::Local, "push.autoSetupRemote", "true")
        .unwrap();
    let local = fx.repo.config_list(ConfigScope::Local);
    assert_eq!(
        local.get("push.autosetupremote").map(String::as_str),
        Some("true")
    );
    assert_eq!(local.get("user.name").map(String::as_str), Some("T"));
}

/// Workflow: ST-12
#[gitti_test::test]
fn staging_works_with_minimal_context() {
    // context_lines 0 is clamped to 1 for the working state, so line staging still applies
    let fx = fixture(&[("a.txt", &numbered(10))]);
    fx.write("a.txt", &numbered(10).replace("line 5\n", "line five\n"));
    let opts = gitti_core::DiffOptions {
        context_lines: 0,
        ..Default::default()
    };
    let d = fx.repo.working_state_with(&opts).unwrap().unstaged[0]
        .diff
        .clone()
        .unwrap();
    assert_eq!(d.hunks[0].lines.len(), 4, "one context line on each side");
    let sel = Selection::lines(0, pick(&d, 0, &["line five"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert!(fx.index("a.txt").contains("line 5\nline five\n"));
}

/// With "ignore whitespace" the shown diff comes from `git diff -w`; staging maps the
/// selection onto the exact diff, so only the visible change is staged.
/// Workflow: ST-10
#[gitti_test::test]
fn stage_hunk_while_ignoring_whitespace() {
    let f = fixture(&[("a.txt", &numbered(20))]);
    let text = numbered(20)
        .replace("line 3\n", "line THREE\n")
        .replace("line 5\n", "line 5  \n");
    f.write("a.txt", &text);
    let opts = gitti_core::DiffOptions {
        ignore_whitespace: true,
        ..Default::default()
    };
    let state = f.repo.working_state_with(&opts).unwrap();
    let wf = state.unstaged.iter().find(|w| w.path() == "a.txt").unwrap();
    let shown = wf.diff.as_ref().unwrap();
    // The -w view hides the whitespace change on line 5.
    let shown_changes: Vec<_> = shown.hunks[0]
        .lines
        .iter()
        .filter(|l| l.is_change())
        .map(|l| l.text.clone())
        .collect();
    assert_eq!(shown_changes, ["line 3", "line THREE"]);
    let real = wf.patch_source().unwrap();
    let sel = gitti_core::patch::map_selection(shown, &Selection::whole_hunk(0), real).unwrap();
    f.repo
        .apply_selection(real, &sel, PatchMode::Stage)
        .unwrap();
    let index = f.index("a.txt");
    assert!(index.contains("line THREE\n"));
    assert!(
        index.contains("line 5\n"),
        "whitespace change must stay unstaged"
    );
    // The whitespace change is still unstaged in the worktree.
    assert_eq!(f.read("a.txt"), text);
}

/// "Discard All": every unstaged change goes, staged changes stay.
/// Workflow: ST-08, DC-03
#[gitti_test::test]
fn discard_all_keeps_staged_changes() {
    let f = fixture(&[("a.txt", "a\n"), ("b.txt", "b\n")]);
    f.write("a.txt", "a staged\n");
    sh(f.repo.workdir(), &["add", "a.txt"]);
    f.write("a.txt", "a staged\nand unstaged\n");
    f.write("b.txt", "b changed\n");
    f.write("new.txt", "untracked\n");
    let state = f.state();
    let entries: Vec<_> = state.unstaged.iter().map(|w| &w.entry).collect();
    assert_eq!(entries.len(), 3);
    f.repo.discard_paths(&entries).unwrap();
    assert_eq!(f.read("a.txt"), "a staged\n");
    assert_eq!(f.read("b.txt"), "b\n");
    assert!(!f.repo.workdir().join("new.txt").exists());
    assert_eq!(f.index("a.txt"), "a staged\n");
    assert!(f.state().unstaged.is_empty());
}

/// Workflow: CM-19
#[gitti_test::test]
fn history_of_repo_without_commits_has_worktree_row() {
    let fx = fixture(&[]);
    fx.write("a.txt", "a\n");
    let h = fx.repo.history(gitti_core::LogOrder::Topo).unwrap();
    assert_eq!(h.commits.len(), 1);
    assert!(h.is_worktree_row(0));
    assert_eq!(fx.state().unstaged.len(), 1);
}

/// Workflow: ST-11
#[gitti_test::test]
fn stage_and_unstage_lines_with_expanded_context() {
    use gitti_core::blobs::Version;
    use gitti_core::context::{Reveal, split_lines};
    use gitti_core::rows::{DiffRow, Expansion, LineSel, Src, build_worktree_rows};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    let fx = fixture(&[("a.txt", &numbered(60))]);
    let changed = numbered(60)
        .replace("line 30\n", "line thirty\n")
        .replace("line 31\n", "line 31\nline 31b\n");
    fx.write("a.txt", &changed);
    let ws = fx.state();
    let d = ws.unstaged[0].diff.clone().unwrap();
    // The new version's lines (a file read for the unstaged side), every gap revealed.
    let mut exp = Expansion::default();
    let text = fx.repo.read_versions(&[(Version::Worktree, "a.txt".into())]).unwrap();
    let lines = Arc::new(split_lines(text[0].as_deref().unwrap()));
    assert!(exp.set_context_text(Src::Unstaged, &d, lines));
    assert!(exp.reveal(Src::Unstaged, &d, None, Reveal::All));
    let rows = build_worktree_rows(&ws, &exp, None);
    let ctx = |r: &DiffRow| matches!(r, DiffRow::Context { .. });
    // 61 lines, 9 of them (3 + 2 changed + 1 + 3) in the hunk.
    assert_eq!(rows.iter().filter(|r| ctx(r)).count(), 61 - 9);
    assert!(!rows.iter().any(|r| matches!(r, DiffRow::Expand { .. })));
    // Select from the first revealed line down to "line thirty" (revealed lines included).
    let first = rows.iter().position(ctx).unwrap();
    let thirty = rows
        .iter()
        .position(|r| match *r {
            DiffRow::Line { hunk, line, .. } => d.hunks[hunk].lines[line].text == "line thirty",
            _ => false,
        })
        .unwrap();
    let is_change = |h: usize, l: usize| d.hunks[h].lines[l].is_change();
    let mut sel = LineSel {
        src: Src::Unstaged,
        file: 0,
        anchor_row: first,
        lines: BTreeSet::new(),
    };
    sel.select_range(&rows, thirty, is_change);
    assert_eq!(sel.lines.len(), 2, "-line 30 / +line thirty only: {:?}", sel.lines);
    fx.repo
        .apply_selection(&d, &sel.to_selection(), PatchMode::Stage)
        .unwrap();
    assert_eq!(fx.index("a.txt"), numbered(60).replace("line 30\n", "line thirty\n"));

    // Unstage from the staged side with its context expanded too (index version).
    let ws = fx.state();
    let s = ws.staged[0].diff.clone().unwrap();
    let text = fx.repo.read_versions(&[(Version::Index, "a.txt".into())]).unwrap();
    let lines = Arc::new(split_lines(text[0].as_deref().unwrap()));
    assert!(exp.set_context_text(Src::Staged, &s, lines));
    assert!(exp.reveal(Src::Staged, &s, Some(0), Reveal::More));
    let rows = build_worktree_rows(&ws, &exp, None);
    let staged_rows: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.file_ref() == Some((Src::Staged, 0)))
        .map(|(i, _)| i)
        .collect();
    assert!(staged_rows.iter().any(|&i| ctx(&rows[i])));
    let is_change = |h: usize, l: usize| s.hunks[h].lines[l].is_change();
    let mut sel = LineSel {
        src: Src::Staged,
        file: 0,
        anchor_row: staged_rows[0],
        lines: BTreeSet::new(),
    };
    sel.select_range(&rows, *staged_rows.last().unwrap(), is_change);
    assert_eq!(sel.lines.len(), 2);
    fx.repo
        .apply_selection(&s, &sel.to_selection(), PatchMode::Unstage)
        .unwrap();
    assert_eq!(fx.index("a.txt"), numbered(60));
    assert_eq!(fx.read("a.txt"), changed, "worktree untouched");
}

// ------------------------------------------------------------------- line staging and re-reads

/// Index of the first `+` / `-` line of hunk 0.
fn first_change(f: &FileDiff) -> Option<usize> {
    f.hunks[0].lines.iter().position(|l| l.is_change())
}

/// Workflow: ST-02
#[gitti_test::test]
fn stage_single_lines_and_unstage_again() {
    use gitti_core::diff::LineKind;
    let fx = fixture(&[("a.txt", "a\nb\nc\nd\n")]);
    fx.write("a.txt", "a\nB1\nB2\nc\nd\nE\n");
    let d = fx.unstaged("a.txt");
    // Only "B2": the unselected removal of "b" stays in the index as context.
    let sel = Selection::lines(0, pick(&d, 0, &["B2"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("a.txt"), "a\nb\nB2\nc\nd\n");
    assert_eq!(fx.read("a.txt"), "a\nB1\nB2\nc\nd\nE\n", "worktree untouched");
    // A removal and an addition at the end of the file, in one selection.
    let d = fx.unstaged("a.txt");
    let mut sel = Selection::default();
    for (h, hunk) in d.hunks.iter().enumerate() {
        let lines: Vec<usize> = (0..hunk.lines.len())
            .filter(|&i| {
                let l = &hunk.lines[i];
                (l.kind == LineKind::Removed && l.text == "b") || (l.kind == LineKind::Added && l.text == "E")
            })
            .collect();
        if !lines.is_empty() {
            sel.add_lines(h, lines);
        }
    }
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("a.txt"), "a\nB2\nc\nd\nE\n");
    // Unstage just "B2" again.
    let s = fx.staged("a.txt");
    let sel = Selection::lines(0, pick(&s, 0, &["B2"]));
    fx.repo.apply_selection(&s, &sel, PatchMode::Unstage).unwrap();
    assert_eq!(fx.index("a.txt"), "a\nc\nd\nE\n");
}

/// `core.autocrlf=true` (Git for Windows' default) with CRLF in the working tree: the
/// diff shows the lines without CR, line stage / unstage keep the index LF, a discard
/// writes CRLF back (git converts on checkout). An untracked CRLF file is shown and
/// partially staged with LF, as `git add` stores it; without autocrlf the CR is kept.
/// Workflow: EX-05
#[gitti_test::test]
fn line_staging_with_autocrlf_and_untracked_crlf_file() {
    let fx = fixture(&[("a.txt", "a\nb\nc\n")]);
    let p = fx.repo.workdir();
    crate::fixture_kit::append_config(p, "[core]\n\tautocrlf = true\n");
    fx.write("a.txt", "a\r\nX\r\nb\r\nY\r\nc\r\n");
    fx.write("new.txt", "one\r\ntwo\r\nthree\r\n");
    let d = fx.unstaged("a.txt");
    assert!(d.hunks[0].lines.iter().all(|l| !l.text.ends_with('\r')), "{d:?}");
    let sel = Selection::lines(0, pick(&d, 0, &["X"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("a.txt"), "a\nX\nb\nc\n");
    let s = fx.staged("a.txt");
    let sel = Selection::lines(0, pick(&s, 0, &["X"]));
    fx.repo.apply_selection(&s, &sel, PatchMode::Unstage).unwrap();
    assert_eq!(fx.index("a.txt"), "a\nb\nc\n");

    let ws = fx.state();
    let d = ws.unstaged.iter().find(|f| f.path() == "a.txt").unwrap().diff.clone().unwrap();
    let sel = Selection::lines(0, pick(&d, 0, &["Y"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Discard).unwrap();
    assert_eq!(fx.read("a.txt"), "a\r\nX\r\nb\r\nc\r\n");
    // The untracked file (from the same read).
    let d = ws.unstaged.iter().find(|f| f.path() == "new.txt").unwrap().diff.clone().unwrap();
    let texts: Vec<&str> = d.hunks[0].lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["one", "two", "three"]);
    let sel = Selection::lines(0, pick(&d, 0, &["one", "three"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("new.txt"), "one\nthree\n");
    let ws = fx.state();
    let added = |name: &str| {
        let f = ws.unstaged.iter().find(|f| f.path() == name).unwrap();
        f.diff.as_ref().unwrap().added_removed()
    };
    assert_eq!(added("a.txt"), (1, 0));
    assert_eq!(added("new.txt"), (1, 0));

    // Without autocrlf the bytes are kept, CR included (what `git add` does then).
    crate::fixture_kit::append_config(p, "[core]\n\tautocrlf = false\n");
    fx.write("other.txt", "x\r\n");
    assert_eq!(fx.unstaged("other.txt").hunks[0].lines[0].text, "x\r");
}

/// The partial re-read after an operation gives exactly what a full read gives.
/// `step` runs an operation, re-reads its paths and compares with a full read, which is
/// the next step's previous state.
struct AfterCheck<'a> {
    fx: &'a Fixture,
    opts: gitti_core::DiffOptions,
    prev: WorkingState,
}

impl AfterCheck<'_> {
    fn new(fx: &Fixture, opts: gitti_core::DiffOptions) -> AfterCheck<'_> {
        let prev = fx.repo.working_state_with(&opts).unwrap();
        AfterCheck { fx, opts, prev }
    }
    fn exact(&self, staged: bool, name: &str) -> FileDiff {
        let list = if staged { &self.prev.staged } else { &self.prev.unstaged };
        let f = list.iter().find(|w| w.path() == name);
        f.and_then(|w| w.patch_source().cloned()).unwrap_or_else(|| panic!("{name}"))
    }
    fn step(&mut self, paths: &[&str], what: &str, op: impl FnOnce(&Repo)) {
        op(&self.fx.repo);
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let after = self.fx.repo.working_state_after(&self.opts, &self.prev, &paths).unwrap();
        let full = self.fx.repo.working_state_with(&self.opts).unwrap();
        assert_eq!(format!("{after:?}"), format!("{full:?}"), "{what} ({:?})", self.opts);
        self.prev = full;
    }
}

fn after_fixture() -> Fixture {
    let fx = fixture(&[
        ("a.txt", &numbered(60)),
        ("b.txt", "b1\nb2\nb3\n"),
        ("gone.txt", "bye\n"),
        ("[x] y.txt", "glob\n"),
        ("z.txt", "z\n"),
    ]);
    let edited: String = numbered(60)
        .lines()
        .enumerate()
        .map(|(i, l)| match i {
            5 | 30 | 55 => format!("{l} changed\n"),
            _ => format!("{l}\n"),
        })
        .collect();
    fx.write("a.txt", &edited);
    fx.write("b.txt", "b1\nB2\nb3\n");
    fx.write("[x] y.txt", "glob changed\n");
    fx.write("new.txt", "fresh\nlines\n");
    fx.write("z.txt", "z  \n");
    fs::remove_file(fx.repo.workdir().join("gone.txt")).unwrap();
    fx
}

/// Hunks and lines (also with "ignore whitespace", where staging uses the exact diff).
/// Workflow: ST-18
#[gitti_test::test]
fn working_state_after_hunks_and_lines_match_a_full_read() {
    let fx = after_fixture();
    let mut c = AfterCheck::new(&fx, Default::default());
    let d = c.exact(false, "a.txt");
    c.step(&["a.txt"], "stage a hunk", |r| {
        r.apply_selection(&d, &Selection::whole_hunk(1), PatchMode::Stage).unwrap();
    });
    let d = c.exact(false, "a.txt");
    c.step(&["a.txt"], "stage a line", |r| {
        r.apply_selection(&d, &Selection::lines(0, first_change(&d)), PatchMode::Stage).unwrap();
    });
    let s = c.exact(true, "a.txt");
    c.step(&["a.txt"], "unstage a hunk", |r| {
        r.apply_selection(&s, &Selection::whole_hunk(0), PatchMode::Unstage).unwrap();
    });
    let mut c = AfterCheck::new(&fx, gitti_core::DiffOptions { ignore_whitespace: true, ..Default::default() });
    let d = c.exact(false, "a.txt");
    c.step(&["a.txt"], "stage a hunk (-w)", |r| {
        r.apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage).unwrap();
    });
    c.step(&["z.txt"], "stage a whitespace-only file (-w)", |r| r.stage_paths(&["z.txt"]).unwrap());
}

/// Whole files: untracked, deleted, a name with glob characters, two at once, discard.
/// Workflow: ST-07, DC-08
#[gitti_test::test]
fn working_state_after_whole_files_match_a_full_read() {
    let fx = after_fixture();
    let mut c = AfterCheck::new(&fx, Default::default());
    c.step(&["new.txt"], "stage new.txt", |r| r.stage_paths(&["new.txt"]).unwrap());
    c.step(&["new.txt"], "unstage new.txt", |r| r.unstage_paths(&["new.txt"]).unwrap());
    c.step(&["gone.txt", "[x] y.txt"], "stage deleted + glob name", |r| {
        r.stage_paths(&["gone.txt", "[x] y.txt"]).unwrap()
    });
    c.step(&["[x] y.txt"], "unstage glob name", |r| r.unstage_paths(&["[x] y.txt"]).unwrap());
    let entry = c.prev.unstaged.iter().find(|w| w.path() == "b.txt").unwrap().entry.clone();
    c.step(&["b.txt"], "discard b.txt", |r| {
        r.discard_paths(&[&entry]).unwrap();
    });
}

/// The watcher re-reads a changed directory as a whole (a new one, one removed or
/// moved away — inotify reports just the directory then): everything below it, and only
/// that, is re-read; a sibling with the same prefix is not touched.
/// Workflow: ST-18
#[gitti_test::test]
fn working_state_after_takes_directories() {
    use crate::fixture_kit::{History, f, repo_with};
    let mut h = History::new();
    h.commit("main", &[], "init", &[
        ("dx.txt", f("x\n")),
        ("keep.txt", f("k\n")),
        ("d/a.txt", f("a\n")),
        ("d/sub/b.txt", f("b\n")),
    ]);
    let (_tmp, w, _) = repo_with(&h);
    let repo = Repo::open(&w).unwrap();
    let opts = gitti_core::DiffOptions::default();
    let full = || repo.working_state_with(&opts).unwrap();
    // Edited files below the directory, an untracked tree next to them.
    let prev = full();
    fs::write(w.join("d/a.txt"), "a2\n").unwrap();
    fs::write(w.join("d/sub/b.txt"), "b2\n").unwrap();
    fs::create_dir_all(w.join("d/new/deep")).unwrap();
    fs::write(w.join("d/new/deep/n.txt"), "n\n").unwrap();
    let after = repo.working_state_after(&opts, &prev, &["d".to_string()]).unwrap();
    let prev = full();
    assert_eq!(format!("{after:?}"), format!("{prev:?}"), "edits and a new tree below d");
    // The whole directory moved away: its files are deleted, the untracked ones gone.
    fs::write(w.join("dx.txt"), "x2\n").unwrap();
    let prev = full();
    let away = tempfile::tempdir().unwrap();
    fs::rename(w.join("d"), away.path().join("d")).unwrap();
    let after = repo.working_state_after(&opts, &prev, &["d".to_string()]).unwrap();
    assert_eq!(format!("{after:?}"), format!("{:?}", full()), "d moved away");
    assert!(after.unstaged.iter().any(|f| f.path() == "dx.txt"), "dx.txt kept");
    assert!(!after.unstaged.iter().any(|f| f.path().starts_with("d/new")));
    // Files the re-read covers (to highlight again).
    let touched = gitti_core::touched_files(&prev, &after, &["d".to_string()]);
    assert_eq!(touched, ["d", "d/a.txt", "d/new/deep/n.txt", "d/sub/b.txt"]);
}

/// The in-process preview of an untracked file is byte for byte what `git diff
/// --no-index` prints, both as a new and as a deleted file (the working tree as the old
/// side of a comparison). One `git diff --no-index` of two directories per direction.
#[gitti_test::test]
fn untracked_preview_matches_git_diff_no_index() {
    let tmp = tempfile::tempdir().unwrap();
    let (empty, w) = (tmp.path().join("empty"), tmp.path().join("w"));
    fs::create_dir_all(&empty).unwrap();
    let cases: &[(&str, &[u8])] = &[
        ("plain.txt", b"one\ntwo\nthree\n"),
        ("single.txt", b"only\n"),
        ("no-eol.txt", b"x\ny"),
        ("empty.txt", b""),
        ("crlf.txt", b"a\r\nb\r\n"),
        ("bin.dat", b"ab\0cd\n"),
        ("bin space.dat", b"\0"),
        ("dir/\u{fc}mlaut \u{e4}.txt", "gr\u{fc}\u{df}e\n".as_bytes()),
        ("run.sh", b"#!/bin/sh\n"),
    ];
    for (name, data) in cases {
        let path = w.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, data).unwrap();
    }
    // An executable script (Windows has no executable bit: there it's a plain file).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(w.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    // `git diff --no-index <dir> .` names the files `a/./name`.
    let no_index = |old: &str, new: &str| -> Vec<String> {
        let out = crate::fixture_kit::git(&w, &["-c", "core.quotepath=false", "diff", "--no-index", "--", old, new])
            .output()
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap().replace("a/./", "a/").replace("b/./", "b/");
        text.split_inclusive('\n').fold(Vec::new(), |mut v: Vec<String>, l| {
            if l.starts_with("diff --git ") || v.is_empty() {
                v.push(String::new());
            }
            v.last_mut().unwrap().push_str(l);
            v
        })
    };
    let empty = empty.to_str().unwrap();
    let added = no_index(empty, ".");
    let deleted = no_index(".", empty);
    assert_eq!((added.len(), deleted.len()), (cases.len(), cases.len()));
    for (name, data) in cases {
        let exec = cfg!(unix) && *name == "run.sh";
        let header = format!("diff --git a/{name} b/{name}\n");
        let find = |all: &[String]| all.iter().find(|d| d.starts_with(&header)).cloned().unwrap_or_default();
        assert_eq!(gitti_core::repo::new_file_diff_text(name, data, exec, gix::hash::Kind::Sha1), find(&added), "{name}");
        assert_eq!(gitti_core::repo::file_diff_text(name, data, exec, true, gix::hash::Kind::Sha1), find(&deleted), "{name} deleted");
    }
}

/// Indices of the change lines in hunk `h` whose exact bytes equal one of `lines`.
fn pick_bytes(f: &FileDiff, h: usize, lines: &[&[u8]]) -> Vec<usize> {
    f.hunks[h]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_change() && lines.contains(&l.bytes()))
        .map(|(i, _)| i)
        .collect()
}

/// Line staging is byte-exact for content that is not UTF-8 (`non_utf8.rs` has the
/// encoding matrix; this is the staging suite's representative): Latin-1 lines as context
/// and as selected lines with CRLF endings (no autocrlf: the CR is content), a selected
/// removal and addition, and Shift-JIS lines whose second bytes are `\` (0x5C). The index
/// and the working tree get exactly the expected bytes after stage, unstage and discard.
/// Workflow: EX-02
#[gitti_test::test]
fn non_utf8_lines_stage_unstage_discard_byte_exact() {
    let fx = fixture(&[]);
    let p = fx.repo.workdir().to_path_buf();
    let index = |name: &str| -> Vec<u8> {
        let out = std::process::Command::new("git")
            .current_dir(&p)
            .args(["show", &format!(":{name}")])
            .output()
            .unwrap();
        assert!(out.status.success());
        out.stdout
    };
    let sjis_old: &[u8] = b"\x83\x5c\x83\x74\x83\x67\nkeep\n";
    fs::write(p.join("c.txt"), b"caf\xe9\r\nmid\r\nna\xefve\r\n").unwrap();
    fs::write(p.join("s.txt"), sjis_old).unwrap();
    sh(&p, &["add", "-A"]);
    sh(&p, &["commit", "-q", "-m", "init"]);
    fs::write(p.join("c.txt"), b"caf\xe9\r\nMID\r\n\xe4 new\r\nna\xefve\r\n").unwrap();
    fs::write(p.join("s.txt"), b"\x83\x5c\x83\x74\x83\x67\nkept\n\x95\x5c\x8e\xa6\n").unwrap();

    let d = fx.unstaged("c.txt");
    let sel = Selection::lines(0, pick_bytes(&d, 0, &[b"mid\r", b"\xe4 new\r"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(index("c.txt"), b"caf\xe9\r\n\xe4 new\r\nna\xefve\r\n");
    let d = fx.unstaged("s.txt");
    let sel = Selection::lines(0, pick_bytes(&d, 0, &[b"\x95\x5c\x8e\xa6"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(index("s.txt"), b"\x83\x5c\x83\x74\x83\x67\nkeep\n\x95\x5c\x8e\xa6\n");

    let s = fx.staged("c.txt");
    let sel = Selection::lines(0, pick_bytes(&s, 0, &[b"\xe4 new\r"]));
    fx.repo.apply_selection(&s, &sel, PatchMode::Unstage).unwrap();
    assert_eq!(index("c.txt"), b"caf\xe9\r\nna\xefve\r\n");

    let d = fx.unstaged("c.txt");
    let sel = Selection::lines(0, pick_bytes(&d, 0, &[b"MID\r"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Discard).unwrap();
    assert_eq!(fs::read(p.join("c.txt")).unwrap(), b"caf\xe9\r\n\xe4 new\r\nna\xefve\r\n");
    let d = fx.unstaged("s.txt");
    let sel = Selection::lines(0, pick_bytes(&d, 0, &[b"keep", b"kept"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Discard).unwrap();
    assert_eq!(fs::read(p.join("s.txt")).unwrap(), b"\x83\x5c\x83\x74\x83\x67\nkeep\n\x95\x5c\x8e\xa6\n");
}
