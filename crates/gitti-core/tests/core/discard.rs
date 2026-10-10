//! Discard All on many files: one go, nested repositories kept (and said so), failures
//! collected instead of stopping halfway, progress and a command-log entry for the work
//! done without git.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use gitti_core::Repo;
use gitti_core::discard::{DiscardFailed, DiscardOptions, DiscardPlan, no_preview_note};
use gitti_core::status::StatusEntry;

use crate::fixture_kit::{History, append_config, f, repo_with, sh};

/// A repository with `a.txt` and `.gitignore` (`*.log`) committed.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Repo) {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("a\n")), (".gitignore", f("*.log\n"))]);
    let (tmp, p, _) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    (tmp, p, repo)
}

/// `dirs` × `per` untracked files under `target/debug/deps/` (plain file writes, no git).
fn build_output(p: &Path, dirs: usize, per: usize) {
    for d in 0..dirs {
        let dir = p.join(format!("target/debug/deps/d{d}"));
        fs::create_dir_all(&dir).unwrap();
        for i in 0..per {
            fs::write(dir.join(format!("f{i}.o")), "x").unwrap();
        }
    }
}

fn unstaged(repo: &Repo) -> Vec<StatusEntry> {
    repo.status()
        .unwrap()
        .entries
        .into_iter()
        .filter(StatusEntry::is_unstaged)
        .collect()
}

fn discard(repo: &Repo, entries: &[StatusEntry], opts: DiscardOptions) -> anyhow::Result<gitti_core::discard::DiscardReport> {
    repo.discard_with(&entries.iter().collect::<Vec<_>>(), opts)
}

/// The Rust `target/` case at a representative size (1 000 files — 10 000 take the same
/// path, only longer): every untracked file goes in one call, its now empty folders with
/// it, an ignored file stays (and keeps its folder), the tracked change is restored by one
/// `git restore` (paths on stdin); the deletion is one entry in the command log and
/// reports a meter (once per percent at most) to the operation's progress sink.
#[gitti_test::test]
fn discard_all_deletes_thousands_of_untracked_files_in_one_go() {
    let (_tmp, p, repo) = fixture();
    build_output(&p, 10, 100);
    fs::write(p.join("target/build.log"), "ignored\n").unwrap();
    fs::write(p.join("a.txt"), "changed\n").unwrap();
    let entries = unstaged(&repo);
    assert_eq!(entries.len(), 1001);
    let plan = DiscardPlan::of(&p, &entries);
    assert_eq!((plan.tracked, plan.untracked), (1, 1000));
    assert!(plan.nested.is_empty());
    assert!(plan.detail().contains("1000 untracked files will be deleted (ignored files stay)"));

    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = lines.clone();
    let sink: gitti_core::process::ProgressSink =
        Arc::new(move |l: &str| seen.lock().unwrap().push(l.to_string()));
    let report = gitti_core::process::with_progress(sink, || {
        discard(&repo, &entries, DiscardOptions::default())
    })
    .unwrap();
    assert_eq!((report.deleted, report.restored), (1000, 1));
    assert_eq!(report.note(), None);
    assert!(!p.join("target/debug").exists(), "emptied folders go too");
    assert!(p.join("target/build.log").exists(), "ignored files stay");
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "a\n");
    assert!(unstaged(&repo).is_empty());

    let lines = lines.lock().unwrap();
    assert_eq!(lines[0], "Discarding 1001 files");
    let meters: Vec<_> = lines
        .iter()
        .filter_map(|l| gitti_core::progress::parse_meter(l))
        .collect();
    assert!(meters.len() <= 102, "{} meter lines", meters.len());
    let last = meters.last().unwrap();
    assert_eq!((last.phase.as_str(), last.done, last.total), ("Deleting untracked files", 1000, Some(1000)));
    assert!(last.finished);

    let log = repo.command_log().snapshot();
    let own: Vec<_> = log.iter().filter(|e| e.in_process).collect();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].command_line(), "delete 1000 untracked files (in-process, no git)");
    assert!(own[0].output.contains("deleted target/debug/deps/d0/f0.o"));
    let writes: Vec<_> = log
        .iter()
        .filter(|e| !e.in_process && !e.read_only)
        .map(|e| e.args.join(" "))
        .collect();
    assert_eq!(writes, ["restore --worktree --pathspec-from-file=- --pathspec-file-nul"]);
}

/// A folder that is a repository of its own (`git status` lists it as `nested/`): kept and
/// named, unless deleting it is asked for. Before, `git clean -f` skipped it without a
/// word and it stayed in the list as if Discard had worked only partly.
#[gitti_test::test]
fn nested_repository_is_kept_and_named_unless_asked() {
    let (_tmp, p, repo) = fixture();
    sh(&p, &["init", "-q", "nested"]);
    fs::write(p.join("nested/n.txt"), "n\n").unwrap();
    fs::write(p.join("new.txt"), "new\n").unwrap();
    let entries = unstaged(&repo);
    let plan = DiscardPlan::of(&p, &entries);
    assert_eq!(plan.nested, ["nested/"]);
    assert_eq!(plan.untracked, 1);
    assert!(plan.detail().contains("1 folder is a separate git repository (own .git): nested/"));
    assert_eq!(plan.nested_check(), "Also delete the repository nested/ (and its history)");
    // The diff pane says what it is (no preview).
    let ws = repo.working_state().unwrap();
    assert_eq!(ws.nested_repos, ["nested/"]);
    let wf = |path: &str| ws.unstaged.iter().find(|w| w.path() == path).unwrap();
    assert_eq!(
        no_preview_note(&ws, wf("nested/")).map(|n| n.0),
        Some(gitti_core::discard::NESTED_REPO_NOTE)
    );
    assert_eq!(no_preview_note(&ws, wf("new.txt")), None);
    // A path-limited re-read keeps it (and drops it once it is gone).
    let after = repo
        .working_state_after(&Default::default(), &ws, &["new.txt".to_string()])
        .unwrap();
    assert!(after.is_nested_repo("nested/"));

    let report = discard(&repo, &entries, DiscardOptions::default()).unwrap();
    assert_eq!(report.nested_kept, ["nested/"]);
    assert!(report.note().unwrap().contains("nested/ was kept: it is a separate git repository"));
    assert!(p.join("nested/n.txt").exists());
    assert!(!p.join("new.txt").exists());

    let entries = unstaged(&repo);
    let report = discard(&repo, &entries, DiscardOptions { delete_nested_repos: true }).unwrap();
    assert_eq!((report.deleted, report.note()), (1, None));
    assert!(!p.join("nested").exists());
}

/// `status.showUntrackedFiles=normal`: a new folder is one entry (`dir/`), deleted by
/// `git clean` — a repository inside it is kept and named.
#[gitti_test::test]
fn untracked_folder_entry_keeps_a_repository_inside() {
    let (_tmp, p, repo) = fixture();
    append_config(&p, "[status]\n\tshowUntrackedFiles = normal\n");
    fs::create_dir_all(p.join("dir/deep")).unwrap();
    fs::write(p.join("dir/deep/f.txt"), "f\n").unwrap();
    sh(&p, &["init", "-q", "dir/sub"]);
    let entries = unstaged(&repo);
    assert_eq!(entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["dir/"]);
    let ws = repo.working_state().unwrap();
    assert!(ws.nested_repos.is_empty(), "dir/ itself has no .git");
    assert_eq!(
        no_preview_note(&ws, &ws.unstaged[0]).map(|n| n.0),
        Some(gitti_core::discard::UNTRACKED_FOLDER_NOTE)
    );
    let report = discard(&repo, &entries, DiscardOptions::default()).unwrap();
    assert_eq!(report.nested_kept, ["dir/sub/"]);
    assert!(!p.join("dir/deep").exists());
    assert!(p.join("dir/sub/.git").exists());
}

/// A file that can't be deleted (its folder is read-only) doesn't stop the rest: every
/// other file is deleted, the tracked change restored, and the error names the file.
#[cfg(unix)]
#[gitti_test::test]
fn a_file_that_cannot_be_deleted_does_not_stop_the_others() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, p, repo) = fixture();
    build_output(&p, 3, 10);
    fs::create_dir(p.join("locked")).unwrap();
    fs::write(p.join("locked/x.txt"), "x\n").unwrap();
    fs::write(p.join("a.txt"), "changed\n").unwrap();
    let entries = unstaged(&repo);
    fs::set_permissions(p.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();
    struct Back<'a>(&'a Path);
    impl Drop for Back<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o755));
        }
    }
    let _back = Back(&p.join("locked"));
    let result = discard(&repo, &entries, DiscardOptions::default());
    if !p.join("locked/x.txt").exists() {
        return; // running as root: nothing is read-only
    }
    let err = result.unwrap_err();
    let failed = err.downcast_ref::<DiscardFailed>().expect("a DiscardFailed");
    assert_eq!(failed.0.failed.len(), 1);
    assert_eq!(failed.0.failed[0].0, "locked/x.txt");
    assert_eq!((failed.0.deleted, failed.0.restored), (30, 1));
    assert!(err.to_string().starts_with("1 file could not be discarded\n  locked/x.txt: "));
    assert!(!p.join("target").exists());
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "a\n");
    let own = repo.command_log().snapshot().into_iter().find(|e| e.in_process).unwrap();
    assert_eq!(own.code, 1);
    assert!(own.output.contains("failed: locked/x.txt"), "{}", own.output);
}
