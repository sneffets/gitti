//! Where the commit-list selection goes when its commit vanished
//! (`gitti_core::reselect::history_row`) beyond the stash and branch workflows (SH-21,
//! BR-20): HEAD rewritten (amend, reset) — the selection follows HEAD; a tag on a commit
//! nothing else reaches deleted — its first parent; the working-directory row stays. And
//! a stash's diff asked for by a stale `stash@{n}`.

use gitti_core::reselect::{Reselect, history_row};
use gitti_core::{LogOrder, ObjectId, Repo};
use gitti_workflow::ext::terminal_git;
use gitti_workflow::fixture::{Fixture, put};

fn oid(hex: &str) -> ObjectId {
    ObjectId::from_hex(hex.as_bytes()).unwrap()
}

#[gitti_test::test]
fn a_vanished_selection_follows_head_else_its_first_parents() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    let one = fx.commit("main", "one", &[put("a.txt", "1\n")]);
    let two = fx.commit("main", "two", &[put("a.txt", "2\n")]);
    // A commit only the tag `t` reaches.
    let tagged = fx.commit_from("tmp", two, "tagged", &[put("t.txt", "t\n")]);
    fx.tag("t", tagged);
    fx.forget("tmp");
    let ids = fx.build(dir);
    let (one, two, tagged) = (oid(&ids[one]), oid(&ids[two]), oid(&ids[tagged]));
    let repo = Repo::open(dir).unwrap();
    let next = |h: &gitti_core::History| repo.history_update(h, LogOrder::Topo).unwrap().expect("a new history");
    let h0 = repo.history(LogOrder::Topo).unwrap();

    // The tag deleted: its commit vanishes, the selection goes to its parent `two`.
    let row = h0.row_of(&tagged).unwrap();
    repo.delete_tag("t").unwrap();
    let h1 = next(&h0);
    assert_eq!(history_row(&h0, row, &h1), Reselect::Replaced(h1.row_of(&two).unwrap()));
    // The working-directory row stays selected.
    assert_eq!(history_row(&h0, 0, &h1), Reselect::Kept(0));

    // HEAD amended: the selection on the old HEAD goes to the new one (not its parent).
    let row = h1.row_of(&two).unwrap();
    assert!(terminal_git(dir, &["commit", "-q", "--amend", "-m", "two again"]).status.success());
    let h2 = next(&h1);
    let head = h2.head().unwrap();
    assert_ne!(head, two);
    assert_eq!(history_row(&h1, row, &h2), Reselect::Replaced(h2.row_of(&head).unwrap()));

    // Reset to `one`: the old HEAD vanishes; the selection follows HEAD to `one`.
    let row = h2.row_of(&head).unwrap();
    assert!(terminal_git(dir, &["reset", "-q", "--hard", &one.to_string()]).status.success());
    let h3 = next(&h2);
    assert_eq!(history_row(&h2, row, &h3), Reselect::Replaced(h3.row_of(&one).unwrap()));
}

/// A stash's diff asked for by a stale `stash@{n}` (a stash above it dropped since the
/// list was read): the diff of the stash asked for by its id — never another one's
/// cached under it; a dropped one is an error, nothing cached.
#[gitti_test::test]
fn a_stash_diff_by_a_stale_index_is_the_stash_asked_for() {
    use gitti_workflow::sh::drop_selected as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    // The list as the app read it: look = stash@{0}, other = stash@{1}.
    assert_eq!(repo.stashes().unwrap().len(), 2);
    assert!(terminal_git(dir, &wf::TERMINAL).status.success());
    let opts = gitti_core::DiffOptions::default();
    let other = repo.load_commit(oid(&s.other), Some(1), &opts).unwrap();
    let paths: Vec<&str> = other.files.iter().map(|f| f.path()).collect();
    assert_eq!(paths, ["b.txt"], "the other stash's diff");
    assert!(repo.load_commit(oid(&s.look), Some(0), &opts).is_err());
    assert!(repo.cached_commit(oid(&s.look), &opts).is_none());
}
