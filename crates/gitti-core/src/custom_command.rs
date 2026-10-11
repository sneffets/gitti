//! Custom commands: a named git or shell command line the user defines in gitti / gitty
//! ("Pull my GitHub subtree"), with placeholders for the repository, the selected commit,
//! branch, remote and file(s) ([`expand`]), kept for one repository or for every one, run
//! from the palette, the menus (Repository ▸ Run Custom Command…) and — where it needs a
//! selection — the commit / branch / remote / file context menus.
//!
//! - Kept in the settings database beside the settings (no own table): the commands for
//!   every repository under the settings key `custom_commands`, a repository's own in
//!   `repo_prefs` under the same key, keyed by its common git dir (shared by its
//!   worktrees). Read once per process and db ([`list`]), written through the store's
//!   writer thread ([`save`], [`remove`]).
//! - [`prepare`] fills the placeholders: a git command gets each value as an argument of
//!   its own, a shell command gets them quoted for the shell where they stand — names of
//!   files or branches can't run anything.
//! - [`Repo::run_custom`] runs it like any operation: in the Git Log (a shell command as
//!   `sh: <line>`), cancellable, askpass for credential prompts, output returned.
//! - [`form`] / [`from_form`] / [`plan`]: the define / edit dialog both apps show, with
//!   the live text of what it will run, where, and what follows.

pub mod expand;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;

pub use expand::{Placeholder, Shell, Values};

use crate::repo::Repo;

/// What runs the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The words after `git` (git gets them as arguments, no shell).
    #[default]
    Git,
    /// A line for the platform's shell ([`Shell::native`]).
    Shell,
}

/// A user's command as stored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CustomCommand {
    pub name: String,
    pub kind: Kind,
    /// With placeholders (`{commit}`, …; [`expand::parse`]).
    pub command: String,
    /// Empty: the repository's folder; relative: inside it; `~/…`; placeholders allowed.
    pub workdir: String,
    /// Show the command line and ask before running it.
    pub confirm: bool,
}

impl Default for CustomCommand {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: Kind::Git,
            command: String::new(),
            workdir: String::new(),
            confirm: true,
        }
    }
}

/// Where a command is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// This repository (and its worktrees) only.
    Repo,
    /// Every repository.
    Global,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Repo => "this repository",
            Scope::Global => "every repository",
        }
    }
}

/// A command and where it is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub scope: Scope,
    pub cmd: CustomCommand,
}

impl Entry {
    /// The placeholders it needs (command line and working folder).
    pub fn needs(&self) -> Vec<Placeholder> {
        let mut v = expand::used(&self.cmd.command);
        for p in expand::used(&self.cmd.workdir) {
            if !v.contains(&p) {
                v.push(p);
            }
        }
        v
    }

    /// The command as typed, `git …` or `sh: …`, for lists.
    pub fn command_text(&self) -> String {
        match self.cmd.kind {
            Kind::Git => {
                let c = self.cmd.command.trim();
                if c.starts_with("git ") { c.to_string() } else { format!("git {c}") }
            }
            Kind::Shell => format!("{}: {}", Shell::native().name(), self.cmd.command.trim()),
        }
    }

    /// The palette's / manager's detail: the command and where it is kept.
    pub fn detail(&self) -> String {
        let mut c = self.command_text();
        if c.chars().count() > 60 {
            c = c.chars().take(59).collect::<String>() + "…";
        }
        match self.scope {
            Scope::Repo => format!("{c} · this repository"),
            Scope::Global => c,
        }
    }

    /// Whether the context menu of `menu` offers it: it uses what that menu is about.
    pub fn offered_in(&self, menu: Menu) -> bool {
        let needs = self.needs();
        match menu {
            Menu::Commit => needs.contains(&Placeholder::Commit),
            Menu::Branch => needs.contains(&Placeholder::Branch),
            // A tag's menu: what it is about is the tag, and its commit.
            Menu::Tag => needs.contains(&Placeholder::Tag) || needs.contains(&Placeholder::Commit),
            Menu::Remote => needs.contains(&Placeholder::Remote),
            Menu::File => needs.contains(&Placeholder::File) || needs.contains(&Placeholder::Files),
        }
    }
}

/// The context menus custom commands appear in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Commit,
    Branch,
    Tag,
    Remote,
    File,
}

// ------------------------------------------------------------------ storage

/// The settings / `repo_prefs` key.
const KEY: &str = "custom_commands";

#[derive(Default)]
struct Cache {
    /// The db the lists were read from (tests switch `GITTI_CONFIG_DIR`).
    db: Option<PathBuf>,
    global: Option<Vec<CustomCommand>>,
    repos: std::collections::HashMap<PathBuf, Vec<CustomCommand>>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn with_cache<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let db = crate::store::db_path();
    let cache = guard.get_or_insert_with(Cache::default);
    if cache.db != db {
        *cache = Cache { db, ..Cache::default() };
    }
    f(cache)
}

/// The key a repository's commands are kept under: its common git dir, so its
/// worktrees share them.
pub fn repo_key(repo: &Repo) -> PathBuf {
    repo.common_dir()
}

fn global_list(c: &mut Cache) -> &mut Vec<CustomCommand> {
    c.global
        .get_or_insert_with(|| crate::store::load_setting(KEY).unwrap_or_default())
}

fn repo_list<'a>(c: &'a mut Cache, key: &Path) -> &'a mut Vec<CustomCommand> {
    c.repos
        .entry(key.to_path_buf())
        .or_insert_with(|| crate::store::load_repo_pref(key, KEY).unwrap_or_default())
}

/// The commands for the repository kept under `key` ([`repo_key`]): its own first, then
/// those for every repository. Read from the db once per process.
pub fn list(key: &Path) -> Vec<Entry> {
    with_cache(|c| {
        let mut out: Vec<Entry> = repo_list(c, key)
            .iter()
            .map(|cmd| Entry { scope: Scope::Repo, cmd: cmd.clone() })
            .collect();
        out.extend(
            global_list(c)
                .iter()
                .map(|cmd| Entry { scope: Scope::Global, cmd: cmd.clone() }),
        );
        out
    })
}

fn write(c: &mut Cache, key: &Path, scope: Scope) {
    match scope {
        Scope::Global => crate::store::save_setting(KEY, global_list(c)),
        Scope::Repo => crate::store::save_repo_pref(key, KEY, repo_list(c, key)),
    }
}

/// Keeps `entry`; `old`: the command it replaces (scope and name before editing) — it
/// keeps that one's place when the scope stays.
pub fn save(key: &Path, old: Option<(Scope, &str)>, entry: Entry) {
    with_cache(|c| {
        let mut at = None;
        if let Some((scope, name)) = old {
            let list = match scope {
                Scope::Global => global_list(c),
                Scope::Repo => repo_list(c, key),
            };
            if let Some(i) = list.iter().position(|x| x.name == name) {
                list.remove(i);
                if scope == entry.scope {
                    at = Some(i);
                }
            }
            if scope != entry.scope {
                write(c, key, scope);
            }
        }
        let list = match entry.scope {
            Scope::Global => global_list(c),
            Scope::Repo => repo_list(c, key),
        };
        // A command of the same name in that scope is replaced.
        if let Some(i) = list.iter().position(|x| x.name == entry.cmd.name) {
            list.remove(i);
            at = at.or(Some(i));
        }
        let i = at.unwrap_or(list.len()).min(list.len());
        list.insert(i, entry.cmd);
        write(c, key, entry.scope);
    })
}

/// Forgets the command `name` kept in `scope`.
pub fn remove(key: &Path, scope: Scope, name: &str) {
    with_cache(|c| {
        let list = match scope {
            Scope::Global => global_list(c),
            Scope::Repo => repo_list(c, key),
        };
        list.retain(|x| x.name != name);
        write(c, key, scope);
    })
}

// ------------------------------------------------------------------ context

/// What the placeholders are filled from where a command runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    pub repo: PathBuf,
    pub values: Values,
}

impl Context {
    pub fn new(repo: &Path) -> Self {
        Self {
            repo: repo.to_path_buf(),
            values: Values {
                repo: Some(repo.to_string_lossy().into_owned()),
                ..Values::default()
            },
        }
    }
}

/// `{remote}` outside a remote's menu: the current branch's upstream remote, else the
/// only remote, else `origin` if there is one.
pub fn default_remote(
    head_branch: Option<&str>,
    tracking: &[crate::ops::Tracking],
    remotes: &[crate::ops::RemoteInfo],
) -> Option<String> {
    let upstream = head_branch
        .and_then(|b| tracking.iter().find(|t| t.branch == b))
        .and_then(|t| t.upstream.as_deref());
    if let Some(up) = upstream {
        // The longest remote name the upstream starts with (names may contain `/`).
        let best = remotes
            .iter()
            .filter(|r| up.strip_prefix(r.name.as_str()).is_some_and(|rest| rest.starts_with('/')))
            .max_by_key(|r| r.name.len());
        if let Some(r) = best {
            return Some(r.name.clone());
        }
    }
    match remotes {
        [one] => Some(one.name.clone()),
        _ => remotes.iter().find(|r| r.name == "origin").map(|r| r.name.clone()),
    }
}

/// A remote branch's menu: its remote (the longest remote name `short` starts with —
/// names may contain `/`) and its name there; `(None, short)` without a match.
pub fn split_remote_branch(short: &str, remotes: &[String]) -> (Option<String>, String) {
    let best = remotes
        .iter()
        .filter(|r| short.strip_prefix(r.as_str()).is_some_and(|rest| rest.starts_with('/')))
        .max_by_key(|r| r.len());
    match best {
        Some(r) => (Some(r.clone()), short[r.len() + 1..].to_string()),
        None => (None, short.to_string()),
    }
}

/// `{files}` in the working directory: the changed files (conflicted, staged, unstaged —
/// each once).
pub fn worktree_files(ws: &crate::repo::WorkingState) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in ws.conflicted.iter().chain(&ws.staged).chain(&ws.unstaged) {
        let p = f.path();
        if !out.iter().any(|x| x == p) {
            out.push(p.to_string());
        }
    }
    out
}

/// `{files}` of a commit: its changed files (a deleted one by its old path).
pub fn diff_files(files: &[crate::diff::FileDiff]) -> Vec<String> {
    files.iter().map(|f| f.path().to_string()).filter(|p| !p.is_empty()).collect()
}

/// Why `entry` can't run in `ctx`: the first placeholder it needs without a value.
pub fn missing(entry: &Entry, ctx: &Context) -> Option<&'static str> {
    entry
        .needs()
        .into_iter()
        .find(|p| ctx.values.get(*p).is_none())
        .map(Placeholder::missing)
}

// ------------------------------------------------------------------ prepare / run

/// A command filled in, ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub name: String,
    pub kind: Kind,
    pub shell: Shell,
    /// git's arguments (Git).
    pub args: Vec<String>,
    /// The shell's command line (Shell).
    pub line: String,
    /// What runs, as shown: `git …` or the shell line.
    pub display: String,
    pub workdir: PathBuf,
    pub confirm: bool,
    /// The command and its values, for a shell line in bytes (file names that are not
    /// UTF-8, [`Repo::run_custom`]).
    template: String,
    values: Values,
}

/// `entry` filled from `ctx`; an error says why it can't run (a missing value, a broken
/// command line).
pub fn prepare(entry: &Entry, ctx: &Context) -> Result<Prepared, String> {
    if let Some(why) = missing(entry, ctx) {
        return Err(why.to_string());
    }
    let c = &entry.cmd;
    let shell = Shell::native();
    let workdir = expand::workdir(&c.workdir, &ctx.values, &ctx.repo)?;
    let (args, line, display) = match c.kind {
        Kind::Git => {
            let args = expand::git_args(&c.command, &ctx.values)?;
            let display = format!("git {}", expand::display_args(&args));
            (args, String::new(), display)
        }
        Kind::Shell => {
            let line = expand::shell_line(&c.command, &ctx.values, shell)?;
            (Vec::new(), line.clone(), line)
        }
    };
    Ok(Prepared {
        name: c.name.clone(),
        kind: c.kind,
        shell,
        args,
        line,
        display,
        workdir,
        confirm: c.confirm,
        template: c.command.clone(),
        values: ctx.values.clone(),
    })
}

/// The confirmation before running: what runs where, and what follows.
pub fn confirm_text(p: &Prepared) -> String {
    let how = match p.kind {
        Kind::Git => "git".to_string(),
        Kind::Shell => format!("the shell ({})", p.shell.name()),
    };
    format!(
        "Runs with {how} in {}:\n\n{}\n\nIts output shows in a window when it is done and in \
         the Git Log; Esc stops it while it runs. The repository is read again afterwards.",
        p.workdir.display(),
        p.display
    )
}

/// The busy label while it runs (and the status after it): its name.
pub fn busy_label(p: &Prepared) -> String {
    p.name.clone()
}

impl Repo {
    /// Runs a prepared custom command (an operation: cancellable, streamed to the
    /// progress sink, in the Git Log); its output, or an error with it and the exit code.
    pub fn run_custom(&self, p: &Prepared) -> Result<String> {
        if !p.workdir.is_dir() {
            anyhow::bail!(
                "{}: the working folder {} doesn't exist — edit the command (Custom Commands…)",
                p.name,
                p.workdir.display()
            );
        }
        let out = match p.kind {
            Kind::Git => {
                let args: Vec<&str> = p.args.iter().map(String::as_str).collect();
                // In the repository's own folder file names that are not UTF-8 reach git
                // as their bytes (`crate::raw_names`; another folder is another
                // repository's names).
                let git = if p.workdir == self.workdir() {
                    self.git().clone()
                } else {
                    self.git().with_workdir(&p.workdir)
                };
                git.run_logged(&args, None, true)?
            }
            Kind::Shell => {
                let (program, args) = p.shell.argv(&p.line);
                #[cfg_attr(not(unix), allow(unused_mut))]
                let mut args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
                // A file name that is not UTF-8: the shell line names it by its bytes.
                #[cfg(unix)]
                if p.shell == Shell::Posix && !self.git().raw_names().is_empty() {
                    use std::os::unix::ffi::OsStringExt as _;
                    let names = self.git().raw_names();
                    let line = expand::shell_line_bytes(&p.template, &p.values, p.shell, |x| {
                        let (dot, path) = match x.strip_prefix("./") {
                            Some(rest) => ("./", rest),
                            None => ("", x),
                        };
                        match names.resolve(path) {
                            crate::raw_names::Resolved::Bytes(b) => Ok(Some([dot.as_bytes(), &b].concat())),
                            crate::raw_names::Resolved::Ambiguous => Err(crate::raw_names::ambiguous_text(path)),
                            _ => Ok(None),
                        }
                    })
                    .map_err(|e| anyhow::anyhow!("{}: {e}", p.name))?;
                    if let Some(last) = args.last_mut() {
                        *last = std::ffi::OsString::from_vec(line);
                    }
                }
                self.git()
                    .run_shell(&program, &args, &p.workdir, p.shell.name(), &p.line)
                    .map_err(|e| match e {
                        crate::git_cli::GitError::Spawn(e) => {
                            anyhow::anyhow!("{}: {program} could not be started: {e}", p.name)
                        }
                        e => e.into(),
                    })?
            }
        };
        let text = format!("{}{}", out.stdout_str(), out.stderr).trim_end().to_string();
        if out.code != 0 {
            let what = match p.kind {
                Kind::Git => "git",
                Kind::Shell => p.shell.name(),
            };
            anyhow::bail!("{} failed ({what} exited with {}):\n{text}", p.name, out.code);
        }
        Ok(text)
    }
}

// ------------------------------------------------------------------ the dialog

/// Field indices of [`form`].
pub const F_NAME: usize = 0;
pub const F_COMMAND: usize = 1;
pub const F_WORKDIR: usize = 2;
/// Checkbox indices of [`form`].
pub const C_SHELL: usize = 0;
pub const C_CONFIRM: usize = 1;
pub const C_REPO: usize = 2;

/// The define / edit dialog's fields and checkboxes (both apps).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub fields: Vec<(&'static str, String)>,
    pub checks: Vec<(&'static str, bool)>,
}

/// The shell checkbox's label on this platform.
pub fn shell_check_label() -> &'static str {
    match Shell::native() {
        Shell::Posix => "Shell command (runs in sh, not git)",
        Shell::PowerShell => "Shell command (runs in PowerShell, not git)",
    }
}

pub fn form(e: &Entry) -> Form {
    Form {
        fields: vec![
            ("Name", e.cmd.name.clone()),
            ("Command (after git, or the shell command line)", e.cmd.command.clone()),
            ("Working folder (empty: the repository's)", e.cmd.workdir.clone()),
        ],
        checks: vec![
            (shell_check_label(), e.cmd.kind == Kind::Shell),
            ("Ask before running (show the command line first)", e.cmd.confirm),
            ("Only in this repository (else in every repository)", e.scope == Scope::Repo),
        ],
    }
}

pub fn from_form(values: &[String], checks: &[bool]) -> Entry {
    let v = |i: usize| values.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
    let c = |i: usize| checks.get(i).copied().unwrap_or(false);
    Entry {
        scope: if c(C_REPO) { Scope::Repo } else { Scope::Global },
        cmd: CustomCommand {
            name: v(F_NAME),
            kind: if c(C_SHELL) { Kind::Shell } else { Kind::Git },
            command: v(F_COMMAND),
            workdir: v(F_WORKDIR),
            confirm: c(C_CONFIRM),
        },
    }
}

/// What the dialog says live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub text: String,
    pub ok_label: String,
    /// Why it can't be saved as it is.
    pub blocked: Option<String>,
}

/// The dialog's live text for `e` (`old`: the command being edited, scope and name;
/// `existing`: every command now; `ctx`: the selection now, for the preview).
pub fn plan(e: &Entry, old: Option<(Scope, &str)>, existing: &[Entry], ctx: &Context) -> Plan {
    let ok_label = if old.is_some() { "Save" } else { "Add" }.to_string();
    let c = &e.cmd;
    let shell = Shell::native();
    let mut blocked = None;
    if c.name.is_empty() {
        blocked = Some("Give it a name (the palette and the menus show it)".to_string());
    } else if existing
        .iter()
        .any(|x| x.scope == e.scope && x.cmd.name == c.name && old != Some((x.scope, x.cmd.name.as_str())))
    {
        blocked = Some(format!("A command named “{}” is kept for {} already", c.name, e.scope.label()));
    }
    let mut text = Vec::new();
    // The preview: with the values there are now, the others as `{name}`.
    let preview = match c.kind {
        Kind::Git => expand::git_args(&c.command, &ctx.values)
            .map(|a| format!("git {}", expand::display_args(&a))),
        Kind::Shell => expand::shell_line(&c.command, &ctx.values, shell)
            .map(|l| format!("{}: {l}", shell.name())),
    };
    let preview = if c.command.is_empty() {
        Err("Type the command line".to_string())
    } else {
        preview
    };
    match &preview {
        Ok(line) => text.push(format!("Runs: {line}")),
        Err(why) => {
            blocked = blocked.or(Some(why.clone()));
        }
    }
    match expand::workdir(&c.workdir, &ctx.values, &ctx.repo) {
        Ok(dir) => text.push(format!("In: {}", dir.display())),
        Err(why) => blocked = blocked.or(Some(format!("Working folder: {why}"))),
    }
    let needs = e.needs();
    let shown: Vec<Placeholder> = needs.iter().copied().filter(|p| *p != Placeholder::Repo).collect();
    if !shown.is_empty() {
        let uses: Vec<String> = shown.iter().map(|p| format!("{{{}}} = {}", p.name(), p.meaning())).collect();
        text.push(format!("Uses {}.", uses.join("; ")));
        let mut menus = Vec::new();
        for (m, label) in [
            (Menu::Commit, "commit"),
            (Menu::Branch, "branch"),
            (Menu::Tag, "tag"),
            (Menu::Remote, "remote"),
            (Menu::File, "file"),
        ] {
            if e.offered_in(m) {
                menus.push(label);
            }
        }
        let mut line = String::new();
        if !menus.is_empty() {
            line = format!("Also in the {} menu (greyed where that is missing).", menus.join(" / "));
        }
        if let Some(why) = missing(e, ctx) {
            line = format!("{line} Now: {why}.").trim().to_string();
        }
        if !line.is_empty() {
            text.push(line);
        }
    }
    match c.kind {
        Kind::Git => {
            let mut line = "Values become git arguments of their own — no shell.".to_string();
            let words: Vec<&str> = c.command.split_whitespace().collect();
            let file_at = words.iter().position(|w| w.contains("{file}") || w.contains("{files}"));
            if let Some(i) = file_at
                && !words[..i].contains(&"--")
            {
                line.push_str(" Tip: put -- before the file placeholder so git reads it as a path.");
            }
            text.push(line);
        }
        Kind::Shell => text.push(format!(
            "Runs in {}; values are quoted for you (also inside '…' or \"…\") — no quotes needed.",
            match shell {
                Shell::Posix => "sh (/bin/sh -c)",
                Shell::PowerShell => "Windows PowerShell",
            }
        )),
    }
    text.push(format!(
        "Kept for {} (in the settings gitti and gitty share, not in the repository). {} Output: a \
         window and the Git Log; Esc stops it.",
        e.scope.label(),
        if c.confirm { "Asks first, showing the command line." } else { "Runs right away when chosen." }
    ));
    // The list as help (the typo message names it already).
    let listed = blocked.as_ref().is_some_and(|b| b.contains(&expand::placeholder_list()));
    if needs.is_empty() && !listed {
        text.push(format!("Placeholders: {} ({{{{ = a literal {{).", expand::placeholder_list()));
    }
    Plan { text: text.join("\n"), ok_label, blocked }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Kind, command: &str) -> Entry {
        Entry {
            scope: Scope::Global,
            cmd: CustomCommand {
                name: "x".into(),
                kind,
                command: command.into(),
                ..CustomCommand::default()
            },
        }
    }

    fn ctx() -> Context {
        Context::new(&std::env::temp_dir())
    }

    #[gitti_test::test]
    fn missing_names_the_first_value_it_needs() {
        let e = entry(Kind::Git, "log {commit} -- {file}");
        let mut c = ctx();
        assert_eq!(missing(&e, &c), Some("no commit selected"));
        c.values.commit = Some("abc".into());
        assert_eq!(missing(&e, &c), Some("no file selected"));
        c.values.file = Some("f".into());
        assert_eq!(missing(&e, &c), None);
        assert!(e.offered_in(Menu::Commit) && e.offered_in(Menu::File));
        assert!(!e.offered_in(Menu::Branch) && !e.offered_in(Menu::Remote));
        // A tag's menu: commands about its commit, and those using {tag}.
        assert!(e.offered_in(Menu::Tag));
        let t = entry(Kind::Git, "push {remote} {tag}");
        assert!(t.offered_in(Menu::Tag) && t.offered_in(Menu::Remote) && !t.offered_in(Menu::Commit));
        assert_eq!(missing(&t, &c), Some("no remote"));
        c.values.remote = Some("origin".into());
        assert_eq!(missing(&t, &c), Some("no tag on the selected commit"));
        let mut w = entry(Kind::Shell, "ls");
        w.cmd.workdir = "{branch}".into();
        assert_eq!(missing(&w, &c), Some("no current branch"));
    }

    #[gitti_test::test]
    fn prepare_git_and_shell() {
        let mut c = ctx();
        c.values.branch = Some("main".into());
        let p = prepare(&entry(Kind::Git, "git subtree pull --prefix=lib/x origin {branch}"), &c).unwrap();
        assert_eq!(p.args, vec!["subtree", "pull", "--prefix=lib/x", "origin", "main"]);
        assert_eq!(p.display, "git subtree pull --prefix=lib/x origin main");
        assert_eq!(p.workdir, c.repo);
        let p = prepare(&entry(Kind::Shell, "echo {branch}"), &c).unwrap();
        assert_eq!(p.args, Vec::<String>::new());
        assert!(p.line.starts_with("echo ") && p.line.contains("main"), "{}", p.line);
        assert_eq!(p.display, p.line);
        let e = prepare(&entry(Kind::Git, "show {commit}"), &c).unwrap_err();
        assert_eq!(e, "no commit selected");
        assert!(confirm_text(&prepare(&entry(Kind::Git, "status"), &c).unwrap()).contains("git status"));
    }

    #[gitti_test::test]
    fn default_remote_follows_the_upstream() {
        use crate::ops::{RemoteInfo, Tracking};
        let r = |n: &str| RemoteInfo { name: n.into(), fetch_url: String::new(), push_url: String::new() };
        let t = Tracking {
            branch: "main".into(),
            upstream: Some("gh/team/main".into()),
            ahead: 0,
            behind: 0,
            gone: false,
        };
        let remotes = [r("origin"), r("gh"), r("gh/team")];
        assert_eq!(default_remote(Some("main"), std::slice::from_ref(&t), &remotes).as_deref(), Some("gh/team"));
        assert_eq!(default_remote(None, &[t], &remotes).as_deref(), Some("origin"));
        assert_eq!(default_remote(None, &[], &[r("up")]).as_deref(), Some("up"));
        assert_eq!(default_remote(None, &[], &[r("a"), r("b")]), None);
        let names = vec!["gh".to_string(), "gh/team".to_string()];
        assert_eq!(
            split_remote_branch("gh/team/feature/x", &names),
            (Some("gh/team".to_string()), "feature/x".to_string())
        );
        assert_eq!(split_remote_branch("other/x", &names), (None, "other/x".to_string()));
    }

    #[gitti_test::test]
    fn form_roundtrip_and_plan_words() {
        let mut e = entry(Kind::Shell, "make {branch}");
        e.scope = Scope::Repo;
        e.cmd.workdir = "sub".into();
        e.cmd.confirm = false;
        let f = form(&e);
        let values: Vec<String> = f.fields.iter().map(|(_, v)| v.clone()).collect();
        let checks: Vec<bool> = f.checks.iter().map(|(_, c)| *c).collect();
        assert_eq!(from_form(&values, &checks), e);

        let c = ctx();
        let p = plan(&e, None, &[], &c);
        assert_eq!(p.blocked, None);
        assert_eq!(p.ok_label, "Add");
        assert!(p.text.contains("Runs: "), "{}", p.text);
        assert!(p.text.contains("{branch}"), "{}", p.text);
        assert!(p.text.contains("Now: no current branch"), "{}", p.text);
        assert!(p.text.contains("Also in the branch menu"), "{}", p.text);
        assert!(p.text.contains("Kept for this repository"), "{}", p.text);
        assert!(p.text.contains("Runs right away"), "{}", p.text);

        // A name taken in the same scope blocks; editing that one doesn't.
        let existing = vec![e.clone()];
        assert!(plan(&e, None, &existing, &c).blocked.unwrap().contains("already"));
        assert_eq!(plan(&e, Some((Scope::Repo, "x")), &existing, &c).blocked, None);
        let mut other = e.clone();
        other.scope = Scope::Global;
        assert_eq!(plan(&other, None, &existing, &c).blocked, None);

        let bad = entry(Kind::Git, "show {comit}");
        assert!(plan(&bad, None, &[], &c).blocked.unwrap().contains("no placeholder"));
        let unnamed = Entry { cmd: CustomCommand { name: String::new(), ..bad.cmd.clone() }, ..bad };
        assert!(plan(&unnamed, None, &[], &c).blocked.unwrap().contains("name"));
        let tip = plan(&entry(Kind::Git, "log {file}"), None, &[], &c);
        assert!(tip.text.contains("put --"), "{}", tip.text);
        let no_tip = plan(&entry(Kind::Git, "log -- {file}"), None, &[], &c);
        assert!(!no_tip.text.contains("put --"), "{}", no_tip.text);
    }

    #[gitti_test::test]
    fn worktree_files_once_each() {
        use crate::repo::{WorkFile, WorkingState};
        let wf = |p: &str| WorkFile {
            entry: crate::status::StatusEntry {
                path: p.into(),
                orig_path: None,
                index: Some('M'),
                worktree: None,
                untracked: false,
                conflicted: false,
                submodule: None,
            },
            diff: None,
            patch_diff: None,
        };
        let ws = WorkingState {
            staged: vec![wf("a"), wf("b")],
            unstaged: vec![wf("b"), wf("c")],
            ..Default::default()
        };
        assert_eq!(worktree_files(&ws), vec!["a", "b", "c"]);
    }
}
