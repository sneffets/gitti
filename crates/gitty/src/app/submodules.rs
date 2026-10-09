//! Submodules in gitty: the menu of a SUBMODULES sidebar entry (Update, Update to
//! Remote Branch, Check Out Commit… (picker over its history), Open — gitty restarts in it —, Sync URL, Copy Path, Remove…; also in the
//! diff's menu on a pointer change), palette entries (the same commands as gitti's
//! Repository menu), and the list kept in step after a stage / unstage / discard. The data
//! comes with the refresh snapshot (`gitti_core::submodule`); the sidebar rows, the
//! toolbar's out-of-date badge and the pointer-change rows of diffs are drawn in `ui.rs`.

use gitti_core::ObjectId;
use gitti_core::submodule::{HISTORY_LIMIT, SubmoduleCommit, SubmoduleState, short_id};

use super::{After, App, Msg};
use crate::edit::TextEdit;
use crate::modal::{MenuItem, Modal, PickItem};

impl App {
    /// The actions on the submodule at `path`.
    pub(crate) fn submodule_menu_items(&self, path: &str) -> Vec<MenuItem> {
        let Some(sm) = self.submodules.iter().find(|s| s.path == path) else {
            return Vec::new();
        };
        let initialised = sm.state != SubmoduleState::Uninitialized;
        let dir = sm.dir(self.repo.workdir()).display().to_string();
        let mut v = Vec::new();
        let p = path.to_string();
        v.push(
            MenuItem::new(
                if initialised { "Update" } else { "Initialise and Update" },
                move |app| app.update_submodules(vec![p]),
            )
            .key('u'),
        );
        let p = path.to_string();
        let remote = match &sm.branch {
            Some(b) => format!("Update to Remote Branch ({b})"),
            None => "Update to Remote Branch".to_string(),
        };
        v.push(MenuItem::new(remote, move |app| app.update_submodules_remote(vec![p])).key('r'));
        if initialised {
            let p = path.to_string();
            v.push(
                MenuItem::new("Check Out Commit in Submodule…", move |app| {
                    app.checkout_commit_picker(&p)
                })
                .key('k'),
            );
        }
        if initialised {
            let p = path.to_string();
            v.push(MenuItem::new("Open (gitty restarts in it)", move |app| app.open_submodule(&p)).key('o'));
        }
        let folder = sm.dir(self.repo.workdir());
        if folder.is_dir() {
            let fm = gitti_core::desktop::file_manager_name();
            v.push(MenuItem::new(format!("Reveal in {fm}"), move |app| {
                if let Err(e) = gitti_core::desktop::open_folder(&folder) {
                    app.set_error(format!("{e:#}"));
                }
            }));
        }
        let p = path.to_string();
        v.push(MenuItem::new("Sync URL", move |app| app.sync_submodules(vec![p])).key('s'));
        v.push(MenuItem::new("Copy Path", move |app| app.copy(dir)).key('c'));
        let p = path.to_string();
        v.push(MenuItem::new("Remove Submodule…", move |app| app.remove_submodule_dialog(&p)).key('d'));
        v
    }

    /// The diff menu's part for a submodule's pointer change at `path` (empty: no such
    /// submodule here): its actions but Copy Path (the diff menu has the file's), without
    /// their keys (the diff menu has its own).
    pub(crate) fn submodule_file_items(&self, path: &str) -> Vec<MenuItem> {
        let mut v = self.submodule_menu_items(path);
        v.retain(|i| i.label != "Copy Path");
        for i in &mut v {
            i.label = format!("Submodule: {}", i.label);
            i.key = None;
        }
        v
    }

    /// `git submodule update --init --recursive --remote [-- paths]` (empty = all).
    pub fn update_submodules_remote(&mut self, paths: Vec<String>) {
        self.run_op("Update submodules to remote branches", true, After::Nothing, move |r| {
            let p: Vec<&str> = paths.iter().map(String::as_str).collect();
            r.update_submodules_remote(&p)
        });
    }

    /// Asks before removing the submodule at `path`: what goes, and the local work that
    /// would be lost (see gitti's `remove_submodule_dialog`).
    pub(crate) fn remove_submodule_dialog(&mut self, path: &str) {
        let Some(sm) = self.submodules.iter().find(|s| s.path == path) else {
            return;
        };
        let work = gitti_core::submodule::local_work(self.repo.workdir(), &self.submodules, path);
        let nested: Vec<&str> = self
            .submodules
            .iter()
            .filter(|s| s.path.starts_with(&format!("{path}/")))
            .map(|s| s.path.as_str())
            .collect();
        let mut body = format!(
            "Removes submodule {path}: its folder, its entry in .gitmodules and its pointer \
             (staged — commit to record the removal), and its repository in .git/modules/{}.",
            sm.name
        );
        if !nested.is_empty() {
            body.push_str(&format!("\nNested submodules go with it: {}.", nested.join(", ")));
        }
        if sm.depth > 0 {
            body.push_str("\nIt is removed from its parent submodule, which then has changes to commit.");
        }
        if !work.is_empty() {
            body.push_str("\n\nThis local work is lost:");
            for w in &work {
                body.push_str(&format!("\n• {}", w.describe()));
            }
        }
        let button = if work.is_empty() { "Remove" } else { "Remove Anyway" };
        let p = path.to_string();
        self.modal = Some(Modal::confirm("Remove Submodule", body, button, move |app: &mut App| {
            app.run_op("Remove submodule", true, After::Nothing, move |r| r.remove_submodule(&p))
        }));
    }

    /// Remove Submodule… from the palette: the one there is, or a picker.
    fn remove_submodule_picker(&mut self) {
        if let [only] = self.submodules.as_slice() {
            let path = only.path.clone();
            self.remove_submodule_dialog(&path);
            return;
        }
        let items = self
            .submodules
            .iter()
            .map(|sm| {
                let path = sm.path.clone();
                PickItem::new(sm.path.clone(), sm.state.label(), move |app: &mut App| {
                    app.remove_submodule_dialog(&path)
                })
            })
            .collect();
        self.modal = Some(Modal::picker("Remove which submodule?", items));
    }

    /// Run Command in Each Submodule…: the git command dialog with `submodule foreach
    /// --recursive ` typed in (its output shows like any command's).
    fn foreach_dialog(&mut self) {
        self.git_command_dialog();
        if let Some(Modal::Input { fields, .. }) = &mut self.modal {
            fields[0].edit = TextEdit::single("submodule foreach --recursive ");
        }
    }

    /// The palette's submodule commands without submodules: say so instead of running git.
    fn without_submodules(&mut self) -> bool {
        if self.submodules.is_empty() {
            self.set_info("This repository has no submodules (palette: Add Submodule…)");
            return true;
        }
        false
    }

    /// `git submodule update --init --recursive [-- paths]` (empty = all).
    pub fn update_submodules(&mut self, paths: Vec<String>) {
        self.run_op("Update submodules", true, After::Nothing, move |r| {
            let p: Vec<&str> = paths.iter().map(String::as_str).collect();
            r.update_submodules(&p)
        });
    }

    /// "Add Submodule…": URL, path (empty = git's default from the URL), branch to track
    /// (optional) → `git submodule add`; staged, a commit records it.
    pub fn add_submodule_dialog(&mut self) {
        self.modal = Some(Modal::input(
            "Add Submodule (staged — commit to record it)".to_string(),
            vec![
                ("Repository URL", ""),
                ("Path (empty = from the URL)", ""),
                ("Branch to track (optional)", ""),
            ],
            move |app, v| {
                let url = v[0].trim().to_string();
                let path = v[1].trim().trim_matches('/').to_string();
                let branch = v[2].trim().to_string();
                if url.is_empty() {
                    app.set_error("Add submodule: the repository URL is empty".into());
                    return;
                }
                app.run_op("Add submodule", true, After::Nothing, move |r| {
                    r.add_submodule(&url, &path, &branch)
                });
            },
        ));
    }

    /// `git submodule sync --recursive [-- paths]`.
    pub fn sync_submodules(&mut self, paths: Vec<String>) {
        self.run_op("Sync submodule URLs", true, After::Nothing, move |r| {
            let p: Vec<&str> = paths.iter().map(String::as_str).collect();
            r.sync_submodules(&p)
        });
    }

    /// gitty shows one repository at a time: asks, then restarts in the submodule.
    pub(crate) fn open_submodule(&mut self, path: &str) {
        let Some(sm) = self.submodules.iter().find(|s| s.path == path) else {
            return;
        };
        if sm.state == SubmoduleState::Uninitialized {
            self.set_info(format!("{path} is not initialised — Update checks it out"));
            return;
        }
        let dir = sm.dir(self.repo.workdir());
        let body = format!(
            "{}\n\ngitty shows one repository at a time: it restarts in the submodule.",
            dir.display()
        );
        self.modal = Some(Modal::confirm(
            "Open the submodule?",
            body,
            "Open",
            move |app: &mut App| app.reopen = Some(dir),
        ));
    }

    /// Palette: gitti's Repository-menu submodule commands (always — without submodules
    /// they say so), open each initialised submodule.
    /// The SUBMODULES header's menu: the Repository commands on all of them.
    pub(super) fn submodule_section_items(&self) -> Vec<MenuItem> {
        vec![
            MenuItem::new("Add Submodule…", |app| app.add_submodule_dialog()).key('a'),
            MenuItem::separator(),
            MenuItem::new("Update Submodules", |app| app.update_submodules(Vec::new())).key('u'),
            MenuItem::new("Update Submodules to Remote Branches", |app| {
                app.update_submodules_remote(Vec::new())
            })
            .key('r'),
            MenuItem::new("Sync Submodule URLs", |app| app.sync_submodules(Vec::new())).key('s'),
            MenuItem::new("Run Command in Each Submodule…", |app| app.foreach_dialog()).key('e'),
        ]
    }

    pub(super) fn submodule_palette_items(&self) -> Vec<PickItem> {
        fn guarded(label: &str, run: fn(&mut App)) -> PickItem {
            PickItem::new(label, "", move |app: &mut App| {
                if !app.without_submodules() {
                    run(app)
                }
            })
        }
        let mut items = vec![
            PickItem::new("Add Submodule…", "", |app: &mut App| {
                app.add_submodule_dialog()
            }),
            guarded("Update Submodules", |app| app.update_submodules(Vec::new())),
            guarded("Update Submodules to Remote Branches", |app| {
                app.update_submodules_remote(Vec::new())
            }),
            guarded("Check Out Commit in Submodule…", |app| {
                app.checkout_commit_submodule_picker()
            }),
            guarded("Sync Submodule URLs", |app| app.sync_submodules(Vec::new())),
            guarded("Remove Submodule…", |app| app.remove_submodule_picker()),
            guarded("Run Command in Each Submodule…", |app| app.foreach_dialog()),
        ];
        for sm in self
            .submodules
            .iter()
            .filter(|s| s.state != SubmoduleState::Uninitialized)
        {
            let path = sm.path.clone();
            items.push(PickItem::new(
                format!("Open Submodule {}", sm.path),
                "",
                move |app: &mut App| app.open_submodule(&path),
            ));
        }
        items
    }

    /// "Check Out Commit in Submodule…" from the palette: the one checked-out submodule, or
    /// a picker of them.
    fn checkout_commit_submodule_picker(&mut self) {
        let ready: Vec<(String, String)> = self
            .submodules
            .iter()
            .filter(|s| s.state != SubmoduleState::Uninitialized)
            .map(|s| (s.path.clone(), s.row_detail()))
            .collect();
        match ready.as_slice() {
            [] => self.set_info("No submodule is checked out — Update Submodules checks them out"),
            [(one, _)] => {
                let path = one.clone();
                self.checkout_commit_picker(&path);
            }
            _ => {
                let items = ready
                    .into_iter()
                    .map(|(path, detail)| {
                        PickItem::new(path.clone(), detail, move |app: &mut App| {
                            app.checkout_commit_picker(&path)
                        })
                    })
                    .collect();
                self.modal = Some(Modal::picker("Check out a commit in which submodule?", items));
            }
        }
    }

    fn picker_title(path: &str) -> String {
        format!("Check out which commit in {path}?")
    }

    /// The picker over the history of the submodule at `path`, filled when the background
    /// read (`Msg::SubmoduleHistory`) is done; type to filter by id / subject.
    pub(crate) fn checkout_commit_picker(&mut self, path: &str) {
        let Some(sm) = self.submodules.iter().find(|s| s.path == path).cloned() else {
            return;
        };
        if sm.state == SubmoduleState::Uninitialized {
            self.set_info(format!("{path} is not initialised — Update checks it out"));
            return;
        }
        let mut m = Modal::picker(Self::picker_title(path), Vec::new());
        if let Modal::Picker { note, .. } = &mut m {
            *note = Some(format!("Reading the history of {path}…"));
        }
        self.modal = Some(m);
        let top = self.repo.workdir().to_path_buf();
        let p = path.to_string();
        self.loads_in_flight += 1;
        super::spawn(
            &self.tx,
            move || gitti_core::submodule::history(&top, &sm, HISTORY_LIMIT),
            move |r| Msg::SubmoduleHistory(p, r),
        );
    }

    /// The submodule's commits into its picker (if it is still open).
    pub(super) fn on_submodule_history(
        &mut self,
        path: &str,
        result: anyhow::Result<Vec<SubmoduleCommit>>,
    ) {
        let title_want = Self::picker_title(path);
        let Some(m) = self.modal.as_mut() else {
            return;
        };
        let Modal::Picker { title, items, note, .. } = m else {
            return;
        };
        if *title != title_want {
            return;
        }
        match result {
            Err(e) => *note = Some(format!("{e:#}")),
            Ok(commits) => {
                let n = commits.len();
                *note = Some(format!(
                    "{}{n} commit{} of {path} — stage + commit the pointer change to record the one you pick",
                    if n >= HISTORY_LIMIT { "the newest " } else { "" },
                    if n == 1 { "" } else { "s" },
                ));
                *items = commits
                    .into_iter()
                    .map(|c| {
                        let marks = c.marks();
                        let mut hint = crate::ui::format_time(c.time);
                        if !marks.is_empty() {
                            hint.push_str(&format!(" · {marks}"));
                        }
                        let (p, id) = (path.to_string(), c.id);
                        let mut item = PickItem::new(
                            format!("{} {}", short_id(c.id), c.subject),
                            hint,
                            move |app: &mut App| app.checkout_submodule_commit(&p, id),
                        );
                        item.strong = c.checked_out;
                        item
                    })
                    .collect();
            }
        }
        m.refilter();
    }

    /// Checks out commit `id` in the submodule at `path` (detached), after a confirmation
    /// naming what is at risk there, if anything (see gitti's `checkout_submodule_commit`).
    pub(crate) fn checkout_submodule_commit(&mut self, path: &str, id: ObjectId) {
        let Some(sm) = self.submodules.iter().find(|s| s.path == path) else {
            return;
        };
        if sm.checked_out == Some(id) {
            self.set_info(format!("{path} is already at {}", short_id(id)));
            return;
        }
        let risks = gitti_core::submodule::checkout_risks(self.repo.workdir(), &self.submodules, path, id);
        let p = path.to_string();
        let run = move |app: &mut App| {
            app.run_op("Check out a commit in the submodule", true, After::Nothing, move |r| {
                r.checkout_submodule_commit(&p, id)
            })
        };
        if risks.is_empty() {
            run(self);
            return;
        }
        let mut body = format!("Checks out {} in {path} with a detached HEAD.\n\nIn the submodule:", short_id(id));
        for r in &risks {
            body.push_str(&format!("\n• {r}"));
        }
        self.modal = Some(Modal::confirm("Check Out Commit in Submodule", body, "Check Out Anyway", run));
    }

    /// After a stage / unstage / discard: the pointers' states again (in-process, nothing
    /// without `.gitmodules`).
    pub(super) fn refresh_submodule_states(&mut self) {
        if !self.submodules.is_empty() || self.repo.has_gitmodules() {
            // The nested ones keep their dirty flags (no git call here).
            self.submodules = self.repo.submodules_after(&self.working.status, &self.submodules);
            self.rebuild_sidebar();
        }
    }
}
