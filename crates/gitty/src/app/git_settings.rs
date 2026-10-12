//! Git Settings: This Repository… / Global… (palette), as gitti's dialog: the common
//! settings (`gitti_core::git_settings::SETTINGS`) with their value in this scope and the
//! other scope's as a hint; ⏎ edits one value (empty = unset), the dialog says live which
//! file gets written and what wins, and the list comes back after the write. Edit
//! Repository / Global Config File… hand the file to the user's editor (a terminal
//! editor gets the terminal; the end re-reads the repository), or — without an editor
//! set — to gitty's own text editor.

use gitti_core::git_settings::{self, GitSettings};
use gitti_core::ops::ConfigScope;

use super::{After, App, Modal};
use crate::modal::{Live, PickItem};

impl App {
    /// The settings of `scope`, the cursor on `at` (the one just edited).
    pub fn git_settings_dialog(&mut self, scope: ConfigScope, at: Option<&'static str>) {
        let settings = self.repo.git_settings(scope);
        let other_name = git_settings::other_name(scope);
        let items: Vec<PickItem> = settings
            .values
            .iter()
            .map(|v| {
                let key = v.setting.key;
                let label = match &v.this {
                    Some(value) => format!("{key} = {value}"),
                    None => key.to_string(),
                };
                let hint = match &v.other {
                    Some(o) => format!("{} · {other_name}: {o}", v.setting.label),
                    None => v.setting.label.to_string(),
                };
                let snapshot = settings.clone();
                let mut item = PickItem::new(label, hint, move |app: &mut App| {
                    app.git_setting_edit(snapshot, key)
                });
                item.strong = v.this.is_some();
                item
            })
            .collect();
        let mut modal = Modal::picker(git_settings::title(scope, &self.repo.name()), items);
        if let Modal::Picker { note, cursor, .. } = &mut modal {
            *note = Some(format!(
                "{} Written to {}. ⏎ edits the value.",
                git_settings::intro(scope),
                settings.file
            ));
            if let Some(at) = at {
                *cursor = git_settings::SETTINGS.iter().position(|s| s.key == at).unwrap_or(0);
            }
        }
        self.modal = Some(modal);
    }

    /// One setting's value: empty removes it from this scope's file; ⏎ writes and goes
    /// back to the list.
    fn git_setting_edit(&mut self, settings: GitSettings, key: &'static str) {
        let Some(v) = settings.get(key).cloned() else {
            return;
        };
        let scope = settings.scope;
        let current = v.this.clone().unwrap_or_default();
        let other = match &v.other {
            Some(o) => format!("{}: {o}", git_settings::other_name(scope)),
            None => format!("{}: not set", git_settings::other_name(scope)),
        };
        let note = format!(
            "{} — {}. Now {} in {} ({other}). Leave it empty to unset it there.",
            v.setting.label,
            v.setting.help_with_values(),
            v.this.as_deref().unwrap_or("not set"),
            settings.file,
        );
        let fills: Vec<(usize, String)> = v.setting.values.iter().map(|x| (0, x.to_string())).collect();
        let unchanged = current.clone();
        self.modal = Some(
            Modal::input(
                git_settings::title(scope, &self.repo.name()),
                vec![(key, current.as_str())],
                move |app: &mut App, values| {
                    let value = values.into_iter().next().unwrap_or_default().trim().to_string();
                    if value == unchanged {
                        app.git_settings_dialog(scope, Some(key));
                        return;
                    }
                    app.run_op("Save git setting", true, After::Nothing, move |r| {
                        r.write_git_setting(scope, key, &value)
                    });
                    // Back to the list once written (it reads the new value).
                    app.defer_while_busy(move |app: &mut App| app.git_settings_dialog(scope, Some(key)));
                },
            )
            .with_note(note)
            .with_live(move |values, _| {
                let value = values.first().map(String::as_str).unwrap_or("");
                let (text, blocked) = match settings.describe(key, value) {
                    Ok(text) => (text, None),
                    Err(why) => (String::new(), Some(why)),
                };
                let ok_label = if value.trim() == v.this.as_deref().unwrap_or("") {
                    "Back"
                } else if value.trim().is_empty() {
                    "Unset"
                } else {
                    "Save"
                };
                Live {
                    text,
                    ok_label: ok_label.into(),
                    blocked,
                    fills: fills.clone(),
                    fills_label: "Values".into(),
                    ..Default::default()
                }
            }),
        );
    }

    /// Edit Repository / Global Config File…: the file in the editor (the setting, else
    /// `$VISUAL` / `$EDITOR`; a terminal editor gets the terminal and its end re-reads the
    /// repository), without one in gitty's own text editor. A missing global file is
    /// created empty first.
    pub fn edit_config_file(&mut self, scope: ConfigScope) {
        let path = match self.repo.config_file_for_editing(scope) {
            Ok(p) => p,
            Err(e) => return self.set_error(format!("{e:#}")),
        };
        let shown = self.repo.config_file_shown(scope);
        if !self.editor_template().trim().is_empty() {
            self.open_file_in_editor(&path, 1, format!("Opened {shown} in the editor"));
            return;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return self.set_error(format!("Reading {shown}: {e}")),
        };
        self.modal = Some(Modal::edit(
            format!("Edit {shown}"),
            "git config file · ctrl-s save · esc cancel",
            &text,
            move |app, text| {
                app.run_op(&format!("Save {shown}"), true, After::Nothing, move |_| {
                    std::fs::write(&path, text)?;
                    Ok("saved".into())
                })
            },
        ));
    }

    /// The palette's entries.
    pub(super) fn git_settings_palette_items(&self) -> Vec<PickItem> {
        vec![
            PickItem::new(git_settings::REPO_LABEL, git_settings::REPO_DETAIL, |app: &mut App| {
                app.git_settings_dialog(ConfigScope::Local, None)
            }),
            PickItem::new(git_settings::GLOBAL_LABEL, git_settings::GLOBAL_DETAIL, |app: &mut App| {
                app.git_settings_dialog(ConfigScope::Global, None)
            }),
            PickItem::new(git_settings::EDIT_REPO_LABEL, git_settings::EDIT_REPO_DETAIL, |app: &mut App| {
                app.edit_config_file(ConfigScope::Local)
            }),
            PickItem::new(git_settings::EDIT_GLOBAL_LABEL, git_settings::EDIT_GLOBAL_DETAIL, |app: &mut App| {
                app.edit_config_file(ConfigScope::Global)
            }),
        ]
    }
}
