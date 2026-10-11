//! BR — branches and switching: create, switch (clean, carrying local changes, blocked
//! and the ways out), rename, delete (merged, unmerged, several at once), remote branches,
//! a detached HEAD and keeping its work; RL-02 (a deleted branch brought back) shares the
//! deletion fixture.

use std::path::{Path, PathBuf};

use crate::expect::{Checks, Expect, Head, OpState};

/// `origin` for a fixture at `dir`: an empty bare repository `<dir>-origin.git`, laid out
/// by hand (no git process) — the remote-tracking refs come from the fixture's history.
fn origin(dir: &Path) -> PathBuf {
    let mut p = dir.as_os_str().to_owned();
    p.push("-origin.git");
    let p = PathBuf::from(p);
    crate::fixture::bare(&p);
    p
}

/// The status as sorted `XY path` lines.
fn status(lines: &[&str]) -> Vec<String> {
    let mut l: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    l.sort();
    l
}

/// HEAD is on `branch`, no operation half-done.
fn on(c: &mut Checks, e: &Expect, branch: &str) {
    c.eq("HEAD", e.head(), Head::Branch(branch.to_string()));
    c.eq("state", e.state(), OpState::Clean);
}

/// **BR-01 / BR-02 — create a branch.** `main`: `init` → `second` → `third`.
///
/// - BR-01: Create Branch at HEAD… [`HERE`], checked out ([`expect_here`]).
/// - BR-02: Create Branch Here… on `second` as [`OLDER`], not checked out
///   ([`expect_older`]).
pub mod create {
    use std::path::Path;

    use super::on;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    /// The branch made at HEAD (BR-01) and the one at the older commit (BR-02).
    pub const HERE: &str = "topic";
    pub const OLDER: &str = "hotfix";

    pub struct Setup {
        pub init: String,
        pub second: String,
        pub third: String,
    }

    /// Three commits on `main`, HEAD on it, clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a1\n")]);
        let second = fx.commit("main", "second", &[put("a.txt", "a2\n")]);
        let third = fx.commit("main", "third", &[put("b.txt", "b\n")]);
        let ids = fx.build(dir);
        Setup { init: ids[init].into(), second: ids[second].into(), third: ids[third].into() }
    }

    /// BR-01: [`HERE`] at `third`, checked out; `main` unmoved. One git process.
    pub fn expect_here(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-01 after Create Branch");
        on(&mut c, &e, HERE);
        c.eq("topic", e.ref_id(HERE), Some(s.third.clone()));
        c.eq("main", e.ref_id("main"), Some(s.third.clone()));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }

    /// BR-02: [`OLDER`] at `second`, HEAD still on `main` at `third`, the files unchanged.
    /// No git.
    pub fn expect_older(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-02 after Create Branch Here");
        on(&mut c, &e, "main");
        c.eq("hotfix", e.ref_id(OLDER), Some(s.second.clone()));
        c.eq("main", e.ref_id("main"), Some(s.third.clone()));
        c.ok("not at init", e.ref_id(OLDER) != Some(s.init.clone()), &s.init);
        c.text("b.txt on disk", e.file("b.txt").as_deref(), Some("b\n"));
        c.done()
    }
}

/// **BR-03 / BR-04 — switch.** `main`: `init` (`a.txt`, `notes.txt`); `feat` from it:
/// `Feature` (`a.txt` changed, `f.txt` new). The user switches to `feat`.
///
/// - BR-03 (clean): HEAD on `feat`, its files on disk ([`expect`] with `dirty` false).
/// - BR-04: `notes.txt` (the same in both branches) edited first: the edit comes along
///   ([`expect`] with `dirty` true).
pub mod switching {
    use std::path::Path;

    use super::on;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "feat";
    /// `a.txt` in `feat`.
    pub const FEAT_A: &str = "a\nchanged in feat\n";
    pub const NOTES_WIP: &str = "notes\nwork in progress\n";

    pub struct Setup {
        pub feat: String,
    }

    /// The two branches, HEAD on `main`; `dirty`: `notes.txt` edited. Three git processes.
    pub fn fixture(dir: &Path, dirty: bool) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n"), put("notes.txt", "notes\n")]);
        let feat = fx.commit_from(
            TARGET,
            init,
            "Feature",
            &[put("a.txt", FEAT_A), put("f.txt", "feature\n")],
        );
        if dirty {
            fx.write("notes.txt", NOTES_WIP);
        }
        let ids = fx.build(dir);
        Setup { feat: ids[feat].into() }
    }

    /// After the switch; one git process.
    pub fn expect(dir: &Path, s: &Setup, dirty: bool) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new(if dirty { "BR-04 after the switch" } else { "BR-03 after the switch" });
        on(&mut c, &e, TARGET);
        c.eq("feat", e.ref_id(TARGET), Some(s.feat.clone()));
        c.text("a.txt on disk", e.file("a.txt").as_deref(), Some(FEAT_A));
        c.text("f.txt on disk", e.file("f.txt").as_deref(), Some("feature\n"));
        let (notes, status) = if dirty {
            (NOTES_WIP, vec![" M notes.txt".to_string()])
        } else {
            ("notes\n", Vec::new())
        };
        c.text("notes.txt on disk", e.file("notes.txt").as_deref(), Some(notes));
        c.eq("status", e.status_lines(), status);
        c.done()
    }
}

/// **BR-05 / BR-06 — a switch blocked by a local change, Stash & Switch.** `main`:
/// `init` (`a.txt`, ten lines); `feat`: `Feature` changes line 1. The user has changed
/// `a.txt` too, so git refuses to switch; the app offers Stash & Switch.
///
/// - BR-05: the change is on line 10 — re-applied on `feat` cleanly, the stash gone
///   ([`expect_carried`]).
/// - BR-06: the change is on line 1 as well — the re-apply conflicts: on `feat`,
///   `a.txt` conflicted, the stash kept ([`expect_conflicted`]).
pub mod blocked {
    use std::path::Path;

    use super::on;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "feat";
    /// The blocking file.
    pub const FILE: &str = "a.txt";

    /// `a.txt`: line 1 and line 10 as given, lines 2–9 `line N`.
    pub fn a(first: &str, tenth: &str) -> String {
        let mut s = format!("{first}\n");
        for i in 2..10 {
            s.push_str(&format!("line {i}\n"));
        }
        s.push_str(&format!("{tenth}\n"));
        s
    }

    pub fn base() -> String {
        a("line 1", "line 10")
    }

    pub fn feat() -> String {
        a("line 1 in feat", "line 10")
    }

    /// The local change: line 10 (`overlap` false) or line 1 (true).
    pub fn local(overlap: bool) -> String {
        if overlap { a("line 1 mine", "line 10") } else { a("line 1", "line 10 mine") }
    }

    pub struct Setup {
        pub feat: String,
    }

    /// HEAD on `main`, `a.txt` changed ([`local`]). Three git processes.
    pub fn fixture(dir: &Path, overlap: bool) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put(FILE, &base())]);
        let feat = fx.commit_from(TARGET, init, "Feature", &[put(FILE, &feat())]);
        fx.write(FILE, &local(overlap));
        let ids = fx.build(dir);
        Setup { feat: ids[feat].into() }
    }

    /// Nothing happened (the switch was refused, the dialog cancelled). No git.
    pub fn expect_refused(dir: &Path, overlap: bool) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-05 after the refused switch");
        on(&mut c, &e, "main");
        c.text("a.txt on disk", e.file(FILE).as_deref(), Some(&local(overlap)));
        c.eq("stashes", e.stashes(), 0);
        c.done()
    }

    /// BR-05: on `feat`, `a.txt` = feat's line 1 and the local line 10, unstaged; no
    /// stash left. One git process.
    pub fn expect_carried(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-05 after Stash & Switch");
        on(&mut c, &e, TARGET);
        c.eq("feat", e.ref_id(TARGET), Some(s.feat.clone()));
        c.text("a.txt on disk", e.file(FILE).as_deref(), Some(&a("line 1 in feat", "line 10 mine")));
        c.eq("stashes", e.stashes(), 0);
        c.eq("status", e.status_lines(), vec![" M a.txt".to_string()]);
        c.done()
    }

    /// BR-06: on `feat`, `a.txt` conflicted (both line-1 versions in markers), the stash
    /// kept. One git process.
    pub fn expect_conflicted(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-06 after Stash & Switch");
        on(&mut c, &e, TARGET);
        c.eq("feat", e.ref_id(TARGET), Some(s.feat.clone()));
        let text = String::from_utf8_lossy(&e.file(FILE).unwrap_or_default()).into_owned();
        c.ok(
            "a.txt has both sides in conflict markers",
            text.contains("<<<<<<<") && text.contains("line 1 in feat") && text.contains("line 1 mine"),
            &text,
        );
        c.eq("stashes", e.stashes(), 1);
        c.eq("status", e.status_lines(), vec!["UU a.txt".to_string()]);
        c.done()
    }
}

/// **BR-09 — many blocking untracked files.** `feat` adds [`COUNT`] generated files;
/// the same paths exist untracked on `main` (with other content), next to `keep.txt`.
/// Stash & Switch moves all of them into one stash and switches; `keep.txt` stays.
pub mod many_untracked {
    use std::path::Path;

    use super::on;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Change, Fixture, put};

    pub const TARGET: &str = "feat";
    pub const COUNT: usize = 200;

    pub fn path(i: usize) -> String {
        format!("gen/out-{i:03}.txt")
    }

    /// HEAD on `main` with the untracked files. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let files: Vec<Change> = (0..COUNT).map(|i| put(&path(i), &format!("feat {i}\n"))).collect();
        fx.commit_from(TARGET, init, "Generated files", &files);
        for i in 0..COUNT {
            fx.write(&path(i), &format!("mine {i}\n"));
        }
        fx.write("keep.txt", "keep\n");
        fx.build(dir);
    }

    /// On `feat`, its versions on disk, one stash with ours, `keep.txt` untracked as
    /// before. One git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-09 after Stash & Switch");
        on(&mut c, &e, TARGET);
        for i in [0, COUNT / 2, COUNT - 1] {
            c.text(&path(i), e.file(&path(i)).as_deref(), Some(&format!("feat {i}\n")));
        }
        c.eq("stashes", e.stashes(), 1);
        c.eq("status", e.status_lines(), vec!["?? keep.txt".to_string()]);
        c.done()
    }
}

/// **BR-10 — switch to a branch with fewer ignore rules.** `plain`: `init`; `main` adds
/// `.gitignore` with `target/`; build output in `target/debug/`. The user switches to
/// `plain`: the output shows as untracked, and the status line says why ([`NOTE`]).
pub mod ignore_rules {
    use std::path::Path;

    use super::on;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "plain";
    /// How the status line starts.
    pub const NOTE: &str = "Switched to plain. 2 files that main ignores are not ignored in plain";

    /// HEAD on `main` with two ignored build files. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        let init = fx.commit("plain", "init", &[put("a.txt", "a\n")]);
        fx.commit_from("main", init, "ignore", &[put(".gitignore", "target/\n")]);
        fx.write("target/debug/x.o", "x");
        fx.write("target/debug/y.o", "y");
        fx.build(dir);
    }

    /// On `plain`, the build files untracked (still there). One git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-10 after the switch");
        on(&mut c, &e, TARGET);
        c.ok("x.o kept", e.file("target/debug/x.o").is_some(), "gone");
        c.eq(
            "status",
            e.status_lines(),
            super::status(&["?? target/debug/x.o", "?? target/debug/y.o"]),
        );
        c.done()
    }
}

/// **BR-11 — rename the current branch.** `main` tracks `origin/main`; the user renames
/// it to [`NEW`]: HEAD follows, the upstream settings move along.
pub mod rename {
    use std::path::Path;

    use super::on;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const NEW: &str = "trunk";

    pub struct Setup {
        pub tip: String,
    }

    /// `main` (two commits) and `origin/main` at its tip, `origin` an empty bare
    /// repository `<dir>-origin.git`. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let tip = fx.commit("main", "second", &[put("a.txt", "b\n")]);
        fx.branch("refs/remotes/origin/main", tip);
        fx.remote("origin", &super::origin(dir));
        fx.upstream("main", "origin");
        let ids = fx.build(dir);
        Setup { tip: ids[tip].into() }
    }

    /// HEAD on [`NEW`] at the old tip, `main` gone, the upstream under the new name. No git.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-11 after the rename");
        on(&mut c, &e, NEW);
        c.eq("trunk", e.ref_id(NEW), Some(s.tip.clone()));
        c.eq("main", e.ref_id("main"), None);
        c.eq("branch.trunk.remote", e.config("branch.trunk.remote"), vec!["origin".to_string()]);
        c.eq("branch.trunk.merge", e.config("branch.trunk.merge"), vec!["refs/heads/main".to_string()]);
        c.eq("branch.main.remote", e.config("branch.main.remote"), Vec::<String>::new());
        c.eq("origin/main", e.ref_id("refs/remotes/origin/main"), Some(s.tip.clone()));
        c.done()
    }
}

/// **BR-12 / BR-13 / RL-02 / BR-19 — delete branches.** `main`: `one` → `two`;
/// [`MERGED`] at `one` (all its commits are in `main`); [`UNMERGED`] from `one` with
/// [`SIDE_WORK`] (the newest commit). Each was checked out once (HEAD's reflog: `side`,
/// then `done`, then `main` again).
///
/// - BR-12: Delete… on [`MERGED`] — gone ([`expect_gone`]).
/// - BR-13: Delete… on [`UNMERGED`] — refused, Force Delete offered naming
///   [`SIDE_WORK`]; forced — gone.
/// - RL-02: after that, HEAD's reflog still has [`SIDE_WORK`]; Create Branch Here… on it
///   as [`UNMERGED`] brings the branch back ([`expect_recovered`]; the apps start from
///   [`fixture_deleted`]).
/// - BR-19: Clean Up Merged Branches… deletes [`MERGED`] only; Delete Branches… with
///   Force deletes [`UNMERGED`].
pub mod delete {
    use std::path::Path;

    use super::on;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const MERGED: &str = "done";
    pub const UNMERGED: &str = "side";
    pub const SIDE_WORK: &str = "Side work";

    pub struct Setup {
        pub one: String,
        pub two: String,
        pub side: String,
    }

    impl Setup {
        /// The tip of `branch` as built.
        pub fn tip(&self, branch: &str) -> &str {
            match branch {
                MERGED => &self.one,
                UNMERGED => &self.side,
                _ => &self.two,
            }
        }
    }

    /// The branches as described; HEAD on `main`. Six git processes (the three checkouts
    /// write HEAD's reflog).
    pub fn fixture(dir: &Path) -> Setup {
        build(dir, false)
    }

    /// [`fixture`] with [`UNMERGED`] force-deleted already (by git): where RL-02 starts.
    /// Seven git processes.
    pub fn fixture_deleted(dir: &Path) -> Setup {
        build(dir, true)
    }

    fn build(dir: &Path, deleted: bool) -> Setup {
        let mut fx = Fixture::new();
        let one = fx.commit("main", "one", &[put("a.txt", "one\n")]);
        let two = fx.commit("main", "two", &[put("a.txt", "two\n")]);
        let side = fx.commit_from(UNMERGED, one, SIDE_WORK, &[put("s.txt", "s\n")]);
        fx.branch(MERGED, one);
        for b in [UNMERGED, MERGED, "main"] {
            fx.git_after(&["checkout", "-q", b]);
        }
        if deleted {
            fx.git_after(&["branch", "-q", "-D", UNMERGED]);
        }
        let ids = fx.build(dir);
        Setup { one: ids[one].into(), two: ids[two].into(), side: ids[side].into() }
    }

    /// Each of `gone` deleted, the other branches where they were; HEAD on `main`, clean.
    /// One git process.
    pub fn expect_gone(dir: &Path, s: &Setup, gone: &[&str], what: &str) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new(what);
        on(&mut c, &e, "main");
        for b in ["main", MERGED, UNMERGED] {
            let want = (!gone.contains(&b)).then(|| s.tip(b).to_string());
            c.eq(b, e.ref_id(b), want);
        }
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }

    /// Nothing deleted (refused or cancelled). One git process.
    pub fn expect_unchanged(dir: &Path, s: &Setup, what: &str) -> Result<(), String> {
        expect_gone(dir, s, &[], what)
    }

    /// RL-02: [`UNMERGED`] back at [`SIDE_WORK`]; HEAD still on `main`. No git.
    pub fn expect_recovered(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("RL-02 after Create Branch Here");
        on(&mut c, &e, "main");
        c.eq("side", e.ref_id(UNMERGED), Some(s.side.clone()));
        c.eq("main", e.ref_id("main"), Some(s.two.clone()));
        c.done()
    }

    /// The entry of HEAD's reflog that moved to [`SIDE_WORK`] (newest first, as the
    /// reflog views list them): where RL-02 finds the deleted branch's commit. No git.
    pub fn side_entry(dir: &Path, s: &Setup) -> Option<usize> {
        Expect::new(dir).reflog("HEAD").iter().position(|(_, new, _)| *new == s.side)
    }
}

/// **BR-14 — switch to a remote branch.** `origin/feat` exists, no local `feat`; the user
/// checks it out: a local `feat` tracking it, checked out.
pub mod remote {
    use std::path::Path;

    use super::on;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const REMOTE_BRANCH: &str = "origin/feat";
    pub const LOCAL: &str = "feat";

    pub struct Setup {
        pub feat: String,
    }

    /// `main` and `origin/main` at `init`; `origin/feat` one commit further (`f.txt`);
    /// `origin` an empty bare repository `<dir>-origin.git`. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.branch("refs/remotes/origin/main", init);
        let feat = fx.commit_from("refs/remotes/origin/feat", init, "Remote feature", &[put("f.txt", "f\n")]);
        fx.remote("origin", &super::origin(dir));
        fx.upstream("main", "origin");
        let ids = fx.build(dir);
        Setup { feat: ids[feat].into() }
    }

    /// On a new `feat` at `origin/feat`'s commit, tracking it. No git.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-14 after checking out origin/feat");
        on(&mut c, &e, LOCAL);
        c.eq("feat", e.ref_id(LOCAL), Some(s.feat.clone()));
        c.eq("origin/feat", e.ref_id("refs/remotes/origin/feat"), Some(s.feat.clone()));
        c.eq("branch.feat.remote", e.config("branch.feat.remote"), vec!["origin".to_string()]);
        c.eq("branch.feat.merge", e.config("branch.feat.merge"), vec!["refs/heads/feat".to_string()]);
        c.text("f.txt on disk", e.file("f.txt").as_deref(), Some("f\n"));
        c.done()
    }
}

/// **BR-16 / BR-18 — a detached HEAD.** `main`: `c1` → `c2` → `c3`; `fix.txt` untracked.
///
/// - BR-16: the user checks out `c2` (detached; `fix.txt` stays), stages `fix.txt`,
///   commits [`WORK`] ([`expect_committed`]), then Create Branch at HEAD… [`KEEP`]
///   ([`expect_kept`]). The second half starts from [`fixture_committed`].
/// - BR-18: from [`fixture_committed`] the user switches to `main` without a branch for
///   [`WORK`]: the status line warns that the commit is on no branch now (it names it,
///   [`LEFT`]) and where it still is; HEAD's reflog has it ([`expect_left`]).
pub mod detached {
    use std::path::Path;

    use crate::expect::{Checks, Expect, Head, OpState};
    use crate::fixture::{Fixture, put};

    pub const WORK: &str = "Fix on c2";
    pub const KEEP: &str = "keep";
    pub const FIX: &str = "fix.txt";
    pub const FIX_TEXT: &str = "the fix\n";
    /// What the status line says after leaving [`WORK`] behind (with its short id in
    /// front of it).
    pub const LEFT: &str = "1 commit of the detached HEAD is on no branch now:";

    pub struct Setup {
        pub c2: String,
        pub c3: String,
        /// [`WORK`] ([`fixture_committed`] only).
        pub work: Option<String>,
    }

    fn history(fx: &mut Fixture) -> (crate::fixture::Mark, crate::fixture::Mark) {
        fx.commit("main", "c1", &[put("a.txt", "a\n")]);
        let c2 = fx.commit("main", "c2", &[put("b.txt", "b\n")]);
        let c3 = fx.commit("main", "c3", &[put("c.txt", "c\n")]);
        (c2, c3)
    }

    /// HEAD on `main`, [`FIX`] untracked. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let (c2, c3) = history(&mut fx);
        fx.write(FIX, FIX_TEXT);
        let ids = fx.build(dir);
        Setup { c2: ids[c2].into(), c3: ids[c3].into(), work: None }
    }

    /// HEAD detached at [`WORK`] (on `c2`, adds [`FIX`]), which no ref has. Three git
    /// processes.
    pub fn fixture_committed(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let (c2, c3) = history(&mut fx);
        let work = fx.commit_from("detached-work", c2, WORK, &[put(FIX, FIX_TEXT)]);
        fx.forget("detached-work");
        fx.detach(work);
        let ids = fx.build(dir);
        Setup { c2: ids[c2].into(), c3: ids[c3].into(), work: Some(ids[work].into()) }
    }

    /// After checking out `c2`: detached there, `main` unmoved, [`FIX`] still untracked.
    /// One git process.
    pub fn expect_checked_out(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-16 after checking out c2");
        c.eq("HEAD", e.head(), Head::Detached(s.c2.clone()));
        c.eq("main", e.ref_id("main"), Some(s.c3.clone()));
        c.eq("c.txt gone", e.file("c.txt"), None);
        c.eq("status", e.status_lines(), vec![format!("?? {FIX}")]);
        c.done()
    }

    /// The commit on the detached HEAD: [`WORK`] on `c2` with [`FIX`]. One git process
    /// for the log, one for the files.
    fn work_commit(c: &mut Checks, e: &Expect, rev: &str, s: &Setup) -> Option<String> {
        let log = e.log(rev, 1);
        let head = log.first()?;
        c.eq("subject", head.subject(), WORK);
        c.eq("parent", head.parents.clone(), vec![s.c2.clone()]);
        let blob = e.blobs(&[&format!("{}:{FIX}", head.id)]).pop().flatten();
        c.text("fix.txt in the commit", blob.as_deref(), Some(FIX_TEXT));
        Some(head.id.clone())
    }

    /// After the commit: HEAD detached at [`WORK`], no branch moved. Three git processes.
    pub fn expect_committed(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-16 after the commit on the detached HEAD");
        c.eq("state", e.state(), OpState::Clean);
        c.ok("HEAD detached", e.branch().is_none(), e.head());
        work_commit(&mut c, &e, "HEAD", s);
        c.eq("main", e.ref_id("main"), Some(s.c3.clone()));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }

    /// After Create Branch at HEAD…: on [`KEEP`], which holds [`WORK`]. Three git processes.
    pub fn expect_kept(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-16 after Create Branch");
        c.eq("HEAD", e.head(), Head::Branch(KEEP.into()));
        c.eq("state", e.state(), OpState::Clean);
        let id = work_commit(&mut c, &e, KEEP, s);
        if let Some(work) = &s.work {
            c.eq("keep at the detached commit", id.as_ref(), Some(work));
        }
        c.eq("main", e.ref_id("main"), Some(s.c3.clone()));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }

    /// BR-18 after switching to `main`: on `main`, [`WORK`] in no ref but in HEAD's
    /// reflog (the entry that left it). One git process.
    pub fn expect_left(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BR-18 after leaving the detached HEAD");
        c.eq("HEAD", e.head(), Head::Branch("main".into()));
        c.eq("main", e.ref_id("main"), Some(s.c3.clone()));
        let work = s.work.clone().unwrap_or_default();
        let last = e.reflog("HEAD").into_iter().next();
        c.eq("HEAD's reflog left the commit", last.map(|l| l.0), Some(work));
        c.eq("fix.txt gone with the commit", e.file(FIX), None);
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **BR-17 — set the upstream.** `main` is one commit ahead of `origin/main` and has no
/// upstream; Set Upstream… → `origin/main`: the branch tracks it.
pub mod upstream {
    use std::path::Path;

    use super::on;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const UPSTREAM: &str = "origin/main";

    /// `main` (two commits), `origin/main` at the first, no upstream; `origin` an empty
    /// bare repository `<dir>-origin.git`. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.commit("main", "local work", &[put("a.txt", "b\n")]);
        fx.branch("refs/remotes/origin/main", init);
        fx.branch("refs/remotes/origin/other", init);
        fx.remote("origin", &super::origin(dir));
        fx.build(dir);
    }

    /// `main` tracks `origin/main` (`tracks` false: no upstream). No git.
    pub fn expect(dir: &Path, tracks: bool) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new(if tracks { "BR-17 after Set Upstream" } else { "BR-17 without an upstream" });
        on(&mut c, &e, "main");
        let (remote, merge) = if tracks {
            (vec!["origin".to_string()], vec!["refs/heads/main".to_string()])
        } else {
            (Vec::new(), Vec::new())
        };
        c.eq("branch.main.remote", e.config("branch.main.remote"), remote);
        c.eq("branch.main.merge", e.config("branch.main.merge"), merge);
        c.done()
    }
}
