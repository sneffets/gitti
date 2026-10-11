//! MG — merge: a fast-forward, a merge commit always (with a message), squash then commit,
//! fast-forward only on diverged branches, a clean three-way merge, conflicts resolved by
//! taking a side / after both sides renamed a file / in a binary file / in a text file
//! `.gitattributes` marks binary / in a submodule's pointer, abort, concluding with the
//! commit box emptied or edited, unrelated histories, a merge into a dirty working tree
//! (overlapping, not overlapping), a merge in a linked worktree; RN-05 (a merge across a
//! rename) shares the module.

use crate::expect::{Checks, Commit, Expect, OpState, conflicted};

/// git's message of `git merge feat`.
pub const MERGE: &str = "Merge branch 'feat'";

/// The status as sorted `XY path` lines.
fn status(lines: &[&str]) -> Vec<String> {
    let mut l: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    l.sort();
    l
}

/// HEAD is on `branch`, no operation half-done.
fn on(k: &mut Checks, e: &Expect, branch: &str) {
    k.eq("branch", e.branch().as_deref(), Some(branch));
    k.eq("state", e.state(), OpState::Clean);
}

/// HEAD is a merge commit of `parents` (in order) with `message`; HEAD's commit for further
/// checks. One git process.
fn merge_commit(k: &mut Checks, e: &Expect, parents: &[&str], message: &str) -> Option<Commit> {
    let head = e.log("HEAD", 1).pop();
    match &head {
        Some(c) => {
            k.eq("parents", c.parents.clone(), parents.iter().map(|p| p.to_string()).collect::<Vec<_>>());
            k.eq("message", c.message.as_str(), message);
        }
        None => k.fail("no HEAD commit".into()),
    }
    head
}

/// **MG-01 / MG-02 — fast-forward, or a merge commit always.** `main`: `init`; `feat` from
/// it: [`FEAT_ONE`], [`FEAT_TWO`] — `main` is behind `feat`. The user merges `feat`:
///
/// - MG-01 with the default options: `main` moves to `feat`, no merge commit
///   ([`expect_fast_forward`]);
/// - MG-02 with "always create a merge commit" (`--no-ff`) and [`MESSAGE`]: a merge commit
///   of `init` and `feat` with that message ([`expect_merge_commit`]).
pub mod ff {
    use std::path::Path;

    use super::{merge_commit, on};
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "feat";
    pub const FEAT_ONE: &str = "Feat one";
    pub const FEAT_TWO: &str = "Feat two";
    /// The message the user types for the merge commit (MG-02).
    pub const MESSAGE: &str = "Bring in feat as one merge";

    pub struct Setup {
        pub init: String,
        pub feat: String,
    }

    /// See the module docs; HEAD on `main`, clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.commit_from(TARGET, init, FEAT_ONE, &[put("f.txt", "f1\n")]);
        let feat = fx.commit(TARGET, FEAT_TWO, &[put("f.txt", "f2\n"), put("g.txt", "g\n")]);
        let ids = fx.build(dir);
        Setup { init: ids[init].into(), feat: ids[feat].into() }
    }

    fn files(k: &mut Checks, e: &Expect) {
        k.text("f.txt on disk", e.file("f.txt").as_deref(), Some("f2\n"));
        k.text("g.txt on disk", e.file("g.txt").as_deref(), Some("g\n"));
        k.eq("status", e.status_lines(), Vec::<String>::new());
    }

    /// MG-01: `main` at `feat`'s commit (no merge commit), `feat`'s files, clean. One git
    /// process.
    pub fn expect_fast_forward(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-01 after the fast-forward merge");
        on(&mut k, &e, "main");
        k.eq("main", e.ref_id("main"), Some(s.feat.clone()));
        k.eq("feat", e.ref_id(TARGET), Some(s.feat.clone()));
        files(&mut k, &e);
        k.done()
    }

    /// MG-02: HEAD a merge commit of `init` and `feat` with [`MESSAGE`], `feat`'s tree,
    /// clean. Two git processes.
    pub fn expect_merge_commit(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-02 after the --no-ff merge");
        on(&mut k, &e, "main");
        let head = merge_commit(&mut k, &e, &[&s.init, &s.feat], MESSAGE);
        let feat_tree = e.log(TARGET, 1).pop().map(|c| c.tree);
        k.eq("tree as feat's", head.map(|c| c.tree), feat_tree);
        files(&mut k, &e);
        k.done()
    }
}

/// **MG-03 / MG-04 / MG-05 — diverged branches.** `main`: `init` (`a.txt`), [`MAIN_WORK`]
/// (`m.txt`); `feat` from `init`: [`FEAT_ONE`], [`FEAT_TWO`] (`f.txt`). The user merges
/// `feat`:
///
/// - MG-03 with squash: `feat`'s change staged, no merge in progress, git's squash message
///   waiting ([`expect_squashed`]); the commit with that message is a one-parent commit
///   ([`expect_squash_committed`]);
/// - MG-04 with fast-forward only: refused, nothing changed ([`expect_unchanged`]);
/// - MG-05 with the default options: a merge commit with both sides' files
///   ([`expect_merged`]).
pub mod diverged {
    use std::path::Path;

    use super::{MERGE, merge_commit, on, status};
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "feat";
    pub const MAIN_WORK: &str = "Main work";
    pub const FEAT_ONE: &str = "Feat one";
    pub const FEAT_TWO: &str = "Feat two";
    /// The first line of git's squash message (`SQUASH_MSG`).
    pub const SQUASHED: &str = "Squashed commit of the following:";

    pub struct Setup {
        pub main: String,
        pub feat: String,
    }

    /// See the module docs; HEAD on `main`, clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.commit_from(TARGET, init, FEAT_ONE, &[put("f.txt", "f1\n")]);
        let feat = fx.commit(TARGET, FEAT_TWO, &[put("f.txt", "f2\n")]);
        let main = fx.commit("main", MAIN_WORK, &[put("m.txt", "m\n")]);
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// MG-03 after the squash merge: still on `main` at its commit, no merge in progress;
    /// `f.txt` staged as `feat` has it; `SQUASH_MSG` names both of `feat`'s commits. One git
    /// process.
    pub fn expect_squashed(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-03 after the squash merge");
        on(&mut k, &e, "main");
        k.eq("main", e.ref_id("main"), Some(s.main.clone()));
        k.ok("no MERGE_HEAD", e.ref_id("MERGE_HEAD").is_none(), "MERGE_HEAD");
        k.eq("status", e.status_lines(), status(&["A  f.txt"]));
        let msg = std::fs::read_to_string(e.git_dir().join("SQUASH_MSG")).unwrap_or_default();
        k.ok(
            "SQUASH_MSG",
            msg.starts_with(SQUASHED) && msg.contains(FEAT_ONE) && msg.contains(FEAT_TWO),
            &msg,
        );
        k.done()
    }

    /// MG-03 after the commit: a one-parent commit on `main`'s with git's squash message
    /// (both commits named) and both sides' files; `feat` unmoved, `SQUASH_MSG` gone,
    /// clean. Two git processes.
    pub fn expect_squash_committed(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-03 after committing the squash");
        on(&mut k, &e, "main");
        match e.log("HEAD", 1).pop() {
            Some(c) => {
                k.eq("parents", c.parents.clone(), vec![s.main.clone()]);
                let m = &c.message;
                k.ok(
                    "git's squash message",
                    m.starts_with(SQUASHED) && m.contains(FEAT_ONE) && m.contains(FEAT_TWO),
                    m,
                );
            }
            None => k.fail("no HEAD commit".into()),
        }
        k.eq("feat", e.ref_id(TARGET), Some(s.feat.clone()));
        k.ok("SQUASH_MSG gone", !e.git_dir().join("SQUASH_MSG").exists(), "SQUASH_MSG");
        let b = e.blobs(&["HEAD:f.txt", "HEAD:m.txt"]);
        k.text("committed f.txt", b[0].as_deref(), Some("f2\n"));
        k.text("committed m.txt", b[1].as_deref(), Some("m\n"));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }

    /// MG-04 after the refused fast-forward-only merge: nothing changed — `main` at its
    /// commit, no `f.txt`, no merge in progress, clean. One git process.
    pub fn expect_unchanged(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-04 after the refused --ff-only merge");
        on(&mut k, &e, "main");
        k.eq("main", e.ref_id("main"), Some(s.main.clone()));
        k.ok("no f.txt", e.file("f.txt").is_none(), "f.txt");
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }

    /// MG-05: a merge commit of `main` and `feat` ([`MERGE`]) with `m.txt` and `f.txt`,
    /// clean. Two git processes.
    pub fn expect_merged(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-05 after the three-way merge");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.feat], MERGE);
        k.text("f.txt on disk", e.file("f.txt").as_deref(), Some("f2\n"));
        k.text("m.txt on disk", e.file("m.txt").as_deref(), Some("m\n"));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **MG-07 / MG-10 — a merge stopped on two conflicts.** `main` and `feat` both changed
/// `a.txt` and `b.txt` ([`MAIN_A`] / [`FEAT_A`], …); `feat` also changed `c.txt`. The merge
/// stopped ([`expect_stopped`]):
///
/// - MG-07: the user takes ours for `a.txt`, theirs for `b.txt`, and continues: the merge
///   is concluded with each file as that side and `feat`'s `c.txt` ([`expect_concluded`]);
/// - MG-10: the user aborts: back at `main`'s commit and files ([`expect_aborted`]).
pub mod sides {
    use std::path::Path;

    use super::{MERGE, conflicted, merge_commit, on, status};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "feat";
    pub const MAIN_A: &str = "a main\n";
    pub const FEAT_A: &str = "a feat\n";
    pub const MAIN_B: &str = "b main\n";
    pub const FEAT_B: &str = "b feat\n";
    pub const FEAT_C: &str = "c feat\n";

    pub struct Setup {
        pub main: String,
        pub feat: String,
    }

    /// See the module docs; the merge stopped (one more git process). Four git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n"), put("b.txt", "b\n"), put("c.txt", "c\n")]);
        let feat = fx.commit_from(
            TARGET,
            init,
            "Feat a b c",
            &[put("a.txt", FEAT_A), put("b.txt", FEAT_B), put("c.txt", FEAT_C)],
        );
        let main = fx.commit("main", "Main a b", &[put("a.txt", MAIN_A), put("b.txt", MAIN_B)]);
        fx.git_after_stops(&["merge", TARGET]);
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// The stopped merge with both files resolved — `a.txt` as ours, `b.txt` as theirs,
    /// staged ([`expect_resolved`]) — for a driver that takes the rest of the story only.
    /// Five git processes.
    pub fn fixture_resolved(dir: &Path) -> Setup {
        let s = fixture(dir);
        std::fs::write(dir.join("a.txt"), MAIN_A).unwrap();
        std::fs::write(dir.join("b.txt"), FEAT_B).unwrap();
        crate::git::git(dir, &["add", "--", "a.txt", "b.txt"]);
        s
    }

    /// Both resolved (ours for `a.txt`, theirs for `b.txt`), nothing conflicted, still
    /// merging. One git process.
    pub fn expect_resolved(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-07 after a side per file");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.text("a.txt (ours)", e.file("a.txt").as_deref(), Some(MAIN_A));
        k.text("b.txt (theirs)", e.file("b.txt").as_deref(), Some(FEAT_B));
        k.eq("status", e.status_lines(), status(&["M  b.txt", "M  c.txt"]));
        k.done()
    }

    /// The merge stopped: merging `feat`, `a.txt` and `b.txt` conflicted, `c.txt` merged
    /// and staged. One git process.
    pub fn expect_stopped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-07 the stopped merge");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.eq("HEAD", e.ref_id("HEAD"), Some(s.main.clone()));
        let st = e.status();
        k.eq("conflicted", conflicted(&st), vec!["a.txt".to_string(), "b.txt".to_string()]);
        k.done()
    }

    /// MG-07: the merge concluded (git's message) with `a.txt` as `main` has it, `b.txt`
    /// and `c.txt` as `feat` has them, clean. Two git processes.
    pub fn expect_concluded(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-07 after taking a side per file and Continue");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.feat], MERGE);
        k.text("a.txt (ours)", e.file("a.txt").as_deref(), Some(MAIN_A));
        k.text("b.txt (theirs)", e.file("b.txt").as_deref(), Some(FEAT_B));
        k.text("c.txt (merged)", e.file("c.txt").as_deref(), Some(FEAT_C));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }

    /// MG-10: no merge in progress, `main` at its commit, its files back, nothing staged
    /// or left over. One git process.
    pub fn expect_aborted(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-10 after Abort");
        on(&mut k, &e, "main");
        k.eq("main", e.ref_id("main"), Some(s.main.clone()));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(MAIN_A));
        k.text("b.txt", e.file("b.txt").as_deref(), Some(MAIN_B));
        k.text("c.txt", e.file("c.txt").as_deref(), Some("c\n"));
        k.ok("no MERGE_MSG", !e.git_dir().join("MERGE_MSG").exists(), "MERGE_MSG");
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **MG-09 — both sides renamed a file differently.** `init` has `r.txt`; `main` renamed it
/// [`OURS_NAME`], `feat` [`THEIRS_NAME`]. The merge stopped with all three names conflicted
/// ([`expect_stopped`]). The user keeps `feat`'s name — Keep on [`THEIRS_NAME`], Delete on
/// [`OURS_NAME`] and `r.txt` — and continues: the merge has the file under that name only
/// ([`expect_concluded`]).
pub mod rename_rename {
    use std::path::Path;

    use super::{MERGE, merge_commit, on, status};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, Mark, del, put};

    pub const TARGET: &str = "feat";
    pub const OLD: &str = "r.txt";
    pub const OURS_NAME: &str = "main-name.txt";
    pub const THEIRS_NAME: &str = "feat-name.txt";
    pub const TEXT: &str = "r1\nr2\nr3\nr4\n";

    pub struct Setup {
        pub main: String,
        pub feat: String,
    }

    fn stopped() -> (Fixture, Mark, Mark) {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put(OLD, TEXT), put("a.txt", "a\n")]);
        let feat = fx.commit_from(TARGET, init, "Feat renames r", &[del(OLD), put(THEIRS_NAME, TEXT)]);
        let main = fx.commit("main", "Main renames r", &[del(OLD), put(OURS_NAME, TEXT)]);
        fx.git_after_stops(&["merge", TARGET]);
        (fx, main, feat)
    }

    /// See the module docs; the merge stopped. Four git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let (fx, main, feat) = stopped();
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// The stopped merge with the first two steps done — [`THEIRS_NAME`] kept, [`OURS_NAME`]
    /// deleted ([`expect_kept`]) — for a driver that takes the rest of the story only.
    /// Six git processes.
    pub fn fixture_kept(dir: &Path) -> Setup {
        let (mut fx, main, feat) = stopped();
        fx.git_after(&["add", "--", THEIRS_NAME]);
        fx.git_after(&["rm", "-q", "--", OURS_NAME]);
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// [`THEIRS_NAME`] kept, [`OURS_NAME`] deleted: still merging, only `r.txt`
    /// conflicted; git sees `main`'s name renamed to `feat`'s. One git process.
    pub fn expect_kept(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-09 after Keep and Delete");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.eq("status", e.status_lines(), status(&["DD r.txt", "R  feat-name.txt"]));
        k.done()
    }

    /// The stopped merge: both new names and the old one conflicted — only in theirs, only
    /// in ours, deleted by both. One git process.
    pub fn expect_stopped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-09 the stopped merge");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.eq(
            "status",
            e.status_lines(),
            status(&["UA feat-name.txt", "AU main-name.txt", "DD r.txt"]),
        );
        k.done()
    }

    /// After keeping `feat`'s name and Continue: a merge commit whose tree has the file as
    /// [`THEIRS_NAME`] only; on disk the same; clean. Three git processes.
    pub fn expect_concluded(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-09 after keeping feat's name and Continue");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.feat], MERGE);
        let b = e.blobs(&[
            &format!("HEAD:{THEIRS_NAME}"),
            &format!("HEAD:{OURS_NAME}"),
            &format!("HEAD:{OLD}"),
        ]);
        k.text("committed feat-name.txt", b[0].as_deref(), Some(TEXT));
        k.text("committed main-name.txt", b[1].as_deref(), None);
        k.text("committed r.txt", b[2].as_deref(), None);
        k.text("feat-name.txt on disk", e.file(THEIRS_NAME).as_deref(), Some(TEXT));
        k.ok("main-name.txt gone", e.file(OURS_NAME).is_none(), OURS_NAME);
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **MG-11 — conclude a merge with the commit box emptied or edited.** `main` and `feat`
/// both changed `a.txt`; the merge stopped and the user resolved it ([`RESOLVED`], staged).
/// The commit box shows git's message with its `# Conflicts:` lines. The user empties the
/// box and commits — git's message without those lines — or commits [`EDITED`]
/// ([`expect_concluded`] with the message).
pub mod conclude {
    use std::path::Path;

    use super::{merge_commit, on};
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};
    use crate::git;

    pub const TARGET: &str = "feat";
    pub const RESOLVED: &str = "a main and feat\n";
    /// The message the user writes instead of git's.
    pub const EDITED: &str = "Merge feat: both a lines kept";

    pub struct Setup {
        pub main: String,
        pub feat: String,
    }

    /// See the module docs. Five git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let feat = fx.commit_from(TARGET, init, "Feat a", &[put("a.txt", "a feat\n")]);
        let main = fx.commit("main", "Main a", &[put("a.txt", "a main\n")]);
        fx.git_after_stops(&["merge", TARGET]);
        let ids = fx.build(dir);
        std::fs::write(dir.join("a.txt"), RESOLVED).unwrap();
        git::git(dir, &["add", "a.txt"]);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// The merge concluded with `message`: no `Conflicts` line, the resolution committed,
    /// clean. Two git processes.
    pub fn expect_concluded(dir: &Path, s: &Setup, message: &str) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-11 after the commit");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.feat], message);
        k.text("committed a.txt", e.blobs(&["HEAD:a.txt"])[0].as_deref(), Some(RESOLVED));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **MG-12 — unrelated histories.** `main` and [`TARGET`] have no commit in common (each
/// its own root). git refuses the merge; the app offers "Merge anyway"
/// (`--allow-unrelated-histories`), which makes a merge commit of both
/// ([`expect_merged`]).
pub mod unrelated {
    use std::path::Path;

    use super::{merge_commit, on};
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "other";
    /// git's message of `git merge other`.
    pub const MESSAGE: &str = "Merge branch 'other'";

    pub struct Setup {
        pub main: String,
        pub other: String,
    }

    /// See the module docs; HEAD on `main`, clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let main = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let other = fx.commit(TARGET, "Other root", &[put("o.txt", "o\n")]);
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), other: ids[other].into() }
    }

    /// Nothing changed (after the refusal). No git.
    pub fn expect_refused(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-12 after the refused merge");
        on(&mut k, &e, "main");
        k.eq("main", e.ref_id("main"), Some(s.main.clone()));
        k.ok("no o.txt", e.file("o.txt").is_none(), "o.txt");
        k.done()
    }

    /// A merge commit of both roots with both files, clean. Two git processes.
    pub fn expect_merged(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-12 after Merge anyway");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.other], MESSAGE);
        k.text("a.txt", e.file("a.txt").as_deref(), Some("a\n"));
        k.text("o.txt", e.file("o.txt").as_deref(), Some("o\n"));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **MG-16 — a merge into a dirty tree, overlapping.** `main` (with [`MAIN_WORK`]) and
/// `feat` diverged; `feat` changes line 1 of `a.txt`, and the user changed line 10 of it
/// ([`local`]). git refuses the merge — it would overwrite `a.txt` — and nothing changes
/// ([`expect_refused`]); Stash and Merge saves the change, merges and puts it back on top
/// ([`expect_stashed_and_merged`]).
pub mod dirty_overlap {
    use std::path::Path;

    use super::{MERGE, merge_commit, on, status};
    use crate::br::blocked::a;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "feat";
    pub const FILE: &str = "a.txt";
    pub const MAIN_WORK: &str = "Main work";

    /// The user's change: line 10.
    pub fn local() -> String {
        a("line 1", "line 10 mine")
    }

    pub struct Setup {
        pub main: String,
        pub feat: String,
    }

    /// See the module docs; HEAD on `main`, `a.txt` changed. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put(FILE, &a("line 1", "line 10"))]);
        let feat = fx.commit_from(TARGET, init, "Feat line 1", &[put(FILE, &a("line 1 feat", "line 10"))]);
        let main = fx.commit("main", MAIN_WORK, &[put("m.txt", "m\n")]);
        fx.write(FILE, &local());
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// Refused: `main` at its commit, `a.txt` still the user's, no stash, no merge. One
    /// git process.
    pub fn expect_refused(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-16 after the refused merge");
        on(&mut k, &e, "main");
        k.eq("main", e.ref_id("main"), Some(s.main.clone()));
        k.text("a.txt", e.file(FILE).as_deref(), Some(&local()));
        k.eq("stashes", e.stashes(), 0);
        k.eq("status", e.status_lines(), status(&[" M a.txt"]));
        k.done()
    }

    /// After Stash and Merge: the merge commit, `a.txt` with `feat`'s line 1 and the
    /// user's line 10 (unstaged again), the stash gone. Two git processes.
    pub fn expect_stashed_and_merged(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-16 after Stash and Merge");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.feat], MERGE);
        k.text("a.txt", e.file(FILE).as_deref(), Some(&a("line 1 feat", "line 10 mine")));
        k.eq("stashes", e.stashes(), 0);
        k.eq("status", e.status_lines(), status(&[" M a.txt"]));
        k.done()
    }
}

/// **MG-17 — a merge into a dirty tree, not overlapping, that conflicts; abort.** The user
/// changed `notes.txt` ([`NOTES`]), which the merge doesn't touch; `main` and `feat` both
/// changed `a.txt`. The merge goes ahead and stops on `a.txt` with the change still there
/// ([`expect_stopped`]); Abort ends it and the change survives ([`expect_aborted`]).
pub mod dirty_apart {
    use std::path::Path;

    use super::{conflicted, on, status};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "feat";
    pub const NOTES: &str = "notes\nmine, not committed\n";
    pub const MAIN_A: &str = "a main\n";

    pub struct Setup {
        pub main: String,
        pub feat: String,
    }

    /// See the module docs; HEAD on `main`, `notes.txt` changed. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n"), put("notes.txt", "notes\n")]);
        let feat = fx.commit_from(TARGET, init, "Feat a", &[put("a.txt", "a feat\n")]);
        let main = fx.commit("main", "Main a", &[put("a.txt", MAIN_A)]);
        fx.write("notes.txt", NOTES);
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// The merge stopped on `a.txt`; `notes.txt` still changed (not staged). One git
    /// process.
    pub fn expect_stopped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-17 the stopped merge");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        let st = e.status();
        k.eq("conflicted", conflicted(&st), vec!["a.txt".to_string()]);
        k.eq("status", e.status_lines(), status(&["UU a.txt", " M notes.txt"]));
        k.text("notes.txt", e.file("notes.txt").as_deref(), Some(NOTES));
        k.done()
    }

    /// After Abort: no merge, `main` at its commit with its `a.txt`; `notes.txt` still the
    /// user's change. One git process.
    pub fn expect_aborted(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-17 after Abort");
        on(&mut k, &e, "main");
        k.eq("main", e.ref_id("main"), Some(s.main.clone()));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(MAIN_A));
        k.text("notes.txt", e.file("notes.txt").as_deref(), Some(NOTES));
        k.eq("status", e.status_lines(), status(&[" M notes.txt"]));
        k.done()
    }
}

/// **MG-20 — a merge conflict in a linked worktree.** `topic` is checked out in the linked
/// worktree [`crate::wt::linked_of`]`(dir, "topic")`; there the merge of `feat` stopped on
/// `a.txt`. The main worktree (`main`) is clean ([`expect_stopped`]). The user opens the
/// linked worktree, takes theirs and continues: `topic` gets the merge commit, the main
/// worktree is untouched ([`expect_concluded`]).
pub mod worktree {
    use std::path::{Path, PathBuf};

    use super::{conflicted, merge_commit, on};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "feat";
    pub const BRANCH: &str = "topic";
    pub const FEAT_A: &str = "a feat\n";
    /// git's message of `git merge feat` on `topic`.
    pub const MESSAGE: &str = "Merge branch 'feat' into topic";

    pub struct Setup {
        pub main: String,
        pub topic: String,
        pub feat: String,
        /// The linked worktree (where the merge stopped).
        pub linked: PathBuf,
    }

    /// See the module docs. Five git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let feat = fx.commit_from(TARGET, init, "Feat a", &[put("a.txt", FEAT_A)]);
        let topic = fx.commit_from(BRANCH, init, "Topic a", &[put("a.txt", "a topic\n")]);
        let linked = crate::wt::linked_of(dir, BRANCH);
        let name = linked.file_name().unwrap().to_string_lossy().into_owned();
        fx.git_after(&["worktree", "add", "-q", &format!("../{name}"), BRANCH]);
        fx.git_after_stops(&["-C", &format!("../{name}"), "merge", TARGET]);
        let ids = fx.build(dir);
        Setup { main: ids[init].into(), topic: ids[topic].into(), feat: ids[feat].into(), linked }
    }

    fn main_untouched(k: &mut Checks, m: &Expect, s: &Setup) {
        k.eq("main worktree: branch", m.branch().as_deref(), Some("main"));
        k.eq("main worktree: state", m.state(), OpState::Clean);
        k.eq("main worktree: main", m.ref_id("main"), Some(s.main.clone()));
        k.text("main worktree: a.txt", m.file("a.txt").as_deref(), Some("a\n"));
        k.eq("main worktree: status", m.status_lines(), Vec::<String>::new());
    }

    /// The merge stopped in the linked worktree (its own git dir has `MERGE_HEAD`); the main
    /// worktree clean, no merge there. Two git processes.
    pub fn expect_stopped(dir: &Path, s: &Setup) -> Result<(), String> {
        let l = Expect::new(&s.linked);
        let m = Expect::new(dir);
        let mut k = Checks::new("MG-20 the merge stopped in the linked worktree");
        k.eq("linked: state", l.state(), OpState::Merging);
        k.eq("linked: MERGE_HEAD", l.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.eq("linked: conflicted", conflicted(&l.status()), vec!["a.txt".to_string()]);
        main_untouched(&mut k, &m, s);
        k.done()
    }

    /// After taking theirs and Continue: `topic` is a merge commit of its commit and `feat`
    /// with `feat`'s `a.txt`, the linked worktree clean; the main worktree untouched. Three
    /// git processes.
    pub fn expect_concluded(dir: &Path, s: &Setup) -> Result<(), String> {
        let l = Expect::new(&s.linked);
        let m = Expect::new(dir);
        let mut k = Checks::new("MG-20 after Continue in the linked worktree");
        on(&mut k, &l, BRANCH);
        merge_commit(&mut k, &l, &[&s.topic, &s.feat], MESSAGE);
        k.text("linked: a.txt", l.file("a.txt").as_deref(), Some(FEAT_A));
        k.eq("linked: status", l.status_lines(), Vec::<String>::new());
        main_untouched(&mut k, &m, s);
        k.done()
    }
}

/// **MG-22 — a binary conflict.** `main` and `feat` both changed the binary `p.bin`
/// ([`OURS`] / [`THEIRS`]). The merge stopped; git can't merge binary contents, so the
/// file holds ours ([`expect_stopped`]). The apps offer no merge editor for it, but take
/// ours / theirs: the user takes theirs and continues ([`expect_concluded`]).
pub mod binary {
    use std::path::Path;

    use super::{MERGE, conflicted, merge_commit, on};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put, put_bytes};

    pub const TARGET: &str = "feat";
    pub const FILE: &str = "p.bin";
    pub const BASE: &[u8] = b"IMG\0\x01base\xff";
    pub const OURS: &[u8] = b"IMG\0\x02main\xff";
    pub const THEIRS: &[u8] = b"IMG\0\x03feat\xff";

    pub struct Setup {
        pub main: String,
        pub feat: String,
    }

    /// See the module docs; the merge stopped. Four git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put_bytes(FILE, BASE), put("a.txt", "a\n")]);
        let feat = fx.commit_from(TARGET, init, "Feat p", &[put_bytes(FILE, THEIRS)]);
        let main = fx.commit("main", "Main p", &[put_bytes(FILE, OURS)]);
        fx.git_after_stops(&["merge", TARGET]);
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// The merge stopped on `p.bin` only; the file on disk is ours, byte for byte (no
    /// markers). One git process.
    pub fn expect_stopped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-22 the stopped merge");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.eq("conflicted", conflicted(&e.status()), vec![FILE.to_string()]);
        k.eq("p.bin on disk", e.file(FILE), Some(OURS.to_vec()));
        k.done()
    }

    /// After taking theirs and Continue: the merge commit with `feat`'s `p.bin` (committed
    /// and on disk), clean. Three git processes.
    pub fn expect_concluded(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-22 after Take Theirs and Continue");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.feat], MERGE);
        k.eq("committed p.bin", e.blobs(&[&format!("HEAD:{FILE}")]).pop().flatten(), Some(THEIRS.to_vec()));
        k.eq("p.bin on disk", e.file(FILE), Some(THEIRS.to_vec()));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **MG-27 — a conflict in a text file `.gitattributes` marks binary.** `data/.gitattributes`
/// (nested) gives `*.csv` the `binary` macro (`-diff -merge -text`); `main` and `feat`
/// both changed line 2 of the text file [`FILE`]. The merge stops ([`fixture`]): git merged
/// the file as binary — no markers, ours on disk ([`expect_stopped`]). The apps show it as
/// a binary conflict with no merge editor; the user takes ours and continues
/// ([`expect_concluded`]).
pub mod binary_attr {
    use std::path::Path;

    use super::{MERGE, conflicted, merge_commit, on};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const TARGET: &str = "feat";
    pub const FILE: &str = "data/table.csv";
    /// The nested attributes file and its rule.
    pub const ATTRIBUTES: &str = "data/.gitattributes";
    pub const RULE: &str = "*.csv binary\n";
    pub const BASE: &str = "id,name\n1,base\n";
    pub const OURS: &str = "id,name\n1,main\n";
    pub const THEIRS: &str = "id,name\n1,feat\n";

    pub struct Setup {
        pub main: String,
        pub feat: String,
    }

    /// See the module docs; the merge stopped. Four git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put(ATTRIBUTES, RULE), put(FILE, BASE), put("a.txt", "a\n")]);
        let feat = fx.commit_from(TARGET, init, "Feat table", &[put(FILE, THEIRS)]);
        let main = fx.commit("main", "Main table", &[put(FILE, OURS)]);
        fx.git_after_stops(&["merge", TARGET]);
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// The merge stopped on the table only; the file on disk is ours as it was (no
    /// markers: git merged it as binary). One git process.
    pub fn expect_stopped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-27 the stopped merge");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.eq("conflicted", conflicted(&e.status()), vec![FILE.to_string()]);
        k.text("table on disk", e.file(FILE).as_deref(), Some(OURS));
        k.done()
    }

    /// After taking ours and Continue: the merge commit with `main`'s table (committed and
    /// on disk), clean. Three git processes.
    pub fn expect_concluded(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-27 after Take Ours and Continue");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.feat], MERGE);
        let committed = e.blobs(&[&format!("HEAD:{FILE}")]).pop().flatten();
        k.text("committed table", committed.as_deref(), Some(OURS));
        k.text("table on disk", e.file(FILE).as_deref(), Some(OURS));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **MG-21 — a submodule pointer conflict.** The submodule `lib` at [`PATH`] (its own
/// repository, checked out at `main`'s commit): `main` moved it from [`LIB_ONE`] to
/// [`LIB_MAIN`], `feat` to [`LIB_FEAT`]. The merge stops on the pointer ([`fixture`],
/// [`expect_stopped`]). The apps show it as a submodule conflict — ours → theirs, no merge
/// editor; the user takes theirs and continues: the merge records `feat`'s commit, the
/// submodule's checkout stays at `main`'s, so it shows as changed ([`expect_concluded`]).
pub mod submodule {
    use std::fs;
    use std::path::Path;

    use super::{MERGE, conflicted, merge_commit, on};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, gitlink, put};
    use crate::sm::pointer;

    pub const TARGET: &str = "feat";
    pub const PATH: &str = "libs/lib";
    pub const LIB_ONE: &str = "lib one";
    pub const LIB_MAIN: &str = "lib main";
    pub const LIB_FEAT: &str = "lib feat";

    pub struct Setup {
        pub main: String,
        pub feat: String,
        pub lib_one: String,
        pub lib_main: String,
        pub lib_feat: String,
    }

    /// See the module docs; the merge stopped. Seven git processes.
    pub fn fixture(dir: &Path) -> Setup {
        // `lib` next to the superproject first (its ids go into the gitlinks), moved into
        // its place after the merge stopped.
        let lib_dir = dir.with_file_name(format!(
            "{}-lib",
            dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        ));
        let mut l = Fixture::new();
        let one = l.commit("main", LIB_ONE, &[put("l.txt", "1\n")]);
        let lib_main = l.commit("main", LIB_MAIN, &[put("l.txt", "1\nmain\n")]);
        let lib_feat = l.commit_from("feat", one, LIB_FEAT, &[put("l.txt", "1\nfeat\n")]);
        let lib = l.build(&lib_dir);
        let (one, lib_main, lib_feat) = (lib[one].to_string(), lib[lib_main].to_string(), lib[lib_feat].to_string());

        let mut fx = Fixture::new();
        let modules = format!("[submodule \"{PATH}\"]\n\tpath = {PATH}\n\turl = ../lib.git\n");
        let init = fx.commit("main", "init", &[put("s.txt", "s\n"), put(".gitmodules", &modules), gitlink(PATH, &one)]);
        let feat = fx.commit_from(TARGET, init, "Feat lib", &[gitlink(PATH, &lib_feat)]);
        let main = fx.commit("main", "Main lib", &[gitlink(PATH, &lib_main)]);
        fx.config(&format!("[submodule \"{PATH}\"]\n\tactive = true\n\turl = ../lib.git\n"));
        fx.git_after_stops(&["merge", TARGET]);
        let ids = fx.build(dir);
        let checkout = dir.join(PATH);
        let _ = fs::remove_dir(&checkout);
        fs::rename(&lib_dir, &checkout).unwrap();
        Setup {
            main: ids[main].into(),
            feat: ids[feat].into(),
            lib_one: one,
            lib_main,
            lib_feat,
        }
    }

    /// The merge stopped on the pointer only; the submodule checked out at `main`'s
    /// commit. Two git processes.
    pub fn expect_stopped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-21 the stopped merge");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.eq("conflicted", conflicted(&e.status()), vec![PATH.to_string()]);
        k.eq("submodule HEAD", Expect::new(&dir.join(PATH)).ref_id("HEAD"), Some(s.lib_main.clone()));
        k.done()
    }

    /// After taking theirs and Continue: the merge commit records `feat`'s commit; the
    /// submodule's checkout is still at `main`'s, so the superproject shows it changed.
    /// Four git processes.
    pub fn expect_concluded(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("MG-21 after Take Theirs and Continue");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.feat], MERGE);
        k.eq("recorded pointer", pointer(dir, &format!("HEAD:{PATH}")), Some(s.lib_feat.clone()));
        k.eq("submodule HEAD", Expect::new(&dir.join(PATH)).ref_id("HEAD"), Some(s.lib_main.clone()));
        k.eq("status", e.status_lines(), vec![format!(" M {PATH}")]);
        k.done()
    }
}

/// **RN-05 — a merge across a rename.** `feat` renamed `a.txt` to [`NEW`]; `main` changed
/// line 9 of `a.txt` ([`MAIN_EDIT`]). The user merges `feat`: no conflict — git follows
/// the rename — and the merge has `main`'s edit in the new name, no `a.txt`
/// ([`expect_merged`]).
pub mod across_rename {
    use std::path::Path;

    use super::{MERGE, merge_commit, on};
    use crate::cp::lines;
    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, del, put};

    pub const TARGET: &str = "feat";
    pub const NEW: &str = "b.txt";
    pub const MAIN_EDIT: &str = "a 9 edited on main";

    /// `a.txt` (ten lines), line 9 edited or not.
    pub fn text(edited: bool) -> String {
        lines("a", 10, 9, edited.then_some(MAIN_EDIT))
    }

    pub struct Setup {
        pub main: String,
        pub feat: String,
    }

    /// See the module docs; HEAD on `main`, clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", &text(false))]);
        let feat = fx.commit_from(TARGET, init, "Rename a to b", &[del("a.txt"), put(NEW, &text(false))]);
        let main = fx.commit("main", "Edit a", &[put("a.txt", &text(true))]);
        let ids = fx.build(dir);
        Setup { main: ids[main].into(), feat: ids[feat].into() }
    }

    /// The merge commit: [`NEW`] with `main`'s edit (committed and on disk), no `a.txt`,
    /// clean. Three git processes.
    pub fn expect_merged(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RN-05 after the merge");
        on(&mut k, &e, "main");
        merge_commit(&mut k, &e, &[&s.main, &s.feat], MERGE);
        let b = e.blobs(&[&format!("HEAD:{NEW}"), "HEAD:a.txt"]);
        k.text("committed b.txt", b[0].as_deref(), Some(&text(true)));
        k.text("committed a.txt", b[1].as_deref(), None);
        k.text("b.txt on disk", e.file(NEW).as_deref(), Some(&text(true)));
        k.ok("a.txt gone", e.file("a.txt").is_none(), "a.txt");
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}
