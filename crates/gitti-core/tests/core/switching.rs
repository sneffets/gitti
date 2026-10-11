//! Switching branches when the `.gitignore` rules differ, and the ways out of a blocked
//! switch (stash, delete / discard, force) in every shape git blocks in.

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::switch::{SwitchBlock, SwitchOutcome, SwitchTarget, SwitchWay};

use crate::fixture_kit::{E, History, f, repo_with, sh};

fn branch(p: &Path) -> String {
    sh(p, &["branch", "--show-current"]).trim().to_string()
}

fn untracked(p: &Path) -> Vec<String> {
    sh(p, &["ls-files", "--others", "--exclude-standard"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn local(name: &str) -> SwitchTarget {
    SwitchTarget::Local(name.into())
}

/// `main` ignores `target/` (and `gen.txt`); `plain` has no `.gitignore` but tracks
/// `gen.txt`.
fn ignore_fixture() -> (tempfile::TempDir, std::path::PathBuf, Repo) {
    let mut h = History::new();
    let root = h.commit("plain", &[], "init", &[("a.txt", f("a\n")), ("gen.txt", f("theirs\n"))]);
    h.commit(
        "main",
        &[root],
        "ignore build output",
        &[(".gitignore", f("target/\ngen.txt\n")), ("gen.txt", E::Delete)],
    );
    let (tmp, p, _) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    (tmp, p, repo)
}

/// The R49 case: a clean branch with lots of ignored build output switches to a branch
/// whose `.gitignore` doesn't ignore it. Ignored files never block (git treats them as
/// expendable: `gen.txt`, ignored here and tracked there, is silently replaced), the
/// build output stays and shows as untracked there — the status line says why — and the
/// way back works and ignores it again.
/// Workflow: BR-10
#[gitti_test::test]
fn switching_to_fewer_ignore_rules_shows_ignored_files_as_untracked() {
    let (_tmp, p, repo) = ignore_fixture();
    for d in 0..3 {
        let dir = p.join(format!("target/debug/d{d}"));
        fs::create_dir_all(&dir).unwrap();
        for i in 0..10 {
            fs::write(dir.join(format!("f{i}.o")), "x").unwrap();
        }
    }
    fs::write(p.join("gen.txt"), "ours, ignored\n").unwrap();
    assert!(untracked(&p).is_empty(), "clean: only ignored files");

    let SwitchOutcome::Done(done) = repo.switch(&local("plain")).unwrap() else {
        panic!("ignored files must not block the switch");
    };
    assert_eq!(branch(&p), "plain");
    assert_eq!(fs::read_to_string(p.join("gen.txt")).unwrap(), "theirs\n", "git replaced it");
    assert_eq!(untracked(&p).len(), 30);
    let note = done.note.clone().expect("a note on the formerly ignored files");
    assert!(note.starts_with("Switched to plain. 30 files that main ignores are not ignored in plain"), "{note}");
    assert!(note.contains("target/") && note.contains("switching back to main ignores them again"), "{note}");
    assert_eq!(done.status_line(), note);

    let SwitchOutcome::Done(back) = repo.switch(&local("main")).unwrap() else {
        panic!("the way back must not be blocked");
    };
    assert_eq!(back.note, None);
    assert_eq!(branch(&p), "main");
    assert!(untracked(&p).is_empty(), "ignored again");
    assert!(p.join("target/debug/d0/f0.o").exists(), "the build output stays");
}

/// A switch between branches with the same `.gitignore` files costs no extra git call
/// (the rules are compared in-process).
/// Workflow: BR-03
#[gitti_test::test]
fn same_ignore_rules_cost_no_extra_git() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("a\n")), (".gitignore", f("*.log\n"))]);
    h.commit("other", &[root], "b", &[("b.txt", f("b\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    let SwitchOutcome::Done(done) = repo.switch(&local("other")).unwrap() else {
        panic!("not switched");
    };
    assert_eq!(done.note, None);
    let cmds: Vec<String> = repo.command_log().snapshot().iter().map(|e| e.args.join(" ")).collect();
    assert_eq!(cmds, ["switch other"]);
}

/// `main` with `a.txt`; `feat` changes `a.txt` and has `x.txt` and a file `d`.
fn block_fixture() -> (tempfile::TempDir, std::path::PathBuf, Repo) {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n2\n3\n")), ("c.txt", f("c\n"))]);
    h.commit(
        "feat",
        &[root],
        "feat",
        &[("a.txt", f("1\n2\nfeat\n")), ("x.txt", f("theirs\n")), ("d", f("a file\n"))],
    );
    let (tmp, p, _) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    (tmp, p, repo)
}

fn blocked(repo: &Repo, target: &SwitchTarget) -> SwitchBlock {
    match repo.switch(target).unwrap() {
        SwitchOutcome::Blocked(block) => block,
        other => panic!("expected a block: {other:?}"),
    }
}

/// A folder with untracked files where the target has a file: git's "would lose untracked
/// files in them" is a block like the others (before, a bare error). Delete & Switch
/// deletes only those files; Stash & Switch keeps them in a stash.
/// Workflow: BR-08
#[gitti_test::test]
fn folder_with_untracked_files_where_the_target_has_a_file() {
    let (_tmp, p, repo) = block_fixture();
    let target = local("feat");
    fs::create_dir(p.join("d")).unwrap();
    fs::write(p.join("d/u.txt"), "mine\n").unwrap();
    fs::write(p.join("keep.txt"), "keep\n").unwrap();
    let block = blocked(&repo, &target);
    assert_eq!(block.dirs, ["d"]);
    assert!(block.untracked.is_empty() && block.local.is_empty(), "{block:?}");
    let text = block.describe("feat");
    assert!(text.contains("These folders hold untracked files, but in feat there is a file"), "{text}");
    assert_eq!(SwitchWay::Delete.label(&block), "Delete & Switch");

    repo.switch_resolving(&target, &block, SwitchWay::Delete).unwrap();
    assert_eq!(branch(&p), "feat");
    assert_eq!(fs::read_to_string(p.join("d")).unwrap(), "a file\n");
    assert_eq!(fs::read_to_string(p.join("keep.txt")).unwrap(), "keep\n");

    sh(&p, &["switch", "-q", "main"]);
    fs::create_dir(p.join("d")).unwrap();
    fs::write(p.join("d/u.txt"), "mine\n").unwrap();
    let block = blocked(&repo, &target);
    let msg = repo.switch_resolving(&target, &block, SwitchWay::Stash).unwrap();
    assert!(msg.contains("1 untracked path saved in stash"), "{msg}");
    assert_eq!(branch(&p), "feat");
    assert_eq!(sh(&p, &["show", "stash@{0}^3:d/u.txt"]), "mine\n");
}

/// An untracked file where the target has a folder: git names it once per file of that
/// folder; the block lists it once.
/// Workflow: BR-08
#[gitti_test::test]
fn untracked_file_where_the_target_has_a_folder_is_listed_once() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("a\n"))]);
    h.commit("feat", &[root], "g", &[("g/1", f("1\n")), ("g/2", f("2\n")), ("g/3", f("3\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    fs::write(p.join("g"), "a file\n").unwrap();
    let block = blocked(&repo, &local("feat"));
    assert_eq!(block.untracked, ["g"]);
}

/// Many blocking untracked files: git's message names only the first ~100 (it cuts every
/// message after 4 KiB) — the block is completed, else Stash & Switch would stash a part
/// and fail again. They go into the stash by one `git stash push` that reads the paths
/// from stdin (a command line has a limit — Windows: 32k characters).
/// Workflow: BR-09
#[gitti_test::test]
fn many_blocking_untracked_files_are_stashed_in_one_go() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("a\n"))]);
    let names: Vec<String> = (0..300).map(|i| format!("gen/a rather long file name {i:03}.txt")).collect();
    let files: Vec<(&str, E)> = names.iter().map(|n| (n.as_str(), f("theirs\n"))).collect();
    h.commit("feat", &[root], "gen", &files);
    let (_tmp, p, _) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    fs::create_dir(p.join("gen")).unwrap();
    for n in &names {
        fs::write(p.join(n), "ours\n").unwrap();
    }
    let target = local("feat");
    let block = blocked(&repo, &target);
    assert!(!block.truncated);
    assert_eq!(block.untracked.len(), 300);
    assert_eq!(block.untracked[299], names[299]);
    let msg = repo.switch_resolving(&target, &block, SwitchWay::Stash).unwrap();
    assert!(msg.contains("300 untracked paths saved in stash"), "{msg}");
    assert_eq!(branch(&p), "feat");
    assert_eq!(fs::read_to_string(p.join(&names[7])).unwrap(), "theirs\n");
    let stash: Vec<String> = repo
        .command_log()
        .snapshot()
        .iter()
        .filter(|e| e.args.first().is_some_and(|a| a == "stash"))
        .map(|e| e.args.join(" "))
        .collect();
    assert_eq!(stash.len(), 1, "{stash:?}");
    assert!(stash[0].ends_with("--pathspec-from-file=- --pathspec-file-nul"), "{stash:?}");
}

/// Local changes that conflict with the target: Discard & Switch drops only those (an
/// unrelated change is carried over); Force Switch drops every change.
/// Workflow: BR-04, BR-08
#[gitti_test::test]
fn discard_and_force_switch_drop_local_changes() {
    let (_tmp, p, repo) = block_fixture();
    let target = local("feat");
    fs::write(p.join("a.txt"), "1\n2\nmine\n").unwrap();
    fs::write(p.join("c.txt"), "c changed\n").unwrap();
    let block = blocked(&repo, &target);
    assert_eq!(block.local, ["a.txt"]);
    assert_eq!(SwitchWay::Delete.label(&block), "Discard & Switch");
    assert!(SwitchWay::Delete.explain(&block, "feat").contains("your changes to the files above are discarded"));
    repo.switch_resolving(&target, &block, SwitchWay::Delete).unwrap();
    assert_eq!(branch(&p), "feat");
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "1\n2\nfeat\n");
    assert_eq!(fs::read_to_string(p.join("c.txt")).unwrap(), "c changed\n", "carried over");

    sh(&p, &["switch", "-q", "--discard-changes", "main"]);
    fs::write(p.join("a.txt"), "1\n2\nmine\n").unwrap();
    fs::write(p.join("c.txt"), "c changed\n").unwrap();
    fs::write(p.join("x.txt"), "ours\n").unwrap();
    let block = blocked(&repo, &target);
    assert_eq!((block.local.len(), block.untracked.len()), (1, 1), "{block:?}");
    repo.switch_resolving(&target, &block, SwitchWay::Force).unwrap();
    assert_eq!(branch(&p), "feat");
    assert_eq!(fs::read_to_string(p.join("x.txt")).unwrap(), "theirs\n");
    assert_eq!(fs::read_to_string(p.join("c.txt")).unwrap(), "c\n", "force drops every change");
}
