//! Pull strategy against a local bare remote: a diverged pull without any
//! `pull.rebase` / `pull.ff` configuration is a `DivergentPull`, each choice of its
//! dialog (merge, rebase, fast-forward only) does what it says, a remembered strategy is
//! passed without asking — unless the user's git configuration decides — and a
//! remembered "Fast-forward only" that can't asks again. Also for "Pull from…". The
//! choice after a refusal joins what the refused pull fetched — no second fetch — and
//! ends where `git pull` would, also after a force push upstream. "Pull with…"
//! passes the chosen strategy whatever the configuration says.

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::pull::{DivergentPull, PullStrategy};

use crate::fx::{self, Import, put};

struct Diverged {
    _dir: tempfile::TempDir,
    work: PathBuf,
    repo: Repo,
    /// The local commit on top of the common one.
    mine: String,
    /// The remote's commit on top of the common one.
    theirs: String,
}

/// `work` on `main` with a commit of its own; `origin.git` (its upstream) has another
/// one on the same base. git sees no global or system configuration.
fn diverged() -> Diverged {
    diverged_from(false)
}

/// `forced`: `origin` had a commit "x" that `work` built on, then rewrote it to "x2"
/// (a force push); `refs/remotes/origin/main` and its reflog still say "x" — `git pull
/// --rebase` leaves "x" out (fork point), a plain `git rebase @{u}` would replay it.
fn diverged_from(forced: bool) -> Diverged {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin) = (dir.path().join("work"), dir.path().join("origin.git"));
    let mut base = Import::new();
    let mut known = base.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    if forced {
        known = base.commit("main", None, &[], "x", &[put("x.txt", "x\n")]);
    }
    base.reset("refs/remotes/origin/main", &known);
    fx::bare(&origin);
    let mut theirs = Import::new();
    theirs.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    if forced {
        theirs.commit("main", None, &[], "x2", &[put("x.txt", "x2\n")]);
    } else {
        theirs.commit("main", None, &[], "theirs", &[put("t.txt", "theirs\n")]);
    }
    let theirs = theirs.run(&origin).remove(1);
    fx::init(&work);
    fx::remote(&work, "origin", &origin);
    fx::upstream(&work, "main", "origin");
    base.commit("main", None, &[], "mine", &[put("m.txt", "mine\n")]);
    let ids = base.run(&work);
    let mine = ids.last().unwrap().clone();
    if forced {
        // The fetch that brought "x" (fast-import writes no reflog).
        let logs = fx::git_dir(&work).join("logs/refs/remotes/origin");
        fs::create_dir_all(&logs).unwrap();
        let line = format!("{} {} T <t@example.com> 1 +0000\tfetch: x\n", "0".repeat(40), ids[1]);
        fs::write(logs.join("main"), line).unwrap();
    }
    fx::checkout(&work);
    let empty = dir.path().join("empty-global-config");
    fs::write(&empty, "").unwrap();
    let mut repo = Repo::open(&work).unwrap();
    repo.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), empty.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    Diverged { _dir: dir, work, repo, mine, theirs }
}

impl Diverged {
    /// HEAD's parents.
    fn parents(&self) -> Vec<String> {
        let line = fx::git(&self.work, &["rev-list", "--parents", "-n1", "HEAD"]);
        line.split_whitespace().skip(1).map(str::to_string).collect()
    }

    /// Back to the local commit (one `git reset --hard`).
    fn reset(&self) {
        fx::git(&self.work, &["reset", "-q", "--hard", &self.mine]);
    }

    fn has(&self, file: &str) -> bool {
        self.work.join(file).exists()
    }

    fn last_pull(&self) -> String {
        let cmds = fx::commands(&self.repo);
        cmds.into_iter().rfind(|c| c.starts_with("pull")).unwrap_or_default()
    }

    fn pulls(&self) -> usize {
        fx::commands(&self.repo).iter().filter(|c| c.starts_with("pull")).count()
    }

    fn last_command(&self) -> String {
        fx::commands(&self.repo).pop().unwrap_or_default()
    }

    /// Subject and tree of each commit from HEAD back (ids differ by commit time).
    fn chain(&self) -> String {
        fx::git(&self.work, &["log", "--format=%s %T"])
    }

    /// HEAD's parents, tree and message.
    fn head_commit(&self) -> String {
        fx::git(&self.work, &["log", "-1", "--format=%P%n%T%n%B"])
    }
}

fn divergent(e: &anyhow::Error) -> &DivergentPull {
    e.downcast_ref::<DivergentPull>()
        .unwrap_or_else(|| panic!("not DivergentPull: {e:#}"))
}

/// No configuration: git refuses, the error asks; each choice of the dialog joins what
/// the refused pull fetched (no second fetch); a `FETCH_HEAD` changed since runs
/// the whole pull again.
/// Workflow: RB-09, RM-04
#[gitti_test::test]
fn a_diverged_pull_asks_and_each_choice_does_what_it_says() {
    let d = diverged();
    let e = d.repo.pull().unwrap_err();
    let ask = divergent(&e).clone();
    assert_eq!((ask.branch.as_str(), ask.upstream.as_str()), ("main", "origin/main"));
    assert_eq!((ask.from.clone(), ask.failed), (None, None));
    assert!(ask.message.contains("divergent branches"), "{}", ask.message);
    assert!(ask.fetched.as_deref().is_some_and(|f| f.starts_with(&d.theirs)), "{:?}", ask.fetched);
    assert_eq!(fx::head(&d.work).as_deref(), Some(d.mine.as_str()), "nothing changed");
    let pulls = d.pulls();

    // Fast-forward only: refuses (as the user chose; not asked again).
    let e = ask.retry(&d.repo, PullStrategy::FastForwardOnly).unwrap_err();
    assert!(e.downcast_ref::<DivergentPull>().is_none(), "{e:#}");
    assert!(format!("{e:#}").contains("Not possible to fast-forward"), "{e:#}");
    assert_eq!(d.last_command(), "merge --ff-only FETCH_HEAD");

    // Rebase: mine on top of theirs.
    ask.retry(&d.repo, PullStrategy::Rebase).unwrap();
    assert_eq!(d.parents(), vec![d.theirs.clone()]);
    assert!(d.has("m.txt") && d.has("t.txt"));
    assert!(d.last_command().starts_with(&format!("rebase --onto {}", d.theirs)));

    // Merge: a merge commit of both.
    d.reset();
    ask.retry(&d.repo, PullStrategy::Merge).unwrap();
    assert_eq!(d.parents(), vec![d.mine.clone(), d.theirs.clone()]);
    assert_eq!(d.last_command(), "merge --no-edit FETCH_HEAD");
    assert_eq!(d.pulls(), pulls, "no second fetch");

    // Another fetch since the refusal: the whole pull again.
    d.reset();
    let fetch_head = fx::git_dir(&d.work).join("FETCH_HEAD");
    fs::write(&fetch_head, format!("{}\n", fs::read_to_string(&fetch_head).unwrap())).unwrap();
    ask.retry(&d.repo, PullStrategy::Merge).unwrap();
    assert_eq!(d.last_pull(), "pull --no-rebase");
    assert_eq!(d.parents(), vec![d.mine.clone(), d.theirs.clone()]);
}

/// The retry ends where `git pull` ends: the same merge commit (parents, tree, message
/// "Merge branch 'main' of …"), the same rebase — after a force push upstream too, where
/// the fork point leaves the rewritten commit out (`git rebase @{u}` would replay it).
/// Workflow: RB-09
#[gitti_test::test]
fn the_retry_ends_where_git_pull_would() {
    let d = diverged_from(true);
    let ask = divergent(&d.repo.pull().unwrap_err()).clone();
    ask.retry(&d.repo, PullStrategy::Rebase).unwrap();
    let ours = d.chain();
    let subjects: Vec<&str> = ours.lines().map(|l| l.split(' ').next().unwrap()).collect();
    assert_eq!(subjects, ["mine", "x2", "one"], "x left out:\n{ours}");
    d.reset();
    d.repo.pull_with(Some(PullStrategy::Rebase)).unwrap();
    assert_eq!(d.last_pull(), "pull --rebase");
    assert_eq!(d.chain(), ours, "git pull --rebase");


    // (Merging "x" and "x2" conflicts — for git pull as well.)
    let d = diverged();
    let ask = divergent(&d.repo.pull().unwrap_err()).clone();
    ask.retry(&d.repo, PullStrategy::Merge).unwrap();
    assert_eq!(d.last_command(), "merge --no-edit FETCH_HEAD");
    let ours = d.head_commit();
    assert!(ours.contains("Merge branch 'main' of "), "{ours}");
    d.reset();
    d.repo.pull_with(Some(PullStrategy::Merge)).unwrap();
    assert_eq!(d.last_pull(), "pull --no-rebase");
    assert_eq!(d.head_commit(), ours, "git pull --no-rebase");
}

/// "Pull with…": the chosen strategy, whatever `pull.rebase` says.
/// Workflow: RB-09
#[gitti_test::test]
fn pull_with_passes_the_choice_over_the_configuration() {
    let d = diverged();
    fx::config(&d.work, "[pull]\n\trebase = false\n");
    d.repo.pull_chosen(PullStrategy::Rebase).unwrap();
    assert_eq!(d.last_pull(), "pull --rebase");
    assert_eq!(d.parents(), vec![d.theirs.clone()]);
}

/// A remembered strategy is passed without asking; a remembered "Fast-forward only" that
/// can't asks again.
/// Workflow: RM-04
#[gitti_test::test]
fn a_remembered_strategy_is_used_without_asking() {
    let d = diverged();
    d.repo.pull_with(Some(PullStrategy::Rebase)).unwrap();
    assert_eq!(d.parents(), vec![d.theirs.clone()], "rebased without asking");
    assert_eq!(d.last_pull(), "pull --rebase");

    d.reset();
    let e = d.repo.pull_with(Some(PullStrategy::FastForwardOnly)).unwrap_err();
    assert_eq!(divergent(&e).failed, Some(PullStrategy::FastForwardOnly));
    assert!(divergent(&e).describe().contains("can't join them"));
    assert_eq!(fx::head(&d.work).as_deref(), Some(d.mine.as_str()));
}

/// "Pull from…" asks the same way and takes a remembered strategy.
/// Workflow: RM-04
#[gitti_test::test]
fn pull_from_asks_and_takes_the_remembered_strategy() {
    let d = diverged();
    let e = d.repo.pull_from("origin").unwrap_err();
    let ask = divergent(&e).clone();
    assert_eq!(ask.from, Some(("origin".into(), "main".into())));
    assert_eq!(ask.upstream, "origin/main");
    d.repo.pull_from_with("origin", Some(PullStrategy::Merge)).unwrap();
    assert_eq!(d.parents(), vec![d.mine.clone(), d.theirs.clone()]);
    assert_eq!(d.last_pull(), "pull --no-rebase origin main");
}

/// The user's `pull.rebase` decides over a remembered strategy: plain `git pull`.
/// Workflow: RM-04
#[gitti_test::test]
fn git_config_decides_over_the_remembered_strategy() {
    let d = diverged();
    fx::config(&d.work, "[pull]\n\trebase = false\n");
    assert!(d.repo.pull_configured("main"));
    d.repo.pull_with(Some(PullStrategy::Rebase)).unwrap();
    assert_eq!(d.last_pull(), "pull");
    assert_eq!(d.parents().len(), 2, "merged as pull.rebase=false says");
}

/// `branch.<name>.rebase` decides too; still: no upstream, no git pull.
/// Workflow: RM-04
#[gitti_test::test]
fn branch_rebase_config_decides_and_no_upstream_runs_no_pull() {
    let d = diverged();
    fx::config(&d.work, "[branch \"main\"]\n\trebase = true\n");
    assert!(d.repo.pull_configured("main"));
    d.repo.pull_with(Some(PullStrategy::Merge)).unwrap();
    assert_eq!(d.last_pull(), "pull");
    assert_eq!(d.parents(), vec![d.theirs.clone()], "rebased as configured");

    let other = Path::new("refs/heads/other");
    fs::write(d.work.join(".git").join(other), format!("{}\n", d.mine)).unwrap();
    fs::write(d.work.join(".git/HEAD"), "ref: refs/heads/other\n").unwrap();
    let before = fx::commands(&d.repo).len();
    let e = d.repo.pull_with(Some(PullStrategy::Merge)).unwrap_err();
    assert!(e.downcast_ref::<gitti_core::push::PullBlock>().is_some(), "{e:#}");
    let ran = fx::commands(&d.repo)[before..].to_vec();
    assert!(!ran.iter().any(|c| c.starts_with("pull")), "{ran:?}");
}
