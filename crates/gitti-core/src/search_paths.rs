//! The file / path side of "Go to Commit…" ([`crate::commit_search`]): which search terms
//! name files or folders, and which commits touched them.
//!
//! - [`PathIndex`]: the paths of HEAD's tree (built once per loaded history, lowercased
//!   for case-insensitive matching).
//! - [`PathIndex::resolve`]: what a term names — `foo.rs` any `*/foo.rs`, `src/x` the
//!   folder `src/x` (or the file), `navigate` a file stem / folder name (a weaker guess).
//! - [`touching_commits`]: the commits that changed those paths, from `git log` streamed in
//!   the background and cancellable: `--follow` for a single file (renames), literal
//!   pathspecs otherwise (git's changed-path Bloom filters make that fast on huge repos),
//!   a case-insensitive glob over all refs for names that are not in HEAD (deleted or
//!   renamed files).

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use gix::ObjectId;

use crate::git_cli::Git;

/// More files than this per term are not searched (the report says how many there were).
pub const MAX_FILES: usize = 32;
/// Folders per term.
pub const MAX_DIRS: usize = 8;

/// The tracked paths of a commit's tree, sorted.
#[derive(Debug, Default)]
pub struct PathIndex {
    paths: Vec<String>,
    lower: Vec<String>,
}

/// How a term was found among the paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathGuess {
    /// The term looks like a path or file name, or is the full name of a file
    /// (`Makefile`): searched together with the text interpretation.
    Strong,
    /// A bare word that is a file stem or folder name (`navigate`, `tests`): only used
    /// when nothing else is found.
    Weak,
}

/// The files and folders of HEAD a term names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMatch {
    pub guess: PathGuess,
    /// Real-case paths, best first, at most [`MAX_FILES`].
    pub files: Vec<String>,
    /// Folders (without a trailing `/`), at most [`MAX_DIRS`].
    pub dirs: Vec<String>,
    /// Files + folders found before capping.
    pub total: usize,
    /// Found by a substring of the file name (no exact name matched).
    pub fuzzy: bool,
}

impl PathMatch {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.dirs.is_empty()
    }

    /// The shown paths (folders with a trailing `/`).
    pub fn labels(&self) -> Vec<String> {
        self.files
            .iter()
            .cloned()
            .chain(self.dirs.iter().map(|d| format!("{d}/")))
            .collect()
    }
}

/// A term without a `/`, with a file-name extension (`foo.rs`, `.gitignore`) or with a
/// `/` (`src/app`, `lib/`) looks like a path. `1.2.3` and `e.g.` do not (no letter in the
/// extension, empty part).
pub fn looks_like_path(term: &str) -> bool {
    let t = term.trim();
    if t.len() < 2 {
        return false;
    }
    if t.contains('/') || t.contains('\\') {
        return t.chars().any(|c| c.is_alphanumeric());
    }
    if let Some(rest) = t.strip_prefix('.') {
        // Dotfiles: `.gitignore`, `.github`.
        return !rest.contains('.') && rest.chars().any(|c| c.is_alphabetic());
    }
    match t.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && !stem.ends_with('.')
                && (1..=10).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// `term` as a repository path: lowercase, `\` → `/`, without `./` and outer `/`.
/// The flag says the term ended with `/` (folders only).
fn normalize(term: &str) -> (String, bool) {
    let t = term.trim().replace('\\', "/").to_lowercase();
    let dir_only = t.ends_with('/');
    let t = t.strip_prefix("./").unwrap_or(&t);
    (t.trim_matches('/').to_string(), dir_only)
}

impl PathIndex {
    pub fn from_paths(mut paths: Vec<String>) -> Self {
        paths.sort();
        paths.dedup();
        let lower = paths.iter().map(|p| p.to_lowercase()).collect();
        Self { paths, lower }
    }

    /// The files of `commit`'s tree (empty when it cannot be read).
    pub fn of_commit(repo: &gix::Repository, commit: ObjectId) -> Self {
        let paths = repo
            .find_commit(commit)
            .ok()
            .and_then(|c| c.tree().ok())
            .and_then(|t| t.traverse().breadthfirst.files().ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.mode.is_tree())
            .map(|e| e.filepath.to_string())
            .collect();
        Self::from_paths(paths)
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The files / folders `term` names, `None` if it names none (or is too short).
    pub fn resolve(&self, term: &str) -> Option<PathMatch> {
        let (t, dir_only) = normalize(term);
        if t.len() < 2 {
            return None;
        }
        let path_like = looks_like_path(term);
        let has_slash = t.contains('/');
        // (rank, index): exact path 0, file name / path suffix 1.
        let mut files: Vec<(usize, usize)> = Vec::new();
        let mut dirs: std::collections::BTreeSet<String> = Default::default();
        let suffix = format!("/{t}");
        let inner = format!("/{t}/");
        let as_prefix = format!("{t}/");
        for (i, pl) in self.lower.iter().enumerate() {
            if !dir_only {
                if *pl == t {
                    files.push((0, i));
                } else if pl.ends_with(&suffix) {
                    files.push((1, i));
                }
            }
            // Folder: `t` is a run of whole components of the path.
            let end = if pl.starts_with(&as_prefix) {
                Some(t.len())
            } else {
                pl.find(&inner).map(|at| at + 1 + t.len())
            };
            if let Some(end) = end {
                let dir = &self.paths[i][..end];
                if !dirs.contains(dir) {
                    dirs.insert(dir.to_string());
                }
            }
        }
        // A bare word naming a folder (`tests`) or a file stem (`navigate` → navigate.rs)
        // is a weak guess; a file name (`Makefile`, `foo.rs`) or a path is a strong one.
        let guess = if path_like || !files.is_empty() {
            PathGuess::Strong
        } else {
            PathGuess::Weak
        };
        let mut fuzzy = false;
        if files.is_empty() && dirs.is_empty() && !dir_only {
            for (i, pl) in self.lower.iter().enumerate() {
                let name = pl.rsplit('/').next().unwrap_or(pl);
                if !has_slash && !path_like {
                    // File stem: `navigate` → `…/navigate.rs`.
                    if name.split_once('.').is_some_and(|(stem, _)| stem == t) {
                        files.push((1, i));
                    }
                } else if t.len() >= 3 {
                    // Fuzzy-ish: part of a file name (`search.rs` → `commit_search.rs`) or,
                    // with a `/`, of a path from a component start (`src/fo` → `src/foo.rs`).
                    let hit = if has_slash {
                        pl.starts_with(&t) || pl.contains(&format!("/{t}"))
                    } else {
                        name.contains(&t)
                    };
                    if hit {
                        files.push((2, i));
                        fuzzy = true;
                    }
                }
            }
        }
        if files.is_empty() && dirs.is_empty() {
            return None;
        }
        let depth = |i: usize| self.paths[i].matches('/').count();
        files.sort_by(|a, b| {
            (a.0, depth(a.1), &self.paths[a.1]).cmp(&(b.0, depth(b.1), &self.paths[b.1]))
        });
        let mut dirs: Vec<String> = dirs.into_iter().collect();
        dirs.sort_by(|a, b| (a.matches('/').count(), a).cmp(&(b.matches('/').count(), b)));
        let total = files.len() + dirs.len();
        Some(PathMatch {
            guess,
            files: files
                .into_iter()
                .take(MAX_FILES)
                .map(|(_, i)| self.paths[i].clone())
                .collect(),
            dirs: dirs.into_iter().take(MAX_DIRS).collect(),
            total,
            fuzzy,
        })
    }
}

/// Glob pathspecs for a term searched in the whole history (any depth, any case): the
/// file `**/t` and everything below a folder `**/t/**`.
pub fn history_globs(term: &str) -> Vec<String> {
    let (t, dir_only) = normalize(term);
    if t.is_empty() {
        return Vec::new();
    }
    // Glob special characters are matched literally.
    let mut esc = String::new();
    for c in t.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '\\') {
            esc.push('\\');
        }
        esc.push(c);
    }
    let anchored = term.trim().starts_with('/');
    let base = if anchored { esc } else { format!("**/{esc}") };
    let file = format!(":(glob,icase){base}");
    let dir = format!(":(glob,icase){base}/**");
    // A name with an extension is a file (one pattern scans the history faster).
    let name = t.rsplit('/').next().unwrap_or(&t);
    if dir_only {
        vec![dir]
    } else if looks_like_path(name) && name.contains('.') {
        vec![file]
    } else {
        vec![file, dir]
    }
}

/// Which paths one `git log` run looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathLog {
    /// One file of HEAD, back through its renames (`git log --follow HEAD`).
    Follow(String),
    /// Files / folders, literally, on all refs.
    Literal(Vec<String>),
    /// Glob pathspecs ([`history_globs`]) on all refs.
    Glob(Vec<String>),
}

/// A commit that changed a searched path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathTouch {
    /// The path at that commit (`None` for merges, which list no files).
    pub path: Option<String>,
    /// The path before, when the commit renamed it.
    pub old_path: Option<String>,
}

/// Runs `run` and hands every commit it lists to `on_commit` as git prints it (newest
/// first) until that returns `false`. `false` when cancelled (git is killed then).
pub fn touching_commits(
    git: &Git,
    run: &PathLog,
    cancel: &AtomicBool,
    on_commit: &mut dyn FnMut(ObjectId, PathTouch) -> bool,
) -> bool {
    let mut args: Vec<&str> = vec!["log", "--format=%x01%H", "-z", "--name-status"];
    let specs: &[String] = match run {
        PathLog::Follow(path) => {
            args.extend(["--follow", "-M", "HEAD", "--"]);
            std::slice::from_ref(path)
        }
        PathLog::Literal(paths) => {
            args.extend(["--all", "--"]);
            paths
        }
        PathLog::Glob(specs) => {
            args.extend(["--all", "--"]);
            specs
        }
    };
    args.extend(specs.iter().map(String::as_str));
    let literal = !matches!(run, PathLog::Glob(_));
    let git = if literal {
        git.with_env(&[("GIT_LITERAL_PATHSPECS", "1")])
    } else {
        git.clone()
    };
    let mut parser = LogParser::default();
    let finished = git.run_cancellable(&args, cancel, &mut |chunk| {
        parser.feed(chunk, &mut *on_commit)
    });
    match finished {
        Ok(true) => {
            parser.finish(on_commit);
            true
        }
        // A failed git (no HEAD yet, bad pathspec): nothing touched it.
        Err(_) => true,
        // Stopped by `on_commit`, or cancelled.
        Ok(false) => !cancel.load(std::sync::atomic::Ordering::Relaxed),
    }
}

/// Splits `git log --format=%x01%H -z --name-status` output into commits as it arrives.
#[derive(Default)]
struct LogParser {
    buf: Vec<u8>,
}

impl LogParser {
    /// `false` once `on_commit` wants no more.
    fn feed(&mut self, chunk: &[u8], on_commit: &mut dyn FnMut(ObjectId, PathTouch) -> bool) -> bool {
        self.buf.extend_from_slice(chunk);
        // Every record but the last one is complete.
        let Some(last) = self.buf.iter().rposition(|&b| b == 1) else {
            return true;
        };
        let done: Vec<u8> = self.buf.drain(..last).collect();
        done.split(|&b| b == 1).all(|rec| parse_record(rec, on_commit))
    }

    fn finish(&mut self, on_commit: &mut dyn FnMut(ObjectId, PathTouch) -> bool) {
        let rest = std::mem::take(&mut self.buf);
        for rec in rest.split(|&b| b == 1) {
            if !parse_record(rec, on_commit) {
                break;
            }
        }
    }
}

/// One record: `<hash>` (40 or 64 hex digits) then (`\n`/`\0` separated) status letters
/// and paths. Returns what `on_commit` returns (`true` for no commit).
fn parse_record(rec: &[u8], on_commit: &mut dyn FnMut(ObjectId, PathTouch) -> bool) -> bool {
    let Some((id, rest)) = crate::object_format::split_leading_id(rec) else {
        return true;
    };
    let mut fields = rest
        .split(|&b| b == 0 || b == b'\n')
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned());
    let mut touch = PathTouch {
        path: None,
        old_path: None,
    };
    // The first file listed (the one the pathspec matched).
    if let Some(status) = fields.next() {
        let two = status.starts_with('R') || status.starts_with('C');
        let first = fields.next();
        if two {
            touch.old_path = first;
            touch.path = fields.next();
        } else {
            touch.path = first;
        }
    }
    on_commit(id, touch)
}

/// The commits that touched any of `runs`' paths, by commit (the first run listing a
/// commit wins — put `Follow` first for the rename info), at most `max` (git is stopped
/// there; the flag says so). `on_progress` sees the commits found so far every 150 ms.
/// `None` when cancelled.
pub fn collect_touches(
    git: &Git,
    runs: &[PathLog],
    max: usize,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(&HashMap<ObjectId, PathTouch>),
) -> Option<(HashMap<ObjectId, PathTouch>, bool)> {
    let mut found: HashMap<ObjectId, PathTouch> = HashMap::new();
    for run in runs {
        if found.len() >= max {
            return Some((found, true));
        }
        let mut last = std::time::Instant::now();
        let mut new = 0usize;
        let ok = touching_commits(git, run, cancel, &mut |id, touch| {
            found.entry(id).or_insert(touch);
            new += 1;
            if last.elapsed() > std::time::Duration::from_millis(150) {
                on_progress(&found);
                last = std::time::Instant::now();
                new = 0;
            }
            found.len() < max
        });
        if !ok {
            return None;
        }
        if new > 0 {
            on_progress(&found);
        }
    }
    let full = found.len() >= max;
    Some((found, full))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> PathIndex {
        PathIndex::from_paths(
            [
                "Makefile",
                "README.md",
                "src/foo.rs",
                "src/app/navigate.rs",
                "lib/foo.rs",
                "lib/deep/x/foo.rs",
                "tests/navigate.rs",
                "src/commit_search.rs",
                "docs/Makefile",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        )
    }

    #[gitti_test::test]
    fn path_like_terms() {
        for t in ["foo.rs", "src/x", "lib/", ".gitignore", "a\\b", "Cargo.toml"] {
            assert!(looks_like_path(t), "{t}");
        }
        for t in ["fix", "1.2.3", "v1.2", "e.", ".", "/", "a", "...", "x..rs"] {
            assert!(!looks_like_path(t), "{t}");
        }
    }

    #[gitti_test::test]
    fn resolve_names_paths_and_folders() {
        let ix = index();
        // A bare file name: every `*/foo.rs`, shallow ones first.
        let m = ix.resolve("FOO.rs").unwrap();
        assert_eq!(m.guess, PathGuess::Strong);
        assert_eq!(m.files, ["lib/foo.rs", "src/foo.rs", "lib/deep/x/foo.rs"]);
        // A path suffix only matches on component boundaries; the exact path ranks first.
        assert_eq!(ix.resolve("src/foo.rs").unwrap().files, ["src/foo.rs"]);
        assert_eq!(ix.resolve("x/foo.rs").unwrap().files, ["lib/deep/x/foo.rs"]);
        assert!(ix.resolve("c/foo.rs").is_none(), "not on a component boundary");
        // Folders.
        let m = ix.resolve("src/app").unwrap();
        assert_eq!((m.files.len(), m.dirs.clone()), (0, vec!["src/app".to_string()]));
        assert_eq!(ix.resolve("deep/").unwrap().dirs, ["lib/deep"]);
        assert_eq!(ix.resolve("./lib/").unwrap().labels(), ["lib/"]);
        // A full file name without extension is strong; a bare folder name is weak.
        let m = ix.resolve("makefile").unwrap();
        assert_eq!((m.guess, m.files.clone()), (PathGuess::Strong, vec!["Makefile".into(), "docs/Makefile".into()]));
        assert_eq!(ix.resolve("tests").unwrap().guess, PathGuess::Weak);
        // A file stem is weak.
        let m = ix.resolve("navigate").unwrap();
        assert_eq!(m.guess, PathGuess::Weak);
        assert_eq!(m.files, ["tests/navigate.rs", "src/app/navigate.rs"]);
        // Fuzzy-ish: part of a file name, for path-like terms only.
        let m = ix.resolve("search.rs").unwrap();
        assert!(m.fuzzy);
        assert_eq!(m.files, ["src/commit_search.rs"]);
        assert!(ix.resolve("search").is_none(), "plain words are not fuzzy-matched");
        assert!(ix.resolve("nothing.rs").is_none());
        assert!(ix.resolve("x").is_none(), "too short");
    }

    #[gitti_test::test]
    fn resolve_caps_many_files() {
        let paths: Vec<String> = (0..100).map(|i| format!("d{i:03}/mod.rs")).collect();
        let m = PathIndex::from_paths(paths).resolve("mod.rs").unwrap();
        assert_eq!(m.files.len(), MAX_FILES);
        assert_eq!(m.total, 100);
        assert_eq!(m.files[0], "d000/mod.rs");
    }

    #[gitti_test::test]
    fn globs() {
        assert_eq!(history_globs("Foo.rs"), [":(glob,icase)**/foo.rs"]);
        assert_eq!(
            history_globs("src/old"),
            [":(glob,icase)**/src/old", ":(glob,icase)**/src/old/**"]
        );
        assert_eq!(history_globs("/lib/"), [":(glob,icase)lib/**"]);
        assert_eq!(history_globs("a*b"), [":(glob,icase)**/a\\*b", ":(glob,icase)**/a\\*b/**"]);
    }

    #[gitti_test::test]
    fn parses_streamed_log() {
        let a = "a".repeat(40);
        // A SHA-256 id (64 hex digits) parses the same.
        let b = "b".repeat(64);
        let c = "c".repeat(40);
        let out = format!(
            "\x01{a}\n\nM\0src/foo.rs\0\x01{b}\n\nR087\0old/foo.rs\0src/foo.rs\0\x01{c}\n\n"
        );
        let mut got = Vec::new();
        let mut p = LogParser::default();
        // Fed in odd pieces, as a pipe delivers it.
        for piece in out.as_bytes().chunks(7) {
            p.feed(piece, &mut |id, t| {
                got.push((id.to_string(), t));
                true
            });
        }
        p.finish(&mut |id, t| {
            got.push((id.to_string(), t));
            true
        });
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].1.path.as_deref(), Some("src/foo.rs"));
        assert_eq!(got[1].1.old_path.as_deref(), Some("old/foo.rs"));
        assert_eq!(got[1].0, b);
        assert_eq!(got[1].1.path.as_deref(), Some("src/foo.rs"));
        assert_eq!((got[2].0.as_str(), got[2].1.path.as_deref()), (c.as_str(), None));
    }
}
