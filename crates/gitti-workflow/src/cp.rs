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

/// The history the CP-12..CP-15 workflows start from: `init` (`a.txt`, `b.txt`, `c.txt`) on
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

/// **CP-14 — pick with a different resolution, merge the source.** `main` changed line 2
/// of `a.txt` itself ([`MAIN_A`]: [`A2_MAIN`]); the pick of [`FIX`] conflicted there and
/// was resolved to a third text ([`A2_RESOLVED`]) — both in the fixture, the pick written
/// as the commit git makes of it (same message and author). The user merges `feat`.
///
/// Expected: git can't know the copy stands for [`FIX`] — the merge stops with a conflict
/// on that line again (the resolution against `feat`'s line), [`CHANGE_B`] merged and
/// staged ([`expect_stopped`]); keeping the resolution (Take Ours) and Continue make the
/// merge commit with it ([`expect_merged`]).
pub mod pick_resolved_then_merge {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect, OpState, conflicted};

    /// `main`'s own change of the picked line, before the pick.
    pub const MAIN_A: &str = "Change a";
    pub const A2_MAIN: &str = "a 2 main";
    /// How the pick's conflict was resolved.
    pub const A2_RESOLVED: &str = "a 2 fixed on main";

    /// The ids the steps need.
    pub struct Setup {
        /// `feat`'s tip ([`CHANGE_B`]).
        pub feat: String,
        /// `main`'s tip: the resolved pick of [`FIX`].
        pub main: String,
    }

    /// [`super::base`] plus, on `main`, [`MAIN_A`] and the resolved pick of [`FIX`]. HEAD
    /// on `main`, clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        build(dir, false, false)
    }

    /// [`fixture`] with `git merge feat` run last: stopped on `a.txt`. One more git
    /// process.
    pub fn fixture_stopped(dir: &Path) -> Setup {
        build(dir, true, false)
    }

    /// [`fixture_stopped`] with the resolution kept (`checkout --ours`, `add`): where
    /// Continue starts. Two more git processes.
    pub fn fixture_resolved(dir: &Path) -> Setup {
        build(dir, true, true)
    }

    fn build(dir: &Path, stopped: bool, resolved: bool) -> Setup {
        let (mut fx, [_, _, change_b, _]) = base();
        fx.commit("main", MAIN_A, &[put("a.txt", &a(Some(A2_MAIN)))]);
        fx.author(ADA);
        let pick = fx.commit("main", FIX, &[put("a.txt", &a(Some(A2_RESOLVED)))]);
        fx.author(crate::fixture::TESTER);
        if stopped {
            fx.git_after_stops(&["merge", "feat"]);
        }
        if resolved {
            fx.git_after(&["checkout", "--ours", "--", "a.txt"]);
            fx.git_after(&["add", "a.txt"]);
        }
        let ids = fx.build(dir);
        Setup { feat: ids[change_b].into(), main: ids[pick].into() }
    }

    /// `a.txt` as the stopped merge leaves it: the resolution against `feat`'s line.
    pub fn conflicted_a() -> String {
        let marked = format!("<<<<<<< HEAD\n{A2_RESOLVED}\n=======\n{FIXED}\n>>>>>>> feat");
        lines("a", 10, 2, Some(&marked))
    }

    /// After the merge stopped: merging `feat`, HEAD unchanged; only `a.txt` conflicted,
    /// exactly around line 2; `b.txt` merged and staged. Two git processes.
    pub fn expect_stopped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-14 after the merge stopped");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        k.eq("HEAD", e.ref_id("HEAD"), Some(s.main.clone()));
        let status = e.status();
        k.eq("conflicted", conflicted(&status), vec!["a.txt".to_string()]);
        let mut lines: Vec<String> = status.iter().map(|s| format!("{} {}", s.xy, s.path)).collect();
        lines.sort();
        k.eq("status", lines, vec!["M  b.txt".to_string(), "UU a.txt".to_string()]);
        k.text("a.txt on disk", e.file("a.txt").as_deref(), Some(&conflicted_a()));
        k.text("b.txt staged", e.index("b.txt").as_deref(), Some(&b(true)));
        k.done()
    }

    /// After keeping the resolution and Continue: a merge commit ([`MERGE`]) of the pick
    /// and `feat` with the resolved line, [`CHANGE_B`] and [`CHANGE_C`]; clean. Three git
    /// processes.
    pub fn expect_merged(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-14 after the merge");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        match e.log("HEAD", 1).first() {
            Some(head) => {
                k.eq("merge message", head.message.as_str(), MERGE);
                k.eq("parents", head.parents.clone(), vec![s.main.clone(), s.feat.clone()]);
            }
            None => k.fail("no HEAD commit".into()),
        }
        let blobs = e.blobs(&["HEAD:a.txt", "HEAD:b.txt", "HEAD:c.txt"]);
        k.text("a.txt", blobs[0].as_deref(), Some(&a(Some(A2_RESOLVED))));
        k.text("b.txt", blobs[1].as_deref(), Some(&b(true)));
        k.text("c.txt", blobs[2].as_deref(), Some(&c(true)));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **CP-15 — pick, revert the pick, merge the source.** [`FIX`] was picked onto `main`
/// and the pick reverted there ([`REVERT`]) — both in the fixture as git writes them. The
/// user merges `feat`.
///
/// Expected: git's three-way merge sees `main` back at the merge base's line 2 and
/// `feat` with the fix — **the fix comes back** with the merge, without a conflict: a
/// merge commit whose tree has [`FIXED`], [`CHANGE_B`] and [`CHANGE_C`]; its diff against
/// `main` shows the fix ([`expect`]).
pub mod pick_revert_then_merge {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect, OpState};

    /// The revert of the pick (git's message for it).
    pub const REVERT: &str = "Revert \"Fix a\"";

    /// The ids the steps need.
    pub struct Setup {
        pub feat: String,
        /// `main`'s tip: [`REVERT`].
        pub main: String,
    }

    /// [`super::base`] plus, on `main`, the pick of [`FIX`] and its revert. HEAD on
    /// `main`, clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let (mut fx, [_, _, change_b, _]) = base();
        fx.author(ADA);
        fx.commit("main", FIX, &[put("a.txt", &a(Some(FIXED)))]);
        fx.author(crate::fixture::TESTER);
        let revert = fx.commit("main", REVERT, &[put("a.txt", &a(None))]);
        let ids = fx.build(dir);
        Setup { feat: ids[change_b].into(), main: ids[revert].into() }
    }

    /// After the merge: no conflict, a merge commit ([`MERGE`]) of [`REVERT`] and `feat`;
    /// its tree has the fix back, [`CHANGE_B`] and [`CHANGE_C`]; against `main` it
    /// changes `a.txt` (the fix) and `b.txt`; clean. Four git processes.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-15 after the merge");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        match e.log("HEAD", 1).first() {
            Some(head) => {
                k.eq("merge message", head.message.as_str(), MERGE);
                k.eq("parents", head.parents.clone(), vec![s.main.clone(), s.feat.clone()]);
            }
            None => k.fail("no HEAD commit".into()),
        }
        let blobs = e.blobs(&["HEAD:a.txt", "HEAD:b.txt", "HEAD:c.txt"]);
        k.text("a.txt: the fix is back", blobs[0].as_deref(), Some(&a(Some(FIXED))));
        k.text("b.txt", blobs[1].as_deref(), Some(&b(true)));
        k.text("c.txt", blobs[2].as_deref(), Some(&c(true)));
        let changed = crate::git::git(dir, &["diff", "--name-only", &s.main, "HEAD"]);
        k.eq("the merge's diff against main", changed.lines().collect::<Vec<_>>(), vec!["a.txt", "b.txt"]);
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **P9 — cherry-pick and revert, one history for all their workflows** (CP-01, CP-02,
/// CP-04..CP-11, CP-16..CP-18 here; RV-01..RV-06 in [`crate::rv`]). Five files of ten
/// numbered lines (`a.txt` … `e.txt`, [`file`]); every commit changes one line.
///
/// - `main`: `init` → [`MAIN_C`] (c:8) → [`MERGE_TOPIC`] (a merge of `topic`: [`TOPIC`],
///   d:9) → [`MAIN_C_AGAIN`] (c:8 again) → [`MAIN_A`] (a:5) → [`MAIN_A_BACK`] (a:5 back).
///   HEAD on `main`, clean.
/// - `feat` (by [`ADA`]): [`FIX`] (a:2), [`CHANGE_B`] (b:5), [`CHANGE_E`] (e:3) — clean
///   picks onto `main`, alone or together (CP-01, CP-02, CP-08, CP-09, CP-16).
/// - `clash` (by [`ADA`]): [`CLASH_A`] (a:9), [`CLASH_C`] (c:8 — conflicts with `main`'s
///   c:8), [`CLASH_E`] (e:6) — the conflicting pick (CP-04, CP-05, CP-10, CP-11) and the
///   sequence that stops at its second commit (CP-07).
/// - `side`: [`SIDE_B`] (b:1), then [`MERGE_SIDE`], a merge of `topic2` ([`TOPIC2`], d:1)
///   — picked against its first parent it brings d:1 only (CP-06).
/// - A commit no branch has, only HEAD's reflog ([`LOST`], new file `l.txt`) (CP-18).
pub mod picks {
    use std::fmt::Write as _;
    use std::path::Path;

    use super::ADA;
    pub use super::{CHANGE_B, FIX};
    use crate::expect::{Checks, Expect, OpState, conflicted};
    use crate::fixture::{Fixture, Mark, TESTER, put};

    pub const MAIN_C: &str = "Change c";
    pub const TOPIC: &str = "Change d";
    pub const MERGE_TOPIC: &str = "Merge branch 'topic'";
    pub const MAIN_C_AGAIN: &str = "Change c again";
    pub const MAIN_A: &str = "Change a";
    pub const MAIN_A_BACK: &str = "Put a back";
    pub const CHANGE_E: &str = "Change e";
    pub const CLASH_A: &str = "Clash a";
    pub const CLASH_C: &str = "Clash c";
    pub const CLASH_E: &str = "Clash e";
    pub const SIDE_B: &str = "Side b";
    pub const TOPIC2: &str = "Change d top";
    pub const MERGE_SIDE: &str = "Merge branch 'topic2' into side";
    pub const LOST: &str = "Lost work";
    /// The lost commit's new file and its text.
    pub const LOST_FILE: &str = "l.txt";
    pub const LOST_TEXT: &str = "lost\n";

    /// `p.txt`'s ten lines `p N` with `edits` (line number, new text).
    pub fn file(p: &str, edits: &[(usize, &str)]) -> String {
        (1..=10).fold(String::new(), |mut s, i| {
            match edits.iter().find(|(at, _)| *at == i) {
                Some((_, w)) => writeln!(s, "{w}").unwrap(),
                None => writeln!(s, "{p} {i}").unwrap(),
            }
            s
        })
    }

    pub const FIXED_A2: &str = super::FIXED;
    pub const B5: &str = "b 5 changed";
    pub const E3: &str = "e 3 changed";
    pub const A9: &str = "a 9 clash";
    pub const C8_CLASH: &str = "c 8 clash";
    pub const E6: &str = "e 6 clash";
    pub const C8: &str = "c 8 changed";
    pub const C8_AGAIN: &str = "c 8 changed again";
    pub const D9: &str = "d 9 topic";
    pub const D1: &str = "d 1 topic2";
    pub const A5: &str = "a 5 main";
    pub const B1: &str = "b 1 side";

    /// The commit ids the steps need (full hex).
    pub struct Setup {
        /// `main`'s tip ([`MAIN_A_BACK`]).
        pub main: String,
        pub main_c: String,
        pub merge_topic: String,
        pub main_c_again: String,
        pub main_a: String,
        pub fix: String,
        pub change_b: String,
        pub change_e: String,
        pub clash_a: String,
        pub clash_c: String,
        pub clash_e: String,
        pub merge_side: String,
        pub lost: String,
        /// [`fixture_fix_picked`]: the copy of [`FIX`] on `main`'s tip.
        pub copy: Option<String>,
    }

    /// The files at `main`'s tip: c:8 [`C8_AGAIN`], d:9 [`D9`], the rest as in `init`.
    pub fn main_files() -> [(&'static str, String); 5] {
        [
            ("a.txt", file("a", &[])),
            ("b.txt", file("b", &[])),
            ("c.txt", file("c", &[(8, C8_AGAIN)])),
            ("d.txt", file("d", &[(9, D9)])),
            ("e.txt", file("e", &[])),
        ]
    }

    /// [`main_files`] with `edits` on top (path, line, text).
    pub fn files_with(edits: &[(&str, usize, &str)]) -> Vec<(&'static str, String)> {
        let mut files = main_files().to_vec();
        for (path, f) in files.iter_mut() {
            let p = &path[..1];
            let mut lines: Vec<(usize, &str)> = match *path {
                "c.txt" => vec![(8, C8_AGAIN)],
                "d.txt" => vec![(9, D9)],
                _ => Vec::new(),
            };
            for (_, at, text) in edits.iter().filter(|(e, ..)| e == path) {
                lines.retain(|(l, _)| l != at);
                lines.push((*at, text));
            }
            *f = file(p, &lines);
        }
        files
    }

    /// The history above, HEAD on `main`, clean. Three git processes (the reflog is
    /// written as a file).
    pub fn fixture(dir: &Path) -> Setup {
        build(dir, &Opts::default())
    }

    /// [`fixture`] with `config` appended to `.git/config` (CP-11: `core.commentChar`).
    pub fn fixture_with_config(dir: &Path, config: &str) -> Setup {
        build(dir, &Opts { config, ..Opts::default() })
    }

    /// [`fixture`] with `path` changed in the working tree (not staged) to `text`
    /// (CP-16).
    pub fn fixture_dirty(dir: &Path, path: &str, text: &str) -> Setup {
        build(dir, &Opts { dirty: Some((path, text)), ..Opts::default() })
    }

    /// [`fixture`] with [`FIX`] picked onto `main` already ([`Setup::copy`], written by
    /// fast-import as a cherry-pick writes it): where CP-09's second pick starts. The
    /// commit list's rows below the copy move down by one.
    pub fn fixture_fix_picked(dir: &Path) -> Setup {
        build(dir, &Opts { fix_picked: true, ..Opts::default() })
    }

    /// [`fixture`] with `config`, then `git <args>` run last, which stops (a conflicting
    /// cherry-pick / revert: its state and index stages as only git writes them). One
    /// more git process. Revisions by branch: [`CLASH_C`] is `clash~1`, the `clash`
    /// sequence `clash~2 clash~1 clash`, [`MAIN_C`] `main~4`.
    pub fn fixture_stopped(dir: &Path, config: &str, args: &[&str]) -> Setup {
        build(dir, &Opts { config, stops: args, ..Opts::default() })
    }

    /// [`fixture_stopped`] with `c.txt` — the conflicted file of every stop here —
    /// resolved by git (`checkout --ours` / `--theirs`, `add`): where Continue starts.
    /// Two more git processes.
    pub fn fixture_resolved(dir: &Path, config: &str, args: &[&str], ours: bool) -> Setup {
        build(dir, &Opts { config, stops: args, resolve: Some(ours), ..Opts::default() })
    }

    #[derive(Default)]
    struct Opts<'a> {
        config: &'a str,
        dirty: Option<(&'a str, &'a str)>,
        fix_picked: bool,
        stops: &'a [&'a str],
        resolve: Option<bool>,
    }

    fn build(dir: &Path, o: &Opts) -> Setup {
        let mut fx = Fixture::new();
        let init_files: Vec<_> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|p| (format!("{p}.txt"), file(p, &[])))
            .collect();
        let changes: Vec<_> = init_files.iter().map(|(p, t)| put(p, t)).collect();
        let init = fx.commit("main", "init", &changes);
        fx.author(ADA);
        let fix = fx.commit_from("feat", init, FIX, &[put("a.txt", &file("a", &[(2, FIXED_A2)]))]);
        let change_b = fx.commit("feat", CHANGE_B, &[put("b.txt", &file("b", &[(5, B5)]))]);
        let change_e = fx.commit("feat", CHANGE_E, &[put("e.txt", &file("e", &[(3, E3)]))]);
        let clash_a = fx.commit_from("clash", init, CLASH_A, &[put("a.txt", &file("a", &[(9, A9)]))]);
        let clash_c = fx.commit("clash", CLASH_C, &[put("c.txt", &file("c", &[(8, C8_CLASH)]))]);
        let clash_e = fx.commit("clash", CLASH_E, &[put("e.txt", &file("e", &[(6, E6)]))]);
        fx.author(TESTER);
        let topic = fx.commit_from("topic", init, TOPIC, &[put("d.txt", &file("d", &[(9, D9)]))]);
        let side_b = fx.commit_from("side", init, SIDE_B, &[put("b.txt", &file("b", &[(1, B1)]))]);
        let topic2 = fx.commit_from("topic2", init, TOPIC2, &[put("d.txt", &file("d", &[(1, D1)]))]);
        let merge_side = fx.merge("side", &[side_b, topic2], MERGE_SIDE, &[put("d.txt", &file("d", &[(1, D1)]))]);
        let main_c = fx.commit("main", MAIN_C, &[put("c.txt", &file("c", &[(8, C8)]))]);
        let merge_topic = fx.merge("main", &[main_c, topic], MERGE_TOPIC, &[put("d.txt", &file("d", &[(9, D9)]))]);
        let main_c_again = fx.commit("main", MAIN_C_AGAIN, &[put("c.txt", &file("c", &[(8, C8_AGAIN)]))]);
        let main_a = fx.commit("main", MAIN_A, &[put("a.txt", &file("a", &[(5, A5)]))]);
        let main_tip = fx.commit("main", MAIN_A_BACK, &[put("a.txt", &file("a", &[]))]);
        let lost = fx.commit_from("lost", main_tip, LOST, &[put(LOST_FILE, LOST_TEXT)]);
        fx.forget("lost");
        let copy = o.fix_picked.then(|| {
            fx.author(ADA);
            let copy = fx.commit("main", FIX, &[put("a.txt", &file("a", &[(2, FIXED_A2)]))]);
            fx.author(TESTER);
            copy
        });
        if !o.config.is_empty() {
            fx.config(o.config);
        }
        if let Some((path, text)) = o.dirty {
            fx.write(path, text);
        }
        if !o.stops.is_empty() {
            fx.git_after_stops(o.stops);
        }
        if let Some(ours) = o.resolve {
            fx.git_after(&["checkout", if ours { "--ours" } else { "--theirs" }, "--", "c.txt"]);
            fx.git_after(&["add", "c.txt"]);
        }
        let ids = fx.build(dir);
        let id = |m: Mark| ids[m].to_string();
        // HEAD's reflog: `lost` was checked out once, then `main` again (the branch
        // since deleted) — written as git writes it.
        let line = |old: &str, new: &str, t: u64, what: &str| {
            format!("{old} {new} {TESTER} {t} +0000\tcheckout: moving from {what}\n")
        };
        let t = crate::fixture::START_TIME + 3600;
        let log = format!(
            "{}{}",
            line(&id(main_tip), &id(lost), t, "main to lost"),
            line(&id(lost), &id(main_tip), t + 60, "lost to main")
        );
        // (Not under a stopped operation's own entries.)
        if o.stops.is_empty() {
            let logs = dir.join(".git").join("logs");
            std::fs::create_dir_all(&logs).unwrap();
            std::fs::write(logs.join("HEAD"), log).unwrap();
        }
        Setup {
            main: id(main_tip),
            main_c: id(main_c),
            merge_topic: id(merge_topic),
            main_c_again: id(main_c_again),
            main_a: id(main_a),
            fix: id(fix),
            change_b: id(change_b),
            change_e: id(change_e),
            clash_a: id(clash_a),
            clash_c: id(clash_c),
            clash_e: id(clash_e),
            merge_side: id(merge_side),
            lost: id(lost),
            copy: copy.map(id),
        }
    }

    /// The entry of HEAD's reflog that moved to [`LOST`] (newest first, as the reflog
    /// views list them). No git.
    pub fn lost_entry(dir: &Path, s: &Setup) -> Option<usize> {
        Expect::new(dir).reflog("HEAD").iter().position(|(_, new, _)| *new == s.lost)
    }

    /// The rows of the commit list (row 0: the working directory; newest first): which
    /// commit each workflow acts on. Fixed by the fixture's fixed ids and times.
    pub mod row {
        pub const MAIN: usize = 1;
        pub const MAIN_A: usize = 2;
        pub const MAIN_C_AGAIN: usize = 3;
        pub const MERGE_TOPIC: usize = 4;
        pub const MAIN_C: usize = 6;
        pub const MERGE_SIDE: usize = 7;
        pub const CLASH_E: usize = 10;
        pub const CLASH_C: usize = 11;
        pub const CLASH_A: usize = 12;
        pub const CHANGE_E: usize = 13;
        pub const CHANGE_B: usize = 14;
        pub const FIX: usize = 15;
        /// The last row (`init`); [`super::fixture_fix_picked`] has one more row above.
        pub const INIT: usize = 16;
    }

    /// Checks the files at HEAD and in the working tree against `files`, and that nothing
    /// is changed or in progress. Two git processes.
    fn clean_with(k: &mut Checks, e: &Expect, files: &[(&str, String)]) {
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        let specs: Vec<String> = files.iter().map(|(p, _)| format!("HEAD:{p}")).collect();
        let specs: Vec<&str> = specs.iter().map(String::as_str).collect();
        for ((p, want), got) in files.iter().zip(e.blobs(&specs)) {
            k.text(&format!("HEAD:{p}"), got.as_deref(), Some(want.as_str()));
        }
        k.eq("status", e.status_lines(), Vec::<String>::new());
    }

    /// `main` unchanged: HEAD at its tip, nothing in progress, the files as they were
    /// (an Abort, a refused or skipped pick / revert). Three git processes.
    pub fn expect_unchanged(dir: &Path, s: &Setup, what: &str) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new(what);
        k.eq("HEAD", e.ref_id("HEAD"), Some(s.main.clone()));
        clean_with(&mut k, &e, &main_files());
        k.done()
    }

    /// After picks: HEAD's newest commits are copies of `picked` (id, message), in that
    /// order (oldest first), on `main`'s old tip, each by [`ADA`] with the user as
    /// committer; `record_origin`: each message ends with git's "(cherry picked from
    /// commit …)". The files are [`files_with`] `edits`; nothing in progress. Three git
    /// processes.
    pub fn expect_picked(
        dir: &Path,
        s: &Setup,
        picked: &[(&str, &str)],
        record_origin: bool,
        edits: &[(&str, usize, &str)],
        what: &str,
    ) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new(what);
        let log = e.log("HEAD", picked.len() + 1);
        let new: Vec<_> = log.iter().take(picked.len()).rev().collect();
        k.eq("commits", new.len(), picked.len());
        let mut parent = s.main.clone();
        for (c, (id, msg)) in new.iter().zip(picked) {
            let want = if record_origin {
                format!("{msg}\n\n(cherry picked from commit {id})")
            } else {
                msg.to_string()
            };
            k.eq("message", c.message.as_str(), want.as_str());
            k.eq("author", c.author.as_str(), ADA);
            k.eq("committer", c.committer.as_str(), TESTER);
            k.eq("parents", c.parents.clone(), vec![parent.clone()]);
            k.ok("a copy, not the original", c.id != *id, &c.id);
            parent = c.id.clone();
        }
        clean_with(&mut k, &e, &files_with(edits));
        k.done()
    }

    /// Stopped in `state` with `paths` conflicted, HEAD at `head`. One git process.
    pub fn expect_stopped(dir: &Path, head: &str, state: OpState, paths: &[&str], what: &str) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new(what);
        k.eq("state", e.state(), state);
        k.eq("HEAD", e.ref_id("HEAD"), Some(head.to_string()));
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        k.eq("conflicted", conflicted(&e.status()), paths);
        k.done()
    }

    /// CP-09 from [`fixture_fix_picked`]: the second pick of [`FIX`] skipped — HEAD still
    /// the one copy, nothing in progress, a:2 [`FIXED_A2`]. Three git processes.
    pub fn expect_picked_once(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-09 after Skip");
        k.eq("HEAD", e.ref_id("HEAD"), s.copy.clone());
        k.eq("one copy", e.subjects("HEAD", 2), vec![FIX.to_string(), MAIN_A_BACK.to_string()]);
        clean_with(&mut k, &e, &files_with(&[("a.txt", 2, FIXED_A2)]));
        k.done()
    }

    /// CP-06: HEAD is one new commit on `main`'s tip with [`MERGE_SIDE`]'s message — not a
    /// merge — that brings `topic2`'s d:1 only (not `side`'s own b:1). Three git
    /// processes.
    pub fn expect_merge_picked(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-06 after picking a merge commit");
        match e.log("HEAD", 1).first() {
            Some(c) => {
                k.eq("message", c.message.as_str(), MERGE_SIDE);
                k.eq("parents", c.parents.clone(), vec![s.main.clone()]);
            }
            None => k.fail("no HEAD commit".into()),
        }
        clean_with(&mut k, &e, &files_with(&[("d.txt", 1, D1)]));
        k.done()
    }

    /// CP-16: the pick was refused — HEAD and everything as before, `path` still changed
    /// in the working tree (`text`), nothing staged. Two git processes.
    pub fn expect_refused(dir: &Path, s: &Setup, path: &str, text: &str) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-16 after the refused pick");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("HEAD", e.ref_id("HEAD"), Some(s.main.clone()));
        k.text(path, e.file(path).as_deref(), Some(text));
        k.eq("status", e.status_lines(), vec![format!(" M {path}")]);
        k.done()
    }

    /// CP-18: HEAD is a copy of [`LOST`] on `main`'s tip with [`LOST_FILE`]. Three git
    /// processes.
    pub fn expect_lost_picked(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("CP-18 after picking the lost commit");
        match e.log("HEAD", 1).first() {
            Some(c) => {
                k.eq("message", c.message.as_str(), LOST);
                k.eq("parents", c.parents.clone(), vec![s.main.clone()]);
                k.ok("a copy, not the original", c.id != s.lost, &c.id);
            }
            None => k.fail("no HEAD commit".into()),
        }
        let mut files = main_files().to_vec();
        files.push((LOST_FILE, LOST_TEXT.to_string()));
        clean_with(&mut k, &e, &files);
        k.done()
    }
}
