//! RV — revert: undoing commits of the current branch with new commits, on the history
//! of [`crate::cp::picks`] (`main`: … → [`MAIN_C`] → [`MERGE_TOPIC`] → [`MAIN_C_AGAIN`] →
//! [`MAIN_A`] → [`MAIN_A_BACK`]).
//!
//! - RV-01: revert [`MAIN_C_AGAIN`] — c:8 back to [`C8`] ([`expect_reverted_c_again`]).
//! - RV-05: then revert that revert — c:8 [`C8_AGAIN`] again ([`expect_reapplied`]).
//! - RV-02: revert the merge [`MERGE_TOPIC`] — against its first parent: d:9 back
//!   ([`expect_merge_reverted`]).
//! - RV-03 / RV-04: revert [`MAIN_C`] — conflicts (c:8 changed again since); resolved and
//!   continued with an edited message ([`expect_resolved`]), or aborted
//!   ([`crate::cp::picks::expect_unchanged`]).
//! - RV-06: revert [`MAIN_A`] — [`MAIN_A_BACK`] undid it already: alone, git does
//!   nothing (said in plain words); in a sequence it stops there with nothing to commit,
//!   Skip goes on with the rest.
//! - RV-07: revert several marked commits — newest first ([`expect_several_reverted`]).
//! - RV-08: revert some lines of an older commit in the working tree only ([`lines`]).

use std::path::Path;

pub use crate::cp::picks::{
    C8, C8_AGAIN, D9, MAIN_A, MAIN_A_BACK, MAIN_C, MAIN_C_AGAIN, MERGE_TOPIC, Setup, expect_unchanged, file,
    files_with, fixture, fixture_resolved, fixture_stopped,
};
use crate::expect::{Checks, Expect, OpState};
use crate::fixture::TESTER;

/// git's message of a revert of `subject` / `id` (`git revert --no-edit`).
pub fn revert_message(subject: &str, id: &str) -> String {
    format!("Revert \"{subject}\"\n\nThis reverts commit {id}.")
}

/// git's message of a revert of the merge `subject` / `id` against its first parent
/// `first`.
pub fn revert_merge_message(subject: &str, id: &str, first: &str) -> String {
    format!("Revert \"{subject}\"\n\nThis reverts commit {id}, reversing\nchanges made to {first}.")
}

/// HEAD is one new commit on `parent` with `message` by the user, and the files are
/// [`files_with`] `edits`; nothing in progress. Three git processes.
fn expect_one(dir: &Path, parent: &str, message: &str, edits: &[(&str, usize, &str)], what: &str) -> Result<String, String> {
    let e = Expect::new(dir);
    let mut k = Checks::new(what);
    let head = e.log("HEAD", 1).into_iter().next();
    match &head {
        Some(c) => {
            k.eq("message", c.message.as_str(), message);
            k.eq("parents", c.parents.clone(), vec![parent.to_string()]);
            k.eq("author", c.author.as_str(), TESTER);
        }
        None => k.fail("no HEAD commit".into()),
    }
    k.eq("state", e.state(), OpState::Clean);
    k.eq("branch", e.branch().as_deref(), Some("main"));
    let files = files_with(edits);
    let specs: Vec<String> = files.iter().map(|(p, _)| format!("HEAD:{p}")).collect();
    let specs: Vec<&str> = specs.iter().map(String::as_str).collect();
    for ((p, want), got) in files.iter().zip(e.blobs(&specs)) {
        k.text(&format!("HEAD:{p}"), got.as_deref(), Some(want.as_str()));
    }
    k.eq("status", e.status_lines(), Vec::<String>::new());
    k.done()?;
    Ok(head.map(|c| c.id).unwrap_or_default())
}

/// RV-01: HEAD is `Revert "Change c again"` on `main`'s tip; c:8 is [`C8`] again. The
/// revert's id. Three git processes.
pub fn expect_reverted_c_again(dir: &Path, s: &Setup) -> Result<String, String> {
    expect_one(
        dir,
        &s.main,
        &revert_message(MAIN_C_AGAIN, &s.main_c_again),
        &[("c.txt", 8, C8)],
        "RV-01 after the revert",
    )
}

/// RV-05: the revert of the revert `revert` (git calls it `Reapply "…"`; older git `Revert
/// "Revert …"`) on it; c:8 is [`C8_AGAIN`] as before both. Three git processes.
pub fn expect_reapplied(dir: &Path, revert: &str) -> Result<(), String> {
    let e = Expect::new(dir);
    let mut k = Checks::new("RV-05 after reverting the revert");
    match e.log("HEAD", 1).first() {
        Some(c) => {
            let subject = c.subject();
            k.ok(
                "subject",
                subject == format!("Reapply \"{MAIN_C_AGAIN}\"")
                    || subject == format!("Revert \"Revert \"{MAIN_C_AGAIN}\"\""),
                subject,
            );
            k.ok("names the revert", c.message.contains(&format!("This reverts commit {revert}.")), &c.message);
            k.eq("parents", c.parents.clone(), vec![revert.to_string()]);
        }
        None => k.fail("no HEAD commit".into()),
    }
    k.eq("state", e.state(), OpState::Clean);
    let c = e.blobs(&["HEAD:c.txt"]).pop().flatten();
    k.text("HEAD:c.txt", c.as_deref(), Some(file("c", &[(8, C8_AGAIN)]).as_str()));
    k.eq("status", e.status_lines(), Vec::<String>::new());
    k.done()
}

/// RV-02: HEAD is `Revert "Merge branch 'topic'"` (one parent: `main`'s tip); d:9 is
/// back as in `init` (the merge undone against its first parent). Three git processes.
pub fn expect_merge_reverted(dir: &Path, s: &Setup) -> Result<(), String> {
    expect_one(
        dir,
        &s.main,
        &revert_merge_message(MERGE_TOPIC, &s.merge_topic, &s.main_c),
        &[("d.txt", 9, "d 9")],
        "RV-02 after reverting the merge",
    )
    .map(|_| ())
}

/// The text RV-03 resolves `c.txt` to: line 8 as `init` had it (what the revert of
/// [`MAIN_C`] wanted — its side of the conflict, "theirs").
pub const RESOLVED_C8: &str = "c 8";

/// RV-03: the revert of [`MAIN_C`], resolved to [`RESOLVED_C8`] and continued with
/// `message`. Three git processes.
pub fn expect_resolved(dir: &Path, s: &Setup, message: &str) -> Result<(), String> {
    expect_one(dir, &s.main, message, &[("c.txt", 8, RESOLVED_C8)], "RV-03 after Continue").map(|_| ())
}

/// RV-07: [`MAIN_C_AGAIN`], [`MERGE_TOPIC`] and [`MAIN_C`] reverted newest first — three
/// `Revert "…"` commits in that order on `main`'s tip; c.txt and d.txt as in `init`.
/// Three git processes.
pub fn expect_several_reverted(dir: &Path, s: &Setup) -> Result<(), String> {
    let e = Expect::new(dir);
    let mut k = Checks::new("RV-07 after reverting three commits");
    let log = e.log("HEAD", 3);
    let got: Vec<&str> = log.iter().rev().map(|c| c.message.as_str()).collect();
    let want = [
        revert_message(MAIN_C_AGAIN, &s.main_c_again),
        revert_merge_message(MERGE_TOPIC, &s.merge_topic, &s.main_c),
        revert_message(MAIN_C, &s.main_c),
    ];
    k.eq("messages, oldest first", got, want.iter().map(String::as_str).collect());
    k.eq("on main's tip", log.last().map(|c| c.parents.clone()), Some(vec![s.main.clone()]));
    k.eq("state", e.state(), OpState::Clean);
    let files = files_with(&[("c.txt", 8, "c 8"), ("d.txt", 9, "d 9")]);
    let specs: Vec<String> = files.iter().map(|(p, _)| format!("HEAD:{p}")).collect();
    let specs: Vec<&str> = specs.iter().map(String::as_str).collect();
    for ((p, want), got) in files.iter().zip(e.blobs(&specs)) {
        k.text(&format!("HEAD:{p}"), got.as_deref(), Some(want.as_str()));
    }
    k.eq("status", e.status_lines(), Vec::<String>::new());
    k.done()
}

/// **RV-08 — revert some lines of an older commit in the working tree.** `main`: `init`
/// (`f.txt`: [`F_LINES`] lines `f N`, `notes.txt`) → [`TUNE_F`] (lines 3–5 replaced by
/// `F 3`..`F 5`: a replaced block, and line 10 by `F 10`) → [`LATER`] (`notes.txt`). The
/// user shows [`TUNE_F`], selects the `-f 4` / `+F 4` pair of its hunk and chooses
/// "Revert These Lines in Working Tree".
///
/// Expected ([`expect_reverted`]): `f.txt` in the working tree is [`TUNE_F`]'s with line 4
/// back to `f 4` — in its place, between `F 3` and `F 5`; nothing staged, no commit made
/// (HEAD is [`LATER`]), the change shows as unstaged.
pub mod lines {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    /// The commit whose lines are reverted.
    pub const TUNE_F: &str = "Tune f";
    /// The commit after it (HEAD).
    pub const LATER: &str = "Later";
    /// Lines of `f.txt`.
    pub const F_LINES: usize = 12;
    /// The selected lines: the old line 4 and its new version.
    pub const OLD_4: &str = "f 4";
    pub const NEW_4: &str = "F 4";

    /// `f.txt` with the lines in `upper` as `F N`, the rest `f N`.
    pub fn f(upper: &[usize]) -> String {
        (1..=F_LINES)
            .map(|i| if upper.contains(&i) { format!("F {i}\n") } else { format!("f {i}\n") })
            .collect()
    }

    /// The commit ids the steps need.
    pub struct Setup {
        pub tune_f: String,
        pub later: String,
    }

    /// The history (see the module docs); clean working tree on `main`. Three git
    /// processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("f.txt", &f(&[])), put("notes.txt", "notes\n")]);
        let tune = fx.commit("main", TUNE_F, &[put("f.txt", &f(&[3, 4, 5, 10]))]);
        let later = fx.commit("main", LATER, &[put("notes.txt", "notes\nmore\n")]);
        let ids = fx.build(dir);
        Setup { tune_f: ids[tune].to_string(), later: ids[later].to_string() }
    }

    /// After the revert: `f.txt` in the working tree is [`TUNE_F`]'s with line 4 as before
    /// it; HEAD and the index unchanged ([`LATER`]); ` M f.txt` only. Two git processes.
    pub fn expect_reverted(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RV-08 after reverting line 4 in the working tree");
        k.eq("HEAD", e.ref_id("HEAD"), Some(s.later.clone()));
        k.eq("state", e.state(), OpState::Clean);
        let want = f(&[3, 5, 10]);
        k.text("f.txt", e.file("f.txt").as_deref(), Some(want.as_str()));
        let index = e.blobs(&[":f.txt"]).pop().flatten();
        k.text(":f.txt (index as HEAD)", index.as_deref(), Some(f(&[3, 4, 5, 10]).as_str()));
        k.eq("status", e.status_lines(), vec![" M f.txt".to_string()]);
        k.done()
    }
}
