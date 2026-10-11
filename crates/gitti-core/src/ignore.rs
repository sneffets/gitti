//! "Add to .gitignore": pattern suggestions for a path, appending a pattern to an ignore
//! file (`.gitignore` at the top or in a folder, or `.git/info/exclude`), and — on request —
//! untracking the tracked files a new pattern matches (ignoring never untracks by itself).
//!
//! The suggestions are shared with "Track with Git LFS…" (`.gitattributes`, see
//! [`RuleKind`]): both are per-folder rule files whose patterns are relative to the folder
//! the file is in.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};

use crate::Repo;

/// A ready-made pattern for a path ("This file", "Folder src/", "All *.log files").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreChoice {
    pub label: String,
    pub pattern: String,
}

/// Where a pattern goes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IgnoreFile {
    /// `.gitignore` at the top of the work tree (shared through commits).
    #[default]
    Gitignore,
    /// `<dir>/.gitignore` (repo-relative folder, no trailing `/`): its patterns are
    /// relative to that folder and only apply inside it.
    Folder(String),
    /// `.git/info/exclude` (this clone only).
    InfoExclude,
}

impl IgnoreFile {
    /// The `.gitignore` of folder `dir` (`""`: the top level).
    pub fn in_dir(dir: &str) -> Self {
        match dir.trim_matches('/') {
            "" => IgnoreFile::Gitignore,
            d => IgnoreFile::Folder(d.to_string()),
        }
    }

    /// The folder the patterns are relative to (`""` for the top level and `info/exclude`).
    pub fn dir(&self) -> &str {
        match self {
            IgnoreFile::Folder(d) => d,
            _ => "",
        }
    }

    pub fn label(&self) -> String {
        match self {
            IgnoreFile::Gitignore => ".gitignore".into(),
            IgnoreFile::Folder(d) => format!("{d}/.gitignore"),
            IgnoreFile::InfoExclude => ".git/info/exclude".into(),
        }
    }
}

/// What [`Repo::add_ignore_pattern`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreOutcome {
    pub pattern: String,
    pub file: IgnoreFile,
    /// The line was new (false: the file had it already).
    pub added: bool,
    /// Files removed from the index (`git rm --cached`), still on disk.
    pub untracked: Vec<String>,
    /// Tracked files the pattern matches that stay tracked (untracking not asked for).
    pub still_tracked: Vec<String>,
}

impl IgnoreOutcome {
    /// One line for the status bar.
    pub fn message(&self) -> String {
        let mut s = if self.added {
            format!("Added {} to {}", self.pattern, self.file.label())
        } else {
            format!("{} already lists {}", self.file.label(), self.pattern)
        };
        if !self.untracked.is_empty() {
            s.push_str(&format!(
                " — stopped tracking {} file(s) (still on disk; commit the removal)",
                self.untracked.len()
            ));
        }
        if !self.still_tracked.is_empty() {
            s.push_str(&format!(
                " — {} tracked file(s) match and stay tracked",
                self.still_tracked.len()
            ));
        }
        s
    }
}

/// `component` as a literal gitignore pattern part: glob characters and backslashes
/// escaped, a leading `#` / `!` escaped, trailing spaces kept.
pub fn escape(component: &str) -> String {
    let mut out = String::with_capacity(component.len());
    for (i, c) in component.chars().enumerate() {
        if matches!(c, '*' | '?' | '[' | '\\') || (i == 0 && matches!(c, '#' | '!')) {
            out.push('\\');
        }
        out.push(c);
    }
    // git strips unescaped trailing spaces.
    let trailing = out.len() - out.trim_end_matches(' ').len();
    if trailing > 0 {
        out.truncate(out.len() - trailing);
        out.push_str(&"\\ ".repeat(trailing));
    }
    out
}

/// Which kind of per-folder rule file a pattern is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    /// `.gitignore`: `dir/` matches the folder and so everything in it.
    Ignore,
    /// `.gitattributes`: a pattern matching a folder doesn't apply to the files in it, so
    /// "everything in a folder" is `dir/**`.
    Attributes,
}

/// Patterns offered for the repo-relative path `path` (`/`-separated; a trailing `/` means
/// a directory) in a top-level `.gitignore`: see [`rule_suggestions`].
pub fn suggestions(path: &str) -> Vec<IgnoreChoice> {
    rule_suggestions(RuleKind::Ignore, path, "")
}

/// Patterns offered for `path` in the `.gitignore` of folder `dir`.
pub fn suggestions_in(path: &str, dir: &str) -> Vec<IgnoreChoice> {
    rule_suggestions(RuleKind::Ignore, path, dir)
}

/// Patterns offered for the repo-relative path `path` in the rule file of folder `dir`
/// (`""`: the top level; patterns are relative to `dir`): the path itself (anchored), every
/// parent folder below `dir` from the nearest up, all files with its extension (in `dir`
/// and below), and its name anywhere (below `dir`). Empty when `path` isn't inside `dir`.
pub fn rule_suggestions(kind: RuleKind, path: &str, dir: &str) -> Vec<IgnoreChoice> {
    let is_dir = path.ends_with('/');
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let base: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= base.len() || parts[..base.len()] != base[..] {
        return Vec::new();
    }
    let rel = &parts[base.len()..];
    let Some((name, dirs)) = rel.split_last() else {
        return Vec::new();
    };
    let prefix = base.iter().map(|d| format!("{d}/")).collect::<String>();
    let within = if base.is_empty() {
        String::new()
    } else {
        format!(" in {prefix}")
    };
    // Everything inside the folder pattern `p` (`/x` or `x`).
    let tree = |p: String| match kind {
        RuleKind::Ignore => format!("{p}/"),
        RuleKind::Attributes => format!("{p}/**"),
    };
    let esc: Vec<String> = rel.iter().map(|p| escape(p)).collect();
    let this = format!("/{}", esc.join("/"));
    let mut out = vec![IgnoreChoice {
        label: if is_dir { "This folder" } else { "This file" }.into(),
        pattern: if is_dir { tree(this) } else { this },
    }];
    for depth in (1..=dirs.len()).rev() {
        out.push(IgnoreChoice {
            label: format!("Folder {prefix}{}/", dirs[..depth].join("/")),
            pattern: tree(format!("/{}", esc[..depth].join("/"))),
        });
    }
    if !is_dir
        && let Some((stem, ext)) = name.rsplit_once('.')
        && !stem.is_empty()
        && !ext.is_empty()
    {
        out.push(IgnoreChoice {
            label: format!("All *.{ext} files{within}"),
            pattern: format!("*.{}", escape(ext)),
        });
    }
    if !dirs.is_empty() || is_dir {
        let pattern = match (kind, is_dir) {
            (_, false) => escape(name),
            (RuleKind::Ignore, true) => tree(escape(name)),
            (RuleKind::Attributes, true) => tree(format!("**/{}", escape(name))),
        };
        out.push(IgnoreChoice {
            label: format!(
                "Anything named {name}{}{within}",
                if is_dir { "/" } else { "" }
            ),
            pattern,
        });
    }
    out
}

/// The folders whose rule file `name` (`.gitignore`, `.gitattributes`) can take a pattern
/// for `path`, from the top: `""` (the top level), every folder above `path` that already
/// has one, and the folder `path` is in.
pub fn rule_dirs(workdir: &Path, path: &str, name: &str) -> Vec<String> {
    rule_dirs_on(path, name, |dir| crate::paths::join_repo_path(workdir, dir))
}

/// [`rule_dirs`] with `on_disk` giving where a folder is.
fn rule_dirs_on(path: &str, name: &str, on_disk: impl Fn(&str) -> PathBuf) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let mut out = vec![String::new()];
    let parents = parts.len().saturating_sub(1);
    for depth in 1..=parents {
        let dir = parts[..depth].join("/");
        if depth == parents || on_disk(&dir).join(name).is_file() {
            out.push(dir);
        }
    }
    out
}

/// `pattern` of the rule file of folder `dir` as the equivalent pattern of a top-level
/// one: anchored ones (a `/` before the end) get the folder in front, the others match at
/// any depth below it.
pub fn rooted(pattern: &str, dir: &str) -> String {
    let base: Vec<String> = dir
        .split('/')
        .filter(|p| !p.is_empty())
        .map(escape)
        .collect();
    if base.is_empty() {
        return pattern.to_string();
    }
    let (neg, p) = match pattern.strip_prefix('!') {
        Some(p) => ("!", p),
        None => ("", pattern),
    };
    let base = base.join("/");
    if p.trim_end_matches('/').contains('/') {
        format!("{neg}/{base}/{}", p.trim_start_matches('/'))
    } else {
        format!("{neg}/{base}/**/{p}")
    }
}

/// The text of rule file `path` (`.gitignore`, `.gitattributes`) for editing it as a
/// whole: `""` when it doesn't exist yet. Refused — never an empty text that saving would
/// write over the file — when it can't be read, or isn't UTF-8 (a pattern for a name in
/// another encoding: the editor would change those bytes).
pub fn read_rules_file(path: &Path) -> Result<String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    String::from_utf8(bytes).map_err(|_| {
        anyhow!(
            "{} is not UTF-8 text — a pattern in it names a file whose name is in another \
             encoding. gitti can't edit it without changing those bytes: edit it in an \
             editor that keeps them.",
            path.display()
        )
    })
}

/// A pattern as typed: one line, not empty, no trailing `\r`.
pub fn check_pattern(pattern: &str) -> Result<&str> {
    let p = pattern.trim_start().trim_end_matches(['\r', '\n']);
    if p.trim().is_empty() {
        bail!("empty pattern");
    }
    if p.contains(['\n', '\r']) {
        bail!("a pattern is a single line");
    }
    Ok(p)
}

/// Appends `pattern` as a line to `file` (created with its directory if missing; a
/// missing final newline is added first; CRLF files get CRLF). `false` when the file
/// already has exactly this line.
pub fn append_pattern(file: &Path, pattern: &str) -> Result<bool> {
    append_pattern_bytes(file, check_pattern(pattern)?.as_bytes())
}

/// [`append_pattern`] of a pattern in bytes (a name that is not UTF-8 keeps them,
/// [`crate::raw_names::Marked`]), checked as one line.
pub fn append_pattern_bytes(file: &Path, pattern: &[u8]) -> Result<bool> {
    if pattern.contains(&b'\n') || pattern.contains(&b'\r') {
        bail!("a pattern is a single line");
    }
    let text = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    if text
        .split(|b| *b == b'\n')
        .any(|l| l.strip_suffix(b"\r").unwrap_or(l) == pattern)
    {
        return Ok(false);
    }
    let eol: &[u8] = if text.windows(2).any(|w| w == b"\r\n") { b"\r\n" } else { b"\n" };
    let mut add = Vec::new();
    if !text.is_empty() && !text.ends_with(b"\n") {
        add.extend_from_slice(eol);
    }
    add.extend_from_slice(pattern);
    add.extend_from_slice(eol);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut f| f.write_all(&add))
        .with_context(|| format!("writing {}", file.display()))?;
    Ok(true)
}

impl Repo {
    /// Path of an ignore file (it may not exist yet). `info/exclude` lives in the common git
    /// dir, shared by all worktrees.
    pub fn ignore_file_path(&self, which: &IgnoreFile) -> Result<PathBuf> {
        Ok(match which {
            IgnoreFile::Gitignore => self.workdir().join(".gitignore"),
            // A folder that is not UTF-8 by its bytes.
            IgnoreFile::Folder(dir) => self.fs_path(dir).join(".gitignore"),
            IgnoreFile::InfoExclude => {
                let repo = self.gix()?;
                crate::paths::canonical(repo.common_dir())
                    .join("info")
                    .join("exclude")
            }
        })
    }

    /// Tracked files `pattern` matches as a line of the `.gitignore` of folder `dir`
    /// (`""`: the top level).
    pub fn tracked_matching(&self, pattern: &str, dir: &str) -> Result<Vec<String>> {
        let pattern = check_pattern(pattern)?;
        match self.raw_ignore_pattern(pattern, dir)? {
            None => {
                let exclude = format!("--exclude={}", rooted(pattern, dir));
                self.ignored_tracked(&["ls-files", "-z", "--cached", "--ignored", &exclude])
            }
            Some((m, c)) => self.tracked_matching_bytes(&m.bytes(&rooted(&c, &m.prefix(dir).unwrap_or_default()))),
        }
    }

    /// The bytes of `pattern` for the `.gitignore` of folder `dir` when it has U+FFFD in
    /// it — made from a name that is not UTF-8 ([`crate::raw_names::RawNames::rule_pattern`]),
    /// with that name marked (for the rooted form); `None`: its own bytes.
    fn raw_ignore_pattern(&self, pattern: &str, dir: &str) -> Result<Option<(crate::raw_names::Marked, String)>> {
        self.git()
            .raw_names()
            .rule_pattern(pattern, |m| {
                m.prefix(dir)
                    .map(|d| rule_suggestions(RuleKind::Ignore, &m.text, &d).into_iter().map(|c| c.pattern).collect())
                    .unwrap_or_default()
            })
            .map_err(|e| anyhow!(e))
    }

    /// [`Self::tracked_matching`] of a top-level pattern in bytes: it can't be an argument
    /// (they are text), so it goes through a scratch exclude file.
    fn tracked_matching_bytes(&self, rooted: &[u8]) -> Result<Vec<String>> {
        let dir = self
            .git_dir()
            .filter(|d| d.to_str().is_some())
            .unwrap_or_else(std::env::temp_dir);
        let file = dir.join(format!("gitti-exclude-{}", std::process::id()));
        std::fs::write(&file, [rooted, b"\n"].concat()).with_context(|| format!("writing {}", file.display()))?;
        let from = format!("--exclude-from={}", file.display());
        let files = self.ignored_tracked(&["ls-files", "-z", "--cached", "--ignored", &from]);
        let _ = std::fs::remove_file(&file);
        files
    }

    /// The paths `git ls-files -z` `args` lists (learned when not UTF-8).
    fn ignored_tracked(&self, args: &[&str]) -> Result<Vec<String>> {
        let out = self.git().run(args)?;
        let mut files: Vec<String> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|p| self.show_path(p))
            .collect();
        files.dedup(); // one line per stage for conflicted files
        Ok(files)
    }

    /// [`rule_dirs`] of the work tree (folders that are not UTF-8 by their bytes).
    pub fn rule_dirs(&self, path: &str, name: &str) -> Vec<String> {
        rule_dirs_on(path, name, |dir| self.fs_path(dir))
    }

    /// Adds `pattern` to `which` (no duplicate line); with `untrack`, the tracked files it
    /// matches are removed from the index (`git rm --cached`, they stay on disk).
    pub fn add_ignore_pattern(
        &self,
        pattern: &str,
        which: IgnoreFile,
        untrack: bool,
    ) -> Result<IgnoreOutcome> {
        let pattern = check_pattern(pattern)?.to_string();
        let file = self.ignore_file_path(&which)?;
        // A pattern made from a name that is not UTF-8 is written in its bytes.
        let added = match self.raw_ignore_pattern(&pattern, which.dir())? {
            Some((m, c)) => append_pattern_bytes(&file, &m.bytes(&c))?,
            None => append_pattern(&file, &pattern)?,
        };
        let tracked = self.tracked_matching(&pattern, which.dir())?;
        let (untracked, still_tracked) = if untrack {
            // Chunked: command lines are limited (Windows: 32k characters).
            for chunk in tracked.chunks(200) {
                let specs: Vec<String> = chunk.iter().map(|p| format!(":(literal){p}")).collect();
                let mut args = vec!["rm", "--cached", "-q", "--"];
                args.extend(specs.iter().map(String::as_str));
                self.git().run(&args)?;
            }
            (tracked, Vec::new())
        } else {
            (Vec::new(), tracked)
        };
        Ok(IgnoreOutcome {
            pattern,
            file: which,
            added,
            untracked,
            still_tracked,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(path: &str) -> Vec<String> {
        suggestions(path).into_iter().map(|c| c.pattern).collect()
    }

    #[gitti_test::test]
    fn suggested_patterns() {
        assert_eq!(
            patterns("build/out/app.log"),
            [
                "/build/out/app.log",
                "/build/out/",
                "/build/",
                "*.log",
                "app.log"
            ]
        );
        assert_eq!(patterns("notes.txt"), ["/notes.txt", "*.txt"]);
        assert_eq!(patterns(".env"), ["/.env"]);
        assert_eq!(patterns("target/"), ["/target/", "target/"]);
        assert_eq!(
            patterns("a b/[x]*.c"),
            ["/a b/\\[x]\\*.c", "/a b/", "*.c", "\\[x]\\*.c"]
        );
        assert_eq!(suggestions("dir/f.rs")[1].label, "Folder dir/");
        assert!(suggestions("").is_empty());
    }

    #[gitti_test::test]
    fn suggestions_in_a_folder() {
        let p = |path, dir| -> Vec<String> {
            suggestions_in(path, dir).into_iter().map(|c| c.pattern).collect()
        };
        assert_eq!(
            p("a/b/c/x.log", "a"),
            ["/b/c/x.log", "/b/c/", "/b/", "*.log", "x.log"]
        );
        assert_eq!(p("a/x.log", "a"), ["/x.log", "*.log"]);
        assert_eq!(suggestions_in("a/x.log", "a")[1].label, "All *.log files in a/");
        assert_eq!(suggestions_in("a/b/x.log", "a")[1].label, "Folder a/b/");
        assert!(p("ab/x.log", "a").is_empty(), "not inside the folder");
        assert!(p("a/", "a").is_empty());
        let attr = |path, dir| -> Vec<String> {
            rule_suggestions(RuleKind::Attributes, path, dir)
                .into_iter()
                .map(|c| c.pattern)
                .collect()
        };
        assert_eq!(
            attr("art/raw/x.psd", ""),
            ["/art/raw/x.psd", "/art/raw/**", "/art/**", "*.psd", "x.psd"]
        );
        assert_eq!(attr("art/raw/", "art"), ["/raw/**", "**/raw/**"]);
        assert_eq!(attr("big.bin", ""), ["/big.bin", "*.bin"]);
    }

    #[gitti_test::test]
    fn rooted_patterns() {
        assert_eq!(rooted("*.log", ""), "*.log");
        assert_eq!(rooted("*.log", "a/b"), "/a/b/**/*.log");
        assert_eq!(rooted("/x.log", "a"), "/a/x.log");
        assert_eq!(rooted("c/x.log", "a"), "/a/c/x.log");
        assert_eq!(rooted("build/", "a"), "/a/**/build/");
        assert_eq!(rooted("!keep", "a"), "!/a/**/keep");
        assert_eq!(rooted("*.o", "[x]"), "/\\[x]/**/*.o");
    }

    #[gitti_test::test]
    fn rule_dirs_of_a_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        std::fs::write(dir.path().join("a/.gitignore"), "").unwrap();
        assert_eq!(rule_dirs(dir.path(), "a/b/c/x", ".gitignore"), ["", "a", "a/b/c"]);
        assert_eq!(rule_dirs(dir.path(), "a/b/c/x", ".gitattributes"), ["", "a/b/c"]);
        assert_eq!(rule_dirs(dir.path(), "x", ".gitignore"), [""]);
        assert_eq!(rule_dirs(dir.path(), "a/b/", ".gitignore"), ["", "a"]);
    }

    #[gitti_test::test]
    fn escaping() {
        assert_eq!(escape("#x"), "\\#x");
        assert_eq!(escape("!x"), "\\!x");
        assert_eq!(escape("a#b"), "a#b");
        assert_eq!(escape("x  "), "x\\ \\ ");
        assert_eq!(escape("a\\b"), "a\\\\b");
    }

    #[gitti_test::test]
    fn appends_once_with_newline_handling() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sub").join(".gitignore");
        assert!(append_pattern(&f, "/a").unwrap(), "creates file and dir");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "/a\n");
        assert!(!append_pattern(&f, "/a").unwrap(), "no duplicate");
        std::fs::write(&f, "x").unwrap();
        assert!(append_pattern(&f, "*.o").unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "x\n*.o\n");
        std::fs::write(&f, "x\r\n").unwrap();
        assert!(append_pattern(&f, "y").unwrap());
        assert!(
            !append_pattern(&f, "x").unwrap(),
            "CRLF line counts as identical"
        );
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "x\r\ny\r\n");
        assert!(append_pattern(&f, "  ").is_err());
        assert!(append_pattern(&f, "a\nb").is_err());
    }
}
