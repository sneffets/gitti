//! Linked worktrees (`git worktree`): the list (cached until git's worktree metadata
//! changes), add / remove / move / lock / unlock / prune / repair, and helpers both
//! frontends share
//! (which worktree has a branch checked out, the default folder for a new one).
//! Everything runs through the repository's [`crate::git_cli::Git`] runner (askpass,
//! command log).

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use gix::ObjectId;

use crate::Repo;
use crate::git_cli::GitError;

mod add_plan;
pub use add_plan::{
    AddInput, AddPlan, OK_CHECK_OUT, OK_CREATE, branch_name_problem, local_branch_name,
    path_problem, plan_add,
};

/// One entry of `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Its folder (canonical when it exists).
    pub path: PathBuf,
    /// The checked-out commit (`None`: unborn branch / bare).
    pub head: Option<ObjectId>,
    /// Short name of the checked-out branch (`None`: detached, bare).
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    /// `Some(reason)` when locked (the reason may be empty).
    pub locked: Option<String>,
    /// `Some(reason)` when git would prune it (its folder is gone).
    pub prunable: Option<String>,
    /// The main worktree (the first entry; its `.git` is the common dir).
    pub is_main: bool,
    /// The worktree this [`Repo`] was opened in.
    pub is_current: bool,
    /// Only part of the tree is checked out (`git sparse-checkout`), see [`Sparse`].
    pub sparse: Option<Sparse>,
}

/// A worktree's sparse checkout: only some folders are in it (`git sparse-checkout`, set
/// per worktree — git keeps `core.sparseCheckout` in the worktree's `config.worktree`
/// and the patterns in its own `info/sparse-checkout`). Files outside are not deleted
/// for git: they carry the skip-worktree bit, status and diffs ignore them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sparse {
    /// Cone mode (git's default): `entries` are folders, each with everything below it;
    /// the files at the top level (and directly in each parent folder) are always there.
    pub cone: bool,
    /// The folders (cone mode) or the raw patterns (non-cone mode).
    pub entries: Vec<String>,
}

impl Sparse {
    /// The terminal's mark of a sparse worktree (a sign instead of the word
    /// "sparse"; gitti draws a funnel icon): a half circle — only part of it is there
    /// (not `◌`, gitty's graph node of the working directory). Geometric Shapes block,
    /// which the common terminal fonts (DejaVu Sans Mono / Menlo) have; others fall back.
    pub const MARK: char = '◐';

    /// The mark's tooltip / explanation: what sparse means, then which folders.
    pub fn mark_tip(&self) -> String {
        format!("Sparse checkout: only some folders\n{}", self.summary())
    }

    /// `a, b/c` — or `3 patterns` (non-cone), `top-level files only` (cone, no folders).
    pub fn summary(&self) -> String {
        match (self.cone, self.entries.len()) {
            (true, 0) => "top-level files only".into(),
            (true, _) => self.entries.join(", "),
            (false, 1) => "1 pattern".into(),
            (false, n) => format!("{n} patterns"),
        }
    }

    /// The text for an edit field: the folders, comma separated (cone mode only).
    pub fn folder_list(&self) -> String {
        if self.cone {
            self.entries.join(", ")
        } else {
            String::new()
        }
    }
}

impl Worktree {
    /// The folder name, for lists and labels.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// What is checked out, for a list next to the folder name: `⎇ <branch>`, else
    /// `detached <short sha>` (`bare` / `unborn`). The branch glyph keeps it apart from
    /// the folder name (a worktree's folder and its branch are independent).
    pub fn checkout_detail(&self) -> String {
        match &self.branch {
            Some(b) if !self.bare => format!("⎇ {b}"),
            _ => self.checkout_label(),
        }
    }

    /// What is checked out: the branch, else `detached <short sha>` (`bare` / `unborn`).
    pub fn checkout_label(&self) -> String {
        if self.bare {
            return "bare".into();
        }
        match (&self.branch, self.head) {
            (Some(b), _) => b.clone(),
            (None, Some(id)) => format!("detached {}", id.to_hex_with_len(7)),
            (None, None) => "unborn".into(),
        }
    }

    /// What "Copy SHA" copies: the full id of the checked-out commit (a branch's tip or
    /// the detached HEAD). `Err(why)` for a greyed-out entry (bare, unborn branch).
    pub fn copy_sha(&self) -> std::result::Result<String, &'static str> {
        match self.head {
            _ if self.bare => Err("bare"),
            Some(id) => Ok(id.to_hex().to_string()),
            None => Err("unborn branch, no commit yet"),
        }
    }

    /// The main worktree shown alone in the WORKTREES section (no linked worktrees): its
    /// menu has no tab, lock, move or remove entries.
    pub fn is_lone_main(&self, list: &[Worktree]) -> bool {
        self.is_main && list.len() == 1
    }

    /// A multi-line description (path, checkout, kind, lock / prune state) for tooltips.
    pub fn describe(&self) -> String {
        let mut s = format!("Folder {}\n", self.path.display());
        s.push_str(&match (&self.branch, self.head) {
            _ if self.bare => "Bare".to_string(),
            (Some(b), _) => format!("Branch {b} checked out"),
            (None, Some(id)) => format!("Detached at {}", id.to_hex_with_len(7)),
            (None, None) => "Unborn branch".to_string(),
        });
        s.push('\n');
        s.push_str(if self.is_main {
            "Main worktree"
        } else {
            "Linked worktree"
        });
        if self.is_current {
            s.push_str(" · open in this tab");
        }
        if let Some(sp) = &self.sparse {
            s.push_str(&match (sp.cone, sp.entries.is_empty()) {
                (true, true) => "\nSparse: top-level files only".to_string(),
                (true, false) => {
                    format!("\nSparse: only {} (and top-level files)", sp.summary())
                }
                (false, _) => {
                    format!("\nSparse: {}\n  {}", sp.summary(), sp.entries.join("\n  "))
                }
            });
        }
        if let Some(r) = &self.locked {
            s.push_str(&if r.is_empty() {
                "\nlocked".to_string()
            } else {
                format!("\nlocked: {r}")
            });
        }
        if let Some(r) = &self.prunable {
            s.push_str(&format!("\nprunable: {r}"));
        }
        s
    }
}

/// What a new worktree checks out ([`Repo::add_worktree`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddMode {
    /// A new branch `name` starting at `start` (`git worktree add -b`).
    NewBranch { name: String, start: String },
    /// An existing local branch that isn't checked out anywhere.
    Existing(String),
    /// A new local branch `name` from the remote branch `upstream` (`origin/x`), tracking
    /// it (`git worktree add --track -b`).
    Track { name: String, upstream: String },
    /// A detached HEAD at a revision.
    Detached(String),
}

/// Result of [`Repo::remove_worktree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed(String),
    /// git refused: the worktree has modified or untracked files (or submodules). Only a
    /// forced remove (which loses them) goes on.
    Dirty(String),
}

/// What happened to a branch deleted with `git branch -d` ([`Repo::delete_branch_checked`],
/// "Also delete branch" of Remove Worktree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchDeletion {
    /// Deleted; git's message.
    Deleted(String),
    /// git refused: not fully merged (into its upstream or HEAD). `lost` = the commits no
    /// other ref reaches (`<short sha> <subject>`, newest first) — what `-D` would drop;
    /// `more` = how many beyond those. Empty `lost`: other refs (e.g. remote branches)
    /// still have every commit.
    NotMerged {
        branch: String,
        lost: Vec<String>,
        more: usize,
    },
    /// Any other refusal (git's message).
    Failed(String),
}

/// Result of [`Repo::remove_worktree_and_branch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveResult {
    /// The worktree is gone; `branch` = what happened to the branch, if asked to delete it.
    Removed {
        message: String,
        branch: Option<BranchDeletion>,
    },
    /// git refused (local changes, untracked files, submodules); nothing was changed.
    /// `changes` = the worktree's `git status --short` lines, what a forced remove deletes.
    Refused {
        message: String,
        changes: Vec<String>,
    },
}

/// The commits listed by name in a "would be lost" message at most.
pub const LOST_COMMITS_SHOWN: usize = 20;

/// Why "Remove Worktree…" can't run for `wt` (for a greyed-out menu item's label), `None`
/// when it can. Its own tab may remove it (the removal runs from the main worktree).
pub fn remove_blocker(wt: &Worktree) -> Option<String> {
    if wt.is_main {
        Some("the main worktree".into())
    } else if wt.bare {
        Some("bare".into())
    } else if wt.locked.is_some() {
        Some("locked — unlock it first".into())
    } else if wt.prunable.is_some() {
        Some("its folder is gone — Prune instead".into())
    } else {
        None
    }
}

/// Why "Move Worktree…" can't run for `wt` (for a greyed-out menu item's label), `None`
/// when it can. git never moves the main worktree, and a locked one only with
/// `--force --force` (not offered: unlock it first). A worktree whose folder is gone was
/// probably moved by hand: Repair Worktrees… reconnects it.
pub fn move_blocker(wt: &Worktree) -> Option<String> {
    if wt.is_main {
        Some("the main worktree".into())
    } else if wt.bare {
        Some("bare".into())
    } else if wt.locked.is_some() {
        Some("locked — unlock it first".into())
    } else if wt.prunable.is_some() {
        Some("its folder is gone — Repair Worktrees… if it was moved".into())
    } else {
        None
    }
}

/// A readable refusal before `git worktree move from to` runs: no target, the same
/// folder, a folder inside the worktree itself, or a target that exists (git would move
/// the worktree *into* an existing folder — the dialog asks for the new folder itself).
pub fn validate_move(from: &Path, to: &Path) -> Result<()> {
    if to.as_os_str().is_empty() || to.file_name().is_none() {
        return Err(anyhow!("enter the new folder of the worktree"));
    }
    let (f, t) = (crate::paths::canonical(from), absolute(to));
    if f == t {
        return Err(anyhow!("the worktree is in {} already", from.display()));
    }
    if t.starts_with(&f) {
        return Err(anyhow!("a worktree can't move into itself"));
    }
    if to.exists() {
        return Err(anyhow!("{} exists already", to.display()));
    }
    Ok(())
}

/// `p` made absolute with its existing ancestor canonical (`to` of a move doesn't exist).
fn absolute(p: &Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(n), Some(parent)) => {
                rest.push(n.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => return p.to_path_buf(),
        }
    }
    let mut out = crate::paths::canonical(&existing);
    for n in rest.iter().rev() {
        out.push(n);
    }
    out
}

/// The first `max` of `lines`, indented, then `… and N more` (for dialogs).
pub fn clip_lines(lines: &[String], max: usize) -> String {
    let mut s: String = lines.iter().take(max).map(|l| format!("  {l}\n")).collect();
    if lines.len() > max {
        s.push_str(&format!("  … and {} more\n", lines.len() - max));
    }
    s.trim_end().to_string()
}

/// Parses `git worktree list --porcelain -z`: records of NUL-terminated `key value` lines,
/// an empty line between records. `current` is the repository's work tree (canonical).
pub fn parse_worktree_list(out: &[u8], current: &Path) -> Vec<Worktree> {
    let mut list = Vec::new();
    let mut cur: Option<Worktree> = None;
    let finish = |w: Option<Worktree>, list: &mut Vec<Worktree>| {
        if let Some(mut w) = w {
            w.is_main = list.is_empty();
            list.push(w);
        }
    };
    for field in out.split(|b| *b == 0) {
        let field = String::from_utf8_lossy(field);
        if field.is_empty() {
            finish(cur.take(), &mut list);
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((&field, ""));
        if key == "worktree" {
            finish(cur.take(), &mut list);
            let raw = PathBuf::from(value);
            let path = if raw.exists() {
                crate::paths::canonical(&raw)
            } else {
                raw
            };
            cur = Some(Worktree {
                is_current: path == current,
                path,
                head: None,
                branch: None,
                detached: false,
                bare: false,
                locked: None,
                prunable: None,
                is_main: false,
                sparse: None,
            });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        match key {
            "HEAD" => {
                w.head = ObjectId::from_hex(value.as_bytes())
                    .ok()
                    .filter(|id| !id.is_null())
            }
            "branch" => {
                w.branch = Some(
                    value
                        .strip_prefix("refs/heads/")
                        .unwrap_or(value)
                        .to_string(),
                )
            }
            "detached" => w.detached = true,
            "bare" => w.bare = true,
            "locked" => w.locked = Some(value.to_string()),
            "prunable" => w.prunable = Some(value.to_string()),
            _ => {}
        }
    }
    finish(cur, &mut list);
    list
}

/// The worktree *other than the current one* that has `branch` checked out.
pub fn branch_worktree<'a>(list: &'a [Worktree], branch: &str) -> Option<&'a Worktree> {
    list.iter()
        .find(|w| !w.is_current && w.branch.as_deref() == Some(branch))
}

/// The worktrees *other than the current one* with a detached HEAD at `id` (the commit
/// list labels them; a branch checked out elsewhere is marked via [`branch_worktree`]).
pub fn detached_at(list: &[Worktree], id: ObjectId) -> impl Iterator<Item = &Worktree> {
    list.iter().filter(move |w| {
        !w.is_current && !w.bare && w.branch.is_none() && w.head == Some(id)
    })
}

/// Whether the sidebar lists the worktrees: always with linked ones; the main worktree
/// alone (the folder the repository lives in) only with `main_alone` (the setting
/// `show_main_worktree_alone`), and never a bare repository's git dir.
pub fn section_shown(list: &[Worktree], main_alone: bool) -> bool {
    match list {
        [] => false,
        [only] => main_alone && only.is_main && !only.bare,
        _ => true,
    }
}

/// The folders typed into a "sparse folders" field: separated by commas, semicolons or
/// line breaks; `\` becomes `/`, leading / trailing slashes and `./` go, duplicates too.
pub fn parse_folder_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in text.split([',', ';', '\n']) {
        let mut f = part.trim().replace('\\', "/");
        while let Some(rest) = f.strip_prefix("./") {
            f = rest.to_string();
        }
        let f = f.trim_matches('/').to_string();
        if !f.is_empty() && f != "." && !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

/// The folders of cone-mode `sparse-checkout` patterns, `None` when they are not in cone
/// form. Cone files look like `/*`, `!/*/`, then per parent `/a/` + `!/a/*/` and per
/// folder `/a/b/`; a folder is included recursively when no `!/<folder>/*/` follows it.
pub fn parse_cone_patterns(text: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let mut dirs: Vec<String> = Vec::new();
    let mut parents: Vec<String> = Vec::new();
    for l in &lines {
        if matches!(*l, "/*" | "!/*/") {
            continue;
        }
        if let Some(d) = l.strip_prefix("!/").and_then(|r| r.strip_suffix("/*/")) {
            parents.push(unescape_pattern(d));
            continue;
        }
        let d = l
            .strip_prefix('/')
            .and_then(|r| r.strip_suffix('/'))
            .filter(|d| !d.is_empty())?;
        dirs.push(unescape_pattern(d));
    }
    dirs.retain(|d| !parents.contains(d));
    Some(dirs)
}

/// git escapes `*`, `?`, `[`, `\` … in cone patterns with a backslash.
fn unescape_pattern(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A boolean of section `[core]` in a git config file (last one wins; includes are not
/// followed — git writes the sparse keys plainly). `None` when the key isn't there.
fn core_bool(file: &Path, key: &str) -> Option<bool> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut in_core = false;
    let mut value = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            let name = line.trim_start_matches('[').split([']', ' ', '"']).next().unwrap_or("");
            in_core = name.eq_ignore_ascii_case("core");
            continue;
        }
        if !in_core || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let (k, v) = match line.split_once('=') {
            Some((k, v)) => (k.trim(), Some(v.trim())),
            None => (line, None),
        };
        if k.eq_ignore_ascii_case(key) {
            value = Some(match v.map(|v| v.trim_matches('"').to_ascii_lowercase()) {
                None => true,
                Some(v) => matches!(v.as_str(), "true" | "yes" | "on" | "1"),
            });
        }
    }
    value
}

/// The git dir of the worktree in `folder`: `.git` itself, or the one its `.git` file
/// points to (`gitdir: …`, relative to the folder).
pub(crate) fn worktree_git_dir(folder: &Path) -> Option<PathBuf> {
    let dot = folder.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    Some(folder.join(target))
}

/// The sparse checkout of the worktree in `folder` (`None`: full checkout), read from
/// files only: `core.sparseCheckout` of its `config.worktree`, else the common `config`;
/// the patterns from its `info/sparse-checkout`.
pub fn read_sparse(folder: &Path, common: &Path) -> Option<Sparse> {
    let git_dir = worktree_git_dir(folder)?;
    let key = |k: &str| {
        core_bool(&git_dir.join("config.worktree"), k)
            .or_else(|| core_bool(&common.join("config"), k))
    };
    if key("sparsecheckout") != Some(true) {
        return None;
    }
    let text = std::fs::read_to_string(git_dir.join("info").join("sparse-checkout"))
        .unwrap_or_default();
    match (key("sparsecheckoutcone"), parse_cone_patterns(&text)) {
        (Some(false), _) | (_, None) => Some(Sparse {
            cone: false,
            entries: text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(String::from)
                .collect(),
        }),
        (_, Some(dirs)) => Some(Sparse {
            cone: true,
            entries: dirs,
        }),
    }
}

/// `branch` (or any text) as a folder name part: path separators and characters that are
/// awkward in file names become `-`.
pub fn folder_part(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '@') {
                c
            } else {
                '-'
            }
        })
        .collect();
    s.trim_matches(|c| c == '-' || c == '.').to_string()
}

/// Folder name suggested for a worktree of `name` (a branch, or a short sha) of the
/// repository whose main worktree is called `repo_name`: `<repo>-<name>`.
pub fn suggested_folder_name(repo_name: &str, name: &str) -> String {
    match folder_part(name) {
        n if n.is_empty() => format!("{repo_name}-worktree"),
        n => format!("{repo_name}-{n}"),
    }
}

/// The folder suggested for a new worktree: a sibling of the main worktree
/// (`../<repo>-<name>`), see [`suggested_folder_name`].
pub fn suggested_path(main_workdir: &Path, name: &str) -> PathBuf {
    let repo_name = main_workdir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let parent = main_workdir.parent().unwrap_or(main_workdir);
    parent.join(suggested_folder_name(&repo_name, name))
}

/// The path from git's "already checked out" refusal (switch / worktree add): git ≥ 2.42
/// says `'b' is already used by worktree at '<path>'`, older ones `… is already checked
/// out at '<path>'`.
pub fn checked_out_elsewhere(stderr: &str) -> Option<PathBuf> {
    for marker in [
        "is already used by worktree at '",
        "is already checked out at '",
    ] {
        if let Some(i) = stderr.find(marker) {
            let rest = &stderr[i + marker.len()..];
            let end = rest.find("'\n").or_else(|| rest.rfind('\''))?;
            return Some(PathBuf::from(&rest[..end]));
        }
    }
    None
}

/// The stderr of a failed git call.
pub(crate) fn git_stderr(err: &anyhow::Error) -> Option<&str> {
    match err.downcast_ref::<GitError>()? {
        GitError::Failed { stderr, .. } => Some(stderr),
        GitError::Spawn(_) | GitError::Cancelled | GitError::OutputCut { .. } => None,
    }
}

impl Repo {
    /// Every worktree of the repository (main first), read again only when git's worktree
    /// metadata changed: `<common>/worktrees/` and per entry its `HEAD`, `locked` and the
    /// `.git` file in its folder (a deleted folder makes it prunable), plus the main
    /// worktree's `HEAD`. Without linked worktrees no git runs at all. Branch worktrees' `head` is looked up from their branch
    /// (in-process), so commits don't need a new `git worktree list`.
    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        if !self.has_linked_worktrees() {
            // The common case, without a process: just this (the main) worktree.
            let head = self.head().unwrap_or_default();
            return Ok(vec![Worktree {
                path: self.workdir().to_path_buf(),
                detached: head.branch.is_none() && head.id.is_some(),
                head: head.id,
                branch: head.branch,
                bare: false,
                locked: None,
                prunable: None,
                is_main: true,
                is_current: true,
                sparse: read_sparse(self.workdir(), &self.common_dir()),
            }]);
        }
        let files = self.worktree_stamp_files();
        let mut list = self.statics_worktrees().get(&files, || {
            let out = self.git().run(&["worktree", "list", "--porcelain", "-z"])?;
            Ok(parse_worktree_list(&out.stdout, self.workdir()))
        })?;
        // Sparse state from a few small files (not stamped: read each time).
        let common = self.common_dir();
        for w in list.iter_mut().filter(|w| w.prunable.is_none() && !w.bare) {
            w.sparse = read_sparse(&w.path, &common);
        }
        if list.iter().any(|w| w.branch.is_some())
            && let Ok(repo) = self.gix()
        {
            for w in list.iter_mut() {
                let Some(b) = &w.branch else { continue };
                if let Ok(mut r) = repo.find_reference(format!("refs/heads/{b}").as_str())
                    && let Ok(id) = r.peel_to_id()
                {
                    w.head = Some(id.detach());
                }
            }
        }
        Ok(list)
    }

    /// The files [`Repo::worktrees`] depends on (see there).
    fn worktree_stamp_files(&self) -> Vec<PathBuf> {
        let common = self.common_dir();
        let dir = common.join("worktrees");
        // A reftable repository keeps the HEADs in its tables (`crate::reftable`).
        let tables = |d: &Path| d.join("reftable").join("tables.list");
        let mut files = vec![common.join("HEAD"), tables(&common), dir.clone()];
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            names.sort();
            for meta in names {
                files.push(meta.join("HEAD"));
                files.push(tables(&meta));
                files.push(meta.join("locked"));
                // `gitdir` holds the path of the worktree's `.git` file.
                let gitdir = meta.join("gitdir");
                if let Ok(text) = std::fs::read_to_string(&gitdir) {
                    files.push(PathBuf::from(text.trim()));
                }
                files.push(gitdir);
            }
        }
        files
    }

    /// The main worktree's folder (the parent of the common dir), else this one's.
    pub fn main_workdir(&self) -> PathBuf {
        let common = self.common_dir();
        match common.file_name() {
            Some(n) if n == ".git" => common.parent().map(Path::to_path_buf),
            _ => None,
        }
        .unwrap_or_else(|| self.workdir().to_path_buf())
    }

    /// Whether this repository has worktrees besides the main one (cheap: one directory
    /// listing, no git).
    pub fn has_linked_worktrees(&self) -> bool {
        std::fs::read_dir(self.common_dir().join("worktrees")).is_ok_and(|mut d| d.next().is_some())
    }

    /// Creates a worktree at `path` (which must not exist or be empty).
    pub fn add_worktree(&self, path: &Path, mode: &AddMode) -> Result<String> {
        self.add_worktree_inner(path, mode, false)
    }

    fn add_worktree_inner(&self, path: &Path, mode: &AddMode, no_checkout: bool) -> Result<String> {
        let p = path.to_string_lossy().into_owned();
        let mut args: Vec<&str> = vec!["worktree", "add"];
        if no_checkout {
            args.push("--no-checkout");
        }
        match mode {
            AddMode::NewBranch { name, start } => {
                args.extend(["-b", name.as_str(), p.as_str(), start.as_str()])
            }
            AddMode::Existing(branch) => args.extend([p.as_str(), branch.as_str()]),
            AddMode::Track { name, upstream } => {
                args.extend(["--track", "-b", name.as_str(), p.as_str(), upstream.as_str()])
            }
            AddMode::Detached(rev) => args.extend(["--detach", p.as_str(), rev.as_str()]),
        }
        let out = self.git().run(&args)?;
        Ok(format!("{}{}", out.stdout_str(), out.stderr)
            .trim()
            .to_string())
    }

    /// Creates a sparse worktree at `path` with only `folders` (cone mode: each folder with
    /// everything below it, plus the top-level files): `worktree add --no-checkout`, then
    /// in it `sparse-checkout set --cone` and `read-tree -mu HEAD` (fills index and folder).
    /// git keeps the sparse settings per worktree (`extensions.worktreeConfig`), the main
    /// worktree stays complete. If the sparse step fails the half-made worktree is removed
    /// again (a new branch stays). Empty `folders` = a normal worktree.
    pub fn add_sparse_worktree(
        &self,
        path: &Path,
        mode: &AddMode,
        folders: &[String],
    ) -> Result<String> {
        if folders.is_empty() {
            return self.add_worktree(path, mode);
        }
        let added = self.add_worktree_inner(path, mode, true)?;
        let wt = self.git().with_workdir(path);
        let list = self.folder_lines(folders)?;
        let sparse = wt
            .run_stdin(&["sparse-checkout", "set", "--cone", "--stdin"], Some(&list))
            .and_then(|_| wt.run(&["read-tree", "-mu", "HEAD"]));
        if let Err(e) = sparse {
            let p = path.to_string_lossy().into_owned();
            let _ = self.git().run(&["worktree", "remove", "--force", &p]);
            return Err(anyhow::Error::from(e).context("sparse checkout of the new worktree"));
        }
        Ok(format!("{added}\nSparse: only {}", folders.join(", ")))
    }

    /// `folders` one per line for `git sparse-checkout set --stdin`, each in its bytes (a
    /// folder name that is not UTF-8, as the folder picker learned it: `crate::raw_names`).
    fn folder_lines(&self, folders: &[String]) -> Result<Vec<u8>> {
        let mut list = Vec::new();
        for f in folders {
            list.extend_from_slice(&self.path_bytes(f)?);
            list.push(b'\n');
        }
        Ok(list)
    }

    /// Changes which folders the worktree at `path` has (cone mode); empty `folders` makes
    /// it complete again (`sparse-checkout disable`). git updates the folder right away
    /// (files with changes outside the new set stay, with a warning).
    pub fn set_worktree_sparse(&self, path: &Path, folders: &[String]) -> Result<String> {
        let wt = self.git().with_workdir(path);
        let out = if folders.is_empty() {
            wt.run(&["sparse-checkout", "disable"])?
        } else {
            let list = self.folder_lines(folders)?;
            wt.run_stdin(&["sparse-checkout", "set", "--cone", "--stdin"], Some(&list))?
        };
        Ok(format!("{}{}", out.stdout_str(), out.stderr)
            .trim()
            .to_string())
    }

    /// Removes the worktree at `path` (its folder and git's metadata). Without `force` a
    /// worktree with modified or untracked files is refused ([`RemoveOutcome::Dirty`]);
    /// locked worktrees must be unlocked first.
    pub fn remove_worktree(&self, path: &Path, force: bool) -> Result<RemoveOutcome> {
        let p = path.to_string_lossy().into_owned();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&p);
        match self.git().with_workdir(self.remove_cwd(path)).run(&args) {
            Ok(out) => Ok(RemoveOutcome::Removed(
                format!("{}{}", out.stdout_str(), out.stderr)
                    .trim()
                    .to_string(),
            )),
            Err(e) => {
                let e = anyhow::Error::from(e);
                match git_stderr(&e) {
                    Some(s) if !force && is_dirty_refusal(s) => {
                        Ok(RemoveOutcome::Dirty(s.trim().to_string()))
                    }
                    _ => Err(e),
                }
            }
        }
    }

    /// Where `git worktree remove <path>` runs: here, unless `path` is this repository's
    /// own worktree (a tab removing itself) — then from the main worktree (the common dir
    /// for a bare main repository), never from inside the folder being deleted.
    fn remove_cwd(&self, path: &Path) -> PathBuf {
        let here = self.workdir();
        if crate::paths::canonical(path) != crate::paths::canonical(here) {
            return here.to_path_buf();
        }
        let common = self.common_dir();
        match common.file_name() {
            Some(n) if n == ".git" => common.parent().map(Path::to_path_buf).unwrap_or(common),
            _ => common,
        }
    }

    /// Removes the worktree at `path` (forced: also with local changes / untracked files,
    /// which are lost), then — when `branch` is given and the removal worked — deletes
    /// that branch with `git branch -d`, git's merged check included
    /// ([`Repo::delete_branch_checked`]). A refused removal changes nothing and lists the
    /// worktree's changes.
    pub fn remove_worktree_and_branch(
        &self,
        path: &Path,
        force: bool,
        branch: Option<&str>,
    ) -> Result<RemoveResult> {
        // `branch -d` compares with HEAD: it runs where the removal runs (a tab's own
        // worktree is gone afterwards).
        let cwd = self.remove_cwd(path);
        let message = match self.remove_worktree(path, force)? {
            RemoveOutcome::Removed(m) => m,
            RemoveOutcome::Dirty(message) => {
                let changes = self.worktree_changes(path).unwrap_or_default();
                return Ok(RemoveResult::Refused { message, changes });
            }
        };
        let branch = branch.map(|b| self.delete_branch_checked_in(&cwd, b));
        Ok(RemoveResult::Removed { message, branch })
    }

    /// `git branch -D <branch>` after [`Repo::remove_worktree_and_branch`] removed the
    /// worktree at `removed` — from where that ran (for a repository opened in that very
    /// worktree: the main worktree; its own folder is gone).
    pub fn force_delete_branch_after_remove(&self, removed: &Path, branch: &str) -> Result<String> {
        let out = self
            .git()
            .with_workdir(self.remove_cwd(removed))
            .run(&["branch", "-D", branch])?;
        Ok(format!("{}{}", out.stdout_str(), out.stderr)
            .trim()
            .to_string())
    }

    /// `git status --short` of the worktree at `path` (an untracked folder as one line):
    /// what a forced remove deletes. Ignored files are not listed (removing deletes them in
    /// any case).
    pub fn worktree_changes(&self, path: &Path) -> Result<Vec<String>> {
        let out = self
            .git()
            .with_workdir(path)
            .run(&["status", "--porcelain=v1", "--untracked-files=normal"])?;
        Ok(out
            .stdout_str()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim_end().to_string())
            .collect())
    }

    /// `git branch -d <branch>`; when git refuses because it isn't fully merged, the
    /// commits a forced delete would drop ([`BranchDeletion::NotMerged`]).
    pub fn delete_branch_checked(&self, branch: &str) -> BranchDeletion {
        self.delete_branch_checked_in(self.workdir(), branch)
    }

    fn delete_branch_checked_in(&self, cwd: &Path, branch: &str) -> BranchDeletion {
        let git = self.git().with_workdir(cwd);
        match git.run(&["branch", "-d", branch]) {
            Ok(out) => BranchDeletion::Deleted(
                format!("{}{}", out.stdout_str(), out.stderr)
                    .trim()
                    .to_string(),
            ),
            Err(e) => {
                let e = anyhow::Error::from(e);
                let text = format!("{e:#}");
                if !text.contains("not fully merged") {
                    return BranchDeletion::Failed(
                        git_stderr(&e).map(|s| s.trim().to_string()).unwrap_or(text),
                    );
                }
                let (lost, more) = lost_commits(&git, branch).unwrap_or_default();
                BranchDeletion::NotMerged {
                    branch: branch.to_string(),
                    lost,
                    more,
                }
            }
        }
    }

    /// The commits of `branch` that no other ref (branches, remote branches, tags, stash,
    /// HEAD) reaches — what deleting it drops: `<short> <subject>`, newest first, at most
    /// [`LOST_COMMITS_SHOWN`], and how many more there are.
    pub fn lost_commits(&self, branch: &str) -> Result<(Vec<String>, usize)> {
        lost_commits(self.git(), branch)
    }

    /// Moves the worktree at `from` to the new folder `to` (`git worktree move`; `to`
    /// must not exist, see [`validate_move`]). git refuses the main worktree, locked ones
    /// and worktrees with submodules. Like a removal it runs from the main worktree when
    /// `from` is this repository's own folder (never from inside the folder that moves).
    pub fn move_worktree(&self, from: &Path, to: &Path) -> Result<String> {
        validate_move(from, to)?;
        let (f, t) = (
            from.to_string_lossy().into_owned(),
            to.to_string_lossy().into_owned(),
        );
        let out = self
            .git()
            .with_workdir(self.remove_cwd(from))
            .run(&["worktree", "move", &f, &t])?;
        let said = format!("{}{}", out.stdout_str(), out.stderr)
            .trim()
            .to_string();
        Ok(if said.is_empty() {
            format!("Moved {} to {}", from.display(), to.display())
        } else {
            said
        })
    }

    /// `git worktree repair [<path>…]`: reconnects worktrees whose folders were moved by
    /// hand — without paths the links between this repository and its worktrees (e.g. the
    /// main worktree moved: the linked ones find it again), with the new folders of moved
    /// linked worktrees those. Returns git's report (one `repair: …` line per fix; empty
    /// = nothing needed repairing).
    pub fn repair_worktrees(&self, paths: &[PathBuf]) -> Result<String> {
        let paths: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let mut args = vec!["worktree", "repair"];
        args.extend(paths.iter().map(String::as_str));
        let out = self.git().run(&args)?;
        Ok(format!("{}{}", out.stdout_str(), out.stderr)
            .trim()
            .to_string())
    }

    /// Locks the worktree at `path` (git won't prune, move or remove it), with an
    /// optional reason.
    pub fn lock_worktree(&self, path: &Path, reason: Option<&str>) -> Result<()> {
        let p = path.to_string_lossy().into_owned();
        let mut args = vec!["worktree", "lock"];
        if let Some(r) = reason.filter(|r| !r.trim().is_empty()) {
            args.extend(["--reason", r]);
        }
        args.push(&p);
        self.git().run(&args)?;
        Ok(())
    }

    pub fn unlock_worktree(&self, path: &Path) -> Result<()> {
        let p = path.to_string_lossy().into_owned();
        self.git().run(&["worktree", "unlock", &p])?;
        Ok(())
    }

    /// `git worktree prune`: forgets worktrees whose folder is gone. Returns what was
    /// pruned (one line per worktree; empty = nothing).
    pub fn prune_worktrees(&self) -> Result<String> {
        let out = self.git().run(&["worktree", "prune", "-v"])?;
        Ok(format!("{}{}", out.stdout_str(), out.stderr)
            .trim()
            .to_string())
    }

    /// The worktree (another one) that has local `branch` checked out, from the cached list.
    pub fn branch_worktree(&self, branch: &str) -> Option<Worktree> {
        let list = self.worktrees().ok()?;
        branch_worktree(&list, branch).cloned()
    }
}

/// git's refusals of `worktree remove` that `--force` overrides (losing data): modified or
/// untracked files, submodules.
fn is_dirty_refusal(stderr: &str) -> bool {
    stderr.contains("contains modified or untracked files")
        || stderr.contains("containing submodules")
}

/// See [`Repo::lost_commits`]; `git` runs where HEAD is the one to compare with.
fn lost_commits(git: &crate::git_cli::Git, branch: &str) -> Result<(Vec<String>, usize)> {
    let full = format!("refs/heads/{branch}");
    let exclude = format!("--exclude={full}");
    let not = ["--not", exclude.as_str(), "--all"];
    let count = git.run(&[&["rev-list", "--count", full.as_str()][..], &not[..]].concat())?;
    let total: usize = count.stdout_str().trim().parse().unwrap_or(0);
    let max = format!("-n{LOST_COMMITS_SHOWN}");
    let log = git.run(
        &[
            &["log", max.as_str(), "--format=%h %s", full.as_str()][..],
            &not[..],
        ]
        .concat(),
    )?;
    let lost: Vec<String> = log.stdout_str().lines().map(str::to_string).collect();
    let more = total.saturating_sub(lost.len());
    Ok((lost, more))
}

/// The "already checked out in another worktree" refusal behind `err`, if that's it.
pub fn checked_out_elsewhere_error(err: &anyhow::Error) -> Option<PathBuf> {
    checked_out_elsewhere(git_stderr(err)?)
}

/// A readable error for an empty name etc. before git runs.
pub fn validate_add(path: &Path, mode: &AddMode) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(anyhow!("enter a folder for the worktree"));
    }
    if path.exists() && std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_some()) {
        return Err(anyhow!("{} exists and is not empty", path.display()));
    }
    match mode {
        AddMode::NewBranch { name, .. } | AddMode::Existing(name) | AddMode::Track { name, .. }
            if name.trim().is_empty() =>
        {
            Err(anyhow!("enter a branch name"))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn parses_porcelain_records() {
        let a = "1111111111111111111111111111111111111111";
        let b = "2222222222222222222222222222222222222222";
        let out = format!(
            "worktree /nonexistent/main\0HEAD {a}\0branch refs/heads/main\0\0\
             worktree /nonexistent/w1\0HEAD {b}\0branch refs/heads/feature/x\0locked on usb\0\0\
             worktree /nonexistent/w2\0HEAD {a}\0detached\0locked\0prunable gitdir file points to non-existent location\0\0"
        );
        let list = parse_worktree_list(out.as_bytes(), Path::new("/nonexistent/w1"));
        assert_eq!(list.len(), 3);
        assert!(list[0].is_main && !list[0].is_current);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].branch.as_deref(), Some("feature/x"));
        assert_eq!(list[1].locked.as_deref(), Some("on usb"));
        assert!(list[1].is_current && !list[1].is_main);
        assert_eq!(list[1].name(), "w1");
        assert!(list[2].detached && list[2].branch.is_none());
        assert_eq!(list[2].locked.as_deref(), Some(""));
        assert!(
            list[2]
                .prunable
                .as_deref()
                .unwrap()
                .contains("non-existent")
        );
        assert_eq!(list[2].checkout_label(), "detached 1111111");
        // Commit-list labels: the detached w2 at `a` (main has a branch there).
        let at_a = ObjectId::from_hex(a.as_bytes()).unwrap();
        let names: Vec<String> = detached_at(&list, at_a).map(|w| w.name()).collect();
        assert_eq!(names, ["w2"]);
        let at_b = ObjectId::from_hex(b.as_bytes()).unwrap();
        assert_eq!(detached_at(&list, at_b).count(), 0, "a branch, and the current one");
        assert_eq!(
            branch_worktree(&list, "main").map(|w| w.name()),
            Some("main".into())
        );
        assert!(
            branch_worktree(&list, "feature/x").is_none(),
            "the current one"
        );
        assert!(parse_worktree_list(b"", Path::new("/")).is_empty());
    }

    #[gitti_test::test]
    fn checkout_details() {
        let wt = |path: &str, branch: Option<&str>, main: bool| Worktree {
            path: PathBuf::from(path),
            head: ObjectId::from_hex(b"e17b7ef0e52b94039f699d19dc084f473fca2ae0").ok(),
            branch: branch.map(String::from),
            detached: branch.is_none(),
            bare: false,
            locked: None,
            prunable: None,
            is_main: main,
            is_current: false,
            sparse: None,
        };
        // The folder name is shown as is, the branch always (even when the folder is
        // named after it: the branch can change later).
        let f = wt("/src/gitti-asas", Some("kakadu"), false);
        assert_eq!(f.name(), "gitti-asas");
        assert_eq!(f.checkout_detail(), "⎇ kakadu");
        assert_eq!(
            f.describe(),
            "Folder /src/gitti-asas\nBranch kakadu checked out\nLinked worktree"
        );
        let d = wt("/src/gitti-e17b7ef", None, false);
        assert_eq!(d.checkout_detail(), "detached e17b7ef");
        let mut m = wt("/src/gitti", Some("main"), true);
        m.is_current = true;
        assert_eq!(
            m.describe(),
            "Folder /src/gitti\nBranch main checked out\nMain worktree · open in this tab"
        );
        // Copy SHA: the full id for a branch and a detached HEAD; greyed when bare / unborn.
        let full = "e17b7ef0e52b94039f699d19dc084f473fca2ae0";
        assert_eq!(m.copy_sha().as_deref(), Ok(full));
        assert_eq!(d.copy_sha().as_deref(), Ok(full));
        let mut unborn = m.clone();
        unborn.head = None;
        assert!(unborn.copy_sha().is_err());
        let mut bare = m.clone();
        bare.bare = true;
        assert_eq!(bare.copy_sha(), Err("bare"));
        // The section: linked worktrees always; the main one alone by the setting; a
        // bare git dir alone never.
        assert!(section_shown(std::slice::from_ref(&m), true));
        assert!(m.is_lone_main(std::slice::from_ref(&m)));
        assert!(!section_shown(std::slice::from_ref(&m), false));
        assert!(!section_shown(std::slice::from_ref(&bare), true));
        assert!(!section_shown(&[], true));
        let two = [m.clone(), f.clone()];
        assert!(section_shown(&two, false) && !m.is_lone_main(&two));
    }

    #[gitti_test::test]
    fn sparse_patterns_and_fields() {
        // What `git sparse-checkout set --cone a b/c "x y"` writes.
        let cone = "/*\n!/*/\n/a/\n/b/\n!/b/*/\n/b/c/\n/x y/\n";
        assert_eq!(
            parse_cone_patterns(cone),
            Some(vec!["a".to_string(), "b/c".into(), "x y".into()])
        );
        assert_eq!(parse_cone_patterns("/*\n!/*/\n"), Some(vec![]));
        assert_eq!(
            parse_cone_patterns("/*\n!/*/\n/w\\*ld/\n"),
            Some(vec!["w*ld".to_string()])
        );
        assert_eq!(parse_cone_patterns("*.rs\n!/target/\n"), None);
        assert_eq!(
            parse_folder_list(" a, ./b\\c/ ;\nd/ , a,, . "),
            ["a", "b/c", "d"]
        );
        let sp = Sparse {
            cone: true,
            entries: vec!["a".into(), "b/c".into()],
        };
        assert_eq!(sp.summary(), "a, b/c");
        assert_eq!(sp.folder_list(), "a, b/c");
        let raw = Sparse {
            cone: false,
            entries: vec!["*.rs".into()],
        };
        assert_eq!(raw.summary(), "1 pattern");
        assert_eq!(raw.folder_list(), "");
    }

    #[gitti_test::test]
    fn core_config_booleans() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("config");
        std::fs::write(
            &f,
            "[core]\n\trepositoryformatversion = 0\n[extensions]\n\tsparseCheckout = false\n\
             [Core]\n\tsparseCheckout = true\n\tsparseCheckoutCone\n[remote \"o\"]\n\turl = x\n",
        )
        .unwrap();
        assert_eq!(core_bool(&f, "sparsecheckout"), Some(true));
        assert_eq!(core_bool(&f, "sparsecheckoutcone"), Some(true), "bare key = true");
        assert_eq!(core_bool(&f, "bare"), None);
        assert_eq!(core_bool(&dir.path().join("missing"), "bare"), None);
    }

    #[gitti_test::test]
    fn folder_suggestions() {
        assert_eq!(
            suggested_folder_name("gitti", "feature/x y"),
            "gitti-feature-x-y"
        );
        assert_eq!(suggested_folder_name("gitti", ""), "gitti-worktree");
        assert_eq!(
            suggested_path(Path::new("/src/gitti"), "fix"),
            PathBuf::from("/src/gitti-fix")
        );
    }

    #[gitti_test::test]
    fn already_checked_out_messages() {
        let new = "fatal: 'f' is already used by worktree at '/tmp/a b/w1'\n";
        let old = "fatal: 'f' is already checked out at '/tmp/w1'\n";
        assert_eq!(
            checked_out_elsewhere(new),
            Some(PathBuf::from("/tmp/a b/w1"))
        );
        assert_eq!(checked_out_elsewhere(old), Some(PathBuf::from("/tmp/w1")));
        assert_eq!(checked_out_elsewhere("fatal: invalid reference: x\n"), None);
    }
}
