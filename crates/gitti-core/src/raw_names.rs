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
            None => Resolved::Unknown,
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
        let (magic, path) = split_magic(arg);
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
            .find(|a| self.resolve(split_magic(a).1) == Resolved::Ambiguous)
            .map(|a| split_magic(a).1)
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
