//! Building partial patches for hunk / line (un)staging and discarding.
//!
//! The patch is always derived from a diff that git produced, filtered to the selected change
//! lines, and then fed to `git apply` (see [`crate::Repo::apply_patch`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::diff::{DiffLine, FileChange, FileDiff, LineKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchMode {
    /// Unstaged diff → index (`git apply --cached`).
    Stage,
    /// Staged diff → index, reversed (`git apply --cached -R`).
    Unstage,
    /// Unstaged diff → worktree, reversed (`git apply -R`).
    Discard,
}

impl PatchMode {
    pub fn is_reverse(self) -> bool {
        !matches!(self, PatchMode::Stage)
    }
}

/// A hunk / line (un)stage or discard refused because the file's name is not valid UTF-8
/// and its bytes are not known for sure ([`crate::raw_names`]: two names that show alike,
/// or one no `git status` listed): the patch would name another file. Names that are not
/// UTF-8 are otherwise put back into their bytes ([`crate::Repo::raw_patch_names`]); file
/// content in any encoding is fine — patches carry the lines' exact bytes
/// ([`crate::diff::DiffLine::bytes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotUtf8 {
    pub path: String,
    pub mode: PatchMode,
}

impl std::fmt::Display for NotUtf8 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let verb = match self.mode {
            PatchMode::Stage => "stage",
            PatchMode::Unstage => "unstage",
            PatchMode::Discard => "discard",
        };
        write!(
            f,
            "The name of {} is not UTF-8 and gitti can't tell for sure which file it is \
             (another file's name shows the same) — cannot {verb} its hunks or lines",
            self.path
        )
    }
}

impl std::error::Error for NotUtf8 {}

/// The short note frontends show where hunk / line actions of a file are refused with
/// [`NotUtf8`] ([`crate::Repo::check_names`]).
pub const NOT_UTF8_NOTE: &str = "file name not UTF-8 and ambiguous: no hunk / line staging";

/// `patch` with each shown name of `names` (with U+FFFD) in its header lines — before
/// the first hunk — replaced by its bytes, also inside a C-quoted name.
pub fn with_raw_names(patch: &[u8], names: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut names: Vec<&(&str, Vec<u8>)> = names.iter().collect();
    // The longer name first: one may be part of another.
    names.sort_by_key(|(shown, _)| std::cmp::Reverse(shown.len()));
    let mut out = Vec::with_capacity(patch.len());
    let mut rest = patch;
    while !rest.is_empty() && !rest.starts_with(b"@@") {
        let end = rest.iter().position(|b| *b == b'\n').map_or(rest.len(), |i| i + 1);
        let mut line = rest[..end].to_vec();
        for (shown, raw) in &names {
            line = replace(&line, shown.as_bytes(), raw);
            let (q_shown, q_raw) = (c_escape(shown.as_bytes()), c_escape(raw));
            if q_shown != shown.as_bytes() {
                line = replace(&line, &q_shown, &q_raw);
            }
        }
        out.extend_from_slice(&line);
        rest = &rest[end..];
    }
    out.extend_from_slice(rest);
    out
}

/// `hay` with every `from` replaced by `to`.
fn replace(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

/// `bytes` escaped the way git writes them inside a C-quoted name (see [`quote_path`]):
/// bytes of 0x80 and up stay as they are (`core.quotepath=false`).
fn c_escape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x07 => out.extend_from_slice(b"\\a"),
            0x08 => out.extend_from_slice(b"\\b"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            0x0b => out.extend_from_slice(b"\\v"),
            0x0c => out.extend_from_slice(b"\\f"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b if b < 0x20 || b == 0x7f => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            b => out.push(b),
        }
    }
    out
}

/// Which lines of a file diff are selected.
///
/// Key = hunk index. `None` = the whole hunk, `Some(set)` = those line indices of the hunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub hunks: BTreeMap<usize, Option<BTreeSet<usize>>>,
}

impl Selection {
    pub fn whole_hunk(hunk: usize) -> Self {
        let mut s = Self::default();
        s.hunks.insert(hunk, None);
        s
    }

    pub fn lines(hunk: usize, lines: impl IntoIterator<Item = usize>) -> Self {
        let mut s = Self::default();
        s.add_lines(hunk, lines);
        s
    }

    pub fn add_lines(&mut self, hunk: usize, lines: impl IntoIterator<Item = usize>) {
        let entry = self
            .hunks
            .entry(hunk)
            .or_insert_with(|| Some(BTreeSet::new()));
        if let Some(set) = entry {
            set.extend(lines);
        }
    }

    fn is_selected(&self, hunk: usize, line: usize) -> bool {
        match self.hunks.get(&hunk) {
            None => false,
            Some(None) => true,
            Some(Some(set)) => set.contains(&line),
        }
    }
}

/// Transfers a selection made on `shown` (e.g. a `git diff -w` view that hides whitespace
/// changes) onto `real`, the exact diff patches are built from. Change lines are matched by
/// their position in the file (old line number for removals, new for additions), which both
/// diffs report identically. `None` if a selected line has no counterpart or nothing is left.
pub fn map_selection(shown: &FileDiff, sel: &Selection, real: &FileDiff) -> Option<Selection> {
    use std::collections::HashMap;
    let key = |kind: LineKind, old: Option<u32>, new: Option<u32>| match kind {
        LineKind::Removed => old.map(|n| (false, n)),
        LineKind::Added => new.map(|n| (true, n)),
        _ => None,
    };
    let mut index: HashMap<(bool, u32), (usize, usize)> = HashMap::new();
    for (h, hunk) in real.hunks.iter().enumerate() {
        for (l, line) in hunk.lines.iter().enumerate() {
            if let Some(k) = key(line.kind, line.old_no, line.new_no) {
                index.insert(k, (h, l));
            }
        }
    }
    let mut out = Selection::default();
    for (&h, lines) in &sel.hunks {
        let hunk = shown.hunks.get(h)?;
        for (l, line) in hunk.lines.iter().enumerate() {
            let selected = lines.as_ref().is_none_or(|set| set.contains(&l));
            if !selected || !line.is_change() {
                continue;
            }
            let (rh, rl) = *index.get(&key(line.kind, line.old_no, line.new_no)?)?;
            out.add_lines(rh, [rl]);
        }
    }
    (!out.hunks.is_empty()).then_some(out)
}

/// One line of a hunk being built: `' '`, `'-'` or `'+'`, its exact bytes, and the `\ No
/// newline at end of file` marker when it is the last line of its side(s) without a newline.
/// `cr`: the line had no newline and gets one, in a CRLF file — a `\r` goes before it.
struct Out<'a> {
    kind: char,
    text: &'a [u8],
    no_eol: Option<&'a [u8]>,
    cr: bool,
}

impl<'a> Out<'a> {
    fn new(kind: char, text: &'a [u8], no_eol: Option<&'a [u8]>) -> Self {
        Out { kind, text, no_eol, cr: false }
    }
}

/// Fixes up the "no newline at end of file" markers of a partial hunk. A marker says
/// "this is the last line of its side", which no longer holds when a line kept as context
/// (or a line of the produced side) is followed by more lines of that side — git then
/// glued the next line onto it (`a` + `b` → `ab`). A context line that is last on one
/// side only is split into `-line` / `+line` with the marker on the side that ends there;
/// a change line that is no longer last loses its marker (its side gets the newline).
///
/// Such a line only ever gains its newline on the side the patch produces (the side it is
/// applied to keeps its last line as it is), and it gets the line ending of its
/// neighbours ([`gets_crlf`]): in a CRLF file `\r\n`, not a bare `\n`.
fn fix_no_eol(lines: Vec<Out<'_>>) -> Vec<Out<'_>> {
    let n = lines.len();
    // Whether lines after index i exist on the old / new side.
    let mut old_after = vec![false; n + 1];
    let mut new_after = vec![false; n + 1];
    for i in (0..n).rev() {
        old_after[i] = old_after[i + 1] || lines[i].kind != '+';
        new_after[i] = new_after[i + 1] || lines[i].kind != '-';
    }
    // Per marked line, whether it gets CRLF on the old / new side should it gain a newline.
    let crlf: Vec<(bool, bool)> = (0..n)
        .map(|i| match lines[i].no_eol {
            Some(_) => (gets_crlf(&lines, i, true), gets_crlf(&lines, i, false)),
            None => (false, false),
        })
        .collect();
    let mut out: Vec<Out<'_>> = Vec::with_capacity(n + 1);
    for (i, l) in lines.into_iter().enumerate() {
        let Some(marker) = l.no_eol else {
            out.push(l);
            continue;
        };
        let (old_more, new_more) = (old_after[i + 1], new_after[i + 1]);
        let (old_cr, new_cr) = crlf[i];
        let cr = |side_cr: bool| side_cr && !l.text.ends_with(b"\r");
        match l.kind {
            ' ' if new_more && !old_more => {
                out.push(Out::new('-', l.text, Some(marker)));
                out.push(Out { cr: cr(new_cr), ..Out::new('+', l.text, None) });
            }
            ' ' if old_more && !new_more => {
                out.push(Out { cr: cr(old_cr), ..Out::new('-', l.text, None) });
                out.push(Out::new('+', l.text, Some(marker)));
            }
            // Last on neither side any more: can't happen (the side the patch applies to
            // ends with it); kept byte for byte so `git apply` refuses rather than guesses.
            ' ' if old_more => out.push(Out { no_eol: None, ..l }),
            '-' if old_more => out.push(Out { no_eol: None, cr: cr(old_cr), ..l }),
            '+' if new_more => out.push(Out { no_eol: None, cr: cr(new_cr), ..l }),
            _ => out.push(l),
        }
    }
    out
}

/// Whether line `i` of a hunk being built, without a newline so far, gets `\r\n` when it
/// gains one on the old (`old_side`) or new side: the line ending of the nearest line of
/// that side that has a newline — before it, else after it, else any line of the hunk.
/// Without any, LF.
fn gets_crlf(lines: &[Out<'_>], i: usize, old_side: bool) -> bool {
    let on_side = |l: &&Out<'_>| l.no_eol.is_none() && if old_side { l.kind != '+' } else { l.kind != '-' };
    lines[..i]
        .iter()
        .rev()
        .find(on_side)
        .or_else(|| lines[i + 1..].iter().find(on_side))
        .or_else(|| lines.iter().find(|l| l.no_eol.is_none()))
        .is_some_and(|l| l.text.ends_with(b"\r"))
}

/// The order in which a patch carries the lines of a hunk (indices into `lines`, without
/// the `\ No newline` markers, which stay with the line before them).
///
/// git writes a replaced block as all its `-` lines, then all its `+` lines. A partial
/// patch that kept that order would move a line: staging the `-` / `+` pair of the 2nd of
/// six replaced lines (every line of a file whose line endings changed) keeps the other
/// five `-` lines as context and then adds the new line after them — at the block's end.
/// So each block is reordered into pairs, every `-` line followed by the `+` line that
/// replaces it, and a selected line (or a kept one) sits where its partner is:
/// - the pairs are the ones the diff view marks ([`crate::intraline::pair_lines`]: a
///   monotone alignment of similar lines, in order for big blocks), so a new line above a
///   changed one stays above it (`-x +new +X` → `+new -x +X`);
/// - the lines left between two such pairs (or before the first / after the last) are
///   paired in order — the 1st `-` with the 1st `+`, … — and the extra lines of the longer
///   side follow (`-a -b -c +X` → `-a +X -b -c`).
///
/// Each side keeps its own order, so the patch applies to exactly what it did before; only
/// where the produced side's lines land among the kept ones changes. The same order for
/// every mode and selection: unstage / discard (reverse) restore a line next to its
/// partner too.
pub fn line_order(lines: &[DiffLine]) -> Vec<usize> {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            LineKind::Context | LineKind::Added => {
                out.push(i);
                i += 1;
                continue;
            }
            LineKind::NoNewline => {
                i += 1;
                continue;
            }
            LineKind::Removed => {}
        }
        let mut removed = Vec::new();
        while i < lines.len() && matches!(lines[i].kind, LineKind::Removed | LineKind::NoNewline) {
            if lines[i].kind == LineKind::Removed {
                removed.push(i);
            }
            i += 1;
        }
        let mut added = Vec::new();
        while i < lines.len() && matches!(lines[i].kind, LineKind::Added | LineKind::NoNewline) {
            if lines[i].kind == LineKind::Added {
                added.push(i);
            }
            i += 1;
        }
        if added.is_empty() || removed.len() + added.len() == 2 {
            out.extend(removed);
            out.extend(added);
            continue;
        }
        let pairs = crate::intraline::pair_lines(lines, &removed, &added);
        // Positions in `removed` / `added`; the alignment is monotone.
        let (mut r, mut a) = (0, 0);
        let gap = |out: &mut Vec<usize>, rs: &[usize], ads: &[usize]| {
            for k in 0..rs.len().max(ads.len()) {
                out.extend(rs.get(k));
                out.extend(ads.get(k));
            }
        };
        for (pr, pa) in pairs {
            let rp = r + removed[r..].iter().position(|&x| x == pr).expect("paired removed line");
            let ap = a + added[a..].iter().position(|&x| x == pa).expect("paired added line");
            gap(&mut out, &removed[r..rp], &added[a..ap]);
            out.push(pr);
            out.push(pa);
            (r, a) = (rp + 1, ap + 1);
        }
        gap(&mut out, &removed[r..], &added[a..]);
    }
    out
}

/// A symlink's diff (mode 120000): its one "line" is the link target.
fn is_symlink(file: &FileDiff) -> bool {
    file.header
        .iter()
        .any(|h| h.ends_with(" 120000") && (h.starts_with("index ") || h.contains(" mode ")))
}

/// For a symlink any selected line of a hunk means the whole hunk: staging "just the
/// added line" would make a link target with a newline in it.
fn symlink_selection(file: &FileDiff, sel: &Selection) -> Selection {
    let mut whole = Selection::default();
    for (&h, lines) in &sel.hunks {
        let hunk = file.hunks.get(h);
        let any = match lines {
            None => true,
            Some(set) => set
                .iter()
                .any(|&l| hunk.and_then(|hk| hk.lines.get(l)).is_some_and(|l| l.is_change())),
        };
        if any {
            whole.hunks.insert(h, None);
        }
    }
    whole
}

/// Builds a patch containing only the selected changes of `file`, ready for `git apply`
/// (with `-R` for reverse modes), as bytes: the lines' exact bytes in whatever encoding the
/// file has. Returns `None` if nothing would change. The header names the file as the
/// diff shows it: a name that is not UTF-8 gets its bytes back from
/// [`crate::Repo::raw_patch_names`].
pub fn build_patch(file: &FileDiff, sel: &Selection, mode: PatchMode) -> Option<Vec<u8>> {
    let symlink_sel;
    let sel = if is_symlink(file) {
        symlink_sel = symlink_selection(file, sel);
        &symlink_sel
    } else {
        sel
    };
    let reverse = mode.is_reverse();
    let mut body: Vec<u8> = Vec::new();
    let mut partial = false;
    let mut any = false;
    // Running difference (new - old) of the emitted hunks, to fix up the "other" start line.
    let mut delta: i64 = 0;

    for (hi, hunk) in file.hunks.iter().enumerate() {
        let mut lines: Vec<Out<'_>> = Vec::with_capacity(hunk.lines.len());
        let mut changes = 0usize;
        // A line's `\ No newline at end of file` marker is the line after it.
        let marker = |li: usize| {
            hunk.lines
                .get(li + 1)
                .filter(|l| l.kind == LineKind::NoNewline)
                .map(|l| l.bytes())
        };

        for li in line_order(&hunk.lines) {
            let line = &hunk.lines[li];
            let selected = sel.is_selected(hi, li);
            match line.kind {
                LineKind::Context => lines.push(Out::new(' ', line.bytes(), marker(li))),
                LineKind::Added | LineKind::Removed => {
                    let added = line.kind == LineKind::Added;
                    if selected {
                        let kind = if added { '+' } else { '-' };
                        lines.push(Out::new(kind, line.bytes(), marker(li)));
                        changes += 1;
                        continue;
                    }
                    partial = true;
                    // Unselected: a line that exists on the side the patch is applied to
                    // becomes context, the other one is dropped (with its marker).
                    let keep_as_context = if reverse { added } else { !added };
                    if keep_as_context {
                        lines.push(Out::new(' ', line.bytes(), marker(li)));
                    }
                }
                LineKind::NoNewline => {}
            }
        }

        if changes == 0 {
            if hunk.lines.iter().any(|l| l.is_change()) {
                partial = true;
            }
            continue;
        }
        any = true;
        let lines = fix_no_eol(lines);
        let old_count = lines.iter().filter(|l| l.kind != '+').count() as u32;
        let new_count = lines.iter().filter(|l| l.kind != '-').count() as u32;

        let (mut old_start, mut new_start) = if reverse {
            let new_start = hunk.new_start as i64;
            (new_start - delta, new_start)
        } else {
            let old_start = hunk.old_start as i64;
            (old_start, old_start + delta)
        };
        // A range with lines must start at >= 1; an empty range names the line before it.
        if old_count > 0 && old_start < 1 {
            old_start = 1;
        }
        if new_count > 0 && new_start < 1 {
            new_start = 1;
        }
        if old_count == 0 && new_count > 0 && !reverse {
            old_start = old_start.max(0);
        }
        delta += new_count as i64 - old_count as i64;

        // The section is display text (maybe decoded): `git apply` ignores it.
        body.extend_from_slice(
            format!(
                "@@ -{},{} +{},{} @@{}\n",
                old_start.max(0),
                old_count,
                new_start.max(0),
                new_count,
                hunk.section
            )
            .as_bytes(),
        );
        for l in lines {
            body.push(l.kind as u8);
            body.extend_from_slice(l.text);
            if l.cr {
                body.push(b'\r');
            }
            body.push(b'\n');
            if let Some(marker) = l.no_eol {
                body.extend_from_slice(marker);
                body.push(b'\n');
            }
        }
    }

    let whole_file_without_hunks = file.hunks.is_empty() && !sel.hunks.is_empty();
    if !any && !whole_file_without_hunks {
        return None;
    }

    let mut out: Vec<u8> = Vec::with_capacity(body.len() + 256);
    for h in patch_header(file, partial, reverse) {
        out.extend_from_slice(h.as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(&body);
    Some(out)
}

/// The file header for a patch.
/// - A mode change (`old mode` / `new mode`, e.g. `chmod +x`) goes only with a hunkless
///   diff (the mode change alone): like `git add -p`, hunks and lines are applied without
///   it, so staging or discarding some lines never flips the exec bit. The `index` line
///   then names the mode the target has (old in forward, new in reverse mode), so `git
///   apply` keeps it (without one it wrote a discarded-from file back as 0644).
/// - When only part of an added / deleted file is applied, the file keeps existing on the
///   side we apply to, so the patch must be a plain modification (its `---` / `+++` names
///   quoted the way git quotes them).
fn patch_header(file: &FileDiff, partial: bool, reverse: bool) -> Vec<String> {
    let drop_mode = !file.hunks.is_empty();
    let as_modification = partial
        && ((!reverse && file.change == FileChange::Deleted)
            || (reverse && file.change == FileChange::Added));
    if !drop_mode && !as_modification {
        return file.header.clone();
    }
    let path = file.path();
    let mode_of = |prefix: &str| {
        file.header
            .iter()
            .find_map(|h| h.strip_prefix(prefix))
            .map(str::to_string)
    };
    let kept_mode = if reverse { mode_of("new mode ") } else { mode_of("old mode ") };
    let mut out = Vec::with_capacity(file.header.len());
    for h in &file.header {
        if drop_mode && (h.starts_with("old mode ") || h.starts_with("new mode ")) {
            continue;
        }
        if !as_modification {
            match &kept_mode {
                Some(mode) if h.starts_with("index ") && h.split(' ').count() == 2 => {
                    out.push(format!("{h} {mode}"))
                }
                _ => out.push(h.clone()),
            }
        } else if h.starts_with("new file mode")
            || h.starts_with("deleted file mode")
            || h.starts_with("index ")
        {
            continue;
        } else if h.starts_with("--- ") {
            out.push(format!("--- {}", quote_path("a/", path)));
        } else if h.starts_with("+++ ") {
            out.push(format!("+++ {}", quote_path("b/", path)));
        } else {
            out.push(h.clone());
        }
    }
    out
}

/// `prefix` + `path` the way git writes it in a patch header: C-quoted when the path has
/// a double quote, a backslash or a control character (with `core.quotepath=false` other
/// non-ASCII characters stay as they are).
pub fn quote_path(prefix: &str, path: &str) -> String {
    if !path.chars().any(|c| c == '"' || c == '\\' || c.is_ascii_control()) {
        return format!("{prefix}{path}");
    }
    let mut out = String::from('"');
    for c in prefix.chars().chain(path.chars()) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\x0b' => out.push_str("\\v"),
            '\x0c' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c.is_ascii_control() => out.push_str(&format!("\\{:03o}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified;

    fn build_patch(f: &FileDiff, sel: &Selection, mode: PatchMode) -> Option<String> {
        super::build_patch(f, sel, mode).map(|p| String::from_utf8(p).unwrap())
    }

    const D: &str = "diff --git a/f b/f
index 1..2 100644
--- a/f
+++ b/f
@@ -1,3 +1,3 @@
 a
-b
+B
 c
";

    /// Workflow: ST-02
    #[gitti_test::test]
    fn stage_only_added_line() {
        let f = &parse_unified(D)[0];
        // line 2 = "+B"; "-b" unselected becomes context in forward mode
        let p = build_patch(f, &Selection::lines(0, [2]), PatchMode::Stage).unwrap();
        assert!(p.ends_with("@@ -1,3 +1,4 @@\n a\n b\n+B\n c\n"), "{p}");
    }

    #[gitti_test::test]
    fn unstage_only_removed_line() {
        let f = &parse_unified(D)[0];
        // reverse: unselected "+B" becomes context, "-b" stays
        let p = build_patch(f, &Selection::lines(0, [1]), PatchMode::Unstage).unwrap();
        assert!(p.ends_with("@@ -1,4 +1,3 @@\n a\n-b\n B\n c\n"), "{p}");
    }

    /// Both versions end without a newline: staging only the added line keeps the old
    /// last line, which then needs its newline (`-a` + marker, `+a`), instead of gluing
    /// `b` onto it.
    #[gitti_test::test]
    fn no_eol_context_followed_by_added_line() {
        let d = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n x\n-a\n\\ No newline at end of file\n+b\n\\ No newline at end of file\n";
        let f = &parse_unified(d)[0];
        let p = build_patch(f, &Selection::lines(0, [3]), PatchMode::Stage).unwrap();
        let nl = "\\ No newline at end of file";
        assert!(p.ends_with(&format!("@@ -1,2 +1,3 @@\n x\n-a\n{nl}\n+a\n+b\n{nl}\n")), "{p}");
        // Reverse: the unselected `+b` stays as context after `-a`, which loses its marker.
        let p = build_patch(f, &Selection::lines(0, [1]), PatchMode::Unstage).unwrap();
        assert!(p.ends_with(&format!("@@ -1,3 +1,2 @@\n x\n-a\n b\n{nl}\n")), "{p}");
    }

    #[gitti_test::test]
    fn quoted_paths_round_trip() {
        for path in ["plain", "tab\there", "q\"x", "b\\s", "nl\nx", "cr\r", "\x01\x07\x7f", "ü\tß"] {
            let quoted = quote_path("a/", path);
            assert_eq!(crate::diff::unquote(&quoted), format!("a/{path}"), "{quoted}");
        }
        assert_eq!(quote_path("b/", "x y"), "b/x y");
        assert_eq!(quote_path("a/", "t\tü"), "\"a/t\\tü\"");
    }

    /// Lines that are not UTF-8 go into the patch as their exact bytes.
    #[gitti_test::test]
    fn latin1_lines_keep_their_bytes() {
        let d = b"diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,2 +1,3 @@\n \xe9\n-\xe4\n+\xf6\n+\xfc\n";
        let f = &crate::diff::parse_unified_bytes(d)[0];
        let p = super::build_patch(f, &Selection::lines(0, [2]), PatchMode::Stage).unwrap();
        assert!(p.ends_with(b"@@ -1,2 +1,3 @@\n \xe9\n \xe4\n+\xf6\n"), "{}", String::from_utf8_lossy(&p));
        let p = super::build_patch(f, &Selection::lines(0, [1]), PatchMode::Discard).unwrap();
        assert!(p.ends_with(b"@@ -1,4 +1,3 @@\n \xe9\n-\xe4\n \xf6\n \xfc\n"), "{}", String::from_utf8_lossy(&p));
    }

    /// The kinds and texts of `d`'s first hunk in [`line_order`].
    fn ordered(d: &str) -> Vec<String> {
        let f = &parse_unified(d)[0];
        let lines = &f.hunks[0].lines;
        line_order(lines)
            .into_iter()
            .map(|i| {
                let l = &lines[i];
                let k = match l.kind {
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                    _ => ' ',
                };
                format!("{k}{}", l.text)
            })
            .collect()
    }

    /// Replaced blocks come out as pairs: similar lines as the diff view pairs them, the
    /// rest in order, extra lines of the longer side after.
    #[gitti_test::test]
    fn line_order_pairs_a_replaced_block() {
        let h = "diff --git a/f b/f\n--- a/f\n+++ b/f\n";
        // Every line's ending changed: each `-` line followed by its `+` line.
        let d = format!("{h}@@ -1,4 +1,4 @@\n x\n-a 1\n-a 2\n-a 3\n+a 1\r\n+a 2\r\n+a 3\r\n");
        assert_eq!(ordered(&d), [" x", "-a 1", "+a 1\r", "-a 2", "+a 2\r", "-a 3", "+a 3\r"]);
        // Dissimilar lines: in order, the extra `-` lines after.
        let d = format!("{h}@@ -1,3 +1,1 @@\n-alpha\n-beta\n-gamma\n+XYZ\n");
        assert_eq!(ordered(&d), ["-alpha", "+XYZ", "-beta", "-gamma"]);
        let d = format!("{h}@@ -1,1 +1,3 @@\n-alpha\n+XYZ\n+UVW\n+RST\n");
        assert_eq!(ordered(&d), ["-alpha", "+XYZ", "+UVW", "+RST"]);
        // A new line above a changed one: the changed line pairs with its new version, the
        // new line goes before them.
        let d = format!("{h}@@ -1,1 +1,2 @@\n-let total = 1;\n+// comment here\n+let total = 2;\n");
        assert_eq!(ordered(&d), ["+// comment here", "-let total = 1;", "+let total = 2;"]);
        // Markers stay out of the order (they go with their line).
        let d = format!("{h}@@ -1,2 +1,2 @@\n-a\n-b\n\\ No newline at end of file\n+A\n+B\n\\ No newline at end of file\n");
        assert_eq!(ordered(&d), ["-a", "+A", "-b", "+B"]);
    }

    /// Q29: the pair of a line in the middle of a replaced block stays in its place —
    /// stage (forward) and discard (reverse).
    #[gitti_test::test]
    fn middle_pair_of_a_replaced_block_stays_in_place() {
        let d = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n-a 1\n-a 2\n-a 3\n+a 1\r\n+a 2\r\n+a 3\r\n";
        let f = &parse_unified(d)[0];
        let p = build_patch(f, &Selection::lines(0, [1, 4]), PatchMode::Stage).unwrap();
        assert!(p.ends_with("@@ -1,3 +1,3 @@\n a 1\n-a 2\n+a 2\r\n a 3\n"), "{p}");
        let p = build_patch(f, &Selection::lines(0, [1, 4]), PatchMode::Discard).unwrap();
        assert!(p.ends_with("@@ -1,3 +1,3 @@\n a 1\r\n-a 2\n+a 2\r\n a 3\r\n"), "{p}");
        // Only the `+` line: next to the kept old line, not after the block.
        let p = build_patch(f, &Selection::lines(0, [4]), PatchMode::Stage).unwrap();
        assert!(p.ends_with("@@ -1,3 +1,4 @@\n a 1\n a 2\n+a 2\r\n a 3\n"), "{p}");
    }

    /// A line without a newline that gets one in a CRLF file gets `\r\n` (it is restored
    /// or kept before a line the patch adds after it).
    #[gitti_test::test]
    fn no_eol_line_gets_the_files_crlf() {
        let nl = "\\ No newline at end of file";
        let d = format!(
            "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n x\r\n-last\n{nl}\n+LAST\n{nl}\n"
        );
        let f = &parse_unified(&d)[0];
        // Stage only `+LAST`: the old last line stays and needs its newline.
        let p = build_patch(f, &Selection::lines(0, [3]), PatchMode::Stage).unwrap();
        assert!(p.ends_with(&format!("@@ -1,2 +1,3 @@\n x\r\n-last\n{nl}\n+last\r\n+LAST\n{nl}\n")), "{p:?}");
        // Discard only `-last`: it comes back before `LAST`, with CRLF.
        let p = build_patch(f, &Selection::lines(0, [1]), PatchMode::Discard).unwrap();
        assert!(p.ends_with(&format!("@@ -1,3 +1,2 @@\n x\r\n-last\r\n LAST\n{nl}\n")), "{p:?}");
        // LF file: LF.
        let lf = d.replace("x\r\n", "x\n");
        let f = &parse_unified(&lf)[0];
        let p = build_patch(f, &Selection::lines(0, [1]), PatchMode::Discard).unwrap();
        assert!(p.ends_with(&format!("@@ -1,3 +1,2 @@\n x\n-last\n LAST\n{nl}\n")), "{p:?}");
    }

    #[gitti_test::test]
    fn nothing_selected() {
        let f = &parse_unified(D)[0];
        assert!(build_patch(f, &Selection::default(), PatchMode::Stage).is_none());
        assert!(build_patch(f, &Selection::lines(0, [0]), PatchMode::Stage).is_none());
    }

    /// Shown names (with U+FFFD) go back to their bytes in the header only — plain and
    /// inside a C-quoted name — never in the hunk lines.
    #[gitti_test::test]
    fn raw_names_go_back_into_the_header() {
        let shown = "caf\u{fffd}.txt";
        let quoted = "a\"caf\u{fffd}";
        let patch = format!(
            "diff --git a/{shown} b/{shown}\n--- a/{shown}\n+++ b/{shown}\n@@ -1 +1 @@\n-{shown}\n+x\n"
        );
        let names = [(shown, b"caf\xe9.txt".to_vec()), (quoted, b"a\"caf\xe9".to_vec())];
        let out = with_raw_names(patch.as_bytes(), &names);
        let want = [
            &b"diff --git a/caf\xe9.txt b/caf\xe9.txt\n--- a/caf\xe9.txt\n+++ b/caf\xe9.txt\n"[..],
            format!("@@ -1 +1 @@\n-{shown}\n+x\n").as_bytes(),
        ]
        .concat();
        assert_eq!(out, want);
        let patch = format!("diff --git \"a/{q}\" \"b/{q}\"\n", q = quoted.replace('"', "\\\""));
        let out = with_raw_names(patch.as_bytes(), &names);
        assert_eq!(out, b"diff --git \"a/a\\\"caf\xe9\" \"b/a\\\"caf\xe9\"\n");
    }
}
