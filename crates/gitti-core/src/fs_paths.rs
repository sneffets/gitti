//! Paths the work tree's file system can't hold.
//!
//! git stores paths as bytes; a work tree is a file system with its own rules. Where the
//! two disagree git can't check a path out as it is, and both apps would otherwise show a
//! silent wrong result (a file that stays "modified" after Discard, a checkout that fails
//! with git's raw "invalid path"):
//!
//! - **case collisions** on a case-insensitive file system (macOS, Windows, FAT / exFAT /
//!   SMB mounts): `xt_TCPMSS.h` and `xt_tcpmss.h` are one file there, so one of each pair
//!   always shows as modified and restoring one overwrites the other; also a file where
//!   another path needs a folder (`docs` vs `Docs/a.md`);
//! - **Unicode forms**: on macOS `é` composed (NFC) and decomposed (NFD) name the same
//!   file; with `core.precomposeunicode` git reads names composed, so a decomposed name in
//!   the index shows as deleted plus an untracked composed one;
//! - **Windows names** (what Git for Windows' `core.protectNTFS` refuses): device names
//!   (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`9`, `LPT1`–`9`, also with superscript digits, any
//!   extension, any case, as folder names too), `<>:"|?*\` and control characters, a
//!   trailing dot or space, paths over `MAX_PATH` without `core.longpaths`;
//! - **aliases of `.git`** git refuses everywhere (`GIT~1`, `.git.`, HFS-ignorable
//!   characters with `core.protectHFS`);
//! - names over 255 bytes;
//! - **symbolic links** with `core.symlinks=false` (checked out as small text files) or on
//!   a file system without them.
//!
//! What applies is decided by the work tree's actual file system ([`probe`]: a case-
//! insensitive APFS volume on Linux-cloned repositories, a FAT stick on Linux, a
//! case-sensitive APFS volume on the Mac) together with git's config — not by the OS
//! alone. The checks themselves are pure ([`check`], [`FsRules`]); [`crate::Repo::fs_paths`]
//! runs them over the index (cached while the index is unchanged) for both apps' status
//! marks, the diff pane's explanation and the repository's note; operations that change
//! the index say which new paths the file system can't hold
//! ([`crate::Repo::noting_fs_paths`]), and git's errors about such paths are put in plain
//! words ([`explain_git_error`]).

use std::collections::HashMap;
use std::path::Path;

use unicode_normalization::UnicodeNormalization as _;

/// Test override of the file system probe, e.g. `case,unicode` (comma-separated:
/// `case`, `unicode`, `chars`, `reserved`, `nosymlinks`, and `windows` / `macos` to take
/// that OS's rules; empty = a case-sensitive file system with everything allowed).
pub const ENV_TEST_FS: &str = "GITTI_TEST_FS";

/// The operating system whose rules apply besides the file system's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Windows,
    MacOs,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        }
    }
}

/// What a probe found out about a file system ([`probe`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FsProbe {
    /// `aB` and `Ab` are one file.
    pub case_insensitive: bool,
    /// `é` composed (NFC) and decomposed (NFD) are one file.
    pub unicode_insensitive: bool,
    /// Names with `?` (and the other characters Windows refuses) can't be created.
    pub refuses_chars: bool,
    /// `aux.x` can't be created (a Windows device name).
    pub refuses_reserved: bool,
    /// Symbolic links can be created (`None`: not probed — Windows, where git's
    /// `core.symlinks` says it).
    pub symlinks: Option<bool>,
}

impl FsProbe {
    /// [`ENV_TEST_FS`]'s probe and OS, when it is set.
    pub fn from_test_env() -> Option<(FsProbe, Os)> {
        let v = std::env::var(ENV_TEST_FS).ok()?;
        let flags: Vec<&str> = v.split(',').map(str::trim).collect();
        let has = |f: &str| flags.contains(&f);
        let os = if has("windows") {
            Os::Windows
        } else if has("macos") {
            Os::MacOs
        } else {
            Os::Other
        };
        Some((
            FsProbe {
                case_insensitive: has("case"),
                unicode_insensitive: has("unicode"),
                refuses_chars: has("chars"),
                refuses_reserved: has("reserved"),
                symlinks: Some(!has("nosymlinks")),
            },
            os,
        ))
    }
}

/// Probe results set by tests for one work tree ([`set_test_probe`]).
static TEST_PROBES: std::sync::Mutex<Vec<(std::path::PathBuf, FsProbe, Os)>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: the work tree at `workdir` (as [`crate::Repo::workdir`] gives it) is taken to be
/// on a file system like `probe`, on `os` — for repositories opened afterwards. Other
/// work trees (tests running alongside) keep their real probe.
pub fn set_test_probe(workdir: &Path, probe: FsProbe, os: Os) {
    let mut probes = TEST_PROBES.lock().unwrap_or_else(|e| e.into_inner());
    probes.retain(|(p, ..)| p != workdir);
    probes.push((workdir.to_path_buf(), probe, os));
}

fn test_probe(workdir: &Path) -> Option<(FsProbe, Os)> {
    let probes = TEST_PROBES.lock().unwrap_or_else(|e| e.into_inner());
    probes
        .iter()
        .find(|(p, ..)| p == workdir)
        .map(|(_, probe, os)| (*probe, *os))
        .or_else(FsProbe::from_test_env)
}

/// Probes the file system of `dir` in a scratch folder created there and removed again:
/// whether it tells names apart by case and by Unicode form, refuses Windows' characters
/// or device names, and has symbolic links. Fails when the folder can't be created (a
/// read-only place).
pub fn probe(dir: &Path) -> std::io::Result<FsProbe> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let scratch = dir.join(format!(".gitti-fs-probe-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&scratch)?;
    let result = probe_in(&scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn probe_in(d: &Path) -> std::io::Result<FsProbe> {
    let create = |name: &str| std::fs::File::create(d.join(name)).is_ok();
    let exists = |name: &str| std::fs::symlink_metadata(d.join(name)).is_ok();
    std::fs::File::create(d.join("aB"))?;
    let case_insensitive = exists("Ab");
    let unicode_insensitive = create("\u{e9}") && exists("e\u{301}");
    let (refuses_chars, refuses_reserved) = if cfg!(windows) {
        // Opening `aux.x` on Windows opens the device: never probe that.
        (true, true)
    } else {
        (!create("a?b"), !create("aux.x"))
    };
    #[cfg(unix)]
    let symlinks = Some(std::os::unix::fs::symlink("aB", d.join("link")).is_ok());
    #[cfg(not(unix))]
    let symlinks = None;
    Ok(FsProbe {
        case_insensitive,
        unicode_insensitive,
        refuses_chars,
        refuses_reserved,
        symlinks,
    })
}

/// git's settings that decide how paths are checked out (`None`: not set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CoreConfig {
    pub ignore_case: Option<bool>,
    pub precompose_unicode: Option<bool>,
    pub protect_ntfs: Option<bool>,
    pub protect_hfs: Option<bool>,
    pub symlinks: Option<bool>,
    pub long_paths: Option<bool>,
}

/// Why symbolic links are not checked out as links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoSymlinks {
    /// `core.symlinks=false`: checked out as small text files holding the target.
    Config,
    /// The file system has none: git can't create them.
    FileSystem,
}

/// The rules a work tree's paths are checked against ([`check`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FsRules {
    /// Names that differ only in case are one file.
    pub case_insensitive: bool,
    /// Names that differ only in Unicode form (NFC / NFD) are one file.
    pub unicode_insensitive: bool,
    /// git reads names composed (macOS, `core.precomposeunicode`): a decomposed name in
    /// the index shows as deleted.
    pub precompose: bool,
    /// `<>:"|?*\`, control characters and a trailing dot or space can't be used.
    pub windows_chars: bool,
    /// Windows device names can't be used.
    pub windows_reserved: bool,
    /// git refuses NTFS aliases of `.git` (`core.protectNTFS`, on by default).
    pub protect_ntfs: bool,
    /// git refuses HFS aliases of `.git` (`core.protectHFS`, macOS).
    pub protect_hfs: bool,
    /// Symbolic links are not checked out as links.
    pub no_symlinks: Option<NoSymlinks>,
    /// The longest repository-relative path (Windows without `core.longpaths`: what is
    /// left of `MAX_PATH` after the work tree's own path).
    pub max_path: Option<usize>,
    /// Name lengths count UTF-16 units (Windows), else bytes.
    pub utf16: bool,
}

/// The longest file or folder name most file systems take (bytes; UTF-16 units on NTFS).
pub const MAX_NAME: usize = 255;

/// Windows' `MAX_PATH` without the terminating NUL.
const WINDOWS_MAX_PATH: usize = 259;

impl FsRules {
    /// The rules of a work tree at a path `workdir_len` UTF-16 units long, on `os`, whose
    /// file system `probe` found (`None`: couldn't probe — the OS's usual file system and
    /// git's config are assumed).
    pub fn new(os: Os, probe: Option<FsProbe>, cfg: &CoreConfig, workdir_len: usize) -> FsRules {
        let windows = os == Os::Windows;
        let macos = os == Os::MacOs;
        let case_insensitive = match probe {
            Some(p) => p.case_insensitive,
            None => cfg.ignore_case.unwrap_or(windows || macos),
        };
        let unicode_insensitive = probe.map_or(macos, |p| p.unicode_insensitive);
        let no_symlinks = if cfg.symlinks == Some(false) {
            Some(NoSymlinks::Config)
        } else if probe.and_then(|p| p.symlinks) == Some(false) {
            Some(NoSymlinks::FileSystem)
        } else {
            None
        };
        FsRules {
            case_insensitive,
            unicode_insensitive,
            precompose: macos && cfg.precompose_unicode.unwrap_or(true),
            windows_chars: windows || probe.is_some_and(|p| p.refuses_chars),
            windows_reserved: windows || probe.is_some_and(|p| p.refuses_reserved),
            protect_ntfs: cfg.protect_ntfs.unwrap_or(true),
            protect_hfs: cfg.protect_hfs.unwrap_or(macos),
            no_symlinks,
            max_path: (windows && !cfg.long_paths.unwrap_or(false))
                .then(|| WINDOWS_MAX_PATH.saturating_sub(workdir_len + 1)),
            utf16: windows,
        }
    }

    /// Whether names are folded (case or Unicode form): collisions are possible.
    fn folds(&self) -> bool {
        self.case_insensitive || self.unicode_insensitive
    }

    /// The name of this kind of file system in the explanations.
    fn fs_kind(&self) -> &'static str {
        match (self.case_insensitive, self.unicode_insensitive) {
            (true, _) => "a case-insensitive file system",
            (false, true) => "a Unicode-normalizing file system",
            _ => "this file system",
        }
    }
}

/// How two colliding names differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fold {
    Case,
    Unicode,
    Both,
}

/// Why a path can't be checked out as it is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Problem {
    /// Another path of the index names the same file here (`other`; ending in `/`: a
    /// folder this file is in the way of, or that is in the way of it).
    Collides { other: String, by: Fold },
    /// A decomposed (NFD) name while git reads names composed: shows as deleted.
    Decomposed,
    /// A Windows device name (`name`: the file or folder name).
    Reserved { name: String },
    /// A character the file system doesn't take.
    BadChar { ch: char },
    /// A name ending in a dot or a space.
    TrailingDotOrSpace { name: String },
    /// An alias of `.git` git refuses to check out.
    DotGitAlias { name: String },
    /// Longer than the longest path (`max`, Windows without `core.longpaths`).
    TooLong { len: usize, max: usize },
    /// A file or folder name over [`MAX_NAME`].
    NameTooLong { name: String },
    /// A symbolic link that isn't checked out as one.
    Symlink(NoSymlinks),
}

impl Problem {
    /// Whether the path can't be checked out as it is (a symbolic link checked out as a
    /// text file can, sort of: it is only noted on its row).
    pub fn cannot_hold(&self) -> bool {
        !matches!(self, Problem::Symlink(NoSymlinks::Config))
    }

    /// Two paths that are one file here.
    pub fn is_collision(&self) -> bool {
        matches!(self, Problem::Collides { .. })
    }

    /// A few words for the path's row in the file list ("collides with xt_TCPMSS.h").
    pub fn short(&self) -> String {
        match self {
            Problem::Collides { other, by } => {
                let how = match by {
                    Fold::Case => "",
                    Fold::Unicode | Fold::Both => " (Unicode form)",
                };
                if let Some(dir) = other.strip_suffix('/') {
                    format!("collides with folder {}/{how}", name_of(dir))
                } else {
                    format!("collides with {}{how}", name_of(other))
                }
            }
            Problem::Decomposed => "decomposed Unicode name".into(),
            Problem::Reserved { name } => format!("{name}: reserved name on Windows"),
            Problem::BadChar { ch } => format!("{} not allowed in names here", char_label(*ch)),
            Problem::TrailingDotOrSpace { .. } => "name ends in a dot or space".into(),
            Problem::DotGitAlias { name } => format!("{name} is an alias of .git"),
            Problem::TooLong { len, max } => format!("path too long ({len} > {max})"),
            Problem::NameTooLong { .. } => format!("name longer than {MAX_NAME}"),
            Problem::Symlink(NoSymlinks::Config) => "symlink checked out as a text file".into(),
            Problem::Symlink(NoSymlinks::FileSystem) => "no symlinks on this file system".into(),
        }
    }

    /// Why, in a sentence (without the path): "it differs only in case from …".
    fn why(&self, rules: &FsRules) -> String {
        match self {
            Problem::Collides { other, by } => {
                let fs = rules.fs_kind();
                match (other.strip_suffix('/'), by) {
                    (Some(dir), _) => format!(
                        "on {fs} it is the same name as the folder {dir}/ another path needs"
                    ),
                    (None, Fold::Case) => {
                        format!("it differs only in case from {other}: on {fs} both are one file")
                    }
                    (None, Fold::Unicode) => format!(
                        "it differs only in Unicode form (composed / decomposed letters) from \
                         {other}: on {fs} both are one file"
                    ),
                    (None, Fold::Both) => format!(
                        "it differs only in case and Unicode form from {other}: on {fs} both \
                         are one file"
                    ),
                }
            }
            Problem::Decomposed => "its name has decomposed Unicode letters (NFD), but git on \
                 macOS reads names composed (core.precomposeunicode)"
                .into(),
            Problem::Reserved { name } => format!(
                "{name} is a reserved device name on Windows (with any extension, in any case)"
            ),
            Problem::BadChar { ch } => {
                format!("{} can't be used in names on this file system", char_label(*ch))
            }
            Problem::TrailingDotOrSpace { name } => {
                format!("\"{name}\" ends in a dot or a space, which Windows drops")
            }
            Problem::DotGitAlias { name } => format!(
                "{name} is another spelling of .git on Windows / macOS file systems; git refuses \
                 it so that no commit can write into a repository's .git"
            ),
            Problem::TooLong { len, max } => format!(
                "it is {len} characters long and only {max} fit after the repository's folder \
                 (Windows' 260-character limit; core.longpaths is off)"
            ),
            Problem::NameTooLong { name } => format!(
                "the name {}… is longer than the {MAX_NAME} {} a file system allows",
                name.chars().take(24).collect::<String>(),
                if rules.utf16 { "characters" } else { "bytes" }
            ),
            Problem::Symlink(NoSymlinks::Config) => {
                "it is a symbolic link, and core.symlinks is off".into()
            }
            Problem::Symlink(NoSymlinks::FileSystem) => {
                "it is a symbolic link, and this file system has none".into()
            }
        }
    }

    /// The explanation under the path's file header in the diff pane: the reason, what
    /// follows for this file, what to do. One line per row.
    pub fn lines(&self, path: &str, rules: &FsRules) -> Vec<String> {
        let mut out = vec![format!(
            "Can't be checked out on this file system: {}.",
            self.why(rules)
        )];
        match self {
            Problem::Collides { other, .. } if other.ends_with('/') => {
                out.push(
                    "The disk can hold only one of them, so git shows the other as modified or \
                     deleted, and Discard can't change that."
                        .into(),
                );
            }
            Problem::Collides { other, .. } => {
                out.push(format!(
                    "The disk holds one of the two, so the other always shows as modified: \
                     Discard restores {} and overwrites {other}, Stage records the content of \
                     whichever is on disk.",
                    name_of(path)
                ));
            }
            Problem::Decomposed => out.push(
                "It shows as deleted, the composed name as untracked. Rename it in a commit \
                 (composed), or set core.precomposeunicode to false for this repository."
                    .into(),
            ),
            Problem::Symlink(NoSymlinks::Config) => {
                out.push(
                    "It is checked out as a small text file holding the link's target; \
                     changing that text changes the target when committed."
                        .into(),
                );
                return out;
            }
            Problem::TooLong { .. } => out.push(
                "git can't create it (it shows as deleted). Turn long paths on: git config \
                 core.longpaths true (Windows 10 or later)."
                    .into(),
            ),
            _ => out.push(
                "git can't create it, so it shows as deleted (staging that deletes it for \
                 everyone), and checking out a commit that has it fails."
                    .into(),
            ),
        }
        out.push(WHAT_TO_DO.into());
        out
    }
}

/// What can be done about paths a file system can't hold (the last explanation line).
pub const WHAT_TO_DO: &str = "To have them all: work on a file system that takes them (e.g. a \
     case-sensitive volume), or leave them out with git sparse-checkout; or hide them here \
     (skip-worktree: git ignores this copy until shown again).";

fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn char_label(ch: char) -> String {
    if (ch as u32) < 0x20 || ch == '\u{7f}' {
        format!("the control character U+{:04X}", ch as u32)
    } else {
        format!("'{ch}'")
    }
}

/// One index path the file system can't hold.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathIssue {
    pub path: String,
    pub problem: Problem,
    /// git ignores the work tree's copy (skip-worktree: hidden).
    pub hidden: bool,
}

/// One index path to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    pub path: &'a str,
    pub symlink: bool,
    /// skip-worktree is set.
    pub hidden: bool,
}

/// The first problem of `path` that doesn't involve other paths.
pub fn check_path(path: &str, symlink: bool, rules: &FsRules) -> Option<Problem> {
    for name in path.split('/') {
        if let Some(p) = check_name(name, rules) {
            return Some(p);
        }
    }
    if let Some(max) = rules.max_path {
        let len = if rules.utf16 {
            path.encode_utf16().count()
        } else {
            path.chars().count()
        };
        if len > max {
            return Some(Problem::TooLong { len, max });
        }
    }
    if rules.precompose && !path.is_ascii() && path.nfc().ne(path.chars()) {
        return Some(Problem::Decomposed);
    }
    if symlink && let Some(why) = rules.no_symlinks {
        return Some(Problem::Symlink(why));
    }
    None
}

/// The problem of one file or folder name.
fn check_name(name: &str, rules: &FsRules) -> Option<Problem> {
    if (rules.protect_ntfs && is_ntfs_dotgit(name)) || (rules.protect_hfs && is_hfs_dotgit(name)) {
        return Some(Problem::DotGitAlias { name: name.to_string() });
    }
    if rules.windows_chars {
        if let Some(ch) = name
            .chars()
            .find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\') || (*c as u32) < 0x20)
        {
            return Some(Problem::BadChar { ch });
        }
        if name.ends_with(['.', ' ']) && name != "." && name != ".." {
            return Some(Problem::TrailingDotOrSpace { name: name.to_string() });
        }
    }
    if rules.windows_reserved && is_reserved(name) {
        return Some(Problem::Reserved { name: name.to_string() });
    }
    let len = if rules.utf16 {
        name.encode_utf16().count()
    } else {
        name.len()
    };
    (len > MAX_NAME).then(|| Problem::NameTooLong { name: name.to_string() })
}

/// A Windows device name: the part before the first dot or colon (trailing spaces
/// ignored) is `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM1`–`COM9` or
/// `LPT1`–`LPT9` (also `¹²³`), in any case.
pub fn is_reserved(name: &str) -> bool {
    let base = name.split(['.', ':']).next().unwrap_or("").trim_end_matches(' ');
    let upper = base.to_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            let Some(rest) = upper.strip_prefix("COM").or_else(|| upper.strip_prefix("LPT")) else {
                return false;
            };
            let mut chars = rest.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some('1'..='9' | '¹' | '²' | '³'), None)
            )
        }
    }
}

/// git's `is_ntfs_dotgit`: `.git` followed by dots / spaces, `git~1`, `.git::$INDEX_ALLOCATION`.
/// `.git` itself is never in a tree (git refuses it on every system).
fn is_ntfs_dotgit(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower == "git~1" || lower.starts_with(".git::$index_allocation") {
        return true;
    }
    lower.len() > 4 && lower.starts_with(".git") && lower[4..].chars().all(|c| c == '.' || c == ' ')
}

/// git's `is_hfs_dotgit`: `.git` with code points HFS+ ignores in between.
fn is_hfs_dotgit(name: &str) -> bool {
    let ignorable = |c: char| {
        matches!(c as u32, 0x200C..=0x200F | 0x202A..=0x202E | 0x206A..=0x206F | 0xFEFF)
    };
    if !name.chars().any(ignorable) {
        return false;
    }
    let kept: String = name.chars().filter(|c| !ignorable(*c)).collect();
    kept.eq_ignore_ascii_case(".git")
}

/// The name a folding file system sees.
fn fold(path: &str, rules: &FsRules) -> String {
    if path.is_ascii() {
        return if rules.case_insensitive {
            path.to_ascii_lowercase()
        } else {
            path.to_string()
        };
    }
    let s: String = if rules.unicode_insensitive {
        path.nfc().collect()
    } else {
        path.to_string()
    };
    if rules.case_insensitive { s.to_lowercase() } else { s }
}

/// How `a` and `b` (one file when folded) differ.
fn how(a: &str, b: &str) -> Fold {
    if a.nfc().eq(b.nfc()) {
        Fold::Unicode
    } else if a.to_lowercase() == b.to_lowercase() {
        Fold::Case
    } else {
        Fold::Both
    }
}

/// The paths among `entries` (an index or a tree: unique paths) the file system can't
/// hold, sorted by path; per path its first problem (its own name before collisions).
pub fn check(entries: &[Entry<'_>], rules: &FsRules) -> Vec<PathIssue> {
    let mut found: HashMap<usize, Problem> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if let Some(p) = check_path(e.path, e.symlink, rules)
            && p.cannot_hold()
        {
            found.insert(i, p);
        }
    }
    if rules.folds() {
        let keys: Vec<String> = entries.iter().map(|e| fold(e.path, rules)).collect();
        let mut by_key: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, k) in keys.iter().enumerate() {
            by_key.entry(k.as_str()).or_default().push(i);
        }
        for group in by_key.values().filter(|g| g.len() > 1) {
            for &i in group {
                let o = *group.iter().find(|&&o| o != i).expect("a group of two or more");
                let (a, b) = (entries[i].path, entries[o].path);
                found.entry(i).or_insert(Problem::Collides {
                    other: b.to_string(),
                    by: how(a, b),
                });
            }
        }
        // A file where another path has a folder: `docs` and `Docs/a.md`.
        let mut sorted: Vec<usize> = (0..entries.len()).collect();
        sorted.sort_by(|&a, &b| keys[a].cmp(&keys[b]));
        for (i, key) in keys.iter().enumerate() {
            let dir = format!("{key}/");
            let at = sorted.partition_point(|&j| keys[j].as_str() < dir.as_str());
            let Some(&j) = sorted.get(at).filter(|&&j| keys[j].starts_with(&dir)) else {
                continue;
            };
            let depth = entries[i].path.matches('/').count() + 1;
            let folder: Vec<&str> = entries[j].path.split('/').take(depth).collect();
            let folder = folder.join("/");
            let by = how(entries[i].path, &folder);
            found.entry(i).or_insert(Problem::Collides {
                other: format!("{folder}/"),
                by,
            });
            found.entry(j).or_insert(Problem::Collides {
                other: entries[i].path.to_string(),
                by,
            });
        }
    }
    let mut out: Vec<PathIssue> = found
        .into_iter()
        .map(|(i, problem)| PathIssue {
            path: entries[i].path.to_string(),
            problem,
            hidden: entries[i].hidden,
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The paths of an index the file system can't hold ([`crate::Repo::fs_paths`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathReport {
    pub rules: FsRules,
    /// Sorted by path.
    pub issues: Vec<PathIssue>,
}

impl PathReport {
    pub fn new(rules: FsRules, issues: Vec<PathIssue>) -> Self {
        Self { rules, issues }
    }

    /// The problem of `path` (`None`: fine).
    pub fn issue(&self, path: &str) -> Option<&PathIssue> {
        self.issues
            .binary_search_by(|i| i.path.as_str().cmp(path))
            .ok()
            .map(|ix| &self.issues[ix])
    }

    /// Paths that can't be checked out as they are (not the symbolic links checked out as
    /// text files).
    pub fn cannot_hold(&self) -> impl Iterator<Item = &PathIssue> {
        self.issues.iter().filter(|i| i.problem.cannot_hold())
    }

    /// Those of [`Self::cannot_hold`] still in the way: not hidden, and not a collision
    /// whose partner is hidden (then this one is the copy git looks at, and it is fine).
    pub fn shown(&self) -> impl Iterator<Item = &PathIssue> {
        self.cannot_hold().filter(|i| {
            !i.hidden
                && !matches!(&i.problem, Problem::Collides { other, .. }
                    if self.issue(other).is_some_and(|o| o.hidden))
        })
    }

    /// The paths "Hide Them" hides: those of [`Self::shown`] that show as changed in the
    /// working directory (`ws`) — one of each colliding pair (the other one is what git
    /// keeps looking at), the missing ones.
    pub fn hidable(&self, ws: &crate::WorkingState) -> Vec<String> {
        if self.issues.is_empty() {
            return Vec::new();
        }
        let shown: std::collections::HashSet<&str> = self.shown().map(|i| i.path.as_str()).collect();
        let mut out: Vec<String> = Vec::new();
        for f in ws.unstaged.iter().filter(|f| !f.entry.untracked && !f.entry.conflicted) {
            let Some(issue) = self.issue(f.path()).filter(|i| shown.contains(i.path.as_str())) else {
                continue;
            };
            // Of a pair that both show as changed, one is enough.
            if let Problem::Collides { other, .. } = &issue.problem
                && out.contains(other)
            {
                continue;
            }
            out.push(f.path().to_string());
        }
        out
    }

    /// Those hidden with skip-worktree (what "Show Hidden Paths Again" brings back).
    pub fn hidden(&self) -> impl Iterator<Item = &PathIssue> {
        self.cannot_hold().filter(|i| i.hidden)
    }

    /// The repository's note while some paths can't be checked out (and aren't hidden):
    /// how many, why, what follows, what to do.
    pub fn note(&self) -> Option<String> {
        let shown: Vec<&PathIssue> = self.shown().collect();
        if shown.is_empty() {
            return None;
        }
        Some(format!(
            "{} can't be checked out on this file system{}. {} {WHAT_TO_DO}",
            crate::discard::count(shown.len(), "path"),
            kinds_summary(&shown),
            consequence(&shown),
        ))
    }

    /// The note in a few words (gitty's toolbar badge).
    pub fn short_note(&self) -> Option<String> {
        let n = self.shown().count();
        (n > 0).then(|| {
            format!(
                "{} can't be checked out here",
                crate::discard::count(n, "path")
            )
        })
    }

    /// What changed from `before` to this report: the paths that can't be held now and
    /// couldn't before (e.g. after a checkout).
    pub fn new_since<'a>(&'a self, before: &PathReport) -> Vec<&'a PathIssue> {
        self.cannot_hold()
            .filter(|i| before.issue(&i.path).is_none_or(|b| !b.problem.cannot_hold()))
            .collect()
    }
}

/// ": 12 differ only in case from another path, 1 is a reserved name on Windows (e.g. …)".
fn kinds_summary(issues: &[&PathIssue]) -> String {
    let mut kinds: Vec<(&'static str, usize, &str)> = Vec::new();
    for i in issues {
        let kind = match &i.problem {
            Problem::Collides { by: Fold::Case, .. } => "differ only in case from another path",
            Problem::Collides { .. } => "differ only in case or Unicode form from another path",
            Problem::Decomposed => "have decomposed Unicode names",
            Problem::Reserved { .. } => "are reserved names on Windows",
            Problem::BadChar { .. } | Problem::TrailingDotOrSpace { .. } => {
                "have characters this file system doesn't take"
            }
            Problem::DotGitAlias { .. } => "are aliases of .git",
            Problem::TooLong { .. } | Problem::NameTooLong { .. } => "are too long",
            Problem::Symlink(_) => "are symbolic links",
        };
        match kinds.iter_mut().find(|k| k.0 == kind) {
            Some(k) => k.1 += 1,
            None => kinds.push((kind, 1, &i.path)),
        }
    }
    let parts: Vec<String> = kinds
        .iter()
        .map(|(kind, n, example)| {
            let kind = if *n == 1 {
                kind.replacen("differ", "differs", 1)
                    .replacen("have ", "has ", 1)
                    .replacen("are ", "is ", 1)
                    .replace("names", "name")
                    .replace("aliases", "an alias")
                    .replace("links", "link")
                    .replace("is reserved name", "is a reserved name")
                    .replace("is symbolic link", "is a symbolic link")
                    .replace("has decomposed Unicode name", "has a decomposed Unicode name")
            } else {
                kind.to_string()
            };
            format!("{n} {kind} (e.g. {example})")
        })
        .collect();
    format!(": {}", parts.join(", "))
}

/// What the user sees because of `issues`.
fn consequence(issues: &[&PathIssue]) -> &'static str {
    if issues.iter().all(|i| i.problem.is_collision()) {
        "The disk holds one file for each colliding group, so the others always show as \
         modified, and Discard can't change that."
    } else {
        "They show as modified or deleted, Discard can't change that, and checking out a \
         commit that has such paths may fail."
    }
}

/// Why staging or discarding some lines of `issue`'s file is refused (`None`: it isn't
/// — unstaging only touches the index).
pub fn partial_refusal(issue: &PathIssue, mode: crate::patch::PatchMode) -> Option<String> {
    if !issue.problem.cannot_hold() || mode == crate::patch::PatchMode::Unstage {
        return None;
    }
    let what = match mode {
        crate::patch::PatchMode::Discard => "discard",
        _ => "stage",
    };
    Some(format!(
        "Can't {what} lines of {}: {} on this file system, so its lines on disk aren't its own. \
         Stage or discard the whole file (it says what follows), or hide it (skip-worktree).",
        issue.path,
        issue.problem.short()
    ))
}

/// The question before staging `issues` (files the file system can't hold): title and
/// text. What shows as their change comes from the file system, not from the user.
pub fn stage_question(issues: &[&PathIssue]) -> (String, String) {
    match issues {
        [one] => {
            let follows = match &one.problem {
                Problem::Collides { other, .. } if !other.ends_with('/') => format!(
                    "It collides with {other} on this file system (one file here): what is on \
                     disk is the content of whichever was written last, so staging records \
                     that content under {} — a commit would change it for everyone.",
                    one.path
                ),
                _ => format!(
                    "This file system can't hold it ({}): staging records it as deleted (or \
                     with what the file system made of it) — a commit would change it for \
                     everyone.",
                    one.problem.short()
                ),
            };
            (
                format!("Stage {}?", one.path),
                format!("{follows} Hide it instead (skip-worktree) if you didn't change it."),
            )
        }
        _ => {
            let examples: Vec<&str> = issues.iter().take(3).map(|i| i.path.as_str()).collect();
            (
                format!(
                    "Stage {} this file system can't hold?",
                    crate::discard::count(issues.len(), "file")
                ),
                format!(
                    "{}{} can't be held on this file system as they are (names that collide, \
                     or that it doesn't take): what shows as their change comes from the file \
                     system, not from you. Staging them records it — a commit would change or \
                     delete them for everyone. Without them, the other files are staged.",
                    examples.join(", "),
                    if issues.len() > examples.len() { ", …" } else { "" },
                ),
            )
        }
    }
}

/// The check box of [`stage_question`] for several files.
pub const STAGE_THEM_TOO: &str = "Stage these files too";

/// What a discard of `issues` (among the files discarded) can't do, for its question
/// (`None`: no such files).
pub fn discard_warning(issues: &[&PathIssue]) -> Option<String> {
    match issues {
        [] => None,
        [one] => Some(match &one.problem {
            Problem::Collides { other, .. } if !other.ends_with('/') => format!(
                "{} collides with {other} on this file system (one file here): Discard \
                 restores {} and thereby overwrites {other}, which then shows as modified \
                 instead. Hide one of them (skip-worktree) to stop this.",
                one.path, one.path
            ),
            _ => format!(
                "{} can't be held on this file system ({}): it still shows as changed after \
                 Discard. Hide it (skip-worktree) to stop this.",
                one.path,
                one.problem.short()
            ),
        }),
        _ => Some(format!(
            "{} of these can't be held on this file system: they still show as changed after \
             Discard (restoring one of a colliding pair overwrites the other). Hide them \
             (skip-worktree) to stop this — see the note above the diff.",
            crate::discard::count(issues.len(), "file")
        )),
    }
}

/// The status line's note after an operation brought `new` paths the file system can't
/// hold into the index (`None`: none).
pub fn change_note(new: &[&PathIssue]) -> Option<String> {
    if new.is_empty() {
        return None;
    }
    let examples: Vec<String> = new
        .iter()
        .take(3)
        .map(|i| format!("{} ({})", i.path, i.problem.short()))
        .collect();
    let more = new.len().saturating_sub(3);
    Some(format!(
        "{} of this checkout can't be held on this file system{}: {}{} — they show as \
         modified or deleted; see the note above the diff.",
        crate::discard::count(new.len(), "path"),
        if new.iter().all(|i| i.problem.is_collision()) {
            " (names that differ only in case)"
        } else {
            ""
        },
        examples.join(", "),
        if more > 0 { format!(" and {more} more") } else { String::new() },
    ))
}

/// The paths git's error output names as ones it couldn't check out: `invalid path
/// 'x'`, `unable to create file x: …`, `unable to create symlink x: …`, in order, once
/// each, with git's reason when it gave one.
pub fn failed_paths(stderr: &str) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for line in stderr.lines() {
        let line = line.trim();
        let found = if let Some(at) = line.find("invalid path '") {
            let rest = &line[at + "invalid path '".len()..];
            rest.rfind('\'').map(|end| (rest[..end].to_string(), None))
        } else {
            ["unable to create file ", "unable to create symlink "]
                .iter()
                .find_map(|lead| line.find(lead).map(|at| &line[at + lead.len()..]))
                .map(|rest| match rest.rsplit_once(": ") {
                    Some((path, why)) => (path.to_string(), Some(why.to_string())),
                    None => (rest.to_string(), None),
                })
        };
        if let Some((path, why)) = found
            && !out.iter().any(|(p, _)| *p == path)
        {
            out.push((path, why));
        }
    }
    out
}

/// The paths of git's "the following paths have collided" warning (clone / checkout on a
/// case-insensitive file system).
pub fn collided_paths(stderr: &str) -> Vec<String> {
    let Some(at) = stderr.find("paths have collided") else {
        return Vec::new();
    };
    stderr[at..]
        .lines()
        .skip(1)
        .map(str::trim)
        .skip_while(|l| !l.starts_with('\''))
        .take_while(|l| l.starts_with('\''))
        .map(|l| l.trim_matches('\'').to_string())
        .collect()
}

/// git's error about paths it couldn't check out, in plain words: which, why on this file
/// system, what to do (`None`: the error isn't about such paths).
pub fn explain_git_error(error: &str, rules: &FsRules) -> Option<String> {
    let failed = failed_paths(error);
    if failed.is_empty() {
        return None;
    }
    let lines: Vec<String> = failed
        .iter()
        .take(8)
        .map(|(path, why)| {
            let reason = check_path(path, false, rules)
                .map(|p| p.why(rules))
                .or_else(|| why.clone())
                .unwrap_or_else(|| "git refuses this path".into());
            format!("  {path} — {reason}")
        })
        .collect();
    let more = failed.len().saturating_sub(8);
    Some(format!(
        "git can't check out {} on this file system:\n{}{}\n\n{WHAT_TO_DO}",
        crate::discard::count(failed.len(), "path"),
        lines.join("\n"),
        if more > 0 { format!("\n  … and {more} more") } else { String::new() },
    ))
}

/// A clone's note from git's output: the paths that collided, in plain words.
pub fn clone_note(stderr: &str) -> Option<String> {
    let collided = collided_paths(stderr);
    if collided.is_empty() {
        return None;
    }
    let examples: Vec<&str> = collided.iter().take(4).map(String::as_str).collect();
    Some(format!(
        "Cloned — but {} collided: their names differ only in case (or Unicode form), and \
         this file system keeps one file for each group ({}{}). The others show as modified \
         right away; that is no change of yours. {WHAT_TO_DO}",
        crate::discard::count(collided.len(), "path"),
        examples.join(", "),
        if collided.len() > examples.len() { ", …" } else { "" },
    ))
}

impl crate::Repo {
    /// The rules of this work tree's paths: its file system (probed once per open, see
    /// [`probe`]) and git's config (read each time, in-process).
    pub fn fs_rules(&self) -> FsRules {
        let (probe_cell, _) = self.statics_fs_paths();
        let (probe, os) = *probe_cell.get_or_init(|| match test_probe(self.workdir()) {
            Some((p, os)) => (Some(p), os),
            None => (probe(&self.probe_dir()).ok(), Os::current()),
        });
        let cfg = self.core_config();
        let len = self.workdir().to_string_lossy().encode_utf16().count();
        FsRules::new(os, probe, &cfg, len)
    }

    /// Where to probe the work tree's file system without the file watcher seeing it: the
    /// git dir when it is on the same file system, else the work tree itself.
    fn probe_dir(&self) -> std::path::PathBuf {
        let wd = self.workdir();
        match self.git_dir() {
            Some(g) if g.starts_with(wd) || same_device(&g, wd) => g,
            _ => wd.to_path_buf(),
        }
    }

    fn core_config(&self) -> CoreConfig {
        let Ok(repo) = self.gix() else {
            return CoreConfig::default();
        };
        let c = repo.config_snapshot();
        let b = |key: &str| c.boolean(key);
        CoreConfig {
            ignore_case: b("core.ignoreCase"),
            precompose_unicode: b("core.precomposeUnicode"),
            protect_ntfs: b("core.protectNTFS"),
            protect_hfs: b("core.protectHFS"),
            symlinks: b("core.symlinks"),
            long_paths: b("core.longPaths"),
        }
    }

    /// The index paths this work tree can't hold, kept while the index and the config
    /// files are unchanged (in-process; empty when the index can't be read).
    pub fn fs_paths(&self) -> std::sync::Arc<PathReport> {
        let Some(git_dir) = self.git_dir() else {
            return Default::default();
        };
        let mut files = vec![git_dir.join("index")];
        files.extend(self.config_files().iter().cloned());
        let (_, cache) = self.statics_fs_paths();
        cache.get(&files, || self.fs_paths_now()).unwrap_or_default()
    }

    /// [`Self::fs_paths`] read now (after an operation wrote the index).
    pub fn fs_paths_now(&self) -> anyhow::Result<std::sync::Arc<PathReport>> {
        use gix::index::entry::{Flags, Mode};
        let rules = self.fs_rules();
        let repo = self.gix()?;
        let index = repo.index_or_empty().map_err(crate::repo::gx)?;
        let mut paths: Vec<(String, bool, bool)> = Vec::with_capacity(index.entries().len());
        for e in index.entries() {
            let path = String::from_utf8_lossy(e.path(&index));
            // Conflicted paths come once per stage.
            if paths.last().is_some_and(|(p, ..)| *p == *path) {
                continue;
            }
            paths.push((
                path.into_owned(),
                e.mode == Mode::SYMLINK,
                e.flags.contains(Flags::SKIP_WORKTREE),
            ));
        }
        let entries: Vec<Entry<'_>> = paths
            .iter()
            .map(|(path, symlink, hidden)| Entry { path, symlink: *symlink, hidden: *hidden })
            .collect();
        let mut issues = check(&entries, &rules);
        // Symbolic links checked out as text files: a note on their rows only.
        if rules.no_symlinks == Some(NoSymlinks::Config) {
            let links: Vec<PathIssue> = entries
                .iter()
                .filter(|e| e.symlink && !issues.iter().any(|i| i.path == e.path))
                .map(|e| PathIssue {
                    path: e.path.to_string(),
                    problem: Problem::Symlink(NoSymlinks::Config),
                    hidden: e.hidden,
                })
                .collect();
            issues.extend(links);
            issues.sort_by(|a, b| a.path.cmp(&b.path));
        }
        Ok(std::sync::Arc::new(PathReport::new(rules, issues)))
    }

    /// Runs `op` (a checkout, reset, cherry-pick, merge, …) and says which paths it brought
    /// into the index that this file system can't hold (the result is that note instead
    /// of git's output); git's error about paths it couldn't check out comes back in plain
    /// words ([`explain_git_error`]).
    pub fn noting_fs_paths(
        &self,
        op: impl FnOnce() -> anyhow::Result<String>,
    ) -> anyhow::Result<String> {
        let before = self.fs_paths();
        match op() {
            Ok(out) => Ok(self.fs_change_note(&before).unwrap_or(out)),
            Err(e) => Err(self.explain_fs_error(e)),
        }
    }

    /// The note about paths the index has now and `before` didn't (see [`change_note`]).
    pub(crate) fn fs_change_note(&self, before: &PathReport) -> Option<String> {
        let after = self.fs_paths_now().ok()?;
        change_note(&after.new_since(before))
    }

    /// `e` with git's complaint about paths put in plain words first (unchanged when it
    /// isn't about such paths).
    pub(crate) fn explain_fs_error(&self, e: anyhow::Error) -> anyhow::Error {
        match explain_git_error(&format!("{e:#}"), &self.fs_rules()) {
            Some(text) => anyhow::anyhow!("{text}\n\ngit said: {e:#}"),
            None => e,
        }
    }

    /// Hides `paths` (paths this file system can't hold) from git's view of the work
    /// tree: `git update-index --skip-worktree`. git then ignores their copies here — no
    /// longer "modified" or "deleted" — until [`Self::show_hidden_fs_paths`].
    pub fn hide_fs_paths(&self, paths: &[String]) -> anyhow::Result<String> {
        if paths.is_empty() {
            return Ok("Nothing to hide".into());
        }
        self.update_index_flag("--skip-worktree", paths)?;
        Ok(format!(
            "Hid {} (skip-worktree) — git ignores their copies here until you show them again",
            crate::discard::count(paths.len(), "path")
        ))
    }

    /// Undoes [`Self::hide_fs_paths`]: every hidden path this file system can't hold is
    /// shown again (`--no-skip-worktree`).
    pub fn show_hidden_fs_paths(&self) -> anyhow::Result<String> {
        let hidden: Vec<String> = self
            .fs_paths_now()?
            .hidden()
            .map(|i| i.path.clone())
            .collect();
        if hidden.is_empty() {
            return Ok("No hidden paths".into());
        }
        self.update_index_flag("--no-skip-worktree", &hidden)?;
        Ok(format!(
            "{} shown again — they show as modified or deleted while this file system can't hold them",
            crate::discard::count(hidden.len(), "path")
        ))
    }

    fn update_index_flag(&self, flag: &str, paths: &[String]) -> anyhow::Result<()> {
        let mut input = Vec::new();
        for p in paths {
            input.extend_from_slice(p.as_bytes());
            input.push(0);
        }
        self.git()
            .run_stdin(&["update-index", flag, "-z", "--stdin"], Some(&input))?;
        Ok(())
    }
}

/// Whether `a` and `b` are on the same file system (unix: the same device).
fn same_device(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(f: impl FnOnce(&mut FsRules)) -> FsRules {
        let mut r = FsRules::default();
        f(&mut r);
        r
    }

    fn entries<'a>(paths: &[&'a str]) -> Vec<Entry<'a>> {
        paths
            .iter()
            .map(|p| Entry { path: p, symlink: false, hidden: false })
            .collect()
    }

    fn issues(paths: &[&str], r: &FsRules) -> Vec<(String, Problem)> {
        check(&entries(paths), r)
            .into_iter()
            .map(|i| (i.path, i.problem))
            .collect()
    }

    #[gitti_test::test]
    fn case_pairs_collide_only_on_a_case_insensitive_file_system() {
        let paths = ["net/xt_TCPMSS.h", "net/xt_tcpmss.h", "net/other.h"];
        assert!(issues(&paths, &FsRules::default()).is_empty());
        let r = rules(|r| r.case_insensitive = true);
        assert_eq!(
            issues(&paths, &r),
            vec![
                (
                    "net/xt_TCPMSS.h".into(),
                    Problem::Collides { other: "net/xt_tcpmss.h".into(), by: Fold::Case }
                ),
                (
                    "net/xt_tcpmss.h".into(),
                    Problem::Collides { other: "net/xt_TCPMSS.h".into(), by: Fold::Case }
                ),
            ]
        );
    }

    #[gitti_test::test]
    fn folders_that_differ_in_case_collide_with_their_files() {
        let r = rules(|r| r.case_insensitive = true);
        // Two folders that are one: only files at the same place collide.
        let got = issues(&["Include/linux/a.h", "include/Linux/a.h", "include/linux/b.h"], &r);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.iter().all(|(p, _)| p.ends_with("a.h")));
        // A file where another path needs a folder.
        let got = issues(&["docs", "Docs/a.md", "x"], &r);
        assert_eq!(
            got,
            vec![
                ("Docs/a.md".into(), Problem::Collides { other: "docs".into(), by: Fold::Case }),
                ("docs".into(), Problem::Collides { other: "Docs/".into(), by: Fold::Case }),
            ]
        );
        assert_eq!(got[1].1.short(), "collides with folder Docs/");
    }

    #[gitti_test::test]
    fn nfc_and_nfd_names_collide_on_a_normalizing_file_system() {
        let (nfc, nfd) = ("caf\u{e9}.txt", "cafe\u{301}.txt");
        assert!(issues(&[nfc, nfd], &rules(|r| r.case_insensitive = true)).is_empty());
        let r = rules(|r| r.unicode_insensitive = true);
        let got = issues(&[nfc, nfd], &r);
        assert_eq!(got.len(), 2);
        // Sorted by path: the decomposed one (`e` + accent) first.
        assert_eq!(got[0], (nfd.into(), Problem::Collides { other: nfc.into(), by: Fold::Unicode }));
        // Case and form at once.
        let r = rules(|r| {
            r.unicode_insensitive = true;
            r.case_insensitive = true;
        });
        let got = issues(&["CAF\u{c9}", "cafe\u{301}"], &r);
        assert!(matches!(got[0].1, Problem::Collides { by: Fold::Both, .. }), "{got:?}");
        // A decomposed name alone, while git reads names composed.
        let r = rules(|r| r.precompose = true);
        assert_eq!(issues(&[nfd, nfc.trim_end_matches(".txt")], &r), vec![(nfd.into(), Problem::Decomposed)]);
    }

    #[gitti_test::test]
    fn windows_device_names_in_any_case_with_any_extension() {
        for name in ["aux", "AUX.c", "Con.txt", "nul", "com1", "LPT9.log", "com\u{b9}", "aux .h", "conin$"] {
            assert!(is_reserved(name), "{name}");
        }
        for name in ["auxiliary.c", "com0", "com10", "lpt", "console", "x.aux", "prn_x"] {
            assert!(!is_reserved(name), "{name}");
        }
        let r = rules(|r| r.windows_reserved = true);
        let got = issues(&["drivers/i2c/aux.c", "include/soc/arc/aux.h", "AUX/x", "fine.c"], &r);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].1, Problem::Reserved { name: "AUX".into() });
        assert_eq!(got[1].1.short(), "aux.c: reserved name on Windows");
        assert!(issues(&["drivers/i2c/aux.c"], &FsRules::default()).is_empty());
    }

    #[gitti_test::test]
    fn windows_characters_and_trailing_dots() {
        let r = rules(|r| r.windows_chars = true);
        let got = issues(&["a:b", "q?", "x\\y", "tab\there", "dir./f", "end ", "ok.txt", "a/.b"], &r);
        let kinds: Vec<&str> = got
            .iter()
            .map(|(p, k)| match k {
                Problem::BadChar { .. } => "char",
                Problem::TrailingDotOrSpace { .. } => "trail",
                _ => panic!("{p}: {k:?}"),
            })
            .collect();
        assert_eq!(got.len(), 6, "{got:?}");
        assert_eq!(kinds.iter().filter(|k| **k == "trail").count(), 2);
        assert!(Problem::BadChar { ch: '\t' }.short().contains("U+0009"));
    }

    #[gitti_test::test]
    fn aliases_of_dot_git() {
        let r = rules(|r| r.protect_ntfs = true);
        for p in ["GIT~1/config", "x/.git./hooks", ".GIT /a", ".git::$INDEX_ALLOCATION/x"] {
            assert!(matches!(check_path(p, false, &r), Some(Problem::DotGitAlias { .. })), "{p}");
        }
        assert_eq!(check_path(".gitignore", false, &r), None);
        assert_eq!(check_path(".github/x", false, &r), None);
        let hfs = rules(|r| r.protect_hfs = true);
        assert!(matches!(check_path(".g\u{200c}it/config", false, &hfs), Some(Problem::DotGitAlias { .. })));
        assert_eq!(check_path(".g\u{200c}it/config", false, &r), None);
    }

    #[gitti_test::test]
    fn long_paths_and_long_names() {
        let os_rules = FsRules::new(Os::Windows, None, &CoreConfig::default(), 40);
        assert_eq!(os_rules.max_path, Some(259 - 41));
        let long = format!("dir/{}", "x".repeat(230));
        assert!(matches!(check_path(&long, false, &os_rules), Some(Problem::TooLong { len: 234, max: 218 })));
        let on = CoreConfig { long_paths: Some(true), ..Default::default() };
        let r = FsRules::new(Os::Windows, None, &on, 40);
        assert_eq!(check_path(&long, false, &r), None);
        let name = "n".repeat(256);
        assert!(matches!(
            check_path(&format!("a/{name}"), false, &FsRules::default()),
            Some(Problem::NameTooLong { .. })
        ));
        // 128 × 'é' is 256 bytes but 128 UTF-16 units.
        let e = "\u{e9}".repeat(128);
        assert!(check_path(&e, false, &FsRules::default()).is_some());
        assert!(check_path(&e, false, &rules(|r| r.utf16 = true)).is_none());
    }

    #[gitti_test::test]
    fn symlinks_without_symlink_support() {
        let r = rules(|r| r.no_symlinks = Some(NoSymlinks::Config));
        let e = [Entry { path: "link", symlink: true, hidden: false }];
        // Only a note on its row: not a path the file system can't hold.
        assert!(check(&e, &r).is_empty());
        assert_eq!(check_path("link", true, &r), Some(Problem::Symlink(NoSymlinks::Config)));
        let r = rules(|r| r.no_symlinks = Some(NoSymlinks::FileSystem));
        assert_eq!(check(&e, &r).len(), 1);
    }

    #[gitti_test::test]
    fn rules_follow_the_probe_before_the_os() {
        let cfg = CoreConfig::default();
        // A case-sensitive APFS volume on the Mac.
        let sensitive = FsProbe { symlinks: Some(true), ..Default::default() };
        let r = FsRules::new(Os::MacOs, Some(sensitive), &cfg, 10);
        assert!(!r.case_insensitive && !r.unicode_insensitive);
        assert!(r.precompose && r.protect_hfs && r.protect_ntfs);
        // A FAT stick on Linux.
        let fat = FsProbe {
            case_insensitive: true,
            refuses_chars: true,
            refuses_reserved: false,
            symlinks: Some(false),
            ..Default::default()
        };
        let r = FsRules::new(Os::Other, Some(fat), &cfg, 10);
        assert!(r.case_insensitive && r.windows_chars && !r.windows_reserved);
        assert_eq!(r.no_symlinks, Some(NoSymlinks::FileSystem));
        assert!(!r.protect_hfs && !r.precompose && r.max_path.is_none());
        // No probe: the OS's usual file system, git's config.
        let r = FsRules::new(Os::Other, None, &CoreConfig { ignore_case: Some(true), symlinks: Some(false), ..cfg }, 10);
        assert!(r.case_insensitive);
        assert_eq!(r.no_symlinks, Some(NoSymlinks::Config));
        let r = FsRules::new(Os::Windows, None, &cfg, 10);
        assert!(r.case_insensitive && r.windows_chars && r.windows_reserved && r.utf16);
    }

    #[gitti_test::test]
    fn the_report_counts_explains_and_compares() {
        let r = rules(|r| {
            r.case_insensitive = true;
            r.windows_reserved = true;
        });
        let mut e = entries(&["a/X.h", "a/x.h", "aux.c", "con", "fine"]);
        e[3].hidden = true;
        let report = PathReport::new(r, check(&e, &r));
        assert_eq!(report.issues.len(), 4);
        assert_eq!(report.shown().count(), 3);
        assert_eq!(report.hidden().count(), 1);
        assert!(report.issue("fine").is_none());
        assert!(report.issue("a/x.h").unwrap().problem.is_collision());
        let note = report.note().unwrap();
        assert!(note.starts_with("3 paths can't be checked out on this file system: 2 differ only in case from another path (e.g. a/X.h), 1 is a reserved name on Windows (e.g. aux.c)."), "{note}");
        assert_eq!(report.short_note().unwrap(), "3 paths can't be checked out here");
        let before = PathReport::new(r, check(&entries(&["aux.c", "con"]), &r));
        let new = report.new_since(&before);
        assert_eq!(new.len(), 2);
        let line = change_note(&new).unwrap();
        assert!(line.starts_with("2 paths of this checkout can't be held on this file system (names that differ only in case): a/X.h (collides with x.h), a/x.h (collides with X.h)"), "{line}");
        let lines = report.issue("a/x.h").unwrap().problem.lines("a/x.h", &r);
        assert!(lines[1].contains("Discard restores x.h and overwrites a/X.h"), "{lines:?}");
        assert_eq!(lines.last().unwrap(), WHAT_TO_DO);
    }

    #[gitti_test::test]
    fn hiding_one_of_a_pair_settles_both() {
        let r = rules(|r| r.case_insensitive = true);
        let mut e = entries(&["X.h", "x.h"]);
        assert_eq!(PathReport::new(r, check(&e, &r)).shown().count(), 2);
        e[1].hidden = true;
        let report = PathReport::new(r, check(&e, &r));
        assert_eq!(report.shown().count(), 0);
        assert_eq!(report.note(), None);
        assert_eq!(report.hidden().count(), 1);
    }

    #[gitti_test::test]
    fn what_stage_and_discard_say() {
        use crate::patch::PatchMode;
        let issue = PathIssue {
            path: "net/xt_tcpmss.h".into(),
            problem: Problem::Collides { other: "net/xt_TCPMSS.h".into(), by: Fold::Case },
            hidden: false,
        };
        assert_eq!(partial_refusal(&issue, PatchMode::Unstage), None);
        let text = partial_refusal(&issue, PatchMode::Stage).unwrap();
        assert!(text.starts_with("Can't stage lines of net/xt_tcpmss.h: collides with xt_TCPMSS.h"), "{text}");
        let (title, text) = stage_question(&[&issue]);
        assert_eq!(title, "Stage net/xt_tcpmss.h?");
        assert!(text.contains("records that content under net/xt_tcpmss.h"), "{text}");
        let (title, _) = stage_question(&[&issue, &issue]);
        assert_eq!(title, "Stage 2 files this file system can't hold?");
        let d = discard_warning(&[&issue]).unwrap();
        assert!(d.contains("Discard restores net/xt_tcpmss.h and thereby overwrites net/xt_TCPMSS.h"), "{d}");
        assert_eq!(discard_warning(&[]), None);
    }

    #[gitti_test::test]
    fn git_errors_in_plain_words() {
        let stderr = "error: invalid path 'drivers/i2c/aux.c'\n\
            error: unable to create file a/b?: Invalid argument\n\
            error: invalid path 'drivers/i2c/aux.c'\n";
        let failed = failed_paths(stderr);
        assert_eq!(
            failed,
            vec![
                ("drivers/i2c/aux.c".into(), None),
                ("a/b?".into(), Some("Invalid argument".into()))
            ]
        );
        let r = rules(|r| {
            r.windows_reserved = true;
            r.windows_chars = true;
        });
        let text = explain_git_error(stderr, &r).unwrap();
        assert!(text.starts_with("git can't check out 2 paths on this file system:\n  drivers/i2c/aux.c — aux.c is a reserved device name on Windows"), "{text}");
        assert!(text.contains("a/b? — '?' can't be used"), "{text}");
        assert_eq!(explain_git_error("fatal: not a git repository", &r), None);
    }

    #[gitti_test::test]
    fn clone_collisions_from_gits_warning() {
        let stderr = "Cloning into 'linux'...\n\
            warning: the following paths have collided (e.g. case-sensitive paths\n\
            on a case-insensitive filesystem) and only one from the same\n\
            colliding group is in the working tree:\n\n  'include/xt_TCPMSS.h'\n  'include/xt_tcpmss.h'\n\
            Updating files: 100% done\n";
        assert_eq!(collided_paths(stderr), vec!["include/xt_TCPMSS.h", "include/xt_tcpmss.h"]);
        let note = clone_note(stderr).unwrap();
        assert!(note.starts_with("Cloned — but 2 paths collided"), "{note}");
        assert_eq!(clone_note("Cloning into 'x'...\n"), None);
    }
}
