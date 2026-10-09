//! Search in changes against a fast-import history: words in changed lines,
//! ranking by proximity, strict mode, merges left out, refining while typing on, the cap
//! with "More…", cancelling.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use gitti_core::change_search::{ChangeSearch, Order, Tier};
use gitti_core::{History, LogOrder, Repo};

use crate::fx::{self, Import, put};

/// Newest first (topological):
/// 10 "Merge side"             (merge; the side's change is in 9)
///  9 "Side: parse token"      d.rs: `parse_token();` — both words in one line
///  8 "Umlaut"                 c.txt: Größe Über
///  7 "Rename value"           a.rs: `let token = old;` → `let token = new;`
///  6 "Add value"              a.rs: + `let token = old;`
///  5 "Near"                   a.rs: `parse` and `token` three lines apart
///  4 "Spread"                 a.rs: `parse`, b.txt: `token`
///  3 "Far in one file"        a.rs: `parse`, `token` 41 lines below
///  2 "One line"               a.rs: `fn parse(t: Token)`
///  1 "Initial"
fn repo() -> (tempfile::TempDir, Repo) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("repo");
    fx::init(&p);
    let filler = |n: usize, tag: &str| (0..n).map(|i| format!("{tag} {i}\n")).collect::<String>();
    let base = filler(60, "line");
    let mut i = Import::new();
    i.commit("main", None, &[], "Initial", &[put("a.rs", &base), put("b.txt", "b\n")]);
    i.commit(
        "main",
        None,
        &[],
        "One line",
        &[put("a.rs", &format!("fn parse(t: Token)\n{base}"))],
    );
    let v3 = format!("fn parse(t: Token)\nparse here\n{}token there\n{}", filler(40, "x"), filler(20, "y"));
    i.commit("main", None, &[], "Far in one file", &[put("a.rs", &v3)]);
    let v4 = format!("{v3}parse again\n");
    i.commit("main", None, &[], "Spread", &[put("a.rs", &v4), put("b.txt", "b\ntoken\n")]);
    let v5 = format!("{v4}z\nparse near\nz\nz\ntoken near\n");
    i.commit("main", None, &[], "Near", &[put("a.rs", &v5)]);
    let v6a = format!("{v5}let token = old;\n");
    i.commit("main", None, &[], "Add value", &[put("a.rs", &v6a)]);
    let v6 = format!("{v5}let token = new;\n");
    i.commit("main", None, &[], "Rename value", &[put("a.rs", &v6)]);
    i.commit("main", None, &[], "Umlaut", &[put("c.txt", "Größe Über\n")]);
    i.commit(
        "side",
        Some(":8"),
        &[],
        "Side: parse token",
        &[put("d.rs", "parse_token();\n")],
    );
    i.commit("main", Some(":8"), &[":9"], "Merge side", &[put("d.rs", "parse_token();\n")]);
    i.run(&p);
    fx::git(&p, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let repo = Repo::open(&p).unwrap();
    (dir, repo)
}

fn history(repo: &Repo) -> Arc<History> {
    Arc::new(repo.history(LogOrder::Topo).unwrap())
}

/// Starts and runs the current query to its end; the subjects of the hits.
fn run(s: &mut ChangeSearch, repo: &Repo, h: &Arc<History>) -> Vec<String> {
    if let Some((generation, job)) = s.start(h) {
        drive(s, repo, generation, job);
    }
    subjects(s, h)
}

fn drive(s: &mut ChangeSearch, repo: &Repo, generation: u64, job: gitti_core::change_search::ScanJob) {
    let no = AtomicBool::new(false);
    job.run(repo.git(), &no, &mut |ev| {
        s.apply(generation, ev);
    });
    s.finished(generation);
}

fn subjects(s: &ChangeSearch, h: &History) -> Vec<String> {
    s.hits()
        .iter()
        .map(|hit| h.meta(h.row_of(&hit.id).unwrap()).summary.clone())
        .collect()
}

#[gitti_test::test]
fn ranks_by_proximity_and_skips_merges() {
    let (_d, repo) = repo();
    let h = history(&repo);
    let mut s = ChangeSearch::new();
    s.set_text("parse token");
    let got = run(&mut s, &repo, &h);
    assert_eq!(
        got,
        ["Side: parse token", "One line", "Near", "Far in one file", "Spread"],
        "{}",
        s.note()
    );
    let tiers: Vec<Tier> = s.hits().iter().map(|h| h.proximity.tier).collect();
    assert_eq!(tiers, [Tier::Line, Tier::Line, Tier::Near, Tier::File, Tier::Commit]);
    assert_eq!(s.hits()[2].proximity.label(), "both words within 4 lines");
    assert_eq!(s.hits()[4].proximity.label(), "both words across 2 files");
    let snippet = s.hits()[1].snippet.clone().unwrap();
    assert_eq!((snippet.path.as_str(), snippet.text.as_str()), ("a.rs", "fn parse(t: Token)"));
    let st = s.status();
    assert_eq!((st.merges, st.total, st.searched, st.capped), (1, 9, 9, false));
    assert!(s.note().starts_with("5 commits · all 9 searched"), "{}", s.note());
    // Newest first instead.
    s.set_order(Order::Newest);
    assert_eq!(
        subjects(&s, &h),
        ["Side: parse token", "Near", "Spread", "Far in one file", "One line"]
    );
}

#[gitti_test::test]
fn strict_needs_the_word_in_the_changed_part() {
    let (_d, repo) = repo();
    let h = history(&repo);
    let mut s = ChangeSearch::new();
    s.set_text("token = ");
    s.set_order(Order::Newest);
    // `token` and `=` both in the changed lines of "Add value" and "Rename value".
    assert_eq!(run(&mut s, &repo, &h), ["Rename value", "Add value"]);
    s.set_strict(true);
    s.set_text("token");
    // Renaming the value didn't change `token`; adding the line did. Smart case: `Token`
    // in "One line" matches the lower-case word too.
    let got = run(&mut s, &repo, &h);
    assert!(!got.contains(&"Rename value".to_string()), "{got:?}");
    assert!(got.contains(&"Add value".to_string()) && got.contains(&"One line".to_string()));
    s.set_strict(false);
    s.set_text("Token");
    assert_eq!(run(&mut s, &repo, &h), ["One line"], "case-sensitive with a capital");
    // Non-ASCII, folded on both sides (and in git's prefilter).
    s.set_text("über größe");
    assert_eq!(run(&mut s, &repo, &h), ["Umlaut"]);
    s.set_text("nowhere");
    assert!(run(&mut s, &repo, &h).is_empty());
    assert!(s.note().starts_with("No commit has it in changed lines"), "{}", s.note());
}

#[gitti_test::test]
fn typing_on_rechecks_only_the_previous_hits() {
    let (_d, repo) = repo();
    let h = history(&repo);
    let mut s = ChangeSearch::new();
    s.set_order(Order::Newest);
    s.set_text("pars");
    assert_eq!(run(&mut s, &repo, &h).len(), 5);
    s.set_text("parse toke");
    let (generation, job) = s.start(&h).unwrap();
    // One chunk (9 commits), limited to the 5 hits of `pars`.
    assert_eq!(job.todo.len(), 1);
    assert_eq!(job.todo[0].1.as_ref().map(Vec::len), Some(5));
    drive(&mut s, &repo, generation, job);
    assert_eq!(subjects(&s, &h).len(), 5);
    // Not a refinement: all commits again.
    s.set_text("value");
    let (_, job) = s.start(&h).unwrap();
    assert_eq!(job.todo[0].1, None);
    assert_eq!(s.jobs, 3);
}

#[gitti_test::test]
fn the_cap_stops_and_more_continues() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("repo");
    fx::init(&p);
    let mut i = Import::new();
    // 40 commits: chunks of 32 and 8.
    for n in 0..40 {
        let text = format!("needle {n}\n");
        i.commit("main", None, &[], &format!("c{n}"), &[put("f.txt", &text)]);
    }
    i.run(&p);
    fx::git(&p, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let repo = Repo::open(&p).unwrap();
    let h = history(&repo);
    let mut s = ChangeSearch::new();
    s.set_cap(5);
    s.set_text("needle");
    let (generation, mut job) = s.start(&h).unwrap();
    // One worker: the first chunk ends the scan (parallel ones would take the second too).
    job.workers = 1;
    drive(&mut s, &repo, generation, job);
    let st = s.status();
    assert!(st.capped && !st.running, "{st:?}");
    assert_eq!((st.hits, st.searched, st.total), (32, 32, 40));
    assert!(s.note().starts_with("First 32 commits · 32 of 40"), "{}", s.note());
    let (generation, job) = s.more().unwrap();
    assert_eq!(job.todo.len(), 1);
    drive(&mut s, &repo, generation, job);
    let st = s.status();
    assert_eq!((st.hits, st.searched, st.capped), (40, 40, false));
    assert!(s.more().is_none());
}

#[gitti_test::test]
fn cancelled_scan_runs_nothing() {
    let (_d, repo) = repo();
    let h = history(&repo);
    let mut s = ChangeSearch::new();
    s.set_text("parse");
    let (generation, job) = s.start(&h).unwrap();
    let cancel = AtomicBool::new(true);
    let t = Instant::now();
    let mut events = 0;
    job.run(repo.git(), &cancel, &mut |ev| {
        events += 1;
        s.apply(generation, ev);
    });
    assert!(t.elapsed().as_millis() < 500);
    assert_eq!(s.hits().len(), 0);
    // An event of an older generation is dropped.
    s.cancel();
    assert!(!s.running());
    let _ = events;
}
