//! RM — remotes, each against a local bare `origin` next to the working repository
//! ([`origin_of`]): fetch and pull what someone else pushed, pulls that stop on a conflict
//! or are refused, a force push whose lease no longer holds, pull from another remote and
//! prune, delete a remote branch, cancel a fetch that hangs.
//!
//! Both repositories come from fixtures: the shared history is written into each by the
//! same fast-import stream (same times, same authors — the same ids), so the working
//! repository's `origin/main` and the remote's `main` agree where they should without a
//! clone or a fetch. "Someone else pushed" is a commit only the remote's fixture has. Four
//! git processes for both (three for the working repository, one for the remote).

use std::fs;
use std::path::{Path, PathBuf};

use crate::cp::lines;
use crate::fixture::{Fixture, Mark, put};

/// The author of what "someone else" pushed.
pub const OTHER: &str = "Other <other@example.com>";
/// The first commit, on both sides.
pub const INIT: &str = "init";
/// The commit someone else pushed to `origin`.
pub const THEIRS: &str = "Theirs";
/// The user's own commit, not pushed yet.
pub const MINE: &str = "Mine";

/// The bare remote of the working repository `work`: `<work>-origin.git` next to it.
pub fn origin_of(work: &Path) -> PathBuf {
    let name = work.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    work.with_file_name(format!("{name}-origin.git"))
}

/// A second bare remote of `work`, `<work>-<name>.git` next to it.
pub fn remote_of(work: &Path, name: &str) -> PathBuf {
    let base = work.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    work.with_file_name(format!("{base}-{name}.git"))
}

/// `a.txt`: 10 lines `a N`, line 3 replaced by `line3`.
pub fn a(line3: Option<&str>) -> String {
    lines("a", 10, 3, line3)
}

/// `b.txt`: 10 lines `b N`, line 5 replaced by `line5`.
pub fn b(line5: Option<&str>) -> String {
    lines("b", 10, 5, line5)
}

/// The commit both sides start from: [`INIT`] with `a.txt` and `b.txt`.
fn init(fx: &mut Fixture) -> Mark {
    fx.commit("main", INIT, &[put("a.txt", &a(None)), put("b.txt", &b(None))])
}

/// `origin` as the working repository's remote (the bare repository at
/// [`origin_of`]`(work)`), `main` tracking `origin/main`.
fn tracks_origin(fx: &mut Fixture, work: &Path) {
    fx.remote("origin", &origin_of(work));
    fx.upstream("main", "origin");
}

/// The id `branch` has in the bare repository `bare` (from the files, no git): loose ref
/// first, then `packed-refs`. `None` = no such branch.
pub fn remote_ref(bare: &Path, branch: &str) -> Option<String> {
    let full = format!("refs/heads/{branch}");
    if let Ok(id) = fs::read_to_string(bare.join(&full)) {
        return Some(id.trim().to_string());
    }
    let packed = fs::read_to_string(bare.join("packed-refs")).ok()?;
    packed.lines().find_map(|l| {
        let (id, name) = l.split_once(' ')?;
        (name == full).then(|| id.to_string())
    })
}

/// **RM-02, RM-03 — someone else pushed: Fetch shows it, Pull fast-forwards.** The user's
/// `main` is at [`INIT`], like `origin/main` as last fetched; someone else pushed
/// [`THEIRS`] (by [`OTHER`], `b.txt` line 5) to `origin` since. The user has an
/// uncommitted edit of `a.txt` (line 1, [`LOCAL`]). Fetch, then Pull.
///
/// Expected: after the fetch `origin/main` is [`THEIRS`], `main` and the working tree
/// unchanged ([`expect_fetched`]); after the pull `main` is fast-forwarded to
/// [`THEIRS`] — no merge commit — with its `b.txt`, the local edit kept
/// ([`expect_pulled`]).
pub mod fetch_and_pull {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect, OpState};

    /// What [`THEIRS`] makes of line 5 of `b.txt`.
    pub const THEIR_LINE: &str = "b 5 theirs";
    /// The user's uncommitted line 1 of `a.txt`.
    pub const LOCAL: &str = "a 1 local";

    pub struct Setup {
        pub init: String,
        pub theirs: String,
    }

    /// `a.txt` with the local edit.
    pub fn a_local() -> String {
        lines("a", 10, 1, Some(LOCAL))
    }

    /// The working repository at `work` and its `origin` (see the module docs). Four git
    /// processes.
    pub fn fixture(work: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = init(&mut fx);
        fx.branch("refs/remotes/origin/main", i);
        tracks_origin(&mut fx, work);
        fx.write("a.txt", &a_local());
        let ids = fx.build(work);
        let mut o = Fixture::new();
        init(&mut o);
        o.author(OTHER);
        let t = o.commit("main", THEIRS, &[put("b.txt", &b(Some(THEIR_LINE)))]);
        let oids = o.build_bare(&origin_of(work));
        Setup { init: ids[i].into(), theirs: oids[t].into() }
    }

    /// After the fetch: `origin/main` = [`THEIRS`]; `main` still [`INIT`]; the local edit
    /// the only change. One git process.
    pub fn expect_fetched(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-02 after the fetch");
        k.eq("origin/main", e.ref_id("refs/remotes/origin/main"), Some(s.theirs.clone()));
        k.eq("main", e.ref_id("main"), Some(s.init.clone()));
        k.eq("branch", e.branch().as_deref(), Some("main"));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&a_local()));
        k.text("b.txt", e.file("b.txt").as_deref(), Some(&b(None)));
        k.eq("status", e.status_lines(), vec![" M a.txt".to_string()]);
        k.done()
    }

    /// After the pull: `main` = [`THEIRS`] itself (fast-forward, no merge commit), its
    /// `b.txt` on disk; the local edit kept; nothing in progress. One git process.
    pub fn expect_pulled(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-03 after the pull");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        k.eq("main (fast-forwarded)", e.ref_id("main"), Some(s.theirs.clone()));
        k.eq("origin/main", e.ref_id("refs/remotes/origin/main"), Some(s.theirs.clone()));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&a_local()));
        k.text("b.txt", e.file("b.txt").as_deref(), Some(&b(Some(THEIR_LINE))));
        k.eq("status", e.status_lines(), vec![" M a.txt".to_string()]);
        k.done()
    }
}

/// **RM-05, RM-06 — a pull that conflicts.** The user committed [`MINE`] (line 3 of
/// `a.txt` = [`MY_LINE`]); someone else pushed [`THEIRS`] to `origin`, changing the same
/// line ([`THEIR_LINE`]) and line 5 of `b.txt`. `origin/main` was last fetched at
/// [`INIT`]. The user pulls with Merge (RM-05) or Rebase (RM-06); git stops on `a.txt`.
/// The user edits it to [`RESOLVED`] (the editor: another program), stages it and
/// commits (merge) or continues (rebase).
///
/// Expected: the merge stops with `a.txt` conflicted and `b.txt` merged and staged
/// ([`expect_merge_stopped`]); the commit concludes it — two parents, git's pull message,
/// the resolution ([`expect_merged`]). The rebase stops on [`MINE`] with HEAD at
/// [`THEIRS`] ([`expect_rebase_stopped`]); Continue replays [`MINE`] on top of
/// [`THEIRS`] with the resolution, linear history ([`expect_rebased`]); Abort brings
/// `main` back to [`MINE`] ([`expect_rebase_aborted`]).
pub mod pull_conflict {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect, OpState, conflicted};

    /// [`MINE`]'s line 3 of `a.txt`.
    pub const MY_LINE: &str = "a 3 mine";
    /// [`THEIRS`]'s line 3 of `a.txt`.
    pub const THEIR_LINE: &str = "a 3 theirs";
    /// [`THEIRS`]'s line 5 of `b.txt` (no conflict).
    pub const THEIR_B: &str = "b 5 theirs";
    /// Line 3 of `a.txt` as the user resolves it.
    pub const RESOLVED: &str = "a 3 mine and theirs";

    pub struct Setup {
        pub init: String,
        pub mine: String,
        pub theirs: String,
    }

    /// `a.txt` as the user resolves it.
    pub fn resolved_a() -> String {
        a(Some(RESOLVED))
    }

    /// The working repository at `work` (`main` = [`MINE`], `origin/main` = [`INIT`]) and
    /// its `origin` (`main` = [`THEIRS`]). Four git processes.
    pub fn fixture(work: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = init(&mut fx);
        fx.branch("refs/remotes/origin/main", i);
        let m = fx.commit("main", MINE, &[put("a.txt", &a(Some(MY_LINE)))]);
        tracks_origin(&mut fx, work);
        let ids = fx.build(work);
        let mut o = Fixture::new();
        init(&mut o);
        o.author(OTHER);
        let t = o.commit(
            "main",
            THEIRS,
            &[put("a.txt", &a(Some(THEIR_LINE))), put("b.txt", &b(Some(THEIR_B)))],
        );
        let oids = o.build_bare(&origin_of(work));
        Setup { init: ids[i].into(), mine: ids[m].into(), theirs: oids[t].into() }
    }

    /// The state a pull that stopped leaves — the second half of the story without its
    /// fetch: the working repository with [`THEIRS`] fetched into `origin/main` and the
    /// merge (`rebase` = false) or rebase (`true`) stopped on `a.txt`, as `git pull`
    /// leaves it (the merge with git's pull message). Four git processes; no remote is
    /// written (the steps from here on don't reach it).
    pub fn fixture_stopped(work: &Path, rebase: bool) -> Setup {
        let mut fx = Fixture::new();
        let i = init(&mut fx);
        let m = fx.commit("main", MINE, &[put("a.txt", &a(Some(MY_LINE)))]);
        fx.author(OTHER);
        let t = fx.commit_from(
            "refs/remotes/origin/main",
            i,
            THEIRS,
            &[put("a.txt", &a(Some(THEIR_LINE))), put("b.txt", &b(Some(THEIR_B)))],
        );
        fx.author(crate::fixture::TESTER);
        tracks_origin(&mut fx, work);
        if rebase {
            fx.git_after_stops(&["rebase", "origin/main"]);
        } else {
            let url = origin_of(work).display().to_string().replace('\\', "/");
            let msg = format!("Merge branch 'main' of {url}");
            fx.git_after_stops(&["merge", "--no-ff", "-m", &msg, "origin/main"]);
        }
        let ids = fx.build(work);
        Setup { init: ids[i].into(), mine: ids[m].into(), theirs: ids[t].into() }
    }

    /// `a.txt` on disk has conflict markers around line 3: `ours` above, `theirs` below.
    fn markers(k: &mut Checks, e: &Expect, ours: &str, theirs: &str) {
        let text = e.file("a.txt").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let at = |s: &str| text.find(s);
        let ok = matches!(
            (at("<<<<<<< "), at(&format!("\n{ours}\n=======\n{theirs}\n>>>>>>> "))),
            (Some(a), Some(b)) if a < b
        );
        k.ok("a.txt has the conflict markers around line 3", ok, &text);
    }

    /// After Pull with Merge stopped: merging [`THEIRS`] into [`MINE`]; only `a.txt`
    /// conflicted (ours [`MY_LINE`], theirs [`THEIR_LINE`]); `b.txt` merged and staged.
    /// Two git processes.
    pub fn expect_merge_stopped(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-05 after the pull stopped");
        k.eq("state", e.state(), OpState::Merging);
        k.eq("HEAD", e.ref_id("HEAD"), Some(s.mine.clone()));
        k.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.theirs.clone()));
        let status = e.status();
        k.eq("conflicted", conflicted(&status), vec!["a.txt".to_string()]);
        let mut l: Vec<String> = status.iter().map(|s| format!("{} {}", s.xy, s.path)).collect();
        l.sort();
        k.eq("status", l, vec!["M  b.txt".to_string(), "UU a.txt".to_string()]);
        markers(&mut k, &e, MY_LINE, THEIR_LINE);
        let blobs = e.blobs(&[":b.txt", ":2:a.txt", ":3:a.txt"]);
        k.text("b.txt staged", blobs[0].as_deref(), Some(&b(Some(THEIR_B))));
        k.text("ours a.txt", blobs[1].as_deref(), Some(&a(Some(MY_LINE))));
        k.text("theirs a.txt", blobs[2].as_deref(), Some(&a(Some(THEIR_LINE))));
        k.done()
    }

    /// After the commit: no merge in progress; HEAD a merge of [`MINE`] and [`THEIRS`]
    /// with git's pull message ("Merge branch 'main' of …"); the resolution and
    /// [`THEIRS`]'s `b.txt` committed; clean. Three git processes.
    pub fn expect_merged(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-05 after the merge was committed");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        match e.log("HEAD", 1).first() {
            Some(head) => {
                k.eq("parents", head.parents.clone(), vec![s.mine.clone(), s.theirs.clone()]);
                k.ok(
                    "git's pull message",
                    head.subject().starts_with("Merge branch 'main' of "),
                    head.subject(),
                );
                k.ok("no conflict comments", !head.message.contains("Conflicts"), &head.message);
            }
            None => k.fail("no HEAD commit".into()),
        }
        let blobs = e.blobs(&["HEAD:a.txt", "HEAD:b.txt"]);
        k.text("a.txt", blobs[0].as_deref(), Some(&resolved_a()));
        k.text("b.txt", blobs[1].as_deref(), Some(&b(Some(THEIR_B))));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }

    /// After Pull with Rebase stopped: rebasing, HEAD (detached) at [`THEIRS`], `main`
    /// still [`MINE`]; `a.txt` conflicted (ours = upstream's [`THEIR_LINE`], theirs = the
    /// replayed [`MY_LINE`]). One git process.
    pub fn expect_rebase_stopped(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-06 after the pull stopped");
        k.eq("state", e.state(), OpState::Rebasing);
        k.eq("HEAD (detached at theirs)", e.ref_id("HEAD"), Some(s.theirs.clone()));
        k.eq("main", e.ref_id("main"), Some(s.mine.clone()));
        k.eq("conflicted", conflicted(&e.status()), vec!["a.txt".to_string()]);
        markers(&mut k, &e, THEIR_LINE, MY_LINE);
        k.done()
    }

    /// After Continue: nothing in progress; `main` is [`MINE`] replayed on [`THEIRS`] (one
    /// parent, same message and author, the resolution) — linear; clean. Three git
    /// processes.
    pub fn expect_rebased(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-06 after Continue");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        match e.log("HEAD", 1).first() {
            Some(head) => {
                k.eq("message", head.message.as_str(), MINE);
                k.eq("author", head.author.as_str(), crate::fixture::TESTER);
                k.eq("parents", head.parents.clone(), vec![s.theirs.clone()]);
                k.ok("a new commit", head.id != s.mine, &head.id);
            }
            None => k.fail("no HEAD commit".into()),
        }
        let blobs = e.blobs(&["HEAD:a.txt", "HEAD:b.txt"]);
        k.text("a.txt", blobs[0].as_deref(), Some(&resolved_a()));
        k.text("b.txt", blobs[1].as_deref(), Some(&b(Some(THEIR_B))));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }

    /// After Abort: nothing in progress; `main` (checked out) back at [`MINE`]; clean.
    /// One git process.
    pub fn expect_rebase_aborted(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-06 after Abort");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        k.eq("main", e.ref_id("main"), Some(s.mine.clone()));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&a(Some(MY_LINE))));
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **RM-07 — pull into a dirty tree.** Someone else pushed [`THEIRS`], changing line 3
/// of `a.txt`; the user has an uncommitted edit of the same file ([`LOCAL`]) and pulls.
///
/// Expected: git refuses — the edit would be overwritten — and nothing the user has
/// changes: `main` still [`INIT`], the edit still on disk, nothing in progress
/// ([`expect_refused`]). The apps say so in plain words ([`REASON`]).
pub mod pull_dirty {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect, OpState};

    /// The user's uncommitted line 3 of `a.txt`.
    pub const LOCAL: &str = "a 3 local";
    /// [`THEIRS`]'s line 3 of `a.txt`.
    pub const THEIR_LINE: &str = "a 3 theirs";
    /// What git's refusal says — the words the apps must show.
    pub const REASON: &str = "would be overwritten";

    pub struct Setup {
        pub init: String,
        pub theirs: String,
    }

    /// `main` = `origin/main` = [`INIT`], `a.txt` edited; `origin` has [`THEIRS`]. Four
    /// git processes.
    pub fn fixture(work: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = init(&mut fx);
        fx.branch("refs/remotes/origin/main", i);
        tracks_origin(&mut fx, work);
        fx.write("a.txt", &a(Some(LOCAL)));
        let ids = fx.build(work);
        let mut o = Fixture::new();
        init(&mut o);
        o.author(OTHER);
        let t = o.commit("main", THEIRS, &[put("a.txt", &a(Some(THEIR_LINE)))]);
        let oids = o.build_bare(&origin_of(work));
        Setup { init: ids[i].into(), theirs: oids[t].into() }
    }

    /// After the refused pull: `main` = [`INIT`], nothing in progress, the local edit on
    /// disk and the only change. One git process.
    pub fn expect_refused(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-07 after the refused pull");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("branch", e.branch().as_deref(), Some("main"));
        k.eq("main", e.ref_id("main"), Some(s.init.clone()));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&a(Some(LOCAL))));
        k.eq("status", e.status_lines(), vec![" M a.txt".to_string()]);
        k.done()
    }
}

/// **RM-10 — force with lease, the remote moved since.** The user pushed [`MINE`], then
/// amended it ([`AMENDED`]); `origin/main` (last fetched) is [`MINE`]. Meanwhile someone
/// else pushed [`THEIRS`] on top of [`MINE`]. The user force-pushes with lease.
///
/// Expected: the app lists what `origin/main` loses as it last saw it ([`MINE`]) and asks;
/// the push is refused — the lease is pinned to [`MINE`], the remote moved — and nothing
/// is overwritten: `origin`'s `main` still [`THEIRS`], the local `main` [`AMENDED`]
/// ([`expect_refused`]).
pub mod force_with_lease {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect};

    /// The amended commit.
    pub const AMENDED: &str = "Mine, amended";

    pub struct Setup {
        pub mine: String,
        pub amended: String,
        pub theirs: String,
    }

    /// `main` = [`AMENDED`] (on [`INIT`]), `origin/main` = [`MINE`]; `origin`'s `main` =
    /// [`THEIRS`] on [`MINE`]. Four git processes.
    pub fn fixture(work: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = init(&mut fx);
        let m = fx.commit("main", MINE, &[put("a.txt", &a(Some("a 3 mine")))]);
        fx.branch("refs/remotes/origin/main", m);
        let am = fx.commit_from("main", i, AMENDED, &[put("a.txt", &a(Some("a 3 mine, amended")))]);
        tracks_origin(&mut fx, work);
        let ids = fx.build(work);
        let mut o = Fixture::new();
        init(&mut o);
        o.commit("main", MINE, &[put("a.txt", &a(Some("a 3 mine")))]);
        o.author(OTHER);
        let t = o.commit("main", THEIRS, &[put("b.txt", &b(Some("b 5 theirs")))]);
        let oids = o.build_bare(&origin_of(work));
        Setup { mine: ids[m].into(), amended: ids[am].into(), theirs: oids[t].into() }
    }

    /// After the refused push: `origin`'s `main` still [`THEIRS`]; local `main`
    /// [`AMENDED`]. No git process.
    pub fn expect_refused(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-10 after the refused force push");
        k.eq("origin's main", remote_ref(&origin_of(work), "main"), Some(s.theirs.clone()));
        k.eq("main", e.ref_id("main"), Some(s.amended.clone()));
        k.eq("branch", e.branch().as_deref(), Some("main"));
        k.done()
    }
}

/// **RM-12 — pull from another remote; prune.** The user's `main` tracks `origin`; a
/// second remote, `upstream` ([`remote_of`]`(work, "upstream")`), got [`FIX`] (by
/// [`OTHER`]). `origin` deleted its branch `old`, which the user still has as
/// `origin/old`. The user pulls from `upstream`, then prunes `origin`.
///
/// Expected: `main` fast-forwarded to `upstream`'s [`FIX`] ([`expect_pulled`]); after the
/// prune `origin/old` is gone, `origin/main` stays ([`expect_pruned`]).
pub mod pull_from_and_prune {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect, OpState};

    /// `upstream`'s commit.
    pub const FIX: &str = "Upstream fix";
    /// What [`FIX`] makes of line 5 of `b.txt`.
    pub const FIX_LINE: &str = "b 5 upstream";

    pub struct Setup {
        pub init: String,
        pub fix: String,
    }

    /// `main` = `origin/main` = `origin/old` = `upstream/main` = [`INIT`]; `origin` has
    /// only `main`; `upstream`'s `main` is [`FIX`]. Five git processes.
    pub fn fixture(work: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = init(&mut fx);
        fx.branch("refs/remotes/origin/main", i);
        fx.branch("refs/remotes/origin/old", i);
        fx.branch("refs/remotes/upstream/main", i);
        tracks_origin(&mut fx, work);
        fx.remote("upstream", &remote_of(work, "upstream"));
        let ids = fx.build(work);
        let mut o = Fixture::new();
        init(&mut o);
        o.build_bare(&origin_of(work));
        o.author(OTHER);
        let f = o.commit("main", FIX, &[put("b.txt", &b(Some(FIX_LINE)))]);
        let oids = o.build_bare(&remote_of(work, "upstream"));
        Setup { init: ids[i].into(), fix: oids[f].into() }
    }

    /// After Pull from upstream: `main` = [`FIX`] (fast-forward), its `b.txt` on disk;
    /// `upstream/main` = [`FIX`]; the upstream setting unchanged (`origin`); clean. One
    /// git process.
    pub fn expect_pulled(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-12 after Pull from upstream");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("main", e.ref_id("main"), Some(s.fix.clone()));
        k.eq("upstream/main", e.ref_id("refs/remotes/upstream/main"), Some(s.fix.clone()));
        k.text("b.txt", e.file("b.txt").as_deref(), Some(&b(Some(FIX_LINE))));
        let config = std::fs::read_to_string(e.git_dir().join("config")).unwrap_or_default();
        k.ok("main still tracks origin", config.contains("[branch \"main\"]\n\tremote = origin\n"), &config);
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }

    /// After Prune Stale Branches of origin: `origin/old` gone; `origin/main` kept. No git
    /// process.
    pub fn expect_pruned(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-12 after the prune");
        k.eq("origin/old", e.ref_id("refs/remotes/origin/old"), None);
        k.eq("origin/main", e.ref_id("refs/remotes/origin/main"), Some(s.init.clone()));
        k.done()
    }
}

/// **RM-13 — delete a remote branch.** `origin` has `feat` ([`FEATURE`]), which the user
/// sees as `origin/feat` (no local `feat`). The user deletes it on the remote.
///
/// Expected: `feat` gone on `origin`, `origin/feat` gone locally; `main` on both sides
/// unchanged ([`expect_deleted`]).
pub mod delete_remote_branch {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect};

    /// `feat`'s commit.
    pub const FEATURE: &str = "Feature";

    pub struct Setup {
        pub init: String,
        pub feature: String,
    }

    /// Both sides: `main` = [`INIT`], `feat` = [`FEATURE`] (the working repository as
    /// `origin/main`, `origin/feat`). Four git processes.
    pub fn fixture(work: &Path) -> Setup {
        // The same commits on both sides; locally `feat` is only the remote-tracking branch.
        let history = |fx: &mut Fixture, feat: &str| {
            let i = init(fx);
            let f = fx.commit_from(feat, i, FEATURE, &[put("f.txt", "feature\n")]);
            (i, f)
        };
        let mut fx = Fixture::new();
        let (i, f) = history(&mut fx, "refs/remotes/origin/feat");
        fx.branch("refs/remotes/origin/main", i);
        tracks_origin(&mut fx, work);
        let ids = fx.build(work);
        let mut o = Fixture::new();
        history(&mut o, "feat");
        o.build_bare(&origin_of(work));
        Setup { init: ids[i].into(), feature: ids[f].into() }
    }

    /// After the delete: `origin` has no `feat`, its `main` unchanged; `origin/feat` gone,
    /// `origin/main` and `main` unchanged. No git process.
    pub fn expect_deleted(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let origin = origin_of(work);
        let mut k = Checks::new("RM-13 after deleting origin/feat");
        k.eq("origin's feat", remote_ref(&origin, "feat"), None);
        k.eq("origin's main", remote_ref(&origin, "main"), Some(s.init.clone()));
        k.eq("origin/feat", e.ref_id("refs/remotes/origin/feat"), None);
        k.eq("origin/main", e.ref_id("refs/remotes/origin/main"), Some(s.init.clone()));
        k.eq("main", e.ref_id("main"), Some(s.init.clone()));
        k.done()
    }
}

/// **RM-15 — cancel a fetch that hangs.** `origin`'s server side never answers (its
/// `uploadpack` sleeps first: a remote that hangs). The user fetches, then cancels.
///
/// Expected: stopped quickly, the app idle again; no ref changed, nothing in progress, no
/// lock left ([`expect_unchanged`]). Unix only (the sleeping upload-pack is a shell
/// command).
pub mod cancel_fetch {
    use std::path::Path;

    use super::*;
    use crate::expect::{Checks, Expect, OpState};

    pub struct Setup {
        pub init: String,
    }

    /// `main` = `origin/main` = [`INIT`]; `origin` has [`THEIRS`], but its upload-pack
    /// sleeps 30 s before it starts. Four git processes.
    pub fn fixture(work: &Path) -> Setup {
        let mut fx = Fixture::new();
        let i = init(&mut fx);
        fx.branch("refs/remotes/origin/main", i);
        tracks_origin(&mut fx, work);
        fx.config("[remote \"origin\"]\n\tuploadpack = \"sleep 30; git-upload-pack\"\n");
        let ids = fx.build(work);
        let mut o = Fixture::new();
        init(&mut o);
        o.author(OTHER);
        o.commit("main", THEIRS, &[put("b.txt", &b(Some("b 5 theirs")))]);
        o.build_bare(&origin_of(work));
        Setup { init: ids[i].into() }
    }

    /// After the cancel: `origin/main` and `main` still [`INIT`]; nothing in progress; no
    /// `index.lock`, no `FETCH_HEAD` from it; clean. One git process.
    pub fn expect_unchanged(work: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new("RM-15 after the cancelled fetch");
        k.eq("state", e.state(), OpState::Clean);
        k.eq("origin/main", e.ref_id("refs/remotes/origin/main"), Some(s.init.clone()));
        k.eq("main", e.ref_id("main"), Some(s.init.clone()));
        k.ok("no index.lock", !e.git_dir().join("index.lock").exists(), "index.lock left");
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}
