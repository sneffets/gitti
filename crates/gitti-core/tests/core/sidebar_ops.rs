//! The git commands behind the sidebar menus' new entries: create a branch from a
//! stash. (Prune a remote and push all tags are covered with the other remote commands
//! in `push.rs`.)

use std::fs;

use gitti_core::Repo;

use crate::fx::{self, Import, put};

#[gitti_test::test]
fn stash_branch_checks_out_a_branch_with_the_stash() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path();
    fx::init(work);
    let mut i = Import::new();
    let base = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.commit("next", Some(&base), &[], "two", &[put("b.txt", "two\n")]);
    let ids = i.run(work);
    fx::checkout(work);
    fs::write(work.join("a.txt"), "changed\n").unwrap();
    fx::git(work, &["stash", "-q"]);
    // HEAD moves on; the branch starts where the stash was made.
    fx::git(work, &["reset", "-q", "--hard", "next"]);
    let repo = Repo::open(work).unwrap();
    repo.stash_branch("from-stash", repo.stashes().unwrap()[0].target()).unwrap();
    assert_eq!(
        fs::read_to_string(work.join(".git/HEAD")).unwrap().trim(),
        "ref: refs/heads/from-stash"
    );
    assert_eq!(fx::head(work), Some(ids[0].clone()));
    assert_eq!(fs::read_to_string(work.join("a.txt")).unwrap(), "changed\n");
    assert!(repo.stashes().unwrap().is_empty(), "the stash is dropped");
}
