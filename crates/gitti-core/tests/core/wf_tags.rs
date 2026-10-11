//! Workflow tests of tags (catalog: `crates/gitti-workflow/WORKFLOWS.md`, TG): the steps
//! through the `Repo` API as the apps call them, the repository checked by the workflow's
//! own `expect` (`gitti_workflow::tg`); the gitti and gitty drivers of the same workflows
//! are `tests/ui/wf_tags.rs` and `tests/tui/wf_tags.rs`.

use gitti_core::Repo;
use gitti_core::tag::new_tag_problem;
use gitti_workflow::tg;

/// TG-03: one tag pushed by name, then all tags.
/// Workflow: TG-03
#[gitti_test::test]
fn tg03_push_one_tag_then_all() {
    use tg::push_and_delete as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    repo.push_tag("origin", "v2").unwrap();
    wf::expect_pushed_one(&s).unwrap();
    repo.push_all_tags("origin").unwrap();
    wf::expect_pushed(&dir, &s).unwrap();
}

/// TG-04: `v1` deleted on `origin`, then here.
/// Workflow: TG-04
#[gitti_test::test]
fn tg04_delete_a_tag_on_the_remote_and_here() {
    use tg::push_and_delete as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    repo.delete_remote_tag("origin", "v1").unwrap();
    wf::expect_deleted_on_origin(&s).unwrap();
    repo.delete_tag("v1").unwrap();
    wf::expect_deleted(&dir, &s).unwrap();
}

/// TG-05: the name of an existing tag is refused before git (git would refuse it too:
/// the tag stays where it was); a new name goes through.
/// Workflow: TG-05
#[gitti_test::test]
fn tg05_an_existing_tag_name_is_refused_before_git() {
    use tg::existing_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let refs = repo.refs().unwrap();
    let why = new_tag_problem(wf::TAKEN, &refs).expect("v1 refused");
    assert!(why.starts_with(&format!("{}{})", wf::REFUSED, &s.init[..7])), "{why}");
    assert!(repo.create_tag(wf::TAKEN, "HEAD", None).is_err(), "git refuses it as well");
    assert_eq!(new_tag_problem(wf::NEW_NAME, &refs), None);
    repo.create_tag(wf::NEW_NAME, "HEAD", None).unwrap();
    wf::expect(dir, &s).unwrap();
}

/// TG-07: an annotated tag with a message of several lines (one starting with `#`) on a
/// detached HEAD.
/// Workflow: TG-07
#[gitti_test::test]
fn tg07_tag_a_detached_commit() {
    use tg::detached as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    assert_eq!(repo.head().unwrap().branch, None, "detached");
    repo.create_tag(wf::NAME, "HEAD", Some(wf::MESSAGE)).unwrap();
    wf::expect(dir, &s).unwrap();
    let details = repo.tag_details(wf::NAME).unwrap();
    assert_eq!(details.target, s.init);
    assert_eq!(details.annotated.map(|a| a.message.trim_end().to_string()).as_deref(), Some(wf::MESSAGE));
}

/// TG-06: the nested fixture as core reads it — tags of tags peel to the commit, the
/// blob's tags point at the blob (the apps' drivers show them).
#[gitti_test::test]
fn tg06_fixture_reads_as_git_writes_it() {
    use tg::nested as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let refs = repo.refs().unwrap();
    let target = |n: &str| refs.iter().find(|r| r.short_name == n).map(|r| r.target.to_string());
    assert_eq!(target(wf::OUTER), Some(s.init.clone()));
    assert_eq!(target("light-blob"), Some(s.blob.clone()));
    let d = repo.tag_details(wf::OUTER).unwrap();
    assert_eq!(d.annotated.map(|a| a.message.trim_end().to_string()).as_deref(), Some(wf::OUTER_MESSAGE));
    wf::expect(dir, &s).unwrap();
}
