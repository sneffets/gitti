//! Repository-level commands (gitti's menu bar): "Add to .gitignore…" (the top-level
//! file, a folder's own one or `info/exclude`), edit `.gitignore` / `.gitattributes`, open
//! the repository in the file manager, clone / init a repository. The Clone dialog takes a
//! repository location from the clipboard (`gitti_core::clone_source`).
//!
//! gitty shows one repository per process. After a clone / init it offers to switch: the
//! main loop then restarts gitty in the new repository (`reopen`), so no job or watcher of
//! the old one can reach the new state.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use gitti_core::create::{expand_home, name_from_url};
use gitti_core::ignore::{IgnoreFile, rule_dirs, suggestions_in};

use super::{After, App, Msg, spawn};
use crate::modal::{MenuItem, Modal, PickItem};

impl App {
    /// "Add to .gitignore" for working-directory file `path`: a menu of suggested patterns
    /// for the top-level `.gitignore` (this file, parent folders, extension, name; each
    /// opens the one-line editor with it), then one entry per folder `.gitignore` that can
    /// take a pattern for it (the file's folder, folders above it that have one).
    pub fn ignore_menu(&mut self, path: &str, tracked: bool) {
        let mut items = self.ignore_items(path, tracked, IgnoreFile::Gitignore);
        let nested: Vec<String> = rule_dirs(self.repo.workdir(), path, ".gitignore")
            .into_iter()
            .filter(|d| !d.is_empty())
            .collect();
        if !nested.is_empty() {
            items.push(MenuItem::separator());
            for (dir, key) in nested.into_iter().zip('a'..='h') {
                let file = IgnoreFile::in_dir(&dir);
                let exists = self
                    .repo
                    .ignore_file_path(&file)
                    .is_ok_and(|p| p.is_file());
                let label = format!(
                    "Patterns for {}…{}",
                    file.label(),
                    if exists { "" } else { " (new file)" }
                );
                let p = path.to_string();
                items.push(
                    MenuItem::new(label, move |app: &mut App| {
                        let items = app.ignore_items(&p, tracked, file.clone());
                        let title = format!("Add to {}", file.label());
                        app.modal = Some(Modal::menu(title, items));
                    })
                    .key(key),
                );
            }
        }
        // The menu is as wide as its items (the path is in the first one).
        let title = if tracked {
            "Ignore (tracked file)"
        } else {
            "Add to .gitignore"
        };
        self.modal = Some(Modal::menu(title, items));
    }

    /// The suggestions for `path` in `file` (patterns relative to its folder) and "Other
    /// pattern…".
    fn ignore_items(&self, path: &str, tracked: bool, file: IgnoreFile) -> Vec<MenuItem> {
        let mut items: Vec<MenuItem> = suggestions_in(path, file.dir())
            .into_iter()
            .zip('1'..='9')
            .map(|(c, key)| {
                let pattern = c.pattern.clone();
                let file = file.clone();
                MenuItem::new(
                    format!("{}   {}", c.label, c.pattern),
                    move |app: &mut App| app.ignore_dialog_to(&pattern, tracked, file),
                )
                .key(key)
            })
            .collect();
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("Other pattern…", move |app: &mut App| {
                app.ignore_dialog_to("", tracked, file)
            })
            .key('o'),
        );
        items
    }

    /// The editable pattern line for the top-level `.gitignore` (or `info/exclude`).
    pub fn ignore_dialog(&mut self, pattern: &str, tracked: bool) {
        self.ignore_dialog_to(pattern, tracked, IgnoreFile::Gitignore);
    }

    /// The editable pattern line for `file`, untracking, and — for the top-level file —
    /// `info/exclude` instead.
    fn ignore_dialog_to(&mut self, pattern: &str, tracked: bool, file: IgnoreFile) {
        let folder = !file.dir().is_empty();
        let label = if folder {
            format!("Pattern (relative to {}/ — applies only inside it)", file.dir())
        } else {
            "Pattern".to_string()
        };
        let untrack = if tracked {
            "Stop tracking matching files (git rm --cached; stays on disk)"
        } else {
            "Stop tracking matching tracked files (git rm --cached)"
        };
        let mut checks = Vec::new();
        if !folder {
            checks.push((
                "Write to .git/info/exclude instead (this clone only)",
                false,
            ));
        }
        checks.push((untrack, false));
        let title = format!("Add to {}", file.label());
        self.modal = Some(Modal::input_checks(
            title,
            vec![(label.as_str(), pattern)],
            checks,
            move |app, v, c| {
                let pattern = v[0].clone();
                let (which, untrack) = if folder {
                    (file, c[0])
                } else if c[0] {
                    (IgnoreFile::InfoExclude, c[1])
                } else {
                    (file, c[1])
                };
                app.run_op("Add to .gitignore", false, After::Nothing, move |r| {
                    Ok(r.add_ignore_pattern(&pattern, which, untrack)?.message())
                });
            },
        ));
    }

    /// The top-level `name` (`.gitignore`, `.gitattributes`) in the multi-line editor
    /// (ctrl-s saves; created if new).
    pub fn edit_rules_file(&mut self, name: &'static str) {
        let path = self.repo.workdir().join(name);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let hint = if name == ".gitattributes" {
            "one pattern and its attributes per line · ctrl-s save · esc cancel"
        } else {
            "one pattern per line · ctrl-s save · esc cancel"
        };
        self.modal = Some(Modal::edit(
            format!("Edit {name}"),
            hint,
            &text,
            move |app, text| {
                app.run_op(&format!("Save {name}"), false, After::Nothing, move |_| {
                    std::fs::write(&path, text)?;
                    Ok("saved".into())
                })
            },
        ));
    }

    fn open_folder(&mut self) {
        match gitti_core::desktop::open_folder(self.repo.workdir()) {
            Ok(()) => self.set_info(format!("Opened {}", self.repo.workdir().display())),
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// Folder offered for new repositories: next to this one.
    fn parent_dir(&self) -> String {
        self.repo
            .workdir()
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    }

    pub fn clone_dialog(&mut self) {
        let parent = self.parent_dir();
        self.modal = Some(Modal::input_checks(
            CLONE_TITLE,
            vec![
                ("Repository URL", ""),
                ("Parent directory", parent.as_str()),
                ("Folder name (empty: from the URL)", ""),
            ],
            vec![("Also clone the submodules (--recurse-submodules)", true)],
            |app, v, c| {
                let url = v[0].trim().to_string();
                let name = match v[2].trim() {
                    "" => name_from_url(&url).unwrap_or_default(),
                    n => n.to_string(),
                };
                if url.is_empty() || name.is_empty() {
                    app.set_error("Clone: enter the repository URL and a folder name".into());
                    return;
                }
                let dest = expand_home(&v[1]).join(name);
                let tx = app.tx.clone();
                let recurse = c[0];
                app.run_op("Clone", false, After::OfferOpen(dest.clone()), move |r| {
                    let progress = &mut |l: &str| {
                        let _ = tx.send(Msg::Progress(l.to_string()));
                    };
                    let c = gitti_core::create::clone_repo_outcome(r.git(), &url, &dest, recurse, progress)?;
                    let (path, note) = (c.dir, c.note);
                    // Submodules git refused from local paths: allow them once, then open?
                    if let Some(refused) = c.refused {
                        let retry = super::submodules::RetryUpdate::Clone { dir: path, note };
                        return Err(super::FollowUp::SubmodulesRefused(Box::new(refused), retry).into());
                    }
                    // The note (paths that collided) goes into the "Open?" question.
                    Ok(match note {
                        Some(note) => format!("cloned into {}\n\n{note}", path.display()),
                        None => format!("cloned into {}", path.display()),
                    })
                });
            },
        ));
        self.read_clipboard_for(CLONE_TITLE);
    }

    /// Sets the clipboard text of a scripted run (`clip:` steps, copies).
    pub fn set_script_clipboard(text: &str) {
        if let Ok(mut c) = SCRIPT_CLIPBOARD.lock() {
            *c = Some(text.to_string());
        }
    }

    /// Reads the clipboard for the Clone / Add Submodule dialog just opened (`title`): on a
    /// thread (the clipboard program can be slow — PowerShell on Windows), the dialog
    /// fills when it answers ([`Self::clone_clipboard`]). Clone takes a local repository
    /// too, Add Submodule URLs only (`clone_source::url_from_clipboard`). A scripted run
    /// takes the script's text (`clip:`) and never reads the system clipboard.
    pub(super) fn read_clipboard_for(&mut self, title: &'static str) {
        let source = move |text: String| {
            if title == CLONE_TITLE {
                gitti_core::clone_source::from_clipboard(&text)
            } else {
                gitti_core::clone_source::url_from_clipboard(&text)
            }
        };
        if std::env::var_os("GITTY_SCRIPT").is_some() {
            let text = SCRIPT_CLIPBOARD.lock().ok().and_then(|c| c.clone());
            self.clone_clipboard(title, text.and_then(source));
            return;
        }
        spawn(
            &self.tx,
            move || {
                let tools = crate::clipboard::paste_tools();
                source(crate::clipboard::paste_with(tools, None, PASTE_TIMEOUT)?)
            },
            move |url| Msg::CloneClipboard(title, url),
        );
    }

    /// The repository location from the clipboard into the URL field of dialog `title` —
    /// selected, with a note — as long as that dialog is open and nothing was typed there.
    pub(super) fn clone_clipboard(&mut self, title: &str, url: Option<String>) {
        let Some(url) = url else {
            return;
        };
        if let Some(m) = &mut self.modal
            && let Modal::Input { title: open, fields, .. } = m
            && open.as_str() == title
            && fields.first().is_some_and(|f| f.edit.is_empty())
        {
            m.prefill(0, &url, gitti_core::clone_source::CLIPBOARD_NOTE);
        }
    }

    pub fn init_dialog(&mut self) {
        let mut dir = self.parent_dir();
        if !dir.is_empty() && !dir.ends_with(std::path::MAIN_SEPARATOR) {
            dir.push(std::path::MAIN_SEPARATOR);
        }
        // SHA-256 is an option, preset from git's own default (one `git config`).
        let format = gitti_core::create::format_choice(self.repo.git());
        let checks = if format.available {
            vec![(gitti_core::create::SHA256_CHECK, format.sha256)]
        } else {
            Vec::new()
        };
        let note = format!(
            "Creates the folder if needed and runs git init there.\n\n{}",
            format.note
        );
        self.modal = Some(
            Modal::input_checks(
                "Init Repository",
                vec![("Directory (created if missing)", dir.as_str())],
                checks,
                move |app, v, c| {
                    let dir = expand_home(&v[0]);
                    if v[0].trim().is_empty() {
                        return;
                    }
                    let format = format.format(c.first().copied().unwrap_or(false));
                    app.run_op("Init", false, After::OfferOpen(dir.clone()), move |r| {
                        let path = gitti_core::create::init_repo_as(r.git(), &dir, format)?;
                        let kind = format.map(|f| format!("{} ", f.name())).unwrap_or_default();
                        Ok(format!("initialized {kind}repository {}", path.display()))
                    });
                },
            )
            .with_note(note),
        );
    }

    /// After a clone / init: switch to the new repository (gitty restarts in it)?
    /// `note`: what the user should know about it first (a clone's collided paths).
    pub(super) fn offer_open(&mut self, path: PathBuf, note: Option<&str>) {
        let body = format!(
            "{}\n\n{}gitty shows one repository at a time: it restarts in the new one.",
            path.display(),
            note.map(|n| format!("{n}\n\n")).unwrap_or_default()
        );
        self.modal = Some(Modal::confirm(
            "Open the new repository?",
            body,
            "Open",
            move |app: &mut App| app.reopen = Some(path),
        ));
    }

    /// Palette entries of these commands (gitti has them in its menu bar).
    pub(super) fn repo_palette_items(&self) -> Vec<PickItem> {
        let fm = gitti_core::desktop::file_manager_name();
        let cursor_file = self
            .rows
            .get(self.diff_cursor)
            .and_then(|r| r.file_ref())
            .and_then(|(src, file)| {
                let w = super::work_files(&self.working, src).get(file)?;
                Some((w.path().to_string(), !w.entry.untracked))
            });
        let cursor_path = cursor_file
            .as_ref()
            .map(|(p, _)| p.clone())
            .unwrap_or_default();
        vec![
            PickItem::new("Clone Repository…", "", |app: &mut App| {
                app.clone_dialog()
            }),
            PickItem::new("Init Repository…", "", |app: &mut App| app.init_dialog()),
            PickItem::new(format!("Open in {fm}"), "", |app: &mut App| {
                app.open_folder()
            }),
            PickItem::new(
                "Add to .gitignore…",
                "",
                move |app: &mut App| match cursor_file {
                    Some((path, tracked)) => app.ignore_menu(&path, tracked),
                    None => app.ignore_dialog("", false),
                },
            ),
            PickItem::new("Edit .gitignore…", "", |app: &mut App| {
                app.edit_rules_file(".gitignore")
            }),
            PickItem::new("Track with Git LFS…", "", move |app: &mut App| {
                app.lfs_track(cursor_path)
            }),
            PickItem::new("Edit .gitattributes…", "", |app: &mut App| {
                app.edit_rules_file(".gitattributes")
            }),
            PickItem::new("Start-up Timings…", "", |app: &mut App| {
                let records = gitti_core::startup::load();
                let text = gitti_core::startup::format_all(&records, &crate::app::local_time);
                app.modal = Some(Modal::text("Start-up Timings", text));
            }),
            PickItem::new("About gitty", "", |app: &mut App| app.show_about()),
            PickItem::new("Third-Party Licences…", "", |app: &mut App| app.show_licences()),
        ]
    }

    /// Help ▸ About: version and build commit first (`y` copies them), licence, the
    /// third-party summary (`l` opens the whole list), this system.
    pub fn show_about(&mut self) {
        use crate::modal::TextKey;
        let (version, build) = (env!("CARGO_PKG_VERSION"), env!("GITTI_BUILD"));
        let extras = gitti_core::about::Extras {
            licences_hint: "press l here, or run `gitty --licenses`",
            renderer: None,
        };
        let text = gitti_core::about::text("gitty", version, build, &extras);
        let keys = vec![
            TextKey {
                key: 'y',
                label: "copy version",
                keep: true,
                run: |app| {
                    let (version, build) = (env!("CARGO_PKG_VERSION"), env!("GITTI_BUILD"));
                    app.copy(gitti_core::about::version_line("gitty", version, build))
                },
            },
            TextKey {
                key: 'l',
                label: "third-party licences",
                keep: false,
                run: App::show_licences,
            },
        ];
        self.modal = Some(Modal::text("About gitty", text).with_text_keys(keys));
    }

    /// The third-party licences of this binary on this platform (also `gitty --licenses`).
    pub fn show_licences(&mut self) {
        let text = gitti_core::third_party::text("gitty");
        self.modal = Some(Modal::text("Third-Party Licences", text));
    }
}

const CLONE_TITLE: &str = "Clone Repository";

/// The Add Submodule dialog's title (`app/submodules.rs`).
pub(super) const ADD_SUBMODULE_TITLE: &str = "Add Submodule (staged — commit to record it)";

/// How long the clipboard program may take.
const PASTE_TIMEOUT: Duration = Duration::from_secs(3);

/// The clipboard of a scripted run: `clip:TEXT` steps and copies.
static SCRIPT_CLIPBOARD: Mutex<Option<String>> = Mutex::new(None);

/// Starts gitty again in `path` (the terminal is already restored): `exec` on unix, a child
/// process whose exit code we pass on elsewhere.
pub fn reopen(path: &std::path::Path) -> anyhow::Result<std::convert::Infallible> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(path);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        Err(cmd.exec().into())
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
