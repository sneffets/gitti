//! SH — stash: stash and pop with its variants (untracked, `--all`, keep the index, only
//! staged — also next to unstaged lines), pop restoring the index, a pop an untracked
//! file blocks, a branch from a stash that an untracked file or local changes block,
//! dropping a stash
//! (also after the list changed outside, and the one being looked at), an apply that
//! conflicts, looking into a stash,
//! stashing lines with odd bytes; and WT-08, a stash made in one worktree and popped in
//! another. Stashes in fixtures come from [`Fixture::stash`] (no git process).

use std::path::{Path, PathBuf};

use crate::expect::{Checks, Expect, OpState};
use crate::fixture::{Fixture, Mark, put, put_bytes};
use crate::rs::lines;

/// `XY path` lines, sorted, as [`Expect::status_lines`] gives them.
fn status(lines: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    v.sort();
    v
}

/// The stash list's messages, newest first (from the stash reflog, no git).
pub fn stash_messages(e: &Expect) -> Vec<String> {
    e.reflog("refs/stash").into_iter().map(|(_, _, m)| m).collect()
}

/// The stash list's commits, newest first (from the stash reflog, no git).
pub fn stash_ids(e: &Expect) -> Vec<String> {
    e.reflog("refs/stash").into_iter().map(|(_, id, _)| id).collect()
}

/// **SH-01, SH-02, SH-03, SH-04 — stash the work, and pop it.** `main` has `a.txt`,
/// `s.txt` and a `.gitignore` for `*.log`. Around it: `s.txt` changed and staged
/// ([`S_STAGED`]), `a.txt` edited ([`A_EDIT`], unstaged), `u.txt` untracked, `build.log`
/// ignored. The user stashes with message [`MESSAGE`] — plainly ([`Kind::Plain`]),
/// including untracked files ([`Kind::Untracked`], `-u`), including ignored ones
/// ([`Kind::All`], `--all`) or keeping the index ([`Kind::KeepIndex`]) — and then pops it.
///
/// Expected after the stash ([`expect_stashed`]): one stash entry holding both changes
/// (`s.txt` staged in its index), plus `u.txt` with `-u` / `--all` and `build.log` with
/// `--all`; the working tree back at HEAD except what stays: `u.txt` without `-u`,
/// `build.log` without `--all`, and with keep-index the staged `s.txt`. After the pop
/// ([`expect_popped`]; not for keep-index): the stash list empty, every file as before —
/// the staged change of `s.txt` comes back *unstaged* (a plain pop doesn't restore the
/// index: SH-17). [`fixture_stashed`] starts at the state after the stash.
pub mod stash_and_pop {
    use super::*;

    pub const MESSAGE: &str = "work in progress";
    pub const S: &str = "s\n";
    pub const S_STAGED: &str = "s, staged\n";
    pub const U: &str = "untracked\n";
    pub const LOG: &str = "log\n";

    pub fn a() -> String {
        lines("a", 6, &[])
    }

    pub fn a_edit() -> String {
        lines("a", 6, &[(3, "a 3 edited")])
    }

    /// How the user stashes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Kind {
        Plain,
        /// `-u`
        Untracked,
        /// `--all`
        All,
        /// `--keep-index`
        KeepIndex,
    }

    /// See the module docs. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a()), put("s.txt", S), put(".gitignore", "*.log\n")]);
        fx.stage(&[put("s.txt", S_STAGED)]);
        fx.write("a.txt", &a_edit());
        fx.write("u.txt", U);
        fx.write("build.log", LOG);
        fx.build(dir);
    }

    /// The state after stashing as `kind` (not keep-index), written directly
    /// ([`Fixture::stash`]) — the start of a pop step. Two git processes.
    pub fn fixture_stashed(dir: &Path, kind: Kind) {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", &a()), put("s.txt", S), put(".gitignore", "*.log\n")]);
        let mut untracked = Vec::new();
        if matches!(kind, Kind::Untracked | Kind::All) {
            untracked.push(put("u.txt", U));
        } else {
            fx.write("u.txt", U);
        }
        if kind == Kind::All {
            untracked.push(put("build.log", LOG));
        } else {
            fx.write("build.log", LOG);
        }
        fx.stash(i, &format!("On main: {MESSAGE}"), &[put("s.txt", S_STAGED)], &[put("a.txt", &a_edit())], &untracked);
        fx.build(dir);
    }

    /// After stashing as `kind`: one entry `On main: MESSAGE` with the changes; the
    /// working tree back at HEAD but for what stays. Two git processes.
    pub fn expect_stashed(dir: &Path, kind: Kind) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new(&format!("SH-01..04 after stashing ({kind:?})"));
        k.eq("stash list", stash_messages(&e), vec![format!("On main: {MESSAGE}")]);
        let b = e.blobs(&["stash@{0}:a.txt", "stash@{0}:s.txt", "stash@{0}^2:s.txt", "stash@{0}^3:u.txt", "stash@{0}^3:build.log"]);
        k.text("a.txt in the stash", b[0].as_deref(), Some(&a_edit()));
        k.text("s.txt in the stash", b[1].as_deref(), Some(S_STAGED));
        k.text("s.txt in the stash's index", b[2].as_deref(), Some(S_STAGED));
        let with_u = matches!(kind, Kind::Untracked | Kind::All);
        k.text("u.txt in the stash", b[3].as_deref(), with_u.then_some(U));
        k.text("build.log in the stash", b[4].as_deref(), (kind == Kind::All).then_some(LOG));
        let want = match kind {
            Kind::Plain => status(&["?? u.txt"]),
            Kind::Untracked | Kind::All => Vec::new(),
            Kind::KeepIndex => status(&["M  s.txt", "?? u.txt"]),
        };
        k.eq("status", e.status_lines(), want);
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&a()));
        let s = if kind == Kind::KeepIndex { S_STAGED } else { S };
        k.text("s.txt", e.file("s.txt").as_deref(), Some(s));
        k.text("u.txt", e.file("u.txt").as_deref(), (!with_u).then_some(U));
        k.text("build.log", e.file("build.log").as_deref(), (kind != Kind::All).then_some(LOG));
        k.done()
    }

    /// After popping the stash of `kind` (not keep-index): the list empty, every file as
    /// before the stash, `s.txt`'s change unstaged now. One git process.
    pub fn expect_popped(dir: &Path, kind: Kind) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new(&format!("SH-01..03 after the pop ({kind:?})"));
        k.eq("stashes", e.stashes(), 0);
        k.eq("status", e.status_lines(), status(&[" M a.txt", " M s.txt", "?? u.txt"]));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&a_edit()));
        k.text("s.txt", e.file("s.txt").as_deref(), Some(S_STAGED));
        k.text("u.txt", e.file("u.txt").as_deref(), Some(U));
        k.text("build.log", e.file("build.log").as_deref(), Some(LOG));
        k.done()
    }
}

/// **SH-05 — stash only the staged changes.** `main` has `a.txt` and `s.txt`; `s.txt`
/// changed and staged ([`S_STAGED`]), `a.txt` edited, not staged. The user stashes with
/// "Only staged changes" (`--staged`).
///
/// Expected ([`expect_stashed`]): the stash holds `s.txt`'s change only (its `a.txt` is
/// HEAD's); `s.txt` back at HEAD, `a.txt`'s edit stays unstaged.
///
/// With [`fixture_overlap`] `t.txt` also has a staged line that was changed again in the
/// working tree — the staged version can't be taken out without the unstaged one.
/// Expected ([`expect_refused`]): refused before anything happens, the app says why and
/// what to do ([`REFUSAL`]); no stash, nothing changed. (Unstaged lines right *next to*
/// staged ones work: SH-22.)
pub mod staged_only {
    use super::*;

    pub const S: &str = "s\n";
    pub const S_STAGED: &str = "s, staged\n";
    pub const T_STAGED: &str = "t 1\nt 2 staged\nt 3\nt 4\n";
    pub const T_WORK: &str = "t 1\nt 2 staged again\nt 3\nt 4\n";
    /// The start of the refusal (`gitti_core::stash_staged`).
    pub const REFUSAL: &str = "Not stashed: t.txt has unstaged changes on the staged lines themselves";

    pub fn a() -> String {
        lines("a", 6, &[])
    }

    pub fn a_edit() -> String {
        lines("a", 6, &[(3, "a 3 edited")])
    }

    fn base(overlap: bool) -> Fixture {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a()), put("s.txt", S), put("t.txt", "t 1\nt 2\nt 3\nt 4\n")]);
        fx.stage(&[put("s.txt", S_STAGED)]);
        fx.write("a.txt", &a_edit());
        if overlap {
            fx.stage(&[put("t.txt", T_STAGED)]);
            fx.write("t.txt", T_WORK);
        }
        fx
    }

    /// See the module docs. Three git processes.
    pub fn fixture(dir: &Path) {
        base(false).build(dir);
    }

    /// With `t.txt`'s staged line changed again in the working tree. Three git processes.
    pub fn fixture_overlap(dir: &Path) {
        base(true).build(dir);
    }

    /// After the stash: `s.txt`'s change in it, nothing else; `a.txt`'s edit stays. Two
    /// git processes.
    pub fn expect_stashed(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-05 after stashing the staged changes");
        k.eq("stashes", e.stashes(), 1);
        let b = e.blobs(&["stash@{0}:s.txt", "stash@{0}:a.txt"]);
        k.text("s.txt in the stash", b[0].as_deref(), Some(S_STAGED));
        k.text("a.txt in the stash (HEAD's)", b[1].as_deref(), Some(&a()));
        k.eq("status", e.status_lines(), status(&[" M a.txt"]));
        k.text("s.txt", e.file("s.txt").as_deref(), Some(S));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&a_edit()));
        k.done()
    }

    /// After the refused stash: no stash; index and working tree as they were. One git
    /// process.
    pub fn expect_refused(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-05 after the refused stash");
        k.eq("stashes", e.stashes(), 0);
        k.eq("status", e.status_lines(), status(&[" M a.txt", "M  s.txt", "MM t.txt"]));
        k.text("t.txt", e.file("t.txt").as_deref(), Some(T_WORK));
        k.done()
    }
}

/// **SH-22 — stash only the staged changes next to unstaged ones.** `main` has `b.txt`
/// ([`B`]) and `t.txt` ([`T`]). Four lines were inserted into `b.txt` — two blank lines,
/// `bla`, a blank line ([`B_WORK`]) — and only the upper two blank lines staged
/// ([`B_STAGED`]); in `t.txt` line 2 is changed and staged, line 3 changed and not staged
/// ([`T_STAGED`], [`T_WORK`]). The user stashes with "Only staged changes". git alone
/// would save the stash and then stop ("Cannot remove worktree changes": its reverse
/// patch needs the line next to the staged ones as it is in the index).
///
/// Expected ([`expect_stashed`]): one stash holding the staged versions (as its index
/// and its working tree); nothing staged; each file HEAD's version plus exactly its
/// unstaged lines, in their places ([`B_LEFT`], [`T_LEFT`]).
pub mod staged_next_to_unstaged {
    use super::*;

    pub const B: &str = "a\nb\nc\nd\n";
    pub const B_STAGED: &str = "a\nb\n\n\nc\nd\n";
    pub const B_WORK: &str = "a\nb\n\n\nbla\n\nc\nd\n";
    pub const B_LEFT: &str = "a\nb\nbla\n\nc\nd\n";
    pub const T: &str = "t 1\nt 2\nt 3\nt 4\n";
    pub const T_STAGED: &str = "t 1\nt 2 staged\nt 3\nt 4\n";
    pub const T_WORK: &str = "t 1\nt 2 staged\nt 3 unstaged\nt 4\n";
    pub const T_LEFT: &str = "t 1\nt 2\nt 3 unstaged\nt 4\n";

    /// See the module docs. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("b.txt", B), put("t.txt", T)]);
        fx.stage(&[put("b.txt", B_STAGED), put("t.txt", T_STAGED)]);
        fx.write("b.txt", B_WORK);
        fx.write("t.txt", T_WORK);
        fx.build(dir);
    }

    /// After the stash: the staged versions in it; the unstaged lines stay, unstaged.
    /// Two git processes.
    pub fn expect_stashed(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-22 after stashing the staged changes");
        k.eq("stashes", e.stashes(), 1);
        let b = e.blobs(&["stash@{0}:b.txt", "stash@{0}:t.txt", "stash@{0}^2:b.txt", "stash@{0}^2:t.txt"]);
        k.text("b.txt in the stash", b[0].as_deref(), Some(B_STAGED));
        k.text("t.txt in the stash", b[1].as_deref(), Some(T_STAGED));
        k.text("b.txt in the stash's index", b[2].as_deref(), Some(B_STAGED));
        k.text("t.txt in the stash's index", b[3].as_deref(), Some(T_STAGED));
        k.eq("status", e.status_lines(), status(&[" M b.txt", " M t.txt"]));
        k.text("b.txt", e.file("b.txt").as_deref(), Some(B_LEFT));
        k.text("t.txt", e.file("t.txt").as_deref(), Some(T_LEFT));
        k.done()
    }
}

/// **SH-08 — stash lines of a file with odd bytes.** `l.txt` is Latin-1 text with CRLF
/// line ends and no newline at the end ([`OLD`]); edited in two places ([`NEW`]: the
/// first line, the last line). The user selects the second hunk (the last line) and
/// stashes those lines.
///
/// Expected ([`expect_stashed`]): `l.txt` keeps the first change byte for byte
/// ([`STAYS`]); the stash's `l.txt` is HEAD's with only the last line changed
/// ([`STASHED`]), byte for byte.
pub mod odd_bytes {
    use super::*;

    pub const OLD: &[u8] = b"caf\xe9\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\nlast";
    pub const NEW: &[u8] = b"caf\xe9!\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\nLAST";
    pub const STAYS: &[u8] = b"caf\xe9!\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\nlast";
    pub const STASHED: &[u8] = b"caf\xe9\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\nLAST";

    /// See the module docs. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put_bytes("l.txt", OLD)]);
        fx.write_bytes("l.txt", NEW);
        fx.build(dir);
    }

    /// After stashing the last line: exact bytes in the stash and on disk. One git
    /// process.
    pub fn expect_stashed(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-08 after stashing the last line");
        k.eq("stashes", e.stashes(), 1);
        let b = e.blobs(&["stash@{0}:l.txt", ":l.txt"]);
        k.eq("l.txt in the stash", b[0].as_deref(), Some(STASHED));
        k.eq("l.txt in the index", b[1].as_deref(), Some(OLD));
        k.eq("l.txt on disk", e.file("l.txt").as_deref(), Some(STAYS));
        k.done()
    }
}

/// **SH-11 — drop one stash of several.** Three stashes on `init`: [`FIRST`] (oldest),
/// [`SECOND`], [`THIRD`] (newest, `stash@{0}`). The user drops [`SECOND`]
/// (`stash@{1}`), confirming.
///
/// Expected ([`expect_dropped`]): [`THIRD`] and [`FIRST`] are left, in that order, with
/// their commits; the working tree untouched (clean).
pub mod drop_one {
    use super::*;

    pub const FIRST: &str = "On main: first";
    pub const SECOND: &str = "On main: second";
    pub const THIRD: &str = "On main: third";

    pub struct Setup {
        /// The stash commits, oldest first.
        pub ids: [String; 3],
    }

    /// See the module docs. Two git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let marks: Vec<Mark> = [FIRST, SECOND, THIRD]
            .iter()
            .map(|m| fx.stash(i, m, &[], &[put("a.txt", &format!("a\n{m}\n"))], &[]))
            .collect();
        let ids = fx.build(dir);
        Setup { ids: [0, 1, 2].map(|n| ids[marks[n]].to_string()) }
    }

    /// After dropping [`SECOND`]. No git process but the status.
    pub fn expect_dropped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-11 after dropping the second stash");
        k.eq("stash list", stash_messages(&e), vec![THIRD.to_string(), FIRST.to_string()]);
        k.eq("stash commits", stash_ids(&e), vec![s.ids[2].clone(), s.ids[0].clone()]);
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **SH-18 — drop after the list changed outside.** One stash, [`CHOSEN`]; `a.txt`
/// edited. The user opens the menu of [`CHOSEN`] (`stash@{0}`); meanwhile a terminal
/// runs [`TERMINAL`] (`git stash push`): [`CHOSEN`] moves to `stash@{1}`. The user picks
/// Drop and confirms.
///
/// Expected ([`expect_dropped`]): the stash the user chose is gone, the terminal's one
/// ([`TERMINAL_MESSAGE`]) stays.
pub mod drop_after_push {
    use super::*;

    pub const CHOSEN: &str = "On main: chosen";
    pub const TERMINAL_MESSAGE: &str = "On main: terminal";
    /// The terminal's command.
    pub const TERMINAL: [&str; 5] = ["stash", "push", "-q", "-m", "terminal"];

    pub struct Setup {
        pub chosen: String,
    }

    /// See the module docs. Two git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let c = fx.stash(i, CHOSEN, &[], &[put("a.txt", "a\nchosen\n")], &[]);
        fx.write("a.txt", "a\nterminal\n");
        let ids = fx.build(dir);
        Setup { chosen: ids[c].into() }
    }

    /// After the drop: only the terminal's stash. No git process.
    pub fn expect_dropped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-18 after the drop");
        k.eq("stash list", stash_messages(&e), vec![TERMINAL_MESSAGE.to_string()]);
        k.ok("the chosen stash is gone", !stash_ids(&e).contains(&s.chosen), stash_ids(&e));
        k.done()
    }
}

/// **SH-21 — drop the stash being looked at.** `main`: `init` (`a.txt`), [`SECOND`]
/// (`b.txt`). Two stashes: [`OTHER`] on [`SECOND`] (`stash@{1}`) and [`LOOK`] on `init`
/// (`stash@{0}`, `a.txt` with [`LOOK_LINE`] added). The user selects [`LOOK`]'s row (its
/// diff shows) and drops it — in the app, or a terminal runs [`TERMINAL`] (the watcher
/// picks it up).
///
/// Expected: only [`OTHER`] is left ([`expect_dropped`]). In the apps the selection moves
/// to [`LOOK`]'s base commit `init` and shows its diff — not [`OTHER`], which took over
/// `stash@{0}`, and nothing of [`LOOK`] stays on screen.
pub mod drop_selected {
    use super::*;

    pub const SECOND: &str = "second";
    pub const OTHER: &str = "On main: other";
    pub const LOOK: &str = "On main: look";
    pub const LOOK_LINE: &str = "looked at";
    /// The terminal's command.
    pub const TERMINAL: [&str; 4] = ["stash", "drop", "-q", "stash@{0}"];

    pub struct Setup {
        pub init: String,
        pub second: String,
        pub other: String,
        pub look: String,
    }

    /// See the module docs. Two git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let s = fx.commit("main", SECOND, &[put("b.txt", "b\n")]);
        let o = fx.stash(s, OTHER, &[], &[put("b.txt", "b\nother\n")], &[]);
        let l = fx.stash(i, LOOK, &[], &[put("a.txt", &format!("a\n{LOOK_LINE}\n"))], &[]);
        let ids = fx.build(dir);
        Setup {
            init: ids[i].into(),
            second: ids[s].into(),
            other: ids[o].into(),
            look: ids[l].into(),
        }
    }

    /// After the drop: only [`OTHER`]. No git process but the status.
    pub fn expect_dropped(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-21 after dropping the stash looked at");
        k.eq("stash list", stash_messages(&e), vec![OTHER.to_string()]);
        k.eq("stash commits", stash_ids(&e), vec![s.other.clone()]);
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **SH-13 — apply a stash that conflicts, resolve it.** A stash on `init` changes line
/// 2 of `a.txt` ([`STASHED`]); `main` moved on, changing the same line ([`MAIN`]). The
/// user applies the stash — it stops on a conflict in `a.txt` (no operation state: a
/// stash has none) — and takes the stash's side.
///
/// Expected: after the apply ([`expect_conflict`]) `a.txt` conflicted, nothing in
/// progress, the stash kept; after taking the stash's side ([`expect_resolved`]) `a.txt`
/// is the stash's version, staged; no conflict, nothing in progress; the stash still
/// listed (apply keeps it). [`fixture_conflicted`] starts at the stopped apply.
pub mod apply_conflict {
    use super::*;

    pub fn base() -> String {
        lines("a", 4, &[])
    }

    pub fn stashed() -> String {
        lines("a", 4, &[(2, "a 2 from the stash")])
    }

    pub fn main() -> String {
        lines("a", 4, &[(2, "a 2 on main")])
    }

    pub const STASH: &str = "On main: line 2";

    /// See the module docs. Two git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", &base())]);
        fx.stash(i, STASH, &[], &[put("a.txt", &stashed())], &[]);
        fx.commit("main", "Change a", &[put("a.txt", &main())]);
        fx.build(dir);
    }

    /// The state after the apply stopped, from the one real `git stash apply` — the start
    /// of the resolve step. Three git processes.
    pub fn fixture_conflicted(dir: &Path) {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", &base())]);
        fx.stash(i, STASH, &[], &[put("a.txt", &stashed())], &[]);
        fx.commit("main", "Change a", &[put("a.txt", &main())]);
        fx.git_after_stops(&["stash", "apply", "-q"]);
        fx.build(dir);
    }

    /// After the apply stopped: `a.txt` conflicted, nothing in progress, the stash kept.
    /// One git process.
    pub fn expect_conflict(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-13 after the apply stopped");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("conflicted", crate::expect::conflicted(&e.status()), vec!["a.txt".to_string()]);
        k.eq("stash list", stash_messages(&e), vec![STASH.to_string()]);
        k.done()
    }

    /// After taking the stash's side: resolved, staged, nothing in progress, the stash
    /// kept. Two git processes.
    pub fn expect_resolved(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-13 after taking the stash's side");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("status", e.status_lines(), status(&["M  a.txt"]));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&stashed()));
        k.text("a.txt staged", e.index("a.txt").as_deref(), Some(&stashed()));
        k.eq("stash list", stash_messages(&e), vec![STASH.to_string()]);
        k.done()
    }
}

/// **SH-15 — look into a stash.** A stash on `init` with `a.txt` changed ([`A_STASHED`])
/// and the untracked `n.txt` ([`N`], `-u`). The user selects the stash's row.
///
/// Expected: its diff lists `a.txt` and the untracked `n.txt`; nothing changes
/// ([`expect_unchanged`]).
pub mod look_into {
    use super::*;

    pub const A_STASHED: &str = "a\nstashed\n";
    pub const N: &str = "new in the stash\n";
    pub const STASH: &str = "On main: look";

    pub struct Setup {
        pub stash: String,
    }

    /// See the module docs. Two git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let w = fx.stash(i, STASH, &[], &[put("a.txt", A_STASHED)], &[put("n.txt", N)]);
        let ids = fx.build(dir);
        Setup { stash: ids[w].into() }
    }

    /// Nothing changed: clean, the stash listed. One git process.
    pub fn expect_unchanged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-15 after looking into the stash");
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.eq("stash list", stash_messages(&e), vec![STASH.to_string()]);
        k.done()
    }
}

/// **SH-17 — pop restoring the index.** A stash on `init` made with `s.txt` staged
/// ([`S_STAGED`]: line 2) and edited further ([`S_WORK`]: line 5), and `a.txt` edited
/// (not staged); the working tree clean. The user pops it restoring the index (`--index`;
/// the app asks first, naming `s.txt` as staged again).
///
/// Expected ([`expect_popped`]): `s.txt` staged as [`S_STAGED`] with [`S_WORK`] on disk,
/// `a.txt` edited, unstaged; the stash list empty.
///
/// With [`fixture_index_moved`] the index has `s.txt`'s line 2 staged differently
/// ([`S_OTHER`]): the stash's staged part doesn't apply to it. Expected
/// ([`expect_refused`]): refused — nothing changed, the stash kept.
pub mod pop_index {
    use super::*;

    pub const STASH: &str = "On main: half staged";

    pub fn s() -> String {
        lines("s", 6, &[])
    }

    pub fn s_staged() -> String {
        lines("s", 6, &[(2, "s 2 staged")])
    }

    pub fn s_work() -> String {
        lines("s", 6, &[(2, "s 2 staged"), (5, "s 5 unstaged")])
    }

    pub fn s_other() -> String {
        lines("s", 6, &[(2, "s 2 other")])
    }

    pub fn a_edit() -> String {
        lines("a", 6, &[(3, "a 3 edited")])
    }

    fn base(moved: bool) -> Fixture {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", &lines("a", 6, &[])), put("s.txt", &s())]);
        fx.stash(i, STASH, &[put("s.txt", &s_staged())], &[put("s.txt", &s_work()), put("a.txt", &a_edit())], &[]);
        if moved {
            fx.stage(&[put("s.txt", &s_other())]);
        }
        fx
    }

    /// See the module docs. Two git processes.
    pub fn fixture(dir: &Path) {
        base(false).build(dir);
    }

    /// The index has `s.txt` staged as [`s_other`]. Two git processes.
    pub fn fixture_index_moved(dir: &Path) {
        base(true).build(dir);
    }

    /// After the pop with the index: `s.txt` staged and edited further, `a.txt` edited;
    /// no stash. Two git processes.
    pub fn expect_popped(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-17 after the pop restoring the index");
        k.eq("stashes", e.stashes(), 0);
        k.eq("status", e.status_lines(), status(&[" M a.txt", "MM s.txt"]));
        k.text("s.txt staged", e.index("s.txt").as_deref(), Some(&s_staged()));
        k.text("s.txt", e.file("s.txt").as_deref(), Some(&s_work()));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&a_edit()));
        k.done()
    }

    /// After the refused pop: nothing changed (`s.txt` staged as [`s_other`]), the stash
    /// kept. Two git processes.
    pub fn expect_refused(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-17 after the refused pop");
        k.eq("stash list", stash_messages(&e), vec![STASH.to_string()]);
        k.eq("status", e.status_lines(), status(&["M  s.txt"]));
        k.text("s.txt staged", e.index("s.txt").as_deref(), Some(&s_other()));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&lines("a", 6, &[])));
        k.done()
    }
}

/// **SH-19 — a pop an untracked file blocks.** A stash on `init` (made with `-u`) has
/// `a.txt` changed ([`A_STASHED`]) and the untracked `x.txt` ([`X_STASHED`]); now an
/// untracked `x.txt` exists ([`X_HERE`]). The user pops the stash.
///
/// Expected ([`expect_refused`]): refused in plain words naming `x.txt`; nothing changed
/// — `a.txt` still HEAD's (git alone would have applied it and then stopped), `x.txt`
/// still the user's — and the stash kept. Once the user moved `x.txt` away
/// ([`move_away`]), the pop works ([`expect_popped`]).
pub mod untracked_in_the_way {
    use super::*;

    pub const A_STASHED: &str = "a\nstashed\n";
    pub const X_STASHED: &str = "x from the stash\n";
    pub const X_HERE: &str = "x, new here\n";
    pub const STASH: &str = "On main: with x";

    /// See the module docs. Two git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.stash(i, STASH, &[], &[put("a.txt", A_STASHED)], &[put("x.txt", X_STASHED)]);
        fx.write("x.txt", X_HERE);
        fx.build(dir);
    }

    /// The user moves `x.txt` to `x-mine.txt` (no git).
    pub fn move_away(dir: &Path) {
        std::fs::rename(dir.join("x.txt"), dir.join("x-mine.txt")).unwrap();
    }

    /// After the refused pop: nothing changed, the stash kept. One git process.
    pub fn expect_refused(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-19 after the refused pop");
        k.eq("stash list", stash_messages(&e), vec![STASH.to_string()]);
        k.eq("status", e.status_lines(), status(&["?? x.txt"]));
        k.text("a.txt", e.file("a.txt").as_deref(), Some("a\n"));
        k.text("x.txt", e.file("x.txt").as_deref(), Some(X_HERE));
        k.done()
    }

    /// After moving `x.txt` away and popping: the stash's files back, the user's
    /// `x-mine.txt` kept, no stash. One git process.
    pub fn expect_popped(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-19 after moving x.txt away and popping");
        k.eq("stashes", e.stashes(), 0);
        k.eq("status", e.status_lines(), status(&[" M a.txt", "?? x-mine.txt", "?? x.txt"]));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(A_STASHED));
        k.text("x.txt", e.file("x.txt").as_deref(), Some(X_STASHED));
        k.text("x-mine.txt", e.file("x-mine.txt").as_deref(), Some(X_HERE));
        k.done()
    }
}

/// **SH-20 — a branch from a stash, blocked.** `main` has `init` (`a.txt`, `b.txt`) and
/// `later` (`b.txt` changed). A stash on `init` (made with `-u`) has `a.txt` changed
/// ([`A_STASHED`]) and the untracked `x.txt` ([`X_STASHED`]); now an untracked `x.txt`
/// exists ([`X_HERE`]). The user creates the branch [`BRANCH`] from the stash.
///
/// Expected ([`expect_refused`]): refused in plain words naming `x.txt`; nothing changed
/// — still on `main` (git alone would have created and checked out the branch, applied
/// `a.txt` and then stopped), no branch [`BRANCH`], `x.txt` still the user's, the stash
/// kept. Once the user moved `x.txt` away ([`move_away`]) it works ([`expect_branched`]):
/// on [`BRANCH`] at `init`, `a.txt` as stashed, `x.txt` back, the user's `x-mine.txt`
/// kept, no stash.
///
/// With [`fixture_local`] there is no `x.txt` but `a.txt` has an uncommitted edit
/// ([`A_LOCAL`]), which the stash changes too: git would check the branch out and then
/// refuse to apply the stash. Expected ([`expect_refused_local`]): refused naming
/// `a.txt`, nothing changed.
pub mod branch_blocked {
    use super::*;

    pub const A_STASHED: &str = "a\nstashed\n";
    pub const A_LOCAL: &str = "a\nedited here\n";
    pub const B_LATER: &str = "b\nlater\n";
    pub const X_STASHED: &str = "x from the stash\n";
    pub const X_HERE: &str = "x, new here\n";
    pub const STASH: &str = "On main: with x";
    pub const BRANCH: &str = "from-stash";

    pub struct Setup {
        pub init: String,
        pub later: String,
    }

    fn base(local: bool) -> (Fixture, Mark, Mark) {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", "a\n"), put("b.txt", "b\n")]);
        let l = fx.commit("main", "later", &[put("b.txt", B_LATER)]);
        fx.stash(i, STASH, &[], &[put("a.txt", A_STASHED)], &[put("x.txt", X_STASHED)]);
        if local {
            fx.write("a.txt", A_LOCAL);
        } else {
            fx.write("x.txt", X_HERE);
        }
        (fx, i, l)
    }

    /// See the module docs. Two git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let (fx, i, l) = base(false);
        let ids = fx.build(dir);
        Setup { init: ids[i].into(), later: ids[l].into() }
    }

    /// No `x.txt`, `a.txt` edited ([`A_LOCAL`]). Two git processes.
    pub fn fixture_local(dir: &Path) -> Setup {
        let (fx, i, l) = base(true);
        let ids = fx.build(dir);
        Setup { init: ids[i].into(), later: ids[l].into() }
    }

    /// The user moves `x.txt` to `x-mine.txt` (no git).
    pub fn move_away(dir: &Path) {
        std::fs::rename(dir.join("x.txt"), dir.join("x-mine.txt")).unwrap();
    }

    fn unchanged(dir: &Path, s: &Setup, k: &mut Checks) -> Expect {
        let e = Expect::new(dir);
        k.eq("branch", e.branch(), Some("main".to_string()));
        k.eq("main", e.ref_id("main"), Some(s.later.clone()));
        k.eq(BRANCH, e.ref_id(BRANCH), None);
        k.eq("stash list", stash_messages(&e), vec![STASH.to_string()]);
        k.text("b.txt", e.file("b.txt").as_deref(), Some(B_LATER));
        e
    }

    /// After the refused branch: nothing changed, the stash kept. One git process.
    pub fn expect_refused(dir: &Path, s: &Setup) -> Result<(), String> {
        let mut k = Checks::new("SH-20 after the refused branch from the stash");
        let e = unchanged(dir, s, &mut k);
        k.eq("status", e.status_lines(), status(&["?? x.txt"]));
        k.text("a.txt", e.file("a.txt").as_deref(), Some("a\n"));
        k.text("x.txt", e.file("x.txt").as_deref(), Some(X_HERE));
        k.done()
    }

    /// After the branch refused over the local edit of `a.txt`: nothing changed. One git
    /// process.
    pub fn expect_refused_local(dir: &Path, s: &Setup) -> Result<(), String> {
        let mut k = Checks::new("SH-20 after the branch from the stash refused over local changes");
        let e = unchanged(dir, s, &mut k);
        k.eq("status", e.status_lines(), status(&[" M a.txt"]));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(A_LOCAL));
        k.done()
    }

    /// After moving `x.txt` away and creating the branch: on [`BRANCH`] at `init`, the
    /// stash's files back, `x-mine.txt` kept, no stash. One git process.
    pub fn expect_branched(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("SH-20 after moving x.txt away and creating the branch");
        k.eq("branch", e.branch(), Some(BRANCH.to_string()));
        k.eq(BRANCH, e.ref_id(BRANCH), Some(s.init.clone()));
        k.eq("main", e.ref_id("main"), Some(s.later.clone()));
        k.eq("stashes", e.stashes(), 0);
        k.eq("status", e.status_lines(), status(&[" M a.txt", "?? x-mine.txt", "?? x.txt"]));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(A_STASHED));
        k.text("b.txt", e.file("b.txt").as_deref(), Some("b\n"));
        k.text("x.txt", e.file("x.txt").as_deref(), Some(X_STASHED));
        k.text("x-mine.txt", e.file("x-mine.txt").as_deref(), Some(X_HERE));
        k.done()
    }
}

/// **WT-08 — a stash made in one worktree, popped in another.** The main worktree is on
/// `main`; a stash made there ([`STASH`]: `a.txt` edited, [`A_STASHED`]). The linked
/// worktree [`crate::wt::linked_of`]`(work, "feat")` has `feat` (at `init` too) checked
/// out, clean. The user opens the linked worktree — its stash list is the shared one —
/// and pops the stash there.
///
/// Expected ([`expect_popped`]): in the linked worktree `a.txt` edited (unstaged); the
/// stash list (shared) empty; the main worktree untouched and clean.
pub mod pop_in_other_worktree {
    use super::*;

    pub const A_STASHED: &str = "a\nfrom the main worktree\n";
    pub const STASH: &str = "On main: made in the main worktree";

    pub struct Setup {
        /// The linked worktree, where the stash is popped.
        pub linked: PathBuf,
    }

    /// See the module docs. Three git processes.
    pub fn fixture(work: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.branch("feat", i);
        fx.stash(i, STASH, &[], &[put("a.txt", A_STASHED)], &[]);
        let linked = crate::wt::linked_of(work, "feat");
        let name = linked.file_name().unwrap().to_string_lossy().into_owned();
        fx.git_after(&["worktree", "add", "-q", &format!("../{name}"), "feat"]);
        fx.build(work);
        Setup { linked }
    }

    /// After the pop in the linked worktree. Two git processes.
    pub fn expect_popped(work: &Path, s: &Setup) -> Result<(), String> {
        let l = Expect::new(&s.linked);
        let m = Expect::new(work);
        let mut k = Checks::new("WT-08 after the pop in the linked worktree");
        k.eq("stashes (shared)", m.stashes(), 0);
        k.eq("linked: branch", l.branch().as_deref(), Some("feat"));
        k.eq("linked: status", l.status_lines(), status(&[" M a.txt"]));
        k.text("linked: a.txt", l.file("a.txt").as_deref(), Some(A_STASHED));
        k.eq("main worktree: status", m.status_lines(), Vec::<String>::new());
        k.text("main worktree: a.txt", m.file("a.txt").as_deref(), Some("a\n"));
        k.done()
    }
}
