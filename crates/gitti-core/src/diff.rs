//! Unified diff model and parser (output of `git diff` / `git show`).

pub mod encoding;

use std::borrow::Cow;

pub use encoding::TextEncoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file` — belongs to the preceding line.
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line content without the leading marker and without trailing newline.
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// The line's exact bytes when `text` is a decoding of them (the line is not UTF-8,
    /// [`FileDiff::encoding`]); `None` when `text` is exact.
    pub raw: Option<Box<[u8]>>,
}

impl DiffLine {
    pub fn is_change(&self) -> bool {
        matches!(self.kind, LineKind::Added | LineKind::Removed)
    }

    /// The line's exact bytes (without marker and newline): what a patch carries.
    pub fn bytes(&self) -> &[u8] {
        self.raw.as_deref().unwrap_or(self.text.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// Text after the second `@@` (usually the enclosing function).
    pub section: String,
    pub lines: Vec<DiffLine>,
    /// Intra-line ranges, computed on first use ([`Hunk::intra_ranges`]).
    pub intra: crate::intraline::IntraCache,
}

impl Hunk {
    /// The changed char ranges (of the expanded text, `rows::expand_line`) of line `line`
    /// against the similar line of the other side it was paired with; empty for context,
    /// unpaired or too dissimilar lines. The whole hunk is computed on the first call (per
    /// whitespace mode: `ignore_ws` = the diff view's ignore-whitespace setting).
    pub fn intra_ranges(&self, line: usize, ignore_ws: bool) -> &[std::ops::Range<u32>] {
        self.intra
            .get(&self.lines, ignore_ws)
            .get(line)
            .map_or(&[], |v| v.as_slice())
    }

    pub fn header(&self) -> String {
        format!(
            "@@ -{},{} +{},{} @@{}",
            self.old_start, self.old_count, self.new_start, self.new_count, self.section
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

impl FileChange {
    /// git's status letter (`git diff --name-status`).
    pub fn letter(self) -> char {
        match self {
            FileChange::Added => 'A',
            FileChange::Deleted => 'D',
            FileChange::Renamed => 'R',
            FileChange::Copied => 'C',
            FileChange::TypeChanged => 'T',
            FileChange::Modified => 'M',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub change: FileChange,
    pub binary: bool,
    /// Raw header lines (`diff --git`, `index`, mode lines, `---`, `+++`), needed to rebuild patches.
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
    /// A submodule pointer change (gitlink): shown as a commit summary, staged by path.
    pub submodule: Option<crate::submodule::SubmoduleDiff>,
    /// Commit view: the diff wasn't loaded — too large (the size of the file's larger
    /// side, bytes) or after the commit's output limit ([`crate::commit_files`]); "Load
    /// anyway" is [`crate::Repo::load_omitted_file`].
    pub too_large: Option<u64>,
    /// How the hunk lines are decoded for display ([`parse_unified_bytes`]); lines that
    /// are not UTF-8 keep their exact bytes ([`DiffLine::raw`]) for patches.
    pub encoding: TextEncoding,
    /// A header line (a file name) was not valid UTF-8 and is shown with U+FFFD in its
    /// place. Paths are strings app-wide: git gets such a name's bytes back from
    /// [`crate::raw_names`] (a patch: [`crate::Repo::raw_patch_names`]).
    pub lossy_name: bool,
    /// A Git LFS file: the pointer change as sizes / ids (and, when the objects are local
    /// text, `hunks` are the content's diff), or a file under an LFS path; whole-file
    /// staging only ([`crate::lfs`]).
    pub lfs: Option<Box<crate::lfs::LfsDiff>>,
}

impl FileDiff {
    /// The path to display / operate on (new path unless deleted).
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("")
    }

    /// Why a text file's diff has no lines, in a few words (both frontends show it in
    /// place of the hunks): an empty file added or deleted, a rename or copy without
    /// content changes, a mode change only.
    pub fn no_lines_note(&self) -> &'static str {
        let mode = self.header.iter().any(|h| h.starts_with("old mode "));
        match self.change {
            FileChange::Added => "New empty file",
            FileChange::Deleted => "Empty file deleted",
            FileChange::Renamed if mode => "Renamed, mode changed — no content changes",
            FileChange::Renamed => "Renamed — no content changes",
            FileChange::Copied => "Copied — no content changes",
            _ if mode => "Mode change only — no content changes",
            _ => "No content changes",
        }
    }

    /// Added / removed lines (none for a submodule pointer: it has no lines of its own).
    pub fn added_removed(&self) -> (usize, usize) {
        if self.submodule.is_some() {
            return (0, 0);
        }
        let mut a = 0;
        let mut r = 0;
        for l in self.hunks.iter().flat_map(|h| &h.lines) {
            match l.kind {
                LineKind::Added => a += 1,
                LineKind::Removed => r += 1,
                _ => {}
            }
        }
        (a, r)
    }
}

/// Parses the output of `git diff` (no color, default prefixes) into file diffs.
pub fn parse_unified(input: &str) -> Vec<FileDiff> {
    parse_lines(input.split('\n').map(|l| (Cow::Borrowed(l), None)), false)
}

/// [`parse_unified`] of git's raw output, byte-exact: a hunk line that is not
/// valid UTF-8 keeps its bytes ([`DiffLine::raw`]) and is decoded for display
/// ([`encoding`]: Windows-1252 / Latin-1, or UTF-16 for a file with a byte order mark),
/// which [`FileDiff::encoding`] names. Valid UTF-8 without NUL bytes (the usual case)
/// costs one validation pass more than [`parse_unified`].
pub fn parse_unified_bytes(input: &[u8]) -> Vec<FileDiff> {
    if !input.contains(&0)
        && let Ok(text) = std::str::from_utf8(input)
    {
        return parse_unified(text);
    }
    // `\n` is never part of a multi-byte UTF-8 sequence: decoding per line is exact.
    parse_lines(
        input.split(|b| *b == b'\n').map(|l| match std::str::from_utf8(l) {
            Ok(s) => (Cow::Borrowed(s), None),
            Err(_) => (String::from_utf8_lossy(l), Some(l)),
        }),
        true,
    )
}

/// The parser behind [`parse_unified`]: lines without their `\n`, each with its exact
/// bytes when the text had to be decoded lossily. `check_encoding`: the input has lines
/// that are not UTF-8 or NUL bytes — decide each file's [`TextEncoding`] at its end.
fn parse_lines<'a>(
    lines: impl Iterator<Item = (Cow<'a, str>, Option<&'a [u8]>)>,
    check_encoding: bool,
) -> Vec<FileDiff> {
    let mut files = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    let mut in_hunk = false;

    for (line, raw) in lines {
        let line: &str = &line;
        let lossy = raw.is_some();
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(f) = cur.take() {
                files.push(f);
            }
            let (a, b) = split_git_paths(rest);
            cur = Some(FileDiff {
                old_path: a,
                new_path: b,
                change: FileChange::Modified,
                binary: false,
                header: vec![line.to_string()],
                hunks: Vec::new(),
                submodule: None,
                too_large: None,
                encoding: TextEncoding::Utf8,
                lossy_name: lossy,
                lfs: None,
            });
            in_hunk = false;
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };

        if line.starts_with("@@") {
            if let Some(mut h) = parse_hunk_header(line) {
                // The section (a function name) in the file's encoding: its ASCII prefix
                // `@@ -a,b +c,d @@` has the same length in the bytes.
                if let Some(raw) = raw {
                    h.section = encoding::decode_8bit(&raw[line.len() - h.section.len()..]);
                }
                old_no = h.old_start;
                new_no = h.new_start;
                f.hunks.push(h);
                in_hunk = true;
            }
            continue;
        }

        if in_hunk {
            let hunk = f.hunks.last_mut().expect("in hunk");
            let (kind, text) = match line.as_bytes().first() {
                Some(b' ') => (LineKind::Context, &line[1..]),
                Some(b'+') => (LineKind::Added, &line[1..]),
                Some(b'-') => (LineKind::Removed, &line[1..]),
                Some(b'\\') => (LineKind::NoNewline, line),
                // Empty line: trailing split artefact or a context line whose space was stripped.
                None => {
                    if hunk_complete(hunk) {
                        continue;
                    }
                    (LineKind::Context, "")
                }
                _ => {
                    in_hunk = false;
                    f.lossy_name |= lossy;
                    f.header.push(line.to_string());
                    continue;
                }
            };
            let (o, n) = match kind {
                LineKind::Context => {
                    old_no += 1;
                    new_no += 1;
                    (Some(old_no - 1), Some(new_no - 1))
                }
                LineKind::Added => {
                    new_no += 1;
                    (None, Some(new_no - 1))
                }
                LineKind::Removed => {
                    old_no += 1;
                    (Some(old_no - 1), None)
                }
                LineKind::NoNewline => (None, None),
            };
            // Not UTF-8: the exact bytes after the marker, and their decoding.
            let (text, raw) = match raw {
                Some(raw) if kind != LineKind::NoNewline => {
                    (encoding::decode_8bit(&raw[1..]), Some(raw[1..].into()))
                }
                _ => (text.to_string(), None),
            };
            hunk.lines.push(DiffLine {
                kind,
                text,
                old_no: o,
                new_no: n,
                raw,
            });
            continue;
        }

        // Extended header lines.
        if line.is_empty() {
            continue;
        }
        f.lossy_name |= lossy;
        f.header.push(line.to_string());
        if line.starts_with("new file mode") {
            f.change = FileChange::Added;
        } else if line.starts_with("deleted file mode") {
            f.change = FileChange::Deleted;
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.change = FileChange::Renamed;
            f.old_path = Some(p.to_string());
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.change = FileChange::Renamed;
            f.new_path = Some(p.to_string());
        } else if line.starts_with("copy from ") {
            f.change = FileChange::Copied;
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            f.binary = true;
        } else if let Some(p) = line.strip_prefix("--- ") {
            f.old_path = strip_prefix_path(p, "a/");
        } else if let Some(p) = line.strip_prefix("+++ ") {
            f.new_path = strip_prefix_path(p, "b/");
        } else if line.starts_with("old mode ") && f.change == FileChange::Modified {
            // mode-only change stays "Modified"
        }
    }
    if let Some(f) = cur.take() {
        files.push(f);
    }
    for f in &mut files {
        if f.change == FileChange::Added {
            f.old_path = None;
        }
        if f.change == FileChange::Deleted {
            f.new_path = None;
        }
        f.submodule = crate::submodule::SubmoduleDiff::detect(&f.header, &f.hunks);
        if check_encoding && !f.binary {
            decide_encoding(f);
        }
    }
    files
}

/// A file's [`TextEncoding`] from its hunk lines ([`encoding`]): UTF-16 re-decodes every
/// line from its bytes; lines that are not UTF-8 were decoded as Windows-1252 already.
fn decide_encoding(f: &mut FileDiff) {
    let lines = || {
        f.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind != LineKind::NoNewline)
    };
    let has_nul = lines().any(|l| l.bytes().contains(&0));
    let utf16 = has_nul
        .then(|| {
            encoding::detect_utf16(
                lines().map(|l| (l.bytes(), l.old_no == Some(1) || l.new_no == Some(1))),
            )
        })
        .flatten();
    if let Some(enc) = utf16 {
        let le = enc == TextEncoding::Utf16Le;
        for l in f.hunks.iter_mut().flat_map(|h| &mut h.lines) {
            if l.kind == LineKind::NoNewline {
                continue;
            }
            let bytes: Box<[u8]> = l.bytes().into();
            l.text = encoding::decode_utf16_line(&bytes, le);
            l.raw = Some(bytes);
        }
        f.encoding = enc;
    } else if lines().any(|l| l.raw.is_some()) {
        f.encoding = encoding::eight_bit(lines().filter_map(|l| l.raw.as_deref()));
    }
}

fn hunk_complete(h: &Hunk) -> bool {
    let mut o = 0;
    let mut n = 0;
    for l in &h.lines {
        match l.kind {
            LineKind::Context => {
                o += 1;
                n += 1
            }
            LineKind::Added => n += 1,
            LineKind::Removed => o += 1,
            LineKind::NoNewline => {}
        }
    }
    o >= h.old_count && n >= h.new_count
}

fn strip_prefix_path(p: &str, prefix: &str) -> Option<String> {
    let p = unquote(p.trim_end_matches('\t'));
    if p == "/dev/null" {
        return None;
    }
    Some(p.strip_prefix(prefix).unwrap_or(&p).to_string())
}

/// Splits `a/foo b/foo` (the part after `diff --git `). Best effort for paths with spaces:
/// if both halves are identical we can split exactly in the middle.
fn split_git_paths(rest: &str) -> (Option<String>, Option<String>) {
    if rest.starts_with('"') {
        // Quoted paths: "a/x y" "b/x y"
        let mut parts = Vec::new();
        let mut chars = rest.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '"' {
                let start = i;
                let mut end = rest.len();
                let mut escaped = false;
                for (j, c2) in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if c2 == '\\' {
                        escaped = true;
                    } else if c2 == '"' {
                        end = j + 1;
                        break;
                    }
                }
                parts.push(unquote(&rest[start..end]));
            } else if c != ' ' {
                let end = rest[i..].find(' ').map(|e| i + e).unwrap_or(rest.len());
                parts.push(rest[i..end].to_string());
                while chars.peek().is_some_and(|(j, _)| *j < end) {
                    chars.next();
                }
            }
        }
        let a = parts
            .first()
            .map(|p| p.strip_prefix("a/").unwrap_or(p).to_string());
        let b = parts
            .get(1)
            .map(|p| p.strip_prefix("b/").unwrap_or(p).to_string());
        return (a, b);
    }
    let len = rest.len();
    if len % 2 == 1 {
        let mid = len / 2;
        let (a, b) = (&rest[..mid], &rest[mid + 1..]);
        if a.strip_prefix("a/") == b.strip_prefix("b/") {
            return (Some(a[2..].to_string()), Some(b[2..].to_string()));
        }
    }
    match rest.split_once(" b/") {
        Some((a, b)) => (
            Some(a.strip_prefix("a/").unwrap_or(a).to_string()),
            Some(b.to_string()),
        ),
        None => (None, None),
    }
}

/// Undo git's C-style quoting (`"foo\tbar"`), including octal escapes of UTF-8 bytes.
pub fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return s.to_string();
    };
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let c = bytes[i + 1];
            match c {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'a' => out.push(0x07),
                b'b' => out.push(0x08),
                b'v' => out.push(0x0b),
                b'f' => out.push(0x0c),
                b'"' => out.push(b'"'),
                b'\\' => out.push(b'\\'),
                b'0'..=b'7' if i + 4 <= bytes.len() => {
                    let oct = std::str::from_utf8(&bytes[i + 1..i + 4]).ok();
                    if let Some(v) = oct.and_then(|o| u8::from_str_radix(o, 8).ok()) {
                        out.push(v);
                        i += 4;
                        continue;
                    }
                    out.push(c);
                }
                _ => out.push(c),
            }
            i += 2;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_hunk_header(line: &str) -> Option<Hunk> {
    // @@ -a,b +c,d @@ section
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let (old_start, old_count) = parse_range(old)?;
    let (new_start, new_count) = parse_range(new)?;
    Some(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        section: section.to_string(),
        lines: Vec::new(),
        intra: Default::default(),
    })
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files without lines say why: an empty file added / deleted, a rename, a mode change.
    #[gitti_test::test]
    fn no_lines_note_names_the_reason() {
        let raw = "diff --git a/e b/e\nnew file mode 100644\nindex 0000000..e69de29\n\
                   diff --git a/f b/f\ndeleted file mode 100644\nindex e69de29..0000000\n\
                   diff --git a/g b/h\nsimilarity index 100%\nrename from g\nrename to h\n\
                   diff --git a/x.sh b/x.sh\nold mode 100644\nnew mode 100755\n";
        let notes: Vec<&str> = parse_unified(raw).iter().map(FileDiff::no_lines_note).collect();
        assert_eq!(
            notes,
            ["New empty file", "Empty file deleted", "Renamed — no content changes", "Mode change only — no content changes"]
        );
    }

    const SAMPLE: &str = "diff --git a/src/x.rs b/src/x.rs
index 111..222 100644
--- a/src/x.rs
+++ b/src/x.rs
@@ -1,4 +1,5 @@ fn main
 a
-b
+B
+C
 c
 d
@@ -10,2 +11,2 @@
 x
-y
+z
\\ No newline at end of file
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
";

    #[gitti_test::test]
    fn parses_files_and_hunks() {
        let files = parse_unified(SAMPLE);
        assert_eq!(files.len(), 2);
        let f = &files[0];
        assert_eq!(f.path(), "src/x.rs");
        assert_eq!(f.hunks.len(), 2);
        assert_eq!(f.hunks[0].lines.len(), 6);
        assert_eq!(f.hunks[0].section, " fn main");
        assert_eq!(f.hunks[1].lines.last().unwrap().kind, LineKind::NoNewline);
        assert_eq!(f.added_removed(), (3, 2));
        assert_eq!(files[1].change, FileChange::Added);
        assert_eq!(files[1].path(), "new.txt");
        assert_eq!(files[1].old_path, None);
        assert_eq!(files[1].hunks[0].new_count, 1);
    }

    #[gitti_test::test]
    fn line_numbers() {
        let files = parse_unified(SAMPLE);
        let h = &files[0].hunks[0];
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[2].new_no, Some(2));
        assert_eq!(h.lines[4].old_no, Some(3));
        assert_eq!(h.lines[4].new_no, Some(4));
    }

    /// A Latin-1 line keeps its bytes and is shown decoded (Windows-1252), only in its
    /// file; a Latin-1 file name marks the file `lossy_name`; valid UTF-8 parses exactly
    /// like the `&str` parser.
    #[gitti_test::test]
    fn lines_that_are_not_utf8_keep_their_bytes() {
        let mut raw = b"diff --git a/l.txt b/l.txt\nindex 1..2 100644\n--- a/l.txt\n+++ b/l.txt\n@@ -1,2 +1,3 @@ f\xfcr\n a\n+\xe4\x80\n b\n".to_vec();
        raw.extend_from_slice(SAMPLE.as_bytes());
        raw.extend_from_slice(b"diff --git a/caf\xe9 b/caf\xe9\n--- a/caf\xe9\n+++ b/caf\xe9\n@@ -1 +1 @@\n-a\n+b\n");
        let files = parse_unified_bytes(&raw);
        assert_eq!(files.len(), 4);
        let l = &files[0];
        assert_eq!((l.encoding, l.lossy_name), (TextEncoding::Windows1252, false));
        assert_eq!(l.hunks[0].section, " für");
        let line = &l.hunks[0].lines[1];
        assert_eq!((line.text.as_str(), line.bytes()), ("ä€", &b"\xe4\x80"[..]));
        assert_eq!(l.hunks[0].lines[0].raw, None);
        assert_eq!(l.hunks[0].lines[2].new_no, Some(3));
        assert_eq!(files[1..3], parse_unified(SAMPLE)[..]);
        assert!(files[3].lossy_name && files[3].encoding == TextEncoding::Utf8);
        assert_eq!(parse_unified_bytes(SAMPLE.as_bytes()), parse_unified(SAMPLE));
    }

    /// A UTF-16 LE file shown as text (a `diff` attribute): every line keeps its bytes
    /// (the newline's NUL starts the next line) and is shown decoded.
    #[gitti_test::test]
    fn utf16_file_lines() {
        let le = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
        let mut raw = b"diff --git a/w.txt b/w.txt\nindex 1..2 100644\n--- a/w.txt\n+++ b/w.txt\n@@ -1,3 +1,4 @@\n ".to_vec();
        let new = le("\u{FEFF}a\nä\nc\n");
        let lines: Vec<&[u8]> = new.split(|b| *b == b'\n').collect();
        raw.extend_from_slice(lines[0]);
        raw.extend_from_slice(b"\n+");
        raw.extend_from_slice(lines[1]);
        raw.extend_from_slice(b"\n ");
        raw.extend_from_slice(lines[2]);
        raw.extend_from_slice(b"\n ");
        raw.extend_from_slice(lines[3]);
        raw.extend_from_slice(b"\n\\ No newline at end of file\n");
        let f = &parse_unified_bytes(&raw)[0];
        assert_eq!(f.encoding, TextEncoding::Utf16Le);
        let texts: Vec<&str> = f.hunks[0].lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["a", "ä", "c", "", "\\ No newline at end of file"]);
        assert_eq!(f.hunks[0].lines[1].bytes(), lines[1]);
        assert_eq!(f.hunks[0].lines[4].raw, None);
    }

    #[gitti_test::test]
    fn paths_with_spaces() {
        assert_eq!(
            split_git_paths("a/my file.txt b/my file.txt"),
            (Some("my file.txt".into()), Some("my file.txt".into()))
        );
        assert_eq!(unquote("\"a/\\303\\244.txt\""), "a/ä.txt");
    }
}
