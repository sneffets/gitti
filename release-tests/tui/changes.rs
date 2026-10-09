//! The unstaged section in groups ("Working Directory" / "Untracked Files": fold,
//! stage, discard per group, no extra git), the line totals of sections, groups and
//! comparisons, and panes that never draw into their neighbours at narrow widths.

use crate::common;
use crate::support;

use common::*;
use support::*;

/// a.txt modified (−1 +2), d.txt deleted (−1), new1.txt / new2.txt untracked (+1, +2), a
/// binary untracked bin.dat, s.txt staged (−1 +1).
fn grouped_repo() -> tempfile::TempDir {
    let dir = empty_repo();
    let d = dir.path();
    std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(d.join("d.txt"), "gone\n").unwrap();
    std::fs::write(d.join("s.txt"), "s\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "init"]);
    std::fs::write(d.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    std::fs::remove_file(d.join("d.txt")).unwrap();
    std::fs::write(d.join("new1.txt"), "n\n").unwrap();
    std::fs::write(d.join("new2.txt"), "n\nm\n").unwrap();
    std::fs::write(d.join("bin.dat"), [0u8, 1, 2, 0, 255]).unwrap();
    std::fs::write(d.join("s.txt"), "S\n").unwrap();
    git(d, &["add", "s.txt"]);
    dir
}

/// The diff rows of a dump (the detail pane at 120 columns starts at column 69).
fn detail(screen: &str) -> Vec<String> {
    screen
        .lines()
        .map(|l| l.chars().skip(69).collect::<String>().trim_end().to_string())
        .collect()
}

#[test]
fn unstaged_groups_fold_stage_and_discard_with_totals() {
    let dir = grouped_repo();
    let d = dir.path();
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "120x40",
        "sleep:600;wait;cmds;key:enter;dump;check;\
         key:j;state;key:enter;state;dump;cmds;\
         key:j;state;cmds;key:s;cmds;state;dump;check;\
         key:k;key:k;key:k;state;key:enter;key:d;dump;key:esc;\
         key:j;key:enter;cmds;state",
    );
    let sc = screens(&out);
    let st = states(&out);
    let blocks = cmds(&out);
    let first = detail(sc[0]);
    let has = |rows: &[String], s: &str| rows.iter().any(|r| r.contains(s));
    // Headers with counts and totals (the binary file counts no lines).
    assert!(has(&first, "UNSTAGED FILES (5)   -2   +5"), "{out}");
    assert!(has(&first, "▾ Working Directory (2)   -2   +2"), "{out}");
    assert!(has(&first, "▾ Untracked Files (3)   -0   +3"), "{out}");
    assert!(has(&first, "STAGED FILES (1)   -1   +1"), "{out}");
    let at = |s: &str| first.iter().position(|r| r.contains(s)).unwrap();
    // Tracked files under their header, untracked ones under theirs.
    assert!(at("Working Directory") < at("M a.txt"), "{out}");
    assert!(at("D d.txt") < at("Untracked Files"), "{out}");
    assert!(at("Untracked Files") < at("? new1.txt"), "{out}");
    assert!(at("? new2.txt") < at(" STAGED FILES (1)"), "{out}");
    // j from the section lands on the group header; ⏎ folds it — no git.
    assert!(st[0].contains("cursor=1 Group(Tracked)"), "{out}");
    assert!(st[1].contains("cursor=1 Group(Tracked)"), "{out}");
    let folded = detail(sc[1]);
    assert!(has(&folded, "▸ Working Directory (2)"), "{out}");
    assert!(!has(&folded, "M a.txt") && has(&folded, "? new1.txt"), "{out}");
    assert!(blocks[1].is_empty(), "folding ran git: {out}");
    // j to the untracked header (no git), s stages the group: one `git add`, then only
    // queries of its paths.
    assert!(st[2].contains("Group(Untracked)"), "{out}");
    assert!(blocks[2].is_empty(), "{out}");
    let stage = &blocks[3];
    assert!(stage[0].starts_with("add -A -- "), "{out}");
    for p in ["new1.txt", "new2.txt", "bin.dat"] {
        assert!(stage[0].contains(p), "{out}");
    }
    assert!(!stage[0].contains("a.txt"), "only the group: {out}");
    assert!(stage.len() <= 4, "{stage:#?}");
    for c in &stage[1..] {
        assert!(c.contains(" -- "), "not path-limited: git {c}\n{out}");
    }
    assert!(st[3].contains("unstaged=2 staged=4"), "{out}");
    // Without untracked files the section has no group headers.
    let after = detail(sc[2]);
    assert!(!has(&after, "Working Directory (") && has(&after, "UNSTAGED FILES (2)"), "{out}");
    assert!(has(&after, "STAGED FILES (4)   -1   +4"), "{out}");
    // d on a group asks first (here the whole section again: the group is gone).
    assert!(st[4].contains("cursor=0 Section(Unstaged)"), "{out}");
    assert!(sc[3].contains("Discard all unstaged changes in 2 files"), "{out}");
}

#[test]
fn discarding_a_group_asks_with_its_own_wording() {
    let dir = grouped_repo();
    let d = dir.path();
    // Cursor: section (0) → tracked header (1) → untracked header, below the tracked files.
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "120x40",
        "sleep:600;wait;key:enter;key:j;key:d;dump;key:esc;key:enter;key:j;key:d;dump",
    );
    let sc = screens(&out);
    assert!(sc[0].contains("Discard the changes of 2 tracked files?"), "{out}");
    assert!(sc[1].contains("Delete 3 untracked files?"), "{out}");
}

#[test]
fn compare_title_shows_the_totals() {
    let dir = empty_repo();
    let d = dir.path();
    std::fs::write(d.join("f.txt"), "1\n2\n3\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "A"]);
    std::fs::write(d.join("f.txt"), "1\nTWO\n3\n4\n5\n").unwrap();
    std::fs::write(d.join("g.txt"), "g\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "B"]);
    // Row 1 = B, row 2 = A: mark A, select B.
    let out = gitty(d, "sleep:600;wait;key:j;key:j;key:C;key:k;dump");
    let sc = screens(&out);
    assert!(sc[0].contains("Compare  2 files  −1 +4"), "{out}");
}

/// Twenty branches from one commit: the graph is up to 16 lanes wide.
fn wide_graph_repo() -> tempfile::TempDir {
    let dir = empty_repo();
    let d = dir.path();
    git(d, &["commit", "-q", "--allow-empty", "-m", "root"]);
    for i in 1..=20 {
        git(d, &["switch", "-qc", &format!("b{i}"), "main"]);
        git(d, &["commit", "-q", "--allow-empty", "-m", &format!("c{i}")]);
    }
    git(d, &["switch", "-q", "main"]);
    dir
}

/// The working-directory row's "No changes" ran past a commit list made narrow by
/// a wide graph, into the detail pane. Every pane is clipped to its area now, and `check`
/// reports any cell a pane drew outside it — swept over widths and heights, with the
/// worktree view (groups, totals), the git log and a commit selected.
#[test]
fn narrow_panes_never_draw_into_their_neighbours() {
    let dir = wide_graph_repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(d, config.path(), "70x30", "dump;check");
    let row = out.lines().nth(2).unwrap();
    let list_w = 31;
    assert!(
        row.chars().skip(list_w).take(3).all(|c| c == ' '),
        "'No changes' runs into the detail pane: {row:?}\n{out}"
    );
    assert!(out.contains("check: ok"), "{out}");
    std::fs::write(d.join("new.txt"), "a\nb\n").unwrap();
    std::fs::write(d.join("x.txt"), "x\n").unwrap();
    git(d, &["add", "x.txt"]);
    git(d, &["commit", "-qm", "x"]);
    std::fs::write(d.join("x.txt"), "y\n").unwrap();
    let tour = "check;key:enter;check;key:j;check;key:esc;key:L;check;key:j;check;\
                key:tab;check;resize:45x12;check;resize:75x20;check;resize:140x16;check";
    for size in ["70x30", "71x40", "90x20", "110x40", "120x12", "60x15", "40x10", "200x8"] {
        let out = gitty_with(d, config.path(), size, tour);
        let bad: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("check: ") && *l != "check: ok")
            .collect();
        assert!(bad.is_empty(), "{size}: {bad:?}\n{out}");
    }
}
