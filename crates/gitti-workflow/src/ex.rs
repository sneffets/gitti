//! EX — changes behind the app's back that are not plain edits: an editor that rewrites
//! line endings, adds or drops a byte order mark, saves in another code page or as UTF-16,
//! adds or drops the last newline; and the rules that convert files on their way in and out
//! of the index (`core.autocrlf`, the `text` / `eol` attributes, renormalizing after
//! `.gitattributes` changed).
//!
//! Every fixture pins `core.autocrlf` (the fixture's [`crate::fixture::TEST_CONFIG`]: false;
//! EX-05: true) so the apps, which read the user's global config, see the same on every
//! platform (Git for Windows installs `core.autocrlf=true` globally). Contents are checked
//! byte for byte ([`Checks::bytes`]).

use std::path::Path;

use crate::expect::{Checks, Expect};
use crate::ext;
use crate::fixture::{Fixture, put, put_bytes};

/// `n` numbered lines `p 1` … `p n`, line `i` ending in CRLF where `crlf(i)` (else LF),
/// the lines in `with` replaced (1-based).
pub fn numbered(p: &str, n: usize, crlf: impl Fn(usize) -> bool, with: &[(usize, &str)]) -> String {
    (1..=n)
        .map(|i| {
            let text = match with.iter().find(|(at, _)| *at == i) {
                Some((_, w)) => w.to_string(),
                None => format!("{p} {i}"),
            };
            format!("{text}{}", if crlf(i) { "\r\n" } else { "\n" })
        })
        .collect()
}

/// The status lines (`XY path`), sorted as [`Expect::status_lines`] gives them.
fn status(lines: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    v.sort();
    v
}

fn lf(_: usize) -> bool {
    false
}

fn crlf(_: usize) -> bool {
    true
}

/// **EX-02 — an editor converts a whole file from LF to CRLF** (no `core.autocrlf`, no
/// attributes). `a.txt` ([`N`] lines, LF) committed; another program rewrites every line
/// ending as CRLF ([`ext::crlf_all`]). The user stages the last line's change (its `-`
/// and its `+` line, [`STAGED`]) and then discards the first line's ([`DISCARDED`]).
///
/// Expected: the diff says why every line changed ([`NOTE`]); afterwards
/// ([`expect`]) the index has line [`STAGED`] with CRLF and the others as committed,
/// the working tree CRLF except line [`DISCARDED`], LF again.
///
/// The last and the first line on purpose: the `-` / `+` pair of a line in the middle of
/// the block (all `-` lines, then all `+` lines) lands at the block's end — line staging
/// keeps the diff's order (open point in `WORKFLOWS.md`).
pub mod lf_to_crlf {
    use super::*;

    pub const FILE: &str = "a.txt";
    pub const N: usize = 4;
    pub const STAGED: usize = 4;
    pub const DISCARDED: usize = 1;
    /// The note above the file's hunks (gitti-core's `eol_note`).
    pub const NOTE: &str = "4 lines differ only in their line ending (LF → CRLF)";

    pub fn text(crlf: impl Fn(usize) -> bool) -> String {
        numbered("a", N, crlf, &[])
    }

    /// `main` with `a.txt` (LF); then another program converts it to CRLF. Three git
    /// processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(FILE, &text(lf))]);
        fx.build(dir);
        ext::crlf_all(&dir.join(FILE));
    }

    /// After staging line [`STAGED`] and discarding line [`DISCARDED`]. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-02 after staging one line and discarding another");
        c.text("index", e.index(FILE).as_deref(), Some(&text(|i| i == STAGED)));
        c.text("on disk", e.file(FILE).as_deref(), Some(&text(|i| i != DISCARDED)));
        c.eq("status", e.status_lines(), status(&["MM a.txt"]));
        c.done()
    }
}

/// **EX-03 — only some lines get CRLF.** `a.txt` ([`N`] lines, LF) committed; an editor
/// writes the lines it touched, [`LINES`], with CRLF ([`ext::crlf_lines`]). The user stages
/// the first of them (its hunk).
///
/// Expected: only those lines show as changed — two hunks — and the diff says why
/// ([`NOTE`]); afterwards ([`expect`]) the index has the first line with CRLF, the other
/// still unstaged.
pub mod crlf_on_some_lines {
    use super::*;

    pub const FILE: &str = "a.txt";
    pub const N: usize = 20;
    pub const LINES: [usize; 2] = [4, 15];
    pub const NOTE: &str = "2 lines differ only in their line ending (LF → CRLF)";

    pub fn text(crlf: impl Fn(usize) -> bool) -> String {
        numbered("a", N, crlf, &[])
    }

    /// `main` with `a.txt` (LF); then lines [`LINES`] get CRLF. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(FILE, &text(lf))]);
        fx.build(dir);
        ext::crlf_lines(&dir.join(FILE), &LINES);
    }

    /// After staging the first hunk. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-03 after staging the first line");
        c.text("index", e.index(FILE).as_deref(), Some(&text(|i| i == LINES[0])));
        c.text("on disk", e.file(FILE).as_deref(), Some(&text(|i| LINES.contains(&i))));
        c.eq("status", e.status_lines(), status(&["MM a.txt"]));
        c.done()
    }
}

/// **EX-04 — a CRLF file saved with LF.** `w.txt` ([`N`] lines) was committed with CRLF;
/// an editor saves it with LF ([`ext::lf_all`]); `b.txt` stays as it is. The user stages
/// everything (Stage All).
///
/// Expected: every line of `w.txt` changed and the diff says why ([`NOTE`]); afterwards
/// ([`expect`]) the index has `w.txt` with LF.
pub mod crlf_to_lf {
    use super::*;

    pub const FILE: &str = "w.txt";
    pub const N: usize = 5;
    pub const NOTE: &str = "5 lines differ only in their line ending (CRLF → LF)";

    pub fn text(crlf: impl Fn(usize) -> bool) -> String {
        numbered("w", N, crlf, &[])
    }

    /// `main` with `w.txt` (CRLF) and `b.txt`; then `w.txt` saved with LF. Three git
    /// processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(FILE, &text(crlf)), put("b.txt", "b\n")]);
        fx.build(dir);
        ext::lf_all(&dir.join(FILE));
    }

    /// After Stage All. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-04 after Stage All");
        c.text("index", e.index(FILE).as_deref(), Some(&text(lf)));
        c.text("on disk", e.file(FILE).as_deref(), Some(&text(lf)));
        c.eq("status", e.status_lines(), status(&["M  w.txt"]));
        c.done()
    }
}

/// **EX-05 — a `core.autocrlf=true` working tree.** `a.txt` ([`N`] lines) is stored with
/// LF; with `core.autocrlf=true` the working tree has CRLF, and the user's editor changed
/// lines 3 and 17 ([`work`], CRLF). The user stages the file, unstages the hunk of line
/// 17 and discards it.
///
/// Expected ([`expect`]): the index stays LF (line 3 changed), the working tree CRLF with
/// line 3 changed and line 17 as committed — the discard wrote CRLF back.
pub mod autocrlf {
    use super::*;

    pub const FILE: &str = "a.txt";
    pub const N: usize = 20;
    pub const CONFIG: &str = "[core]\n\tautocrlf = true\n";

    fn with(l3: bool, l17: bool) -> Vec<(usize, &'static str)> {
        let mut w = Vec::new();
        if l3 {
            w.push((3, "a 3 changed"));
        }
        if l17 {
            w.push((17, "a 17 changed"));
        }
        w
    }

    /// `a.txt` with LF (the index form) or CRLF (the working-tree form), lines 3 / 17
    /// changed or not.
    pub fn text(crlf: bool, l3: bool, l17: bool) -> String {
        numbered("a", N, |_| crlf, &with(l3, l17))
    }

    /// The working tree as the editor left it.
    pub fn work() -> String {
        text(true, true, true)
    }

    /// `main` with `a.txt` (LF), `core.autocrlf=true`; the checkout writes CRLF, the edit
    /// keeps it. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.config(CONFIG);
        fx.commit("main", "init", &[put(FILE, &text(false, false, false))]);
        fx.write(FILE, &work());
        fx.build(dir);
    }

    /// After staging the file, unstaging and discarding line 17's hunk. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-05 after stage, unstage and discard");
        c.text("index (LF)", e.index(FILE).as_deref(), Some(&text(false, true, false)));
        c.text("on disk (CRLF)", e.file(FILE).as_deref(), Some(&text(true, true, false)));
        c.eq("status", e.status_lines(), status(&["M  a.txt"]));
        c.done()
    }
}

/// **EX-06 — files converted by the `eol=crlf` attribute.** `.gitattributes` says
/// [`ATTRS`]; `a.txt` ([`N`] lines) is stored with LF, the working tree has CRLF, and the
/// user's editor changed lines 3 and 17. The user stages line 3's change (its lines) and
/// discards line 17's hunk.
///
/// Expected ([`expect`]): the index LF with line 3 changed, the working tree CRLF with
/// line 3 changed and line 17 as committed.
pub mod eol_attribute {
    use super::*;

    pub const FILE: &str = "a.txt";
    pub const N: usize = 20;
    pub const ATTRS: &str = "*.txt text eol=crlf\n";

    pub fn text(crlf: bool, l3: bool, l17: bool) -> String {
        super::autocrlf::text(crlf, l3, l17)
    }

    /// `main` with `.gitattributes` and `a.txt` (LF); the checkout writes CRLF, the edit
    /// keeps it. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(".gitattributes", ATTRS), put(FILE, &text(false, false, false))]);
        fx.write(FILE, &text(true, true, true));
        fx.build(dir);
    }

    /// After staging line 3 and discarding line 17. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-06 after staging one line and discarding another");
        c.text("index (LF)", e.index(FILE).as_deref(), Some(&text(false, true, false)));
        c.text("on disk (CRLF)", e.file(FILE).as_deref(), Some(&text(true, true, false)));
        c.eq("status", e.status_lines(), status(&["M  a.txt"]));
        c.done()
    }
}

/// **EX-07 — `.gitattributes` added: renormalize.** `c.txt` was committed with CRLF,
/// `l.txt` with LF, `d.txt` with CRLF. The user writes a `.gitattributes` with [`ATTRS`]
/// (untracked) and has an unfinished edit in `d.txt`. git shows no change of `c.txt`
/// ([`expect_before`]: with `text=auto` it keeps CRLF that is already stored). The user
/// runs Renormalize Line Endings (files with unstaged changes left out, the default).
///
/// Expected ([`expect_renormalized`]): `c.txt` is staged with LF and the staged diff says
/// so ([`NOTE`]), its working-tree file still has CRLF and no unstaged change; `d.txt`
/// and its edit stay unstaged, `.gitattributes` untracked, `l.txt` as it was. With
/// `d.txt` included ([`expect_renormalized_all`]): `d.txt` is staged too, with LF and the
/// edit.
pub mod renormalize {
    use super::*;

    pub const ATTRS: &str = "* text=auto\n";
    /// The staged diff's note of `c.txt`.
    pub const NOTE: &str = "2 lines differ only in their line ending (CRLF → LF)";
    /// The status line after it (gitti-core's `renormalize::Report::note`).
    pub const DONE: &str = "Renormalized line endings: 1 file staged (c.txt) — commit to record it";
    pub const DONE_ALL: &str = "Renormalized line endings: 2 files staged (c.txt, d.txt) — commit to record it";

    pub fn c(crlf: bool) -> String {
        numbered("c", 2, |_| crlf, &[])
    }

    pub fn l() -> String {
        numbered("l", 2, lf, &[])
    }

    pub fn d(crlf: bool, edited: bool) -> String {
        numbered("d", 3, |_| crlf, if edited { &[(2, "d 2 edited")] } else { &[] })
    }

    /// The three files committed; `.gitattributes` and the edit of `d.txt` in the working
    /// tree. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("c.txt", &c(true)), put("l.txt", &l()), put("d.txt", &d(true, false))]);
        fx.write(".gitattributes", ATTRS);
        fx.write("d.txt", &d(true, true));
        fx.build(dir);
    }

    /// Before: only the new `.gitattributes` and the edit show. One git process.
    pub fn expect_before(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-07 before renormalizing");
        c.eq("status", e.status_lines(), status(&["?? .gitattributes", " M d.txt"]));
        c.done()
    }

    fn after(dir: &Path, all: bool) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new(if all { "EX-07 after renormalizing all" } else { "EX-07 after renormalizing" });
        let ix = e.blobs(&[":c.txt", ":l.txt", ":d.txt"]);
        k.text("index c.txt (LF)", ix[0].as_deref(), Some(&c(false)));
        k.text("index l.txt", ix[1].as_deref(), Some(&l()));
        let d_index = if all { d(false, true) } else { d(true, false) };
        k.text("index d.txt", ix[2].as_deref(), Some(&d_index));
        k.text("c.txt on disk (not rewritten)", e.file("c.txt").as_deref(), Some(&c(true)));
        k.text("d.txt on disk", e.file("d.txt").as_deref(), Some(&d(true, true)));
        let d_status = if all { "M  d.txt" } else { " M d.txt" };
        k.eq("status", e.status_lines(), status(&["?? .gitattributes", "M  c.txt", d_status]));
        k.done()
    }

    /// After Renormalize Line Endings without the files with unstaged changes. Two git
    /// processes.
    pub fn expect_renormalized(dir: &Path) -> Result<(), String> {
        after(dir, false)
    }

    /// After Renormalize Line Endings with them. Two git processes.
    pub fn expect_renormalized_all(dir: &Path) -> Result<(), String> {
        after(dir, true)
    }
}

/// **EX-08 — a byte order mark added and removed.** `a.txt` and `b.txt` ([`N`] lines)
/// are committed, `b.txt` with a UTF-8 byte order mark; an editor adds one to `a.txt`
/// ([`ext::add_bom`]) and drops `b.txt`'s ([`ext::strip_bom`]). The user stages `a.txt`'s
/// hunk and discards `b.txt`'s.
///
/// Expected: line 1 of each shows as changed and the diff says why ([`NOTE_ADDED`],
/// [`NOTE_REMOVED`]); afterwards ([`expect`]) the index has `a.txt` with the mark, the
/// working tree `b.txt` with its mark again — byte for byte.
pub mod bom {
    use super::*;

    pub const N: usize = 4;
    pub const NOTE_ADDED: &str = "Byte order mark (BOM) added at the start of the file";
    pub const NOTE_REMOVED: &str = "Byte order mark (BOM) removed from the start of the file";

    pub fn text(p: &str, bom: bool) -> Vec<u8> {
        let mut out = if bom { ext::BOM.to_vec() } else { Vec::new() };
        out.extend_from_slice(numbered(p, N, lf, &[]).as_bytes());
        out
    }

    /// Both files committed; then the mark added to `a.txt`, removed from `b.txt`. Three
    /// git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put_bytes("a.txt", &text("a", false)), put_bytes("b.txt", &text("b", true))]);
        fx.build(dir);
        ext::add_bom(&dir.join("a.txt"));
        ext::strip_bom(&dir.join("b.txt"));
    }

    /// After staging `a.txt` and discarding `b.txt`. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-08 after staging a.txt and discarding b.txt");
        let ix = e.blobs(&[":a.txt", ":b.txt"]);
        c.bytes("index a.txt", ix[0].as_deref(), Some(&text("a", true)));
        c.bytes("index b.txt", ix[1].as_deref(), Some(&text("b", true)));
        c.bytes("a.txt on disk", e.file("a.txt").as_deref(), Some(&text("a", true)));
        c.bytes("b.txt on disk", e.file("b.txt").as_deref(), Some(&text("b", true)));
        c.eq("status", e.status_lines(), status(&["M  a.txt"]));
        c.done()
    }
}

/// **EX-09 — a UTF-8 file saved again as Windows-1252.** `u.txt` ([`TEXT`], UTF-8) is
/// committed; an editor saves it as Windows-1252 ([`ext::reencode`]): lines 1 and 3 (the
/// ones with `€` / `ü` `ß`) change, line 2 is ASCII and stays. The user stages line 1's
/// change (its `-` and `+` line).
///
/// Expected: the changed lines show decoded, the file is named Windows-1252
/// ([`LABEL`]); afterwards ([`expect`]) the index has line 1 in Windows-1252 bytes and
/// line 3 still in UTF-8.
pub mod code_page {
    use super::*;

    pub const FILE: &str = "u.txt";
    pub const TEXT: &str = "Preis: 5 €\nplain\nGrüße\n";
    /// The encoding the apps name on the file row (gitti-core's `TextEncoding::label`).
    pub const LABEL: &str = "Windows-1252";

    /// Line 1 in Windows-1252, the rest as committed.
    pub fn first_line_switched() -> Vec<u8> {
        let mut out = ext::encode("Preis: 5 €\n", ext::Encoding::Cp1252);
        out.extend_from_slice("plain\nGrüße\n".as_bytes());
        out
    }

    /// `u.txt` committed; then saved as Windows-1252. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(FILE, TEXT)]);
        fx.build(dir);
        ext::reencode(&dir.join(FILE), ext::Encoding::Cp1252);
    }

    /// After staging line 1. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-09 after staging line 1");
        c.bytes("index", e.index(FILE).as_deref(), Some(&first_line_switched()));
        let disk = ext::encode(TEXT, ext::Encoding::Cp1252);
        c.bytes("on disk", e.file(FILE).as_deref(), Some(&disk));
        c.eq("status", e.status_lines(), status(&["MM u.txt"]));
        c.done()
    }
}

/// **EX-10 — a text file saved as UTF-16.** `t.txt` ([`TEXT`]) is committed; an editor
/// saves it as UTF-16 LE with a byte order mark ([`ext::reencode`]) — a binary file for
/// git. The user stages the whole file.
///
/// Expected: listed as a binary change (no lines to pick); afterwards ([`expect`]) the
/// index has the UTF-16 bytes exactly.
pub mod utf16 {
    use super::*;

    pub const FILE: &str = "t.txt";
    pub const TEXT: &str = "hello\nworld\n";

    pub fn utf16() -> Vec<u8> {
        ext::encode(TEXT, ext::Encoding::Utf16Le)
    }

    /// `t.txt` committed; then saved as UTF-16. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(FILE, TEXT)]);
        fx.build(dir);
        ext::reencode(&dir.join(FILE), ext::Encoding::Utf16Le);
    }

    /// After staging the file. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-10 after staging the file");
        c.bytes("index", e.index(FILE).as_deref(), Some(&utf16()));
        c.eq("status", e.status_lines(), status(&["M  t.txt"]));
        c.done()
    }
}

/// **EX-11 — the last newline added and removed.** `a.txt` ([`N`] lines) was committed
/// without a newline after its last line, `b.txt` with one. An editor changes line 2 of
/// `a.txt` and adds the missing newline (two hunks), and drops `b.txt`'s last newline.
/// The user stages `a.txt`'s second hunk (the newline) and discards `b.txt`'s hunk.
///
/// Expected ([`expect`]): the index has `a.txt` as committed plus the newline (line 2
/// unchanged), `b.txt` on disk has its newline again.
pub mod trailing_newline {
    use super::*;

    pub const N: usize = 20;

    /// `p.txt`'s lines, line 2 changed or not, with or without the last newline.
    pub fn text(p: &str, l2: bool, newline: bool) -> String {
        let mut s = numbered(p, N, lf, if l2 { &[(2, "a 2 changed")] } else { &[] });
        if !newline {
            s.pop();
        }
        s
    }

    /// Both files committed; the editor's changes in the working tree. Three git
    /// processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &text("a", false, false)), put("b.txt", &text("b", false, true))]);
        fx.write("a.txt", &text("a", true, true));
        fx.write("b.txt", &text("b", false, false));
        fx.build(dir);
    }

    /// After staging `a.txt`'s second hunk and discarding `b.txt`'s. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("EX-11 after staging the newline and discarding the other");
        c.text("index a.txt", e.index("a.txt").as_deref(), Some(&text("a", false, true)));
        c.text("a.txt on disk", e.file("a.txt").as_deref(), Some(&text("a", true, true)));
        c.text("b.txt on disk", e.file("b.txt").as_deref(), Some(&text("b", false, true)));
        c.eq("status", e.status_lines(), status(&["MM a.txt"]));
        c.done()
    }
}
