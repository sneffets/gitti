//! CP — cherry-pick, and what happens when the picked commit's branch is merged later.

use crate::fixture::{Change, Fixture, Mark, put};

/// The author of the commits on `feat` (a cherry-pick keeps the author; the committer
/// becomes the user).
pub const ADA: &str = "Ada <ada@example.com>";
/// The commit that gets picked.
pub const FIX: &str = "Fix a";
/// `feat`'s second commit (not picked).
pub const CHANGE_B: &str = "Change b";
/// `main`'s own commit after `init`.
pub const CHANGE_C: &str = "Change c";
/// The merge commit's message (git's default for `git merge feat`).
pub const MERGE: &str = "Merge branch 'feat'";

/// A file of `n` numbered lines `p N`, line `at` replaced by `with`.
pub fn lines(p: &str, n: usize, at: usize, with: Option<&str>) -> String {
    (1..=n)
        .map(|i| match with {
            Some(w) if i == at => format!("{w}\n"),
            _ => format!("{p} {i}\n"),
        })
        .collect()
}

/// `a.txt` with [`FIX`]'s change (line 2) or `None` = as in `init`.
pub fn a(line2: Option<&str>) -> String {
    lines("a", 10, 2, line2)
}

/// `b.txt`, changed by [`CHANGE_B`] (line 5).
pub fn b(changed: bool) -> String {
    lines("b", 10, 5, changed.then_some("b 5 changed"))
}

/// `c.txt`, changed by [`CHANGE_C`] (line 8).
pub fn c(changed: bool) -> String {
    lines("c", 10, 8, changed.then_some("c 8 changed"))
}

/// What [`FIX`] makes of line 2 of `a.txt`.
pub const FIXED: &str = "a 2 fixed";

/// The history both CP workflows start from: `init` (`a.txt`, `b.txt`, `c.txt`) on
/// `main`; `feat` from `init`: [`FIX`] then [`CHANGE_B`] by [`ADA`]; `main`:
/// [`CHANGE_C`]. Returns the fixture (HEAD on `main`, clean) and the marks of `init`,
/// [`FIX`], [`CHANGE_B`], [`CHANGE_C`].
fn base() -> (Fixture, [Mark; 4]) {
    let mut fx = Fixture::new();
    let init = fx.commit(
        "main",
        "init",
        &[put("a.txt", &a(None)), put("b.txt", &b(false)), put("c.txt", &c(false))],
    );
    fx.author(ADA);
    let fix = fx.commit_from("feat", init, FIX, &[put("a.txt", &a(Some(FIXED)))]);
    let change_b = fx.commit("feat", CHANGE_B, &[put("b.txt", &b(true))]);
    fx.author(crate::fixture::TESTER);
    let change_c = fx.commit("main", CHANGE_C, &[put("c.txt", &c(true))]);
    (fx, [init, fix, change_b, change_c])
}

/// **CP-12 — pick, then merge the source branch.** `main` and `feat` diverged from
/// `init` (see [`fixture`]). The user cherry-picks [`FIX`] from `feat` onto `main`, and
/// later merges `feat` into `main`.
///
/// Expected: the pick is a new commit on `main` with [`FIX`]'s change, message and author
/// ([`expect_picked`]); the merge then goes through **without a conflict** — git's
/// three-way merge sees the same change on both sides — and makes a merge commit whose
/// tree has the fix once, [`CHANGE_B`] and [`CHANGE_C`]; both the original and the copy
/// are in the history ([`expect`]).
pub mod pick_then_merge {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect, OpState};

    /// The commit ids the steps need.
    pub struct Setup {
        /// [`FIX`] on `feat`: the commit to cherry-pick.
        pub fix: String,
        /// `feat`'s tip ([`CHANGE_B`]).
        pub feat: String,
        /// `main`'s tip before the pick ([`CHANGE_C`]).
        pub main: String,
    }

    /// The history of [`super::base`], HEAD on `main`, clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let (fx, [_, fix, change_b, change_c]) = base();
        let ids = fx.build(dir);
        Setup { fix: ids[fix].into(), feat: ids[change_b].into(), main: ids[change_c].into() }
    }

    /// After the pick: `main` is a copy of [`FIX`] (other id, same message and author, the
    /// user as committer) on [`CHANGE_C`]; its tree has the fix; clean. Three git
    /// processes.
    pub fn expect_picked(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-12 after the cherry-pick");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        let log = e.log("HEAD", 1);
        match log.first() {
            Some(head) => {
                k.eq("message", head.message.as_str(), FIX);
                k.eq("author", head.author.as_str(), ADA);
                k.eq("committer", head.committer.as_str(), crate::fixture::TESTER);
                k.eq("parents", head.parents.clone(), vec![s.main.clone()]);
                k.ok("a copy, not the original", head.id != s.fix, &head.id);
            }
            None => k.fail("no HEAD commit".into()),
        }
        let blobs = e.blobs(&["HEAD:a.txt", "HEAD:b.txt", "HEAD:c.txt"]);
        k.text("a.txt", blobs[0].as_deref(), Some(&a(Some(FIXED))));
        k.text("b.txt", blobs[1].as_deref(), Some(&b(false)));
        k.text("c.txt", blobs[2].as_deref(), Some(&c(true)));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }

    /// After the merge: no operation in progress, no conflict; HEAD a merge commit
    /// ([`MERGE`]) of the pick and `feat`; the tree has the fix (once), [`CHANGE_B`] and
    /// [`CHANGE_C`]; [`FIX`] twice in the history (original and copy); clean. Three git
    /// processes.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-12 after the merge");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        let log = e.log("HEAD", 10);
        match log.first() {
            Some(head) => {
                k.eq("merge message", head.message.as_str(), MERGE);
                k.eq("second parent", head.parents.get(1), Some(&s.feat));
                let first = log.iter().find(|l| Some(&l.id) == head.parents.first());
                k.eq("first parent", first.map(|l| l.subject()), Some(FIX));
                k.ok("the copy is not the original", head.parents.first() != Some(&s.fix), &head.parents);
            }
            None => k.fail("no HEAD commit".into()),
        }
        let fixes = log.iter().filter(|l| l.subject() == FIX).count();
        k.eq("FIX in the history (original and copy)", fixes, 2);
        k.ok("the original is in the history", log.iter().any(|l| l.id == s.fix), &log);
        let blobs = e.blobs(&["HEAD:a.txt", "HEAD:b.txt", "HEAD:c.txt"]);
        k.text("a.txt", blobs[0].as_deref(), Some(&a(Some(FIXED))));
        k.text("b.txt", blobs[1].as_deref(), Some(&b(true)));
        k.text("c.txt", blobs[2].as_deref(), Some(&c(true)));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **CP-13 — pick, change the same lines on `main`, merge the source.** As
/// [`pick_then_merge`], but after the pick `main` changed the picked line again
/// ([`EDIT`]: [`FIXED_BETTER`]). Both steps up to there are in the fixture (the pick written
/// as a copy by fast-import); the user merges `feat`.
///
/// Expected: a conflict exactly on that line of `a.txt` — git can't know that `main`'s
/// edit supersedes the fix — with `main`'s line against `feat`'s; [`CHANGE_B`] merged
/// and staged; the merge in progress ([`expect`]).
pub mod pick_edit_then_merge {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect, OpState, conflicted};

    /// `main`'s commit that changes the picked line again.
    pub const EDIT: &str = "Fix a better";
    /// What [`EDIT`] makes of line 2 of `a.txt`.
    pub const FIXED_BETTER: &str = "a 2 fixed better";

    /// The ids the steps need.
    pub struct Setup {
        /// `feat`'s tip ([`CHANGE_B`]).
        pub feat: String,
        /// `main`'s tip ([`EDIT`]).
        pub main: String,
    }

    /// [`super::base`] plus, on `main`, the copy of [`FIX`] (as a cherry-pick writes it:
    /// same change, message and author) and [`EDIT`]. HEAD on `main`, clean. Three git
    /// processes.
    pub fn fixture(dir: &Path) -> Setup {
        let (mut fx, [_, _, change_b, _]) = base();
        fx.author(ADA);
        let fixed: Vec<Change> = vec![put("a.txt", &a(Some(FIXED)))];
        fx.commit("main", FIX, &fixed);
        fx.author(crate::fixture::TESTER);
        let edit = fx.commit("main", EDIT, &[put("a.txt", &a(Some(FIXED_BETTER)))]);
        let ids = fx.build(dir);
        Setup { feat: ids[change_b].into(), main: ids[edit].into() }
    }

    /// `a.txt` as the stopped merge leaves it: `main`'s line against `feat`'s in git's
    /// default markers (the fixture pins `merge.conflictStyle = merge`).
    pub fn conflicted_a() -> String {
        let marked = format!("<<<<<<< HEAD\n{FIXED_BETTER}\n=======\n{FIXED}\n>>>>>>> feat");
        lines("a", 10, 2, Some(&marked))
    }

    /// After the merge stopped: merging, `MERGE_HEAD` = `feat`, HEAD unchanged; only
    /// `a.txt` conflicted, its markers exactly around line 2; `b.txt` merged and staged;
    /// `c.txt` as on `main`. Two git processes.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-13 after the merge stopped");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.eq("HEAD", e.ref_id("HEAD"), Some(s.main.clone()));
        let status = e.status();
        k.eq("conflicted", conflicted(&status), vec!["a.txt".to_string()]);
        let mut lines: Vec<String> = status.iter().map(|s| format!("{} {}", s.xy, s.path)).collect();
        lines.sort();
        k.eq("status", lines, vec!["M  b.txt".to_string(), "UU a.txt".to_string()]);
        k.text("a.txt on disk", e.file("a.txt").as_deref(), Some(&conflicted_a()));
        let blobs = e.blobs(&[":b.txt", ":2:a.txt", ":3:a.txt"]);
        k.text("b.txt staged", blobs[0].as_deref(), Some(&b(true)));
        k.text("ours a.txt", blobs[1].as_deref(), Some(&a(Some(FIXED_BETTER))));
        k.text("theirs a.txt", blobs[2].as_deref(), Some(&a(Some(FIXED))));
        k.text("c.txt", e.file("c.txt").as_deref(), Some(&c(true)));
        k.done()
    }
}
