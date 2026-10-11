//! CM — commit, amend, reword: what goes into a commit, what stays as work in progress,
//! and what is refused before git runs.

use crate::expect::{Checks, Commit};

/// The commits `ids` among `log` (any order), by id; a missing one fails the checks.
fn by_id<'a>(c: &mut Checks, log: &'a [Commit], id: &str) -> Option<&'a Commit> {
    let found = log.iter().find(|l| l.id == id);
    if found.is_none() {
        c.fail(format!("commit {id} not in the log"));
    }
    found
}

/// The status as sorted `XY path` lines.
fn status(lines: &[&str]) -> Vec<String> {
    let mut l: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    l.sort();
    l
}

/// **CM-02 — amend: add a forgotten file.** `Add feature` is HEAD; the helper it needs,
/// `b.txt`, was never added. The user stages `b.txt` and amends with HEAD's message as
/// the app fills it in.
///
/// Expected ([`expect`]): HEAD replaced — a new commit on the same parent, its tree with
/// `b.txt`, the message unchanged; nothing left to commit.
pub mod amend_add_file {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    /// HEAD's message, kept by the amend.
    pub const MESSAGE: &str = "Add feature";
    /// The forgotten file and its content.
    pub const FORGOTTEN: &str = "b.txt";
    pub const B: &str = "helper for the feature\n";

    /// The ids the checks need.
    pub struct Setup {
        /// `init`, HEAD's parent.
        pub base: String,
        /// HEAD before the amend.
        pub old: String,
    }

    /// `init` (`a.txt`) → [`MESSAGE`] (`feature.txt`, `a.txt` changed) on `main`;
    /// [`FORGOTTEN`] untracked in the working tree. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let base = fx.commit("main", "init", &[put("a.txt", "alpha\n")]);
        let old = fx.commit(
            "main",
            MESSAGE,
            &[put("feature.txt", "feature\n"), put("a.txt", "alpha\nuses feature\n")],
        );
        fx.write(FORGOTTEN, B);
        let ids = fx.build(dir);
        Setup { base: ids[base].to_string(), old: ids[old].to_string() }
    }

    /// After the amend; three git processes.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-02 after the amend");
        c.eq("state", e.state(), OpState::Clean);
        c.eq("branch", e.branch().as_deref(), Some("main"));
        let log = e.log("HEAD", 3);
        c.eq("subjects", log.iter().map(|l| l.subject()).collect(), vec![MESSAGE, "init"]);
        if let Some(head) = log.first() {
            c.ok("HEAD replaced", head.id != s.old, &head.id);
            c.eq("same parent", head.parents.clone(), vec![s.base.clone()]);
            c.eq("message kept", head.message.as_str(), MESSAGE);
        }
        let blobs = e.blobs(&["HEAD:b.txt", "HEAD:feature.txt", "HEAD:a.txt"]);
        c.text("HEAD:b.txt", blobs[0].as_deref(), Some(B));
        c.text("HEAD:feature.txt", blobs[1].as_deref(), Some("feature\n"));
        c.text("HEAD:a.txt", blobs[2].as_deref(), Some("alpha\nuses feature\n"));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **CM-03 — amend the message only.** HEAD's message has a typo ([`TYPO`]); nothing is
/// staged, a change to `notes.txt` is work in progress. The user amends with
/// [`FIXED`].
///
/// Expected ([`expect`]): a new HEAD with the same parent and the same tree, only the
/// message replaced; `notes.txt` still unstaged.
pub mod amend_message_only {
    use std::path::Path;

    use super::by_id;
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const TYPO: &str = "Add featrue";
    pub const FIXED: &str = "Add feature";
    /// `notes.txt` in the working tree (unstaged).
    pub const NOTES_WIP: &str = "notes\nmore notes\n";

    pub struct Setup {
        pub base: String,
        pub old: String,
    }

    /// `init` (`notes.txt`) → [`TYPO`] (`feature.txt`); `notes.txt` changed, unstaged.
    /// Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let base = fx.commit("main", "init", &[put("notes.txt", "notes\n")]);
        let old = fx.commit("main", TYPO, &[put("feature.txt", "feature\n")]);
        fx.write("notes.txt", NOTES_WIP);
        let ids = fx.build(dir);
        Setup { base: ids[base].to_string(), old: ids[old].to_string() }
    }

    /// After the amend; two git processes.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-03 after the amend");
        c.eq("state", e.state(), OpState::Clean);
        // HEAD's line and the old commit (still in the object store) in one log.
        let log = e.log_args(&["HEAD", &s.old]);
        let head = e.ref_id("HEAD").unwrap_or_default();
        c.ok("HEAD replaced", head != s.old, &head);
        if let (Some(new), Some(old)) = (by_id(&mut c, &log, &head), by_id(&mut c, &log, &s.old)) {
            c.eq("message", new.message.as_str(), FIXED);
            c.eq("same parent", new.parents.clone(), vec![s.base.clone()]);
            c.eq("same tree", new.tree.as_str(), old.tree.as_str());
        }
        c.eq("status", e.status_lines(), vec![" M notes.txt".to_string()]);
        c.text("notes.txt on disk", e.file("notes.txt").as_deref(), Some(NOTES_WIP));
        c.done()
    }
}

/// **CM-04 — amend with unstaged work around.** HEAD [`MESSAGE`] changed line 3 of
/// `parser.txt`; the user staged a fix of line 18 for it, while line 10 has a debug line
/// that is not ready, `notes.txt` is changed and `scratch.txt` is new — none of them
/// staged. The user amends with the message edited to [`EDITED`].
///
/// Expected ([`expect`]): HEAD replaced on the same parent with the staged fix and the
/// edited message — and nothing else: the debug line, `notes.txt` and `scratch.txt` stay
/// as they were, unstaged. [`expect_untouched`] is the starting state (a refused commit,
/// CM-09).
pub mod amend_with_unstaged_work {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const MESSAGE: &str = "Parse config";
    pub const EDITED: &str = "Parse config and its defaults";
    pub const NOTES_WIP: &str = "notes\nwork in progress\n";
    pub const SCRATCH: &str = "scratch\n";

    /// `parser.txt`: 20 lines `p N`; line 3 = [`MESSAGE`]'s change, line 18 the staged
    /// fix, line 10 the unstaged debug line.
    pub fn parser(line3: bool, line18: bool, line10: bool) -> String {
        (1..=20)
            .map(|i| match i {
                3 if line3 => "p 3 parses config\n".to_string(),
                10 if line10 => "p 10 dbg!(config)\n".to_string(),
                18 if line18 => "p 18 default = 4\n".to_string(),
                _ => format!("p {i}\n"),
            })
            .collect()
    }

    pub struct Setup {
        pub base: String,
        pub old: String,
    }

    /// `init` (`parser.txt`, `notes.txt`) → [`MESSAGE`]; the line-18 fix staged, the debug
    /// line, `notes.txt` and the untracked `scratch.txt` not. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let base = fx.commit(
            "main",
            "init",
            &[put("parser.txt", &parser(false, false, false)), put("notes.txt", "notes\n")],
        );
        let old = fx.commit("main", MESSAGE, &[put("parser.txt", &parser(true, false, false))]);
        fx.stage(&[put("parser.txt", &parser(true, true, false))]);
        fx.write("parser.txt", &parser(true, true, true));
        fx.write("notes.txt", NOTES_WIP);
        fx.write("scratch.txt", SCRATCH);
        let ids = fx.build(dir);
        Setup { base: ids[base].to_string(), old: ids[old].to_string() }
    }

    /// The work in progress on disk, unchanged by anything the workflow did.
    fn work_kept(c: &mut Checks, e: &Expect) {
        c.text("parser.txt on disk", e.file("parser.txt").as_deref(), Some(&parser(true, true, true)));
        c.text("notes.txt on disk", e.file("notes.txt").as_deref(), Some(NOTES_WIP));
        c.text("scratch.txt on disk", e.file("scratch.txt").as_deref(), Some(SCRATCH));
    }

    /// After the amend; three git processes.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-04 after the amend");
        c.eq("state", e.state(), OpState::Clean);
        let log = e.log("HEAD", 3);
        c.eq("subjects", log.iter().map(|l| l.subject()).collect(), vec![EDITED, "init"]);
        if let Some(head) = log.first() {
            c.ok("HEAD replaced", head.id != s.old, &head.id);
            c.eq("same parent", head.parents.clone(), vec![s.base.clone()]);
        }
        let blobs = e.blobs(&["HEAD:parser.txt", "HEAD:notes.txt", "HEAD:scratch.txt", ":parser.txt"]);
        c.text("HEAD:parser.txt (the staged fix only)", blobs[0].as_deref(), Some(&parser(true, true, false)));
        c.text("HEAD:notes.txt", blobs[1].as_deref(), Some("notes\n"));
        c.text("HEAD:scratch.txt", blobs[2].as_deref(), None);
        c.text("index parser.txt", blobs[3].as_deref(), Some(&parser(true, true, false)));
        work_kept(&mut c, &e);
        c.eq(
            "status",
            e.status_lines(),
            super::status(&[" M notes.txt", " M parser.txt", "?? scratch.txt"]),
        );
        c.done()
    }

    /// Nothing happened (a refused commit): HEAD is still [`MESSAGE`], the fix staged,
    /// the rest as it was. Three git processes.
    pub fn expect_untouched(dir: &Path, s: &Setup, what: &str) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new(what);
        c.eq("state", e.state(), OpState::Clean);
        c.eq("HEAD", e.ref_id("HEAD"), Some(s.old.clone()));
        c.eq("subjects", e.subjects("HEAD", 3), vec![MESSAGE.to_string(), "init".to_string()]);
        c.text("index parser.txt", e.index("parser.txt").as_deref(), Some(&parser(true, true, false)));
        work_kept(&mut c, &e);
        c.eq(
            "status",
            e.status_lines(),
            super::status(&[" M notes.txt", "MM parser.txt", "?? scratch.txt"]),
        );
        c.done()
    }
}

/// **CM-06 — amend a pushed commit, then push.** HEAD [`MESSAGE`] is on `origin` (a
/// local bare repository); the user amends its message to [`AMENDED`] and pushes: the
/// push can't add to `origin/main`, the app offers Push with Lease, which replaces it.
///
/// Expected: after the amend ([`expect_amended`]) HEAD is new on the same parent and
/// `origin/main` still the old commit; after the push ([`expect_pushed`]) `origin`'s
/// `main` is the amended commit.
pub mod amend_after_push {
    use std::path::{Path, PathBuf};

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const MESSAGE: &str = "Add feature";
    pub const AMENDED: &str = "Add feature, documented";

    pub struct Setup {
        pub base: String,
        /// The pushed HEAD (`origin/main`).
        pub pushed: String,
        /// The bare remote `origin`.
        pub origin: PathBuf,
    }

    /// `origin` = `<dir>-origin.git` with `init` → [`MESSAGE`]; the repository at `dir`
    /// with the same commits, `origin/main` there, `main` tracking it. Four git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut origin = dir.as_os_str().to_owned();
        origin.push("-origin.git");
        let origin = PathBuf::from(origin);
        let mut fx = Fixture::new();
        let base = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let pushed = fx.commit("main", MESSAGE, &[put("feature.txt", "feature\n")]);
        fx.build_bare(&origin);
        fx.branch("refs/remotes/origin/main", pushed);
        fx.remote("origin", &origin);
        fx.upstream("main", "origin");
        let ids = fx.build(dir);
        Setup { base: ids[base].to_string(), pushed: ids[pushed].to_string(), origin }
    }

    /// After the amend, before the push; one git process.
    pub fn expect_amended(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-06 after the amend");
        c.eq("state", e.state(), OpState::Clean);
        let log = e.log("HEAD", 1);
        if let Some(head) = log.first() {
            c.ok("HEAD replaced", head.id != s.pushed, &head.id);
            c.eq("message", head.message.as_str(), AMENDED);
            c.eq("same parent", head.parents.clone(), vec![s.base.clone()]);
        }
        c.eq("origin/main", e.ref_id("refs/remotes/origin/main"), Some(s.pushed.clone()));
        c.eq("origin's main", Expect::bare_ref(&s.origin, "refs/heads/main"), Some(s.pushed.clone()));
        c.done()
    }

    /// After Push with Lease; no git process.
    pub fn expect_pushed(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-06 after the push");
        let head = e.ref_id("HEAD");
        c.ok("HEAD is the amended commit", head.as_deref() != Some(s.pushed.as_str()), &head);
        c.eq("origin's main", Expect::bare_ref(&s.origin, "refs/heads/main"), head.clone());
        c.eq("origin/main", e.ref_id("refs/remotes/origin/main"), head);
        c.done()
    }
}

/// **CM-07 — amend while a merge is in progress.** `git merge --no-commit feat` went
/// through without conflicts and waits for its commit; the user turns Amend on and
/// commits.
///
/// Expected: refused with the reason before git runs — still merging, HEAD and the merge
/// result as they were ([`expect`]); with Amend off, Commit concludes the merge
/// ([`expect_concluded`]).
pub mod amend_during_merge {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    /// `main`'s commit after `init` (HEAD).
    pub const MAIN: &str = "Main work";
    /// `feat`'s commit.
    pub const FEAT: &str = "Feature";
    /// git's message for the merge.
    pub const MERGE: &str = "Merge branch 'feat'";
    /// Part of the refusal both apps show (`gitti_core::ops::amend_blocker`).
    pub const REFUSAL: &str = "Can't amend while a merge is in progress";

    pub struct Setup {
        pub head: String,
        pub feat: String,
    }

    /// `init` → [`MAIN`] on `main`, `feat` from `init` → [`FEAT`] (`f.txt`); `git merge
    /// --no-commit --no-ff feat` stopped before committing. Four git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let feat = fx.commit_from("feat", init, FEAT, &[put("f.txt", "f\n")]);
        let head = fx.commit("main", MAIN, &[put("m.txt", "m\n")]);
        fx.git_after(&["merge", "--no-commit", "--no-ff", "feat"]);
        let ids = fx.build(dir);
        Setup { head: ids[head].to_string(), feat: ids[feat].to_string() }
    }

    /// The merge still waits, nothing amended; one git process.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-07 after the refused amend");
        c.eq("state", e.state(), OpState::Merging);
        c.eq("HEAD", e.ref_id("HEAD"), Some(s.head.clone()));
        c.eq("MERGE_HEAD", e.ref_id("MERGE_HEAD"), Some(s.feat.clone()));
        c.text("index f.txt (the merge result)", e.index("f.txt").as_deref(), Some("f\n"));
        c.done()
    }

    /// Concluded: a merge commit of HEAD and `feat`; one git process.
    pub fn expect_concluded(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-07 after concluding the merge");
        c.eq("state", e.state(), OpState::Clean);
        let log = e.log("HEAD", 1);
        if let Some(head) = log.first() {
            c.eq("parents", head.parents.clone(), vec![s.head.clone(), s.feat.clone()]);
            c.eq("subject", head.subject(), MERGE);
        }
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **CM-10 — a failing pre-commit hook, fixed, committed.** The pre-commit hook refuses
/// while a `TODO` is staged; the user commits `a.txt` with one ([`MESSAGE`]), reads the
/// hook's reason ([`HOOK_SAYS`]), fixes the line, stages it again and commits.
///
/// Expected: no commit and the change still staged after the refusal
/// ([`expect_refused`]); then one commit with the fixed file ([`expect_committed`]).
pub mod hook_refuses {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const MESSAGE: &str = "Finish a";
    /// What the hook prints when it refuses.
    pub const HOOK_SAYS: &str = "pre-commit: a TODO is left in a.txt";
    pub const WITH_TODO: &str = "a\nTODO: finish\n";
    pub const FIXED: &str = "a\nfinished\n";

    /// `init` (`a.txt`); `a.txt` with a `TODO` line staged; the pre-commit hook. Three git
    /// processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.stage(&[put("a.txt", WITH_TODO)]);
        fx.hook(
            "pre-commit",
            &format!("#!/bin/sh\nif git grep --cached -q TODO -- a.txt; then echo '{HOOK_SAYS}' >&2; exit 1; fi\n"),
        );
        fx.build(dir);
    }

    /// The hook refused: no commit, `a.txt` still staged; two git processes.
    pub fn expect_refused(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-10 after the refused commit");
        c.eq("state", e.state(), OpState::Clean);
        c.eq("subjects", e.subjects("HEAD", 2), vec!["init".to_string()]);
        c.text("index a.txt", e.index("a.txt").as_deref(), Some(WITH_TODO));
        c.done()
    }

    /// Committed after the fix; three git processes.
    pub fn expect_committed(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-10 after the commit");
        c.eq("subjects", e.subjects("HEAD", 3), vec![MESSAGE.to_string(), "init".to_string()]);
        c.text("HEAD:a.txt", e.blobs(&["HEAD:a.txt"])[0].as_deref(), Some(FIXED));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **CM-11 — a commit-msg hook adds a trailer.** The repository's commit-msg hook
/// appends [`TRAILER`]; the user commits the staged `b.txt` as [`MESSAGE`].
///
/// Expected ([`expect`]): the commit's message is the hook's, [`COMMITTED`].
pub mod hook_rewrites_message {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const MESSAGE: &str = "Add b";
    pub const TRAILER: &str = "Reviewed-by: Hook <hook@example.com>";
    /// The message as committed.
    pub const COMMITTED: &str = "Add b\n\nReviewed-by: Hook <hook@example.com>";

    /// `init` (`a.txt`); `b.txt` staged; the commit-msg hook. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.stage(&[put("b.txt", "b\n")]);
        fx.hook("commit-msg", &format!("#!/bin/sh\nprintf '\\n{TRAILER}\\n' >> \"$1\"\n"));
        fx.build(dir);
    }

    /// Committed with the hook's message; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-11 after the commit");
        c.eq("state", e.state(), OpState::Clean);
        let log = e.log("HEAD", 3);
        c.eq("commits", log.len(), 2);
        if let Some(head) = log.first() {
            c.eq("message", head.message.as_str(), COMMITTED);
        }
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **CM-14 — `#123` lines in a message.** `commit.cleanup = strip` is configured (here:
/// in the repository; a global one acts the same); the user commits a message whose body
/// starts lines with `#` ([`MESSAGE`]), then amends it ([`AMENDED`]).
///
/// Expected: both messages kept as typed, `#` lines included ([`expect_committed`],
/// [`expect_amended`]).
pub mod hash_lines {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const MESSAGE: &str = "Fix the parser\n\n#123 was the cause\n# not a comment";
    pub const AMENDED: &str = "Fix the parser\n\n#123 was the cause\n#124 too";

    /// `init` (`a.txt`), `commit.cleanup = strip`; `a.txt` changed and staged. Three git
    /// processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.config("[commit]\n\tcleanup = strip\n");
        fx.stage(&[put("a.txt", "a fixed\n")]);
        fx.build(dir);
    }

    fn head_message(dir: &Path, what: &str, want: &str) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new(what);
        let log = e.log("HEAD", 3);
        c.eq("commits", log.len(), 2);
        if let Some(head) = log.first() {
            c.eq("message", head.message.as_str(), want);
        }
        c.done()
    }

    /// One git process.
    pub fn expect_committed(dir: &Path) -> Result<(), String> {
        head_message(dir, "CM-14 after the commit", MESSAGE)
    }

    /// One git process.
    pub fn expect_amended(dir: &Path) -> Result<(), String> {
        head_message(dir, "CM-14 after the amend", AMENDED)
    }
}

/// **CM-15 / CM-16 — reword HEAD / an older commit with work in progress.** Three
/// commits ([`PARSER`], [`LEXER`], [`DOCS`]); the user has staged a change of
/// `lexer.txt` and the new `new.txt`, changed `lexer.txt` again after staging it, edited
/// `docs.txt` without staging it and has an untracked `scratch.txt`. Edit Commit Message
/// on HEAD ([`HEAD_NEW`]), or on [`LEXER`] ([`OLDER_NEW`]).
///
/// Expected: the message replaced, every tree as before, the commits below untouched
/// ([`expect_head_reworded`], [`expect_older_reworded`]); the work in progress exactly as
/// it was — staged stays staged (also the staged version of a file changed again),
/// unstaged stays unstaged, nothing committed, no rebase or stash left.
pub mod reword_with_work {
    use std::path::Path;

    use super::by_id;
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const PARSER: &str = "Add parser";
    pub const LEXER: &str = "Add lexer";
    pub const DOCS: &str = "Add docs";
    pub const HEAD_NEW: &str = "Add docs for the parser";
    pub const OLDER_NEW: &str = "Add the lexer\n\nIt splits the input into tokens.";
    pub const LEXER_STAGED: &str = "lexer v2\n";
    pub const LEXER_WORK: &str = "lexer v3\n";
    pub const DOCS_WORK: &str = "docs\nmore docs\n";

    pub struct Setup {
        pub parser: String,
        pub lexer: String,
        pub docs: String,
    }

    /// The three commits; the work in progress described above. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let parser = fx.commit("main", PARSER, &[put("parser.txt", "parser\n")]);
        let lexer = fx.commit("main", LEXER, &[put("lexer.txt", "lexer\n")]);
        let docs = fx.commit("main", DOCS, &[put("docs.txt", "docs\n")]);
        fx.stage(&[put("lexer.txt", LEXER_STAGED), put("new.txt", "new\n")]);
        fx.write("lexer.txt", LEXER_WORK);
        fx.write("docs.txt", DOCS_WORK);
        fx.write("scratch.txt", "scratch\n");
        let ids = fx.build(dir);
        Setup {
            parser: ids[parser].to_string(),
            lexer: ids[lexer].to_string(),
            docs: ids[docs].to_string(),
        }
    }

    /// The work in progress as the fixture left it (two git processes), no operation or
    /// stash left over.
    fn work_kept(c: &mut Checks, e: &Expect) {
        c.eq("state", e.state(), OpState::Clean);
        c.eq("stashes", e.stashes(), 0);
        c.ok("no message file left", !e.git_dir().join("GITTI_REWORD_MSG").exists(), "GITTI_REWORD_MSG");
        let blobs = e.blobs(&[":lexer.txt", ":new.txt", ":docs.txt"]);
        c.text("index lexer.txt (staged)", blobs[0].as_deref(), Some(LEXER_STAGED));
        c.text("index new.txt (staged)", blobs[1].as_deref(), Some("new\n"));
        c.text("index docs.txt (not staged)", blobs[2].as_deref(), Some("docs\n"));
        c.text("lexer.txt on disk", e.file("lexer.txt").as_deref(), Some(LEXER_WORK));
        c.text("docs.txt on disk", e.file("docs.txt").as_deref(), Some(DOCS_WORK));
        c.text("scratch.txt on disk", e.file("scratch.txt").as_deref(), Some("scratch\n"));
        c.eq(
            "status",
            e.status_lines(),
            super::status(&["MM lexer.txt", "A  new.txt", " M docs.txt", "?? scratch.txt"]),
        );
    }

    /// HEAD reworded; three git processes.
    pub fn expect_head_reworded(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-15 after rewording HEAD");
        let log = e.log_args(&["HEAD", &s.docs]);
        let head = e.ref_id("HEAD").unwrap_or_default();
        c.ok("HEAD replaced", head != s.docs, &head);
        if let (Some(new), Some(old)) = (by_id(&mut c, &log, &head), by_id(&mut c, &log, &s.docs)) {
            c.eq("message", new.message.as_str(), HEAD_NEW);
            c.eq("same parent", new.parents.clone(), vec![s.lexer.clone()]);
            c.eq("same tree (nothing staged went in)", new.tree.as_str(), old.tree.as_str());
        }
        work_kept(&mut c, &e);
        c.done()
    }

    /// [`LEXER`] reworded, [`DOCS`] rewritten on it; three git processes.
    pub fn expect_older_reworded(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-16 after rewording an older commit");
        // The new line and the old one in one log: 3 + 2 commits.
        let log = e.log_args(&["HEAD", &s.docs]);
        let head = e.ref_id("HEAD").unwrap_or_default();
        let new_docs = by_id(&mut c, &log, &head);
        let new_lexer = new_docs.and_then(|d| d.parents.first()).and_then(|p| by_id(&mut c, &log, p));
        let (old_docs, old_lexer) = (by_id(&mut c, &log, &s.docs), by_id(&mut c, &log, &s.lexer));
        if let (Some(nd), Some(nl), Some(od), Some(ol)) = (new_docs, new_lexer, old_docs, old_lexer) {
            c.ok("HEAD rewritten", nd.id != od.id, &nd.id);
            c.eq("HEAD's message", nd.message.as_str(), DOCS);
            c.eq("HEAD's tree", nd.tree.as_str(), od.tree.as_str());
            c.ok("the lexer commit rewritten", nl.id != ol.id, &nl.id);
            c.eq("the lexer commit's message", nl.message.as_str(), OLDER_NEW);
            c.eq("the lexer commit's tree", nl.tree.as_str(), ol.tree.as_str());
            c.eq("below it untouched", nl.parents.clone(), vec![s.parser.clone()]);
        }
        work_kept(&mut c, &e);
        c.done()
    }
}

/// **CM-17 — reword the root, a pushed commit, one below a merge.** `main`: [`ROOT`] →
/// [`PUSHED`] (= `origin/main`) → [`BELOW`]; `feat` from [`BELOW`]; the merge of `feat`;
/// [`TOP`] (HEAD). Edit Commit Message on [`PUSHED`] and on [`BELOW`]: refused with the
/// reason ([`PUSHED_WHY`], [`MERGE_WHY`]), nothing changed ([`expect_unchanged`]). In a
/// second repository without remote or merge ([`fixture_root`]) the root commit is
/// reworded ([`expect_root_reworded`]).
pub mod reword_refusals {
    use std::path::Path;

    use super::by_id;
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const ROOT: &str = "Add parser";
    pub const PUSHED: &str = "Add lexer";
    pub const BELOW: &str = "Start docs";
    pub const FEAT: &str = "Docs feature";
    pub const MERGE: &str = "Merge branch 'feat'";
    pub const TOP: &str = "Polish";
    pub const ROOT_NEW: &str = "Add the parser";
    /// Parts of the reasons (`gitti_core::Repo::reword_blocker`).
    pub const PUSHED_WHY: &str = "already pushed (origin/main)";
    pub const MERGE_WHY: &str = "merge commits from it up to HEAD";

    pub struct Setup {
        pub root: String,
        pub pushed: String,
        pub below: String,
        pub top: String,
    }

    /// The history above; clean. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let root = fx.commit("main", ROOT, &[put("parser.txt", "parser\n")]);
        let pushed = fx.commit("main", PUSHED, &[put("lexer.txt", "lexer\n")]);
        let below = fx.commit("main", BELOW, &[put("docs.txt", "docs\n")]);
        let feat = fx.commit_from("feat", below, FEAT, &[put("feat.txt", "feat\n")]);
        let merge = fx.merge("main", &[below, feat], MERGE, &[put("feat.txt", "feat\n")]);
        let top = fx.commit_from("main", merge, TOP, &[put("docs.txt", "docs, polished\n")]);
        fx.branch("refs/remotes/origin/main", pushed);
        let ids = fx.build(dir);
        Setup {
            root: ids[root].to_string(),
            pushed: ids[pushed].to_string(),
            below: ids[below].to_string(),
            top: ids[top].to_string(),
        }
    }

    /// Nothing rewritten; two git processes.
    pub fn expect_unchanged(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-17 after the refusals");
        c.eq("state", e.state(), OpState::Clean);
        c.eq("HEAD", e.ref_id("HEAD"), Some(s.top.clone()));
        c.eq("origin/main", e.ref_id("refs/remotes/origin/main"), Some(s.pushed.clone()));
        c.eq(
            "first-parent subjects",
            e.log_args(&["--first-parent", "HEAD"]).iter().map(|l| l.subject().to_string()).collect(),
            [TOP, MERGE, BELOW, PUSHED, ROOT].map(String::from).to_vec(),
        );
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }

    /// The root commit to reword: [`ROOT`] → [`PUSHED`] (here not pushed), clean. Three
    /// git processes.
    pub fn fixture_root(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let root = fx.commit("main", ROOT, &[put("parser.txt", "parser\n")]);
        let top = fx.commit("main", PUSHED, &[put("lexer.txt", "lexer\n")]);
        let ids = fx.build(dir);
        Setup {
            root: ids[root].to_string(),
            pushed: String::new(),
            below: String::new(),
            top: ids[top].to_string(),
        }
    }

    /// The root reworded, the commit above rewritten on it, trees kept; two git processes.
    pub fn expect_root_reworded(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-17 after rewording the root");
        c.eq("state", e.state(), OpState::Clean);
        let log = e.log_args(&["HEAD", &s.top]);
        let head = e.ref_id("HEAD").unwrap_or_default();
        let new_top = by_id(&mut c, &log, &head);
        let new_root = new_top.and_then(|t| t.parents.first()).and_then(|p| by_id(&mut c, &log, p));
        let (old_top, old_root) = (by_id(&mut c, &log, &s.top), by_id(&mut c, &log, &s.root));
        if let (Some(nt), Some(nr), Some(ot), Some(or)) = (new_top, new_root, old_top, old_root) {
            c.eq("the root's message", nr.message.as_str(), ROOT_NEW);
            c.ok("still a root", nr.parents.is_empty(), &nr.parents);
            c.eq("the root's tree", nr.tree.as_str(), or.tree.as_str());
            c.eq("HEAD's message", nt.message.as_str(), PUSHED);
            c.eq("HEAD's tree", nt.tree.as_str(), ot.tree.as_str());
        }
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **CM-19 — the first commit in a fresh repository.** `git init`, two files written;
/// the user stages them and commits [`MESSAGE`].
///
/// Expected ([`expect`]): a root commit on `main` with both files; nothing left.
pub mod first_commit {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::Fixture;

    pub const MESSAGE: &str = "Initial commit";
    pub const README: &str = "# Project\n";
    pub const MAIN: &str = "fn main() {}\n";

    /// A fresh repository (`main` unborn) with `README.md` and `src/main.rs`, untracked.
    /// One git process.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.write("README.md", README);
        fx.write("src/main.rs", MAIN);
        fx.build(dir);
    }

    /// The root commit; three git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-19 after the first commit");
        c.eq("state", e.state(), OpState::Clean);
        c.eq("branch", e.branch().as_deref(), Some("main"));
        let log = e.log("HEAD", 2);
        c.eq("commits", log.len(), 1);
        if let Some(head) = log.first() {
            c.eq("message", head.message.as_str(), MESSAGE);
            c.ok("a root commit", head.parents.is_empty(), &head.parents);
        }
        let blobs = e.blobs(&["HEAD:README.md", "HEAD:src/main.rs"]);
        c.text("HEAD:README.md", blobs[0].as_deref(), Some(README));
        c.text("HEAD:src/main.rs", blobs[1].as_deref(), Some(MAIN));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **CM-21 — stop a commit a hook holds.** The pre-commit hook hangs; the user stops
/// the commit (Stop and Quit, Esc on the busy mark).
///
/// Expected ([`expect_stopped`]): no commit, the change still staged, git's
/// `index.lock` gone — nothing to clean up by hand.
pub mod commit_held_by_hook {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const MESSAGE: &str = "Never committed";

    /// `init` (`a.txt`); `a.txt` changed and staged; a pre-commit hook that sleeps 30 s.
    /// Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.stage(&[put("a.txt", "b\n")]);
        fx.hook("pre-commit", "#!/bin/sh\nsleep 30\n");
        fx.build(dir);
    }

    /// Stopped: nothing committed, nothing lost; three git processes.
    pub fn expect_stopped(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-21 after stopping the commit");
        c.eq("state", e.state(), OpState::Clean);
        c.eq("subjects", e.subjects("HEAD", 2), vec!["init".to_string()]);
        c.text("index a.txt", e.index("a.txt").as_deref(), Some("b\n"));
        c.ok("no index.lock", !e.git_dir().join("index.lock").exists(), "index.lock");
        c.eq("status", e.status_lines(), vec!["M  a.txt".to_string()]);
        c.done()
    }
}
