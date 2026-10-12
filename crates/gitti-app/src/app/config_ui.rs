//! Editing git configuration (identity and common settings) per scope, plus raw editing
//! of the config files; Commit Encoding… (`i18n.commitEncoding` with what it means).

use gitti_core::commit_encoding::change;
use gitti_core::git_settings;
use gitti_core::ops::ConfigScope;
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalSpec};

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
        let settings = repo.git_settings(scope);
        let other_name = git_settings::other_name(scope);
        let mut spec = ModalSpec::new(git_settings::title(scope, &repo.name()))
            .message(format!("{} Written to {}.", git_settings::intro(scope), settings.file))
            .ok("Save");
        let mut originals = Vec::new();
        for v in &settings.values {
            let value = v.this.clone().unwrap_or_default();
            let mut label = format!("{} — {}", v.setting.key, v.setting.help_with_values());
            if let Some(other) = &v.other {
                label.push_str(&format!("   ({other_name}: {other})"));
            }
            spec = spec.field(label, value.clone());
            originals.push(value);
        }
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let changes: Vec<(&'static str, String)> = git_settings::SETTINGS
                .iter()
                .zip(res.values)
                .zip(originals)
                .filter(|((_, new), old)| new.trim() != old)
                .map(|((s, new), _)| (s.key, new.trim().to_string()))
                .collect();
            if changes.is_empty() {
                return;
            }
            let n = changes.len();
            this.git_op("Save git settings", w, cx, move |r| {
                let mut said = String::new();
                for (k, v) in &changes {
                    said = r.write_git_setting(scope, k, v)?;
                }
                Ok(if n == 1 {
                    said
                } else {
                    format!("Saved {n} settings in {}", r.config_file_shown(scope))
                })
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
