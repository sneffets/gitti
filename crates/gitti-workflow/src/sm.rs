//! SM — submodules: a commit made inside a submodule, a commit of it checked out, a
//! submodule with local work removed, and a superproject cloned with and without its
//! submodules.
//!
//! The superproject [`superproject`] builds is laid out as `git submodule add` leaves it,
//! without a clone: the submodule `lib` at [`PATH`], its git directory absorbed into
//! `.git/modules/libs/lib` (`core.worktree` back to the checkout), `.gitmodules` and the
//! `submodule.*` config, `lib`'s remote a bare repository next to the superproject
//! ([`lib_remote`]). Seven git processes: the bare remote, the superproject, the
//! submodule's checkout (each a fixture).

use std::fs;
use std::path::{Path, PathBuf};

use crate::fixture::{Fixture, Mark, gitlink, put};

/// The submodule's path in the superproject.
pub const PATH: &str = "libs/lib";
/// `lib`'s first commit.
pub const LIB_ONE: &str = "lib one";
/// `lib`'s second commit: the one the superproject records.
pub const LIB_TWO: &str = "lib two";
/// The superproject's commit that added the submodule.
pub const ADD_LIB: &str = "Add lib";

/// `lib`'s bare remote: `<work>-lib.git` next to the superproject `work`.
pub fn lib_remote(work: &Path) -> PathBuf {
    crate::rm::remote_of(work, "lib")
}

/// The submodule's checkout in `work`.
pub fn sub(work: &Path) -> PathBuf {
    work.join(PATH)
}

/// `lib`'s `l.txt` after `n` commits (`1\n` … `n\n`).
pub fn l_txt(n: usize) -> String {
    (1..=n).map(|i| format!("{i}\n")).collect()
}

/// `lib`'s history: [`LIB_ONE`], [`LIB_TWO`] on `branch`.
fn lib_history(fx: &mut Fixture, branch: &str) -> (Mark, Mark) {
    let one = fx.commit(branch, LIB_ONE, &[put("l.txt", &l_txt(1))]);
    let two = fx.commit(branch, LIB_TWO, &[put("l.txt", &l_txt(2))]);
    (one, two)
}

/// A path as git config / `.gitmodules` take it (forward slashes).
fn slash(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// The ids of the superproject's commits and the submodule's.
pub struct Super {
    /// [`ADD_LIB`].
    pub add_lib: String,
    pub lib_one: String,
    pub lib_two: String,
}

/// The superproject at `work` with `lib` at [`PATH`] (see the module docs), recording and
/// checked out at [`LIB_TWO`] on `lib`'s `main`; `s.txt` and `.gitmodules` committed in
/// [`ADD_LIB`]. Clean. `superproject_config` is appended to the superproject's config.
pub fn superproject(work: &Path, superproject_config: &str) -> Super {
    superproject_with(work, superproject_config, &[])
}

/// [`superproject`] with `more` commits made inside the submodule's checkout on top of
/// [`LIB_TWO`] (message, `l.txt`), not recorded in the superproject.
pub fn superproject_with(work: &Path, superproject_config: &str, more: &[(&str, &str)]) -> Super {
    let remote = lib_remote(work);
    let mut l = Fixture::new();
    lib_history(&mut l, "main");
    let lib = l.build_bare(&remote);
    let (one, two) = (lib[Mark(1)].to_string(), lib[Mark(2)].to_string());
    let url = slash(&remote);
    let mut fx = Fixture::new();
    let add = fx.commit(
        "main",
        ADD_LIB,
        &[
            put("s.txt", "s\n"),
            put(".gitmodules", &format!("[submodule \"{PATH}\"]\n\tpath = {PATH}\n\turl = {url}\n")),
            gitlink(PATH, &two),
        ],
    );
    fx.config(&format!("[submodule \"{PATH}\"]\n\tactive = true\n\turl = {url}\n{superproject_config}"));
    let ids = fx.build(work);
    // The submodule's checkout, then its git directory absorbed as `submodule add` does.
    let checkout = sub(work);
    let mut s = Fixture::new();
    let (_, t) = lib_history(&mut s, "main");
    s.branch("refs/remotes/origin/main", t);
    for (msg, text) in more {
        s.commit("main", msg, &[put("l.txt", text)]);
    }
    s.remote("origin", &remote);
    s.upstream("main", "origin");
    s.build(&checkout);
    let modules = work.join(".git/modules").join(PATH);
    fs::create_dir_all(modules.parent().unwrap()).unwrap();
    fs::rename(checkout.join(".git"), &modules).unwrap();
    fs::write(checkout.join(".git"), "gitdir: ../../.git/modules/libs/lib\n").unwrap();
    let mut config = fs::read_to_string(modules.join("config")).unwrap();
    config.push_str("[core]\n\tworktree = ../../../../libs/lib\n");
    fs::write(modules.join("config"), config).unwrap();
    Super { add_lib: ids[add].into(), lib_one: one, lib_two: two }
}

/// The commit `rev` of the superproject's `spec` (`HEAD:libs/lib` = the recorded
/// pointer, `:libs/lib` = the staged one), one git process; `None` = no such entry.
pub fn pointer(work: &Path, spec: &str) -> Option<String> {
    let out = crate::git::run(work, &["rev-parse", "--verify", "-q", spec], None);
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// **SM-07 — commit inside a submodule.** The user changed `l.txt` inside the submodule's
/// checkout, opens the submodule's own repository, stages it and commits [`LIB_THREE`];
/// then, in the superproject, stages the submodule's new pointer and commits
/// [`UPDATE_LIB`].
///
/// Expected: the submodule's `main` is [`LIB_THREE`] on [`LIB_TWO`], clean; the
/// superproject sees the submodule modified (new commits), its pointer not recorded yet
/// ([`expect_committed`]); after its commit the superproject records [`LIB_THREE`], clean
/// ([`expect_recorded`]).
pub mod commit_in_submodule {
    use super::*;
    use crate::expect::{Checks, Expect};

    /// The commit made inside the submodule.
    pub const LIB_THREE: &str = "lib three";
    /// The superproject's commit of the new pointer.
    pub const UPDATE_LIB: &str = "Update lib";

    /// [`superproject`] with `l.txt` changed inside the submodule. Seven git processes.
    pub fn fixture(work: &Path) -> Super {
        let s = superproject(work, "");
        fs::write(sub(work).join("l.txt"), l_txt(3)).unwrap();
        s
    }

    /// The state after the commit inside the submodule ([`expect_committed`]) without its
    /// steps: [`LIB_THREE`] committed there, not recorded. Seven git processes.
    pub fn fixture_committed(work: &Path) -> Super {
        superproject_with(work, "", &[(LIB_THREE, &l_txt(3))])
    }

    /// After the commit inside the submodule. Three git processes.
    pub fn expect_committed(work: &Path, s: &Super) -> Result<(), String> {
        let e = Expect::new(&sub(work));
        let p = Expect::new(work);
        let mut k = Checks::new("SM-07 after the commit inside the submodule");
        k.eq("submodule: branch", e.branch().as_deref(), Some("main"));
        match e.log("HEAD", 1).first() {
            Some(head) => {
                k.eq("submodule: message", head.message.as_str(), LIB_THREE);
                k.eq("submodule: parents", head.parents.clone(), vec![s.lib_two.clone()]);
            }
            None => k.fail("submodule: no HEAD commit".into()),
        }
        k.eq("submodule: status", e.status_lines(), Vec::<String>::new());
        k.eq("superproject: status", p.status_lines(), vec![format!(" M {PATH}")]);
        k.eq("superproject: recorded pointer", pointer(work, &format!("HEAD:{PATH}")), Some(s.lib_two.clone()));
        k.done()
    }

    /// After the superproject's commit: HEAD [`UPDATE_LIB`] on [`ADD_LIB`] records the
    /// submodule's HEAD; clean. Three git processes.
    pub fn expect_recorded(work: &Path, s: &Super) -> Result<(), String> {
        let p = Expect::new(work);
        let lib_head = Expect::new(&sub(work)).ref_id("HEAD");
        let mut k = Checks::new("SM-07 after the superproject's commit");
        match p.log("HEAD", 1).first() {
            Some(head) => {
                k.eq("message", head.message.as_str(), UPDATE_LIB);
                k.eq("parents", head.parents.clone(), vec![s.add_lib.clone()]);
            }
            None => k.fail("no HEAD commit".into()),
        }
        k.eq("recorded pointer", pointer(work, &format!("HEAD:{PATH}")), lib_head);
        k.eq("status", p.status_lines(), Vec::<String>::new());
        k.done()
    }
}

/// **SM-06 — check out a commit in a submodule.** The user checks out [`LIB_ONE`] (an
/// older commit) in the submodule from the superproject.
///
/// Expected: the submodule detached at [`LIB_ONE`] with its files; the superproject sees
/// the pointer change, not recorded yet ([`expect_checked_out`]).
pub mod checkout_commit_in_submodule {
    use super::*;
    use crate::expect::{Checks, Expect, Head};

    /// [`superproject`], clean. Seven git processes.
    pub fn fixture(work: &Path) -> Super {
        superproject(work, "")
    }

    /// After the checkout. Two git processes.
    pub fn expect_checked_out(work: &Path, s: &Super) -> Result<(), String> {
        let e = Expect::new(&sub(work));
        let p = Expect::new(work);
        let mut k = Checks::new("SM-06 after checking out lib one in the submodule");
        k.eq("submodule: HEAD", e.head(), Head::Detached(s.lib_one.clone()));
        k.text("submodule: l.txt", e.file("l.txt").as_deref(), Some(&l_txt(1)));
        k.eq("superproject: status", p.status_lines(), vec![format!(" M {PATH}")]);
        k.eq("superproject: recorded pointer", pointer(work, &format!("HEAD:{PATH}")), Some(s.lib_two.clone()));
        k.done()
    }
}

/// **SM-05 — remove a submodule with local work.** The submodule has an untracked file
/// ([`LOCAL_FILE`]) — work that exists nowhere else. The user removes the submodule.
///
/// Expected: the app warns about the local work before anything happens; confirmed, the
/// submodule is gone: its checkout, `.git/modules/libs/lib`, `.gitmodules` (it was the
/// only one) and its config section; the removal staged ([`expect_removed`]).
pub mod remove_submodule {
    use super::*;
    use crate::expect::{Checks, Expect};

    /// The untracked file inside the submodule.
    pub const LOCAL_FILE: &str = "notes.txt";

    /// [`superproject`] with [`LOCAL_FILE`] in the submodule. Seven git processes.
    pub fn fixture(work: &Path) -> Super {
        let s = superproject(work, "");
        fs::write(sub(work).join(LOCAL_FILE), "my notes\n").unwrap();
        s
    }

    /// Before the confirmation: nothing removed yet. No git process.
    pub fn expect_untouched(work: &Path) -> Result<(), String> {
        let mut k = Checks::new("SM-05 while asking");
        k.ok("the local work is still there", sub(work).join(LOCAL_FILE).is_file(), LOCAL_FILE);
        k.ok(".git/modules/libs/lib still there", work.join(".git/modules").join(PATH).is_dir(), PATH);
        k.done()
    }

    /// After the removal. One git process.
    pub fn expect_removed(work: &Path) -> Result<(), String> {
        let p = Expect::new(work);
        let mut k = Checks::new("SM-05 after removing the submodule");
        k.ok("checkout gone", !sub(work).exists(), sub(work));
        k.ok(".git/modules/libs/lib gone", !work.join(".git/modules").join(PATH).exists(), PATH);
        k.ok(".gitmodules gone", !work.join(".gitmodules").exists(), ".gitmodules");
        let config = fs::read_to_string(p.git_dir().join("config")).unwrap_or_default();
        k.ok("no submodule config", !config.contains("[submodule"), &config);
        k.eq("status", p.status_lines(), vec!["D  .gitmodules".to_string(), format!("D  {PATH}")]);
        k.done()
    }
}

/// **SM-08 — clone: uninitialised vs recursive.** A bare superproject `<work>-super.git`
/// ([`super_remote`]) records `lib` ([`lib_remote`]) at [`PATH`]. The user clones it once
/// without and once with its submodules.
///
/// Expected: the plain clone has the submodule uninitialised — an empty folder, no
/// `.git/modules`, not reported as a change ([`expect_plain`]); the recursive one has it
/// checked out at the recorded commit, clean ([`expect_recursive`]).
pub mod clone_with_submodules {
    use super::*;
    use crate::expect::{Checks, Expect, Head};

    /// The bare superproject next to `work`.
    pub fn super_remote(work: &Path) -> PathBuf {
        crate::rm::remote_of(work, "super")
    }

    /// The two bare repositories (two git processes); `work` itself isn't created.
    pub fn fixture(work: &Path) -> Super {
        let remote = lib_remote(work);
        let mut l = Fixture::new();
        lib_history(&mut l, "main");
        let lib = l.build_bare(&remote);
        let (one, two) = (lib[Mark(1)].to_string(), lib[Mark(2)].to_string());
        let url = slash(&remote);
        let mut fx = Fixture::new();
        let add = fx.commit(
            "main",
            ADD_LIB,
            &[
                put("s.txt", "s\n"),
                put(".gitmodules", &format!("[submodule \"{PATH}\"]\n\tpath = {PATH}\n\turl = {url}\n")),
                gitlink(PATH, &two),
            ],
        );
        let ids = fx.build_bare(&super_remote(work));
        Super { add_lib: ids[add].into(), lib_one: one, lib_two: two }
    }

    /// The plain clone at `clone`. One git process.
    pub fn expect_plain(clone: &Path, s: &Super) -> Result<(), String> {
        let e = Expect::new(clone);
        let mut k = Checks::new("SM-08 after the plain clone");
        k.eq("HEAD", e.ref_id("HEAD"), Some(s.add_lib.clone()));
        k.text("s.txt", e.file("s.txt").as_deref(), Some("s\n"));
        let empty = fs::read_dir(sub(clone)).map(|mut d| d.next().is_none()).unwrap_or(false);
        k.ok("the submodule's folder is there and empty", empty, sub(clone));
        k.ok("no .git/modules", !clone.join(".git/modules").exists(), ".git/modules");
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }

    /// The recursive clone at `clone`. One git process.
    pub fn expect_recursive(clone: &Path, s: &Super) -> Result<(), String> {
        let e = Expect::new(clone);
        let mut k = Checks::new("SM-08 after the recursive clone");
        k.eq("HEAD", e.ref_id("HEAD"), Some(s.add_lib.clone()));
        let m = Expect::new(&sub(clone));
        k.eq("submodule: HEAD", m.head(), Head::Detached(s.lib_two.clone()));
        k.text("submodule: l.txt", m.file("l.txt").as_deref(), Some(&l_txt(2)));
        k.ok(".git/modules/libs/lib", clone.join(".git/modules").join(PATH).is_dir(), PATH);
        k.eq("status", e.status_lines(), Vec::<String>::new());
        k.done()
    }
}
