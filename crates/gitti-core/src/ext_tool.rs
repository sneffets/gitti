//! "Open in External Diff Tool" / "Open in External Merge Tool": git's own `git difftool`
//! / `git mergetool`, so every tool setting works exactly as on the command line
//! (`diff.tool`, `merge.tool`, `diff.guitool` / `merge.guitool` with
//! `difftool.guiDefault` / `mergetool.guiDefault`, `difftool.<name>.cmd` /
//! `mergetool.<name>.cmd`, `.path`, `.trustExitCode`, `mergetool.keepBackup`,
//! `mergetool.keepTemporaries`, `mergetool.writeToTemp`, …).
//!
//! What gitti adds around git:
//! - the tool is resolved here first, the way git's `mergetool--lib` does, and passed as
//!   `--tool=<name>`: without a configured tool git would *guess* one (and could pick a
//!   terminal tool), so an unset tool is reported with what to do instead;
//! - tools that need a terminal (vimdiff, nvimdiff, emerge, a user command running vim,
//!   …) are refused with the reason where the app starts git without a terminal
//!   (`setsid`: gitti), since such a tool would hang or fail; gitty hands them its
//!   terminal ([`ToolStart::Terminal`], [`TerminalRun`]);
//! - an untracked file (git's diff doesn't see it) is shown with `--no-index` against an
//!   empty side; a stash's untracked file comes from the stash's third parent;
//! - after a merge tool run: whether the file is resolved (git mergetool stages it), the
//!   `.orig` backup git kept, and how many conflicts are left. A cancelled run is cleaned
//!   up the way git cleans up a failed one (the file put back, the temporary copies
//!   removed unless `mergetool.keepTemporaries`).
//!
//! Without a tool set up, the apps' "Open in External … Tool" offers the set-up dialog
//! ([`ToolStart::Choose`], [`crate::tool_setup`]); the list of tools git knows
//! ([`Repo::tool_help`], `git difftool --tool-help`) is read only then.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use gix::ObjectId;

use crate::Repo;
use crate::compare::CompareSide;
use crate::git_cli::GitError;
use crate::ops::split_command_line;

/// Which of git's two tool commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Diff,
    Merge,
}

impl ToolKind {
    /// git's command: `difftool` / `mergetool`.
    pub fn command(self) -> &'static str {
        match self {
            ToolKind::Diff => "difftool",
            ToolKind::Merge => "mergetool",
        }
    }

    /// "diff tool" / "merge tool".
    pub fn noun(self) -> &'static str {
        match self {
            ToolKind::Diff => "diff tool",
            ToolKind::Merge => "merge tool",
        }
    }

    /// The config key a chooser writes: `diff.tool` / `merge.tool`.
    pub fn key(self) -> &'static str {
        match self {
            ToolKind::Diff => "diff.tool",
            ToolKind::Merge => "merge.tool",
        }
    }

    /// The keys git reads the tool from, in its order (`get_configured_merge_tool`).
    fn keys(self, gui: bool) -> &'static [&'static str] {
        match (self, gui) {
            (ToolKind::Diff, true) => &["diff.guitool", "merge.guitool", "diff.tool", "merge.tool"],
            (ToolKind::Diff, false) => &["diff.tool", "merge.tool"],
            (ToolKind::Merge, true) => &["merge.guitool", "merge.tool"],
            (ToolKind::Merge, false) => &["merge.tool"],
        }
    }

    /// The config sections of a tool's own settings, in git's lookup order (a diff tool
    /// also takes `mergetool.<name>.cmd` / `.path`).
    fn sections(self) -> &'static [&'static str] {
        match self {
            ToolKind::Diff => &["difftool", "mergetool"],
            ToolKind::Merge => &["mergetool"],
        }
    }
}

/// The tool git runs and where that came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub kind: ToolKind,
    pub name: String,
    /// The config key that chose it (`diff.tool`, `merge.guitool`, …).
    pub key: String,
    /// The user's own command line (`difftool.<name>.cmd` / `mergetool.<name>.cmd`); it
    /// replaces a built-in tool of the same name, as in git.
    pub cmd: Option<String>,
    /// `<section>.<name>.path`: where a built-in tool's program is.
    pub path: Option<String>,
}

impl Tool {
    /// The tool runs inside a terminal (vimdiff & co., Emacs' emerge, a user command
    /// starting vim / nano / …): it can't run without one.
    pub fn needs_terminal(&self) -> bool {
        if let Some(cmd) = &self.cmd {
            return command_needs_terminal(cmd);
        }
        if let Some(path) = &self.path {
            return command_needs_terminal(&format!("\"{path}\""));
        }
        let base = self.name.trim_end_matches(|c: char| c.is_ascii_digit());
        matches!(base, "vimdiff" | "nvimdiff" | "emerge")
    }

    /// "meld (diff.tool)".
    pub fn describe(&self) -> String {
        format!("{} ({})", self.name, self.key)
    }
}

/// Programs that run inside the terminal they are started from.
const TERMINAL_PROGRAMS: &[&str] = &[
    "vim", "nvim", "vi", "vimdiff", "nvimdiff", "view", "nano", "emacs", "kak", "micro", "hx",
    "helix", "less", "more", "most", "mcdiff", "mc",
];

/// Whether a tool's command line starts a terminal program (`vim -d "$LOCAL" "$REMOTE"`,
/// `emacsclient -t …`); leading `VAR=value` words are skipped.
pub fn command_needs_terminal(cmd: &str) -> bool {
    let argv = split_command_line(cmd);
    let mut words = argv.iter().skip_while(|w| {
        w.split_once('=')
            .is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    });
    let Some(program) = words.next() else {
        return false;
    };
    let stem = Path::new(program)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if stem == "emacsclient" {
        return words.any(|a| matches!(a.as_str(), "-t" | "-nw" | "--tty"));
    }
    TERMINAL_PROGRAMS.contains(&stem.as_str())
}

/// The tool settings of git's config: `diff.tool` / `merge.tool` / `*.guitool` and the
/// `difftool.*` / `mergetool.*` sections (one `git config --get-regexp`).
#[derive(Debug, Clone, Default)]
pub struct ToolConfig {
    /// Canonical key (section and name lower-case, a tool's name as written) → value;
    /// the last value of a key wins, as with `git config --get`.
    values: HashMap<String, String>,
}

/// Tests: a folder with `difftool.txt` / `mergetool.txt` holding `--tool-help` output,
/// read by [`Repo::tool_help`] instead of running git (which sources every tool script
/// and looks for its program: seconds).
pub const ENV_TEST_TOOL_HELP: &str = "GITTI_TEST_TOOL_HELP";

/// The `git config --get-regexp` pattern of [`ToolConfig`].
const CONFIG_PATTERN: &str = r"^(diff|merge)\.(gui)?tool$|^(diff|merge)tool\.";

impl ToolConfig {
    /// From `git config -z --get-regexp` output (`key\nvalue\0`).
    pub fn parse(out: &[u8]) -> Self {
        let values = String::from_utf8_lossy(out)
            .split('\0')
            .filter_map(|e| {
                let (k, v) = e.split_once('\n').unwrap_or((e, ""));
                (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
            })
            .collect();
        Self { values }
    }

    /// From `key = value` pairs (tests).
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self {
            values: pairs.iter().map(|(k, v)| (canonical(k), v.to_string())).collect(),
        }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.values.get(&canonical(key)).map(String::as_str)
    }

    /// git's boolean: `None` when unset or not a boolean.
    fn get_bool(&self, key: &str) -> Option<bool> {
        parse_bool(self.get(key)?)
    }

    /// The `<kind>tool.guiDefault` setting: `auto` = a graphical session (`DISPLAY`).
    fn gui_default(&self, kind: ToolKind, display: bool) -> bool {
        let key = format!("{}.guiDefault", kind.command());
        match self.get(&key) {
            Some(v) if v.eq_ignore_ascii_case("auto") => display,
            Some(v) => parse_bool(v).unwrap_or(false),
            None => false,
        }
    }

    /// The tool git would run for `kind` (`None`: none configured — git would guess).
    /// `display`: a graphical session (`DISPLAY` is set), for `guiDefault = auto`.
    pub fn tool(&self, kind: ToolKind, display: bool) -> Option<Tool> {
        let gui = self.gui_default(kind, display);
        let (key, name) = kind.keys(gui).iter().find_map(|k| {
            let v = self.get(k)?.trim();
            (!v.is_empty()).then(|| (k.to_string(), v.to_string()))
        })?;
        Some(Tool {
            key,
            ..self.named(kind, &name)
        })
    }

    /// Tool `name` for `kind` with the user's own settings for it (`.cmd`, `.path`); its
    /// `key` is the one a chooser writes ([`ToolKind::key`]).
    pub fn named(&self, kind: ToolKind, name: &str) -> Tool {
        let own = |what: &str| {
            kind.sections().iter().find_map(|s| {
                self.values
                    .get(&format!("{s}.{name}.{what}"))
                    .filter(|v| !v.is_empty())
                    .cloned()
            })
        };
        Tool {
            kind,
            name: name.to_string(),
            key: kind.key().to_string(),
            cmd: own("cmd"),
            path: own("path"),
        }
    }

    /// The value of `key` (`diff.tool`, …) as git reads it (the last one wins).
    pub fn value(&self, key: &str) -> Option<&str> {
        self.get(key)
    }

    /// The user's own tools for `kind`: the names with a command of their own
    /// (`difftool.<name>.cmd`, `mergetool.<name>.cmd`), sorted.
    pub fn own_tools(&self, kind: ToolKind) -> Vec<String> {
        let names: BTreeSet<String> = self
            .values
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .filter_map(|(k, _)| {
                kind.sections().iter().find_map(|s| {
                    let name = k.strip_prefix(s)?.strip_prefix('.')?.strip_suffix(".cmd")?;
                    (!name.is_empty()).then(|| name.to_string())
                })
            })
            .collect();
        names.into_iter().collect()
    }

    /// `mergetool.keepBackup` (git's default: on).
    pub fn keep_backup(&self) -> bool {
        self.get_bool("mergetool.keepBackup").unwrap_or(true)
    }

    /// `mergetool.keepTemporaries` (git's default: off).
    pub fn keep_temporaries(&self) -> bool {
        self.get_bool("mergetool.keepTemporaries").unwrap_or(false)
    }
}

/// `section.sub.name` with section and name lower-cased (git's canonical form).
fn canonical(key: &str) -> String {
    match (key.find('.'), key.rfind('.')) {
        (Some(a), Some(b)) if a < b => format!(
            "{}{}{}",
            key[..a].to_lowercase(),
            &key[a..b],
            key[b..].to_lowercase()
        ),
        _ => key.to_lowercase(),
    }
}

pub(crate) fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" | "" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// What a set-up hint says when no tool is configured.
pub fn no_tool_message(kind: ToolKind) -> String {
    let (key, cmd) = (kind.key(), kind.command());
    let read = match kind {
        ToolKind::Diff => "diff.tool (else merge.tool)",
        ToolKind::Merge => "merge.tool",
    };
    format!(
        "No external {} is set up: git takes it from {read}. Set one, e.g. \
         `git config --global {key} meld` (`git {cmd} --tool-help` lists the tools git \
         knows and which of them are installed), then try again.",
        kind.noun()
    )
}

/// Why a terminal tool can't run from the app.
pub fn terminal_tool_message(tool: &Tool) -> String {
    format!(
        "The {} {} runs inside a terminal, and git tools are started here without one, so \
         it can't run from this app. Run `git {} -- <file>` in a terminal (gitty, the \
         terminal app, runs it too), or set {} to a tool with its own window: {} lists \
         them.",
        tool.kind.noun(),
        tool.describe(),
        tool.kind.command(),
        tool.key,
        crate::tool_setup::CHOOSE_LABEL,
    )
}

/// Whether this is a graphical session for git's `guiDefault = auto` (git checks
/// `DISPLAY`).
fn graphical_session() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
}

/// What starting an external tool came to ([`Repo::start_diff_tool`],
/// [`Repo::start_merge_tool`]).
#[derive(Debug, Clone)]
pub enum ToolStart {
    /// The tool ran and was closed: what the status line says.
    Done(String),
    /// No tool is set up for it: what the set-up dialog offers. After the choice is
    /// written ([`Repo::write_tool_settings`]) the caller starts again.
    Choose(Box<crate::tool_setup::ToolChoices>),
    /// A tool that runs inside the terminal (the caller said it can hand the terminal
    /// over): run [`TerminalRun::command`] with the terminal, then
    /// [`Repo::finish_terminal_tool`].
    Terminal(Box<TerminalRun>),
}

/// A tool run in the user's terminal: the `git difftool` / `git mergetool` command and
/// what to check afterwards.
#[derive(Clone)]
pub struct TerminalRun {
    pub tool: Tool,
    /// The file it shows / merges.
    pub path: String,
    args: Vec<String>,
    git: crate::git_cli::Git,
    /// A `--no-index` diff (exit code 1 = the files differ).
    no_index: bool,
    merge: Option<MergeWatch>,
}

impl std::fmt::Debug for TerminalRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalRun")
            .field("tool", &self.tool.name)
            .field("args", &self.args)
            .finish()
    }
}

impl TerminalRun {
    /// The command to run with the terminal handed over (the user's locale and
    /// environment, the terminal's stdin / stdout / stderr).
    pub fn command(&self) -> std::process::Command {
        self.git.terminal_command(&self.args)
    }

    /// `git difftool -y --tool=vimdiff -- a.txt`.
    pub fn command_line(&self) -> String {
        format!("git {}", self.args.join(" "))
    }
}

/// What a merge tool run is checked against afterwards: the file, whether its `.orig`
/// backup was there before, git's temporary copies that were there before.
#[derive(Debug, Clone)]
struct MergeWatch {
    path: String,
    file: PathBuf,
    orig: PathBuf,
    had_orig: bool,
    before: BTreeSet<PathBuf>,
    keep_temporaries: bool,
}

/// The two sides of the diff a diff tool shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSides {
    /// Index → working tree (`git difftool`). `untracked`: the file is not in the
    /// index (git's diff wouldn't see it): shown against an empty side.
    Unstaged { untracked: bool },
    /// HEAD → index (`git difftool --cached`).
    Staged,
    /// A commit (or stash) against its first parent (`None`: a root commit — against the
    /// empty tree). A stash's untracked files are in its third parent.
    Commit {
        id: ObjectId,
        parent: Option<ObjectId>,
        stash: bool,
    },
    /// Compare mode: `old` → `new` (either may be the working tree).
    Compare { old: CompareSide, new: CompareSide },
}

/// A file to show in the diff tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffToolTarget {
    pub sides: DiffSides,
    /// The file (its new path).
    pub path: String,
    /// The old path of a rename: both paths are passed, so git pairs them.
    pub old_path: Option<String>,
}

impl DiffSides {
    /// The sides of a commit / stash / comparison view.
    pub fn of_view(src: crate::version_file::DiffSource) -> Self {
        use crate::version_file::DiffSource;
        let side = |c: Option<ObjectId>| c.map_or(CompareSide::Worktree, CompareSide::Commit);
        match src {
            DiffSource::Commit { id, parents, stash } => DiffSides::Commit {
                id,
                parent: parents.first().copied(),
                stash,
            },
            DiffSource::Compare { old, new } => DiffSides::Compare {
                old: side(old),
                new: side(new),
            },
        }
    }
}

impl DiffToolTarget {
    /// `file` of a diff with `sides` (a rename passes both paths).
    pub fn new(sides: DiffSides, file: &crate::diff::FileDiff) -> Self {
        let path = file.path().to_string();
        let old_path = file.old_path.clone().filter(|o| *o != path);
        Self {
            sides,
            path,
            old_path,
        }
    }
}

/// What `git difftool` gets after `--tool=<name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DiffArgs {
    /// `<revs…> -- <paths…>` (pathspecs, literal).
    Revs(Vec<String>),
    /// `--no-index -- <old file> <new file>`.
    NoIndex(String, String),
}

/// The empty side of a `--no-index` diff (git knows the name on every platform).
const NULL_FILE: &str = "/dev/null";

/// The outcome of a merge tool run (the status line's text: [`MergeToolResult::describe`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeToolResult {
    pub path: String,
    pub tool: String,
    /// git mergetool staged the file (no conflict entry left for it).
    pub resolved: bool,
    /// `<path>.orig`, kept by git (`mergetool.keepBackup`).
    pub backup: Option<String>,
    /// Conflicted files left (all files).
    pub conflicts_left: usize,
}

impl MergeToolResult {
    pub fn describe(&self) -> String {
        let mut s = if self.resolved {
            format!("{} resolved {}: git staged it", self.tool, self.path)
        } else {
            format!(
                "{} closed; {} is still conflicted (the tool did not mark it merged)",
                self.tool, self.path
            )
        };
        if let Some(b) = &self.backup {
            s.push_str(&format!(
                ". Kept git's backup {b} (mergetool.keepBackup; untracked)"
            ));
        }
        match self.conflicts_left {
            0 if self.resolved => s.push_str(". No conflicts left: commit / continue"),
            0 => {}
            1 => s.push_str(". 1 conflicted file left"),
            n => s.push_str(&format!(". {n} conflicted files left")),
        }
        s
    }
}

impl Repo {
    /// The tool settings (one git call).
    pub fn tool_config(&self) -> ToolConfig {
        match self
            .git()
            .run_raw(&["config", "-z", "--get-regexp", CONFIG_PATTERN], None)
        {
            Ok(o) if o.code == 0 => ToolConfig::parse(&o.stdout),
            _ => ToolConfig::default(),
        }
    }

    /// The tool git would run for `kind`, or why there is none / it can't run here.
    pub fn external_tool(&self, kind: ToolKind) -> Result<(Tool, ToolConfig)> {
        match self.tool_for(kind, false)? {
            Some(found) => Ok(found),
            None => bail!(no_tool_message(kind)),
        }
    }

    /// The tool git would run for `kind` (`None`: none is set up); a terminal tool is
    /// refused unless the caller can hand it the `terminal`.
    fn tool_for(&self, kind: ToolKind, terminal: bool) -> Result<Option<(Tool, ToolConfig)>> {
        let config = self.tool_config();
        let Some(tool) = config.tool(kind, graphical_session()) else {
            return Ok(None);
        };
        if tool.needs_terminal() && !terminal {
            bail!(terminal_tool_message(&tool));
        }
        Ok(Some((tool, config)))
    }

    /// "Open in External Diff Tool" as the apps run it: `target` in the configured diff
    /// tool ([`ToolStart::Done`] when it is closed); without a tool, what the set-up
    /// dialog offers; a terminal tool for a caller that can hand over the `terminal`, as
    /// a command to run there (else it is refused with the reason).
    pub fn start_diff_tool(&self, target: &DiffToolTarget, terminal: bool) -> Result<ToolStart> {
        let Some((tool, _)) = self.tool_for(ToolKind::Diff, terminal)? else {
            return Ok(ToolStart::Choose(Box::new(self.tool_choices()?)));
        };
        if tool.needs_terminal() {
            let (args, no_index) = self.diff_tool_command(&tool, target)?;
            return Ok(ToolStart::Terminal(Box::new(TerminalRun {
                path: target.path.clone(),
                args,
                git: self.tool_git(),
                no_index,
                merge: None,
                tool,
            })));
        }
        self.run_diff_tool(&tool, target).map(ToolStart::Done)
    }

    /// "Open in External Merge Tool" as the apps run it: conflicted `path` in the
    /// configured merge tool, like [`Repo::start_diff_tool`] (the outcome:
    /// [`MergeToolResult::describe`]).
    pub fn start_merge_tool(&self, path: &str, terminal: bool) -> Result<ToolStart> {
        let Some((tool, config)) = self.tool_for(ToolKind::Merge, terminal)? else {
            return Ok(ToolStart::Choose(Box::new(self.tool_choices()?)));
        };
        if tool.needs_terminal() {
            let watch = self.watch_merge(path, &config)?;
            return Ok(ToolStart::Terminal(Box::new(TerminalRun {
                path: path.to_string(),
                args: merge_tool_args(&tool, path),
                git: self.tool_git(),
                no_index: false,
                merge: Some(watch),
                tool,
            })));
        }
        self.run_merge_tool(&tool, &config, path)
            .map(|r| ToolStart::Done(r.describe()))
    }

    /// After a [`TerminalRun`] (its exit `status`, how long it ran): logs it and says how
    /// it went — for a merge tool whether the file is resolved, the backup git kept and
    /// the conflicts left, as after [`Repo::open_merge_tool`].
    pub fn finish_terminal_tool(
        &self,
        run: &TerminalRun,
        status: std::io::Result<std::process::ExitStatus>,
        took: std::time::Duration,
    ) -> Result<String> {
        let code = match &status {
            Ok(s) => s.code().unwrap_or(-1),
            Err(_) => -1,
        };
        run.git.log_terminal_run(&run.args, took, code);
        if let Err(e) = status {
            bail!("cannot start {}: {e}", run.command_line());
        }
        let tool = &run.tool;
        match &run.merge {
            Some(watch) => {
                let r = self.merge_outcome(watch, tool, code, "")?;
                Ok(r.describe())
            }
            None if code == 0 || (run.no_index && code == 1) => {
                Ok(format!("Diff tool {} closed: {}", tool.name, run.path))
            }
            None => bail!(
                "{} (its messages are in the terminal)",
                tool_failure(tool, "", code)
            ),
        }
    }

    /// git for a tool run: literal pathspecs, the user's locale (the tool's language and its file names),
    /// not the `C` locale gitti reads git's output in.
    fn tool_git(&self) -> crate::git_cli::Git {
        let lc_all = std::env::var("LC_ALL").unwrap_or_default();
        // Pathspecs are file names (also for the git commands the tool scripts run).
        self.git()
            .with_env(&[("LC_ALL", &lc_all), ("GIT_LITERAL_PATHSPECS", "1")])
    }

    /// Opens `target` in the configured diff tool (`git difftool -y --tool=<name> …`);
    /// returns when the tool is closed.
    pub fn open_diff_tool(&self, target: &DiffToolTarget) -> Result<String> {
        let (tool, _) = self.external_tool(ToolKind::Diff)?;
        self.run_diff_tool(&tool, target)
    }

    fn run_diff_tool(&self, tool: &Tool, target: &DiffToolTarget) -> Result<String> {
        let (args, no_index) = self.diff_tool_command(tool, target)?;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.tool_git().run_raw(&args, None)?;
        // `--no-index` diffs exit 1 when the files differ (as `--exit-code`).
        let differ = no_index && out.code == 1 && out.stderr.trim().is_empty();
        if out.code != 0 && !differ {
            bail!(tool_failure(tool, &out.stderr, out.code));
        }
        Ok(format!("Diff tool {} closed: {}", tool.name, target.path))
    }

    /// `git difftool` with `tool` for `target`'s two sides, and whether it is a
    /// `--no-index` diff.
    fn diff_tool_command(&self, tool: &Tool, target: &DiffToolTarget) -> Result<(Vec<String>, bool)> {
        let diff = self.diff_tool_args(target)?;
        let mut args: Vec<String> = vec!["difftool".into(), "-y".into(), format!("--tool={}", tool.name)];
        let no_index = matches!(diff, DiffArgs::NoIndex(..));
        match diff {
            DiffArgs::Revs(revs) => {
                args.extend(revs);
                args.push("--".into());
                args.push(target.path.clone());
                if let Some(old) = target.old_path.as_deref().filter(|o| *o != target.path) {
                    args.push(old.to_string());
                }
            }
            DiffArgs::NoIndex(old, new) => {
                args.extend(["--no-index".into(), "--".into(), old, new]);
            }
        }
        Ok((args, no_index))
    }

    fn diff_tool_args(&self, t: &DiffToolTarget) -> Result<DiffArgs> {
        let hex = |id: &ObjectId| id.to_string();
        Ok(match &t.sides {
            DiffSides::Unstaged { untracked: true } => {
                DiffArgs::NoIndex(NULL_FILE.into(), t.path.clone())
            }
            DiffSides::Unstaged { untracked: false } => DiffArgs::Revs(Vec::new()),
            DiffSides::Staged => DiffArgs::Revs(vec!["--cached".into()]),
            DiffSides::Commit { id, parent, stash } => {
                let old = parent.unwrap_or_else(|| ObjectId::empty_tree(self.object_hash()));
                // A stash's untracked file is only in its third parent.
                let new = if *stash && !self.has_path(*id, &t.path) {
                    format!("{id}^3")
                } else {
                    hex(id)
                };
                DiffArgs::Revs(vec![hex(&old), new])
            }
            DiffSides::Compare { old, new } => match (old, new) {
                (CompareSide::Commit(a), CompareSide::Commit(b)) => {
                    DiffArgs::Revs(vec![hex(a), hex(b)])
                }
                (CompareSide::Worktree, CompareSide::Worktree) => {
                    bail!("both sides are the working tree: nothing to compare")
                }
                (side, CompareSide::Worktree) | (CompareSide::Worktree, side) => {
                    let CompareSide::Commit(c) = *side else {
                        unreachable!("the other side is a commit");
                    };
                    let reverse = matches!(old, CompareSide::Worktree);
                    // A rename: the old path is on the old side.
                    let old_path = t.old_path.as_deref().unwrap_or(&t.path);
                    let (on_disk, in_commit) = if reverse {
                        (old_path, t.path.as_str())
                    } else {
                        (t.path.as_str(), old_path)
                    };
                    if self.in_index(on_disk) {
                        // `git diff <commit>`: the commit → the working tree.
                        let mut revs = vec![hex(&c)];
                        if reverse {
                            revs.insert(0, "-R".into());
                        }
                        DiffArgs::Revs(revs)
                    } else {
                        // An untracked file: its version in the commit (a read-only
                        // copy) or nothing, against the file on disk.
                        let other = match self.export_version(c, in_commit, false) {
                            Ok(copy) => copy.path.to_string_lossy().into_owned(),
                            Err(_) => NULL_FILE.into(),
                        };
                        if reverse {
                            DiffArgs::NoIndex(on_disk.to_string(), other)
                        } else {
                            DiffArgs::NoIndex(other, on_disk.to_string())
                        }
                    }
                }
            },
        })
    }

    /// `path` is in commit `id`'s tree (in-process; the path by its bytes).
    fn has_path(&self, id: ObjectId, path: &str) -> bool {
        let Ok(repo) = self.gix() else {
            return false;
        };
        repo.find_commit(id)
            .ok()
            .and_then(|c| c.tree().ok())
            .is_some_and(|t| self.tree_entry(&t, path).ok().flatten().is_some())
    }

    /// `path` is in the index.
    fn in_index(&self, path: &str) -> bool {
        let spec = format!(":(literal){path}");
        self.git()
            .run_raw(&["ls-files", "-z", "--", &spec], None)
            .is_ok_and(|o| o.code == 0 && !o.stdout.is_empty())
    }

    /// The unmerged paths (`git ls-files -u`).
    fn unmerged_paths(&self) -> Result<BTreeSet<String>> {
        let out = self.git().run(&["ls-files", "-u", "-z"])?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter_map(|e| e.split_once('\t').map(|(_, p)| p.to_string()))
            .collect())
    }

    /// Resolves conflicted `path` in the configured merge tool (`git mergetool -y
    /// --tool=<name> -- <path>`); returns when the tool is closed. If the tool merged
    /// the file, git stages it and keeps `<path>.orig` as `mergetool.keepBackup` says.
    pub fn open_merge_tool(&self, path: &str) -> Result<MergeToolResult> {
        let (tool, config) = self.external_tool(ToolKind::Merge)?;
        self.run_merge_tool(&tool, &config, path)
    }

    fn run_merge_tool(&self, tool: &Tool, config: &ToolConfig, path: &str) -> Result<MergeToolResult> {
        let watch = self.watch_merge(path, config)?;
        let args = merge_tool_args(tool, path);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let run = self.tool_git().run_raw(&args, None);
        if let Err(GitError::Cancelled) = &run {
            clean_up_cancelled_merge(&watch);
            return Err(GitError::Cancelled.into());
        }
        let out = run?;
        self.merge_outcome(&watch, tool, out.code, &out.stderr)
    }

    /// Before a merge tool run on `path`: it must be conflicted; what is there now.
    fn watch_merge(&self, path: &str, config: &ToolConfig) -> Result<MergeWatch> {
        if !self.unmerged_paths()?.contains(path) {
            bail!("{path} has no conflict to merge");
        }
        if self.submodule_conflict(path) {
            bail!(
                "{path} is a submodule whose commit both sides changed: git mergetool would only \
                 ask in a terminal which side's commit to take. Take ours / theirs instead, or \
                 check out the commit you want inside the submodule and mark it resolved."
            );
        }
        // A name that is not UTF-8 by its bytes.
        let file = self.fs_path(path);
        let mut orig = file.clone().into_os_string();
        orig.push(".orig");
        let orig = PathBuf::from(orig);
        Ok(MergeWatch {
            path: path.to_string(),
            had_orig: orig.symlink_metadata().is_ok(),
            before: temp_files(&file),
            orig,
            file,
            keep_temporaries: config.keep_temporaries(),
        })
    }

    /// After a merge tool run that exited with `code`: resolved (git staged it), the
    /// backup git kept, the conflicts left; an error when it failed and the file is still
    /// conflicted.
    fn merge_outcome(&self, w: &MergeWatch, tool: &Tool, code: i32, stderr: &str) -> Result<MergeToolResult> {
        let path = &w.path;
        let left = self.unmerged_paths()?;
        let resolved = !left.contains(path);
        if code != 0 && !resolved {
            bail!(
                "{}; {path} is still conflicted, as it was before",
                tool_failure(tool, stderr, code)
            );
        }
        let backup = (!w.had_orig && w.orig.symlink_metadata().is_ok()).then(|| format!("{path}.orig"));
        Ok(MergeToolResult {
            path: path.clone(),
            tool: tool.name.clone(),
            resolved,
            backup,
            conflicts_left: left.len(),
        })
    }

    /// The tools git knows for `kind` (`git <kind>tool --tool-help`, which checks which
    /// are installed — only when a chooser asks).
    pub fn tool_help(&self, kind: ToolKind) -> Result<ToolHelp> {
        if let Some(dir) = self.git().env_var(ENV_TEST_TOOL_HELP) {
            let file = Path::new(&dir).join(format!("{}.txt", kind.command()));
            let text = std::fs::read_to_string(&file)
                .map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
            return Ok(ToolHelp::parse(&text));
        }
        let out = self.git().run(&[kind.command(), "--tool-help"])?;
        Ok(ToolHelp::parse(&out.stdout_str()))
    }
}

/// `git mergetool -y --tool=<name> -- <path>`.
fn merge_tool_args(tool: &Tool, path: &str) -> Vec<String> {
    vec![
        "mergetool".into(),
        "-y".into(),
        format!("--tool={}", tool.name),
        "--".into(),
        path.to_string(),
    ]
}

/// After a cancelled `git mergetool` (its process group was stopped, so git could not
/// clean up): what git does when a tool fails — the file put back from git's backup
/// copy, the copies of the sides removed unless `mergetool.keepTemporaries`.
fn clean_up_cancelled_merge(w: &MergeWatch) {
    let new: Vec<PathBuf> = temp_files(&w.file).difference(&w.before).cloned().collect();
    let is = |p: &Path, role: &str| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().contains(&format!("_{role}_")))
    };
    let backups: Vec<&PathBuf> = new.iter().filter(|p| is(p, "BACKUP")).collect();
    if let [backup] = backups.as_slice() {
        let _ = std::fs::rename(backup, &w.file);
    }
    if !w.keep_temporaries {
        for p in new.iter().filter(|p| !is(p, "BACKUP")) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The error text of a failed tool run: git's message, plus what to do when the tool's
/// program isn't there.
fn tool_failure(tool: &Tool, stderr: &str, code: i32) -> String {
    let msg = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let msg = if msg.is_empty() {
        format!("git {} exited with code {code}", tool.kind.command())
    } else {
        msg
    };
    let (name, key, command) = (&tool.name, &tool.key, tool.kind.command());
    if msg.contains("is not available as") {
        format!(
            "{msg}\nInstall {name}, point {command}.{name}.path at its program, or choose \
             another tool in {key}."
        )
    } else if msg.contains("not set for tool") || msg.contains("Unknown ") {
        format!(
            "{msg}\n{name} ({key}) is neither a tool git knows nor one with a command of \
             your own ({command}.{name}.cmd): check the name (`git {command} --tool-help` \
             lists the tools)."
        )
    } else {
        msg
    }
}

/// The copies `git mergetool` writes next to `file`: `<name>_BACKUP_<pid><.ext>`,
/// `_LOCAL_`, `_REMOTE_`, `_BASE_`.
fn temp_files(file: &Path) -> BTreeSet<PathBuf> {
    let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else {
        return BTreeSet::new();
    };
    let name = name.to_string_lossy();
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (&name[..], ""),
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            ["BACKUP", "LOCAL", "REMOTE", "BASE"].iter().any(|role| {
                n.strip_prefix(&format!("{stem}_{role}_"))
                    .and_then(|rest| rest.strip_suffix(ext))
                    .is_some_and(|pid| !pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit()))
            })
        })
        .map(|e| e.path())
        .collect()
}

/// One tool in `--tool-help`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownTool {
    pub name: String,
    /// git's words ("Use Meld (requires a graphical session)"); empty on older gits.
    pub description: String,
}

impl KnownTool {
    /// git says it needs a terminal — or, on gits that don't say, a known terminal tool.
    pub fn needs_terminal(&self) -> bool {
        Tool {
            kind: ToolKind::Diff,
            name: self.name.clone(),
            key: String::new(),
            cmd: None,
            path: None,
        }
        .needs_terminal()
            && !self.description.contains("graphical session")
    }
}

/// `git difftool --tool-help` / `git mergetool --tool-help`, parsed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolHelp {
    /// Built-in tools whose program is installed.
    pub available: Vec<KnownTool>,
    /// The user's own tools (`difftool.<name>.cmd` / `mergetool.<name>.cmd`).
    pub user_defined: Vec<String>,
    /// Built-in tools git knows but can't find.
    pub unavailable: Vec<KnownTool>,
}

impl ToolHelp {
    pub fn parse(text: &str) -> Self {
        #[derive(PartialEq)]
        enum Part {
            None,
            Available,
            User,
            Unavailable,
        }
        let mut part = Part::None;
        let mut help = ToolHelp::default();
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.contains("may be set to one of the following") {
                part = Part::Available;
                continue;
            }
            if t == "user-defined:" {
                part = Part::User;
                continue;
            }
            if t.starts_with("The following tools are valid, but not currently available") {
                part = Part::Unavailable;
                continue;
            }
            if !line.starts_with('\t') {
                // "No suitable tool…", "Some of the tools listed above…".
                part = Part::None;
                continue;
            }
            let (name, desc) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
            let tool = KnownTool {
                name: name.to_string(),
                description: desc.trim().to_string(),
            };
            match part {
                Part::Available => help.available.push(tool),
                Part::Unavailable => help.unavailable.push(tool),
                // `<name>.cmd <command>` (git prints the config line), or the name.
                Part::User => {
                    let name = t
                        .split_once(".cmd")
                        .map(|(n, _)| n)
                        .unwrap_or(name)
                        .to_string();
                    if !help.user_defined.contains(&name) {
                        help.user_defined.push(name);
                    }
                }
                Part::None => {}
            }
        }
        help
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn resolves_the_tool_like_git() {
        let c = ToolConfig::from_pairs(&[("merge.tool", "kdiff3")]);
        let t = c.tool(ToolKind::Diff, false).unwrap();
        assert_eq!((t.name.as_str(), t.key.as_str()), ("kdiff3", "merge.tool"));
        assert!(ToolConfig::default().tool(ToolKind::Diff, false).is_none());
        // diff.tool first; the merge tool never takes diff.tool.
        let c = ToolConfig::from_pairs(&[("diff.tool", "meld"), ("merge.tool", "kdiff3")]);
        assert_eq!(c.tool(ToolKind::Diff, false).unwrap().name, "meld");
        assert_eq!(c.tool(ToolKind::Merge, false).unwrap().name, "kdiff3");
        let c = ToolConfig::from_pairs(&[("diff.tool", "meld")]);
        assert!(c.tool(ToolKind::Merge, false).is_none());
        // guiDefault: the gui tool first; `auto` = a display.
        let c = ToolConfig::from_pairs(&[
            ("diff.tool", "vimdiff"),
            ("diff.guitool", "meld"),
            ("difftool.guiDefault", "auto"),
        ]);
        assert_eq!(c.tool(ToolKind::Diff, false).unwrap().name, "vimdiff");
        assert_eq!(c.tool(ToolKind::Diff, true).unwrap().name, "meld");
        let c = ToolConfig::from_pairs(&[
            ("merge.tool", "vimdiff"),
            ("merge.guitool", "meld"),
            ("mergetool.guiDefault", "true"),
        ]);
        assert_eq!(c.tool(ToolKind::Merge, false).unwrap().key, "merge.guitool");
        // A diff tool takes mergetool.<name>.cmd too; names keep their case.
        let c = ToolConfig::parse(b"diff.tool\nMyTool\0mergetool.MyTool.cmd\nvim -d \"$LOCAL\"\0");
        let t = c.tool(ToolKind::Diff, false).unwrap();
        assert_eq!(t.cmd.as_deref(), Some("vim -d \"$LOCAL\""));
        assert!(t.needs_terminal());
        assert!(c.keep_backup() && !c.keep_temporaries());
        assert!(!ToolConfig::from_pairs(&[("mergetool.keepBackup", "false")]).keep_backup());
    }

    #[gitti_test::test]
    fn terminal_tools() {
        let tool = |name: &str, cmd: Option<&str>, path: Option<&str>| Tool {
            kind: ToolKind::Merge,
            name: name.into(),
            key: "merge.tool".into(),
            cmd: cmd.map(Into::into),
            path: path.map(Into::into),
        };
        for t in ["vimdiff", "nvimdiff", "vimdiff3", "nvimdiff1", "emerge"] {
            assert!(tool(t, None, None).needs_terminal(), "{t}");
        }
        for t in ["meld", "gvimdiff", "opendiff", "kdiff3", "vscode", "bc3"] {
            assert!(!tool(t, None, None).needs_terminal(), "{t}");
        }
        // The user's command / path decide.
        assert!(!tool("vimdiff", Some("meld \"$LOCAL\" \"$REMOTE\""), None).needs_terminal());
        assert!(tool("x", Some("TERM=xterm nvim -d \"$LOCAL\" \"$REMOTE\""), None).needs_terminal());
        assert!(!tool("vimdiff", None, Some("/usr/bin/gvim")).needs_terminal());
        assert!(tool("x", Some("emacsclient -t --eval x"), None).needs_terminal());
        assert!(!tool("x", Some("emacsclient -c --eval x"), None).needs_terminal());
        assert!(!tool("x", Some("/Applications/My Tool.app/x \"$LOCAL\""), None).needs_terminal());
    }

    #[gitti_test::test]
    fn parses_tool_help() {
        let text = "'git difftool --tool=<tool>' may be set to one of the following:\n\
            \t\tnvimdiff         Use Neovim\n\
            \t\topendiff         Use FileMerge (requires a graphical session)\n\
            \n\
            \tuser-defined:\n\
            \t\tmyt.cmd true\n\
            \n\
            The following tools are valid, but not currently available:\n\
            \t\tmeld             Use Meld (requires a graphical session)\n\
            \t\temerge\n\
            \n\
            Some of the tools listed above only work in a windowed\n\
            environment. If run in a terminal-only session, they will fail.\n";
        let h = ToolHelp::parse(text);
        let names = |v: &[KnownTool]| v.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&h.available), ["nvimdiff", "opendiff"]);
        assert_eq!(h.user_defined, ["myt"]);
        assert_eq!(names(&h.unavailable), ["meld", "emerge"]);
        assert!(h.available[0].needs_terminal() && !h.available[1].needs_terminal());
        assert_eq!(h.available[1].description, "Use FileMerge (requires a graphical session)");
        let none = ToolHelp::parse("'git mergetool --tool=<tool>' may be set to one of the following:\nNo suitable tool for 'git mergetool --tool=<tool>' found.\n");
        assert!(none.available.is_empty());
    }

    #[gitti_test::test]
    fn finds_mergetool_copies() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.b.txt");
        for n in ["a.b_LOCAL_123.txt", "a.b_BACKUP_9.txt", "a.b_LOCAL_x.txt", "a.b.txt", "c_BASE_1.txt"] {
            std::fs::write(dir.path().join(n), "").unwrap();
        }
        let found: Vec<String> = temp_files(&file)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(found, ["a.b_BACKUP_9.txt", "a.b_LOCAL_123.txt"]);
    }

    /// Workflow: MG-24
    #[gitti_test::test]
    fn describes_the_merge_outcome() {
        let r = MergeToolResult {
            path: "a.txt".into(),
            tool: "meld".into(),
            resolved: true,
            backup: Some("a.txt.orig".into()),
            conflicts_left: 0,
        };
        let s = r.describe();
        assert!(s.starts_with("meld resolved a.txt: git staged it"), "{s}");
        assert!(s.contains("backup a.txt.orig") && s.ends_with("commit / continue"), "{s}");
        let r = MergeToolResult {
            resolved: false,
            backup: None,
            conflicts_left: 2,
            ..r
        };
        assert!(r.describe().contains("still conflicted") && r.describe().ends_with("2 conflicted files left"));
    }
}
