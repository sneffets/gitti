//! Submodules against real temporary repositories: the list and its states
//! (up to date, new commits, out of date, dirty, uninitialised, nested, conflicted) against
//! `git submodule status --recursive`, pointer changes in working-state / commit diffs as
//! commit summaries, staging them as one unit, update / sync / discard, clone with
//! `--recurse-submodules`, and no cost for repositories without submodules.
//!
//! The fixture is laid out by hand the way `git submodule add` / `update --init
//! --recursive` leave it — `.git` files, `.git/modules/…` with `core.worktree`, remotes,
//! `submodule.*` config — with every repository's objects in one fast-import store they
//! share as alternates: five git processes instead of ~15 with three clones (~1.2 s).
//! The first test checks the layout against `git submodule status --recursive`.

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::submodule::{SubmoduleState, out_of_date};

use crate::fixture_kit::{C, E, History, Ids, f, oid, slash};

/// Local clones as submodules need `protocol.file.allow=always` (git ≥ 2.38.1); commits
/// inside a cloned submodule need an identity even without a global git config (Linux CI).
const FILE_PROTOCOL: [(&str, &str); 7] = [
    ("GIT_CONFIG_COUNT", "1"),
    ("GIT_CONFIG_KEY_0", "protocol.file.allow"),
    ("GIT_CONFIG_VALUE_0", "always"),
    ("GIT_AUTHOR_NAME", "T"),
    ("GIT_AUTHOR_EMAIL", "t@example.com"),
    ("GIT_COMMITTER_NAME", "T"),
    ("GIT_COMMITTER_EMAIL", "t@example.com"),
];

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = crate::fixture_kit::git(dir, args).envs(FILE_PROTOCOL).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Commits a change of a tracked file (one git process).
fn commit_tracked(p: &Path, name: &str, text: &str, msg: &str) {
    fs::write(p.join(name), text).unwrap();
    sh(p, &["commit", "-qam", msg]);
}

fn head(p: &Path) -> String {
    sh(p, &["rev-parse", "HEAD"]).trim().to_string()
}

fn open(p: &Path) -> Repo {
    let mut r = Repo::open(p).unwrap();
    r.set_env(FILE_PROTOCOL.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
    r
}

struct Commits {
    inner_one: C,
    lib_two: C,
    lib_four: C,
    lib_a: C,
    lib_b: C,
}

/// `<tmp>/inner` (1 commit), `<tmp>/lib` (2 commits, `inner` as submodule `vendor/inner`),
/// `<tmp>/super` with `lib` as submodule `libs/lib`, checked out recursively. The lib
/// submodule is on its branch `main` (as `submodule add` leaves it), the nested one
/// detached (as `update` leaves it).
struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    lib: PathBuf,
    sup: PathBuf,
    ids: Ids,
    c: Commits,
    /// The superproject's `.git/config` as `git init` wrote it (probed `core.*` values).
    config: String,
    store: PathBuf,
}

/// The standard fixture.
fn setup() -> Fx {
    setup_with(false)
}

/// `lib_ahead`: the lib submodule has two more commits on its `main` ("lib three",
/// "lib four", on no remote) and is checked out there.
fn setup_with(lib_ahead: bool) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(tmp.path());
    let (inner, lib, sup) = (base.join("inner"), base.join("lib"), base.join("super"));
    crate::fixture_kit::init(&sup);
    let config = fs::read_to_string(sup.join(".git/config")).unwrap();

    let mut h = History::new();
    let inner_one = h.commit("inner", &[], "inner one", &[("i.txt", f("i\n"))]);
    let lib_one = h.commit("lib", &[], "lib one", &[("l.txt", f("1\n"))]);
    let modules = format!("[submodule \"vendor/inner\"]\n\tpath = vendor/inner\n\turl = {}\n", slash(&inner));
    let lib_two = h.commit("lib", &[lib_one], "lib two: inner", &[
        (".gitmodules", f(&modules)),
        ("vendor/inner", E::Gitlink(inner_one)),
    ]);
    let lib_three = h.commit("lib", &[lib_two], "lib three", &[("l.txt", f("1\n2\n"))]);
    let lib_four = h.commit("lib", &[lib_three], "lib four", &[("l.txt", f("1\n2\n3\n"))]);
    // Two diverging lib commits (for conflicting pointers).
    let lib_a = h.commit("lib-a", &[lib_two], "lib a", &[("l.txt", f("a\n"))]);
    let lib_b = h.commit("lib-b", &[lib_two], "lib b", &[("l.txt", f("b\n"))]);
    let super_one = h.commit("main", &[], "super one", &[("s.txt", f("s\n"))]);
    let modules = format!("[submodule \"libs/lib\"]\n\tpath = libs/lib\n\turl = {}\n", slash(&lib));
    let super_two = h.commit("main", &[super_one], "add lib", &[
        (".gitmodules", f(&modules)),
        ("libs/lib", E::Gitlink(lib_two)),
    ]);
    let store = base.join("store.git");
    for d in ["objects", "refs/heads"] {
        fs::create_dir_all(store.join(d)).unwrap();
    }
    fs::write(store.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(store.join("config"), "[core]\n\trepositoryformatversion = 0\n\tbare = true\n").unwrap();
    let ids = h.import_to(&store);
    let fx = Fx {
        _tmp: tmp,
        base: base.clone(),
        lib: lib.clone(),
        sup: sup.clone(),
        ids,
        c: Commits { inner_one, lib_two, lib_four, lib_a, lib_b },
        config,
        store,
    };
    let id = |c: C| fx.ids[c].to_string();
    let origin = |url: &Path| {
        format!(
            "[remote \"origin\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
             [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n",
            slash(url)
        )
    };
    let active = |name: &str, url: &Path| format!("[submodule \"{name}\"]\n\tactive = true\n\turl = {}\n", slash(url));
    let tracking = |c: C| {
        [
            ("refs/remotes/origin/main", id(c)),
            ("refs/remotes/origin/HEAD", "ref: refs/remotes/origin/main".to_string()),
        ]
    };

    // The sources.
    fx.git_dir(&inner.join(".git"), "ref: refs/heads/main", "", &[("refs/heads/main", id(inner_one))]);
    fx.git_dir(&lib.join(".git"), "ref: refs/heads/main", "", &[("refs/heads/main", id(lib_two))]);
    // The superproject and its submodules.
    fx.git_dir(&sup.join(".git"), "ref: refs/heads/main", &active("libs/lib", &lib), &[("refs/heads/main", id(super_two))]);
    let lib_git = sup.join(".git/modules/libs/lib");
    let lib_head = if lib_ahead { lib_four } else { lib_two };
    let [t0, t1] = tracking(lib_two);
    fx.git_dir(
        &lib_git,
        "ref: refs/heads/main",
        &format!("[core]\n\tworktree = ../../../../libs/lib\n{}{}", origin(&lib), active("vendor/inner", &inner)),
        &[("refs/heads/main", id(lib_head)), t0, t1],
    );
    let [t0, t1] = tracking(inner_one);
    fx.git_dir(
        &lib_git.join("modules/vendor/inner"),
        &id(inner_one),
        &format!("[core]\n\tworktree = ../../../../../../../libs/lib/vendor/inner\n{}", origin(&inner)),
        &[("refs/heads/main", id(inner_one)), t0, t1],
    );
    let gitfile = |dir: PathBuf, target: &str| {
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".git"), format!("gitdir: {target}\n")).unwrap();
    };
    gitfile(sup.join("libs/lib"), "../../.git/modules/libs/lib");
    gitfile(sup.join("libs/lib/vendor/inner"), "../../../../.git/modules/libs/lib/modules/vendor/inner");
    for dir in ["", "libs/lib", "libs/lib/vendor/inner"] {
        sh(&sup.join(dir), &["reset", "-q", "--hard"]);
    }
    fx
}

impl Fx {
    /// A git dir written by hand (the probed config + `extra`) that borrows the store's
    /// objects.
    fn git_dir(&self, dir: &Path, head: &str, extra: &str, refs: &[(&str, String)]) {
        let objects = self.store.join("objects");
        crate::fixture_kit::write_git_dir(dir, &self.config, head, extra, refs, Some(&objects));
    }
    fn sub(&self) -> PathBuf {
        self.sup.join("libs/lib")
    }
    fn inner(&self) -> PathBuf {
        self.sup.join("libs/lib/vendor/inner")
    }
    fn id(&self, c: C) -> String {
        self.ids[c].to_string()
    }
}

/// `(prefix, sha, path)` of `git submodule status --recursive`.
fn git_status(p: &Path) -> Vec<(char, String, String)> {
    sh(p, &["submodule", "status", "--recursive"])
        .lines()
        .map(|l| {
            let prefix = l.chars().next().unwrap();
            let mut parts = l[1..].split_whitespace();
            let sha = parts.next().unwrap().to_string();
            let path = parts.next().unwrap().to_string();
            (prefix, sha, path)
        })
        .collect()
}

fn list(repo: &Repo) -> Vec<gitti_core::submodule::Submodule> {
    let ws = repo.working_state().unwrap();
    repo.submodules(&ws.status)
}

/// Workflow: SM-01
#[gitti_test::test]
fn list_matches_git_and_follows_the_states() {
    let fx = setup();
    let repo = open(&fx.sup);
    let subs = list(&repo);
    let paths: Vec<&str> = subs.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["libs/lib", "libs/lib/vendor/inner"]);
    assert_eq!((subs[0].depth, subs[1].depth), (0, 1));
    assert!(subs[0].url.as_deref().unwrap().ends_with("lib"));
    assert!(subs.iter().all(|s| s.state == SubmoduleState::UpToDate), "{subs:#?}");
    // Same paths and commits as git's own listing (also checks the hand-made layout).
    let git = git_status(&fx.sup);
    assert_eq!(git.len(), 2);
    for ((prefix, sha, path), s) in git.iter().zip(&subs) {
        assert_eq!(*prefix, ' ');
        assert_eq!(path, &s.path);
        assert_eq!(sha, &s.checked_out.unwrap().to_string());
        assert_eq!(s.recorded, s.checked_out);
    }

    // New commit inside the submodule: "modified" (1 new commit), status says `C`.
    let sub = fx.sub();
    commit_tracked(&sub, "l.txt", "1\n2\n", "lib three");
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::Modified);
    assert_eq!((subs[0].ahead, subs[0].behind), (1, 0));
    assert!(subs[0].flags.commit_changed);
    assert_eq!(subs[0].detail(), "1 new commit");

    // Checked out behind the recorded commit: "out of date"; update fixes it.
    sh(&sub, &["checkout", "-q", "HEAD~2"]);
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::OutOfDate, "{:#?}", subs[0]);
    assert_eq!((subs[0].ahead, subs[0].behind), (0, 1));
    assert_eq!(out_of_date(&subs).len(), 1);
    repo.update_submodules(&[]).unwrap();
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::UpToDate);
    assert!(out_of_date(&subs).is_empty());

    // Uncommitted changes inside: modified, and the nested one stays up to date.
    fs::write(sub.join("l.txt"), "dirty\n").unwrap();
    fs::write(sub.join("new.txt"), "n\n").unwrap();
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::Modified);
    assert!(subs[0].flags.modified && subs[0].flags.untracked);
    assert_eq!(subs[0].detail(), "modified");
    assert_eq!(subs[1].state, SubmoduleState::UpToDate);
}

/// Workflow: SM-02
#[gitti_test::test]
fn pointer_change_is_a_commit_summary_staged_as_one_unit() {
    let fx = setup_with(true);
    let repo = open(&fx.sup);
    let sub = fx.sub();
    let (old, new) = (fx.id(fx.c.lib_two), fx.id(fx.c.lib_four));

    let ws = repo.working_state().unwrap();
    let f = ws.unstaged.iter().find(|f| f.path() == "libs/lib").unwrap();
    assert!(f.entry.submodule.is_some());
    let d = f.diff.as_ref().unwrap().submodule.as_ref().expect("gitlink diff");
    assert_eq!(d.old.unwrap().to_string(), old);
    assert_eq!(d.new.unwrap().to_string(), new);
    assert!(!d.dirty);
    let log = d.log.as_ref().unwrap();
    assert_eq!((log.added_total, log.removed_total), (2, 0));
    let subjects: Vec<&str> = log.added.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(subjects, ["lib four", "lib three"]);
    assert_eq!(
        d.summary("libs/lib"),
        format!("Submodule libs/lib: {} → {} (+2 commits)", &old[..7], &new[..7])
    );
    // The rows: summary + two commits, no hunk / line rows.
    let rows = gitti_core::rows::build_worktree_rows(&ws, &Default::default(), None);
    let kinds: Vec<&str> = rows.iter().map(|r| r.kind_name()).collect();
    assert_eq!(kinds, ["section", "spacer", "file", "submodule", "submodule", "submodule"]);

    // Staged by path: the staged diff carries the same summary, the list says up to date.
    repo.stage_paths(&["libs/lib"]).unwrap();
    let ws = repo.working_state().unwrap();
    assert!(ws.unstaged.is_empty(), "{:?}", ws.unstaged);
    let staged = ws.staged.iter().find(|f| f.path() == "libs/lib").unwrap();
    let d = staged.diff.as_ref().unwrap().submodule.as_ref().unwrap();
    assert_eq!(d.log.as_ref().unwrap().added_total, 2);
    assert_eq!(repo.submodules(&ws.status)[0].state, SubmoduleState::UpToDate);

    // Committed: the commit diff shows it too.
    repo.commit("bump lib", false).unwrap();
    let files = repo.commit_diff(oid(&head(&fx.sup))).unwrap();
    let d = files[0].submodule.as_ref().unwrap();
    assert_eq!(d.log.as_ref().unwrap().added.len(), 2);

    // Unstaging works the same way (after another move).
    commit_tracked(&sub, "l.txt", "x\n", "lib five");
    repo.stage_paths(&["libs/lib"]).unwrap();
    repo.unstage_paths(&["libs/lib"]).unwrap();
    let ws = repo.working_state().unwrap();
    assert!(ws.staged.is_empty());
    assert_eq!(ws.unstaged.len(), 1);

    // Discard = back to the recorded commit.
    repo.discard_paths(&[&ws.unstaged[0].entry]).unwrap();
    let ws = repo.working_state().unwrap();
    assert!(ws.unstaged.is_empty(), "{:?}", ws.unstaged);
    assert_eq!(head(&sub), new);
}

#[gitti_test::test]
fn dirty_submodule_diff_has_a_note() {
    let fx = setup();
    let repo = open(&fx.sup);
    fs::write(fx.sub().join("l.txt"), "changed\n").unwrap();
    let ws = repo.working_state().unwrap();
    let f = &ws.unstaged[0];
    let d = f.diff.as_ref().unwrap().submodule.as_ref().unwrap();
    assert!(d.dirty);
    assert_eq!(d.old, d.new);
    assert_eq!(d.row_count(), 2);
    assert!(d.row("libs/lib", 1).unwrap().text().starts_with("Uncommitted changes"));
    // Discarding leaves the content alone (git does too) and doesn't fail.
    repo.discard_paths(&[&f.entry]).unwrap();
    assert_eq!(fs::read_to_string(fx.sub().join("l.txt")).unwrap(), "changed\n");
}

/// Workflow: SM-08
#[gitti_test::test]
fn uninitialised_after_plain_clone_and_recursive_clone() {
    let fx = setup();
    let git = open(&fx.sup).git().clone();
    let url = slash(&fx.sup);
    let plain = gitti_core::create::clone_repo(&git, &url, &fx.base.join("plain"), &mut |_| {}).unwrap();
    let repo = open(&plain);
    let subs = list(&repo);
    assert_eq!(subs.len(), 1, "nested ones only appear once checked out");
    assert_eq!(subs[0].state, SubmoduleState::Uninitialized);
    assert_eq!(subs[0].detail(), "not initialised");
    repo.update_submodules(&["libs/lib"]).unwrap();
    let subs = list(&repo);
    assert_eq!(subs.len(), 2);
    assert!(subs.iter().all(|s| s.state == SubmoduleState::UpToDate), "{subs:#?}");
    repo.sync_submodules(&[]).unwrap();

    let deep = gitti_core::create::clone_repo_with(&git, &url, &fx.base.join("deep"), true, &mut |_| {}).unwrap();
    let subs = list(&open(&deep));
    assert_eq!(subs.len(), 2);
    assert!(subs.iter().all(|s| s.state == SubmoduleState::UpToDate), "{subs:#?}");
}

/// Workflow: SM-03
#[gitti_test::test]
fn pull_that_moves_the_pointer_makes_it_out_of_date() {
    let fx = setup();
    let git = open(&fx.sup).git().clone();
    let clone = fx.base.join("clone");
    gitti_core::create::clone_repo_with(&git, &slash(&fx.sup), &clone, true, &mut |_| {}).unwrap();
    // Upstream: lib gets a commit (only in lib's own objects), super records it.
    let two = fx.id(fx.c.lib_two);
    let tree = format!("{two}^{{tree}}");
    let up = sh(&fx.lib, &["commit-tree", &tree, "-p", &two, "-m", "lib upstream"]);
    fs::write(fx.lib.join(".git/refs/heads/main"), &up).unwrap();
    let info = format!("160000,{},libs/lib", up.trim());
    sh(&fx.sup, &["update-index", "--cacheinfo", &info]);
    sh(&fx.sup, &["commit", "-qm", "bump lib"]);
    // The clone pulls: its submodule is behind (the commit isn't even fetched there).
    sh(&clone, &["pull", "-q", "--no-recurse-submodules"]);
    let repo = open(&clone);
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::OutOfDate);
    assert!(subs[0].missing);
    assert_eq!(subs[0].detail(), "not fetched");
    // The pull's commit diff names the commits as not available yet.
    let id = oid(&head(&clone));
    let d = repo.commit_diff(id).unwrap()[0].submodule.clone().unwrap();
    assert!(d.log.unwrap().missing);
    repo.update_submodules(&[]).unwrap();
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::UpToDate, "{:#?}", subs[0]);
    let d = repo.commit_diff(id).unwrap()[0].submodule.clone().unwrap();
    assert_eq!(d.log.unwrap().added[0].1, "lib upstream");
}

/// Workflow: MG-21
#[gitti_test::test]
fn conflicting_pointers() {
    let fx = setup();
    let sup = &fx.sup;
    // Two diverging lib commits, one per super branch.
    let pin = |c: C, msg: &str| {
        let info = format!("160000,{},libs/lib", fx.id(c));
        sh(sup, &["update-index", "--cacheinfo", &info]);
        sh(sup, &["commit", "-qm", msg]);
    };
    sh(sup, &["checkout", "-q", "-b", "x"]);
    pin(fx.c.lib_a, "lib → a");
    sh(sup, &["checkout", "-q", "main"]);
    pin(fx.c.lib_b, "lib → b");
    assert!(!crate::fixture_kit::sh_ok(sup, &["merge", "-q", "x"]), "the merge conflicts");
    let repo = open(sup);
    let ws = repo.working_state().unwrap();
    assert_eq!(ws.conflicted.len(), 1);
    let subs = repo.submodules(&ws.status);
    assert_eq!(subs[0].state, SubmoduleState::Conflicted);
    assert_eq!(git_status(sup)[0].0, 'U');
}

fn submodule_calls(repo: &Repo) -> usize {
    let log = repo.command_log().snapshot();
    log.iter().filter(|e| e.args.iter().any(|a| a == "submodule")).count()
}

#[gitti_test::test]
fn repos_without_submodules_pay_nothing() {
    let mut h = History::new();
    h.commit("main", &[], "one", &[("a.txt", f("a\n"))]);
    let (_tmp, p, _) = crate::fixture_kit::repo_with(&h);
    let repo = open(&p);
    assert!(!repo.has_gitmodules());
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert!(snap.submodules.is_empty());
    assert_eq!(submodule_calls(&repo), 0);

    // With submodules, the snapshot lists them without any git call of its own.
    let fx = setup();
    let repo = open(&fx.sup);
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert_eq!(snap.submodules.len(), 2);
    assert_eq!(submodule_calls(&repo), 0);
}

/// Workflow: SM-04
#[gitti_test::test]
fn add_submodule_stages_it_with_its_default_path_and_branch() {
    let fx = setup();
    let sup = &fx.sup;
    // `other`: main = "lib two", dev = "lib four" (objects from the store).
    let other = fx.base.join("other");
    fx.git_dir(&other.join(".git"), "ref: refs/heads/main", "", &[
        ("refs/heads/main", fx.id(fx.c.lib_two)),
        ("refs/heads/dev", fx.id(fx.c.lib_four)),
    ]);
    let repo = open(sup);
    let url = slash(&other);
    assert_eq!(gitti_core::submodule::default_path(&url), "other");
    // Empty path = git's default (the URL's last component), a tracked branch.
    repo.add_submodule(&url, "", "dev", false).unwrap();
    let ws = repo.working_state().unwrap();
    let staged: Vec<_> = ws.staged.iter().map(|f| f.path()).collect();
    assert!(staged.contains(&".gitmodules") && staged.contains(&"other"), "{staged:?}");
    let subs = repo.submodules(&ws.status);
    let added = subs.iter().find(|s| s.path == "other").expect("listed");
    assert_eq!(added.state, SubmoduleState::UpToDate, "{added:?}");
    assert_eq!(added.branch.as_deref(), Some("dev"));
    assert_eq!(added.checked_out, Some(fx.ids[fx.c.lib_four]));

    // An explicit path; a second add of the same path is git's error.
    repo.add_submodule(&slash(&fx.lib), "vendor/lib2", "", false).unwrap();
    assert!(sup.join("vendor/lib2/l.txt").is_file());
    assert!(repo.add_submodule(&url, "other", "", false).is_err());
}

/// A local path where git refuses the `file` transport for submodules (here:
/// `protocol.file.allow = user` from the environment git runs with; this repository's own
/// `always` doesn't count — the submodule's clone doesn't read it): the dialog's context
/// says so, a plain add fails with the refusal explained, an add the user allowed runs
/// with `-c protocol.file.allow=always` and stores no such setting; `.gitmodules` records
/// the path as typed.
/// Workflow: SM-04
#[gitti_test::test]
fn add_submodule_from_a_local_path_asks_for_the_file_transport() {
    use gitti_core::submodule::add::FilePermission;
    let fx = setup();
    let sup = &fx.sup;
    let config = sup.join(".git/config");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("[protocol \"file\"]\n\tallow = always\n");
    fs::write(&config, &text).unwrap();
    let mut repo = Repo::open(sup).unwrap();
    // No global / system config of this machine.
    repo.set_env(
        [
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "protocol.file.allow"),
            ("GIT_CONFIG_VALUE_0", "user"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
    );
    let url = slash(&fx.lib);
    let ctx = repo.submodule_add_context();
    assert_eq!(ctx.file_allow.as_deref(), Some("user"));
    assert_eq!(ctx.needs_permission(&url), FilePermission::Ask);
    assert_eq!(ctx.needs_permission("https://example.com/lib.git"), FilePermission::NotNeeded);
    let err = repo.add_submodule(&url, "vendor/a", "", false).unwrap_err().to_string();
    assert!(err.contains("protocol.file.allow") && err.contains("not allowed"), "{err}");
    repo.add_submodule(&url, "vendor/a", "", true).unwrap();
    assert!(sup.join("vendor/a/l.txt").is_file());
    let gitmodules = fs::read_to_string(sup.join(".gitmodules")).unwrap();
    assert!(gitmodules.contains(&format!("url = {url}")), "{gitmodules}");
    let after = fs::read_to_string(&config).unwrap();
    assert_eq!(after.matches("always").count(), 1, "only this add allowed it: {after}");
}

/// `git status` calls of the nested-submodule kind (`--untracked-files=normal`) so far.
fn nested_status_calls(repo: &Repo) -> usize {
    repo.command_log()
        .snapshot()
        .iter()
        .filter(|e| e.args.iter().any(|a| a == "status") && e.args.iter().any(|a| a == "--untracked-files=normal"))
        .count()
}

/// Workflow: SM-01
#[gitti_test::test]
fn nested_dirty_state_comes_from_a_status_in_its_dirty_parent() {
    let fx = setup();
    let repo = open(&fx.sup);
    let inner = fx.inner();
    // Clean: no status call beyond the top level's.
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert!(snap.submodules.iter().all(|s| s.state == SubmoduleState::UpToDate));
    assert_eq!(nested_status_calls(&repo), 0);

    // A modified file in the nested one: both are modified, one status in libs/lib.
    fs::write(inner.join("i.txt"), "changed\n").unwrap();
    let snap = repo.snapshot(&Default::default()).unwrap();
    let s = &snap.submodules;
    assert_eq!(s[0].state, SubmoduleState::Modified, "{s:#?}");
    assert_eq!(s[1].path, "libs/lib/vendor/inner");
    assert_eq!(s[1].state, SubmoduleState::Modified, "{s:#?}");
    assert!(s[1].flags.modified && !s[1].flags.untracked);
    assert_eq!(s[1].detail(), "modified");
    assert_eq!(nested_status_calls(&repo), 1);
    // After a stage / unstage / discard: kept from the previous list, no git call.
    let after = repo.submodules_after(&snap.working.status, s);
    assert_eq!(after, *s);
    assert_eq!(nested_status_calls(&repo), 1);

    // Only an untracked file in the nested one.
    fs::write(inner.join("i.txt"), "i\n").unwrap();
    fs::write(inner.join("u.txt"), "u\n").unwrap();
    let s = repo.snapshot(&Default::default()).unwrap().submodules;
    assert!(s[1].flags.untracked && !s[1].flags.modified, "{s:#?}");
    assert_eq!(s[1].detail(), "untracked files");

    // Dirty parent, clean nested one: the nested one stays up to date.
    fs::remove_file(inner.join("u.txt")).unwrap();
    fs::write(fx.sub().join("l.txt"), "dirty\n").unwrap();
    let s = repo.snapshot(&Default::default()).unwrap().submodules;
    assert_eq!(s[0].state, SubmoduleState::Modified);
    assert_eq!(s[1].state, SubmoduleState::UpToDate, "{s:#?}");
}

/// Workflow: SM-03
#[gitti_test::test]
fn update_to_the_remote_branch() {
    let fx = setup();
    let repo = open(&fx.sup);
    let sub = fx.sub();
    // lib's main moves on upstream (the ref file; the objects are in the shared store).
    fs::write(fx.lib.join(".git/refs/heads/main"), format!("{}\n", fx.id(fx.c.lib_four))).unwrap();

    // --remote follows lib's main, a plain update goes back to the recorded commit.
    repo.update_submodules_remote(&["libs/lib"]).unwrap();
    assert_eq!(head(&sub), fx.id(fx.c.lib_four));
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::Modified);
    assert_eq!(subs[0].detail(), "2 new commits");
    repo.update_submodules(&[]).unwrap();
    assert_eq!(head(&sub), fx.id(fx.c.lib_two));
}

/// A nested submodule's path runs in its parent submodule (git of the top level doesn't
/// know it).
/// Workflow: SM-03
#[gitti_test::test]
fn update_and_sync_nested_paths() {
    let fx = setup();
    let repo = open(&fx.sup);
    let inner = fx.inner();
    commit_tracked(&inner, "i.txt", "local\n", "inner local");
    assert_eq!(list(&repo)[1].state, SubmoduleState::Modified);
    repo.update_submodules(&["libs/lib/vendor/inner"]).unwrap();
    assert_eq!(head(&inner), fx.id(fx.c.inner_one));
    repo.sync_submodules(&["libs/lib/vendor/inner", "libs/lib"]).unwrap();
}

/// Workflow: SM-05
#[gitti_test::test]
fn remove_submodule_warns_about_local_work_and_cleans_up() {
    let fx = setup();
    let sup = &fx.sup;
    let repo = open(sup);
    let sub = fx.sub();
    let work = |repo: &Repo| {
        let subs = repo.snapshot(&Default::default()).unwrap().submodules;
        gitti_core::submodule::local_work(sup, &subs, "libs/lib")
    };
    // Fresh clones: nothing to lose.
    assert_eq!(work(&repo), []);
    // A commit no remote has (nested), an untracked file, a stash.
    commit_tracked(&fx.inner(), "i.txt", "local\n", "inner local");
    fs::write(sub.join("u.txt"), "u\n").unwrap();
    fs::write(sub.join("l.txt"), "stash me\n").unwrap();
    sh(&sub, &["stash", "-q"]);
    let w = work(&repo);
    assert_eq!(w.len(), 2, "{w:#?}");
    assert!(w[0].path == "libs/lib" && w[0].untracked && w[0].modified && w[0].stash, "{w:#?}");
    assert_eq!(w[1].unpushed, 1);
    assert_eq!(w[1].describe(), "libs/lib/vendor/inner: 1 commit on no remote");

    // Removal: files, .gitmodules (it was the only one), config, .git/modules/libs/lib.
    let modules = sup.join(".git/modules/libs/lib");
    assert!(modules.is_dir());
    repo.remove_submodule("libs/lib").unwrap();
    assert!(!sub.exists());
    assert!(!modules.exists());
    assert!(!sup.join(".gitmodules").exists());
    assert!(!repo.has_gitmodules());
    assert_eq!(list(&repo), []);
    let ws = repo.working_state().unwrap();
    let staged: Vec<_> = ws.staged.iter().map(|f| f.path()).collect();
    assert_eq!(staged, [".gitmodules", "libs/lib"]);
    assert!(ws.unstaged.is_empty(), "{:?}", ws.unstaged);
    let config = fs::read_to_string(sup.join(".git/config")).unwrap();
    assert!(!config.contains("[submodule"), "{config}");
}

/// Workflow: SM-05
#[gitti_test::test]
fn remove_nested_and_uninitialised_submodules() {
    let fx = setup();
    let sup = &fx.sup;
    let repo = open(sup);
    // Nested: removed from its parent submodule, which now has changes.
    repo.remove_submodule("libs/lib/vendor/inner").unwrap();
    assert!(!fx.inner().exists());
    assert!(!sup.join(".git/modules/libs/lib/modules/vendor/inner").exists());
    assert!(sup.join(".git/modules/libs/lib").is_dir());
    let subs = list(&repo);
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].state, SubmoduleState::Modified, "{subs:#?}");

    // A second submodule (not initialised here), then the first one uninitialised too:
    // .gitmodules keeps the other.
    let modules = fs::read_to_string(sup.join(".gitmodules")).unwrap();
    let two = format!("{modules}[submodule \"two\"]\n\tpath = two\n\turl = {}\n", slash(&fx.lib));
    fs::write(sup.join(".gitmodules"), two).unwrap();
    fs::create_dir(sup.join("two")).unwrap();
    let info = format!("160000,{},two", fx.id(fx.c.lib_two));
    sh(sup, &["update-index", "--add", "--cacheinfo", &info, ".gitmodules"]);
    sh(sup, &["submodule", "deinit", "-q", "-f", "--", "libs/lib"]);
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::Uninitialized);
    repo.remove_submodule("libs/lib").unwrap();
    let paths: Vec<String> = list(&repo).into_iter().map(|s| s.path).collect();
    assert_eq!(paths, ["two"]);
    assert!(fs::read_to_string(sup.join(".gitmodules")).unwrap().contains("[submodule \"two\"]"));
    assert!(!sup.join(".git/modules/libs/lib").exists());
}

/// "Check Out Commit in Submodule…" — the submodule's history (newest first, with
/// what is recorded / checked out), the detached checkout, the pointer change it leaves in
/// the superproject (staged and committed like any file, which records that commit).
/// Workflow: SM-06
#[gitti_test::test]
fn check_out_a_commit_in_the_submodule_records_it() {
    let fx = setup_with(true);
    let repo = open(&fx.sup);
    let sub = fx.sub();
    let two = fx.id(fx.c.lib_two);
    let four = fx.id(fx.c.lib_four);

    let subs = list(&repo);
    let sm = &subs[0];
    assert_eq!(sm.committed, Some(oid(&two)));
    assert_eq!(sm.pointer_chain(), Some(format!("{} → {}", &two[..7], &four[..7])));
    assert_eq!(sm.row_detail(), format!("{} → {}", &two[..7], &four[..7]));
    let commits = gitti_core::submodule::history(&fx.sup, sm, 100).unwrap();
    let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["lib four", "lib three", "lib two: inner", "lib one"]);
    assert!(commits[0].checked_out && !commits[0].recorded);
    assert_eq!(commits[0].refs, ["main"]);
    assert_eq!(commits[0].marks(), "checked out · main");
    assert!(commits[2].recorded && commits[2].committed && !commits[2].checked_out);
    assert_eq!(commits[2].marks(), "recorded · origin/main");
    assert_eq!(gitti_core::submodule::history(&fx.sup, sm, 1).unwrap().len(), 1);
    // HEAD on a branch, nothing changed: no questions.
    let three = commits[1].id;
    let risks = gitti_core::submodule::checkout_risks(&fx.sup, &subs, "libs/lib", three);
    assert_eq!(risks, Vec::<String>::new());

    // Check out an older one: detached there, the pointer change shows up unstaged.
    repo.checkout_submodule_commit("libs/lib", three).unwrap();
    assert_eq!(sh(&sub, &["rev-parse", "HEAD", "--abbrev-ref", "HEAD"]), format!("{three}\nHEAD\n"));
    let ws = repo.working_state().unwrap();
    let f = ws.unstaged.iter().find(|f| f.path() == "libs/lib").expect("pointer change");
    let d = f.diff.as_ref().unwrap().submodule.as_ref().unwrap();
    assert_eq!((d.old, d.new), (Some(oid(&two)), Some(three)));
    assert_eq!(d.log.as_ref().unwrap().added_total, 1);

    // Staged: the index records it (HEAD still the old one) …
    repo.stage_paths(&["libs/lib"]).unwrap();
    let ws = repo.working_state().unwrap();
    let sm = repo.submodules(&ws.status).remove(0);
    assert_eq!(sm.state, SubmoduleState::UpToDate);
    assert_eq!((sm.committed, sm.recorded), (Some(oid(&two)), Some(three)));
    let short3 = &three.to_string()[..7];
    assert_eq!(sm.row_detail(), format!("{} → {short3}", &two[..7]));
    assert!(sm.describe().contains(&format!("recorded in the index (staged): {short3}")), "{}", sm.describe());
    // … committed: the superproject's history now records that commit.
    repo.commit("pin lib to lib three", false).unwrap();
    assert_eq!(sh(&fx.sup, &["rev-parse", "HEAD:libs/lib"]).trim(), three.to_string());
    let sm = list(&repo).remove(0);
    assert_eq!(sm.pointer_chain(), None);
    assert_eq!(sm.row_detail(), "");
}

/// The risks named before a submodule checkout (a commit only the detached HEAD has,
/// uncommitted changes), git's refusal reaching the caller, nested submodules (checked
/// out in their own repository, the pointer change shows in the parent submodule), and
/// a clear error for an uninitialised one.
/// Workflow: SM-06
#[gitti_test::test]
fn submodule_checkout_risks_nested_and_uninitialised() {
    let fx = setup();
    let repo = open(&fx.sup);
    let sub = fx.sub();
    let lib_one = oid(&sh(&sub, &["rev-parse", "HEAD~1"]));
    // Detached at the recorded commit, a commit there that nothing else has, dirty.
    sh(&sub, &["checkout", "-q", "--detach"]);
    commit_tracked(&sub, "l.txt", "detached\n", "detached work");
    fs::write(sub.join("l.txt"), "dirty\n").unwrap();
    let subs = list(&repo);
    let risks = gitti_core::submodule::checkout_risks(&fx.sup, &subs, "libs/lib", lib_one);
    assert_eq!(risks.len(), 2, "{risks:?}");
    assert!(risks[0].starts_with("uncommitted changes"), "{risks:?}");
    assert!(risks[1].starts_with("1 commit on no branch"), "{risks:?}");
    // git refuses when the commit changes the dirty file: the error reaches the caller.
    let err = repo.checkout_submodule_commit("libs/lib", lib_one).unwrap_err();
    assert!(format!("{err:#}").contains("l.txt"), "{err:#}");
    sh(&sub, &["checkout", "-q", "--", "l.txt"]);

    // Nested: checked out in its own repository; the parent submodule has the change.
    let inner = fx.inner();
    let inner_one = fx.ids[fx.c.inner_one];
    commit_tracked(&inner, "i.txt", "i\n2\n", "inner two");
    let subs = list(&repo);
    let nested = subs.iter().find(|s| s.path == "libs/lib/vendor/inner").unwrap();
    let commits = gitti_core::submodule::history(&fx.sup, nested, 100).unwrap();
    assert_eq!(commits.len(), 2);
    repo.checkout_submodule_commit("libs/lib/vendor/inner", inner_one).unwrap();
    assert_eq!(head(&inner), inner_one.to_string());
    repo.checkout_submodule_commit("libs/lib/vendor/inner", commits[0].id).unwrap();
    let ws = repo.working_state().unwrap();
    let nested = repo.submodules(&ws.status).into_iter().find(|s| s.depth == 1).unwrap();
    assert_eq!(nested.state, SubmoduleState::Modified);
    assert_eq!((nested.ahead, nested.behind), (1, 0));

    // Not initialised: a clear error instead of a git failure.
    sh(&fx.sup, &["submodule", "deinit", "-q", "-f", "--", "libs/lib"]);
    let subs = list(&repo);
    assert!(gitti_core::submodule::history(&fx.sup, &subs[0], 10).is_err());
    assert!(repo.checkout_submodule_commit("libs/lib", lib_one).is_err());
}
