//! The common git settings both apps offer to edit per scope (gitti: Repository ▸ Git
//! Settings: This Repository… / Global…, one field per setting; gitty: the palette, a
//! list of the settings and one value edited at a time), and the raw config files
//! (Edit Repository / Global Config File…: gitti's text editor, gitty hands the terminal
//! to the user's editor).
//!
//! [`SETTINGS`] is the list (key, label, help, the values git takes where it is a fixed
//! set); [`Repo::git_settings`] reads both scopes for a dialog, [`describe`] says what
//! saving a value does (which file, what wins), [`Repo::write_git_setting`] writes or
//! unsets one value and says where it went.

use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::Repo;
use crate::ops::ConfigScope;

/// One setting of the dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitSetting {
    /// The config key as git documents it (`push.autoSetupRemote`).
    pub key: &'static str,
    /// A short name ("Auto-create upstream").
    pub label: &'static str,
    /// What it does, in plain words.
    pub help: &'static str,
    /// The values git takes, when it is a fixed set (offered to fill in; others are
    /// written as typed with a warning). Empty: free text.
    pub values: &'static [&'static str],
}

impl GitSetting {
    /// `help` with its values: "Sign every commit (true / false)".
    pub fn help_with_values(&self) -> String {
        if self.values.is_empty() {
            self.help.to_string()
        } else {
            format!("{} ({})", self.help, self.values.join(" / "))
        }
    }

    /// Whether git takes `value` as one of [`Self::values`] (booleans in every form git
    /// reads: yes / on / 1 …; other names ignoring case, as git compares them).
    pub fn takes(&self, value: &str) -> bool {
        let v = value.trim().to_ascii_lowercase();
        if self.values.is_empty() || self.values.iter().any(|x| x.eq_ignore_ascii_case(&v)) {
            return true;
        }
        let boolean = self.values.contains(&"true") && self.values.contains(&"false");
        boolean && matches!(v.as_str(), "yes" | "no" | "on" | "off" | "1" | "0")
    }
}

const BOOL: &[&str] = &["true", "false"];

/// The settings both dialogs offer, in their order.
pub const SETTINGS: &[GitSetting] = &[
    GitSetting { key: "user.name", label: "Name", help: "Your name in commits", values: &[] },
    GitSetting { key: "user.email", label: "E-mail", help: "Your e-mail in commits", values: &[] },
    GitSetting {
        key: "user.signingkey",
        label: "Signing key",
        help: "Key used to sign commits (a key id, or an SSH key file with gpg.format ssh)",
        values: &[],
    },
    GitSetting { key: "commit.gpgsign", label: "Sign commits", help: "Sign every commit", values: BOOL },
    GitSetting {
        key: "gpg.format",
        label: "Signature format",
        help: "Kind of signature",
        values: &["openpgp", "ssh", "x509"],
    },
    GitSetting {
        key: "pull.rebase",
        label: "Rebase on pull",
        help: "Rebase instead of merge on pull",
        values: &["true", "false", "merges", "interactive"],
    },
    GitSetting {
        key: "pull.ff",
        label: "Fast-forward on pull",
        help: "only: refuse a pull that can't fast-forward; false: always a merge commit",
        values: &["only", "true", "false"],
    },
    GitSetting {
        key: "push.default",
        label: "Push default",
        help: "What git push without a branch pushes",
        values: &["simple", "current", "upstream", "matching", "nothing"],
    },
    GitSetting {
        key: "push.autoSetupRemote",
        label: "Auto-create upstream",
        help: "Create the upstream automatically on the first push",
        values: BOOL,
    },
    GitSetting {
        key: "fetch.prune",
        label: "Prune on fetch",
        help: "Remove deleted remote branches on fetch",
        values: BOOL,
    },
    GitSetting {
        key: "rebase.autoStash",
        label: "Auto-stash on rebase",
        help: "Stash before rebase, re-apply after",
        values: BOOL,
    },
    GitSetting {
        key: "merge.conflictStyle",
        label: "Conflict style",
        help: "How conflicts are written into files",
        values: &["merge", "diff3", "zdiff3"],
    },
    GitSetting {
        key: "init.defaultBranch",
        label: "Default branch",
        help: "Branch name for new repositories",
        values: &[],
    },
    GitSetting {
        key: "core.autocrlf",
        label: "Line endings",
        help: "Line ending conversion",
        values: &["true", "false", "input"],
    },
    GitSetting { key: "core.excludesFile", label: "Global ignore file", help: "Global ignore file", values: &[] },
    GitSetting {
        key: crate::commit_encoding::KEY,
        label: "Commit encoding",
        help: "Encoding of new commit messages (empty: UTF-8; existing commits keep theirs)",
        values: crate::commit_encoding::change::COMMON,
    },
    GitSetting {
        key: "diff.tool",
        label: "Diff tool",
        help: "The tool git difftool opens (Choose External Diff / Merge Tool… lists the installed ones)",
        values: &[],
    },
    GitSetting {
        key: "merge.tool",
        label: "Merge tool",
        help: "The tool git mergetool opens (also the diff tool while diff.tool is unset)",
        values: &[],
    },
    GitSetting {
        key: "mergetool.keepBackup",
        label: "Keep merge backups",
        help: "Keep git's <file>.orig after a merge tool resolved a file",
        values: BOOL,
    },
];

/// The setting of `key` (any case).
pub fn find(key: &str) -> Option<&'static GitSetting> {
    SETTINGS.iter().find(|s| s.key.eq_ignore_ascii_case(key))
}

/// The menu / palette entries.
pub const REPO_LABEL: &str = "Git Settings: This Repository…";
pub const GLOBAL_LABEL: &str = "Git Settings: Global…";
pub const EDIT_REPO_LABEL: &str = "Edit Repository Config File…";
pub const EDIT_GLOBAL_LABEL: &str = "Edit Global Config File…";
/// Palette details.
pub const REPO_DETAIL: &str = "user.name, email, pull, push, tools…";
pub const GLOBAL_DETAIL: &str = "~/.gitconfig";
pub const EDIT_REPO_DETAIL: &str = ".git/config";
pub const EDIT_GLOBAL_DETAIL: &str = "~/.gitconfig";

/// The settings dialog's entry of `scope`.
pub fn menu_label(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => REPO_LABEL,
        ConfigScope::Global => GLOBAL_LABEL,
    }
}

/// The config file editor's entry of `scope`.
pub fn edit_label(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => EDIT_REPO_LABEL,
        ConfigScope::Global => EDIT_GLOBAL_LABEL,
    }
}

/// The dialog's title (`repo`: the repository's name).
pub fn title(scope: ConfigScope, repo: &str) -> String {
    match scope {
        ConfigScope::Local => format!("Git settings for this repository ({repo})"),
        ConfigScope::Global => "Global git settings (~/.gitconfig)".to_string(),
    }
}

/// What the dialog says above its settings.
pub fn intro(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => {
            "Values here override the global settings for this repository only. Leave a field empty to use the global value."
        }
        ConfigScope::Global => "Defaults for all repositories. Leave a field empty to unset.",
    }
}

/// How the dialog names the other scope's value ("global: true").
pub fn other_name(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => "global",
        ConfigScope::Global => "this repo",
    }
}

/// The other scope.
pub fn other(scope: ConfigScope) -> ConfigScope {
    match scope {
        ConfigScope::Local => ConfigScope::Global,
        ConfigScope::Global => ConfigScope::Local,
    }
}

/// One setting's values in a dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingValue {
    pub setting: &'static GitSetting,
    /// In the dialog's scope (`None`: not set there).
    pub this: Option<String>,
    /// In the other scope (the hint).
    pub other: Option<String>,
}

/// What a settings dialog shows: every setting of [`SETTINGS`] in both scopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSettings {
    pub scope: ConfigScope,
    /// The file the dialog writes, shown (`.git/config`, `~/.gitconfig`).
    pub file: String,
    pub values: Vec<SettingValue>,
}

impl GitSettings {
    /// From the `git config --list` of both scopes (keys lower-cased, as git lists them).
    pub fn from_lists(
        scope: ConfigScope,
        file: String,
        this: &HashMap<String, String>,
        other: &HashMap<String, String>,
    ) -> Self {
        let values = SETTINGS
            .iter()
            .map(|s| {
                let k = s.key.to_lowercase();
                SettingValue { setting: s, this: this.get(&k).cloned(), other: other.get(&k).cloned() }
            })
            .collect();
        Self { scope, file, values }
    }

    /// The value of `key` (any case).
    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.values.iter().find(|v| v.setting.key.eq_ignore_ascii_case(key))
    }

    /// What saving `new` for `key` does — which file gets written, what wins — or why it
    /// can't (`Err`: a name gitti can't write commit messages in).
    pub fn describe(&self, key: &str, new: &str) -> Result<String, String> {
        let Some(v) = self.get(key) else {
            return Err(format!("{key} is not one of the settings here"));
        };
        describe(v, self.scope, &self.file, new)
    }
}

/// What saving `new` for setting `v` in `scope` (written to `file`) does, in plain words;
/// `Err`: why it can't.
pub fn describe(v: &SettingValue, scope: ConfigScope, file: &str, new: &str) -> Result<String, String> {
    let s = v.setting;
    let key = s.key;
    let new = new.trim();
    if s.key == crate::commit_encoding::KEY
        && let Some(why) = crate::commit_encoding::change::refusal(new)
    {
        return Err(why);
    }
    let other = v.other.as_deref();
    let current = v.this.as_deref();
    if current == Some(new) || (current.is_none() && new.is_empty()) {
        return Ok(match current {
            Some(c) => format!("No change: {key} is {c} in {file}."),
            None => format!("No change: {key} is not set in {file}."),
        });
    }
    let mut text = if new.is_empty() {
        let mut t = format!("Removes {key} from {file}.");
        match (scope, other) {
            (ConfigScope::Local, Some(g)) => t.push_str(&format!(" The global value {g} applies here again.")),
            (ConfigScope::Local, None) => t.push_str(" git's default applies here (no global value)."),
            (ConfigScope::Global, Some(l)) => {
                t.push_str(&format!(" This repository keeps its own value {l}; the others get git's default."))
            }
            (ConfigScope::Global, None) => t.push_str(" Repositories without their own value get git's default."),
        }
        t
    } else {
        let mut t = format!("Writes {key} = {new} to {file}.");
        match (scope, other) {
            (ConfigScope::Local, Some(g)) => {
                t.push_str(&format!(" For this repository only — it overrides the global value {g}."))
            }
            (ConfigScope::Local, None) => t.push_str(" For this repository only."),
            (ConfigScope::Global, Some(l)) => t.push_str(&format!(
                " For every repository without its own value — this one sets {l}, which wins here."
            )),
            (ConfigScope::Global, None) => t.push_str(" For every repository without its own value."),
        }
        if !s.takes(new) {
            t.push_str(&format!(
                " Note: {new} is not one of {} — git may refuse or ignore it.",
                s.values.join(" / ")
            ));
        }
        t
    };
    if s.key == crate::commit_encoding::KEY && !new.is_empty() {
        text.push_str(" Existing commits keep their encoding.");
    }
    Ok(text)
}

impl Repo {
    /// Both scopes' values of [`SETTINGS`] for the dialog of `scope` (three git processes:
    /// two `config --list`, the global file's name).
    pub fn git_settings(&self, scope: ConfigScope) -> GitSettings {
        let this = self.config_list(scope);
        let other = self.config_list(other(scope));
        GitSettings::from_lists(scope, self.config_file_shown(scope), &this, &other)
    }

    /// The config file of `scope` as the dialogs name it (`.git/config`, `~/.gitconfig`).
    pub fn config_file_shown(&self, scope: ConfigScope) -> String {
        match self.config_path(scope) {
            Ok(p) => crate::commit_encoding::shown_path(&p, self.workdir()),
            Err(_) => match scope {
                ConfigScope::Local => ".git/config".into(),
                ConfigScope::Global => "~/.gitconfig".into(),
            },
        }
    }

    /// Sets `key` to `value` in `scope` (empty: removes it there) and says what went where
    /// ("Set pull.rebase = true in .git/config"). `i18n.commitEncoding` refuses a name
    /// gitti can't write commit messages in.
    pub fn write_git_setting(&self, scope: ConfigScope, key: &str, value: &str) -> Result<String> {
        let value = value.trim();
        if key.trim().is_empty() {
            bail!("no setting given");
        }
        if key.eq_ignore_ascii_case(crate::commit_encoding::KEY)
            && let Some(why) = crate::commit_encoding::change::refusal(value)
        {
            bail!(why);
        }
        self.config_set(scope, key, value)?;
        let file = self.config_file_shown(scope);
        Ok(if value.is_empty() {
            format!("Removed {key} from {file}")
        } else {
            format!("Set {key} = {value} in {file}")
        })
    }

    /// The config file of `scope` for editing it as text: its path, created empty when it
    /// doesn't exist yet (a user without a `~/.gitconfig`), so an editor can open it.
    pub fn config_file_for_editing(&self, scope: ConfigScope) -> Result<std::path::PathBuf> {
        let path = self.config_path(scope)?;
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(kv: &[(&str, &str)]) -> HashMap<String, String> {
        kv.iter().map(|(k, v)| (k.to_lowercase(), v.to_string())).collect()
    }

    #[gitti_test::test]
    fn keys_are_unique_and_have_texts() {
        let mut seen = std::collections::HashSet::new();
        for s in SETTINGS {
            assert!(seen.insert(s.key.to_lowercase()), "twice: {}", s.key);
            assert!(s.key.contains('.') && !s.label.is_empty() && !s.help.is_empty(), "{s:?}");
        }
    }

    #[gitti_test::test]
    fn values_and_booleans() {
        let sign = find("commit.GPGSIGN").unwrap();
        assert_eq!(sign.help_with_values(), "Sign every commit (true / false)");
        for v in ["true", "False", "yes", "off", "1"] {
            assert!(sign.takes(v), "{v}");
        }
        assert!(!sign.takes("maybe"));
        let style = find("merge.conflictStyle").unwrap();
        assert!(style.takes("zdiff3") && !style.takes("yes"));
        assert!(find("user.name").unwrap().takes("anything"));
    }

    #[gitti_test::test]
    fn describe_names_the_file_and_what_wins() {
        let local = GitSettings::from_lists(
            ConfigScope::Local,
            ".git/config".into(),
            &list(&[("pull.rebase", "true")]),
            &list(&[("pull.rebase", "false"), ("diff.tool", "meld")]),
        );
        assert_eq!(local.get("PULL.REBASE").unwrap().this.as_deref(), Some("true"));
        assert_eq!(local.get("diff.tool").unwrap().other.as_deref(), Some("meld"));
        assert_eq!(local.describe("pull.rebase", "true").unwrap(), "No change: pull.rebase is true in .git/config.");
        assert_eq!(
            local.describe("pull.rebase", "").unwrap(),
            "Removes pull.rebase from .git/config. The global value false applies here again."
        );
        assert_eq!(
            local.describe("diff.tool", "kdiff3").unwrap(),
            "Writes diff.tool = kdiff3 to .git/config. For this repository only — it overrides the global value meld."
        );
        assert!(local.describe("fetch.prune", "sometimes").unwrap().contains("not one of true / false"));
        assert!(local.describe("i18n.commitEncoding", "latin-x").unwrap_err().contains("latin-x"));
        let global = GitSettings::from_lists(
            ConfigScope::Global,
            "~/.gitconfig".into(),
            &list(&[]),
            &list(&[("pull.rebase", "true")]),
        );
        assert_eq!(
            global.describe("pull.rebase", "merges").unwrap(),
            "Writes pull.rebase = merges to ~/.gitconfig. For every repository without its own value — this one \
             sets true, which wins here."
        );
    }
}
