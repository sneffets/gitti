//! "Clean Untracked / Ignored Files…" (`gitti_core::clean`): the dry-run list (folders
//! collapsed as git shows them, the rule of each ignored entry, folders with a `.git` of
//! their own found inside listed folders), the default ticks, and the deletion — exactly
//! what is ticked, nested repositories only when ticked themselves. The workflow DC-10's
//! gitti and gitty drivers are `tests/ui/wf_discard.rs` and `tests/tui/wf_discard.rs`.

use std::fs;

use gitti_core::Repo;
use gitti_core::clean::{Choice, Kind, Nested};
use gitti_workflow::dc;

use crate::fixture_kit::{History, append_config, f, repo_with, sh};

/// The list of DC-10's fixture: every entry, its kind, files, rule and what it is; the
/// ticks it opens with; two git processes.
/// Workflow: DC-10
#[gitti_test::test]
fn list_names_rules_files_and_nested_repositories() {
    use dc::clean as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let before = repo.command_log().snapshot().len();
    let list = repo.clean_list().unwrap();
    let ran = repo.command_log().snapshot().len() - before;
    assert_eq!(ran, 2, "one status, one check-ignore");
    let paths: Vec<&str> = list.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, wf::ENTRIES, "vendor/ holds only the repository: that is the entry");
    let by = |p: &str| list.entries.iter().find(|e| e.path == p).unwrap();
    assert_eq!(by(".env").rule.as_deref(), Some(".git/info/exclude:1: .env"));
    assert_eq!(by("app.log").rule.as_deref(), Some(".gitignore:2: *.log"));
    assert_eq!(by("notes/debug.log").rule.as_deref(), Some(".gitignore:2: *.log"));
    assert_eq!(by("target/").rule.as_deref(), Some(".gitignore:1: target/"));
    assert_eq!(by("new.txt").rule, None);
    // `notes/` without its ignored file, `target/` without the worktree.
    assert_eq!((by("notes/").kind, by("notes/").files), (Kind::Untracked, 2));
    assert_eq!((by("target/").kind, by("target/").files), (Kind::Ignored, 2));
    let wt = by("target/wt/");
    assert_eq!((wt.kind, wt.nested), (Kind::Ignored, Some(Nested::Worktree)));
    assert_eq!(wt.rule.as_deref(), Some(".gitignore:1: target/"), "its folder's rule");
    let lib = by("vendor/lib/");
    assert_eq!((lib.kind, lib.nested), (Kind::Untracked, Some(Nested::Repository)));
    assert_eq!(
        lib.label(),
        "vendor/lib/ — separate repository (own .git)"
    );
    assert_eq!(by("target/").label(), "target/ (2 files) — ignored by .gitignore:1: target/");
    // Ticked: the untracked entries that aren't repositories; no ignored one.
    let ticked: Vec<&str> = list
        .entries
        .iter()
        .filter(|e| e.ticked())
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(ticked, ["new.txt", "notes/"]);
    assert_eq!(list.default_choice(), Choice::Untracked);
    assert_eq!(list.choice_label(Choice::Ignored), "Ignored (5)");
    let live = list.live(Choice::Untracked, &list.default_ticks());
    assert_eq!(live.text, "Deletes 3 untracked files. This can't be undone.");
}

/// DC-10: Untracked and Ignored, the default ticks and `target/`: exactly those files
/// go — an ignored file in the ticked new folder, the worktree in the ticked build folder
/// and the nested repository stay — in-process (no git but the list's), named in the
/// report and the command log.
/// Workflow: DC-10
#[gitti_test::test]
fn dc10_clean_untracked_and_ignored_files() {
    use dc::clean as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let list = repo.clean_list().unwrap();
    let mut ticks = list.default_ticks();
    ticks[wf::TARGET] = true;
    let chosen = list.chosen(Choice::Both, &ticks);
    let before = repo.command_log().snapshot().len();
    let report = repo.clean_chosen(&list, &chosen).unwrap();
    assert_eq!(report.files, wf::DELETED_FILES);
    // `notes/` and `target/` keep what wasn't ticked in them.
    assert_eq!(report.deleted, ["new.txt"]);
    assert!(report.kept.is_empty() && report.failed.is_empty(), "{report:?}");
    assert_eq!(report.note(), "Cleaned: deleted 5 files (new.txt)");
    let log = repo.command_log().snapshot();
    let entries = &log[before..];
    assert_eq!(entries.len(), 1, "in-process: one log entry, no git");
    assert_eq!(entries[0].args, ["clean: delete 5 files"]);
    assert!(entries[0].output.contains("target/ (2 files)"), "{}", entries[0].output);
    wf::expect(dir).unwrap();
}

/// A repository of its own at the top (`git status` lists it as `nested/`) is never
/// ticked; ticked, it goes whole — its `.git` too.
#[gitti_test::test]
fn nested_repository_goes_only_when_ticked() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("a\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    sh(&p, &["init", "-q", "nested"]);
    fs::write(p.join("nested/n.txt"), "n\n").unwrap();
    fs::write(p.join("new.txt"), "new\n").unwrap();
    let repo = Repo::open(&p).unwrap();
    let list = repo.clean_list().unwrap();
    let paths: Vec<&str> = list.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["nested/", "new.txt"]);
    assert_eq!(list.entries[0].nested, Some(Nested::Repository));
    assert_eq!(list.default_ticks(), [false, true]);
    let chosen = list.chosen(Choice::Untracked, &list.default_ticks());
    repo.clean_chosen(&list, &chosen).unwrap();
    assert!(!p.join("new.txt").exists());
    assert!(p.join("nested/n.txt").exists() && p.join("nested/.git").is_dir());
    let list = repo.clean_list().unwrap();
    let report = repo.clean_chosen(&list, &[0]).unwrap();
    assert_eq!(report.deleted, ["nested/"]);
    assert!(!p.join("nested").exists());
    assert_eq!(repo.clean_list().unwrap().entries, []);
}

/// `core.excludesFile` names its file; only ignored files ticked: the untracked one stays.
#[gitti_test::test]
fn excludes_file_rule_and_ignored_only() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("a\n"))]);
    let (tmp, p, _) = repo_with(&h);
    let global = tmp.path().join("global-ignore");
    fs::write(&global, "# editor files\n*.swp\n").unwrap();
    append_config(&p, &format!("[core]\n\texcludesFile = {}\n", global.display()));
    fs::write(p.join("a.txt.swp"), "swap\n").unwrap();
    fs::write(p.join("new.txt"), "new\n").unwrap();
    let repo = Repo::open(&p).unwrap();
    let list = repo.clean_list().unwrap();
    let swp = &list.entries[0];
    assert_eq!(swp.path, "a.txt.swp");
    let rule = swp.rule.as_deref().unwrap();
    assert!(rule.ends_with("global-ignore:2: *.swp"), "{rule}");
    let mut ticks = list.default_ticks();
    assert_eq!(ticks, [false, true]);
    ticks[0] = true;
    let chosen = list.chosen(Choice::Ignored, &ticks);
    assert_eq!(chosen, [0], "new.txt is ticked but not shown under Ignored");
    repo.clean_chosen(&list, &chosen).unwrap();
    assert!(!p.join("a.txt.swp").exists());
    assert!(p.join("new.txt").exists());
}
