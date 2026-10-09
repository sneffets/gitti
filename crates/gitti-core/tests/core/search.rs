//! "Go to Commit…" smart search against temp repositories: terms guessed as hash / text /
//! file or path, file history following renames, deleted files, ambiguous names, the
//! weak "bare word as file" fallback, combined terms, the hit cap and cancelling; the plain
//! search (subject, author, hash prefix) on the same big history.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use gitti_core::commit_search::{CommitQuery, MatchReason, PathSource, SearchResult};
use gitti_core::{History, LogOrder, ObjectId, Repo};

use crate::fx::{self, Import, Op, put};

/// A body long enough for git's rename detection.
fn body(tag: &str) -> String {
    (0..20).map(|i| format!("line {i} of the parser\n")).collect::<String>() + tag
}

/// History (newest first), all by Ada Lovelace:
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
    fx::init(&p);
    let (v1, v2, v3) = (body("v1"), body("v2"), body("v3"));
    let mut i = Import::new();
    i.author("Ada Lovelace <ada@example.com>");
    let mut c = |msg: &str, ops: &[Op]| {
        i.commit("main", None, &[], msg, ops);
    };
    c(
        "Initial import",
        &[
            put("src/foo.rs", &v1),
            put("gone.txt", "old\n"),
            put("a/util.rs", "a\n"),
            put("b/util.rs", "b\n"),
            put("src/app/navigate.rs", "nav\n"),
            put("notes.txt", "notes\n"),
        ],
    );
    c("Mention FOO.rs in a message", &[put("notes.txt", "notes 2\n")]);
    c("Tweak the parser\n\nAlso fixes the widget.", &[put("src/foo.rs", &v2)]);
    c("Move the parser", &[Op::Mv("src/foo.rs", "src/core/bar.rs")]);
    c("Edit bar", &[put("src/core/bar.rs", &v3)]);
    c("Drop the gone file", &[Op::Del("gone.txt")]);
    c("Refactor helpers", &[put("a/util.rs", "a2\n"), put("b/util.rs", "b2\n")]);
    c("Polish the picker", &[put("src/app/navigate.rs", "nav 2\n")]);
    i.run(&p);
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

#[gitti_test::test]
fn file_history_follows_renames_and_deleted_files() {
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
        MatchReason::Path { path: "src/core/bar.rs".into(), old_path: Some("src/foo.rs".into()) }
    );
    assert_eq!(r.hits[2].path(), Some("src/foo.rs"), "the name at that commit");
    assert_eq!(
        r.report.note().unwrap(),
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

#[gitti_test::test]
fn ambiguous_names_text_and_combined_terms() {
    let (_d, repo) = repo();
    let history = repo.history(LogOrder::Topo).unwrap();
    // Two files named util.rs: both, each hit with its path.
    let r = search(&repo, &history, "util.rs");
    assert_eq!(subjects(&r), ["Refactor helpers", "Initial import"]);
    assert_eq!(r.report.terms[0].path.as_ref().unwrap().paths, ["a/util.rs", "b/util.rs"]);
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

#[gitti_test::test]
fn bare_words_fall_back_to_file_names() {
    let (_d, repo) = repo();
    let history = repo.history(LogOrder::Topo).unwrap();
    // No message says "navigate": the file stem navigate.rs is tried.
    let r = search(&repo, &history, "navigate");
    assert_eq!(subjects(&r), ["Polish the picker", "Initial import"]);
    assert!(r.report.terms[0].path.as_ref().unwrap().weak);
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

/// 230 commits "Step i of the plan" changing n.txt, then "Bob was here" by Bob Builder.
fn big_history(p: &Path) -> Vec<String> {
    fx::init(p);
    let mut i = Import::new();
    let texts: Vec<String> = (0..230).map(|n| format!("{n}\n")).collect();
    for (n, text) in texts.iter().enumerate() {
        i.commit("main", None, &[], &format!("Step {n} of the plan"), &[put("n.txt", text)]);
    }
    i.author("Bob Builder <bob@example.com>");
    i.commit("main", None, &[], "Bob was here", &[put("n.txt", "bob\n")]);
    i.run(p)
}

/// The smart search reports early (file hits while `git log` runs), stops at the cap,
/// and gives no answer once cancelled; the plain search (navigate's "Go to Commit…")
/// matches subjects, authors and hash prefixes in list order.
#[gitti_test::test]
fn early_results_cap_cancel_and_plain_search() {
    let dir = tempfile::tempdir().unwrap();
    let ids = big_history(dir.path());
    let repo = Repo::open(dir.path()).unwrap();
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
    assert_eq!(r.hits[0].summary, "Bob was here");
    assert_eq!(r.hits[1].summary, "Step 229 of the plan");
    // Cancelled: no answer, git killed.
    let cancelled = AtomicBool::new(true);
    assert!(
        history
            .search_smart(repo.git(), &CommitQuery::parse("n.txt"), 200, &cancelled, &mut |_| {})
            .is_none()
    );
    // Hash prefix first.
    let target = ObjectId::from_hex(ids[130].as_bytes()).unwrap();
    let r = search(&repo, &history, &target.to_hex_with_len(10).to_string());
    assert!(r.hits[0].by_hash());
    assert_eq!(r.hits[0].id, target);

    // The plain search: subject, case-insensitive, list order (newest first), limited.
    let plain = |q: &str, limit| history.search(&CommitQuery::parse(q), limit, &no).unwrap();
    let hits = plain("step", 5);
    assert_eq!(hits.len(), 5);
    assert_eq!(hits[0].summary, "Step 229 of the plan");
    assert!(hits.windows(2).all(|h| h[0].row < h[1].row));
    let ones = (0..230).filter(|n: &i32| n.to_string().contains('1')).count();
    assert_eq!(plain("STEP 1 of", 300).len(), ones, "every term must match");
    // Author.
    let bob = plain("builder", 100);
    assert_eq!(bob.len(), 1);
    assert_eq!(bob[0].author, "Bob Builder");
    // Hash prefix: found by hash first, the row is the commit's.
    let target = ObjectId::from_hex(ids[223].as_bytes()).unwrap();
    let hits = plain(&target.to_hex_with_len(8).to_string(), 10);
    assert!(hits[0].by_hash());
    assert_eq!(hits[0].id, target);
    assert_eq!(history.commits.at(hits[0].row).id, target);
    // Nothing / empty query.
    assert!(plain("no such words", 10).is_empty());
    assert!(plain("  ", 10).is_empty());
    // Cancelled before it starts: no answer.
    assert!(history.search(&CommitQuery::parse("step"), 10, &cancelled).is_none());
}
