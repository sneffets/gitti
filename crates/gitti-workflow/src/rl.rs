//! RL — undo and recovery through the reflog (RL-04, undoing a rebase, starts from
//! [`crate::rb`]).

/// **RL-03 — undo an amend.** `main`: `init` → [`NOTES`] (adds `n.txt`: [`ONE`]); then
/// amended — the same message, `n.txt` [`ONE_TWO`] — so the first [`NOTES`] is only in
/// the reflog. The user opens HEAD's reflog, takes the entry of the commit before the
/// amend ([`BEFORE_AMEND_ENTRY`]) and resets `main` there, soft.
///
/// Expected ([`expect`]): `main` at the commit before the amend; what the amend added
/// staged (nothing lost); nothing in progress.
pub mod undo_amend {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const NOTES: &str = "Add notes";
    pub const ONE: &str = "one\n";
    pub const ONE_TWO: &str = "one\ntwo\n";
    /// HEAD's reflog as the views list it, newest first: the entry of the commit before
    /// the amend.
    pub const BEFORE_AMEND_ENTRY: usize = 1;

    /// The ids the steps need.
    pub struct Setup {
        pub init: String,
        /// [`NOTES`] before the amend.
        pub before: String,
        /// … and after it.
        pub amended: Option<String>,
    }

    /// `main`: `init` → [`NOTES`], HEAD's and `main`'s reflogs as git writes them; `n.txt`
    /// [`ONE_TWO`] staged — where the amend starts. Three git processes.
    pub fn fixture_before(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let before = fx.commit("main", NOTES, &[put("n.txt", ONE)]);
        fx.stage(&[put("n.txt", ONE_TWO)]);
        let ids = fx.build(dir);
        let s = Setup { init: ids[init].into(), before: ids[before].into(), amended: None };
        let log = [("", s.init.as_str(), "commit (initial): init"), (&s.init, &s.before, "commit: Add notes")];
        crate::rs::write_reflog(dir, "HEAD", &log);
        crate::rs::write_reflog(dir, "refs/heads/main", &log);
        s
    }

    /// After the amend (written as git writes it): `main` at the amended [`NOTES`], the
    /// first one only in the reflogs; clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let before = fx.commit("main", NOTES, &[put("n.txt", ONE)]);
        let amended = fx.commit_from("main", init, NOTES, &[put("n.txt", ONE_TWO)]);
        let ids = fx.build(dir);
        let s = Setup { init: ids[init].into(), before: ids[before].into(), amended: Some(ids[amended].into()) };
        let amended = s.amended.as_deref().unwrap();
        let log = [
            ("", s.init.as_str(), "commit (initial): init"),
            (&s.init, &s.before, "commit: Add notes"),
            (&s.before, amended, "commit (amend): Add notes"),
        ];
        crate::rs::write_reflog(dir, "HEAD", &log);
        crate::rs::write_reflog(dir, "refs/heads/main", &log);
        s
    }

    /// The amended state: `main` a commit [`NOTES`] on `init` with [`ONE_TWO`], not the
    /// first one; clean. Two git processes.
    pub fn expect_amended(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RL-03 after the amend");
        match e.log("main", 1).first() {
            Some(c) => {
                k.eq("message", c.message.as_str(), NOTES);
                k.eq("parents", c.parents.clone(), vec![s.init.clone()]);
                k.ok("a new commit", c.id != s.before, &c.id);
            }
            None => k.fail("no commit".into()),
        }
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }

    /// After the soft reset to the commit before the amend: `main` there, `n.txt`
    /// [`ONE_TWO`] staged (what the amend added) and on disk; nothing in progress. Two git
    /// processes.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RL-03 after undoing the amend");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        k.eq("main", e.ref_id("main"), Some(s.before.clone()));
        k.eq("status", e.status_lines(), vec!["M  n.txt".to_string()]);
        k.text("n.txt staged", e.index("n.txt").as_deref(), Some(ONE_TWO));
        k.text("n.txt", e.file("n.txt").as_deref(), Some(ONE_TWO));
        k.done()
    }
}
