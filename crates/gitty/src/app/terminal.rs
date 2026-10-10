//! "Open in Terminal" in gitty: the repository folder (palette), a worktree's or a
//! submodule's folder (their sidebar menus), and the terminal chooser (palette ▸ Choose
//! Terminal…) — the same detection and shared setting `terminal_command` as gitti
//! (`gitti_core::terminal`). Automatic prefers the terminal gitty runs in.

use std::path::PathBuf;

use gitti_core::terminal::{SystemHost, redetect};

use super::{App, spawn};
use crate::modal::{Live, Modal, PickItem};

impl App {
    /// Opens the terminal in `dir` on a job (detection and macOS `open` take a moment);
    /// the status line says which one opened, or why none could and what to do.
    pub fn open_terminal_at(&mut self, dir: PathBuf) {
        if self.terminal_job {
            return;
        }
        self.terminal_job = true;
        let setting = self.terminal_command.clone();
        spawn(
            &self.tx,
            {
                let dir = dir.clone();
                move || {
                    gitti_core::desktop::open_terminal(&dir, &setting).map_err(|e| format!("{e:#}"))
                }
            },
            move |result| super::Msg::TerminalOpened(result, dir),
        );
    }

    pub(super) fn terminal_opened(&mut self, result: Result<String, String>, dir: PathBuf) {
        self.terminal_job = false;
        match result {
            Ok(name) => self.set_info(format!("Opened {name} in {}", dir.display())),
            Err(e) => self.set_error(e),
        }
    }

    /// Palette entries: open the repository folder, choose the terminal.
    pub(super) fn terminal_palette_items(&self) -> Vec<PickItem> {
        let dir = self.repo.workdir().to_path_buf();
        let label = gitti_core::terminal::cached()
            .describe(&self.terminal_command)
            .0;
        vec![
            PickItem::new("Open in Terminal", "repository folder", move |app: &mut App| {
                app.open_terminal_at(dir)
            }),
            PickItem::new(
                "Choose Terminal…",
                format!("for Open in Terminal — now: {label}"),
                |app: &mut App| app.terminal_chooser(),
            ),
        ]
    }

    /// The terminals found on this computer (Automatic first), then "Custom Command…".
    pub fn terminal_chooser(&mut self) {
        // A fresh look: a terminal may have been installed meanwhile.
        let det = redetect();
        let current = self.terminal_command.trim().to_string();
        let mut items = Vec::new();
        let (auto_label, auto_runs) = det.describe("");
        let mut auto = PickItem::new(auto_label, auto_runs, |app: &mut App| {
            app.set_terminal_command(String::new())
        });
        auto.strong = current.is_empty();
        items.push(auto);
        for e in det.installed() {
            let command = e.command.clone();
            let mut it = PickItem::new(e.name.clone(), e.command.clone(), move |app: &mut App| {
                app.set_terminal_command(command)
            });
            it.strong = current == e.command;
            items.push(it);
        }
        let custom_now = !current.is_empty() && det.entry_for(&current).is_none();
        let mut custom = PickItem::new(
            "Custom Command…",
            if custom_now {
                format!("now: {current}")
            } else {
                "a command line, {dir} = the folder".to_string()
            },
            |app: &mut App| app.terminal_command_dialog(),
        );
        custom.strong = custom_now;
        items.push(custom);
        for e in det.entries.iter().filter(|e| !e.installed) {
            items.push(PickItem::new(e.name.clone(), "", |_| {}).unavailable("not found on this computer"));
        }
        self.modal = Some(Modal::picker("Choose Terminal", items));
    }

    /// The command line of "Open in Terminal", typed: says live what it runs and warns
    /// when its program is missing.
    pub fn terminal_command_dialog(&mut self) {
        let det = redetect();
        let current = self.terminal_command.clone();
        let mut note = "The command that opens your terminal in a folder: {dir} is replaced by \
                        the folder, and it runs in that folder. Empty = automatic (the \
                        terminal gitty runs in, else the system's default terminal)."
            .to_string();
        if let Some(env) = std::env::var("GITTI_TERMINAL")
            .ok()
            .filter(|t| !t.trim().is_empty())
        {
            note.push_str(&format!(
                "\n\nNote: GITTI_TERMINAL is set (`{env}`) and overrides this setting."
            ));
        }
        self.modal = Some(
            Modal::input(
                "Terminal Command",
                vec![("Command", current.as_str())],
                |app, values| app.set_terminal_command(values[0].trim().to_string()),
            )
            .with_note(note)
            .with_live(move |values, _| {
                let command = values.first().map(|v| v.trim()).unwrap_or("");
                let mut text = det.describe(command).1;
                if let Some(w) = det.problem(command, &SystemHost) {
                    text.push_str(&format!(" ⚠ {w}"));
                }
                Live {
                    text,
                    ok_label: "Save".into(),
                    ..Default::default()
                }
            }),
        );
    }

    /// Stores the choice (shared with gitti) and says what it is.
    fn set_terminal_command(&mut self, command: String) {
        self.terminal_command = command;
        let det = gitti_core::terminal::cached();
        let (label, _) = det.describe(&self.terminal_command);
        match det.problem(&self.terminal_command, &SystemHost) {
            Some(w) => self.set_error(format!("Terminal: {label} — {w}")),
            None => self.set_info(format!("Open in Terminal uses {label}")),
        }
    }
}
