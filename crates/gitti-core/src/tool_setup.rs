//! Setting up the external diff / merge tool ([`crate::ext_tool`]): the dialog both apps
//! show when "Open in External Diff / Merge Tool" finds none set up, and behind
//! [`CHOOSE_LABEL`]. What it offers — the tools git knows (`--tool-help`; installed ones
//! first, terminal ones marked) and the user's own (`difftool.<name>.cmd` /
//! `mergetool.<name>.cmd`) —, where each setting is now, what a choice writes into which
//! config file (this repository's or the global one), and the dialog's live text.
//!
//! Backups: git's own `mergetool.keepBackup` / `mergetool.keepTemporaries`, shown as
//! options and written as chosen (default as git: the `.orig` backup is kept). The apps
//! never delete a `.orig` file: it stays an untracked file until the user deletes it.

use std::collections::HashMap;

use anyhow::{Result, anyhow};

use crate::Repo;
use crate::ext_tool::{ToolConfig, ToolHelp, ToolKind, parse_bool};
use crate::ops::ConfigScope;

/// The palette's (and gitti's Repository menu's) entry.
pub const CHOOSE_LABEL: &str = "Choose External Diff / Merge Tool…";
/// The dialog's title.
pub const DIALOG_TITLE: &str = "External Diff / Merge Tool";
/// The scope choice (gitti: the options row, in this order).
pub const SCOPE_OPTIONS: [&str; 2] = ["This Repository", "Global"];
/// gitty's check for the global scope (unchecked: this repository).
pub const GLOBAL_CHECK: &str = "Global: all your repositories (off: only this one)";
pub const DIFF_FIELD: &str = "Diff tool (diff.tool; empty: takes the merge tool)";
pub const MERGE_FIELD: &str = "Merge tool (merge.tool)";
pub const KEEP_BACKUP_LABEL: &str = "Keep git's backup <file>.orig when a tool merged a file \
     (mergetool.keepBackup; git's default: on). It stays an untracked file until you delete it.";
pub const KEEP_TEMPORARIES_LABEL: &str = "Keep the copies git hands the tool (<file>_LOCAL_…, \
     _REMOTE_…, _BASE_…) when the tool fails (mergetool.keepTemporaries; git's default: off)";
/// The two options in a line each (gitty's checks), explained by [`BACKUPS_NOTE`].
pub const KEEP_BACKUP_SHORT: &str = "Keep <file>.orig backups (mergetool.keepBackup)";
pub const KEEP_TEMPORARIES_SHORT: &str = "Keep copies if the tool fails (mergetool.keepTemporaries)";
pub const BACKUPS_NOTE: &str = "Backups: after a tool merged a file, git keeps the conflicted \
     version as <file>.orig (mergetool.keepBackup, on by default) — an untracked file the app \
     never deletes. mergetool.keepTemporaries keeps the copies git hands the tool \
     (<file>_LOCAL_…, _REMOTE_…, _BASE_…) when it fails (off by default).";
/// What the fill-in buttons / ctrl-n lists are.
pub const FILLS_LABEL: &str = "Installed:";

const KEEP_BACKUP: &str = "mergetool.keepBackup";
const KEEP_TEMPORARIES: &str = "mergetool.keepTemporaries";

/// Where a tool stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolState {
    /// The user's own command (`difftool.<name>.cmd` / `mergetool.<name>.cmd`).
    Own,
    /// A tool git knows whose program is installed.
    Installed,
    /// A tool git knows but whose program it can't find.
    Missing,
}

/// One tool the dialog lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEntry {
    pub name: String,
    /// git's words ("Use Meld (requires a graphical session)"); the user's own: their
    /// command.
    pub description: String,
    pub state: ToolState,
    /// It runs inside a terminal (vimdiff, emerge, a command starting vim, …).
    pub terminal: bool,
    /// git offers it for `git difftool` / `git mergetool`.
    pub diff: bool,
    pub merge: bool,
}

impl ToolEntry {
    fn serves(&self, kind: ToolKind) -> bool {
        match kind {
            ToolKind::Diff => self.diff,
            ToolKind::Merge => self.merge,
        }
    }

    /// "meld", "kompare (diff only)".
    fn label(&self) -> String {
        match (self.diff, self.merge) {
            (true, false) => format!("{} (diff only)", self.name),
            (false, true) => format!("{} (merge only)", self.name),
            _ => self.name.clone(),
        }
    }
}

/// One config file's own tool settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ScopeValues(HashMap<String, String>);

impl ScopeValues {
    /// From `Repo::config_list` (keys lower-cased by git).
    fn from_list(list: &HashMap<String, String>) -> Self {
        let keys = [ToolKind::Diff.key(), ToolKind::Merge.key(), KEEP_BACKUP, KEEP_TEMPORARIES];
        Self(
            keys.iter()
                .filter_map(|k| Some((k.to_string(), list.get(&k.to_lowercase())?.trim().to_string())))
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }
}

/// What the user chose in the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolChoice {
    pub scope: ConfigScope,
    /// `diff.tool` (empty: none of its own — git difftool takes the merge tool).
    pub diff: String,
    pub merge: String,
    pub keep_backup: bool,
    pub keep_temporaries: bool,
}

impl ToolChoice {
    fn tool(&self, kind: ToolKind) -> &str {
        match kind {
            ToolKind::Diff => self.diff.trim(),
            ToolKind::Merge => self.merge.trim(),
        }
    }
}

/// The settings a choice writes into one config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolWrites {
    pub scope: ConfigScope,
    /// The file (for the messages).
    pub file: String,
    /// Key → value (`None`: removed from that file).
    pub set: Vec<(String, Option<String>)>,
}

impl ToolWrites {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// "diff.tool = meld, mergetool.keepBackup = false; removes merge.tool".
    pub fn describe(&self) -> String {
        let set: Vec<String> = self
            .set
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k} = {v}")))
            .collect();
        let removed: Vec<&str> = self
            .set
            .iter()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.as_str())
            .collect();
        match (set.is_empty(), removed.is_empty()) {
            (_, true) => set.join(", "),
            (true, false) => format!("removes {}", removed.join(", ")),
            (false, false) => format!("{}; removes {}", set.join(", "), removed.join(", ")),
        }
    }
}

/// What the set-up dialog offers and knows (one background read: [`Repo::tool_choices`]).
#[derive(Debug, Clone)]
pub struct ToolChoices {
    /// The user's own tools, then the installed ones (those with a window first), then
    /// those git knows but can't find; each group by name.
    pub tools: Vec<ToolEntry>,
    config: ToolConfig,
    local: ScopeValues,
    global: ScopeValues,
    local_file: String,
    global_file: String,
}

impl ToolChoices {
    /// From both `--tool-help` lists, the effective tool settings and each config file's
    /// own (`config_list`), and the two files' paths.
    pub fn new(
        diff: &ToolHelp,
        merge: &ToolHelp,
        config: ToolConfig,
        local: &HashMap<String, String>,
        global: &HashMap<String, String>,
        local_file: String,
        global_file: String,
    ) -> Self {
        let mut tools: Vec<ToolEntry> = Vec::new();
        for (kind, help) in [(ToolKind::Diff, diff), (ToolKind::Merge, merge)] {
            let mut own: Vec<String> = help.user_defined.clone();
            for n in config.own_tools(kind) {
                if !own.contains(&n) {
                    own.push(n);
                }
            }
            let known = help
                .available
                .iter()
                .map(|t| (t, ToolState::Installed))
                .chain(help.unavailable.iter().map(|t| (t, ToolState::Missing)));
            let found = own
                .iter()
                .map(|n| (n.clone(), String::new(), ToolState::Own, None))
                .chain(known.map(|(t, s)| (t.name.clone(), t.description.clone(), s, Some(t))));
            for (name, description, state, known) in found {
                let tool = config.named(kind, &name);
                let terminal = match known {
                    // git's word ("graphical session") decides unless the user's settings do.
                    Some(t) if tool.cmd.is_none() && tool.path.is_none() => t.needs_terminal(),
                    _ => tool.needs_terminal(),
                };
                let (state, description) = match &tool.cmd {
                    // A command of the user's own replaces a built-in tool of that name.
                    Some(cmd) => (ToolState::Own, cmd.clone()),
                    None => (state, description),
                };
                let entry = match tools.iter_mut().find(|e| e.name == name) {
                    Some(e) => e,
                    None => {
                        tools.push(ToolEntry {
                            name: name.clone(),
                            description,
                            state,
                            terminal,
                            diff: false,
                            merge: false,
                        });
                        tools.last_mut().expect("just pushed")
                    }
                };
                entry.state = entry.state.min(state);
                match kind {
                    ToolKind::Diff => entry.diff = true,
                    ToolKind::Merge => entry.merge = true,
                }
            }
        }
        // By name within each group (as git lists them; the merge list's variants —
        // vimdiff1, … — join their base tool).
        tools.sort_by(|a, b| (a.state, a.terminal, &a.name).cmp(&(b.state, b.terminal, &b.name)));
        Self {
            tools,
            config,
            local: ScopeValues::from_list(local),
            global: ScopeValues::from_list(global),
            local_file,
            global_file,
        }
    }

    fn scope(&self, scope: ConfigScope) -> &ScopeValues {
        match scope {
            ConfigScope::Local => &self.local,
            ConfigScope::Global => &self.global,
        }
    }

    /// The config file of `scope`.
    pub fn file(&self, scope: ConfigScope) -> &str {
        match scope {
            ConfigScope::Local => &self.local_file,
            ConfigScope::Global => &self.global_file,
        }
    }

    /// The dialog's values when it opens: the settings as git reads them now; this
    /// repository when it has tool settings of its own, else global.
    pub fn initial(&self) -> ToolChoice {
        let own = |k: ToolKind| self.local.get(k.key()).is_some();
        let value = |k: ToolKind| self.config.value(k.key()).unwrap_or("").trim().to_string();
        ToolChoice {
            scope: if own(ToolKind::Diff) || own(ToolKind::Merge) {
                ConfigScope::Local
            } else {
                ConfigScope::Global
            },
            diff: value(ToolKind::Diff),
            merge: value(ToolKind::Merge),
            keep_backup: self.config.keep_backup(),
            keep_temporaries: self.config.keep_temporaries(),
        }
    }

    /// The tools offered to fill in for `kind`: the user's own and the installed ones
    /// (terminal ones only where the app can hand them the `terminal`).
    pub fn offered(&self, kind: ToolKind, terminal: bool) -> Vec<String> {
        self.tools
            .iter()
            .filter(|t| t.serves(kind) && t.state != ToolState::Missing && (terminal || !t.terminal))
            .map(|t| t.name.clone())
            .collect()
    }

    /// The dialog's text above its fields: why it opened (`need`: from a failing "Open
    /// in External … Tool"), then the tools git knows.
    pub fn intro(&self, need: Option<ToolKind>, terminal: bool) -> String {
        let mut s = match need {
            Some(kind) => format!(
                "No external {} is set up yet. Choose one: it opens right after, and git \
                 uses it for `git {}` too.",
                kind.noun(),
                kind.command()
            ),
            None => "git runs these tools for Open in External Diff Tool / Merge Tool \
                     (`git difftool` / `git mergetool`), on the command line too."
                .to_string(),
        };
        let list = |f: &dyn Fn(&ToolEntry) -> bool| {
            self.tools
                .iter()
                .filter(|t| f(t))
                .map(ToolEntry::label)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let own = list(&|t| t.state == ToolState::Own);
        let window = list(&|t| t.state == ToolState::Installed && !t.terminal);
        let in_terminal = list(&|t| t.state == ToolState::Installed && t.terminal);
        let missing = list(&|t| t.state == ToolState::Missing);
        if !own.is_empty() {
            s.push_str(&format!(" Your own: {own}."));
        }
        if !window.is_empty() {
            s.push_str(&format!(" Installed: {window}."));
        }
        if !in_terminal.is_empty() {
            s.push_str(&format!(" Installed, running inside a terminal: {in_terminal}"));
            s.push_str(if terminal {
                " (the terminal is handed over to them)."
            } else {
                " (not from this app: it starts tools without a terminal)."
            });
        }
        if own.is_empty() && window.is_empty() && in_terminal.is_empty() {
            s.push_str(" git finds none of the tools it knows installed.");
        }
        if !missing.is_empty() {
            s.push_str(&format!(" Known to git but not installed: {missing}."));
        }
        s
    }

    /// The value git would read for `key` after writing `w` (this file's own value wins
    /// over the global one; a value from elsewhere — system config, an include — stays
    /// when neither file has one).
    fn after(&self, w: &ToolWrites, key: &str) -> Option<String> {
        let written = w.set.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let file = |scope: ConfigScope| match (&written, w.scope == scope) {
            (Some(v), true) => v.clone(),
            _ => self.scope(scope).get(key).map(str::to_string),
        };
        let elsewhere = (self.local.get(key).is_none() && self.global.get(key).is_none())
            .then(|| self.config.value(key).map(|v| v.trim().to_string()))
            .flatten()
            .filter(|v| !v.is_empty());
        file(ConfigScope::Local).or(file(ConfigScope::Global)).or(elsewhere)
    }

    /// What choice `c` writes into its file: a tool that differs from that file's own
    /// value (an emptied field removes it there), an option that differs from what git
    /// reads now.
    pub fn writes(&self, c: &ToolChoice) -> ToolWrites {
        let own = self.scope(c.scope);
        let mut set = Vec::new();
        for kind in [ToolKind::Diff, ToolKind::Merge] {
            let (key, new) = (kind.key(), c.tool(kind));
            match own.get(key) {
                Some(_) if new.is_empty() => set.push((key.to_string(), None)),
                Some(v) if v == new => {}
                _ if new.is_empty() => {}
                _ => set.push((key.to_string(), Some(new.to_string()))),
            }
        }
        for (key, on, now) in [
            (KEEP_BACKUP, c.keep_backup, self.config.keep_backup()),
            (KEEP_TEMPORARIES, c.keep_temporaries, self.config.keep_temporaries()),
        ] {
            let same = match own.get(key).and_then(parse_bool) {
                Some(v) => v == on,
                None => on == now,
            };
            if !same {
                set.push((key.to_string(), Some(on.to_string())));
            }
        }
        ToolWrites {
            scope: c.scope,
            file: self.file(c.scope).to_string(),
            set,
        }
    }

    /// Why OK can't write `c` (`need`: the dialog opened for that kind of tool, which must
    /// then be set).
    pub fn problem(&self, c: &ToolChoice, need: Option<ToolKind>) -> Option<String> {
        for kind in [ToolKind::Diff, ToolKind::Merge] {
            let name = c.tool(kind);
            if name.is_empty() {
                continue;
            }
            if name.chars().any(char::is_whitespace) {
                return Some(format!("`{name}` is not a tool name (it has a space)"));
            }
            let known = self.tools.iter().any(|t| t.name == name && t.serves(kind));
            if !known {
                return Some(format!(
                    "{name} is not a {} git knows, and there is no command of your own for it \
                     ({}.{name}.cmd)",
                    kind.noun(),
                    kind.command()
                ));
            }
        }
        let w = self.writes(c);
        let diff = self.after(&w, ToolKind::Diff.key());
        let merge = self.after(&w, ToolKind::Merge.key());
        match need {
            Some(ToolKind::Merge) if merge.is_none() => return Some("Choose a merge tool".into()),
            Some(ToolKind::Diff) if diff.is_none() && merge.is_none() => {
                return Some("Choose a diff tool (or a merge tool: git difftool takes it too)".into());
            }
            _ => {}
        }
        if w.is_empty() {
            return Some("Nothing to change: the settings are as shown".into());
        }
        None
    }

    /// What the dialog says live about `c`: what it writes where, how git will use it,
    /// and what to know about the chosen tools (`terminal`: the app can hand a terminal
    /// tool the terminal).
    pub fn describe(&self, c: &ToolChoice, terminal: bool) -> String {
        let w = self.writes(c);
        let mut out: Vec<String> = Vec::new();
        if !w.is_empty() {
            out.push(format!("Writes to {}: {}.", w.file, w.describe()));
        }
        let diff = self.after(&w, ToolKind::Diff.key());
        let merge = self.after(&w, ToolKind::Merge.key());
        if diff.is_none()
            && let Some(m) = &merge
        {
            out.push(format!("git difftool takes the merge tool {m}."));
        }
        let mut seen: Vec<&str> = Vec::new();
        for kind in [ToolKind::Diff, ToolKind::Merge] {
            let name = c.tool(kind);
            if name.is_empty() || seen.contains(&name) {
                continue;
            }
            seen.push(name);
            let Some(t) = self.tools.iter().find(|t| t.name == name) else {
                continue;
            };
            if t.terminal {
                out.push(if terminal {
                    format!("{name} runs inside the terminal: it gets the terminal until it quits.")
                } else {
                    format!(
                        "{name} runs inside a terminal: this app starts tools without one, so it \
                         can't run {name} (gitty, the terminal app, can)."
                    )
                });
            }
            if t.state == ToolState::Missing {
                out.push(format!(
                    "{name} is not installed: git knows it but can't find its program — \
                     install it, or set {}.{name}.path.",
                    kind.command()
                ));
            }
        }
        if c.scope == ConfigScope::Global {
            for (key, _) in &w.set {
                if let Some(v) = self.local.get(key) {
                    out.push(format!(
                        "This repository's own {key} = {v} still wins here (choose This \
                         Repository to change it)."
                    ));
                }
            }
        }
        out.join(" ")
    }
}

impl Repo {
    /// What the set-up dialog offers: both `--tool-help` lists (read side by side —
    /// each takes a second or two), the tool settings and where they are.
    pub fn tool_choices(&self) -> Result<ToolChoices> {
        let (diff, merge) = std::thread::scope(|s| {
            let diff = s.spawn(|| self.tool_help(ToolKind::Diff));
            let merge = self.tool_help(ToolKind::Merge);
            let diff = diff
                .join()
                .unwrap_or_else(|_| Err(anyhow!("git difftool --tool-help failed")));
            (diff, merge)
        });
        let (diff, merge) = (diff?, merge?);
        let file = |scope: ConfigScope| {
            self.config_path(scope)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| match scope {
                    ConfigScope::Local => "this repository's config".into(),
                    ConfigScope::Global => "your global git config".into(),
                })
        };
        Ok(ToolChoices::new(
            &diff,
            &merge,
            self.tool_config(),
            &self.config_list(ConfigScope::Local),
            &self.config_list(ConfigScope::Global),
            file(ConfigScope::Local),
            file(ConfigScope::Global),
        ))
    }

    /// Writes the settings of a choice ([`ToolChoices::writes`]); says what went where.
    pub fn write_tool_settings(&self, w: &ToolWrites) -> Result<String> {
        for (key, value) in &w.set {
            self.config_set(w.scope, key, value.as_deref().unwrap_or(""))?;
        }
        Ok(format!("Wrote {} to {}", w.describe(), w.file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF_HELP: &str = "'git difftool --tool=<tool>' may be set to one of the following:\n\
        \t\tkompare          Use Kompare (requires a graphical session)\n\
        \t\tmeld             Use Meld (requires a graphical session)\n\
        \t\tvimdiff          Use Vim\n\
        \n\
        \tuser-defined:\n\
        \t\tmine.cmd my-diff \"$LOCAL\" \"$REMOTE\"\n\
        \n\
        The following tools are valid, but not currently available:\n\
        \t\tkdiff3           Use KDiff3 (requires a graphical session)\n";
    const MERGE_HELP: &str = "'git mergetool --tool=<tool>' may be set to one of the following:\n\
        \t\tmeld             Use Meld (requires a graphical session)\n\
        \t\tvimdiff          Use Vim\n\
        \n\
        The following tools are valid, but not currently available:\n\
        \t\tkdiff3           Use KDiff3 (requires a graphical session)\n\
        \t\ttortoisemerge    Use TortoiseMerge (requires a graphical session)\n";

    fn choices(pairs: &[(&str, &str)], local: &[(&str, &str)], global: &[(&str, &str)]) -> ToolChoices {
        let map = |kv: &[(&str, &str)]| kv.iter().map(|(k, v)| (k.to_lowercase(), v.to_string())).collect();
        ToolChoices::new(
            &ToolHelp::parse(DIFF_HELP),
            &ToolHelp::parse(MERGE_HELP),
            ToolConfig::from_pairs(pairs),
            &map(local),
            &map(global),
            "/r/.git/config".into(),
            "/home/u/.gitconfig".into(),
        )
    }

    #[gitti_test::test]
    fn lists_own_installed_then_missing_and_marks_terminal_tools() {
        let c = choices(&[("difftool.mine.cmd", "my-diff \"$LOCAL\" \"$REMOTE\"")], &[], &[]);
        let names: Vec<(&str, ToolState, bool)> =
            c.tools.iter().map(|t| (t.name.as_str(), t.state, t.terminal)).collect();
        assert_eq!(
            names,
            [
                ("mine", ToolState::Own, false),
                ("kompare", ToolState::Installed, false),
                ("meld", ToolState::Installed, false),
                ("vimdiff", ToolState::Installed, true),
                ("kdiff3", ToolState::Missing, false),
                ("tortoisemerge", ToolState::Missing, false),
            ]
        );
        assert_eq!(c.offered(ToolKind::Diff, false), ["mine", "kompare", "meld"]);
        assert_eq!(c.offered(ToolKind::Diff, true), ["mine", "kompare", "meld", "vimdiff"]);
        // A diff-only tool is not offered for merging; the own diff command neither.
        assert_eq!(c.offered(ToolKind::Merge, true), ["meld", "vimdiff"]);
        let intro = c.intro(Some(ToolKind::Diff), false);
        assert!(intro.starts_with("No external diff tool is set up yet."), "{intro}");
        assert!(intro.contains("Your own: mine (diff only)."), "{intro}");
        assert!(intro.contains("Installed: kompare (diff only), meld."), "{intro}");
        assert!(intro.contains("inside a terminal: vimdiff (not from this app"), "{intro}");
        assert!(intro.contains("not installed: kdiff3, tortoisemerge (merge only)."), "{intro}");
        // The user's own command makes a terminal tool of any name.
        let c = choices(&[("mergetool.meld.cmd", "nvim -d \"$LOCAL\" \"$REMOTE\"")], &[], &[]);
        let meld = c.tools.iter().find(|t| t.name == "meld").unwrap();
        assert!(meld.terminal && meld.state == ToolState::Own, "{meld:?}");
    }

    #[gitti_test::test]
    fn writes_only_what_changes_into_the_chosen_file() {
        let c = choices(&[], &[], &[]);
        let mut ch = c.initial();
        assert_eq!(ch.scope, ConfigScope::Global);
        assert_eq!((ch.diff.as_str(), ch.merge.as_str(), ch.keep_backup, ch.keep_temporaries), ("", "", true, false));
        // Nothing set up: the kind the dialog opened for must be chosen.
        assert_eq!(c.problem(&ch, Some(ToolKind::Merge)).as_deref(), Some("Choose a merge tool"));
        assert!(c.problem(&ch, Some(ToolKind::Diff)).unwrap().starts_with("Choose a diff tool"));
        assert!(c.problem(&ch, None).unwrap().starts_with("Nothing to change"));
        // A merge tool serves git difftool too.
        ch.merge = "meld".into();
        assert_eq!(c.problem(&ch, Some(ToolKind::Diff)), None);
        let w = c.writes(&ch);
        assert_eq!(w.set, [("merge.tool".to_string(), Some("meld".to_string()))]);
        let live = c.describe(&ch, false);
        assert!(live.starts_with("Writes to /home/u/.gitconfig: merge.tool = meld."), "{live}");
        assert!(live.contains("git difftool takes the merge tool meld."), "{live}");
        // Backups off: written as git's own option.
        ch.keep_backup = false;
        ch.scope = ConfigScope::Local;
        let w = c.writes(&ch);
        assert_eq!(w.file, "/r/.git/config");
        assert_eq!(w.describe(), "merge.tool = meld, mergetool.keepBackup = false");
        // Unknown names are refused; terminal / missing tools are explained.
        ch.diff = "nosuch".into();
        assert!(c.problem(&ch, None).unwrap().contains("nosuch is not a diff tool git knows"));
        ch.diff = "tortoisemerge".into();
        assert!(c.problem(&ch, None).unwrap().contains("not a diff tool"), "merge only");
        ch.diff = "vimdiff".into();
        assert!(c.describe(&ch, false).contains("can't run vimdiff"));
        assert!(c.describe(&ch, true).contains("vimdiff runs inside the terminal"));
        ch.diff = "kdiff3".into();
        assert!(c.describe(&ch, true).contains("kdiff3 is not installed"));
    }

    #[gitti_test::test]
    fn the_file_s_own_values_decide_and_an_override_is_named() {
        // This repository has its own merge tool: the dialog opens on it.
        let c = choices(
            &[("merge.tool", "vimdiff"), ("mergetool.keepBackup", "false")],
            &[("merge.tool", "vimdiff")],
            &[("merge.tool", "meld"), ("mergetool.keepBackup", "false")],
        );
        let mut ch = c.initial();
        assert_eq!((ch.scope, ch.merge.as_str(), ch.keep_backup), (ConfigScope::Local, "vimdiff", false));
        assert!(c.problem(&ch, None).unwrap().starts_with("Nothing to change"));
        // Emptied: removed from this repository; git takes the global one again.
        ch.merge.clear();
        let w = c.writes(&ch);
        assert_eq!(w.describe(), "removes merge.tool");
        assert_eq!(c.after(&w, "merge.tool").as_deref(), Some("meld"));
        // Global: the repository's own value still wins — said so.
        ch.scope = ConfigScope::Global;
        ch.merge = "kdiff3".into();
        let live = c.describe(&ch, false);
        assert!(live.contains("own merge.tool = vimdiff still wins"), "{live}");
        // Backups back on in the global file (it has them off).
        ch.keep_backup = true;
        assert!(c.writes(&ch).describe().contains("mergetool.keepBackup = true"));
    }
}
