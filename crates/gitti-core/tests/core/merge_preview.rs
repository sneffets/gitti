//! The merge preview in the commit graph: while a merge is in progress the
//! working-directory row is laid out as the pending merge commit — HEAD plus the
//! `MERGE_HEAD` commits as parents, the edges to the latter pending (dashed) until they
//! reach their commit. `MERGE_HEAD` is written by hand (what a conflicting `git merge`
//! leaves), so a fixture costs the three git processes of [`repo_with`].

use std::fs;

use gitti_core::graph::{GraphRow, Half};
use gitti_core::{LogOrder, ObjectId, Repo};

use crate::fixture_kit::{History, f, repo_with};

fn graph(h: &gitti_core::History) -> Vec<GraphRow> {
    h.graph.rows(0..h.graph.len())
}

fn pending_out(row: &GraphRow) -> usize {
    row.segments
        .iter()
        .filter(|s| s.half == Half::Bottom && s.from != s.to && s.pending)
        .count()
}

/// Rows `1..row(merge head)` carry a pending lane; nothing below the last merge head
/// is pending; row 0's dashed edges go out to as many lanes as there are merge heads.
fn assert_preview(h: &gitti_core::History, heads: &[ObjectId]) {
    let rows = graph(h);
    assert_eq!(pending_out(&rows[0]), heads.len());
    assert!(rows[0].segments.iter().all(|s| s.pending == (s.from != s.to)));
    let mut last = 0;
    for m in heads {
        let r = h.row_of(m).unwrap();
        last = last.max(r);
        let lane = rows[r].node;
        // The dashed lane ends in the merge head's node from above, …
        assert!(
            rows[r]
                .segments
                .iter()
                .any(|s| s.half == Half::Top && s.pending && s.to == lane),
            "row {r}: {:?}",
            rows[r].segments
        );
        // … and passes every row between.
        for (i, row) in rows.iter().enumerate().take(r).skip(1) {
            assert!(row.segments.iter().any(|s| s.pending), "row {i}");
        }
    }
    for row in &rows[last + 1..] {
        assert!(row.segments.iter().all(|s| !s.pending));
    }
}

/// `git merge side` stopped with conflicts: the preview, also in the quick first screen;
/// gone when MERGE_HEAD goes (commit / abort); none for a cherry-pick.
#[gitti_test::test]
fn merge_head_makes_the_worktree_row_the_pending_merge() {
    let mut h = History::new();
    let init = h.commit("main", &[], "init", &[("a", f("1\n"))]);
    let s1 = h.commit("side", &[init], "side 1", &[("b", f("1\n"))]);
    let m1 = h.commit("main", &[init], "main 1", &[("a", f("2\n"))]);
    let s2 = h.commit("side", &[s1], "side 2", &[("b", f("2\n"))]);
    let m2 = h.commit("main", &[m1], "main 2", &[("a", f("3\n"))]);
    let (_tmp, dir, ids) = repo_with(&h);
    let (head, side) = (ids[m2], ids[s2]);
    let repo = Repo::open(&dir).unwrap();
    let plain = repo.history(LogOrder::Date).unwrap();
    assert!(plain.merge_heads.is_empty());
    assert!(graph(&plain).iter().flat_map(|r| &r.segments).all(|s| !s.pending));
    assert_eq!(plain.commits.at(0).parents, [head]);

    let merge_head = dir.join(".git/MERGE_HEAD");
    fs::write(&merge_head, format!("{side}\n")).unwrap();
    for h in [
        repo.history(LogOrder::Date).unwrap(),
        repo.history(LogOrder::Topo).unwrap(),
        repo.history_newest(LogOrder::Date, 100).unwrap(),
    ] {
        assert_eq!(h.merge_heads, [side]);
        assert_eq!(h.commits.at(0).parents, [head, side]);
        assert_eq!(h.commits.len(), plain.commits.len());
        assert_preview(&h, &[side]);
    }

    // A cherry-pick's CHERRY_PICK_HEAD is no parent of the commit: no preview.
    fs::remove_file(&merge_head).unwrap();
    fs::write(dir.join(".git/CHERRY_PICK_HEAD"), format!("{side}\n")).unwrap();
    let after = repo.history(LogOrder::Date).unwrap();
    assert!(after.merge_heads.is_empty());
    assert_eq!(after.commits.at(0).parents, [head]);
    assert!(graph(&after).iter().flat_map(|r| &r.segments).all(|s| !s.pending));
}

/// `git merge <sha>` of a commit no ref reaches, plus a branch (octopus): both are
/// loaded (with their metadata) as parents of the pending merge.
#[gitti_test::test]
fn unreachable_merge_head_and_octopus() {
    let mut h = History::new();
    let init = h.commit("main", &[], "init", &[("a", f("1\n"))]);
    let hidden = h.commit("refs/hidden/x", &[init], "hidden work", &[("h", f("1\n"))]);
    let side = h.commit("side", &[init], "side work", &[("b", f("1\n"))]);
    h.commit("main", &[init], "main work", &[("a", f("2\n"))]);
    let (_tmp, dir, ids) = repo_with(&h);
    let (hidden, side) = (ids[hidden], ids[side]);
    let repo = Repo::open(&dir).unwrap();
    let plain = repo.history(LogOrder::Date).unwrap();
    assert!(!plain.contains(&hidden), "no ref reaches it");

    fs::write(dir.join(".git/MERGE_HEAD"), format!("{hidden}\n{side}\n")).unwrap();
    let h = repo.history(LogOrder::Date).unwrap();
    assert_eq!(h.merge_heads, [hidden, side]);
    assert_eq!(h.commits.at(0).parents.len(), 3);
    assert_eq!(h.commits.len(), plain.commits.len() + 1);
    assert_eq!(h.meta(h.row_of(&hidden).unwrap()).summary, "hidden work");
    assert_preview(&h, &[hidden, side]);

    // A MERGE_HEAD naming nothing loadable (garbage, a missing object) is ignored.
    fs::write(dir.join(".git/MERGE_HEAD"), "not a sha\n0123456789012345678901234567890123456789\n")
        .unwrap();
    let h = repo.history(LogOrder::Date).unwrap();
    assert!(h.merge_heads.is_empty());
    assert_eq!(h.commits.at(0).parents.len(), 1);
}
