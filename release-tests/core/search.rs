//! "Go to Commit…" smart search against temp repositories: terms guessed as hash / text /
//! file or path, file history following renames, deleted files, ambiguous names, the
//! weak "bare word as file" fallback, combined terms, the hit cap and cancelling.

use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use gitti_core::commit_search::{CommitQuery, MatchReason, PathSource, SearchResult};
use gitti_core::{History, LogOrder, ObjectId, Repo};

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
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn commit(dir: &Path, msg: &str) {
    sh(dir, &["add", "-A"]);
    sh(dir, &["commit", "-qm", msg]);
}

/// A body long enough for git's rename detection.
fn body(tag: &str) -> String {
    (0..20).map(|i| format!("line {i} of the parser\n")).collect::<String>() + tag
}

/// History (newest first):
/// 8 "Polish the picker"            M src/app/navigate.rs
/// 7 "Refactor helpers"             M a/util.rs, M b/util.rs
/// 6 "Drop the gone file"           D gone.txt
/// 5 "Edit bar"                     M src/core/bar.rs
/// 4 "Move the parser"              R src/foo.rs → src/core/bar.rs
/// 3 "Tweak the parser" (+ body)    M src/foo.rs
/// 2 "Mention FOO.rs in a message"  M notes.txt
/// 1 "Initial import"               A src/foo.rs, gone.txt, a/util.rs, b/util.rs, src/app/navigate.rs, notes.txt
fn repo() -> (tempfile::TempDir, Repo) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("repo");
    fs::create_dir_all(&p).unwrap();
    sh(&p, &["init", "-q", "-b", "main"]);
    sh(&p, &["config", "user.name", "Ada Lovelace"]);
    sh(&p, &["config", "user.email", "ada@example.com"]);
    sh(&p, &["config", "commit.gpgsign", "false"]);
    sh(&p, &["config", "core.autocrlf", "false"]);
    write(&p, "src/foo.rs", &body("v1"));
    write(&p, "gone.txt", "old\n");
    write(&p, "a/util.rs", "a\n");
    write(&p, "b/util.rs", "b\n");
    write(&p, "src/app/navigate.rs", "nav\n");
    write(&p, "notes.txt", "notes\n");
    commit(&p, "Initial import");
    write(&p, "notes.txt", "notes 2\n");
    commit(&p, "Mention FOO.rs in a message");
    write(&p, "src/foo.rs", &body("v2"));
    sh(&p, &["add", "-A"]);
    sh(&p, &["commit", "-qm", "Tweak the parser", "-m", "Also fixes the widget."]);
    fs::create_dir_all(p.join("src/core")).unwrap();
    sh(&p, &["mv", "src/foo.rs", "src/core/bar.rs"]);
    commit(&p, "Move the parser");
    write(&p, "src/core/bar.rs", &body("v3"));
    commit(&p, "Edit bar");
    fs::remove_file(p.join("gone.txt")).unwrap();
    commit(&p, "Drop the gone file");
    write(&p, "a/util.rs", "a2\n");
    write(&p, "b/util.rs", "b2\n");
    commit(&p, "Refactor helpers");
    write(&p, "src/app/navigate.rs", "nav 2\n");
    commit(&p, "Polish the picker");
    let repo = Repo::open(&p).unwrap();
    (dir, repo)
}

fn search(repo: &Repo, history: &History, q: &str) -> SearchResult {
    let no = AtomicBool::new(false);
    history
        .search_smart(repo.git(), &CommitQuery::parse(q), 200, &no, &mut |_| {})
        .unwrap()
}

fn subjects(r: &SearchResult) -> Vec<&str> {
    r.hits.iter().map(|h| h.summary.as_str()).collect()
}

#[test]
fn file_history_follows_renames() {
    let (_d, repo) = repo();
    let history = repo.history(LogOrder::Topo).unwrap();
    // `bar.rs` is src/core/bar.rs in HEAD; before the move it was src/foo.rs.
    let r = search(&repo, &history, "bar.rs");
    assert_eq!(
        subjects(&r),
        ["Edit bar", "Move the parser", "Tweak the parser", "Initial import"]
    );
    assert_eq!(r.hits[0].path(), Some("src/core/bar.rs"));
    assert_eq!(
        r.hits[1].reasons[0],
        MatchReason::Path {
            path: "src/core/bar.rs".into(),
            old_path: Some("src/foo.rs".into())
        }
    );
    assert_eq!(r.hits[2].path(), Some("src/foo.rs"), "the name at that commit");
    let note = r.report.note().unwrap();
    assert_eq!(
        note,
        "No commit message or author contains 'bar.rs' — showing commits that changed src/core/bar.rs (renames followed)"
    );
    // Rows are the history's rows, in list order.
    assert!(r.hits.windows(2).all(|h| h[0].row < h[1].row));
    assert!(r.hits.iter().all(|h| history.commits.at(h.row).id == h.id));
    // A full path and a folder.
    assert_eq!(subjects(&search(&repo, &history, "src/core/bar.rs")).len(), 4);
    let r = search(&repo, &history, "core/");
    assert_eq!(subjects(&r), ["Edit bar", "Move the parser"]);
    assert_eq!(r.report.terms[0].path.as_ref().unwrap().paths, ["src/core/"]);
}

#[test]
fn deleted_and_renamed_away_files_come_from_history() {
    let (_d, repo) = repo();
    let history = repo.history(LogOrder::Topo).unwrap();
    // Not in HEAD any more: searched through the whole history, any case.
    let r = search(&repo, &history, "GONE.TXT");
    assert_eq!(subjects(&r), ["Drop the gone file", "Initial import"]);
    let p = r.report.terms[0].path.as_ref().unwrap();
    assert_eq!((p.source, p.paths.clone()), (PathSource::History, vec!["gone.txt".to_string()]));
    assert!(r.report.note().unwrap().contains("gone.txt (no longer in HEAD)"));
    // src/foo.rs was renamed away; its old commits are found by the old name, and the
    // commit whose message mentions it, too.
    let r = search(&repo, &history, "foo.rs");
    assert_eq!(
        subjects(&r),
        ["Move the parser", "Tweak the parser", "Mention FOO.rs in a message", "Initial import"]
    );
    assert_eq!(r.hits[2].reasons, [MatchReason::Subject]);
    assert!(r.report.note().unwrap().starts_with("'foo.rs': commits that changed src/foo.rs"));
}

#[test]
fn ambiguous_names_and_text() {
    let (_d, repo) = repo();
    let history = repo.history(LogOrder::Topo).unwrap();
    // Two files named util.rs: both, each hit with its path.
    let r = search(&repo, &history, "util.rs");
    assert_eq!(subjects(&r), ["Refactor helpers", "Initial import"]);
    let p = r.report.terms[0].path.as_ref().unwrap();
    assert_eq!(p.paths, ["a/util.rs", "b/util.rs"]);
    assert!(r.report.note().unwrap().contains("a/util.rs and 1 more"));
    // A narrower path picks one.
    let r = search(&repo, &history, "b/util.rs");
    assert_eq!(r.report.terms[0].path.as_ref().unwrap().paths, ["b/util.rs"]);
    // Full message body.
    let r = search(&repo, &history, "widget");
    assert_eq!(subjects(&r), ["Tweak the parser"]);
    assert_eq!(r.hits[0].reasons, [MatchReason::Message]);
    assert_eq!(r.report.note(), None, "plain text search with hits");
    // Author.
    assert_eq!(search(&repo, &history, "lovelace").hits.len(), 8);
}

#[test]
fn combined_terms_and_partial_matches() {
    let (_d, repo) = repo();
    let history = repo.history(LogOrder::Topo).unwrap();
    // Path AND text.
    let r = search(&repo, &history, "bar.rs edit");
    assert_eq!(subjects(&r), ["Edit bar"]);
    assert_eq!(r.hits[0].reason_label(), "src/core/bar.rs · subject");
    let r = search(&repo, &history, "bar.rs tweak");
    assert_eq!(subjects(&r), ["Tweak the parser"]);
    // Nothing matches both: the ones matching most terms, said so.
    let r = search(&repo, &history, "bar.rs zzzz");
    assert!(r.report.some_terms_only);
    assert_eq!(r.hits.len(), 4);
    let note = r.report.note().unwrap();
    assert!(note.starts_with("No commit matches all of 'bar.rs zzzz'"), "{note}");
    assert!(note.contains("nothing matches 'zzzz'"), "{note}");
}

#[test]
fn bare_words_fall_back_to_file_names() {
    let (_d, repo) = repo();
    let history = repo.history(LogOrder::Topo).unwrap();
    // No message says "navigate": the file stem navigate.rs is tried.
    let r = search(&repo, &history, "navigate");
    assert_eq!(subjects(&r), ["Polish the picker", "Initial import"]);
    let p = r.report.terms[0].path.as_ref().unwrap();
    assert!(p.weak);
    assert_eq!(
        r.report.note().unwrap(),
        "No commit message or author contains 'navigate' — showing commits that changed src/app/navigate.rs"
    );
    // A word with text hits stays a text search (no git run for "parser").
    let before = repo.command_log().snapshot().len();
    let r = search(&repo, &history, "parser");
    assert_eq!(subjects(&r), ["Move the parser", "Tweak the parser"]);
    assert_eq!(repo.command_log().snapshot().len(), before);
    // Nothing at all: said so.
    let r = search(&repo, &history, "nosuch.rs");
    assert!(r.hits.is_empty());
    assert!(r.report.note().unwrap().starts_with("Nothing matches 'nosuch.rs'"));
    let r = search(&repo, &history, "qwertz");
    assert!(r.report.note().unwrap().starts_with("Nothing matches 'qwertz'"));
}

#[test]
fn early_results_cap_and_cancel() {
    let (_d, repo) = repo();
    let w = repo.workdir().to_path_buf();
    for i in 0..230 {
        write(&w, "n.txt", &format!("{i}\n"));
        commit(&w, &format!("Step {i}"));
    }
    let history = repo.history(LogOrder::Topo).unwrap();
    let no = AtomicBool::new(false);
    let mut early = Vec::new();
    let r = history
        .search_smart(repo.git(), &CommitQuery::parse("n.txt"), 200, &no, &mut |r| {
            early.push(r.clone())
        })
        .unwrap();
    assert_eq!(r.hits.len(), 200, "capped");
    assert!(!r.report.complete);
    assert!(r.done);
    assert!(early[0].hits.is_empty(), "first: what is searched");
    assert!(early.iter().any(|e| !e.hits.is_empty()), "file hits reported while git log ran");
    assert!(early.iter().all(|e| !e.done && e.hits.len() <= 200));
    assert!(early.last().unwrap().note().unwrap().starts_with("Searching the history of n.txt"));
    assert_eq!(r.hits[0].summary, "Step 229");
    // Cancelled: no answer, git killed.
    let cancelled = AtomicBool::new(true);
    assert!(
        history
            .search_smart(repo.git(), &CommitQuery::parse("n.txt"), 200, &cancelled, &mut |_| {})
            .is_none()
    );
    // Hash prefix still first.
    let target = ObjectId::from_hex(sh(&w, &["rev-parse", "HEAD~100"]).as_bytes()).unwrap();
    let r = search(&repo, &history, &target.to_hex_with_len(10).to_string());
    assert!(r.hits[0].by_hash());
    assert_eq!(r.hits[0].id, target);
}
