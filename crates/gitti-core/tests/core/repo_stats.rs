//! Repository Statistics (`gitti_core::repo_stats`) against small fixtures: the overview
//! from the index and the loaded history, the object store from `git count-objects`,
//! commits per author with `.mailmap`, the whole-history count's progress and cancel, the
//! commit encoding set and the ones the history holds.

use std::sync::atomic::AtomicBool;

use gitti_core::commit_encoding::SetIn;
use gitti_core::repo_stats::{self, AllCount, Scope, Stats, TextOptions};
use gitti_core::{LogOrder, Repo};

use crate::fixture_kit::{History, append_config, f, repo_with, sh};

/// main: Alice ×3 (one adds `.mailmap`), Bob under two addresses ×2 (mapped to
/// "Bob Builder"); side: one commit by T; a tag, a remote-tracking branch of `origin`
/// (a promisor remote: a partial clone), a stash. Five git processes.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let mut h = History::new();
    h.author("Alice <alice@example.com>");
    let a1 = h.commit("main", &[], "one", &[("a.txt", f("hello\n"))]);
    let a2 = h.commit(
        "main",
        &[a1],
        "two",
        &[(".mailmap", f("Bob Builder <bob@example.com>\nBob Builder <bob@example.com> <bobby@old.example>\n"))],
    );
    h.author("Bob <bob@example.com>");
    let b1 = h.commit("main", &[a2], "three", &[("b.txt", f("0123456789"))]);
    h.author("bobby <bobby@old.example>");
    let b2 = h.commit("main", &[b1], "four", &[("dir/c.txt", f("abc\n"))]);
    h.author("Alice <alice@example.com>");
    let a3 = h.commit("main", &[b2], "five", &[("a.txt", f("hello, world\n"))]);
    h.author("T <t@example.com>");
    h.commit("side", &[a1], "side", &[("s.txt", f("s\n"))]);
    h.tag("v1", a3, "release");
    h.set_ref("refs/remotes/origin/main", b2);
    let (tmp, dir, _) = repo_with(&h);
    append_config(
        &dir,
        "[remote \"origin\"]\n\turl = https://example.com/r.git\n\tpromisor = true\n",
    );
    std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
    sh(&dir, &["stash", "-q"]);
    (tmp, dir)
}

fn options<'a>(date: &'a dyn Fn(i64) -> String) -> TextOptions<'a> {
    TextOptions {
        date,
        now: 1_700_000_000 + 400 * 86_400,
        count_all: "Count All Commits",
        change_encoding: "Repository ▸ Commit Encoding…",
    }
}

#[gitti_test::test]
fn overview_storage_and_authors() {
    let (_tmp, dir) = fixture();
    let repo = Repo::open(&dir).unwrap();
    let history = repo.history(LogOrder::Topo).unwrap();
    let o = repo.stats_overview(Some(&history));
    assert_eq!((o.branches, o.remote_branches, o.tags, o.other_refs), (2, 1, 1, 0));
    assert_eq!((o.remotes, o.stashes, o.linked_worktrees), (1, 1, 0));
    assert_eq!(o.promisors, vec!["origin".to_string()]);
    assert!(!o.shallow);
    assert_eq!(o.object_format, "SHA-1");
    assert_eq!(o.git_dir, o.common_dir);
    // HEAD's files: a.txt (13), .mailmap (80), b.txt (10), dir/c.txt (4); the stash put
    // a.txt back.
    assert_eq!((o.tracked.files, o.tracked.bytes), (4, 13 + 80 + 10 + 4));
    let c = o.commits.unwrap();
    // Six commits: the stash and its index commit are not counted.
    assert_eq!((c.count, c.roots, c.partial), (6, 1, false));
    assert_eq!((c.oldest, c.newest), (Some(1_700_000_060), Some(1_700_000_360)));

    let git_before = repo.command_log().snapshot().len();
    let st = repo.stats_storage().unwrap();
    let ran: Vec<String> = repo.command_log().snapshot()[git_before..]
        .iter()
        .map(|e| e.args.join(" "))
        .collect();
    assert!(ran.len() == 1 && ran[0].ends_with("count-objects -v"), "one git process: {ran:?}");
    // Few objects: fast-import and the stash leave them loose.
    let c = &st.objects;
    assert!(c.in_pack + c.loose >= 6 + 2 && c.loose_bytes > 0, "{c:?}");
    assert!(st.git_dir_bytes > st.objects.bytes() / 2 && st.git_dir_files > 5, "{st:?}");
    assert!(st.lfs.is_none());

    let no = AtomicBool::new(false);
    let all = repo_stats::authors(&history, Scope::All, &no, &mut |_, _| {}).unwrap();
    assert_eq!((all.commits, all.total), (6, 6));
    assert!(all.mailmap);
    assert_eq!(
        all.people,
        vec![("Alice".to_string(), 3), ("Bob Builder".to_string(), 2), ("T".to_string(), 1)]
    );
    // The newest three: side (T), five (Alice), four (bobby → Bob Builder).
    let newest = repo_stats::authors(&history, Scope::Newest(3), &no, &mut |_, _| {}).unwrap();
    assert_eq!((newest.commits, newest.total), (3, 6));
    assert_eq!((newest.from, newest.to), (1_700_000_240, 1_700_000_360));
    let names: Vec<&str> = newest.people.iter().map(|p| p.0.as_str()).collect();
    assert_eq!(names, ["Alice", "Bob Builder", "T"]);

    let date = |s: i64| format!("<{s}>");
    let mut stats = Stats {
        overview: Some(o),
        storage: Some(Ok(st)),
        authors: Some(newest),
        all: AllCount::Offered,
    };
    assert!(stats.offers_count_all());
    let text = repo_stats::text(&stats, &options(&date));
    for part in [
        "Commits:         6 — reachable from the branches",
        "Oldest commit:   <1700000060>, 1 year 1 month ago",
        "2 branches, 1 remote-tracking branch, 1 tag\n",
        "1 remote, 1 stash, 0 linked worktrees, 0 submodules\n",
        "Partial clone:   objects that are not here",
        "Git data:",
        "the history: all objects in",
        "Tracked files:   107 bytes in 4 files",
        "Commits per author — the newest 3 of 6 commits, <1700000240> – <1700000360>; .mailmap applied",
        "Count All Commits counts the whole history",
    ] {
        assert!(text.contains(part), "lacks `{part}`:\n{text}");
    }
    stats.all = AllCount::Done(all);
    let text = repo_stats::text(&stats, &options(&date));
    assert!(text.contains("Commits per author — all 6 commits"), "{text}");
    assert!(text.contains("  3   50.0 %  Alice\n"), "{text}");
    assert!(!text.contains("Count All Commits"), "{text}");
}

/// A linked worktree: its own git dir, the common dir named; the main one counts it.
#[gitti_test::test]
fn linked_worktree_names_the_common_dir() {
    let mut h = History::new();
    let c = h.commit("main", &[], "one", &[("a.txt", f("a\n"))]);
    h.commit("other", &[c], "two", &[("b.txt", f("b\n"))]);
    let (tmp, dir, _) = repo_with(&h);
    let wt = tmp.path().join("wt");
    sh(&dir, &["worktree", "add", "-q", wt.to_str().unwrap(), "other"]);
    let main = Repo::open(&dir).unwrap().stats_overview(None);
    assert_eq!(main.linked_worktrees, 1);
    let linked = Repo::open(&wt).unwrap().stats_overview(None);
    assert_ne!(linked.git_dir, linked.common_dir);
    assert_eq!(linked.common_dir, main.git_dir);
    assert_eq!((linked.tracked.files, linked.commits), (2, None));
    let date = |s: i64| s.to_string();
    let stats = Stats {
        overview: Some(linked),
        ..Default::default()
    };
    let text = repo_stats::text(&stats, &options(&date));
    for part in [
        "this is a linked worktree",
        "Commits:         the history is still loading",
        "Git data:        reading…",
        "Commits per author\n  counting…",
    ] {
        assert!(text.contains(part), "lacks `{part}`:\n{text}");
    }
}

/// The whole-history count reports progress and stops when cancelled.
#[gitti_test::test]
fn whole_count_progress_and_cancel() {
    let mut h = History::new();
    let mut c = h.commit("main", &[], "0", &[]);
    for i in 1..5_000 {
        c = h.commit("main", &[c], &i.to_string(), &[]);
    }
    let (_tmp, dir, _) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let history = repo.history(LogOrder::Topo).unwrap();
    let mut seen = Vec::new();
    let go = AtomicBool::new(false);
    let all = repo_stats::authors(&history, Scope::All, &go, &mut |d, of| seen.push((d, of))).unwrap();
    assert_eq!((all.commits, all.people.len()), (5_000, 1));
    assert_eq!(seen, vec![(4_096, 5_000)]);
    let stop = AtomicBool::new(true);
    assert!(repo_stats::authors(&history, Scope::All, &stop, &mut |_, _| {}).is_none());
}

/// `i18n.commitEncoding` and where it is set (in-process), and the commits per `encoding`
/// header counted in the authors' pass (no git process per commit). Three git processes.
#[gitti_test::test]
fn commit_encodings_set_and_in_the_history() {
    let mut h = History::new();
    let mut c = h.commit("main", &[], "plain", &[("a.txt", f("a\n"))]);
    c = h.commit("main", &[c], "plain too", &[]);
    h.encoding(Some("ISO-8859-1"));
    c = h.commit("main", &[c], "latin one", &[]);
    c = h.commit("main", &[c], "latin two", &[]);
    h.encoding(Some("KOI8-R"));
    c = h.commit("main", &[c], "cyrillic", &[]);
    h.encoding(None);
    h.commit("main", &[c], "plain again", &[]);
    let (_tmp, dir, _) = repo_with(&h);
    append_config(&dir, "[i18n]\n\tcommitEncoding = ISO-8859-1\n");
    let repo = Repo::open(&dir).unwrap();
    let history = repo.history(LogOrder::Topo).unwrap();

    let git_before = repo.command_log().snapshot().len();
    let o = repo.stats_overview(Some(&history));
    let no = AtomicBool::new(false);
    let all = repo_stats::authors(&history, Scope::All, &no, &mut |_, _| {}).unwrap();
    let newest = repo_stats::authors(&history, Scope::Newest(2), &no, &mut |_, _| {}).unwrap();
    assert_eq!(repo.command_log().snapshot().len(), git_before, "read in-process");

    let e = &o.commit_encoding;
    assert_eq!((e.name(), e.scope), ("ISO-8859-1", Some(SetIn::Repository)));
    assert_eq!(e.where_text(), "set in .git/config (this repository)");
    let enc = |v: &[(Option<&str>, usize)]| -> Vec<(Option<String>, usize)> {
        v.iter().map(|(h, n)| (h.map(String::from), *n)).collect()
    };
    assert_eq!(all.encodings, enc(&[(None, 3), (Some("ISO-8859-1"), 2), (Some("KOI8-R"), 1)]));
    // The newest two: "plain again" and "cyrillic".
    assert_eq!(newest.encodings, enc(&[(None, 1), (Some("KOI8-R"), 1)]));

    let date = |s: i64| s.to_string();
    let mut stats = Stats {
        overview: Some(o),
        authors: Some(newest),
        ..Default::default()
    };
    let text = repo_stats::text(&stats, &options(&date));
    for part in [
        "\nCommit messages\n  Encoding:        ISO-8859-1 — i18n.commitEncoding set in .git/config \
         (this repository): new commit messages are written in ISO-8859-1",
        "\n  In the history:  the newest 2 of 6 commits: 1 UTF-8 (no encoding header), 1 KOI8-R\n",
        "\n  Change:          Repository ▸ Commit Encoding… — for new commits only; existing ones \
         keep their encoding. Re-encoding existing commits is not offered",
    ] {
        assert!(text.contains(part), "lacks `{part}`:\n{text}");
    }
    stats.all = AllCount::Done(all);
    let text = repo_stats::text(&stats, &options(&date));
    let part = "\n  In the history:  all 6 commits: 3 UTF-8 (no encoding header), 2 ISO-8859-1, 1 KOI8-R\n";
    assert!(text.contains(part), "lacks `{part}`:\n{text}");
    stats.authors = None;
    stats.all = AllCount::Offered;
    let text = repo_stats::text(&stats, &options(&date));
    assert!(text.contains("\n  In the history:  counting…\n"), "{text}");
}
