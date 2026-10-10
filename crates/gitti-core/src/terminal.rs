//! "Open in Terminal": the terminal emulators gitti and gitty know, which of them are
//! installed, which one is the user's default, and the command line that opens each of
//! them in a folder.
//!
//! There is no single "default terminal" setting on most systems, so the automatic choice
//! (an empty setting `terminal_command`) is, best first:
//! 1. the terminal the app was started from (gitty runs inside one; gitti started from a
//!    shell): `$TERM`, `$TERM_PROGRAM`, the terminals' own variables
//!    (`$KITTY_WINDOW_ID`, `$WEZTERM_PANE`, `$WT_SESSION`, …), macOS `__CFBundleIdentifier`;
//! 2. the desktop's default: macOS the handler the user set for shell scripts
//!    (`public.unix-executable`, e.g. iTerm2's "Make iTerm2 Default Term"), else
//!    Terminal; Linux `$TERMINAL`, the xdg-terminals list (`xdg-terminal-exec`), the
//!    desktop's setting (KDE `kdeglobals`, GNOME / Cinnamon / MATE `gsettings`, Xfce
//!    `helpers.rc`), then Debian's `x-terminal-emulator`; Windows the "default terminal
//!    application" (`HKCU\Console\%%Startup`): Windows Terminal unless the console host
//!    is chosen;
//! 3. the first installed terminal in the platform's order.
//!
//! Detection reads the environment and the file system, plus one short query where a
//! setting lives in no file (macOS `plutil`, `gsettings`, Windows `reg`) — all through
//! [`Host`], so tests describe a machine instead of using the real one. A chosen
//! terminal is stored as its command line (the setting stays a plain command line that
//! every version understands).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use crate::ops::split_command_line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

/// How a preset's terminal is found.
#[derive(Debug, Clone, Copy)]
pub enum Find {
    /// A macOS app bundle (`Ghostty.app`) in the usual application folders; `{exe}` =
    /// `Contents/MacOS/<exe>` inside it.
    App {
        bundle: &'static str,
        exe: &'static str,
    },
    /// A program on `PATH` (Windows: also with `.exe`); `{exe}` = the name.
    Program(&'static str),
    /// Windows: Git for Windows' `git-bash.exe`, next to the `git` on `PATH`.
    GitBash,
}

/// How to tell that the app runs inside a terminal (see [`Detection::started_from`]),
/// most telling first: `TERM` is set by the innermost terminal, `TERM_PROGRAM` by many
/// (but `tmux` overwrites it), the other variables are inherited from outer terminals.
#[derive(Debug, Clone, Copy)]
pub enum Mark {
    /// `$TERM` is this or starts with `this-` (`foot-extra`).
    Term(&'static str),
    /// `$TERM_PROGRAM` is this (case-insensitive).
    Program(&'static str),
    /// This variable is set.
    Var(&'static str),
}

/// A terminal gitti knows how to open in a folder.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    /// Stable key ("ghostty").
    pub id: &'static str,
    pub name: &'static str,
    pub platform: Platform,
    pub find: Find,
    /// The command line: `{dir}` = the folder (one argument however it is spelt); it
    /// also runs in that folder. `{exe}` = the program [`Find`] found (quoted as needed).
    pub command: &'static str,
    pub marks: &'static [Mark],
    /// macOS bundle ids / Linux desktop entry ids (xdg-terminals list, KDE's
    /// `TerminalService`).
    pub ids: &'static [&'static str],
    /// A variant of another preset (a new tab instead of a window): listed when that one
    /// is installed, never the automatic choice.
    pub variant: bool,
}

const fn preset(
    id: &'static str,
    name: &'static str,
    platform: Platform,
    find: Find,
    command: &'static str,
    marks: &'static [Mark],
    ids: &'static [&'static str],
) -> Preset {
    Preset {
        id,
        name,
        platform,
        find,
        command,
        marks,
        ids,
        variant: false,
    }
}

const fn app(bundle: &'static str, exe: &'static str) -> Find {
    Find::App { bundle, exe }
}

use Find::Program as P;
use Mark::{Program as TP, Term as T, Var as V};
use Platform::{Linux as L, Mac as M, Windows as W};

/// The known terminals, per platform in the order of the automatic fallback.
#[rustfmt::skip]
pub const PRESETS: &[Preset] = &[
    // ---- macOS: `open -a` hands the folder to the running app (a new window there);
    // terminals without that get their own command-line option.
    preset("terminal", "Terminal", M, app("Terminal.app", "Terminal"), "open -a Terminal {dir}",
        &[TP("Apple_Terminal")], &["com.apple.Terminal"]),
    preset("ghostty", "Ghostty", M, app("Ghostty.app", "ghostty"), "open -a Ghostty {dir}",
        &[T("xterm-ghostty"), TP("ghostty"), V("GHOSTTY_RESOURCES_DIR")], &["com.mitchellh.ghostty"]),
    preset("iterm2", "iTerm2", M, app("iTerm.app", "iTerm2"), "open -a iTerm {dir}",
        &[TP("iTerm.app"), V("ITERM_SESSION_ID")], &["com.googlecode.iterm2"]),
    preset("kitty", "kitty", M, app("kitty.app", "kitty"), "{exe} --single-instance --directory {dir}",
        &[T("xterm-kitty"), V("KITTY_WINDOW_ID")], &["net.kovidgoyal.kitty"]),
    preset("wezterm", "WezTerm", M, app("WezTerm.app", "wezterm"), "{exe} start --cwd {dir}",
        &[T("wezterm"), TP("WezTerm"), V("WEZTERM_PANE")], &["com.github.wez.wezterm"]),
    preset("alacritty", "Alacritty", M, app("Alacritty.app", "alacritty"), "{exe} --working-directory {dir}",
        &[T("alacritty"), V("ALACRITTY_WINDOW_ID")], &["org.alacritty", "io.alacritty"]),
    preset("warp", "Warp", M, app("Warp.app", "stable"), "open -a Warp {dir}",
        &[TP("WarpTerminal")], &["dev.warp.Warp-Stable"]),
    preset("rio", "Rio", M, app("Rio.app", "rio"), "{exe} --working-dir {dir}",
        &[TP("rio")], &["com.raphaelamorim.rio"]),
    preset("hyper", "Hyper", M, app("Hyper.app", "Hyper"), "open -a Hyper {dir}",
        &[TP("Hyper")], &["co.zeit.hyper"]),
    preset("tabby", "Tabby", M, app("Tabby.app", "Tabby"), "open -a Tabby {dir}",
        &[TP("Tabby")], &["org.tabby"]),
    // ---- Linux: each with its working-directory option (the folder is also the cwd,
    // which the others use).
    preset("gnome-terminal", "GNOME Terminal", L, P("gnome-terminal"), "gnome-terminal --working-directory={dir}",
        &[V("GNOME_TERMINAL_SCREEN")], &["org.gnome.Terminal.desktop"]),
    preset("ptyxis", "Ptyxis", L, P("ptyxis"), "ptyxis --new-window --working-directory={dir}",
        &[V("PTYXIS_VERSION")], &["org.gnome.Ptyxis.desktop"]),
    preset("kgx", "GNOME Console", L, P("kgx"), "kgx --working-directory={dir}",
        &[TP("kgx")], &["org.gnome.Console.desktop"]),
    preset("konsole", "Konsole", L, P("konsole"), "konsole --workdir {dir}",
        &[V("KONSOLE_VERSION")], &["org.kde.konsole.desktop", "konsole.desktop"]),
    preset("xfce4-terminal", "Xfce Terminal", L, P("xfce4-terminal"), "xfce4-terminal --working-directory={dir}",
        &[], &["xfce4-terminal.desktop", "org.xfce.terminal.desktop"]),
    preset("ghostty", "Ghostty", L, P("ghostty"), "ghostty --working-directory={dir}",
        &[T("xterm-ghostty"), TP("ghostty"), V("GHOSTTY_RESOURCES_DIR")], &["com.mitchellh.ghostty.desktop"]),
    preset("kitty", "kitty", L, P("kitty"), "kitty --directory {dir}",
        &[T("xterm-kitty"), V("KITTY_WINDOW_ID")], &["kitty.desktop"]),
    preset("wezterm", "WezTerm", L, P("wezterm"), "wezterm start --cwd {dir}",
        &[T("wezterm"), TP("WezTerm"), V("WEZTERM_PANE")], &["org.wezfurlong.wezterm.desktop"]),
    preset("alacritty", "Alacritty", L, P("alacritty"), "alacritty --working-directory {dir}",
        &[T("alacritty"), V("ALACRITTY_WINDOW_ID")], &["Alacritty.desktop", "alacritty.desktop"]),
    preset("foot", "foot", L, P("foot"), "foot --working-directory={dir}",
        &[T("foot")], &["foot.desktop", "org.codeberg.dnkl.foot.desktop"]),
    preset("tilix", "Tilix", L, P("tilix"), "tilix --working-directory={dir}",
        &[V("TILIX_ID")], &["com.gexperts.Tilix.desktop"]),
    preset("terminator", "Terminator", L, P("terminator"), "terminator --working-directory={dir}",
        &[V("TERMINATOR_UUID")], &["terminator.desktop"]),
    preset("mate-terminal", "MATE Terminal", L, P("mate-terminal"), "mate-terminal --working-directory={dir}",
        &[], &["mate-terminal.desktop"]),
    preset("lxterminal", "LXTerminal", L, P("lxterminal"), "lxterminal --working-directory={dir}",
        &[], &["lxterminal.desktop"]),
    preset("rio", "Rio", L, P("rio"), "rio --working-dir {dir}",
        &[TP("rio")], &["rio.desktop"]),
    preset("warp", "Warp", L, P("warp-terminal"), "warp-terminal",
        &[TP("WarpTerminal")], &["dev.warp.Warp.desktop"]),
    preset("xterm", "xterm", L, P("xterm"), "xterm",
        &[V("XTERM_VERSION")], &["xterm.desktop", "debian-xterm.desktop"]),
    // ---- Windows: `start` gives console programs a window of their own (in the default
    // terminal application); `-d .` is the folder for Windows Terminal (a `;` in a path
    // would split its command line).
    preset("windows-terminal", "Windows Terminal", W, P("wt"), "wt.exe -d .",
        &[V("WT_SESSION")], &[]),
    Preset { variant: true, ..preset("windows-terminal-tab", "Windows Terminal (new tab)", W,
        P("wt"), "wt.exe -w 0 new-tab -d .", &[], &[]) },
    preset("pwsh", "PowerShell", W, P("pwsh"), "cmd.exe /c start \"\" pwsh.exe -NoLogo",
        &[], &[]),
    preset("powershell", "Windows PowerShell", W, P("powershell"),
        "cmd.exe /c start \"\" powershell.exe -NoLogo", &[], &[]),
    preset("cmd", "Command Prompt", W, P("cmd"), "cmd.exe /c start \"\" cmd.exe /k", &[], &[]),
    preset("git-bash", "Git Bash", W, Find::GitBash, "{exe} --cd={dir}", &[], &[]),
    preset("wezterm", "WezTerm", W, P("wezterm"), "wezterm start --cwd {dir}",
        &[T("wezterm"), TP("WezTerm"), V("WEZTERM_PANE")], &[]),
    preset("alacritty", "Alacritty", W, P("alacritty"), "alacritty --working-directory {dir}",
        &[T("alacritty"), V("ALACRITTY_WINDOW_ID")], &[]),
];

/// The WezTerm variant that opens a tab in the running WezTerm (all platforms).
const WEZTERM_TAB: &str = "{exe} cli spawn --cwd {dir}";

/// What detection needs from the machine (faked in tests).
pub trait Host {
    fn platform(&self) -> Platform;
    /// An environment variable, `None` when unset or blank.
    fn var(&self, name: &str) -> Option<String>;
    /// A program on `PATH` (or at a path).
    fn find_program(&self, name: &str) -> Option<PathBuf>;
    fn exists(&self, path: &Path) -> bool;
    fn read_file(&self, path: &Path) -> Option<String>;
    /// Where a link (chain) points.
    fn canonicalize(&self, path: &Path) -> Option<PathBuf>;
    /// The output of a short query command (`None` if it failed or doesn't exist).
    fn query(&self, argv: &[&str]) -> Option<String>;
}

/// This machine.
pub struct SystemHost;

impl Host for SystemHost {
    fn platform(&self) -> Platform {
        Platform::current()
    }

    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.trim().is_empty())
    }

    fn find_program(&self, name: &str) -> Option<PathBuf> {
        find_on_path(name, std::env::var_os("PATH").as_deref())
    }

    fn exists(&self, path: &Path) -> bool {
        path.symlink_metadata().is_ok()
    }

    fn read_file(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    fn query(&self, argv: &[&str]) -> Option<String> {
        let (program, args) = argv.split_first()?;
        let mut cmd = std::process::Command::new(program);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// A program on `path` (the `PATH` value), or at a path. On Windows also with `.exe` /
/// `.cmd` / `.bat`, and app execution aliases (`WindowsApps\wt.exe`, links whose
/// metadata can't be read) count.
pub fn find_on_path(name: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let p = crate::create::expand_home(name);
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    let found = |base: &Path| {
        exts.iter().find_map(|e| {
            let c = PathBuf::from(format!("{}{e}", base.display()));
            let m = c.symlink_metadata().ok()?;
            let file = if m.file_type().is_symlink() {
                // A dangling link is no program; an unreadable one (app alias) may be.
                c.metadata().map(|m| m.is_file()).unwrap_or(cfg!(windows))
            } else {
                m.is_file()
            };
            file.then_some(c)
        })
    };
    if p.components().count() > 1 || p.is_absolute() {
        return found(&p);
    }
    std::env::split_paths(path?).find_map(|dir| found(&dir.join(&p)))
}

/// A terminal as offered in the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The preset id, or the launcher (`$TERMINAL`, `xdg-terminal-exec`,
    /// `x-terminal-emulator`).
    pub id: String,
    pub name: String,
    /// The command line to store (`{dir}` = the folder).
    pub command: String,
    pub installed: bool,
    /// Why it is special ("started from it", "macOS default", "$TERMINAL", …).
    pub note: Option<String>,
}

/// The terminals of this machine and what "Automatic" opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub platform: Platform,
    /// Installed ones first (the automatic choice first), then the known ones not found.
    pub entries: Vec<Entry>,
    /// Index into `entries`: what an empty setting opens (`None`: nothing found).
    pub auto: Option<usize>,
    /// Why that one, in words ("gitti was started from it" → "started from it").
    pub auto_why: String,
}

/// Where a choice came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The environment variable `GITTI_TERMINAL`.
    Override,
    /// The setting.
    Setting,
    /// Detected (the setting is empty).
    Auto,
}

/// The terminal "Open in Terminal" will start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// "Ghostty", or the program of a custom command.
    pub name: String,
    pub command: String,
    pub source: Source,
    /// Why the automatic choice is this one.
    pub why: String,
}

impl Choice {
    /// Program and arguments for `dir` (`{dir}` replaced inside the arguments).
    pub fn argv(&self, dir: &Path) -> Vec<String> {
        command_argv(&self.command, dir)
    }
}

/// `command` split into program and arguments, `{dir}` replaced by `dir` (spaces and
/// quotes in it stay one argument).
pub fn command_argv(command: &str, dir: &Path) -> Vec<String> {
    let d = dir.to_string_lossy();
    split_command_line(command)
        .into_iter()
        .map(|a| a.replace("{dir}", &d))
        .collect()
}

/// `s` as one word of a command line ([`split_command_line`] gives it back).
pub fn quote_arg(s: &str) -> String {
    quote_arg_for(s, cfg!(windows))
}

fn quote_arg_for(s: &str, windows: bool) -> String {
    let plain = !s.is_empty()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '$' | '`'));
    if plain || (windows && !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'')) {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        if c == '"' || (c == '\\' && !windows) {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

impl Detection {
    /// What "Open in Terminal" starts: `GITTI_TERMINAL` (`env_override`), else the setting,
    /// else the automatic choice.
    pub fn choose(&self, setting: &str, env_override: Option<&str>) -> Result<Choice> {
        let custom = |command: &str, source| {
            let command = command.trim().to_string();
            Choice {
                name: self.name_of(&command),
                command,
                source,
                why: String::new(),
            }
        };
        if let Some(env) = env_override.filter(|e| !e.trim().is_empty()) {
            return Ok(custom(env, Source::Override));
        }
        if !setting.trim().is_empty() {
            return Ok(custom(setting, Source::Setting));
        }
        match self.auto.and_then(|i| self.entries.get(i)) {
            Some(e) => Ok(Choice {
                name: e.name.clone(),
                command: e.command.clone(),
                source: Source::Auto,
                why: self.auto_why.clone(),
            }),
            None => bail!(
                "No terminal found (looked for {}). Install one, or enter the command that \
                 opens yours in a folder — gitti: Settings ▸ Diff ▸ Terminal, gitty: palette ▸ \
                 Choose Terminal… — e.g. `kitty --directory {{dir}}`.",
                self.entries
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// The entry a stored command line is (exactly) — the chooser shows its name.
    pub fn entry_for(&self, command: &str) -> Option<&Entry> {
        let c = command.trim();
        (!c.is_empty())
            .then(|| self.entries.iter().find(|e| e.command == c))
            .flatten()
    }

    /// A short name for a command line: the entry's, else its program's file name.
    pub fn name_of(&self, command: &str) -> String {
        if let Some(e) = self.entry_for(command) {
            return e.name.clone();
        }
        let argv = split_command_line(command);
        let program = argv.first().map(String::as_str).unwrap_or("");
        // `open -a Foo …` (macOS): the app.
        if program == "open"
            && let Some(i) = argv.iter().position(|a| a == "-a" || a == "-na")
            && let Some(app) = argv.get(i + 1)
        {
            return app.trim_end_matches(".app").to_string();
        }
        Path::new(program)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| program.to_string())
    }

    pub fn installed(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.installed)
    }

    /// The automatic choice's entry.
    pub fn auto_entry(&self) -> Option<&Entry> {
        self.auto.and_then(|i| self.entries.get(i))
    }

    /// How the settings show the setting `command`: a short label for the chooser
    /// ("Automatic (Ghostty)", "Ghostty", "Custom (myterm)") and what it runs, in words.
    pub fn describe(&self, command: &str) -> (String, String) {
        let command = command.trim();
        if command.is_empty() {
            return match self.auto_entry() {
                Some(e) => (
                    format!("Automatic ({})", e.name),
                    format!(
                        "Automatic: {} ({}) — runs {}",
                        e.name, self.auto_why, e.command
                    ),
                ),
                None => (
                    "Automatic (none found)".to_string(),
                    "Automatic: no terminal found on this computer.".to_string(),
                ),
            };
        }
        match self.entry_for(command) {
            Some(e) => (e.name.clone(), format!("{} — runs {command}", e.name)),
            None => (
                format!("Custom ({})", self.name_of(command)),
                format!("Custom — runs {command}"),
            ),
        }
    }

    /// What is wrong with the setting `command` on this machine (`host`), in words with
    /// what to do: its program is missing, or nothing was found for "Automatic".
    pub fn problem(&self, command: &str, host: &dyn Host) -> Option<String> {
        if command.trim().is_empty() {
            return self.auto.is_none().then(|| {
                "No terminal found: install one, or enter the command that opens yours."
                    .to_string()
            });
        }
        problem(command, host)
    }
}

/// Why `command` can't start a terminal on this machine, in words with what to do
/// (`None`: its program is there). An empty command is the automatic choice: fine.
pub fn problem(command: &str, host: &dyn Host) -> Option<String> {
    let argv = split_command_line(command);
    let program = argv.first()?;
    let todo = "install it, or choose another terminal";
    if host.platform() == Platform::Mac && program == "open" {
        let i = argv.iter().position(|a| a == "-a" || a == "-na")?;
        let app = argv.get(i + 1)?;
        let bundle = if app.ends_with(".app") {
            app.clone()
        } else {
            format!("{app}.app")
        };
        if Path::new(app).is_absolute() {
            return (!host.exists(Path::new(app)))
                .then(|| format!("{app} does not exist — {todo}."));
        }
        return find_app(host, &bundle).is_none().then(|| {
            format!("{bundle} is not in Applications (/Applications, ~/Applications) — {todo}.")
        });
    }
    if host.find_program(program).is_some() {
        return None;
    }
    Some(if Path::new(program).is_absolute() || program.contains(['/', '\\']) {
        format!("{program} does not exist — {todo}.")
    } else {
        format!("`{program}` is not installed (not found on PATH) — {todo}.")
    })
}

/// The application folders searched on macOS.
fn app_dirs(host: &dyn Host) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
        "/System/Applications/Utilities",
        "/Applications/Setapp",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = host.var("HOME") {
        dirs.push(Path::new(&home).join("Applications"));
    }
    dirs
}

fn find_app(host: &dyn Host, bundle: &str) -> Option<PathBuf> {
    app_dirs(host)
        .into_iter()
        .map(|d| d.join(bundle))
        .find(|p| host.exists(p))
}

/// Where a preset's program is (`None`: not installed).
fn locate(host: &dyn Host, p: &Preset) -> Option<PathBuf> {
    match p.find {
        Find::App { bundle, exe } => {
            find_app(host, bundle).map(|b| b.join("Contents").join("MacOS").join(exe))
        }
        Find::Program(name) => host.find_program(name),
        Find::GitBash => {
            let git = host.find_program("git")?;
            // `…\Git\cmd\git.exe` or `…\Git\mingw64\bin\git.exe` → `…\Git\git-bash.exe`.
            git.ancestors()
                .skip(1)
                .take(3)
                .map(|d| d.join("git-bash.exe"))
                .find(|p| host.exists(p))
        }
    }
}

fn fill(command: &str, exe: Option<&Path>, fallback: &str) -> String {
    let exe = exe
        .map(|e| quote_arg(&e.to_string_lossy()))
        .unwrap_or_else(|| fallback.to_string());
    command.replace("{exe}", &exe)
}

fn exe_name(p: &Preset) -> &'static str {
    match p.find {
        Find::App { exe, .. } => exe,
        Find::Program(name) => name,
        Find::GitBash => "git-bash.exe",
    }
}

/// The program's name without path, extension and Debian's `.wrapper`.
fn program_stem(program: &str) -> String {
    let name = Path::new(program.trim())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = name.strip_suffix(".wrapper").unwrap_or(&name);
    let name = name.strip_suffix(".exe").unwrap_or(name);
    name.to_string()
}

/// Detects the terminals of this machine (a fresh look: something may have been
/// installed meanwhile). See [`detect_with`].
pub fn detect() -> Detection {
    detect_with(&SystemHost)
}

static CACHE: Mutex<Option<Arc<Detection>>> = Mutex::new(None);

/// The last detection (detected now if there is none) — for display.
pub fn cached() -> Arc<Detection> {
    if let Some(d) = CACHE.lock().ok().and_then(|c| c.clone()) {
        return d;
    }
    redetect()
}

/// Detects again and remembers the result for [`cached`].
pub fn redetect() -> Arc<Detection> {
    let d = Arc::new(detect());
    if let Ok(mut c) = CACHE.lock() {
        *c = Some(d.clone());
    }
    d
}

/// The terminals of the machine `host` describes and the automatic choice.
pub fn detect_with(host: &dyn Host) -> Detection {
    let platform = host.platform();
    let presets: Vec<&Preset> = PRESETS.iter().filter(|p| p.platform == platform).collect();
    // (entry, preset) — preset entries in table order.
    let mut entries: Vec<(Entry, Option<&Preset>)> = Vec::new();
    for p in &presets {
        let exe = locate(host, p);
        entries.push((
            Entry {
                id: p.id.to_string(),
                name: p.name.to_string(),
                command: fill(p.command, exe.as_deref(), exe_name(p)),
                installed: exe.is_some(),
                note: None,
            },
            Some(*p),
        ));
        // WezTerm: also a new tab in the running WezTerm (listed when installed).
        if p.id == "wezterm" && exe.is_some() {
            entries.push((
                Entry {
                    id: "wezterm-tab".to_string(),
                    name: "WezTerm (new tab)".to_string(),
                    command: fill(WEZTERM_TAB, exe.as_deref(), "wezterm"),
                    installed: true,
                    note: None,
                },
                None,
            ));
        }
    }
    let usable = |i: usize, entries: &[(Entry, Option<&Preset>)]| {
        entries[i].0.installed && entries[i].1.is_some_and(|p| !p.variant)
    };
    let by_id = |id: &str, entries: &[(Entry, Option<&Preset>)]| {
        (0..entries.len()).find(|&i| {
            usable(i, entries)
                && entries[i]
                    .1
                    .is_some_and(|p| p.ids.iter().any(|x| x.eq_ignore_ascii_case(id)))
        })
    };
    let by_program = |program: &str, entries: &[(Entry, Option<&Preset>)]| {
        let stem = program_stem(program);
        (0..entries.len()).find(|&i| {
            usable(i, entries)
                && entries[i].1.is_some_and(|p| {
                    exe_name(p).eq_ignore_ascii_case(&stem) || p.id.eq_ignore_ascii_case(&stem)
                })
        })
    };

    // 1. Started from.
    let mut auto: Option<(usize, String)> = started_from(host, &presets)
        .and_then(|id| (0..entries.len()).find(|&i| entries[i].0.id == id && usable(i, &entries)))
        .map(|i| (i, "started from it".to_string()));

    // 2. The desktop's default.
    let default = match platform {
        Platform::Mac => {
            let handler = mac_shell_handler(host);
            match handler.as_deref().and_then(|id| by_id(id, &entries)) {
                Some(i) => Some((i, "default terminal (shell scripts open in it)".to_string())),
                None => by_id("com.apple.Terminal", &entries)
                    .map(|i| (i, "macOS default".to_string())),
            }
        }
        Platform::Linux => linux_default(host, &mut entries, &by_id, &by_program),
        Platform::Windows => {
            let console = windows_delegation(host);
            let wt = (0..entries.len()).find(|&i| entries[i].0.id == "windows-terminal" && usable(i, &entries));
            let cmd = (0..entries.len()).find(|&i| entries[i].0.id == "cmd" && usable(i, &entries));
            match console {
                Some(Delegation::ConsoleHost) => {
                    cmd.map(|i| (i, "default terminal application: Windows Console Host".into()))
                }
                Some(Delegation::WindowsTerminal) => {
                    wt.map(|i| (i, "default terminal application".to_string()))
                }
                None => wt.map(|i| (i, "Windows default".to_string())),
            }
        }
    };
    if let Some((i, why)) = &default {
        entries[*i].0.note = Some(why.clone());
    }
    if let Some((i, why)) = &auto {
        entries[*i].0.note = Some(match &default {
            Some((d, _)) if d == i => format!("{why}; {}", entries[*i].0.note.clone().unwrap_or_default()),
            _ => why.clone(),
        });
    }
    auto = auto.or(default);

    // 3. The first installed one.
    let auto = auto.or_else(|| {
        (0..entries.len())
            .find(|&i| usable(i, &entries))
            .map(|i| (i, "installed".to_string()))
    });

    // Installed first (the automatic one first), then the others in table order.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    let auto_ix = auto.as_ref().map(|(i, _)| *i);
    order.sort_by_key(|&i| (Some(i) != auto_ix, !entries[i].0.installed));
    let new_auto = auto_ix.and_then(|a| order.iter().position(|&i| i == a));
    Detection {
        platform,
        entries: order.into_iter().map(|i| entries[i].0.clone()).collect(),
        auto: new_auto,
        auto_why: auto.map(|(_, w)| w).unwrap_or_default(),
    }
}

/// The preset id of the terminal the app runs in, by its marks (`TERM` first, then
/// `TERM_PROGRAM`, the terminals' variables, macOS `__CFBundleIdentifier`).
fn started_from(host: &dyn Host, presets: &[&Preset]) -> Option<&'static str> {
    let term = host.var("TERM").unwrap_or_default();
    let program = host.var("TERM_PROGRAM").unwrap_or_default();
    let candidates = || presets.iter().filter(|p| !p.variant);
    let hit = |f: &dyn Fn(&Mark) -> bool| {
        candidates()
            .find(|p| p.marks.iter().any(f))
            .map(|p| p.id)
    };
    hit(&|m| matches!(m, Mark::Term(t) if term == *t || term.starts_with(&format!("{t}-"))))
        .or_else(|| hit(&|m| matches!(m, Mark::Program(t) if program.eq_ignore_ascii_case(t))))
        .or_else(|| hit(&|m| matches!(m, Mark::Var(v) if host.var(v).is_some())))
        .or_else(|| {
            let bundle = host.var("__CFBundleIdentifier")?;
            candidates()
                .find(|p| {
                    matches!(p.find, Find::App { .. })
                        && p.ids.iter().any(|i| i.eq_ignore_ascii_case(&bundle))
                })
                .map(|p| p.id)
        })
}

/// macOS: the bundle id the user made the handler of shell scripts / executables
/// (LaunchServices; e.g. iTerm2's "Make iTerm2 Default Term").
fn mac_shell_handler(host: &dyn Host) -> Option<String> {
    let home = host.var("HOME")?;
    let plist = Path::new(&home)
        .join("Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist");
    if !host.exists(&plist) {
        return None;
    }
    let json = host.query(&["plutil", "-convert", "json", "-o", "-", &plist.to_string_lossy()])?;
    shell_handler_from_json(&json)
}

/// The handler of `public.unix-executable` (then `com.apple.terminal.shell-script`,
/// `public.shell-script`) in LaunchServices' handler list as JSON.
pub fn shell_handler_from_json(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let handlers = v.get("LSHandlers")?.as_array()?;
    ["public.unix-executable", "com.apple.terminal.shell-script", "public.shell-script"]
        .iter()
        .find_map(|ty| {
            handlers.iter().find_map(|h| {
                let t = h.get("LSHandlerContentType")?.as_str()?;
                if !t.eq_ignore_ascii_case(ty) {
                    return None;
                }
                ["LSHandlerRoleShell", "LSHandlerRoleAll"]
                    .iter()
                    .find_map(|r| h.get(*r)?.as_str())
                    .filter(|b| !b.is_empty() && *b != "-")
                    .map(str::to_string)
            })
        })
}

enum Delegation {
    ConsoleHost,
    WindowsTerminal,
}

/// Windows' "default terminal application" (Settings ▸ System ▸ For developers ▸
/// Terminal); `None` = "Let Windows decide" / not set.
fn windows_delegation(host: &dyn Host) -> Option<Delegation> {
    let out = host.query(&[
        "reg",
        "query",
        "HKCU\\Console\\%%Startup",
        "/v",
        "DelegationTerminal",
    ])?;
    let guid = out
        .lines()
        .find(|l| l.contains("DelegationTerminal"))?
        .split_whitespace()
        .last()?
        .to_ascii_uppercase();
    match guid.as_str() {
        "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}" => Some(Delegation::ConsoleHost),
        "{E12CFF52-A866-4C77-9A90-F570A7AA2C6B}"
        | "{86633F1F-6454-40EC-89CE-DA4EBA977EE2}"
        | "{06EC847C-C0A5-46B8-92CB-7C92F6E35CD5}" => Some(Delegation::WindowsTerminal),
        _ => None,
    }
}

type Pick<'a> = dyn Fn(&str, &[(Entry, Option<&'a Preset>)]) -> Option<usize> + 'a;

/// Linux: `$TERMINAL`, the xdg-terminals list / `xdg-terminal-exec`, the desktop's
/// setting, `x-terminal-emulator` — the first that names an installed terminal. A
/// launcher that maps to no preset becomes an entry of its own (it runs in the folder).
fn linux_default<'a>(
    host: &dyn Host,
    entries: &mut Vec<(Entry, Option<&'a Preset>)>,
    by_id: &Pick<'a>,
    by_program: &Pick<'a>,
) -> Option<(usize, String)> {
    let launcher = |entries: &mut Vec<(Entry, Option<&'a Preset>)>,
                        id: &str,
                        name: String,
                        command: String,
                        why: &str| {
        let program = split_command_line(&command).into_iter().next()?;
        host.find_program(&program)?;
        entries.push((
            Entry {
                id: id.to_string(),
                name,
                command,
                installed: true,
                note: None,
            },
            None,
        ));
        Some((entries.len() - 1, why.to_string()))
    };
    // $TERMINAL: a command line (`kitty --single-instance`).
    if let Some(t) = host.var("TERMINAL") {
        let program = split_command_line(&t).into_iter().next().unwrap_or_default();
        if let Some(i) = by_program(&program, entries) {
            return Some((i, "$TERMINAL".into()));
        }
        let name = format!("$TERMINAL ({})", program_stem(&program));
        if let Some(hit) = launcher(entries, "$TERMINAL", name, t.trim().to_string(), "$TERMINAL") {
            return Some(hit);
        }
    }
    // The xdg-terminals list (`xdg-terminal-exec`'s configuration).
    for id in xdg_terminals(host) {
        if let Some(i) = by_id(&id, entries) {
            return Some((i, "default terminal (xdg-terminals.list)".into()));
        }
    }
    if host.find_program("xdg-terminal-exec").is_some()
        && let Some(hit) = launcher(
            entries,
            "xdg-terminal-exec",
            "Default terminal (xdg-terminal-exec)".into(),
            "xdg-terminal-exec".into(),
            "the desktop's default terminal",
        )
    {
        return Some(hit);
    }
    // The desktop's own setting.
    if let Some((program_or_id, why)) = desktop_setting(host)
        && let Some(i) =
            by_id(&program_or_id, entries).or_else(|| by_program(&program_or_id, entries))
    {
        return Some((i, why));
    }
    // Debian's alternatives.
    if let Some(x) = host.find_program("x-terminal-emulator") {
        let target = host.canonicalize(&x).unwrap_or(x);
        if let Some(i) = by_program(&target.to_string_lossy(), entries) {
            return Some((i, "default terminal (x-terminal-emulator)".into()));
        }
        return launcher(
            entries,
            "x-terminal-emulator",
            "Default terminal (x-terminal-emulator)".into(),
            "x-terminal-emulator".into(),
            "Debian's default terminal",
        );
    }
    None
}

/// The desktop entry ids of the xdg-terminals list (`[$desktop-]xdg-terminals.list` in
/// `$XDG_CONFIG_HOME`, then `$XDG_CONFIG_DIRS`), the first file found.
fn xdg_terminals(host: &dyn Host) -> Vec<String> {
    let mut dirs = Vec::new();
    match host.var("XDG_CONFIG_HOME") {
        Some(c) => dirs.push(PathBuf::from(c)),
        None => {
            if let Some(h) = host.var("HOME") {
                dirs.push(Path::new(&h).join(".config"));
            }
        }
    }
    let config_dirs = host.var("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".into());
    dirs.extend(config_dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    let mut names: Vec<String> = host
        .var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| format!("{}-xdg-terminals.list", d.to_lowercase()))
        .collect();
    names.push("xdg-terminals.list".into());
    for dir in &dirs {
        for name in &names {
            if let Some(text) = host.read_file(&dir.join(name)) {
                return text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('-'))
                    .map(|l| {
                        let l = l.trim_start_matches('+');
                        // `id.desktop:action` → `id.desktop`.
                        l.split_once(".desktop:")
                            .map(|(id, _)| format!("{id}.desktop"))
                            .unwrap_or_else(|| l.to_string())
                    })
                    .collect();
            }
        }
    }
    Vec::new()
}

/// The terminal in the desktop's settings: (program or desktop id, why).
fn desktop_setting(host: &dyn Host) -> Option<(String, String)> {
    let desktop = host.var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    let config = host
        .var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| host.var("HOME").map(|h| Path::new(&h).join(".config")))?;
    let has = |d: &str| desktop.split(':').any(|x| x == d);
    if has("kde") {
        let text = host.read_file(&config.join("kdeglobals")).unwrap_or_default();
        let general = ini_section(&text, "General");
        let why = "default terminal (KDE settings)".to_string();
        if let Some(service) = general.iter().find(|(k, _)| k == "TerminalService") {
            return Some((service.1.clone(), why));
        }
        if let Some(app) = general.iter().find(|(k, _)| k == "TerminalApplication") {
            let program = split_command_line(&app.1).into_iter().next()?;
            return Some((program, why));
        }
        return Some(("konsole".into(), "KDE default".into()));
    }
    if has("xfce") {
        let text = host.read_file(&config.join("xfce4/helpers.rc")).unwrap_or_default();
        let v = text.lines().find_map(|l| l.trim().strip_prefix("TerminalEmulator="));
        return Some((
            v.unwrap_or("xfce4-terminal").trim().to_string(),
            "default terminal (Xfce settings)".into(),
        ));
    }
    let schema = if has("x-cinnamon") || has("cinnamon") {
        "org.cinnamon.desktop.default-applications.terminal"
    } else if has("mate") {
        "org.mate.applications-terminal"
    } else if has("gnome") || has("unity") || has("budgie") {
        "org.gnome.desktop.default-applications.terminal"
    } else {
        return None;
    };
    let out = host.query(&["gsettings", "get", schema, "exec"])?;
    let program = out.trim().trim_matches('\'').trim();
    (!program.is_empty()).then(|| (program.to_string(), "default terminal (desktop settings)".into()))
}

/// The `key=value` lines of `[section]` of an ini file.
fn ini_section(text: &str, section: &str) -> Vec<(String, String)> {
    let header = format!("[{section}]");
    let mut inside = false;
    let mut out = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == header;
        } else if inside && let Some((k, v)) = line.split_once('=') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// A described machine.
    #[derive(Default)]
    struct Fake {
        platform: Option<Platform>,
        vars: HashMap<String, String>,
        programs: HashMap<String, PathBuf>,
        paths: HashSet<PathBuf>,
        files: HashMap<PathBuf, String>,
        links: HashMap<PathBuf, PathBuf>,
        queries: HashMap<String, String>,
    }

    impl Fake {
        fn new(platform: Platform) -> Self {
            Self {
                platform: Some(platform),
                ..Default::default()
            }
        }
        fn var(mut self, k: &str, v: &str) -> Self {
            self.vars.insert(k.into(), v.into());
            self
        }
        fn program(mut self, name: &str, path: &str) -> Self {
            self.programs.insert(name.into(), path.into());
            self.paths.insert(path.into());
            self
        }
        fn path(mut self, p: &str) -> Self {
            self.paths.insert(p.into());
            self
        }
        fn file(mut self, p: &str, text: &str) -> Self {
            self.files.insert(p.into(), text.into());
            self.paths.insert(p.into());
            self
        }
        fn link(mut self, from: &str, to: &str) -> Self {
            self.links.insert(from.into(), to.into());
            self
        }
        fn query(mut self, argv: &str, out: &str) -> Self {
            self.queries.insert(argv.into(), out.into());
            self
        }
    }

    impl Host for Fake {
        fn platform(&self) -> Platform {
            self.platform.unwrap()
        }
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned().filter(|v| !v.trim().is_empty())
        }
        fn find_program(&self, name: &str) -> Option<PathBuf> {
            if name.contains('/') {
                return self.paths.contains(Path::new(name)).then(|| name.into());
            }
            self.programs.get(name).cloned()
        }
        fn exists(&self, path: &Path) -> bool {
            self.paths.contains(path)
        }
        fn read_file(&self, path: &Path) -> Option<String> {
            self.files.get(path).cloned()
        }
        fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
            self.links.get(path).cloned().or_else(|| Some(path.into()))
        }
        fn query(&self, argv: &[&str]) -> Option<String> {
            self.queries.get(&argv.join(" ")).cloned()
        }
    }

    fn mac() -> Fake {
        Fake::new(Platform::Mac)
            .var("HOME", "/Users/u")
            .path("/System/Applications/Utilities/Terminal.app")
    }

    fn auto(d: &Detection) -> (&str, &str, &str) {
        let e = &d.entries[d.auto.unwrap()];
        (e.id.as_str(), e.command.as_str(), d.auto_why.as_str())
    }

    #[gitti_test::test]
    fn mac_default_is_terminal_and_installed_apps_come_first() {
        let d = detect_with(&mac().path("/Applications/Ghostty.app"));
        assert_eq!(auto(&d), ("terminal", "open -a Terminal {dir}", "macOS default"));
        let installed: Vec<&str> = d.installed().map(|e| e.id.as_str()).collect();
        assert_eq!(installed, ["terminal", "ghostty"]);
        // The others follow, not installed.
        assert!(d.entries.len() > 5);
        assert!(d.entries[2..].iter().all(|e| !e.installed));
        let ghostty = d.entries.iter().find(|e| e.id == "ghostty").unwrap();
        assert_eq!(ghostty.command, "open -a Ghostty {dir}");
        // Choosing it stores its command; the chooser recognises it.
        assert_eq!(d.entry_for("open -a Ghostty {dir}").unwrap().name, "Ghostty");
        assert_eq!(d.name_of("open -a Hyper {dir}"), "Hyper");
        assert_eq!(d.name_of("/opt/bin/myterm -x"), "myterm");
    }

    #[gitti_test::test]
    fn mac_started_from_wins_then_the_shell_handler() {
        let base = || {
            mac()
                .path("/Applications/Ghostty.app")
                .path("/Users/u/Applications/iTerm.app")
                .path("/Applications/kitty.app")
        };
        // gitty inside Ghostty.
        let d = detect_with(&base().var("TERM_PROGRAM", "ghostty").var("TERM", "xterm-ghostty"));
        assert_eq!(auto(&d).0, "ghostty");
        assert_eq!(auto(&d).2, "started from it");
        // Inside tmux in kitty: TERM_PROGRAM is tmux, kitty's variable still tells.
        let d = detect_with(&base().var("TERM_PROGRAM", "tmux").var("KITTY_WINDOW_ID", "3"));
        assert_eq!(auto(&d).0, "kitty");
        assert_eq!(
            auto(&d).1,
            "/Applications/kitty.app/Contents/MacOS/kitty --single-instance --directory {dir}"
        );
        // Started from iTerm2 by bundle id only (a GUI app started from it).
        let d = detect_with(&base().var("__CFBundleIdentifier", "com.googlecode.iterm2"));
        assert_eq!(auto(&d).0, "iterm2");
        // A terminal that isn't installed where we look is not chosen.
        let d = detect_with(&mac().var("TERM_PROGRAM", "WezTerm"));
        assert_eq!(auto(&d).0, "terminal");
        // The user's handler for shell scripts.
        let plist = "/Users/u/Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist";
        let json = r#"{"LSHandlers":[
            {"LSHandlerURLScheme":"https","LSHandlerRoleAll":"com.apple.safari"},
            {"LSHandlerContentType":"public.unix-executable","LSHandlerRoleShell":"com.googlecode.iTerm2"}]}"#;
        let d = detect_with(&base().path(plist).query(
            &format!("plutil -convert json -o - {plist}"),
            json,
        ));
        assert_eq!(auto(&d).0, "iterm2");
        assert_eq!(auto(&d).2, "default terminal (shell scripts open in it)");
        assert_eq!(d.auto, Some(0), "the automatic choice is listed first");
    }

    #[gitti_test::test]
    fn linux_default_sources_in_order() {
        let base = || {
            Fake::new(Platform::Linux)
                .var("HOME", "/home/u")
                .program("gnome-terminal", "/usr/bin/gnome-terminal")
                .program("kitty", "/usr/bin/kitty")
                .program("foot", "/usr/bin/foot")
                .program("x-terminal-emulator", "/usr/bin/x-terminal-emulator")
                .link("/usr/bin/x-terminal-emulator", "/usr/bin/gnome-terminal.wrapper")
        };
        // x-terminal-emulator resolves to a preset (its dir option is used).
        let d = detect_with(&base());
        assert_eq!(auto(&d).0, "gnome-terminal");
        assert_eq!(auto(&d).1, "gnome-terminal --working-directory={dir}");
        assert_eq!(auto(&d).2, "default terminal (x-terminal-emulator)");
        // GNOME's setting.
        let d = detect_with(
            &base()
                .var("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")
                .program("gsettings", "/usr/bin/gsettings")
                .query("gsettings get org.gnome.desktop.default-applications.terminal exec", "'foot'\n"),
        );
        assert_eq!(auto(&d).0, "foot");
        // The xdg-terminals list beats it.
        let d = detect_with(
            &base()
                .var("XDG_CURRENT_DESKTOP", "GNOME")
                .file("/home/u/.config/gnome-xdg-terminals.list", "# mine\nnone.desktop\nkitty.desktop:new-window\n"),
        );
        assert_eq!(auto(&d).0, "kitty");
        // $TERMINAL beats the list; with arguments, a program gitti knows → its preset.
        let d = detect_with(&base().var("TERMINAL", "foot --server"));
        assert_eq!((auto(&d).0, auto(&d).2), ("foot", "$TERMINAL"));
        // An unknown $TERMINAL runs as it is (in the folder).
        let d = detect_with(&base().var("TERMINAL", "st -f mono").program("st", "/usr/bin/st"));
        assert_eq!(auto(&d), ("$TERMINAL", "st -f mono", "$TERMINAL"));
        assert_eq!(d.entries[0].name, "$TERMINAL (st)");
        // A $TERMINAL that isn't installed is skipped.
        let d = detect_with(&base().var("TERMINAL", "nosuchterm"));
        assert_eq!(auto(&d).0, "gnome-terminal");
        // Started from kitty beats everything.
        let d = detect_with(&base().var("TERMINAL", "foot").var("TERM", "xterm-kitty"));
        assert_eq!(auto(&d).0, "kitty");
        // KDE: kdeglobals.
        let d = detect_with(
            &base()
                .var("XDG_CURRENT_DESKTOP", "KDE")
                .program("konsole", "/usr/bin/konsole")
                .file("/home/u/.config/kdeglobals", "[General]\nTerminalApplication=kitty -1\n"),
        );
        // x-terminal-emulator is after the desktop's setting.
        assert_eq!(auto(&d).0, "kitty");
        assert_eq!(auto(&d).2, "default terminal (KDE settings)");
        // Nothing but xdg-terminal-exec: it is the default launcher.
        let d = detect_with(
            &Fake::new(Platform::Linux).program("xdg-terminal-exec", "/usr/bin/xdg-terminal-exec"),
        );
        assert_eq!(auto(&d).0, "xdg-terminal-exec");
        // Nothing at all: a clear error.
        let d = detect_with(&Fake::new(Platform::Linux));
        assert_eq!(d.auto, None);
        let e = d.choose("", None).unwrap_err().to_string();
        assert!(e.contains("No terminal found") && e.contains("Settings ▸ Diff ▸ Terminal"), "{e}");
    }

    #[gitti_test::test]
    fn windows_terminal_unless_the_console_host_is_the_default() {
        let base = || {
            Fake::new(Platform::Windows)
                .program("wt", "C:\\Users\\u\\AppData\\Local\\Microsoft\\WindowsApps\\wt.exe")
                .program("cmd", "C:\\Windows\\System32\\cmd.exe")
                .program("git", "C:/Program Files/Git/cmd/git.exe")
                .path("C:/Program Files/Git/git-bash.exe")
        };
        let d = detect_with(&base());
        assert_eq!(auto(&d), ("windows-terminal", "wt.exe -d .", "Windows default"));
        let gb = d.entries.iter().find(|e| e.id == "git-bash").unwrap();
        assert!(gb.installed);
        assert_eq!(
            split_command_line(&gb.command),
            ["C:/Program Files/Git/git-bash.exe", "--cd={dir}"]
        );
        // The tab variant is listed, never chosen automatically.
        assert!(d.installed().any(|e| e.id == "windows-terminal-tab"));
        let reg = "reg query HKCU\\Console\\%%Startup /v DelegationTerminal";
        let out = "\r\nHKEY_CURRENT_USER\\Console\\%%Startup\r\n    DelegationTerminal    REG_SZ    {B23D10C0-E52E-411E-9D5B-C09FDF709C7D}\r\n";
        let d = detect_with(&base().query(reg, out));
        assert_eq!(auto(&d).0, "cmd");
        // Inside Windows Terminal (gitty): that one.
        let d = detect_with(&base().query(reg, out).var("WT_SESSION", "abc"));
        assert_eq!(auto(&d).0, "windows-terminal");
    }

    #[gitti_test::test]
    fn the_setting_and_the_override_come_first() {
        let d = detect_with(&mac().path("/Applications/Ghostty.app"));
        let c = d.choose("open -a Ghostty {dir}", None).unwrap();
        assert_eq!((c.name.as_str(), c.source.clone()), ("Ghostty", Source::Setting));
        assert_eq!(
            c.argv(Path::new("/w/my repo")),
            ["open", "-a", "Ghostty", "/w/my repo"]
        );
        let c = d.choose("kitty", Some("wezterm start --cwd {dir}")).unwrap();
        assert_eq!(c.source, Source::Override);
        assert_eq!(c.argv(Path::new("/a b")), ["wezterm", "start", "--cwd", "/a b"]);
        // Blank values count as unset.
        let c = d.choose("  ", Some(" ")).unwrap();
        assert_eq!((c.source, c.name.as_str()), (Source::Auto, "Terminal"));
    }

    #[gitti_test::test]
    fn the_chooser_names_the_setting() {
        let d = detect_with(&mac().path("/Applications/Ghostty.app").var("TERM_PROGRAM", "ghostty"));
        let (label, runs) = d.describe("");
        assert_eq!(label, "Automatic (Ghostty)");
        assert_eq!(runs, "Automatic: Ghostty (started from it) — runs open -a Ghostty {dir}");
        assert_eq!(d.describe("open -a Terminal {dir}").0, "Terminal");
        assert_eq!(
            d.describe(" kitty --directory {dir} "),
            ("Custom (kitty)".to_string(), "Custom — runs kitty --directory {dir}".to_string())
        );
        let none = detect_with(&Fake::new(Platform::Linux));
        assert_eq!(none.describe("").0, "Automatic (none found)");
        assert!(none.problem("", &Fake::new(Platform::Linux)).is_some());
        assert_eq!(d.problem("", &mac()), None);
    }

    #[gitti_test::test]
    fn problems_say_what_is_missing() {
        let host = mac()
            .path("/Applications/Ghostty.app")
            .program("kitty", "/usr/local/bin/kitty");
        assert_eq!(problem("open -a Ghostty {dir}", &host), None);
        assert_eq!(problem("kitty --directory {dir}", &host), None);
        assert_eq!(problem("", &host), None);
        let p = problem("open -a iTerm {dir}", &host).unwrap();
        assert!(p.contains("iTerm.app is not in Applications"), "{p}");
        let p = problem("alacritty --working-directory {dir}", &host).unwrap();
        assert!(p.contains("`alacritty` is not installed") && p.contains("choose another"), "{p}");
        let p = problem("/opt/x/term", &host).unwrap();
        assert!(p.contains("/opt/x/term does not exist"), "{p}");
    }

    #[gitti_test::test]
    fn quoting_survives_the_split() {
        for s in [
            "/Applications/kitty.app/Contents/MacOS/kitty",
            "/Volumes/My Disk/Apps/WezTerm.app/Contents/MacOS/wezterm",
            "it's \"quoted\"",
            "C:\\Program Files\\Git\\git-bash.exe",
        ] {
            let q = quote_arg(s);
            assert_eq!(split_command_line(&format!("{q} --x {{dir}}")), [s, "--x", "{dir}"], "{q}");
        }
        assert_eq!(quote_arg("/usr/bin/kitty"), "/usr/bin/kitty");
        assert_eq!(quote_arg_for("C:\\Git\\git-bash.exe", true), "C:\\Git\\git-bash.exe");
        assert_eq!(
            quote_arg_for("C:\\Program Files\\Git\\git-bash.exe", true),
            "\"C:\\Program Files\\Git\\git-bash.exe\""
        );
        // An app in a folder with spaces gets a quoted {exe}.
        let d = detect_with(
            &mac().var("HOME", "/Users/a b").path("/Users/a b/Applications/WezTerm.app"),
        );
        let w = d.entries.iter().find(|e| e.id == "wezterm").unwrap();
        assert_eq!(
            command_argv(&w.command, Path::new("/r/x y")),
            ["/Users/a b/Applications/WezTerm.app/Contents/MacOS/wezterm", "start", "--cwd", "/r/x y"]
        );
        let tab = d.entries.iter().find(|e| e.id == "wezterm-tab").unwrap();
        assert!(tab.command.ends_with(" cli spawn --cwd {dir}"), "{}", tab.command);
    }

    #[gitti_test::test]
    fn launch_services_json() {
        assert_eq!(shell_handler_from_json("{}"), None);
        assert_eq!(shell_handler_from_json("not json"), None);
        let j = r#"{"LSHandlers":[{"LSHandlerContentType":"com.apple.terminal.shell-script","LSHandlerRoleAll":"net.kovidgoyal.kitty"}]}"#;
        assert_eq!(shell_handler_from_json(j).as_deref(), Some("net.kovidgoyal.kitty"));
    }

    #[gitti_test::test]
    fn presets_have_a_folder_or_run_in_it() {
        let mut seen = HashSet::new();
        for p in PRESETS {
            assert!(seen.insert((p.id, p.platform as u8)), "duplicate {}", p.id);
            let argv = split_command_line(p.command);
            assert!(!argv.is_empty(), "{}", p.id);
        }
    }
}
