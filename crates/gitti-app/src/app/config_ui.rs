//! Editing git configuration (identity and common settings) per scope, plus raw editing
//! of the config files; Commit Encoding… (`i18n.commitEncoding` with what it means).

use gitti_core::commit_encoding::change;
use gitti_core::ops::ConfigScope;
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalSpec};

/// Settings offered in the dialog: (key, description).
pub const KEYS: &[(&str, &str)] = &[
    ("user.name", "Your name in commits"),
    ("user.email", "Your e-mail in commits"),
    ("user.signingkey", "Key used to sign commits"),
    ("commit.gpgsign", "Sign every commit (true/false)"),
    ("gpg.format", "openpgp / ssh / x509"),
    (
        "pull.rebase",
        "Rebase instead of merge on pull (true/false/merges)",
    ),
    ("pull.ff", "only / true / false"),
    ("push.default", "simple / current / upstream / matching"),
    (
        "push.autoSetupRemote",
        "Create upstream automatically on first push (true/false)",
    ),
    (
        "fetch.prune",
        "Remove deleted remote branches on fetch (true/false)",
    ),
    (
        "rebase.autoStash",
        "Stash before rebase, re-apply after (true/false)",
    ),
    ("merge.conflictStyle", "merge / diff3 / zdiff3"),
    ("init.defaultBranch", "Branch name for new repositories"),
    ("core.autocrlf", "Line ending conversion (true/false/input)"),
    ("core.excludesFile", "Global ignore file"),
    (
        "i18n.commitEncoding",
        "Encoding of new commit messages (empty: UTF-8; existing commits keep theirs)",
    ),
];

impl GittiApp {
    pub(super) fn open_config_dialog(
        &mut self,
        scope: ConfigScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let this_scope = repo.config_list(scope);
        let other_scope = repo.config_list(match scope {
            ConfigScope::Local => ConfigScope::Global,
            ConfigScope::Global => ConfigScope::Local,
        });
        let (title, other_name) = match scope {
            ConfigScope::Local => (
                format!("Git settings for this repository ({})", repo.name()),
                "global",
            ),
            ConfigScope::Global => (
                "Global git settings (~/.gitconfig)".to_string(),
                "this repo",
            ),
        };
        let mut spec = ModalSpec::new(title)
            .message(match scope {
                ConfigScope::Local => {
                    "Values here override the global settings for this repository only. Leave a field empty to use the global value."
                }
                ConfigScope::Global => "Defaults for all repositories. Leave a field empty to unset.",
            })
            .ok("Save");
        let mut originals = Vec::new();
        for (key, desc) in KEYS {
            let lower = key.to_lowercase();
            let value = this_scope.get(&lower).cloned().unwrap_or_default();
            let mut label = format!("{key} — {desc}");
            if let Some(other) = other_scope.get(&lower) {
                label.push_str(&format!("   ({other_name}: {other})"));
            }
            spec = spec.field(label, value.clone());
            originals.push(value);
        }
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let changes: Vec<(String, String)> = KEYS
                .iter()
                .zip(res.values)
                .zip(originals)
                .filter(|((_, new), old)| new.trim() != old)
                .map(|(((key, _), new), _)| (key.to_string(), new.trim().to_string()))
                .collect();
            if changes.is_empty() {
                return;
            }
            let n = changes.len();
            this.git_op("Save git settings", w, cx, move |r| {
                for (k, v) in &changes {
                    r.config_set(scope, k, v)?;
                }
                Ok(format!("Saved {n} setting(s)"))
            });
        });
    }

    /// Repository ▸ Commit Encoding…: `i18n.commitEncoding` for new commits, in this
    /// repository or globally — the dialog says which file gets written and what follows
    /// (texts in `gitti_core::commit_encoding::change`, the same in gitty).
    pub(super) fn commit_encoding_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let current = repo.commit_encoding_shown();
        let scopes = [ConfigScope::Local, ConfigScope::Global];
        let labels = scopes.map(change::scope_label);
        let start = usize::from(change::initial_scope(&current) == ConfigScope::Global);
        let spec = ModalSpec::new(change::TITLE)
            .message(change::intro(&current))
            .options(&labels, start)
            .field(change::FIELD_LABEL, current.configured.clone().unwrap_or_default())
            .ok("Save")
            .live(move |r| {
                let value = r.values.first().map(String::as_str).unwrap_or("");
                let fills = change::COMMON.iter().map(|n| (0, n.to_string())).collect();
                match change::describe(value, scopes[r.option.min(1)], &current) {
                    Ok(text) => ModalLive {
                        text,
                        fills,
                        fills_label: "Common:".into(),
                        ..Default::default()
                    },
                    Err(why) => ModalLive {
                        blocked: Some(why),
                        fills,
                        fills_label: "Common:".into(),
                        ..Default::default()
                    },
                }
            });
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let scope = scopes[res.option.min(1)];
            let value = res.values.first().cloned().unwrap_or_default();
            this.git_op("Commit encoding", w, cx, move |r| r.set_commit_encoding(scope, &value));
        });
    }

    pub(super) fn edit_config_file(
        &mut self,
        scope: ConfigScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let path = match repo.config_path(scope) {
            Ok(p) => p,
            Err(e) => {
                self.set_error(format!("{e:#}"));
                return;
            }
        };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let title = format!("Edit {}", path.display());
        self.show_edit_modal(&title, None, text, window, cx, move |this, text, _, cx| {
            match std::fs::write(&path, text) {
                Ok(()) => this.set_info(format!("Saved {}", path.display())),
                Err(e) => this.set_error(format!("Saving {}: {e}", path.display())),
            }
            cx.notify();
        });
    }
}
