//! RB — rebase the current branch onto another, its stops and what it leaves out; RL-04
//! (undo a rebase from the reflog) starts here too.
//!
//! One history ([`fixture`] and its variants, [`Base`]): three files of ten numbered
//! lines (`a.txt`, `b.txt`, `c.txt`, [`file`]); every commit changes one line.
//!
//! - `feat` (by [`ADA`], checked out): `init` → [`FEAT_A`] (a:2 [`A2_FEAT`]) →
//!   [`FEAT_B`] (b:5 [`B5_FEAT`]).
//! - `main`: `init` → [`MAIN_C`] (c:8 [`C8_MAIN`]), then per [`Base`]: nothing more
//!   ([`Base::Apart`]: a clean rebase), [`MAIN_A`] changing a:2 ([`Base::Clash`]: the
//!   rebase stops at [`FEAT_A`]), [`MAIN_AB`] changing a:2 and b:5 ([`Base::ClashTwice`]:
//!   it stops at both), or the pick of [`FEAT_A`] ([`Base::Picked`]: git leaves
//!   [`FEAT_A`] out).
//!
//! A stopped rebase comes from git itself ([`Opts::stops`]: `git rebase main` run last),
//! its conflict resolved by git when a workflow starts at Continue ([`Opts::resolve`]).

use std::fmt::Write as _;
use std::path::Path;

use crate::expect::{Checks, Expect, OpState, conflicted};
use crate::fixture::{Fixture, TESTER, put};

/// The author of `feat`'s commits; a rebase keeps the author, the committer becomes the
/// user.
pub const ADA: &str = "Ada <ada@example.com>";
pub const FEAT_A: &str = "Feat a";
pub const FEAT_B: &str = "Feat b";
pub const MAIN_C: &str = "Main c";
pub const MAIN_A: &str = "Main a";
pub const MAIN_AB: &str = "Main a and b";

pub const A2_FEAT: &str = "a 2 feat";
pub const B5_FEAT: &str = "b 5 feat";
pub const C8_MAIN: &str = "c 8 main";
pub const A2_MAIN: &str = "a 2 main";
pub const B5_MAIN: &str = "b 5 main";

/// The message typed in the commit box before Continue (RB-02, RB-04, RB-08).
pub const EDITED: &str = "Feat a, resolved on main";

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

/// What `main` has besides [`MAIN_C`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// Nothing: `feat` replays cleanly.
    Apart,
    /// [`MAIN_A`]: a:2 [`A2_MAIN`] — [`FEAT_A`] conflicts on `a.txt`.
    Clash,
    /// [`MAIN_AB`]: a:2 [`A2_MAIN`] and b:5 [`B5_MAIN`] — both of `feat`'s commits
    /// conflict.
    ClashTwice,
    /// The pick of [`FEAT_A`] (same change, message and author).
    Picked,
}

/// Which side a stopped rebase's conflict is resolved to (`git checkout --ours /
/// --theirs`; during a rebase "ours" is the new base).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The new base's line ([`A2_MAIN`]): nothing of the commit is left.
    Base,
    /// The replayed commit's line.
    Commit,
}

/// How the fixture ends.
#[derive(Clone, Copy, Debug, Default)]
pub struct Opts<'a> {
    /// `git rebase main` run last, which stops ([`Base::Clash`], [`Base::ClashTwice`]).
    pub stops: bool,
    /// … with the apply backend (`rebase --apply`).
    pub apply: bool,
    /// The conflicted `a.txt` resolved (and staged) by git.
    pub resolve: Option<Side>,
    /// … then `git rebase --continue`, which stops at the next conflict
    /// ([`Base::ClashTwice`]: `b.txt`).
    pub then_continue: bool,
    /// A tracked file changed in the working tree (path, text).
    pub dirty: Option<(&'a str, &'a str)>,
    /// `git rebase main` run after the reflogs are written, which goes through
    /// ([`Base::Apart`]): where RL-04's undo starts.
    pub rebased: bool,
    /// Appended to `.git/config`.
    pub config: &'a str,
}

/// The ids the steps need (full hex).
#[derive(Clone, Debug)]
pub struct Setup {
    pub init: String,
    /// `main`'s tip.
    pub main: String,
    pub feat_a: String,
    /// `feat`'s tip before the rebase.
    pub feat_b: String,
}

/// The history above for `base`, HEAD on `feat`, clean. Three git processes; HEAD's and
/// the branches' reflogs written as git writes them (when nothing runs after).
pub fn fixture(dir: &Path, base: Base) -> Setup {
    fixture_with(dir, base, Opts::default())
}

/// [`fixture`] ending as `o` says (one git process more per command run last).
/// [`Opts::stops`] and [`Opts::rebased`] exclude each other.
pub fn fixture_with(dir: &Path, base: Base, o: Opts) -> Setup {
    let mut fx = Fixture::new();
    let init = fx.commit(
        "main",
        "init",
        &[put("a.txt", &file("a", &[])), put("b.txt", &file("b", &[])), put("c.txt", &file("c", &[]))],
    );
    fx.author(ADA);
    let feat_a = fx.commit_from("feat", init, FEAT_A, &[put("a.txt", &file("a", &[(2, A2_FEAT)]))]);
    let feat_b = fx.commit("feat", FEAT_B, &[put("b.txt", &file("b", &[(5, B5_FEAT)]))]);
    fx.author(TESTER);
    let main_c = fx.commit("main", MAIN_C, &[put("c.txt", &file("c", &[(8, C8_MAIN)]))]);
    let main = match base {
        Base::Apart => main_c,
        Base::Clash => fx.commit("main", MAIN_A, &[put("a.txt", &file("a", &[(2, A2_MAIN)]))]),
        Base::ClashTwice => fx.commit(
            "main",
            MAIN_AB,
            &[put("a.txt", &file("a", &[(2, A2_MAIN)])), put("b.txt", &file("b", &[(5, B5_MAIN)]))],
        ),
        Base::Picked => {
            fx.author(ADA);
            let pick = fx.commit("main", FEAT_A, &[put("a.txt", &file("a", &[(2, A2_FEAT)]))]);
            fx.author(TESTER);
            pick
        }
    };
    fx.checkout("feat");
    if !o.config.is_empty() {
        fx.config(o.config);
    }
    if let Some((path, text)) = o.dirty {
        fx.write(path, text);
    }
    if o.stops {
        if o.apply {
            fx.git_after_stops(&["rebase", "--apply", "main"]);
        } else {
            fx.git_after_stops(&["rebase", "main"]);
        }
    }
    if let Some(side) = o.resolve {
        fx.git_after(&["checkout", if side == Side::Base { "--ours" } else { "--theirs" }, "--", "a.txt"]);
        fx.git_after(&["add", "a.txt"]);
    }
    if o.then_continue {
        fx.git_after_stops(&["-c", "core.editor=true", "rebase", "--continue"]);
    }
    let ids = fx.build(dir);
    let s = Setup {
        init: ids[init].into(),
        main: ids[main].into(),
        feat_a: ids[feat_a].into(),
        feat_b: ids[feat_b].into(),
    };
    if !o.stops {
        let (i, m, c, a, b) = (s.init.as_str(), s.main.as_str(), &ids[main_c], s.feat_a.as_str(), s.feat_b.as_str());
        let mut main_log = vec![("", i, "commit (initial): init"), (i, c, "commit: Main c")];
        if m != c {
            main_log.push((c, m, "commit: the rest of main"));
        }
        let feat_log = [("", i, "branch: Created from main"), (i, a, "commit: Feat a"), (a, b, "commit: Feat b")];
        let mut head_log = main_log.clone();
        head_log.push((m, i, "checkout: moving from main to feat"));
        head_log.extend_from_slice(&feat_log[1..]);
        crate::rs::write_reflog(dir, "HEAD", &head_log);
        crate::rs::write_reflog(dir, "refs/heads/main", &main_log);
        crate::rs::write_reflog(dir, "refs/heads/feat", &feat_log);
    }
    if o.rebased {
        crate::git::git(dir, &["rebase", "main"]);
    }
    s
}

/// `feat`'s files before the rebase.
pub fn feat_files() -> [(&'static str, String); 3] {
    [("a.txt", file("a", &[(2, A2_FEAT)])), ("b.txt", file("b", &[(5, B5_FEAT)])), ("c.txt", file("c", &[]))]
}

/// Where [`old_tip_entry`] is after a rebase of [`fixture`]`(Base::Apart)` that replayed
/// both of `feat`'s commits: under git's four entries of it (start, two picks, finish).
pub const OLD_TIP_AFTER_REBASE: usize = 4;

/// The entry of HEAD's reflog (newest first, as the reflog views list it) that moved to
/// `feat`'s old tip — what RL-04 resets to. No git.
pub fn old_tip_entry(dir: &Path, s: &Setup) -> Option<usize> {
    Expect::new(dir).reflog("HEAD").iter().position(|(_, new, _)| *new == s.feat_b)
}

/// Checks HEAD's and the working tree's files against `files` and that nothing is
/// changed or in progress, HEAD on `feat`. Two git processes.
fn clean_with(k: &mut Checks, e: &Expect, files: &[(&str, String)], status: &[&str]) {
    k.eq("state", e.state(), OpState::Clean);
    k.eq("branch", e.branch().as_deref(), Some("feat"));
    let specs: Vec<String> = files.iter().map(|(p, _)| format!("HEAD:{p}")).collect();
    let specs: Vec<&str> = specs.iter().map(String::as_str).collect();
    for ((p, want), got) in files.iter().zip(e.blobs(&specs)) {
        k.text(&format!("HEAD:{p}"), got.as_deref(), Some(want.as_str()));
    }
    k.eq("status", e.status_lines(), status.iter().map(|s| s.to_string()).collect::<Vec<_>>());
}

/// `feat` as before the rebase: at its old tip, its files, nothing in progress (an
/// Abort, a refused rebase, the undo from the reflog); `status`: the working tree's
/// status lines (`[]` = clean). Three git processes.
pub fn expect_unchanged(dir: &Path, s: &Setup, status: &[&str], what: &str) -> Result<(), String> {
    let e = Expect::new(dir);
    let mut k = Checks::new(what);
    k.eq("feat", e.ref_id("feat"), Some(s.feat_b.clone()));
    k.eq("main", e.ref_id("main"), Some(s.main.clone()));
    let files: Vec<(&str, String)> = feat_files().to_vec();
    clean_with(&mut k, &e, &files, status);
    k.done()
}

/// After the rebase: `feat`'s newest commits are copies of `replayed` (original id,
/// message) in that order (oldest first) right on `main`'s tip, each by [`ADA`] with the
/// user as committer; its files are `files`; `status` the working tree's status lines.
/// Three git processes.
pub fn expect_rebased(
    dir: &Path,
    s: &Setup,
    replayed: &[(&str, &str)],
    files: &[(&str, String)],
    status: &[&str],
    what: &str,
) -> Result<(), String> {
    let e = Expect::new(dir);
    let mut k = Checks::new(what);
    let log = e.log("feat", replayed.len() + 1);
    let new: Vec<_> = log.iter().take(replayed.len()).rev().collect();
    k.eq("commits", new.len(), replayed.len());
    k.eq("the new base", log.get(replayed.len()).map(|c| c.id.clone()), Some(s.main.clone()));
    let mut parent = s.main.clone();
    for (c, (id, msg)) in new.iter().zip(replayed) {
        k.eq("message", c.message.as_str(), *msg);
        k.eq("author", c.author.as_str(), ADA);
        k.eq("committer", c.committer.as_str(), TESTER);
        k.eq("parents", c.parents.clone(), vec![parent.clone()]);
        k.ok("a copy, not the original", c.id != *id, &c.id);
        parent = c.id.clone();
    }
    k.eq("main", e.ref_id("main"), Some(s.main.clone()));
    clean_with(&mut k, &e, files, status);
    k.done()
}

/// The files after replaying `feat` on `main`: `feat`'s lines over `main`'s ([`C8_MAIN`]),
/// then `edits` (path, line, text).
pub fn rebased_files(edits: &[(&str, usize, &str)]) -> Vec<(&'static str, String)> {
    let mut lines: Vec<(&'static str, usize, String)> =
        vec![("a.txt", 2, A2_FEAT.into()), ("b.txt", 5, B5_FEAT.into()), ("c.txt", 8, C8_MAIN.into())];
    for (path, at, text) in edits {
        lines.retain(|(p, l, _)| !(p == path && l == at));
        let p = ["a.txt", "b.txt", "c.txt"].into_iter().find(|p| p == path).expect("a fixture file");
        lines.push((p, *at, text.to_string()));
    }
    ["a.txt", "b.txt", "c.txt"]
        .into_iter()
        .map(|p| {
            let edits: Vec<(usize, &str)> =
                lines.iter().filter(|(q, ..)| *q == p).map(|(_, l, t)| (*l, t.as_str())).collect();
            (p, file(&p[..1], &edits))
        })
        .collect()
}

/// A rebase stopped with `paths` conflicted, `feat` not moved yet. One git process.
pub fn expect_stopped(dir: &Path, s: &Setup, paths: &[&str], what: &str) -> Result<(), String> {
    let e = Expect::new(dir);
    let mut k = Checks::new(what);
    k.eq("state", e.state(), OpState::Rebasing);
    k.eq("feat", e.ref_id("feat"), Some(s.feat_b.clone()));
    let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    k.eq("conflicted", conflicted(&e.status()), paths);
    k.done()
}
