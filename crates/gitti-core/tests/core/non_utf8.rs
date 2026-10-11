//! Files that are not UTF-8: hunk / line staging, unstaging and
//! discarding move their exact bytes (patches are built from the lines' bytes, the shown
//! text is a decoding), and the diff says which encoding it shows; the merge editor
//! writes them back byte for byte. Refused only: a file whose *name* is not UTF-8 (paths
//! are strings app-wide).

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::diff::{FileDiff, LineKind, TextEncoding};
use gitti_core::patch::{NotUtf8, PatchMode, Selection};

use crate::fixture_kit::{E, History, f, git, repo_with};

/// The exact bytes of `:name` in the index.
fn index_bytes(p: &Path, name: &str) -> Vec<u8> {
    let out = git(p, &["show", &format!(":{name}")]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

fn find<'a>(files: &'a [gitti_core::WorkFile], name: &str) -> &'a FileDiff {
    files
        .iter()
        .find(|w| w.path() == name)
        .and_then(|w| w.diff.as_ref())
        .unwrap_or_else(|| panic!("no diff of {name}"))
}

/// `(hunk, line)` of the `n`-th added line.
fn added(d: &FileDiff, n: usize) -> (usize, usize) {
    d.hunks
        .iter()
        .enumerate()
        .flat_map(|(h, hunk)| hunk.lines.iter().enumerate().map(move |(l, line)| (h, l, line)))
        .filter(|(_, _, line)| line.kind == LineKind::Added)
        .map(|(h, l, _)| (h, l))
        .nth(n)
        .unwrap_or_else(|| panic!("no added line {n} in {}", d.path()))
}

fn line_sel((h, l): (usize, usize)) -> Selection {
    Selection::lines(h, [l])
}

/// The display text of the `n`-th added line.
fn added_text(d: &FileDiff, n: usize) -> &str {
    let (h, l) = added(d, n);
    &d.hunks[h].lines[l].text
}

/// Latin-1, Windows-1252 and a mixed file (UTF-8 lines and one that is not): the lines
/// are shown decoded, named by their encoding; staging one line, discarding another and
/// unstaging the staged one give exactly the expected bytes.
/// Workflow: EX-09, NU-01
#[gitti_test::test]
fn eight_bit_lines_stage_unstage_discard_exactly() {
    struct Case {
        name: &'static str,
        base: &'static [u8],
        new: &'static [u8],
        encoding: TextEncoding,
        /// The shown text of the two added lines.
        shown: [&'static str; 2],
        /// The index after staging the first added line = the worktree after discarding
        /// the second one.
        one: &'static [u8],
    }
    let cases = [
        Case {
            name: "l.txt",
            base: b"a\ncaf\xe9\nz\n",
            new: b"a\ncaf\xe9\n\xe4\xf6\nna\xefve\nz\n",
            encoding: TextEncoding::Latin1,
            shown: ["äö", "naïve"],
            one: b"a\ncaf\xe9\n\xe4\xf6\nz\n",
        },
        Case {
            name: "w.txt",
            base: b"a\n\x80 1\nz\n",
            new: b"a\n\x80 1\n\x93q\x94\n\x96 x\nz\n",
            encoding: TextEncoding::Windows1252,
            shown: ["“q”", "– x"],
            one: b"a\n\x80 1\n\x93q\x94\nz\n",
        },
        Case {
            name: "m.txt",
            base: "a\nä\nz\n".as_bytes(),
            new: b"a\n\xc3\xa4\n\xe4 latin\n\xc3\xbc utf8\nz\n",
            encoding: TextEncoding::Latin1,
            shown: ["ä latin", "ü utf8"],
            one: b"a\n\xc3\xa4\n\xe4 latin\nz\n",
        },
    ];
    let mut h = History::new();
    let files: Vec<(&str, E)> = cases.iter().map(|c| (c.name, E::File(c.base))).collect();
    h.commit("main", &[], "init", &files);
    let (_tmp, p, _) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    for c in &cases {
        fs::write(p.join(c.name), c.new).unwrap();
    }

    let ws = r.working_state().unwrap();
    for c in &cases {
        let d = find(&ws.unstaged, c.name);
        assert_eq!((d.encoding, d.lossy_name), (c.encoding, false), "{}", c.name);
        assert_eq!([added_text(d, 0), added_text(d, 1)], c.shown, "{}", c.name);
        r.apply_selection(d, &line_sel(added(d, 0)), PatchMode::Stage).unwrap();
        assert_eq!(index_bytes(&p, c.name), c.one, "{} staged", c.name);
        // The worktree's diff against the old index still applies to the worktree.
        r.apply_selection(d, &line_sel(added(d, 1)), PatchMode::Discard).unwrap();
        assert_eq!(fs::read(p.join(c.name)).unwrap(), c.one, "{} discarded", c.name);
    }

    let ws = r.working_state().unwrap();
    for c in &cases {
        let d = find(&ws.staged, c.name);
        assert_eq!(d.encoding, c.encoding, "{} staged", c.name);
        r.apply_selection(d, &line_sel(added(d, 0)), PatchMode::Unstage).unwrap();
        assert_eq!(index_bytes(&p, c.name), c.base, "{} unstaged", c.name);
    }
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// UTF-16 LE with a byte order mark, shown as text by a `diff` attribute: git splits its
/// lines at the byte 0x0A (the newline's NUL starts the next line). Lines are shown
/// decoded; staging, discarding and unstaging a line move the exact bytes.
/// Workflow: EX-10, NU-02
#[gitti_test::test]
fn utf16_lines_stage_unstage_discard_exactly() {
    let base = utf16le("\u{FEFF}a\nb\nz\n");
    let new = utf16le("\u{FEFF}a\nb\nä x\n中 y\nz\n");
    let mut h = History::new();
    let attrs = f("*.txt diff\n");
    h.commit("main", &[], "init", &[(".gitattributes", attrs), ("u.txt", E::File(&base))]);
    let (_tmp, p, _) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    fs::write(p.join("u.txt"), &new).unwrap();

    let ws = r.working_state().unwrap();
    let d = find(&ws.unstaged, "u.txt");
    assert_eq!(d.encoding, TextEncoding::Utf16Le);
    assert_eq!([added_text(d, 0), added_text(d, 1)], ["ä x", "中 y"]);
    r.apply_selection(d, &line_sel(added(d, 0)), PatchMode::Stage).unwrap();
    let one = utf16le("\u{FEFF}a\nb\nä x\nz\n");
    assert_eq!(index_bytes(&p, "u.txt"), one);
    r.apply_selection(d, &line_sel(added(d, 1)), PatchMode::Discard).unwrap();
    assert_eq!(fs::read(p.join("u.txt")).unwrap(), one);

    let ws = r.working_state().unwrap();
    let d = find(&ws.staged, "u.txt");
    assert_eq!(d.encoding, TextEncoding::Utf16Le);
    r.apply_selection(d, &line_sel(added(d, 0)), PatchMode::Unstage).unwrap();
    assert_eq!(index_bytes(&p, "u.txt"), base);
}

/// `working-tree-encoding`: git converts the file to UTF-8 itself, so its diff is UTF-8
/// and a discarded line leaves the file in its encoding; an untracked such file is diffed
/// by git (asked via `check-attr`), so a staged line reaches the index as UTF-8, like
/// `git add`. An untracked Latin-1 file without attributes is built in-process and stages
/// its exact bytes.
/// Workflow: NU-03
#[gitti_test::test]
fn working_tree_encoding_and_untracked_files() {
    let mut h = History::new();
    let attrs = f("*.txt working-tree-encoding=UTF-16LE-BOM\n");
    h.commit("main", &[], "init", &[(".gitattributes", attrs), ("t.txt", f("a\nz\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    // `reset --hard` wrote the file in its working-tree encoding.
    assert_eq!(fs::read(p.join("t.txt")).unwrap(), utf16le("\u{FEFF}a\nz\n"));
    fs::write(p.join("t.txt"), utf16le("\u{FEFF}a\nä\nz\n")).unwrap();
    fs::write(p.join("n.txt"), utf16le("\u{FEFF}x\nö\n")).unwrap();
    fs::write(p.join("n.lat"), b"x\n\xf6\n").unwrap();

    let ws = r.working_state().unwrap();
    let t = find(&ws.unstaged, "t.txt");
    assert_eq!((t.encoding, added_text(t, 0)), (TextEncoding::Utf8, "ä"));
    let n = find(&ws.unstaged, "n.txt");
    assert_eq!((n.encoding, added_text(n, 1)), (TextEncoding::Utf8, "ö"));
    let lat = find(&ws.unstaged, "n.lat");
    assert_eq!((lat.encoding, added_text(lat, 1)), (TextEncoding::Latin1, "ö"));

    r.apply_selection(t, &line_sel(added(t, 0)), PatchMode::Discard).unwrap();
    assert_eq!(fs::read(p.join("t.txt")).unwrap(), utf16le("\u{FEFF}a\nz\n"));
    r.apply_selection(n, &line_sel(added(n, 1)), PatchMode::Stage).unwrap();
    assert_eq!(index_bytes(&p, "n.txt"), "ö\n".as_bytes());
    r.apply_selection(lat, &line_sel(added(lat, 1)), PatchMode::Stage).unwrap();
    assert_eq!(index_bytes(&p, "n.lat"), b"\xf6\n");
}

/// Ignore whitespace: the shown diff is `-w`, staging maps the selection onto the exact
/// diff (`map_selection`) — with lines that are not UTF-8 too.
/// Workflow: ST-10
#[gitti_test::test]
fn ignore_whitespace_maps_lines_that_are_not_utf8() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("l.txt", E::File(b"a\n\xe9 x\nz\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    fs::write(p.join("l.txt"), b"a\n\xe9  x\n\xe4\nz\n").unwrap();
    let opts = gitti_core::DiffOptions { ignore_whitespace: true, ..Default::default() };
    let ws = r.working_state_with(&opts).unwrap();
    let w = ws.unstaged.iter().find(|w| w.path() == "l.txt").unwrap();
    let (shown, real) = (w.diff.as_ref().unwrap(), w.patch_diff.as_ref().unwrap());
    // `-w` hides the whitespace change: one added line.
    assert_eq!(shown.added_removed(), (1, 0));
    assert_eq!(added_text(shown, 0), "ä");
    let sel = gitti_core::patch::map_selection(shown, &line_sel(added(shown, 0)), real).unwrap();
    r.apply_selection(real, &sel, PatchMode::Stage).unwrap();
    assert_eq!(index_bytes(&p, "l.txt"), b"a\n\xe9 x\n\xe4\nz\n");
}

/// A file *name* that is not UTF-8 whose bytes gitti doesn't know (no `git status` listed
/// it, `gitti_core::raw_names`) is refused, before any git runs: the patch's header would
/// name another file. Known ones go back to their bytes (`wf_shapes::nu04_*`).
/// Workflow: NU-04
#[gitti_test::test]
fn names_that_are_not_utf8_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("repo");
    crate::fixture_kit::init(&p);
    let r = Repo::open(&p).unwrap();
    let raw = b"diff --git a/caf\xe9 b/caf\xe9\nindex 1..2 100644\n--- a/caf\xe9\n+++ b/caf\xe9\n@@ -1 +1 @@\n-a\n+b\n";
    let d = &gitti_core::diff::parse_unified_bytes(raw)[0];
    assert!(d.lossy_name);
    let before = r.command_log().snapshot().len();
    for mode in [PatchMode::Stage, PatchMode::Unstage, PatchMode::Discard] {
        let err = r.apply_selection(d, &Selection::whole_hunk(0), mode).unwrap_err();
        let typed = err.downcast_ref::<NotUtf8>().expect("a NotUtf8 error");
        assert_eq!((typed.path.as_str(), typed.mode), (d.path(), mode));
        assert!(err.to_string().contains("is not UTF-8"), "{err}");
    }
    assert_eq!(r.command_log().snapshot().len(), before);
}

/// A repository with `history` imported and checked out under `config` (appended before
/// the checkout, so `core.autocrlf` / attributes shape the working tree): three git
/// processes, like `repo_with`.
fn repo_with_config(history: &History, config: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = gitti_core::paths::canonical(tmp.path()).join("repo");
    crate::fixture_kit::init(&dir);
    crate::fixture_kit::append_config(&dir, config);
    history.import(&dir);
    crate::fixture_kit::sh(&dir, &["reset", "-q", "--hard"]);
    (tmp, dir)
}

/// A file of [`conflicted`]: name, base, ours, theirs.
type Versions<'a> = (&'a str, &'a [u8], &'a [u8], &'a [u8]);

/// `base` → `ours` on main, `base` → `theirs` on side (each file of `files`), merged into
/// main with a conflict.
fn conflicted(
    files: &[Versions],
    config: &str,
) -> (tempfile::TempDir, std::path::PathBuf, std::sync::Arc<Repo>) {
    let mut h = History::new();
    let pick = |i: usize| -> Vec<(&str, E)> {
        files.iter().map(|f| (f.0, E::File([f.1, f.2, f.3][i]))).collect()
    };
    let root = h.commit("main", &[], "init", &pick(0));
    h.commit("side", &[root], "side", &pick(2));
    h.commit("main", &[root], "main", &pick(1));
    let (tmp, p) = repo_with_config(&h, config);
    let r = std::sync::Arc::new(Repo::open(&p).unwrap());
    assert!(r.merge("side").unwrap().starts_with("Merge stopped on "), "a conflict: a stop, not a failure");
    (tmp, p, r)
}

fn open_merge(r: &std::sync::Arc<Repo>, name: &str) -> gitti_core::merge_session::MergeSession {
    let sides = gitti_core::ops::ConflictSides { ours: "main".into(), theirs: "side".into() };
    gitti_core::merge_session::MergeSession::open(r.clone(), name, sides).unwrap()
}

/// The merge editor holds a file that is not UTF-8 in one exact encoding and writes it
/// back byte for byte: a Latin-1 conflict (git's markers on disk count as unedited), and a
/// mixed file (UTF-8 and Latin-1 lines, Shift-JIS bytes) whose untouched lines keep their
/// bytes whichever side is taken. `git add` then stores exactly those bytes.
/// Workflow: MG-15
#[gitti_test::test]
fn merge_editor_writes_non_utf8_files_exactly() {
    let sjis: &[u8] = b"\x83\x5c\x83\x74\x83\x67"; // "ソフト": 0x5C (`\`) as a second byte
    let mixed_base = [&b"\xc3\xa4 utf8\n"[..], b"\xe4 latin\n", sjis, b"\nx\n"].concat();
    let mixed_ours = [&b"\xc3\xa4 utf8\n"[..], b"\xe4 latin\n", sjis, b"\nours \xe9\n"].concat();
    let mixed_theirs = [&b"\xc3\xa4 utf8\n"[..], b"\xe4 latin\n", sjis, b"\ntheirs \x80\n"].concat();
    let (_tmp, p, r) = conflicted(
        &[
            ("l.txt", b"a\nb\nc\n", b"a\n\xf6main\nc\n", b"a\n\xe4side\nc\n"),
            ("m.txt", &mixed_base, &mixed_ours, &mixed_theirs),
        ],
        "",
    );

    let mut s = open_merge(&r, "l.txt");
    assert_eq!(s.doc.encoding, TextEncoding::Latin1);
    assert!(!s.is_modified(), "git's conflict file is the unedited start");
    assert!(s.text().contains("\nömain\n=======\näside\n"), "{}", s.text());
    let conflict = s.doc.conflicts().next().unwrap();
    s.choose(conflict, Some(gitti_core::merge::Resolution::TheirsThenOurs));
    s.write().unwrap();
    assert_eq!(fs::read(p.join("l.txt")).unwrap(), b"a\n\xe4side\n\xf6main\nc\n");

    let mut s = open_merge(&r, "m.txt");
    assert_eq!(s.doc.encoding, TextEncoding::Windows1252);
    let conflict = s.doc.conflicts().next().unwrap();
    s.choose(conflict, Some(gitti_core::merge::Resolution::Theirs));
    s.write().unwrap();
    assert_eq!(fs::read(p.join("m.txt")).unwrap(), mixed_theirs);
    // An unresolved conflict is written with markers around the exact bytes.
    s.choose(conflict, None);
    s.write().unwrap();
    let ours_then = [&mixed_base[..mixed_base.len() - 2], b"<<<<<<< main\nours \xe9\n=======\ntheirs \x80\n>>>>>>> side\n"].concat();
    assert_eq!(fs::read(p.join("m.txt")).unwrap(), ours_then);

    s.choose(conflict, Some(gitti_core::merge::Resolution::Ours));
    s.write().unwrap();
    r.mark_resolved(&["l.txt".into(), "m.txt".into()]).unwrap();
    assert_eq!(index_bytes(&p, "l.txt"), b"a\n\xe4side\n\xf6main\nc\n");
    assert_eq!(index_bytes(&p, "m.txt"), mixed_ours);
}

/// Files git converts on checkout: the merge editor works on the stages as the working tree
/// has them (`cat-file --filters`) and writes that form — CRLF with `core.autocrlf`, the
/// `working-tree-encoding` (UTF-16 here, Latin-1 bytes in UTF-8 stages for ISO-8859-1) —
/// so `git add` stores exactly what a resolution in the index would be.
/// Workflow: MG-15
#[gitti_test::test]
fn merge_editor_writes_the_working_tree_form() {
    let attrs: &[u8] = b"u.txt working-tree-encoding=UTF-16LE-BOM\nl.txt working-tree-encoding=ISO-8859-1\n";
    let (_tmp, p, r) = conflicted(
        &[
            (".gitattributes", attrs, attrs, attrs),
            ("c.txt", b"a\nb\nc\n", b"a\nours\nc\n", b"a\ntheirs\nc\n"),
            ("u.txt", "a\nb\n".as_bytes(), "a\nöurs\n".as_bytes(), "a\n中\n".as_bytes()),
            ("l.txt", "a\nb\n".as_bytes(), "a\nöurs\n".as_bytes(), "a\nthéirs\n".as_bytes()),
        ],
        "[core]\n\tautocrlf = true\n",
    );
    let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };

    let mut s = open_merge(&r, "c.txt");
    assert_eq!(s.doc.encoding, TextEncoding::Utf8);
    assert!(!s.text().contains('\r') && !s.is_modified(), "{:?}", s.text());
    let c = s.doc.conflicts().next().unwrap();
    s.choose(c, Some(gitti_core::merge::Resolution::Theirs));
    s.write().unwrap();
    assert_eq!(fs::read(p.join("c.txt")).unwrap(), b"a\r\ntheirs\r\nc\r\n");

    let mut s = open_merge(&r, "u.txt");
    assert_eq!(s.doc.encoding, TextEncoding::Utf16Le);
    assert!(s.text().contains("\nöurs\n=======\n中\n"), "{:?}", s.text());
    let c = s.doc.conflicts().next().unwrap();
    s.choose(c, Some(gitti_core::merge::Resolution::OursThenTheirs));
    s.write().unwrap();
    assert_eq!(fs::read(p.join("u.txt")).unwrap(), utf16("\u{FEFF}a\r\nöurs\r\n中\r\n"));

    let mut s = open_merge(&r, "l.txt");
    assert_eq!(s.doc.encoding, TextEncoding::Latin1);
    let c = s.doc.conflicts().next().unwrap();
    s.choose(c, Some(gitti_core::merge::Resolution::Theirs));
    s.write().unwrap();
    assert_eq!(fs::read(p.join("l.txt")).unwrap(), b"a\r\nth\xe9irs\r\n");
    // A character ISO-8859-1 has no byte for is refused, nothing written.
    s.set_text("a\n中\n".into());
    let err = s.write().unwrap_err().to_string();
    assert!(err.contains("'中' (line 2) can't be written in Latin-1"), "{err}");
    assert_eq!(fs::read(p.join("l.txt")).unwrap(), b"a\r\nth\xe9irs\r\n");

    r.mark_resolved(&["c.txt".into(), "u.txt".into(), "l.txt".into()]).unwrap();
    assert_eq!(index_bytes(&p, "c.txt"), b"a\ntheirs\nc\n");
    assert_eq!(index_bytes(&p, "u.txt"), "a\nöurs\n中\n".as_bytes());
    assert_eq!(index_bytes(&p, "l.txt"), "a\nthéirs\n".as_bytes());
}

// ------------------------------------------------------------------ random byte lines

/// xorshift64*: small, deterministic per seed (as in the release suite's `staging_fuzz`).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// Byte pieces of the random lines: ASCII, Latin-1, UTF-8, Windows-1252, broken UTF-8.
const TOKENS: [&[u8]; 10] = [
    b"a", b"b", b" ", b"\xe4", b"\xc3\xa4", b"\x80", b"\x93\x94", b"\xff", b"\xc3", b"\xe2\x82",
];

/// A pool of a few random byte lines (sometimes with a CR): files drawn from it have few
/// distinct lines, so diffs align in interesting ways.
fn random_pool(rng: &mut Rng) -> Vec<Vec<u8>> {
    (0..8)
        .map(|_| {
            let n = 1 + rng.below(3);
            let mut l: Vec<u8> = (0..n).flat_map(|_| TOKENS[rng.below(TOKENS.len())].to_vec()).collect();
            if rng.chance(10) {
                l.push(b'\r');
            }
            l
        })
        .collect()
}

/// `lines` joined with `\n`, the last one without a newline sometimes.
fn join(rng: &mut Rng, lines: &[Vec<u8>]) -> Vec<u8> {
    let mut out = lines.join(&b'\n');
    if !lines.is_empty() && !rng.chance(15) {
        out.push(b'\n');
    }
    out
}

/// A random selection of change lines (some per hunk, at least one).
fn random_selection(rng: &mut Rng, d: &FileDiff) -> Selection {
    let mut sel = Selection::default();
    for (h, hunk) in d.hunks.iter().enumerate() {
        let lines = hunk.lines.iter().enumerate().filter(|(_, l)| l.is_change() && rng.chance(50));
        let lines: Vec<usize> = lines.map(|(i, _)| i).collect();
        if !lines.is_empty() {
            sel.add_lines(h, lines);
        }
    }
    if sel.hunks.is_empty() {
        let l = d.hunks[0].lines.iter().position(|l| l.is_change()).unwrap();
        sel.add_lines(0, [l]);
    }
    sel
}

/// The bytes after applying `sel` of `d` (old side `old` → new side `new`) as `mode`
/// does: stage keeps the old side and takes the selected changes; unstage / discard keep
/// the new side and drop them. Lines outside the hunks come from the side kept.
fn model(old: &[u8], new: &[u8], d: &FileDiff, sel: &Selection, mode: PatchMode) -> Vec<u8> {
    let forward = mode == PatchMode::Stage;
    let side = if forward { old } else { new };
    let raw: Vec<(Vec<u8>, bool)> = side
        .split_inclusive(|b| *b == b'\n')
        .map(|l| (l.strip_suffix(b"\n").unwrap_or(l).to_vec(), l.ends_with(b"\n")))
        .collect();
    let picked = |h: usize, l: usize| match sel.hunks.get(&h) {
        None => false,
        Some(None) => true,
        Some(Some(set)) => set.contains(&l),
    };
    let mut out: Vec<(Vec<u8>, bool)> = Vec::new();
    let mut next = 0usize;
    for (h, hunk) in d.hunks.iter().enumerate() {
        let (start, count) = if forward {
            (hunk.old_start, hunk.old_count)
        } else {
            (hunk.new_start, hunk.new_count)
        };
        let first = if count == 0 { start } else { start.saturating_sub(1) } as usize;
        let first = first.clamp(next, raw.len());
        out.extend(raw[next..first].iter().cloned());
        let mut used = 0usize;
        for (i, l) in hunk.lines.iter().enumerate() {
            let eol = hunk.lines.get(i + 1).is_none_or(|n| n.kind != LineKind::NoNewline);
            let line = (l.bytes().to_vec(), eol);
            let picked = picked(h, i);
            match l.kind {
                LineKind::NoNewline => {}
                LineKind::Context => {
                    out.push(line);
                    used += 1;
                }
                LineKind::Removed if forward => {
                    used += 1;
                    if !picked {
                        out.push(line);
                    }
                }
                LineKind::Added if !forward => {
                    used += 1;
                    if !picked {
                        out.push(line);
                    }
                }
                LineKind::Removed | LineKind::Added => {
                    if picked {
                        out.push(line);
                    }
                }
            }
        }
        next = first + used;
    }
    out.extend(raw[next.min(raw.len())..].iter().cloned());
    // Only the last line can lack its newline: one that ends up in the middle gets one.
    let n = out.len();
    let mut bytes = Vec::new();
    for (i, (text, eol)) in out.into_iter().enumerate() {
        bytes.extend_from_slice(&text);
        if eol || i + 1 < n {
            bytes.push(b'\n');
        }
    }
    bytes
}

/// Random byte lines, fixed seeds: staging / unstaging / discarding random line
/// selections gives exactly the model's bytes — a fast, deterministic cousin of the
/// release suite's `staging_fuzz`, over bytes instead of text.
/// Workflow: ST-20
#[gitti_test::test]
fn random_byte_lines_stage_exactly() {
    let tmp = tempfile::tempdir().unwrap();
    let p = gitti_core::paths::canonical(tmp.path()).join("repo");
    crate::fixture_kit::init(&p);
    let r = Repo::open(&p).unwrap();
    let file = p.join("f.txt");
    let modes = [PatchMode::Stage, PatchMode::Unstage, PatchMode::Discard];
    let mut checked = 0;
    // Two seeds × three modes: ~5 git processes a round (~35 ms each on Windows).
    for (round, seed) in [0x9E37_79B9_7F4A_7C15_u64, 0xDEAD_BEEF_CAFE_F00D]
        .into_iter()
        .enumerate()
        .flat_map(|(i, s)| (0..3).map(move |k| (i * 3 + k, s.rotate_left(k as u32 * 7))))
    {
        let mode = modes[round % 3];
        let mut rng = Rng(seed | 1);
        let pool = random_pool(&mut rng);
        let pick = |rng: &mut Rng| pool[rng.below(pool.len())].clone();
        let base_lines: Vec<Vec<u8>> = (0..1 + rng.below(20)).map(|_| pick(&mut rng)).collect();
        let mut new_lines = base_lines.clone();
        for _ in 0..1 + rng.below(6) {
            let at = rng.below(new_lines.len() + 1);
            match rng.below(3) {
                0 if at < new_lines.len() => {
                    new_lines.remove(at);
                }
                1 if at < new_lines.len() => new_lines[at] = pick(&mut rng),
                _ => new_lines.insert(at, pick(&mut rng)),
            }
        }
        let base = join(&mut rng, &base_lines);
        let new = join(&mut rng, &new_lines);
        if base == new {
            continue;
        }
        // index = base, worktree = new: the unstaged diff. For unstage the index becomes
        // `new` too (`apply --cached -R` looks at the index content alone).
        fs::write(&file, &base).unwrap();
        crate::fixture_kit::sh(&p, &["add", "f.txt"]);
        fs::write(&file, &new).unwrap();
        let ws = r.working_state().unwrap();
        let d = find(&ws.unstaged, "f.txt").clone();
        let sel = random_selection(&mut rng, &d);
        if mode == PatchMode::Unstage {
            crate::fixture_kit::sh(&p, &["add", "f.txt"]);
        }
        let context = || format!("round {round} {mode:?} {sel:?}\nbase {base:?}\nnew  {new:?}");
        r.apply_selection(&d, &sel, mode)
            .unwrap_or_else(|e| panic!("{}: {e:#}", context()));
        let got = match mode {
            PatchMode::Discard => fs::read(&file).unwrap(),
            _ => index_bytes(&p, "f.txt"),
        };
        assert_eq!(got, model(&base, &new, &d, &sel, mode), "{}", context());
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} rounds checked");
}
