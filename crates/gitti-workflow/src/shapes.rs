//! File shapes and encodings in the apps: renames (RN), binary files (BN), big and many
//! (BG), names and content that aren't plain UTF-8 (NU), file-system paths and file types
//! (FS), ignore rules (IG-06) and Unicode / legacy-encoded commit messages (CM-20). The core tests of
//! `unix_fs` / `non_utf8` / `repo_shapes` pin the git side of each shape; these workflows
//! are the representative app runs — one repository per shape group, every file of it
//! staged / unstaged / discarded once, the rows and the result checked
//! (`tests/{core,ui,tui}/wf_shapes.rs`).
//!
//! Shapes only some file systems hold (symlinks, FIFOs, non-UTF-8 names, two names that
//! differ in case or Unicode form) have a `holds…` probe: the drivers skip the step where
//! the disk can't make the shape (macOS refuses non-UTF-8 names and folds case and NFD).

use std::path::Path;

use crate::git;

/// `n` lines `{prefix}{i}` (1-based).
fn numbered(prefix: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{prefix}{i}\n")).collect()
}

/// The index entries' modes of `paths` (`100644`, `120000`, …; `None` = not in the
/// index) — one `git ls-files -s`.
pub fn index_modes(dir: &Path, paths: &[&str]) -> Vec<Option<String>> {
    let out = git::bytes(dir, &["ls-files", "-s", "-z"]);
    let text = String::from_utf8_lossy(&out).to_string();
    let entries: Vec<(&str, &str)> = text
        .split('\0')
        .filter(|e| !e.is_empty())
        .filter_map(|e| {
            let (head, path) = e.split_once('\t')?;
            Some((head.split(' ').next()?, path))
        })
        .collect();
    paths
        .iter()
        .map(|p| entries.iter().find(|(_, q)| q == p).map(|(m, _)| m.to_string()))
        .collect()
}

/// The index contents of `paths` — names with any bytes, a newline included (which `git
/// cat-file --batch` can't take as `:path`): the ids from one `git ls-files -s -z`, the
/// contents in one `git cat-file --batch`. `None` = not in the index.
pub fn index_blobs(dir: &Path, paths: &[&str]) -> Vec<Option<Vec<u8>>> {
    let out = git::bytes(dir, &["ls-files", "-s", "-z"]);
    let text = String::from_utf8_lossy(&out).to_string();
    let ids: Vec<(String, &str)> = text
        .split('\0')
        .filter_map(|e| {
            let (head, path) = e.split_once('\t')?;
            Some((head.split(' ').nth(1)?.to_string(), path))
        })
        .collect();
    let wanted: Vec<Option<&str>> =
        paths.iter().map(|p| ids.iter().find(|(_, q)| q == p).map(|(id, _)| id.as_str())).collect();
    let specs: Vec<&str> = wanted.iter().flatten().copied().collect();
    let mut found = crate::expect::Expect::new(dir).blobs(&specs).into_iter();
    wanted.iter().map(|w| w.and_then(|_| found.next().flatten())).collect()
}

/// **RN-01 / RN-02 / RN-04 — renames.** History: `init` (`old.txt`, `draft.txt`,
/// `a.txt`), then [`RENAME_SUBJECT`] renaming `a.txt` to `b.txt` with one line added. In the
/// working tree `old.txt` was moved to `new.txt` unchanged (RN-01) and `draft.txt` to
/// `final.txt` with two lines added (RN-02); nothing staged — git lists each move as a
/// deletion and an untracked file.
///
/// - RN-01: the user stages both halves of `old.txt` → `new.txt` ([`expect_rn01_staged`]:
///   one rename in the index), then unstages both ([`expect_rn01_unstaged`]: the deletion
///   and the untracked file again).
/// - RN-02: the user stages the lines of `final.txt` up to [`KEPT`] (not [`LEFT`]), then the
///   deletion of `draft.txt` ([`expect_rn02`]: a rename staged, `final.txt` without
///   [`LEFT`] in the index and with it on disk).
/// - RN-04: the user selects the commit [`RENAME_SUBJECT`]: its file is shown as
///   `a.txt → b.txt`.
pub mod renames {
    use std::path::Path;

    use crate::expect::{Checks, Expect, Status};
    use crate::fixture::{Fixture, del, put};

    pub const OLD: &str = "old.txt";
    pub const NEW: &str = "new.txt";
    pub const DRAFT: &str = "draft.txt";
    pub const FINAL: &str = "final.txt";
    /// The last added line of `final.txt` staged, and the one left unstaged.
    pub const KEPT: &str = "extra 1";
    pub const LEFT: &str = "extra 2";
    pub const FROM: &str = "a.txt";
    pub const TO: &str = "b.txt";
    pub const RENAME_SUBJECT: &str = "Rename a.txt to b.txt";
    /// How the apps name the file of the rename commit.
    pub const SHOWN: &str = "a.txt → b.txt";

    pub fn old_text() -> String {
        super::numbered("old line ", 10)
    }

    pub fn draft_text() -> String {
        super::numbered("draft line ", 10)
    }

    /// `final.txt`: the draft plus [`KEPT`] (if `kept`) and [`LEFT`] (if `left`).
    pub fn final_text(kept: bool, left: bool) -> String {
        let mut s = draft_text();
        if kept {
            s.push_str(&format!("{KEPT}\n"));
        }
        if left {
            s.push_str(&format!("{LEFT}\n"));
        }
        s
    }

    /// The history and the two moves in the working tree. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        let a = super::numbered("a line ", 6);
        fx.commit(
            "main",
            "init",
            &[put(OLD, &old_text()), put(DRAFT, &draft_text()), put(FROM, &a)],
        );
        fx.commit("main", RENAME_SUBJECT, &[del(FROM), put(TO, &format!("{a}a line 7\n"))]);
        fx.remove(OLD);
        fx.write(NEW, &old_text());
        fx.remove(DRAFT);
        fx.write(FINAL, &final_text(true, true));
        fx.build(dir);
    }

    fn entry<'a>(status: &'a [Status], path: &str) -> Option<&'a Status> {
        status.iter().find(|s| s.path == path)
    }

    /// RN-01 after staging both halves: `new.txt` staged as a rename of `old.txt`. Two git
    /// processes.
    pub fn expect_rn01_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("RN-01 after staging both halves");
        let st = e.status();
        let new = entry(&st, NEW).map(|s| (s.xy.clone(), s.from.clone()));
        c.eq("status of new.txt", new, Some(("R ".to_string(), Some(OLD.to_string()))));
        c.ok("old.txt not listed on its own", entry(&st, OLD).is_none(), &st);
        let ix = e.blobs(&[&format!(":{NEW}"), &format!(":{OLD}")]);
        c.text("index new.txt", ix[0].as_deref(), Some(&old_text()));
        c.ok("old.txt gone from the index", ix[1].is_none(), "still there");
        c.done()
    }

    /// RN-01 after unstaging both halves: the deletion and the untracked file again. Two
    /// git processes.
    pub fn expect_rn01_unstaged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("RN-01 after unstaging both halves");
        let st = e.status();
        c.eq("status of old.txt", entry(&st, OLD).map(|s| s.xy.as_str()), Some(" D"));
        c.eq("status of new.txt", entry(&st, NEW).map(|s| s.xy.as_str()), Some("??"));
        let ix = e.blobs(&[&format!(":{OLD}"), &format!(":{NEW}")]);
        c.text("index old.txt", ix[0].as_deref(), Some(&old_text()));
        c.ok("new.txt not in the index", ix[1].is_none(), "staged");
        c.text("new.txt on disk", e.file(NEW).as_deref(), Some(&old_text()));
        c.done()
    }

    /// RN-02 after staging the lines and the deletion: a rename with [`KEPT`] staged,
    /// [`LEFT`] only on disk. Two git processes.
    pub fn expect_rn02(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("RN-02 after staging the lines and the deletion");
        let st = e.status();
        let f = entry(&st, FINAL).map(|s| (s.xy.clone(), s.from.clone()));
        c.eq("status of final.txt", f, Some(("RM".to_string(), Some(DRAFT.to_string()))));
        let ix = e.blobs(&[&format!(":{FINAL}"), &format!(":{DRAFT}")]);
        c.text("index final.txt", ix[0].as_deref(), Some(&final_text(true, false)));
        c.ok("draft.txt gone from the index", ix[1].is_none(), "still there");
        c.text("final.txt on disk", e.file(FINAL).as_deref(), Some(&final_text(true, true)));
        c.done()
    }
}

/// **BN-01 / BN-02 — binary files.** `bin.dat` (binary) and `a.txt` (text) are committed,
/// both changed; `new.bin` is a new binary file. `flip.txt` was text, a binary version is
/// staged, and the working tree has text again.
///
/// - BN-01: the user stages the line [`PICKED`] of `a.txt`, the whole `bin.dat` and
///   `new.bin` ([`expect_bn01_staged`]); then unstages `bin.dat` and discards it
///   ([`expect_bn01_discarded`]: back to the committed bytes). A binary file has no lines
///   to pick: the apps show it whole.
/// - BN-02: `flip.txt` shows as binary in both sections; staged whole, its staged diff is
///   text (HEAD and index are text) and its line [`AGAIN`] unstages ([`expect_bn02`]).
pub mod binary {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put, put_bytes};

    pub const BIN: &str = "bin.dat";
    pub const TEXT: &str = "a.txt";
    pub const NEW_BIN: &str = "new.bin";
    pub const FLIP: &str = "flip.txt";
    pub const BIN_HEAD: &[u8] = &[0, 1, 2, 3, 255];
    pub const BIN_WORK: &[u8] = &[0, 1, 2, 4, 255, 0];
    pub const NEW_BIN_DATA: &[u8] = b"\0\x01new";
    pub const FLIP_STAGED: &[u8] = b"text\n\0bin";
    /// The added lines of `a.txt`: [`PICKED`] is staged, [`OTHER`] stays.
    pub const PICKED: &str = "X";
    pub const OTHER: &str = "Y";
    /// `flip.txt`'s added line in the working tree.
    pub const AGAIN: &str = "again";

    /// Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit(
            "main",
            "init",
            &[put_bytes(BIN, BIN_HEAD), put(TEXT, "a\nb\n"), put(FLIP, "text\n")],
        );
        fx.stage(&[put_bytes(FLIP, FLIP_STAGED)]);
        fx.write_bytes(BIN, BIN_WORK);
        fx.write(TEXT, &format!("a\n{PICKED}\nb\n{OTHER}\n"));
        fx.write_bytes(NEW_BIN, NEW_BIN_DATA);
        fx.write(FLIP, &format!("text\n{AGAIN}\n"));
        fx.build(dir);
    }

    /// The state after staging [`PICKED`] alone (the line beside the binary files). One
    /// git process.
    pub fn expect_bn01_line(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BN-01 after staging a line beside the binary files");
        let ix = e.blobs(&[&format!(":{TEXT}"), &format!(":{BIN}")]);
        c.text("index a.txt", ix[0].as_deref(), Some(&format!("a\n{PICKED}\nb\n")));
        c.bytes("index bin.dat", ix[1].as_deref(), Some(BIN_HEAD));
        c.done()
    }

    /// The state after staging the two binary files alone. One git process.
    pub fn expect_bn01_binaries(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BN-01 after staging the binary files");
        let ix = e.blobs(&[&format!(":{TEXT}"), &format!(":{BIN}"), &format!(":{NEW_BIN}")]);
        c.text("index a.txt", ix[0].as_deref(), Some("a\nb\n"));
        c.bytes("index bin.dat", ix[1].as_deref(), Some(BIN_WORK));
        c.bytes("index new.bin", ix[2].as_deref(), Some(NEW_BIN_DATA));
        c.done()
    }

    /// After unstaging a staged `bin.dat` (the fixture's change staged with `git add`):
    /// the committed bytes in the index, the change still on disk. One git process.
    pub fn expect_bn01_unstaged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BN-01 after unstaging bin.dat");
        c.bytes("index bin.dat", e.index(BIN).as_deref(), Some(BIN_HEAD));
        c.bytes("bin.dat on disk", e.file(BIN).as_deref(), Some(BIN_WORK));
        c.done()
    }

    /// After discarding the unstaged `bin.dat`: the committed bytes on disk. No git
    /// process.
    pub fn expect_bn01_discarded_only(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BN-01 after discarding bin.dat");
        c.bytes("bin.dat on disk", e.file(BIN).as_deref(), Some(BIN_HEAD));
        c.done()
    }

    /// The state after staging `flip.txt` whole: text in HEAD and index, the staged diff a
    /// text one. Three git processes (the fixture with `flip.txt`'s text staged).
    pub fn fixture_flip_staged(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(FLIP, "text\n")]);
        fx.stage(&[put(FLIP, &format!("text\n{AGAIN}\n"))]);
        fx.build(dir);
    }

    /// After staging `flip.txt` whole: its text in the index. One git process.
    pub fn expect_bn02_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BN-02 after staging flip.txt");
        c.text("index flip.txt", e.index(FLIP).as_deref(), Some(&format!("text\n{AGAIN}\n")));
        c.done()
    }

    /// After staging the line and both binary files. One git process.
    pub fn expect_bn01_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BN-01 after staging a line and the binary files");
        let ix = e.blobs(&[&format!(":{TEXT}"), &format!(":{BIN}"), &format!(":{NEW_BIN}")]);
        c.text("index a.txt", ix[0].as_deref(), Some(&format!("a\n{PICKED}\nb\n")));
        c.bytes("index bin.dat", ix[1].as_deref(), Some(BIN_WORK));
        c.bytes("index new.bin", ix[2].as_deref(), Some(NEW_BIN_DATA));
        c.done()
    }

    /// After unstaging and discarding `bin.dat`: the committed bytes in index and on disk,
    /// the rest as staged before. One git process.
    pub fn expect_bn01_discarded(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BN-01 after unstaging and discarding bin.dat");
        let ix = e.blobs(&[&format!(":{BIN}"), &format!(":{NEW_BIN}")]);
        c.bytes("index bin.dat", ix[0].as_deref(), Some(BIN_HEAD));
        c.bytes("bin.dat on disk", e.file(BIN).as_deref(), Some(BIN_HEAD));
        c.bytes("index new.bin", ix[1].as_deref(), Some(NEW_BIN_DATA));
        c.done()
    }

    /// After staging `flip.txt` and unstaging its line [`AGAIN`]: the index as committed,
    /// the line still on disk. One git process.
    pub fn expect_bn02(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BN-02 after staging flip.txt and unstaging a line");
        c.text("index flip.txt", e.index(FLIP).as_deref(), Some("text\n"));
        c.text("flip.txt on disk", e.file(FLIP).as_deref(), Some(&format!("text\n{AGAIN}\n")));
        c.done()
    }
}

/// **BG-02 — very long lines.** `long.txt` holds a 200 000-character line, which the user's
/// edit changed by its last character ([`after`]) and an added line [`ADDED`]. The user
/// stages the changed line (its `-` and `+`) and discards [`ADDED`].
///
/// Expected ([`expect`]): index and working tree both hold the new long line byte for
/// byte, without [`ADDED`].
pub mod long_lines {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const PATH: &str = "long.txt";
    pub const LEN: usize = 200_000;
    pub const ADDED: &str = "added";

    /// The committed long line.
    pub fn before() -> String {
        "a".repeat(LEN)
    }

    /// The edited long line (the last character changed).
    pub fn after() -> String {
        format!("{}b", "a".repeat(LEN - 1))
    }

    /// Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(PATH, &format!("short\n{}\nend\n", before()))]);
        fx.write(PATH, &format!("short\n{}\nend\n{ADDED}\n", after()));
        fx.build(dir);
    }

    /// After staging the changed long line alone: index as [`expect`] wants it, the
    /// added line still on disk. One git process.
    pub fn expect_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BG-02 after staging the long line");
        let want = format!("short\n{}\nend\n", after());
        let ix = e.index(PATH);
        c.ok("index long.txt", ix.as_deref() == Some(want.as_bytes()), ix.as_ref().map(Vec::len));
        c.done()
    }

    /// After discarding [`ADDED`] alone: the long line as edited on disk, without it. No
    /// git process.
    pub fn expect_discarded(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BG-02 after discarding the added line");
        let want = format!("short\n{}\nend\n", after());
        let disk = e.file(PATH);
        c.ok("long.txt on disk", disk.as_deref() == Some(want.as_bytes()), disk.as_ref().map(Vec::len));
        c.done()
    }

    /// One git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BG-02 after staging the long line and discarding the added one");
        let want = format!("short\n{}\nend\n", after());
        // Compared as lengths and equality: a 200 KB mismatch printed whole helps nobody.
        let ix = e.index(PATH);
        let disk = e.file(PATH);
        c.ok("index long.txt", ix.as_deref() == Some(want.as_bytes()), ix.as_ref().map(Vec::len));
        c.ok("long.txt on disk", disk.as_deref() == Some(want.as_bytes()), disk.as_ref().map(Vec::len));
        c.done()
    }
}

/// **BG-03 — a long history.** [`COUNT`] commits on `main`, each changing `n.txt` (one
/// `git fast-import`). The user scrolls to the oldest commits and selects the first one.
///
/// Expected: every commit listed (the newest first, a partial history before the full
/// one), the selected commit's diff shown; nothing in the repository changes
/// ([`expect`]).
pub mod long_history {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const COUNT: usize = 5_000;
    pub const PATH: &str = "n.txt";

    pub fn subject(i: usize) -> String {
        format!("commit {i}")
    }

    /// Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        for i in 1..=COUNT {
            fx.commit("main", &subject(i), &[put(PATH, &format!("{i}\n"))]);
        }
        fx.build(dir);
    }

    /// No git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BG-03 after browsing the long history");
        c.eq("state", e.state(), OpState::Clean);
        c.eq("branch", e.branch(), Some("main".to_string()));
        c.text("n.txt", e.file(PATH).as_deref(), Some(&format!("{COUNT}\n")));
        c.done()
    }
}

/// **BG-01 — a commit with a huge diff.** `side` adds [`FILES`] files of [`LINES`] lines
/// (over 100 000 diff lines — more than the apps' commit cache keeps); `main` merges it
/// ([`MERGE_SUBJECT`], HEAD). The user selects the merge, then its neighbour.
///
/// Expected: the merge shows after one load (one `git show` of it, every file listed) and
/// is not loaded again for the neighbour's prefetch; nothing in the repository changes.
pub mod huge_merge {
    use std::path::Path;

    use crate::fixture::{Change, Fixture, Ids, Mark, put};

    pub const FILES: usize = 1_700;
    pub const LINES: usize = 60;
    pub const MERGE_SUBJECT: &str = "Merge side";

    /// The `i`th file (0-based).
    pub fn name(i: usize) -> String {
        format!("f/{i:04}.txt")
    }

    /// The commits: `init`, the side commit, the merge.
    pub struct Setup {
        pub ids: Ids,
        pub init: Mark,
        pub side: Mark,
        pub merge: Mark,
    }

    /// Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("README", "r\n")]);
        let files: Vec<Change> = (0..FILES)
            .map(|i| put(&name(i), &super::numbered(&format!("{i} line "), LINES)))
            .collect();
        let side = fx.commit_from("side", init, "Add many files", &files);
        let merge = fx.merge("main", &[init, side], MERGE_SUBJECT, &files);
        let ids = fx.build(dir);
        Setup { ids, init, side, merge }
    }
}

/// **NU-02 — UTF-16 with a `diff` attribute.** `.gitattributes` says `*.txt diff`, so git
/// shows `u.txt` — UTF-16 LE with a byte order mark — as text. The user's editor added
/// [`STAGED`] and [`DISCARDED`]; the user stages the first, discards the second
/// ([`expect_staged`]), then unstages the staged line again ([`expect_unstaged`]).
///
/// Expected: each step moves the exact UTF-16 bytes.
pub mod utf16 {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::ext::{Encoding, encode};
    use crate::fixture::{Fixture, put, put_bytes};

    pub const PATH: &str = "u.txt";
    pub const STAGED: &str = "ä x";
    pub const DISCARDED: &str = "中 y";

    pub fn base() -> Vec<u8> {
        encode("a\nb\nz\n", Encoding::Utf16Le)
    }

    /// `base` with [`STAGED`] (after `b`).
    pub fn one() -> Vec<u8> {
        encode(&format!("a\nb\n{STAGED}\nz\n"), Encoding::Utf16Le)
    }

    /// Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(".gitattributes", "*.txt diff\n"), put_bytes(PATH, &base())]);
        fx.write_bytes(PATH, &encode(&format!("a\nb\n{STAGED}\n{DISCARDED}\nz\n"), Encoding::Utf16Le));
        fx.build(dir);
    }

    /// The state after [`expect_staged`] made directly: [`STAGED`] staged, the working
    /// tree the same. Three git processes.
    pub fn fixture_staged(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(".gitattributes", "*.txt diff\n"), put_bytes(PATH, &base())]);
        fx.stage(&[put_bytes(PATH, &one())]);
        fx.build(dir);
    }

    /// After staging one line and discarding the other. One git process.
    pub fn expect_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-02 after staging a line and discarding another");
        c.bytes("index u.txt", e.index(PATH).as_deref(), Some(&one()));
        c.bytes("u.txt on disk", e.file(PATH).as_deref(), Some(&one()));
        c.done()
    }

    /// After unstaging the staged line. One git process.
    pub fn expect_unstaged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-02 after unstaging the line");
        c.bytes("index u.txt", e.index(PATH).as_deref(), Some(&base()));
        c.bytes("u.txt on disk", e.file(PATH).as_deref(), Some(&one()));
        c.done()
    }
}

/// **NU-05 — odd names everywhere.** Committed files whose names have spaces, a leading
/// dash, glob and pathspec-magic characters, CJK and emoji — on unix also a newline, a
/// tab, quotes and a backslash — each changed by one added line; as many untracked files
/// with odd names. The user stages the hunks of some (patches name them), stages every
/// file by group (their paths in one `git add`), unstages them all, discards them by
/// group.
///
/// Expected ([`expect_lines_of`], [`expect_all_staged`], [`expect_nothing_staged`],
/// [`expect_clean`]): each step reaches exactly the named files.
pub mod odd_names {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    /// Names every file system takes.
    const PORTABLE: &[&str] =
        &["with space.txt", "-rf", "日本語.txt", "[x] y.txt", "emoji 🦀.rs", "dir with space/-x #y.txt"];
    /// Names only unix allows.
    #[cfg(unix)]
    const UNIX: &[&str] = &["new\nline.txt", "q\"uote.txt", "tab\there.txt", "back\\slash.txt", "*star?.txt", ":(glob)magic"];

    /// The tracked files' names on this platform.
    pub fn tracked() -> Vec<&'static str> {
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut names = PORTABLE.to_vec();
        #[cfg(unix)]
        names.extend(UNIX);
        names
    }

    /// The untracked files' names: the tracked ones (without their folder) with ` new`
    /// appended.
    pub fn untracked() -> Vec<String> {
        tracked().iter().map(|n| format!("{} new", n.replace('/', " "))).collect()
    }

    /// The line each tracked file gains (and the user stages).
    pub fn added(i: usize) -> String {
        format!("added {i}")
    }

    /// Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        let names = tracked();
        let files: Vec<_> = names.iter().map(|n| put(n, "one\ntwo\n")).collect();
        fx.commit("main", "init", &files);
        for (i, n) in names.iter().enumerate() {
            fx.write(n, &format!("one\ntwo\n{}\n", added(i)));
        }
        for n in untracked() {
            fx.write(&n, "new\n");
        }
        fx.build(dir);
    }

    /// After staging the added line (or hunk) of the tracked files `names`: exactly
    /// theirs staged. One git process.
    pub fn expect_lines_of(dir: &Path, names: &[&str]) -> Result<(), String> {
        let mut c = Checks::new(&format!("NU-05 after staging the lines of {names:?}"));
        let all = tracked();
        for (i, (n, got)) in all.iter().zip(super::index_blobs(dir, &all)).enumerate() {
            let want = if names.contains(n) { format!("one\ntwo\n{}\n", added(i)) } else { "one\ntwo\n".into() };
            c.text(&format!("index {n:?}"), got.as_deref(), Some(&want));
        }
        c.done()
    }

    /// After staging every file: each tracked one modified, each untracked one added —
    /// in the index only. One git process.
    pub fn expect_all_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-05 after staging every file");
        let mut want: Vec<String> = untracked().iter().map(|n| format!("A  {n}")).collect();
        want.extend(tracked().iter().map(|n| format!("M  {n}")));
        want.sort();
        c.eq("status", e.status_lines(), want);
        c.done()
    }

    /// After unstaging every file: as the fixture left them. One git process.
    pub fn expect_nothing_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-05 after unstaging every file");
        let mut want: Vec<String> = untracked().iter().map(|n| format!("?? {n}")).collect();
        want.extend(tracked().iter().map(|n| format!(" M {n}")));
        want.sort();
        c.eq("status", e.status_lines(), want);
        c.done()
    }

    /// After discarding every change and deleting the untracked files: a clean tree. One
    /// git process.
    pub fn expect_clean(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-05 after discarding everything");
        c.eq("status", e.status_lines(), Vec::<String>::new());
        for n in tracked() {
            c.text(&format!("{n:?} on disk"), e.file(n).as_deref(), Some("one\ntwo\n"));
        }
        c.done()
    }
}

/// **NU-04 — a file name that isn't UTF-8.** An untracked file `caf\xe9.txt` (Latin-1)
/// beside a tracked `keep.txt`. The apps list it (its name with U+FFFD), refuse its hunks
/// and lines before any git runs — in plain words — and leave the repository alone
/// ([`expect`]). Only where the file system takes such a name ([`holds`]: Linux; macOS
/// refuses it).
pub mod non_utf8_name {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    /// The name's bytes and how the apps show it.
    pub const RAW: &[u8] = b"caf\xe9.txt";
    pub const SHOWN: &str = "caf\u{fffd}.txt";

    /// Whether `dir`'s file system takes a name that isn't UTF-8 (writes and removes a
    /// probe file).
    #[cfg(unix)]
    pub fn holds(dir: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt as _;
        let p = dir.join(std::ffi::OsStr::from_bytes(b"probe-\xe9"));
        let ok = std::fs::write(&p, "").is_ok();
        let _ = std::fs::remove_file(&p);
        ok
    }

    /// `keep.txt` committed (three git processes); the untracked file written when the
    /// file system takes it ([`holds`]). Whether it was.
    pub fn fixture(dir: &Path) -> bool {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("keep.txt", "k\n")]);
        fx.build(dir);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            if holds(dir) {
                std::fs::write(dir.join(std::ffi::OsStr::from_bytes(RAW)), "one\ntwo\n").unwrap();
                return true;
            }
        }
        false
    }

    /// Nothing staged, the file untouched. One git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-04 after the refused line stage");
        c.eq("status", e.status_lines(), vec![format!("?? {SHOWN}")]);
        c.done()
    }
}

/// **FS-03 / FS-04 — names that differ only in case or Unicode form.** `Readme.md`
/// renamed to `README.md` in the working tree (FS-03), and two committed files whose names
/// are "café" in NFC and in NFD (FS-04), both changed. Only where the file system keeps
/// such names apart ([`holds_case`], [`holds_nfd`]: Linux; macOS folds both).
///
/// - FS-03: the user stages both halves (a rename once staged), then unstages them.
/// - FS-04: the user stages the NFD file's hunk and discards the NFC file's added line
///   ([`expect_fs04`]: each change reached its own file).
pub mod case_and_form {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const BEFORE: &str = "Readme.md";
    pub const AFTER: &str = "README.md";
    pub const NFC: &str = "caf\u{e9}.txt";
    pub const NFD: &str = "cafe\u{301}.txt";

    /// Whether `dir`'s file system keeps `x` and `X` apart.
    pub fn holds_case(dir: &Path) -> bool {
        let (a, b) = (dir.join("probe-case-x"), dir.join("probe-case-X"));
        std::fs::write(&a, "a").unwrap();
        let apart = !b.exists();
        let _ = std::fs::remove_file(&a);
        apart
    }

    /// Whether `dir`'s file system keeps NFC and NFD names apart.
    pub fn holds_nfd(dir: &Path) -> bool {
        let (a, b) = (dir.join("probe-caf\u{e9}"), dir.join("probe-cafe\u{301}"));
        std::fs::write(&a, "a").unwrap();
        let apart = !b.exists();
        let _ = std::fs::remove_file(&a);
        apart
    }

    /// The case rename where the disk keeps the names apart, the NFC / NFD pair where it
    /// does. Three git processes. (whether the case part, the form part were written).
    pub fn fixture(dir: &Path) -> (bool, bool) {
        std::fs::create_dir_all(dir).unwrap();
        let (case, form) = (holds_case(dir), holds_nfd(dir));
        let mut fx = Fixture::new();
        let mut files = vec![put("other.txt", "o\n")];
        if case {
            files.push(put(BEFORE, "# hi\n"));
        }
        if form {
            files.push(put(NFC, "nfc\n"));
            files.push(put(NFD, "nfd\n"));
        }
        fx.commit("main", "init", &files);
        if case {
            fx.remove(BEFORE);
            fx.write(AFTER, "# hi\n");
        }
        if form {
            fx.write(NFC, "nfc\nchanged\n");
            fx.write(NFD, "nfd\nchanged\n");
        }
        fx.build(dir);
        (case, form)
    }

    /// FS-03 after staging both halves: `README.md` staged as a rename. One git process.
    pub fn expect_fs03_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("FS-03 after staging both halves");
        let st = e.status();
        let r = st.iter().find(|s| s.path == AFTER).map(|s| (s.xy.clone(), s.from.clone()));
        c.eq("status of README.md", r, Some(("R ".to_string(), Some(BEFORE.to_string()))));
        c.done()
    }

    /// FS-03 after unstaging both halves. One git process.
    pub fn expect_fs03_unstaged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("FS-03 after unstaging both halves");
        let st = e.status();
        let of = |p: &str| st.iter().find(|s| s.path == p).map(|s| s.xy.clone());
        c.eq("status of Readme.md", of(BEFORE), Some(" D".into()));
        c.eq("status of README.md", of(AFTER), Some("??".into()));
        c.done()
    }

    /// FS-04 after staging the NFD hunk and discarding the NFC line. One git process.
    pub fn expect_fs04(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("FS-04 after staging NFD and discarding NFC");
        let ix = e.blobs(&[&format!(":{NFD}"), &format!(":{NFC}")]);
        c.text("index NFD", ix[0].as_deref(), Some("nfd\nchanged\n"));
        c.text("index NFC", ix[1].as_deref(), Some("nfc\n"));
        c.text("NFC on disk", e.file(NFC).as_deref(), Some("nfc\n"));
        c.text("NFD on disk", e.file(NFD).as_deref(), Some("nfd\nchanged\n"));
        c.done()
    }
}

/// **FS-05 / FS-06 — symlinks** (unix). Committed: `target.txt`, `link` → `target.txt`,
/// `gone` → `target.txt`, `real/x.txt` and `alias` → `real` (a folder). In the working
/// tree `link` points at `other.txt` now, `gone` was deleted, `new_link` → `target.txt`
/// and `dangling` → `nowhere` are new, and `real/x.txt` was edited (also reachable as
/// `alias/x.txt`, beyond a symlink).
///
/// The user stages the retargeted `link`'s hunk, `new_link` and `dangling` whole and the
/// edit of `real/x.txt` ([`expect_staged`]: the index holds the links — mode 120000,
/// their targets as content — never what they point at; nothing beyond `alias`), and
/// discards the deletion of `gone` ([`expect_restored`]: a link again).
#[cfg(unix)]
pub mod symlinks {
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, link, put};

    pub const LINK: &str = "link";
    pub const GONE: &str = "gone";
    pub const NEW_LINK: &str = "new_link";
    pub const DANGLING: &str = "dangling";
    pub const BEYOND: &str = "real/x.txt";
    /// What `link` points at now.
    pub const RETARGET: &str = "other.txt";

    /// Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit(
            "main",
            "init",
            &[
                put("target.txt", "t\n"),
                link(LINK, "target.txt"),
                link(GONE, "target.txt"),
                put(BEYOND, "x\n"),
                link("alias", "real"),
            ],
        );
        fx.remove(LINK);
        fx.remove(GONE);
        fx.write(BEYOND, "x\ny\n");
        fx.build(dir);
        symlink(RETARGET, dir.join(LINK)).unwrap();
        symlink("target.txt", dir.join(NEW_LINK)).unwrap();
        symlink("nowhere", dir.join(DANGLING)).unwrap();
    }

    /// After the stages. Two git processes.
    pub fn expect_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("FS-05 / FS-06 after staging the links");
        let ix = e.blobs(&[":link", ":new_link", ":dangling", ":real/x.txt", ":target.txt"]);
        c.text("index link", ix[0].as_deref(), Some(RETARGET));
        c.text("index new_link", ix[1].as_deref(), Some("target.txt"));
        c.text("index dangling", ix[2].as_deref(), Some("nowhere"));
        c.text("index real/x.txt", ix[3].as_deref(), Some("x\ny\n"));
        c.text("index target.txt", ix[4].as_deref(), Some("t\n"));
        let modes = super::index_modes(dir, &[LINK, NEW_LINK, DANGLING, "alias/x.txt"]);
        c.eq(
            "index modes",
            modes,
            vec![Some("120000".into()), Some("120000".into()), Some("120000".into()), None],
        );
        c.text("target.txt on disk", e.file("target.txt").as_deref(), Some("t\n"));
        c.done()
    }

    /// After discarding the deletion of `gone`: a link again. One git process.
    pub fn expect_restored(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("FS-05 after discarding the deleted link");
        let gone = std::fs::read_link(dir.join(GONE)).ok();
        c.eq("gone is a link again", gone, Some("target.txt".into()));
        c.ok("gone not listed", !e.status_lines().iter().any(|l| l.ends_with(" gone")), "listed");
        c.done()
    }
}

/// **FS-07 — special files** (unix). Committed: `a` and `b` with the same content,
/// `empty` (empty). After the checkout `b` is made a hard link of `a`, and `a` edited —
/// so `b` changed too; `empty` got a line; `new_empty` is a new empty file; `pipe` is a
/// FIFO (git skips it). The user stages `b`'s hunk, `empty`'s hunk and `new_empty` whole
/// ([`expect_staged`]), and discards `a`'s hunk ([`expect_discarded`]: `a` as committed,
/// `b` keeps the edit — git replaces the file, so the link breaks). The refresh never
/// blocks on the FIFO.
#[cfg(unix)]
pub mod special_files {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const A: &str = "a";
    pub const B: &str = "b";
    pub const EMPTY: &str = "empty";
    pub const NEW_EMPTY: &str = "new_empty";
    pub const PIPE: &str = "pipe";

    /// Three git processes and `mkfifo` (whether the FIFO could be made).
    pub fn fixture(dir: &Path) -> bool {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(A, "1\n2\n"), put(B, "1\n2\n"), put(EMPTY, "")]);
        fx.write(EMPTY, "now\n");
        fx.write(NEW_EMPTY, "");
        fx.build(dir);
        std::fs::remove_file(dir.join(B)).unwrap();
        std::fs::hard_link(dir.join(A), dir.join(B)).unwrap();
        std::fs::write(dir.join(A), "1\n2\n3\n").unwrap();
        std::process::Command::new("mkfifo")
            .arg(dir.join(PIPE))
            .status()
            .is_ok_and(|s| s.success())
    }

    /// After staging `b`'s hunk, `empty` and `new_empty`. One git process.
    pub fn expect_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("FS-07 after staging");
        let ix = e.blobs(&[":a", ":b", ":empty", ":new_empty"]);
        c.text("index a", ix[0].as_deref(), Some("1\n2\n"));
        c.text("index b", ix[1].as_deref(), Some("1\n2\n3\n"));
        c.text("index empty", ix[2].as_deref(), Some("now\n"));
        c.text("index new_empty", ix[3].as_deref(), Some(""));
        c.done()
    }

    /// After discarding `a`'s hunk: `a` as committed, its hard link `b` keeps the edit
    /// (git writes `a` anew). No git process.
    pub fn expect_discarded(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("FS-07 after discarding a's hunk");
        c.text("a on disk", e.file(A).as_deref(), Some("1\n2\n"));
        c.text("b on disk", e.file(B).as_deref(), Some("1\n2\n3\n"));
        c.done()
    }
}

/// **FS-08 — git refuses a `.git` alias; a clone's names collide.** `main` has `a.txt`;
/// [`EVIL`] adds `GIT~1/config` — another spelling of `.git` on Windows file systems,
/// which git refuses to check out everywhere (`core.protectNTFS`). The user switches to
/// [`EVIL`]: refused, explained in plain words ([`REFUSAL`]), HEAD stays on `main`
/// ([`expect_refused`]).
///
/// [`collide_remote`]: a bare repository whose `main` has `X.h` and `x.h`; cloned onto a
/// case-insensitive disk (a [`super::case_and_form::holds_case`] that is `false`), the apps
/// say which paths collided ([`COLLIDED`]).
pub mod git_alias {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    pub const EVIL: &str = "evil";
    pub const ALIAS: &str = "GIT~1/config";
    /// How the apps' error starts.
    pub const REFUSAL: &str = "git can't check out 1 path on this file system";
    /// How the clone's note starts.
    pub const COLLIDED: &str = "Cloned — but 2 paths collided";

    /// Four git processes: [`EVIL`] comes from a second `git fast-import` that is told to
    /// take the path (`core.protectNTFS=false` for that process only — the repository keeps
    /// git's default, which refuses it).
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.build(dir);
        let who = crate::fixture::TESTER;
        let t = crate::fixture::START_TIME + 3600;
        let stream = format!(
            "commit refs/heads/{EVIL}\ncommitter {who} {t} +0000\ndata 6\nalias\nfrom refs/heads/main^0\n\
             M 100644 inline {ALIAS}\ndata 2\nx\n\n"
        );
        let out = crate::git::run(
            dir,
            &["-c", "core.protectNTFS=false", "fast-import", "--quiet"],
            Some(stream.as_bytes()),
        );
        assert!(out.status.success(), "fast-import: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A bare repository at `dir` whose `main` holds `X.h` and `x.h` (one git process).
    pub fn collide_remote(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "the pair", &[put("X.h", "upper\n"), put("x.h", "lower\n")]);
        fx.build_bare(dir);
    }

    /// After the refused switch: still on `main`, nothing changed. One git process.
    pub fn expect_refused(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("FS-08 after the refused switch");
        c.eq("branch", e.branch(), Some("main".to_string()));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **IG-06 — negation and the excludes file.** `.gitignore` ignores `*.log`,
/// `logs/.gitignore` takes `keep.log` back (`!keep.log`), and `core.excludesFile` (a file
/// outside the work tree, as a global one is) ignores `*.tmp`. Untracked: `a.log`,
/// `logs/keep.log`, `logs/drop.log`, `x.tmp`, `shown.txt`. The user stages the untracked
/// files.
///
/// Expected ([`expect`]): listed and staged are exactly what git decides —
/// [`SHOWN`].
pub mod ignore_rules {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    /// The untracked files git shows, sorted.
    pub const SHOWN: [&str; 2] = ["logs/keep.log", "shown.txt"];
    /// The ones it ignores.
    pub const HIDDEN: [&str; 3] = ["a.log", "logs/drop.log", "x.tmp"];

    /// Three git processes. The excludes file lives next to the repository (`dir` +
    /// `.excludes`).
    pub fn fixture(dir: &Path) {
        let excludes = dir.with_extension("excludes");
        std::fs::create_dir_all(excludes.parent().unwrap()).unwrap();
        std::fs::write(&excludes, "*.tmp\n").unwrap();
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(".gitignore", "*.log\n"), put("logs/.gitignore", "!keep.log\n")]);
        let path = excludes.display().to_string().replace('\\', "/");
        fx.config(&format!("[core]\n\texcludesFile = {path}\n"));
        for f in SHOWN.iter().chain(&HIDDEN) {
            fx.write(f, "x\n");
        }
        fx.build(dir);
    }

    /// One git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("IG-06 after staging the untracked files");
        let want: Vec<String> = SHOWN.iter().map(|p| format!("A  {p}")).collect();
        c.eq("status", e.status_lines(), want);
        for h in HIDDEN {
            c.ok(&format!("{h} still on disk"), e.file(h).is_some(), "gone");
        }
        c.done()
    }
}

/// **CM-20 — Unicode and other encodings in messages.** History: `init`, then
/// [`LATIN1_SUBJECT`] committed with `i18n.commitEncoding = ISO-8859-1` (its message in
/// Latin-1, an `encoding` header). `a.txt` is changed. The user sees the old commit's
/// message decoded, stages `a.txt` and commits [`MESSAGE`] (CJK, emoji, a family emoji,
/// right-to-left text).
///
/// Expected ([`expect`]): the new commit's message is exactly [`MESSAGE`] (UTF-8, no
/// encoding header), as git reads it back.
pub mod unicode_messages {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    /// The Latin-1 commit's subject, decoded.
    pub const LATIN1_SUBJECT: &str = "Grüße für Jürgen";
    /// What the user commits.
    pub const MESSAGE: &str = "修正：日本語 🦀 zoë\n\nRücksicht 👩‍👩‍👧 مرحبا";

    /// Four git processes (the Latin-1 commit is git's own).
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.write("a.txt", "a\nb\n");
        let latin1: Vec<u8> = LATIN1_SUBJECT.chars().map(|c| c as u8).collect();
        fx.write_bytes(".git/latin1-message", &latin1);
        fx.git_after(&[
            "-c",
            "i18n.commitEncoding=ISO-8859-1",
            "commit",
            "-q",
            "--allow-empty",
            "-F",
            ".git/latin1-message",
        ]);
        fx.build(dir);
    }

    /// After the commit. One git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-20 after committing a Unicode message");
        let log = e.log("HEAD", 2);
        c.eq("subjects", log.iter().map(|l| l.message.clone()).collect::<Vec<_>>(), vec![
            MESSAGE.to_string(),
            LATIN1_SUBJECT.to_string(),
        ]);
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **CM-20 — a repository whose messages are in a legacy encoding.**
/// `i18n.commitEncoding = ISO-8859-1` in `.git/config`, `a.txt` changed. The user stages
/// it and types [`TYPED`]: before committing the box says what ISO-8859-1 lacks and what
/// is written instead ([`NOTE`]); then commits.
///
/// Expected ([`expect`]): the commit records `encoding ISO-8859-1` and its message is
/// [`WRITTEN`] in Latin-1 bytes (the umlauts as themselves, the dash, quotes and euro as
/// their ASCII equivalents, the CJK character as `?`); the work tree is clean.
pub mod legacy_encoding {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};

    /// What the user types.
    pub const TYPED: &str = "Grüße – „Größe“ 5 € 漢";
    /// What the commit holds (decoded).
    pub const WRITTEN: &str = "Grüße - \"Größe\" 5 EUR ?";
    /// What the box says before committing.
    pub const NOTE: &str =
        "Not in ISO-8859-1: – → -, „ → \", “ → \", € → EUR, 漢 → ? (i18n.commitEncoding in .git/config)";

    /// Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.config("[i18n]\n\tcommitEncoding = ISO-8859-1\n");
        fx.write("a.txt", "a\nb\n");
        fx.build(dir);
    }

    /// After the commit. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("CM-20 after committing in ISO-8859-1");
        let raw = e.blobs(&["HEAD"]).pop().flatten().unwrap_or_default();
        let at = raw.windows(2).position(|w| w == b"\n\n").unwrap_or(raw.len());
        let header = String::from_utf8_lossy(&raw[..at]).into_owned();
        let enc = header.lines().find_map(|l| l.strip_prefix("encoding ")).map(String::from);
        c.eq("encoding header", enc.as_deref(), Some("ISO-8859-1"));
        let latin1: Vec<u8> = WRITTEN.chars().map(|ch| ch as u8).chain(*b"\n").collect();
        c.bytes("message bytes", raw.get(at + 2..), Some(&latin1[..]));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}
