//! Workflow tests, worktrees / submodules / Git LFS (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, WT, SM, LF): the steps through the `Repo` API as
//! the apps call it, the repository checked by the workflow's own `expect`
//! (`gitti_workflow::{wt, sm, lf}`); the gitti and gitty drivers of the same workflows are
//! `tests/ui/wf_modules.rs` and `tests/tui/wf_modules.rs`.

use gitti_core::Repo;
use gitti_core::worktree::{AddInput, AddMode, plan_add};
use gitti_workflow::{lf, sm, wt};

/// WT-03: a change staged and committed in a repository opened at a linked worktree —
/// the branch shared with the main worktree moves, the main worktree is untouched.
/// Workflow: WT-03
#[gitti_test::test]
fn wt03_stage_and_commit_in_a_linked_worktree() {
    use wt::commit_in_linked_worktree as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    let s = wf::fixture(&dir);
    let repo = Repo::open(&s.linked).unwrap();
    repo.stage_paths(&["a.txt"]).unwrap();
    repo.commit(wf::WORK, false).unwrap();
    wf::expect_committed(&dir, &s).unwrap();
}

/// WT-07: `origin/topic` typed as the branch of a new worktree plans a local `topic`
/// tracking it, and adding it does that.
/// Workflow: WT-07
#[gitti_test::test]
fn wt07_track_a_remote_branch_in_a_new_worktree() {
    use wt::track_remote_branch as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let remotes: Vec<String> = repo.remotes().unwrap().into_iter().map(|r| r.name).collect();
    let input = AddInput {
        branch: "origin/topic",
        detached: false,
        start: "HEAD",
        start_label: "HEAD",
        path: &s.linked,
        remotes: &remotes,
    };
    let plan = plan_add(&repo.refs().unwrap(), &repo.worktrees().unwrap(), &input);
    let mode = plan.runnable().expect("runnable").clone();
    assert_eq!(mode, AddMode::Track { name: "topic".into(), upstream: "origin/topic".into() });
    repo.add_worktree(&s.linked, &mode).unwrap();
    wf::expect_added(&dir, &s).unwrap();
}

/// The submodules of `repo` as the apps list them (one snapshot).
fn submodules(repo: &Repo) -> Vec<gitti_core::submodule::Submodule> {
    repo.snapshot(&Default::default()).unwrap().submodules
}

/// SM-05: the local work in the submodule is named before the removal; Remove takes the
/// checkout, its repository in `.git/modules`, `.gitmodules` and its config, staged.
/// Workflow: SM-05
#[gitti_test::test]
fn sm05_remove_a_submodule_with_local_work() {
    use sm::remove_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let work = gitti_core::submodule::local_work(&dir, &submodules(&repo), sm::PATH);
    assert!(work.len() == 1 && work[0].untracked, "{work:#?}");
    wf::expect_untouched(&dir).unwrap();
    repo.remove_submodule(sm::PATH).unwrap();
    wf::expect_removed(&dir).unwrap();
}

/// SM-06: an older commit of the submodule checked out from the superproject: detached
/// there, the pointer change seen (not recorded).
/// Workflow: SM-06
#[gitti_test::test]
fn sm06_check_out_a_commit_in_a_submodule() {
    use sm::checkout_commit_in_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let one: gitti_core::ObjectId = s.lib_one.parse().unwrap();
    repo.checkout_submodule_commit(sm::PATH, one).unwrap();
    wf::expect_checked_out(&dir, &s).unwrap();
}

/// SM-07: a commit made in the submodule's own repository; the superproject sees the
/// new pointer, stages it as one unit and records it.
/// Workflow: SM-07
#[gitti_test::test]
fn sm07_commit_inside_a_submodule() {
    use sm::commit_in_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    let s = wf::fixture(&dir);
    let lib = Repo::open(sm::sub(&dir)).unwrap();
    lib.stage_paths(&["l.txt"]).unwrap();
    lib.commit(wf::LIB_THREE, false).unwrap();
    wf::expect_committed(&dir, &s).unwrap();
    let repo = Repo::open(&dir).unwrap();
    let state = repo.working_state().unwrap();
    let unstaged: Vec<&str> = state.unstaged.iter().map(|f| f.path()).collect();
    assert_eq!(unstaged, [sm::PATH]);
    repo.stage_paths(&[sm::PATH]).unwrap();
    repo.commit(wf::UPDATE_LIB, false).unwrap();
    wf::expect_recorded(&dir, &s).unwrap();
}

/// The global config with git-lfs's filters, for `Repo::set_env` / `Git::set_env`
/// (written into `dir`).
fn lfs_global_config(dir: &std::path::Path) -> Vec<(String, String)> {
    let path = dir.join("global.gitconfig");
    std::fs::write(&path, lf::LFS_FILTERS).unwrap();
    vec![
        ("GIT_CONFIG_GLOBAL".into(), path.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]
}

/// The workflow's pointer constants are what gitti-core computes.
#[gitti_test::test]
fn lf_pointers_are_what_gitti_core_writes() {
    use gitti_core::lfs::Pointer;
    for c in [lf::whole_file_only::DOC1, lf::whole_file_only::DOC2, lf::clone_content::X] {
        assert_eq!(Pointer::of(c.data).text(), c.pointer());
    }
}

/// LF-01: an LFS file's change shows the content's lines; a hunk of it is refused before
/// git runs; the whole file stages the new pointer.
/// Workflow: LF-01
#[gitti_test::test]
fn lf01_an_lfs_file_stages_only_whole() {
    use gitti_core::lfs::LfsContent;
    use gitti_core::patch::{PatchMode, Selection};
    use lf::whole_file_only as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let state = repo.working_state().unwrap();
    let doc = state.unstaged.iter().find(|f| f.path() == "doc.dat").and_then(|f| f.diff.clone()).expect("doc.dat");
    assert_eq!(doc.lfs.as_ref().map(|l| l.content), Some(LfsContent::Diffed));
    let err = repo.apply_selection(&doc, &Selection::whole_hunk(0), PatchMode::Stage).unwrap_err();
    assert!(err.to_string().contains(wf::REFUSED), "{err}");
    wf::expect_unstaged(&dir).unwrap();
    repo.stage_paths(&["doc.dat"]).unwrap();
    wf::expect_staged(&dir).unwrap();
}

/// LF-06: a clone of a repository with an LFS file (git-lfs installed) has the content
/// and nothing to commit.
/// Workflow: LF-06
#[gitti_test::test]
fn lf06_clone_checks_out_the_content() {
    use lf::clone_content as wf;
    if !gitti_core::lfs::LfsCaps::get().installed() {
        println!("note: git-lfs isn't installed — LF-06 (clone with LFS content) is not checked");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(tmp.path());
    let work = base.join("work");
    let head = wf::fixture(&work);
    let env = lfs_global_config(&base);
    let mut git = gitti_core::git_cli::Git::new(&base);
    git.set_env(env.clone());
    let clone = base.join("clone");
    let url = gitti_workflow::rm::url_of(&wf::remote(&work));
    gitti_core::create::clone_repo(&git, &url, &clone, &mut |_| {}).unwrap();
    wf::expect_cloned(&clone, &head).unwrap();
    let mut repo = Repo::open(&clone).unwrap();
    repo.set_env(env);
    let state = repo.working_state().unwrap();
    assert!(state.unstaged.is_empty() && state.staged.is_empty(), "nothing to commit");
}
