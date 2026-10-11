//! File names that are not UTF-8.
//!
//! Paths are strings in both apps: a name whose bytes are not valid UTF-8 (a Latin-1
//! `caf\xe9.txt` on Linux) is shown with U+FFFD in place of each invalid sequence. git
//! needs the real bytes back. Every [`crate::git_cli::Git`] runner of a repository shares
//! one [`RawNames`] table — shown name → bytes — learned from `git status -z`
//! ([`RawNames::learn_status`]); a path handed to git is put back into its bytes there:
//!
//! - arguments (`git add -- caf�.txt`, `:(literal)caf�.txt`, a folder above such a file)
//!   by the runner itself ([`RawNames::arg`]; unix only — Windows file names are UTF-16
//!   and reach git as UTF-8);
//! - pathspec lists on stdin and file-system access by the callers
//!   ([`crate::Repo::path_bytes`], [`crate::Repo::fs_path`]);
//! - the header of a hunk / line patch ([`crate::Repo::raw_patch_names`]).
//!
//! Two different names that show alike (`caf\xe9` and `caf\xe8` both read `caf�`) can't
//! be told apart from the shown one: such a name is [`Raw::Ambiguous`], and what would
//! act on it is refused in plain words before git runs ([`ambiguous_text`]). A valid
//! UTF-8 name that itself contains U+FFFD is learned too (as its own bytes), so it is
//! never mistaken for another one.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Mutex;

/// The replacement character `String::from_utf8_lossy` puts in place of invalid bytes.
pub const REPLACEMENT: char = '\u{FFFD}';

/// What a shown name stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raw {
    /// These bytes.
    One(Vec<u8>),
    /// More than one name shows like this.
    Ambiguous,
}

/// The shown names (with U+FFFD) of a repository's files and their bytes.
#[derive(Debug, Default)]
pub struct RawNames {
    map: Mutex<HashMap<String, Raw>>,
}

/// How a path given as a shown name resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// No U+FFFD in it: the path is its own bytes.
    Plain,
    /// The bytes of the name (or folder) it shows.
    Bytes(Vec<u8>),
    /// Two or more names show like it.
    Ambiguous,
    /// U+FFFD in it, but no name learned shows like it.
    Unknown,
}

/// How a path git printed (or a tree holds) shows, and its bytes when that is not the
/// same name (not UTF-8, or UTF-8 with U+FFFD in it): what [`RawNames::learn`] keeps.
pub fn shown(raw: &[u8]) -> (String, Option<Vec<u8>>) {
    match std::str::from_utf8(raw) {
        Ok(s) if !s.contains(REPLACEMENT) => (s.to_string(), None),
        _ => (String::from_utf8_lossy(raw).into_owned(), Some(raw.to_vec())),
    }
}

/// The refusal for a name more than one file shows as.
pub fn ambiguous_text(path: &str) -> String {
    format!(
        "More than one file name here reads “{path}” — their names are not UTF-8 and \
         differ only where gitti shows �, so gitti can't tell which one is meant. Rename \
         one of them (outside gitti) to act on it here."
    )
}

impl RawNames {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Raw>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether any name has been learned (nothing to put back otherwise).
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Learns `raw`, a path as git printed it, if it shows with U+FFFD (not UTF-8, or
    /// UTF-8 with U+FFFD in it).
    pub fn learn(&self, raw: &[u8]) {
        let shown = String::from_utf8_lossy(raw);
        if !shown.contains(REPLACEMENT) {
            return;
        }
        let mut map = self.lock();
        match map.get(shown.as_ref()) {
            Some(Raw::One(b)) if b.as_slice() == raw => {}
            Some(Raw::Ambiguous) => {}
            Some(Raw::One(_)) => {
                map.insert(shown.into_owned(), Raw::Ambiguous);
            }
            None => {
                map.insert(shown.into_owned(), Raw::One(raw.to_vec()));
            }
        }
    }

    /// `raw` (a path git printed, or a tree / index holds) as shown, learned when it
    /// doesn't read as itself ([`shown`]).
    pub fn show(&self, raw: &[u8]) -> String {
        let (text, bytes) = shown(raw);
        if bytes.is_some() {
            self.learn(raw);
        }
        text
    }

    /// Learns the NUL-separated paths of `out` (`git ls-files -z`, `diff --name-only -z`,
    /// …; cheap when there is nothing to learn).
    pub fn learn_list(&self, out: &[u8]) {
        if !needs_learning(out) {
            return;
        }
        for p in out.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            self.learn(p);
        }
    }

    /// Learns the paths of `git status --porcelain=v2 -z` output (cheap when there is
    /// nothing to learn: one UTF-8 check of the output).
    pub fn learn_status(&self, out: &[u8]) {
        if !needs_learning(out) {
            return;
        }
        let mut records = out.split(|b| *b == 0);
        while let Some(rec) = records.next() {
            let skip = match rec.first() {
                Some(b'1') => 8,
                Some(b'2') => 9,
                Some(b'u') => 10,
                Some(b'?' | b'!') => {
                    if let Some(p) = rec.get(2..) {
                        self.learn(p);
                    }
                    continue;
                }
                _ => continue,
            };
            if let Some(p) = after_fields(rec, skip) {
                self.learn(p);
            }
            if rec.first() == Some(&b'2')
                && let Some(orig) = records.next()
            {
                self.learn(orig);
            }
        }
    }

    /// What the shown path `path` stands for: a learned name, or a folder above learned
    /// names (`d�` / `d�/` for `d\xe9/x.txt`).
    pub fn resolve(&self, path: &str) -> Resolved {
        if !path.contains(REPLACEMENT) {
            return Resolved::Plain;
        }
        let map = self.lock();
        match map.get(path) {
            Some(Raw::One(b)) => return Resolved::Bytes(b.clone()),
            Some(Raw::Ambiguous) => return Resolved::Ambiguous,
            None => {}
        }
        // A folder: the same number of `/`-separated parts of the names below it (`/` is
        // ASCII, never part of an invalid sequence, so the parts line up).
        let dir = path.trim_end_matches('/');
        let parts = dir.split('/').count();
        let mut found: Option<Vec<u8>> = None;
        for (shown, raw) in map.iter() {
            if !shown.starts_with(dir) || shown.as_bytes().get(dir.len()) != Some(&b'/') {
                continue;
            }
            let Raw::One(raw) = raw else {
                return Resolved::Ambiguous;
            };
            let prefix: Vec<u8> = raw.split(|b| *b == b'/').take(parts).collect::<Vec<_>>().join(&b'/');
            match &found {
                Some(f) if *f != prefix => return Resolved::Ambiguous,
                _ => found = Some(prefix),
            }
        }
        match found {
            Some(mut raw) => {
                if path.ends_with('/') {
                    raw.push(b'/');
                }
                Resolved::Bytes(raw)
            }
            None => {
                drop(map);
                self.resolve_below(dir, path.ends_with('/'))
            }
        }
    }

    /// A path no learned name shows like, inside a folder one does (`d�/new.txt`,
    /// `d�/.gitattributes`): the folder's bytes and the rest, when the rest is UTF-8.
    fn resolve_below(&self, dir: &str, slash: bool) -> Resolved {
        // The last folder boundary: the rest must read as itself.
        let Some(i) = dir.rfind('/') else {
            return Resolved::Unknown;
        };
        let (head, rest) = (&dir[..i], &dir[i..]);
        if rest.contains(REPLACEMENT) {
            return Resolved::Unknown;
        }
        // The folder resolves like any path (learned, or inside a learned folder itself).
        match self.resolve(&format!("{head}/")) {
            Resolved::Bytes(mut raw) => {
                raw.pop(); // its `/`
                raw.extend_from_slice(rest.as_bytes());
                if slash {
                    raw.push(b'/');
                }
                Resolved::Bytes(raw)
            }
            other => other,
        }
    }

    /// The bytes git must get for the shown path `path`: its learned name's, or its own.
    /// `Err` (the plain-words refusal) when more than one name shows like it.
    pub fn bytes<'a>(&self, path: &'a str) -> Result<Cow<'a, [u8]>, String> {
        match self.resolve(path) {
            Resolved::Bytes(b) => Ok(Cow::Owned(b)),
            Resolved::Ambiguous => Err(ambiguous_text(path)),
            Resolved::Plain | Resolved::Unknown => Ok(Cow::Borrowed(path.as_bytes())),
        }
    }

    /// A git argument with a shown path put back into its bytes: the path itself or after
    /// a pathspec magic prefix (`:(literal)…`, `:(exclude,literal)…`). `None`: the
    /// argument stays as it is.
    pub fn arg(&self, arg: &str) -> Option<Vec<u8>> {
        if !arg.contains(REPLACEMENT) {
            return None;
        }
        let (magic, path) = split_arg(arg);
        match self.resolve(path) {
            Resolved::Bytes(raw) => Some([magic.as_bytes(), &raw].concat()),
            _ => None,
        }
    }

    /// The first of `args` that names more than one file ([`Resolved::Ambiguous`]).
    pub fn ambiguous_arg<'a>(&self, args: &[&'a str]) -> Option<&'a str> {
        args.iter()
            .copied()
            .filter(|a| a.contains(REPLACEMENT))
            .find(|a| self.resolve(split_arg(a).1) == Resolved::Ambiguous)
            .map(|a| split_arg(a).1)
    }
}

/// A name that is not UTF-8 with each invalid sequence (what shows as one U+FFFD) replaced
/// by a marker character of its own (private use, U+F0000 + n): text that string code can
/// work on — escape it for a rule file, cut folders off — and still turn back into the
/// exact bytes ([`Self::bytes`]) or into the shown text ([`Self::shown`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marked {
    /// The name with markers.
    pub text: String,
    parts: Vec<Vec<u8>>,
}

const MARK: u32 = 0xF_0000;

impl Marked {
    pub fn of(raw: &[u8]) -> Marked {
        let mut text = String::with_capacity(raw.len());
        let mut parts = Vec::new();
        for chunk in raw.utf8_chunks() {
            text.push_str(chunk.valid());
            if !chunk.invalid().is_empty() {
                text.extend(char::from_u32(MARK + parts.len() as u32));
                parts.push(chunk.invalid().to_vec());
            }
        }
        Marked { text, parts }
    }

    fn part(&self, c: char) -> Option<&[u8]> {
        let i = (c as u32).checked_sub(MARK)?;
        self.parts.get(i as usize).map(Vec::as_slice)
    }

    /// `s` (made from [`Self::text`]) as shown: each marker as U+FFFD.
    pub fn shown(&self, s: &str) -> String {
        s.chars().map(|c| if self.part(c).is_some() { REPLACEMENT } else { c }).collect()
    }

    /// `s` (made from [`Self::text`]) in bytes: each marker as the bytes it stands for.
    pub fn bytes(&self, s: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(s.len());
        let mut buf = [0; 4];
        for c in s.chars() {
            match self.part(c) {
                Some(b) => out.extend_from_slice(b),
                None => out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
            }
        }
        out
    }

    /// `shown` — the start of the name as shown (a folder above it) — marked like the
    /// name; `None` when the name doesn't start like that.
    pub fn prefix(&self, shown: &str) -> Option<String> {
        let text: String = self.text.chars().take(shown.chars().count()).collect();
        (self.shown(&text) == shown).then_some(text)
    }
}

/// The refusal for a rule-file pattern with U+FFFD whose bytes aren't known.
pub fn unknown_pattern_text(pattern: &str) -> String {
    format!(
        "“{pattern}” is made from a file name that is not UTF-8, and gitti can't tell which \
         bytes the � stand for. Choose one of the suggested patterns, write the pattern \
         without � (a ? or * in its place), or add it to the file outside gitti."
    )
}

impl RawNames {
    /// The bytes of `pattern`, a pattern for a rule file (`.gitignore`, `.gitattributes`)
    /// made from a shown name, with U+FFFD where the name is not UTF-8: `make` gives the
    /// patterns made from a name ([`Marked`]); every learned name is tried, and the one
    /// candidate that shows as `pattern` gives its bytes — with the marked name, for more
    /// patterns made from it. `Ok(None)`: no U+FFFD in `pattern` (its own bytes). `Err`
    /// in plain words: no learned name makes it (an edited pattern), or names that show
    /// alike make it with different bytes.
    pub fn rule_pattern(
        &self,
        pattern: &str,
        make: impl Fn(&Marked) -> Vec<String>,
    ) -> Result<Option<(Marked, String)>, String> {
        if !pattern.contains(REPLACEMENT) {
            return Ok(None);
        }
        let names: Vec<Vec<u8>> = self
            .lock()
            .values()
            .filter_map(|r| match r {
                Raw::One(b) => Some(b.clone()),
                Raw::Ambiguous => None,
            })
            .collect();
        let mut found: Option<(Marked, String, Vec<u8>)> = None;
        for raw in names {
            let m = Marked::of(&raw);
            for c in make(&m) {
                if m.shown(&c) != pattern {
                    continue;
                }
                let bytes = m.bytes(&c);
                match &found {
                    Some((.., b)) if *b != bytes => return Err(ambiguous_text(pattern)),
                    Some(_) => {}
                    None => found = Some((m.clone(), c, bytes)),
                }
            }
        }
        // A name more than one file shows as may make it too: then its bytes aren't
        // known (each U+FFFD stands for some invalid sequence: marked as one).
        let ambiguous: Vec<String> = self
            .lock()
            .iter()
            .filter(|(_, r)| **r == Raw::Ambiguous)
            .map(|(shown, _)| shown.clone())
            .collect();
        for shown in ambiguous {
            let mut raw = Vec::with_capacity(shown.len());
            let mut buf = [0; 4];
            for c in shown.chars() {
                match c {
                    REPLACEMENT => raw.push(0xFF),
                    c => raw.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
                }
            }
            let m = Marked::of(&raw);
            if make(&m).iter().any(|c| m.shown(c) == pattern) {
                return Err(ambiguous_text(pattern));
            }
        }
        match found {
            Some((m, c, _)) => Ok(Some((m, c))),
            None => Err(unknown_pattern_text(pattern)),
        }
    }
}

/// `out` may hold a path that shows with U+FFFD.
fn needs_learning(out: &[u8]) -> bool {
    std::str::from_utf8(out).is_err() || out.windows(3).any(|w| w == "\u{FFFD}".as_bytes())
}

/// The rest of `rec` after `n` space-separated fields.
fn after_fields(rec: &[u8], n: usize) -> Option<&[u8]> {
    let mut at = 0;
    for _ in 0..n {
        at += rec[at..].iter().position(|b| *b == b' ')? + 1;
    }
    Some(&rec[at..])
}

/// What comes before the path in a git argument naming one, and the path: pathspec magic
/// ([`split_magic`]), `./` (a path written so it doesn't read as an option), `--prefix=`
/// (`git subtree`'s folder).
fn split_arg(arg: &str) -> (&str, &str) {
    let (magic, path) = split_magic(arg);
    if !magic.is_empty() {
        return (magic, path);
    }
    for prefix in ["./", "--prefix="] {
        if let Some(rest) = arg.strip_prefix(prefix) {
            return (prefix, rest);
        }
    }
    ("", arg)
}

/// A pathspec's magic prefix (`:(literal)`, `:(top,exclude,literal)`) and the path.
fn split_magic(arg: &str) -> (&str, &str) {
    if arg.starts_with(":(")
        && let Some(end) = arg.find(')')
    {
        return arg.split_at(end + 1);
    }
    ("", arg)
}

/// The OS form of a git argument in bytes (unix: any bytes; elsewhere UTF-8 only, which
/// [`RawNames::arg`] never returns otherwise: Windows names are UTF-16).
#[cfg(unix)]
pub(crate) fn os_arg(bytes: Vec<u8>) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt as _;
    std::ffi::OsString::from_vec(bytes)
}

#[cfg(not(unix))]
pub(crate) fn os_arg(bytes: Vec<u8>) -> std::ffi::OsString {
    String::from_utf8_lossy(&bytes).into_owned().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &[u8] = b"caf\xe9.txt";
    const SHOWN: &str = "caf\u{fffd}.txt";

    fn status(paths: &[&[u8]]) -> Vec<u8> {
        let mut out = b"# branch.head main\0".to_vec();
        for p in paths {
            out.extend_from_slice(b"1 .M N... 100644 100644 100644 0000000 0000000 ");
            out.extend_from_slice(p);
            out.push(0);
        }
        out
    }

    #[gitti_test::test]
    fn learns_names_from_status_and_puts_them_back() {
        let names = RawNames::default();
        let mut out = status(&[b"plain.txt", RAW, b"d\xe9/x y.txt"]);
        out.extend_from_slice(b"2 R. N... 100644 100644 100644 0 0 R100 new.txt\0old\xe4.txt\0");
        out.extend_from_slice(b"? new\xfc.txt\0");
        names.learn_status(&out);
        assert_eq!(names.resolve("plain.txt"), Resolved::Plain);
        assert_eq!(names.resolve(SHOWN), Resolved::Bytes(RAW.to_vec()));
        assert_eq!(names.resolve("old\u{fffd}.txt"), Resolved::Bytes(b"old\xe4.txt".to_vec()));
        assert_eq!(names.resolve("new\u{fffd}.txt"), Resolved::Bytes(b"new\xfc.txt".to_vec()));
        // A name with a space after the fields; its folder, with and without the slash.
        assert_eq!(names.resolve("d\u{fffd}/x y.txt"), Resolved::Bytes(b"d\xe9/x y.txt".to_vec()));
        assert_eq!(names.resolve("d\u{fffd}"), Resolved::Bytes(b"d\xe9".to_vec()));
        assert_eq!(names.resolve("d\u{fffd}/"), Resolved::Bytes(b"d\xe9/".to_vec()));
        assert_eq!(names.resolve("other\u{fffd}"), Resolved::Unknown);
        // Arguments: the path, after pathspec magic; others untouched.
        assert_eq!(names.arg(SHOWN), Some(RAW.to_vec()));
        assert_eq!(names.arg(&format!(":(literal){SHOWN}")), Some(b":(literal)caf\xe9.txt".to_vec()));
        assert_eq!(names.arg("--"), None);
        assert_eq!(names.arg("other\u{fffd}"), None);
        assert_eq!(names.bytes("a.txt").unwrap().as_ref(), b"a.txt");
        assert_eq!(names.bytes(SHOWN).unwrap().as_ref(), RAW);
    }

    /// A path inside a learned folder (`d�/new.txt`), `./` and `--prefix=` in front of a
    /// path, NUL-separated lists, a tree name shown and learned.
    #[gitti_test::test]
    fn paths_below_learned_folders_and_argument_forms() {
        let names = RawNames::default();
        names.learn_list(b"d\xe9/x.txt\0plain\0");
        assert_eq!(names.resolve("d\u{fffd}/.gitattributes"), Resolved::Bytes(b"d\xe9/.gitattributes".to_vec()));
        assert_eq!(names.resolve("d\u{fffd}/a/b/"), Resolved::Bytes(b"d\xe9/a/b/".to_vec()));
        assert_eq!(names.resolve("d\u{fffd}/caf\u{fffd}"), Resolved::Unknown);
        assert_eq!(names.resolve("e\u{fffd}/x"), Resolved::Unknown);
        assert_eq!(names.arg("./d\u{fffd}/x.txt"), Some(b"./d\xe9/x.txt".to_vec()));
        assert_eq!(names.arg("--prefix=d\u{fffd}"), Some(b"--prefix=d\xe9".to_vec()));
        assert_eq!(names.show(b"n\xfc.txt"), "n\u{fffd}.txt");
        assert_eq!(names.resolve("n\u{fffd}.txt"), Resolved::Bytes(b"n\xfc.txt".to_vec()));
        assert_eq!(shown(b"plain"), ("plain".to_string(), None));
    }

    /// Marked names: string work on the text, the bytes and the shown text back.
    #[gitti_test::test]
    fn marked_names_keep_their_bytes() {
        let m = Marked::of(b"d\xe9/caf\xe8 \xff\xfe.txt");
        assert_eq!(m.shown(&m.text), "d\u{fffd}/caf\u{fffd} \u{fffd}\u{fffd}.txt");
        assert_eq!(m.bytes(&m.text), b"d\xe9/caf\xe8 \xff\xfe.txt");
        let escaped = format!("/{}", m.text.replace(' ', "\\ "));
        assert_eq!(m.bytes(&escaped), b"/d\xe9/caf\xe8\\ \xff\xfe.txt");
        let dir = m.prefix("d\u{fffd}").unwrap();
        assert_eq!(m.bytes(&dir), b"d\xe9");
        assert_eq!(m.prefix("x"), None);
    }

    /// A rule-file pattern made from a shown name gets the name's bytes; an edited one or
    /// one names that show alike make is refused in plain words.
    #[gitti_test::test]
    fn rule_patterns_get_the_bytes_of_the_name_they_were_made_from() {
        let names = RawNames::default();
        names.learn_list(b"d\xe9/caf\xe9.txt\0x\xe8.bin\0");
        // Candidates: the name itself and its last part (as the suggestions do).
        let make = |m: &Marked| vec![format!("/{}", m.text), m.text.rsplit('/').next().unwrap().to_string()];
        assert_eq!(names.rule_pattern("*.txt", make).unwrap(), None);
        let (m, c) = names.rule_pattern("/d\u{fffd}/caf\u{fffd}.txt", make).unwrap().unwrap();
        assert_eq!(m.bytes(&c), b"/d\xe9/caf\xe9.txt");
        let (m, c) = names.rule_pattern("caf\u{fffd}.txt", make).unwrap().unwrap();
        assert_eq!(m.bytes(&c), b"caf\xe9.txt");
        let err = names.rule_pattern("/caf\u{fffd}.md", make).unwrap_err();
        assert!(err.contains("can't tell which"), "{err}");
        // Another name whose last part shows the same: `caf�.txt` alone is ambiguous.
        names.learn(b"e/caf\xe8.txt");
        let err = names.rule_pattern("caf\u{fffd}.txt", make).unwrap_err();
        assert!(err.contains("More than one file name"), "{err}");
        assert!(names.rule_pattern("/e/caf\u{fffd}.txt", make).is_ok());
        // A name that is ambiguous itself.
        names.learn(b"d\xe8/caf\xe9.txt");
        assert!(names.rule_pattern("/d\u{fffd}/caf\u{fffd}.txt", make).is_err());
    }

    #[gitti_test::test]
    fn valid_utf8_output_learns_nothing() {
        let names = RawNames::default();
        names.learn_status(&status(&[b"caf\xc3\xa9.txt"]));
        assert!(names.is_empty());
    }

    #[gitti_test::test]
    fn names_that_show_alike_are_ambiguous() {
        let names = RawNames::default();
        names.learn_status(&status(&[RAW, b"caf\xe8.txt", b"d\xe9/a", b"d\xe8/b"]));
        assert_eq!(names.resolve(SHOWN), Resolved::Ambiguous);
        assert_eq!(names.resolve("d\u{fffd}"), Resolved::Ambiguous);
        assert_eq!(names.arg(SHOWN), None);
        assert_eq!(names.ambiguous_arg(&["add", "--", &format!(":(literal){SHOWN}")]), Some(SHOWN));
        let err = names.bytes(SHOWN).unwrap_err();
        assert!(err.contains("can't tell which one"), "{err}");
        // A valid name with U+FFFD in it is its own: told apart from a Latin-1 one.
        let names = RawNames::default();
        names.learn_status(&status(&[SHOWN.as_bytes()]));
        assert_eq!(names.resolve(SHOWN), Resolved::Bytes(SHOWN.as_bytes().to_vec()));
        names.learn_status(&status(&[RAW]));
        assert_eq!(names.resolve(SHOWN), Resolved::Ambiguous);
    }
}
