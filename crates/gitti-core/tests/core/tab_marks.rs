//! The marks next to a repository's name: commits not pushed against an upstream, without
//! one, with a gone one; none when detached or without any remote; the snapshot carries
//! them without an extra git process; uncommitted = what `git status` lists (a stash is
//! not); one mark shown, uncommitted before unpushed; a local upstream (`remote = .`)
//! compares with that branch; the other worktrees' marks (one `git status` each).

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::tab_marks::{self, Mark, Marks, MarksStore, PushTarget, Unpushed};
use gitti_core::worktree::AddMode;

use crate::fx::{self, Import, put};

fn unpushed(repo: &Repo) -> Option<Unpushed> {
    tab_marks::unpushed(repo, &repo.tracking().unwrap())
}

fn set_head(work: &Path, to: &str) {
    fs::write(work.join(".git").join("HEAD"), format!("{to}\n")).unwrap();
}

/// Workflow: RM-18
#[gitti_test::test]
fn commits_not_pushed_with_and_without_an_upstream() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fx::init_files(&work); // `set_head` writes HEAD by hand.
    fx::remote(&work, "origin", &dir.path().join("origin.git"));
    fx::upstream(&work, "main", "origin");
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.reset("refs/remotes/origin/main", &one);
    i.commit("main", None, &[], "two", &[put("a.txt", "two\n")]);
    let three = i.commit("main", None, &[], "three", &[put("a.txt", "three\n")]);
    i.commit("feature", Some(&three), &[], "four", &[put("b.txt", "four\n")]);
    // A stash (only its ref matters here): not an uncommitted change.
    i.reset("refs/stash", &one);
    let ids = i.run(&work);
    fx::checkout(&work);
    let repo = Repo::open(&work).unwrap();

    // Ahead of the upstream: its count, from what the refresh reads anyway.
    let n_before = fx::commands(&repo).len();
    let snap = repo.snapshot(&Default::default()).unwrap();
    let want = Unpushed {
        count: 2,
        target: PushTarget::Upstream("origin/main".into()),
    };
    assert_eq!(snap.unpushed, Some(want));
    assert!(!tab_marks::uncommitted(&snap.working.status), "a clean checkout");
    let mark = tab_marks::mark(&snap.working.status, snap.unpushed.as_ref());
    assert_eq!(mark, Some(Mark::Unpushed), "clean with a stash, two commits not pushed");
    let ran = fx::commands(&repo)[n_before..].to_vec();
    assert!(
        !ran.iter().any(|c| c.starts_with("rev-list") || c.starts_with("log")),
        "the snapshot counted with git: {ran:?}"
    );

    // A changed and an untracked file: uncommitted.
    fs::write(work.join("a.txt"), "changed\n").unwrap();
    fs::write(work.join("new.txt"), "new\n").unwrap();
    let w = repo.working_state().unwrap();
    assert_eq!(
        tab_marks::describe_uncommitted(&w.status).as_deref(),
        Some("Uncommitted changes in 2 files: 1 unstaged, 1 untracked")
    );
    // Uncommitted outranks unpushed: one filled circle.
    let mark = tab_marks::mark(&w.status, snap.unpushed.as_ref());
    assert_eq!(mark, Some(Mark::Uncommitted));

    // No upstream: the commits no remote-tracking branch has (two, three, four).
    set_head(&work, "ref: refs/heads/feature");
    let got = unpushed(&repo).expect("feature has unpushed commits");
    assert_eq!(got.count, 3);
    assert_eq!(got.target, PushTarget::NoUpstream { branch: "feature".into() });

    // An upstream that is gone on the remote: counted the same way.
    fx::upstream(&work, "feature", "origin");
    let got = unpushed(&repo).expect("feature has unpushed commits");
    assert_eq!(got.count, 3);
    assert_eq!(got.target, PushTarget::Gone { upstream: "origin/feature".into() });

    // Detached: no branch to push.
    set_head(&work, &ids[2]);
    assert_eq!(unpushed(&repo), None);

    // Up to date with the upstream: nothing.
    set_head(&work, "ref: refs/heads/main");
    fx::git(&work, &["update-ref", "refs/remotes/origin/main", "main"]);
    assert_eq!(unpushed(&repo), None);

    // No remote at all: nowhere to push, no mark (the branch without upstream had one).
    let config = work.join(".git").join("config");
    let text = fs::read_to_string(&config).unwrap();
    let kept: Vec<&str> = text
        .split_inclusive('\n')
        .scan(false, |in_remote, line| {
            if line.starts_with('[') {
                *in_remote = line.starts_with("[remote ") || line.starts_with("[branch ");
            }
            Some((!*in_remote).then_some(line))
        })
        .flatten()
        .collect();
    fs::write(&config, kept.concat()).unwrap();
    set_head(&work, "ref: refs/heads/feature");
    assert_eq!(unpushed(&repo), None);
}

/// `branch.topic.remote = .`: the upstream is the local branch `main` — the commits `main`
/// doesn't have, in words that say so; none once `main` has them; also without any remote.
/// Workflow: RM-18
#[gitti_test::test]
fn a_local_upstream_compares_with_that_branch() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fx::init_files(&work); // `set_head` writes HEAD by hand.
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.commit("topic", Some(&one), &[], "two", &[put("a.txt", "two\n")]);
    i.commit("topic", None, &[], "three", &[put("a.txt", "three\n")]);
    i.run(&work);
    fx::config(&work, "[branch \"topic\"]\n\tremote = .\n\tmerge = refs/heads/main\n");
    set_head(&work, "ref: refs/heads/topic");
    fx::checkout(&work);
    let repo = Repo::open(&work).unwrap();

    // No remote at all, yet its upstream is here: two commits not in main.
    let got = unpushed(&repo).expect("topic is ahead of main");
    assert_eq!(got.count, 2);
    assert_eq!(got.target, PushTarget::LocalUpstream("main".into()));
    assert_eq!(got.describe(), "2 commits not in main (the local branch it tracks)");

    // With a remote that has none of them: still compared with main, not "not pushed".
    fx::remote(&work, "origin", &dir.path().join("origin.git"));
    let got = unpushed(&repo).expect("topic is ahead of main");
    assert_eq!(got.target, PushTarget::LocalUpstream("main".into()));

    // main has them (fast-forwarded): nothing.
    fx::git(&work, &["update-ref", "refs/heads/main", "topic"]);
    assert_eq!(unpushed(&repo), None);
}

/// The sidebar's worktree entries: the other worktrees' marks — one `git status` in each,
/// unpushed commits counted in-process from the shared refs; the current one left out
/// (its own come with the refresh). The repository's mark takes in those without a tab
/// of their own, naming each.
/// Workflow: WT-06
#[gitti_test::test]
fn other_worktrees_get_their_own_marks() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fx::init(&work);
    fx::remote(&work, "origin", &dir.path().join("origin.git"));
    fx::upstream(&work, "main", "origin");
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.reset("refs/remotes/origin/main", &one);
    i.commit("feature", Some(&one), &[], "two", &[put("b.txt", "two\n")]);
    let ids = i.run(&work);
    fx::checkout(&work);
    let repo = Repo::open(&work).unwrap();
    let topic = dir.path().join("topic");
    let loose = dir.path().join("loose");
    repo.add_worktree(&topic, &AddMode::Existing("feature".into())).unwrap();
    repo.add_worktree(&loose, &AddMode::Detached(ids[0].clone())).unwrap();
    fs::write(loose.join("new.txt"), "new\n").unwrap();
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert_eq!(snap.worktrees.len(), 3);

    let n_before = fx::commands(&repo).len();
    let marks = tab_marks::other_worktrees(&repo, &snap.worktrees, &snap.tracking);
    let ran = fx::commands(&repo)[n_before..].to_vec();
    assert_eq!(ran.iter().filter(|c| c.starts_with("status")).count(), 2, "{ran:?}");
    assert_eq!(ran.len(), 2, "only the two statuses: {ran:?}");
    let of = |p: &Path| {
        let p = gitti_core::paths::canonical(p);
        marks.iter().find(|(w, _)| *w == p).map(|(_, m)| m.clone())
    };
    assert!(of(&work).is_none(), "the current worktree is left out");
    // feature: clean, one commit no remote has.
    let m = of(&topic).expect("topic's marks");
    assert_eq!(m.mark(), Some(Mark::Unpushed));
    assert_eq!(
        m.unpushed,
        Some(Unpushed {
            count: 1,
            target: PushTarget::NoUpstream { branch: "feature".into() }
        })
    );
    // Detached with an untracked file: uncommitted, nothing to push.
    let m = of(&loose).expect("loose's marks");
    assert_eq!(
        m,
        Marks {
            uncommitted: Some("Uncommitted changes in 1 file: 1 untracked".into()),
            unpushed: None,
        }
    );
    assert_eq!(m.mark(), Some(Mark::Uncommitted));
    assert_eq!(m.describe(), "Uncommitted changes in 1 file: 1 untracked");

    // The repository's mark (its tab's, gitty's top line) takes them in: clean and pushed
    // itself, filled from `loose`, each line naming its worktree (uncommitted first).
    let mut store = MarksStore::default();
    let own = Marks::of(&snap.working.status, snap.unpushed.as_ref());
    store.publish(repo.workdir(), 1, own.clone());
    let common = repo.common_dir();
    assert!(store.begin_read(&common));
    store.finish_read(&common, marks.clone());
    let shown = store.repo_marks(own.clone(), &snap.worktrees);
    assert_eq!(shown.own.mark(), None);
    assert_eq!(shown.mark(), Some(Mark::Uncommitted));
    let sep = std::path::MAIN_SEPARATOR;
    let short = &ids[0][..7];
    assert_eq!(
        shown.legend(),
        format!(
            "● Worktree ..{sep}loose (detached {short}): uncommitted changes in 1 file: 1 untracked\n\
             ○ Worktree ..{sep}topic (branch feature): 1 commit on no remote (feature has no upstream branch)"
        )
    );
    // `loose` opens in a tab of its own: its mark moves there, the hollow one stays.
    store.publish(&gitti_core::paths::canonical(&loose), 2, of(&loose).unwrap());
    assert_eq!(store.repo_marks(own, &snap.worktrees).mark(), Some(Mark::Unpushed));

    // Seen from the linked worktree, the main one is an "other" one (clean, pushed).
    let linked = Repo::open(&topic).unwrap();
    let snap = linked.snapshot(&Default::default()).unwrap();
    let marks = tab_marks::other_worktrees(&linked, &snap.worktrees, &snap.tracking);
    assert_eq!(marks.len(), 2);
    let main = marks.iter().find(|(p, _)| *p == gitti_core::paths::canonical(&work));
    assert_eq!(main.map(|(_, m)| m.mark()), Some(None), "main: clean and pushed");
}

/// One definition: a worktree's marks read by its own tab (snapshot → `Marks::of`) equal
/// what another worktree's tab reads of it (`other_worktrees`) — untracked files in a new
/// folder, ignored files, a stash, staged changes, a branch ahead of its upstream, and with
/// `status.showUntrackedFiles=no`.
#[gitti_test::test]
fn a_worktrees_own_marks_equal_what_the_others_read() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fx::init(&work);
    fx::remote(&work, "origin", &dir.path().join("origin.git"));
    fx::upstream(&work, "feature", "origin");
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n"), put(".gitignore", "*.log\n")]);
    i.reset("refs/remotes/origin/feature", &one);
    i.commit("feature", Some(&one), &[], "two", &[put("b.txt", "two\n")]);
    i.reset("refs/stash", &one);
    i.run(&work);
    fx::checkout(&work);
    let repo = Repo::open(&work).unwrap();
    let wt = dir.path().join("wt");
    repo.add_worktree(&wt, &AddMode::Existing("feature".into())).unwrap();
    let wt = gitti_core::paths::canonical(&wt);
    let own = Repo::open(&wt).unwrap();
    let compare = |what: &str| {
        let snap = own.snapshot(&Default::default()).unwrap();
        let mine = Marks::of(&snap.working.status, snap.unpushed.as_ref());
        let main = repo.snapshot(&Default::default()).unwrap();
        let read = tab_marks::other_worktrees(&repo, &main.worktrees, &main.tracking);
        let theirs = read.iter().find(|(p, _)| *p == wt).map(|(_, m)| m.clone());
        assert_eq!(Some(mine.clone()), theirs, "{what}");
        mine
    };
    let m = compare("clean, one commit ahead of origin/feature");
    assert_eq!(m.mark(), Some(Mark::Unpushed));
    fs::create_dir_all(wt.join("new/deep")).unwrap();
    fs::write(wt.join("new/deep/x.txt"), "x\n").unwrap();
    fs::write(wt.join("new/y.txt"), "y\n").unwrap();
    fs::write(wt.join("debug.log"), "ignored\n").unwrap();
    let m = compare("untracked folder + ignored file");
    assert_eq!(m.uncommitted.as_deref(), Some("Uncommitted changes in 2 files: 2 untracked"));
    fs::write(wt.join("b.txt"), "changed\n").unwrap();
    fx::git(&wt, &["add", "b.txt"]);
    compare("staged");
    fx::config(&work, "[status]\n\tshowUntrackedFiles = no\n");
    fx::git(&wt, &["reset", "-q", "--hard"]);
    let m = compare("untracked hidden by the config");
    assert_eq!(m.mark(), Some(Mark::Unpushed), "only untracked files left, hidden: {m:?}");
}

/// The file watcher names another worktree by its git dir when its index changes (`git
/// add` there): the main worktree's is the common dir, a linked one's
/// `<common>/worktrees/<name>`. Reading those worktrees' marks never writes their index
/// (`GIT_OPTIONAL_LOCKS=0`, also when its stat data is stale): no change of ours that
/// the watchers would report again — no loop.
/// Workflow: EX-25
#[gitti_test::test]
fn index_changes_name_their_worktree_and_our_reads_write_no_index() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fx::init(&work);
    let mut i = Import::new();
    i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.run(&work);
    fx::checkout(&work);
    let repo = Repo::open(&work).unwrap();
    let topic = dir.path().join("topic");
    repo.add_worktree(&topic, &AddMode::Detached("main".into())).unwrap();
    let topic = gitti_core::paths::canonical(&topic);
    let common = repo.common_dir();
    let topic_git = common.join("worktrees").join("topic");
    let names = |v: Vec<gitti_core::worktree::Worktree>| v.into_iter().map(|w| w.path).collect::<Vec<_>>();

    let from_main = repo.worktrees().unwrap();
    let got = names(tab_marks::with_git_dirs(from_main.clone(), &common, std::slice::from_ref(&topic_git)));
    assert_eq!(got, vec![topic.clone()]);
    assert!(tab_marks::with_git_dirs(from_main.clone(), &common, &[]).is_empty());
    let linked = Repo::open(&topic).unwrap();
    let from_linked = linked.worktrees().unwrap();
    let main = gitti_core::paths::canonical(&work);
    assert_eq!(names(tab_marks::with_git_dirs(from_linked, &common, std::slice::from_ref(&common))), vec![main]);

    // Read again only when its index was written after the last read started (by the
    // file's stamp: a late event of an earlier write doesn't count).
    let mut store = MarksStore::default();
    let changed = |store: &MarksStore| names(store.index_changed(from_main.clone(), &common, std::slice::from_ref(&topic_git)));
    assert_eq!(changed(&store), vec![topic.clone()], "never read");
    let topic_wt: Vec<_> = from_main.iter().filter(|w| w.path == topic).cloned().collect();
    store.read_starts(&topic_wt, gitti_core::watch::read_start().1);
    assert!(changed(&store).is_empty(), "written before the read started");
    fs::write(topic.join("b.txt"), "b\n").unwrap();
    fx::git(&topic, &["add", "b.txt"]);
    assert_eq!(changed(&store), vec![topic.clone()], "git add after it");
    fx::git(&topic, &["reset", "-q"]);
    fs::remove_file(topic.join("b.txt")).unwrap();

    // Stale stat data in topic's index (same content, new mtime): git status would refresh
    // and write it — ours doesn't.
    let file = fs::File::options().write(true).open(topic.join("a.txt")).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
    drop(file);
    let index = topic_git.join("index");
    let stamp = || fs::metadata(&index).map(|m| (m.len(), m.modified().unwrap())).unwrap();
    let before = stamp();
    let marks = tab_marks::other_worktrees(&repo, &from_main, &[]);
    assert_eq!(marks, vec![(topic.clone(), Marks::default())], "clean");
    assert_eq!(stamp(), before, "our read wrote topic's index");
}
