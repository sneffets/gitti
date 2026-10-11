//! WT — worktrees: a commit made in a linked worktree, a remote branch checked out in a
//! new one.

use std::path::{Path, PathBuf};

use crate::fixture::{Fixture, Mark, put};

/// A linked worktree of `work` named `<work's name>-<suffix>`, next to it.
pub fn linked_of(work: &Path, suffix: &str) -> PathBuf {
    let name = work.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    work.with_file_name(format!("{name}-{suffix}"))
}

/// **WT-03 — stage and commit in a linked worktree.** `main` is checked out in the main
/// worktree; `feat` ([`FEATURE`]) in the linked worktree [`linked_of`]`(work, "feat")`,
/// where the user changed `a.txt` ([`EDIT`]). The user opens the linked worktree, stages
/// the change and commits [`WORK`].
///
/// Expected: `feat` — shared by both worktrees — is the new commit on [`FEATURE`], the
/// linked worktree clean; the main worktree untouched: `main` checked out, same commit,
/// clean ([`expect_committed`]).
pub mod commit_in_linked_worktree {
    use super::*;
    use crate::expect::{Checks, Expect, OpState};

    /// `feat`'s commit before the user's.
    pub const FEATURE: &str = "Feature";
    /// The user's commit in the linked worktree.
    pub const WORK: &str = "Work in the linked worktree";
    /// `a.txt` as the user changed it in the linked worktree.
    pub const EDIT: &str = "a\nedited in feat\n";

    pub struct Setup {
        pub init: String,
        pub feature: String,
        /// The linked worktree.
        pub linked: PathBuf,
    }

    /// `main` with `init` (`a.txt`), `feat` with [`FEATURE`] checked out in the linked
    /// worktree, `a.txt` edited there. Four git processes.
    pub fn fixture(work: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i: Mark = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let f = fx.commit_from("feat", i, FEATURE, &[put("f.txt", "feature\n")]);
        let linked = linked_of(work, "feat");
        let name = linked.file_name().unwrap().to_string_lossy().into_owned();
        fx.git_after(&["worktree", "add", "-q", &format!("../{name}"), "feat"]);
        let ids = fx.build(work);
        std::fs::write(linked.join("a.txt"), EDIT).unwrap();
        Setup { init: ids[i].into(), feature: ids[f].into(), linked }
    }

    /// After the commit: in the linked worktree `feat` checked out at [`WORK`] (parent
    /// [`FEATURE`], its `a.txt`), clean; the main worktree on `main` at `init`, its `a.txt`
    /// unchanged, clean, and it sees `feat` moved. Three git processes.
    pub fn expect_committed(work: &Path, s: &Setup) -> Result<(), String> {
        let l = Expect::new(&s.linked);
        let m = Expect::new(work);
        let mut k = Checks::new("WT-03 after the commit in the linked worktree");
        k.eq("linked: state", l.state(), OpState::Clean);
        k.eq("linked: branch", l.branch().as_deref(), Some("feat"));
        match l.log("HEAD", 1).first() {
            Some(head) => {
                k.eq("linked: message", head.message.as_str(), WORK);
                k.eq("linked: parents", head.parents.clone(), vec![s.feature.clone()]);
                k.eq("feat as the main worktree sees it", m.ref_id("feat"), Some(head.id.clone()));
            }
            None => k.fail("linked: no HEAD commit".into()),
        }
        k.text("linked: committed a.txt", l.blobs(&["HEAD:a.txt"])[0].as_deref(), Some(EDIT));
        k.eq("linked: status", l.status_lines(), Vec::<String>::new());
        k.eq("main worktree: branch", m.branch().as_deref(), Some("main"));
        k.eq("main worktree: main", m.ref_id("main"), Some(s.init.clone()));
        k.text("main worktree: a.txt", m.file("a.txt").as_deref(), Some("a\n"));
        k.eq("main worktree: status", m.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **WT-07 — track a remote branch in a new worktree.** `origin` has `topic` ([`TOPIC`]),
/// which the user has as `origin/topic` only. The user adds a worktree typing
/// `origin/topic` as the branch, folder [`linked_of`]`(work, "topic")`.
///
/// Expected: a local `topic` at `origin/topic`'s commit, tracking it, checked out in the new
/// worktree; the main worktree still on `main`, clean ([`expect_added`]).
pub mod track_remote_branch {
    use super::*;
    use crate::expect::{Checks, Expect};
    use crate::rm::origin_of;

    /// `topic`'s commit.
    pub const TOPIC: &str = "Topic";

    pub struct Setup {
        pub init: String,
        pub topic: String,
        /// Where the user puts the new worktree.
        pub linked: PathBuf,
    }

    /// Both sides: `main` = `init`, `topic` = [`TOPIC`] (the working repository as
    /// `origin/main`, `origin/topic`; `main` tracks `origin/main`). Four git processes.
    pub fn fixture(work: &Path) -> Setup {
        let history = |fx: &mut Fixture, topic: &str| {
            let i = fx.commit("main", "init", &[put("a.txt", "a\n")]);
            let t = fx.commit_from(topic, i, TOPIC, &[put("t.txt", "topic\n")]);
            (i, t)
        };
        let mut fx = Fixture::new();
        let (i, t) = history(&mut fx, "refs/remotes/origin/topic");
        fx.branch("refs/remotes/origin/main", i);
        fx.remote("origin", &origin_of(work));
        fx.upstream("main", "origin");
        let ids = fx.build(work);
        let mut o = Fixture::new();
        history(&mut o, "topic");
        o.build_bare(&origin_of(work));
        Setup { init: ids[i].into(), topic: ids[t].into(), linked: linked_of(work, "topic") }
    }

    /// After Add Worktree: the new worktree on `topic` at [`TOPIC`], its `t.txt` there,
    /// clean; `topic` tracks `origin/topic`; the main worktree on `main` at `init`, clean.
    /// Two git processes.
    pub fn expect_added(work: &Path, s: &Setup) -> Result<(), String> {
        let mut k = Checks::new("WT-07 after adding the worktree");
        let m = Expect::new(work);
        if !s.linked.join(".git").is_file() {
            k.fail(format!("no worktree at {}", s.linked.display()));
            return k.done();
        }
        let l = Expect::new(&s.linked);
        k.eq("new worktree: branch", l.branch().as_deref(), Some("topic"));
        k.eq("topic", m.ref_id("topic"), Some(s.topic.clone()));
        k.text("new worktree: t.txt", l.file("t.txt").as_deref(), Some("topic\n"));
        k.eq("new worktree: status", l.status_lines(), Vec::<String>::new());
        let config = std::fs::read_to_string(m.git_dir().join("config")).unwrap_or_default();
        k.ok(
            "topic tracks origin/topic",
            config.contains("[branch \"topic\"]\n\tremote = origin\n\tmerge = refs/heads/topic\n"),
            &config,
        );
        k.eq("main worktree: branch", m.branch().as_deref(), Some("main"));
        k.eq("main worktree: main", m.ref_id("main"), Some(s.init.clone()));
        k.eq("main worktree: status", m.status_lines(), Vec::<String>::new());
        k.done()
    }
}
