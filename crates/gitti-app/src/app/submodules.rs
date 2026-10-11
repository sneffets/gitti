//! Submodules: the SUBMODULES sidebar rows and their menu (Update, Update to Remote
//! Branch, Check Out Commit…, Open, Reveal, Open in Terminal, Sync URL, Copy Path,
//! Remove…) — also in the diff's menu on a submodule's pointer change —, the "Submodules
//! out of date" banner, the Repository menu / palette commands, and the rows of a pointer
//! change in diffs (`DiffRow::Submodule`). The data comes with the refresh snapshot
//! (`gitti_core::submodule`, no git call of its own).
//!
//! "Check Out Commit in Submodule…": a picker over the submodule's history (read in
//! the background), the chosen commit checked out detached in the submodule — the pointer
//! change then shows in the changes; staging and committing it records that commit in the
//! superproject's history (git stores the submodule's commit id per superproject commit).

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gitti_core::ObjectId;
use gitti_core::submodule::{
    HISTORY_LIMIT, SubRow, Submodule, SubmoduleCommit, SubmoduleState, out_of_date, short_id,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalSpec};
use gitti_core::submodule::add::{ALLOW_UPDATE, FilePermission, FileRefused};
use super::picker::PickerItem;
use super::repo_actions::AppEvent;
use crate::probe::ProbeExt as _;
use crate::rows::Src;
use crate::theme::{Palette, PaletteExt as _, mono_font};
use crate::{
    AddSubmodule, CheckoutSubmoduleCommit, ForeachSubmodule, RemoveSubmodule, SyncSubmodules,
    UpdateSubmodules, UpdateSubmodulesRemote,
};

type AppHandle = WeakEntity<GittiApp>;

/// The Repository menu's commands on all submodules.
#[derive(Clone, Copy)]
enum SubCommand {
    Update,
    UpdateRemote,
    CheckoutCommit,
    Sync,
    Remove,
    Foreach,
}

/// The out-of-date submodules a dismissed banner was about (it comes back when they change).
pub(super) type BannerKey = Vec<(String, Option<ObjectId>)>;

/// What [`GittiApp::submodules_refused`] runs again with the `file` transport allowed.
#[derive(Debug, Clone)]
pub enum RetryUpdate {
    /// Update Submodules of `paths` (empty = all), `remote`: to their remote branches.
    Update { paths: Vec<String>, remote: bool },
    /// The submodules of this repository, just cloned with `--recurse-submodules`
    /// (`note`: what else the clone has to say).
    Clone { note: Option<String> },
}

fn item(
    label: impl Into<SharedString>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> PopupMenuItem {
    let app = app.clone();
    super::menus::menu_entry(label).on_click(move |_, window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

/// The colour of a state mark.
pub(super) fn state_color(state: SubmoduleState, p: &Palette) -> Hsla {
    match state {
        SubmoduleState::UpToDate => p.success,
        SubmoduleState::OutOfDate => p.warning,
        SubmoduleState::Modified => p.accent,
        SubmoduleState::Conflicted => p.danger,
        SubmoduleState::Uninitialized => p.text_faint,
    }
}

/// "Update to Remote Branch (dev)": the branch `.gitmodules` names, else the remote's HEAD.
fn remote_update_label(sm: &Submodule) -> String {
    match &sm.branch {
        Some(b) => format!("Update to Remote Branch ({b})"),
        None => "Update to Remote Branch".to_string(),
    }
}

/// "Check Out Commit in Submodule…" in a submodule's menus.
fn checkout_commit_item(app: &AppHandle, sm: &Submodule) -> PopupMenuItem {
    let p = sm.path.clone();
    item("Check Out Commit in Submodule…", app, move |this, w, cx| {
        this.checkout_commit_picker(&p, w, cx)
    })
    .disabled(sm.state == SubmoduleState::Uninitialized)
}

/// The actions on one submodule (its sidebar entry, its pointer change in a diff).
fn submodule_menu(menu: PopupMenu, app: &AppHandle, sm: &Submodule, dir: PathBuf) -> PopupMenu {
    let fm = gitti_core::desktop::file_manager_name();
    let initialised = sm.state != SubmoduleState::Uninitialized;
    let path = sm.path.clone();
    let update_label = if initialised { "Update" } else { "Initialise and Update" };
    let menu = menu.item({
        let p = path.clone();
        item(update_label, app, move |this, w, cx| {
            this.update_submodules(vec![p.clone()], w, cx)
        })
    })
    .item({
        let p = path.clone();
        item(remote_update_label(sm), app, move |this, w, cx| {
            this.update_submodules_remote(vec![p.clone()], w, cx)
        })
    })
    .item(checkout_commit_item(app, sm))
    .item({
        let p = path.clone();
        item("Open in New Tab", app, move |this, _, cx| this.open_submodule(&p, cx))
            .disabled(!initialised)
    })
    .item({
        let d = dir.clone();
        item(format!("Reveal in {fm}"), app, move |this, _, cx| {
            if let Err(e) = gitti_core::desktop::open_folder(&d) {
                this.set_error(format!("{e:#}"));
            }
            cx.notify();
        })
        .disabled(!dir.is_dir())
    })
    .item({
        let d = dir.clone();
        item("Open in Terminal", app, move |this, _, cx| this.open_terminal_at(d.clone(), cx))
            .disabled(!dir.is_dir())
    })
    .separator()
    .item({
        let p = path.clone();
        item("Sync URL", app, move |this, w, cx| {
            this.sync_submodules(vec![p.clone()], w, cx)
        })
    })
    .item({
        super::menus::copy_item("Copy Path", dir.display().to_string())
    });
    // Its checked-out commit (uninitialised: the recorded one, labelled so).
    let menu = sm.copy_sha_entries().into_iter().fold(menu, |menu, (label, id)| {
        menu.item(match id {
            Ok(id) => super::menus::copy_item(label, id),
            Err(why) => super::menus::menu_entry(format!("{label} ({why})")).disabled(true),
        })
    });
    menu.separator().item({
        let p = path.clone();
        item("Remove Submodule…", app, move |this, w, cx| {
            this.remove_submodule_dialog(&p, w, cx)
        })
    })
}

/// The SUBMODULES section header's menu: the Repository menu's commands on all of them.
pub(super) fn section_menu(menu: PopupMenu, app: &AppHandle) -> PopupMenu {
    let cmd = |label: &str, c: SubCommand| {
        item(label.to_string(), app, move |this, w, cx| this.submodule_command(c, w, cx))
    };
    menu.item(item("Add Submodule…", app, |this, w, cx| {
        this.add_submodule_dialog(w, cx)
    }))
    .separator()
    .item(cmd("Update Submodules", SubCommand::Update))
    .item(cmd("Update Submodules to Remote Branches", SubCommand::UpdateRemote))
    .item(cmd("Sync Submodule URLs", SubCommand::Sync))
    .item(cmd("Run Command in Each Submodule…", SubCommand::Foreach))
}

/// The diff's context menu on a submodule's pointer change (its file row or summary /
/// commit rows): the submodule's own actions after the diff's.
pub(super) fn diff_menu_items(menu: PopupMenu, app: &AppHandle, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let this = this.read(cx);
    let Some(sm) = this.menu_row.and_then(|ix| this.submodule_of_row(ix)) else {
        return menu;
    };
    let dir = sm.dir(this.workdir_path());
    let menu = menu.separator().label(format!("Submodule {}", sm.path));
    submodule_menu(menu, app, sm, dir)
}

impl GittiApp {
    /// A submodule: state mark, name (nested ones indented, relative to their parent),
    /// a faint state detail; everything in the tooltip. Double click opens it in a tab.
    pub(super) fn submodule_row(
        &self,
        si: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sm = self.submodules[si].clone();
        let app = cx.entity().downgrade();
        let parent = self.submodules[..si]
            .iter()
            .rev()
            .find(|s| s.depth + 1 == sm.depth && sm.path.starts_with(&format!("{}/", s.path)));
        let label = match parent {
            Some(par) => sm.path[par.path.len() + 1..].to_string(),
            None => sm.path.clone(),
        };
        // The state and, when they differ, the recorded vs. checked-out commit.
        let detail = sm.row_detail();
        let tip = sm.describe();
        let color = state_color(sm.state, p);
        let small = self.settings.ui_small();
        let dir = sm.dir(self.workdir_path());
        let path = sm.path.clone();
        self.sidebar_row(format!("sub-{}", sm.path), 1 + sm.depth, p)
            .cursor_pointer()
            .when(sm.state == SubmoduleState::Uninitialized, |d| d.text_color(p.text_muted))
            .child(
                div()
                    .probe(format!("sub-mark:{}", sm.path))
                    .w(px(self.settings.ui_font_size + 4.))
                    .flex_none()
                    .text_color(color)
                    .child(sm.state.mark().to_string()),
            )
            // Name and detail shrink by their widths (neither eats the other); the spacer
            // pushes the detail to the right edge.
            .child(div().flex_shrink(1.).min_w_0().truncate().child(label))
            .child(div().flex_1())
            .when(!detail.is_empty(), |d| {
                d.child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .max_w(relative(0.6))
                        .truncate()
                        .text_size(small)
                        .text_color(if sm.state == SubmoduleState::OutOfDate {
                            p.warning
                        } else {
                            p.text_muted
                        })
                        .child(detail),
                )
            })
            .tooltip(move |window, cx| {
                Tooltip::new(format!("{tip}\nDouble click opens it in a tab")).build(window, cx)
            })
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() >= 2 {
                    this.open_submodule(&path, cx);
                }
            }))
            .context_menu(move |menu, _, _| submodule_menu(menu, &app, &sm, dir.clone()))
    }

    /// The submodule whose pointer change diff row `ix` belongs to (`None`: another file, or
    /// a submodule this checkout doesn't have, e.g. in an old commit).
    fn submodule_of_row(&self, ix: usize) -> Option<&Submodule> {
        let (src, file) = self.rows.get(ix)?.file_ref()?;
        let diff = self.file_diff(src, file)?;
        diff.submodule.as_ref()?;
        let path = diff.path();
        self.submodules.iter().find(|s| s.path == path)
    }

    fn workdir_path(&self) -> &std::path::Path {
        self.repo
            .as_ref()
            .map(|r| r.workdir())
            .unwrap_or(std::path::Path::new(""))
    }

    /// Opens the submodule at `path` (relative to the work dir) in a new tab.
    pub(super) fn open_submodule(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(sm) = self.submodules.iter().find(|s| s.path == path) else {
            return;
        };
        if sm.state == SubmoduleState::Uninitialized {
            self.set_info(format!("{path} is not initialised — Update checks it out"));
            cx.notify();
            return;
        }
        cx.emit(AppEvent::OpenRepository(sm.dir(self.workdir_path())));
    }

    /// Menu-bar actions of this module (Repository ▸ Add / Update / Sync / Remove / Run
    /// Command in Each).
    pub(super) fn submodule_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &AddSubmodule, w, cx| {
            this.add_submodule_dialog(w, cx)
        }))
        .on_action(cx.listener(|this, _: &UpdateSubmodules, w, cx| {
            this.submodule_command(SubCommand::Update, w, cx)
        }))
        .on_action(cx.listener(|this, _: &UpdateSubmodulesRemote, w, cx| {
            this.submodule_command(SubCommand::UpdateRemote, w, cx)
        }))
        .on_action(cx.listener(|this, _: &CheckoutSubmoduleCommit, w, cx| {
            this.submodule_command(SubCommand::CheckoutCommit, w, cx)
        }))
        .on_action(cx.listener(|this, _: &SyncSubmodules, w, cx| {
            this.submodule_command(SubCommand::Sync, w, cx)
        }))
        .on_action(cx.listener(|this, _: &RemoveSubmodule, w, cx| {
            this.submodule_command(SubCommand::Remove, w, cx)
        }))
        .on_action(cx.listener(|this, _: &ForeachSubmodule, w, cx| {
            this.submodule_command(SubCommand::Foreach, w, cx)
        }))
    }

    /// A Repository-menu / palette command on all submodules (Remove: picks one).
    fn submodule_command(&mut self, c: SubCommand, window: &mut Window, cx: &mut Context<Self>) {
        if self.no_submodules(cx) {
            return;
        }
        match c {
            SubCommand::Update => self.update_submodules(Vec::new(), window, cx),
            SubCommand::UpdateRemote => self.update_submodules_remote(Vec::new(), window, cx),
            SubCommand::CheckoutCommit => self.checkout_commit_submodule_picker(window, cx),
            SubCommand::Sync => self.sync_submodules(Vec::new(), window, cx),
            SubCommand::Remove => self.remove_submodule_picker(window, cx),
            SubCommand::Foreach => {
                // The git command line of the Git Log panel, prefilled: the output shows
                // there like any typed command's.
                self.focus_command_input(window, cx);
                self.cmd_input.update(cx, |s, cx| {
                    s.set_value("submodule foreach --recursive ", window, cx)
                });
                self.set_info("Type the command to run in each submodule (git submodule foreach)");
                cx.notify();
            }
        }
    }

    /// Commands without submodules: say so instead of running git.
    fn no_submodules(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.submodules.is_empty() {
            return false;
        }
        self.set_info("This repository has no submodules (Repository ▸ Add Submodule…)");
        cx.notify();
        true
    }

    /// "Add Submodule…": URL, path (git's default from the URL until edited), branch to
    /// track (optional) → `git submodule add`; the result is staged, a commit records it.
    /// A repository URL on the clipboard (`clone_source::url_from_clipboard`) is put into
    /// the URL field, selected, the path derived as if it was pasted — as in Clone. A local
    /// path git refuses for submodules (`submodule::add`): said under the fields while
    /// typed, and OK asks before it allows the `file` transport for this one add.
    pub(super) fn add_submodule_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.as_ref() else {
            return;
        };
        let ctx = repo.submodule_add_context();
        let from_clipboard = super::repo_actions::clipboard_text(cx)
            .and_then(|t| gitti_core::clone_source::url_from_clipboard(&t));
        let live_ctx = ctx.clone();
        let mut spec = ModalSpec::new("Add Submodule")
            .message(
                "Clones the repository into a folder of this one and stages it with \
                 .gitmodules — commit to record it.",
            )
            .field("Repository URL", from_clipboard.clone().unwrap_or_default())
            .field("Path in this repository", "")
            .derive(0, 1, gitti_core::submodule::default_path)
            .field("Branch to track (optional)", "")
            .ok("Add Submodule")
            .live(move |r| add_submodule_live(&live_ctx, r.values[0].trim()));
        if from_clipboard.is_some() {
            spec = spec.prefilled(0, gitti_core::clone_source::CLIPBOARD_NOTE);
        }
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let url = res.values[0].trim().to_string();
            let path = res.values[1].trim().trim_matches('/').to_string();
            let branch = res.values[2].trim().to_string();
            if url.is_empty() {
                this.set_error("Add submodule: the repository URL is empty".into());
                cx.notify();
                return;
            }
            if ctx.needs_permission(&url) != FilePermission::Ask {
                this.add_submodule(url, path, branch, false, w, cx);
                return;
            }
            let spec = ModalSpec::new("Allow a Local Path for This Submodule?")
                .message(gitti_core::submodule::add::permission_text(&ctx, &url))
                .ok("Allow for This Add");
            this.open_modal(spec, w, cx, move |this, _, w, cx| {
                this.add_submodule(url, path, branch, true, w, cx)
            });
        });
    }

    /// Runs `git submodule add` (see `Repo::add_submodule`; `allow_file`: the user agreed
    /// to the `file` transport for this add).
    fn add_submodule(
        &mut self,
        url: String,
        path: String,
        branch: String,
        allow_file: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = if path.is_empty() {
            gitti_core::submodule::default_path(&url)
        } else {
            path.clone()
        };
        self.run_op(
            "Adding submodule",
            true,
            window,
            cx,
            move |r| r.add_submodule(&url, &path, &branch, allow_file),
            move |this, _, _, _| {
                this.set_info(format!("Added submodule {shown} (staged — commit to record it)"))
            },
        );
    }

    /// `git submodule update --init --recursive [-- paths]` (empty = all), then a refresh.
    pub(super) fn update_submodules(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_update(paths, false, false, window, cx);
    }

    /// `git submodule update --init --recursive --remote [-- paths]` (empty = all): the
    /// tips of the branches they follow; the new pointers show as changes to commit.
    pub(super) fn update_submodules_remote(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_update(paths, true, false, window, cx);
    }

    /// Update Submodules (`remote`: to their remote branches; `allow_file`: the user
    /// agreed to clone from local paths this once). git's refusal of a local path
    /// (`submodule::add::FileRefused`) ends in [`Self::submodules_refused`], not an error.
    fn run_update(
        &mut self,
        paths: Vec<String>,
        remote: bool,
        allow_file: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let what = match paths.as_slice() {
            [one] => one.clone(),
            _ => "submodules".to_string(),
        };
        let label = if remote {
            "Updating submodules to their remote branches"
        } else {
            "Updating submodules"
        };
        let retry = paths.clone();
        self.run_op(
            label,
            true,
            window,
            cx,
            move |r| {
                let p: Vec<&str> = paths.iter().map(String::as_str).collect();
                match r.update_submodules_with(&p, remote, allow_file) {
                    Ok(out) => Ok(Ok(out)),
                    Err(e) => e.downcast::<FileRefused>().map(Err),
                }
            },
            move |this, result, window, cx| match result {
                Ok(_) if remote => this.set_info(format!(
                    "Updated {what} to the remote branch — commit the new pointers to record them"
                )),
                Ok(_) => this.set_info(format!("Updated {what}")),
                Err(refused) => {
                    let retry = RetryUpdate::Update { paths: retry, remote };
                    this.submodules_refused(refused, retry, window, cx)
                }
            },
        );
    }

    /// git refused to clone submodules from a local path (Update Submodules, or the
    /// clone of this repository with `--recurse-submodules`): says which and why, and
    /// offers the update again with the `file` transport allowed for that one command —
    /// nothing written to a config. Refused by `GIT_ALLOW_PROTOCOL`: only says what to
    /// change.
    pub fn submodules_refused(
        &mut self,
        refused: FileRefused,
        retry: RetryUpdate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cloned = matches!(retry, RetryUpdate::Clone { .. });
        let (paths, remote, note) = match retry {
            RetryUpdate::Update { paths, remote } => (paths, remote, None),
            RetryUpdate::Clone { note } => (Vec::new(), false, note),
        };
        let mut text = refused.explain(cloned);
        if let Some(n) = note {
            text = format!("{n}\n\n{text}");
        }
        if refused.blocked {
            self.set_error("Submodules not cloned: GIT_ALLOW_PROTOCOL doesn't allow local paths".into());
            self.show_notice("Submodules Not Cloned", text, window, cx);
            return;
        }
        self.set_info("Submodules not cloned yet: git refuses local paths unless allowed");
        let spec = ModalSpec::new("Clone Submodules From Local Paths?")
            .message(text)
            .ok(ALLOW_UPDATE)
            .cancel(Some("Not Now"));
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            this.run_update(paths, remote, true, w, cx)
        });
    }

    /// Remove Submodule… from the menu bar / palette: the one there is, or a picker.
    fn remove_submodule_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let [only] = self.submodules.as_slice() {
            let path = only.path.clone();
            self.remove_submodule_dialog(&path, window, cx);
            return;
        }
        let items = self
            .submodules
            .iter()
            .map(|sm| {
                let path = sm.path.clone();
                super::picker::item(sm.path.clone(), sm.state.label(), move |t, w, cx| {
                    t.remove_submodule_dialog(&path, w, cx)
                })
            })
            .collect();
        self.open_picker("Remove which submodule?", items, window, cx);
    }

    /// Confirms the removal of the submodule at `path`, naming what goes (its folder, its
    /// `.gitmodules` entry and pointer — staged —, its repository in `.git/modules`, nested
    /// ones) and, prominently, the local work that would be lost (uncommitted changes,
    /// commits on no remote, stashes; read in-process). With such work the button says
    /// "Remove Anyway".
    pub(super) fn remove_submodule_dialog(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sm) = self.submodules.iter().find(|s| s.path == path) else {
            return;
        };
        let top = self.workdir_path().to_path_buf();
        let work = gitti_core::submodule::local_work(&top, &self.submodules, path);
        let nested: Vec<&str> = self
            .submodules
            .iter()
            .filter(|s| s.path.starts_with(&format!("{path}/")))
            .map(|s| s.path.as_str())
            .collect();
        let mut msg = format!(
            "Removes submodule {path}: its folder, its entry in .gitmodules and its pointer \
             (staged — commit to record the removal), and its repository in .git/modules/{}.",
            sm.name
        );
        if !nested.is_empty() {
            msg.push_str(&format!("\nNested submodules go with it: {}.", nested.join(", ")));
        }
        if sm.depth > 0 {
            msg.push_str("\nIt is removed from its parent submodule, which then has changes to commit.");
        }
        if !work.is_empty() {
            msg.push_str("\n\nThis local work is lost:");
            for w in &work {
                msg.push_str(&format!("\n• {}", w.describe()));
            }
        }
        let ok = if work.is_empty() { "Remove Submodule" } else { "Remove Anyway" };
        let spec = ModalSpec::new("Remove Submodule").message(msg).ok(ok).danger();
        let path = path.to_string();
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            let p = path.clone();
            this.run_op(
                "Removing submodule",
                true,
                w,
                cx,
                move |r| r.remove_submodule(&p),
                move |this, _, _, _| {
                    this.set_info(format!("Removed submodule {path} (staged — commit to record it)"))
                },
            );
        });
    }

    /// `git submodule sync --recursive [-- paths]`.
    pub(super) fn sync_submodules(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.git_op("Syncing submodule URLs", window, cx, move |r| {
            let p: Vec<&str> = paths.iter().map(String::as_str).collect();
            r.sync_submodules(&p)
        });
    }

    /// "Check Out Commit in Submodule…" from the menu bar / palette: the one checked-out
    /// submodule, or a picker of them.
    fn checkout_commit_submodule_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ready: Vec<&Submodule> = self
            .submodules
            .iter()
            .filter(|s| s.state != SubmoduleState::Uninitialized)
            .collect();
        match ready.as_slice() {
            [] => {
                self.set_info("No submodule is checked out — Update Submodules checks them out");
                cx.notify();
            }
            [one] => {
                let path = one.path.clone();
                self.checkout_commit_picker(&path, window, cx);
            }
            _ => {
                let items = ready
                    .iter()
                    .map(|sm| {
                        let path = sm.path.clone();
                        super::picker::item(sm.path.clone(), sm.row_detail(), move |t, w, cx| {
                            t.checkout_commit_picker(&path, w, cx)
                        })
                    })
                    .collect();
                self.open_picker("Check out a commit in which submodule?", items, window, cx);
            }
        }
    }

    /// The picker over the history of the submodule at `path` (newest first, read in the
    /// background; type to filter by subject, id, branch); choosing a commit checks it out.
    pub(super) fn checkout_commit_picker(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sm) = self.submodules.iter().find(|s| s.path == path).cloned() else {
            return;
        };
        if sm.state == SubmoduleState::Uninitialized {
            self.set_info(format!("{path} is not initialised — Update checks it out"));
            cx.notify();
            return;
        }
        let top = self.workdir_path().to_path_buf();
        self.open_picker(
            &format!("Check out which commit in {path}? (subject, id, branch…)"),
            Vec::new(),
            window,
            cx,
        );
        self.set_picker_note(Some(format!("Reading the history of {path}…")), cx);
        let job = cx
            .background_executor()
            .spawn(async move { gitti_core::submodule::history(&top, &sm, HISTORY_LIMIT) });
        let path = path.to_string();
        // Dropped with the picker (closed, or another one opened): nothing to fill then.
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| this.show_submodule_history(&path, result, cx))
                .ok();
        });
        self.picker_search_started(Arc::new(AtomicBool::new(false)), task);
    }

    /// The submodule's commits into the open picker.
    fn show_submodule_history(
        &mut self,
        path: &str,
        result: anyhow::Result<Vec<SubmoduleCommit>>,
        cx: &mut Context<Self>,
    ) {
        let commits = match result {
            Ok(c) => c,
            Err(e) => {
                self.fill_picker(Vec::new(), Some(format!("{e:#}")), cx);
                return;
            }
        };
        let n = commits.len();
        let note = format!(
            "{}{n} commit{} of {path} — the one you check out is recorded once you stage and \
             commit the pointer change",
            if n >= HISTORY_LIMIT { "the newest " } else { "" },
            if n == 1 { "" } else { "s" },
        );
        let items = commits
            .into_iter()
            .map(|c| {
                let marks = c.marks();
                let mut detail = format!("{} · {}", short_id(c.id), super::format_time(c.time));
                if !marks.is_empty() {
                    detail.push_str(&format!(" · {marks}"));
                }
                let label = if c.subject.is_empty() { short_id(c.id) } else { c.subject };
                let (p, id) = (path.to_string(), c.id);
                PickerItem {
                    label,
                    detail,
                    keys: String::new(),
                    color: None,
                    action: None,
                    run: Rc::new(move |t, w, cx| t.checkout_submodule_commit(&p, id, w, cx)),
                }
            })
            .collect();
        self.fill_picker(items, Some(note), cx);
    }

    /// Checks out commit `id` in the submodule at `path` (detached) — after a confirmation
    /// naming what is at risk there (uncommitted changes, commits on no branch), if any.
    pub(super) fn checkout_submodule_commit(
        &mut self,
        path: &str,
        id: ObjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sm) = self.submodules.iter().find(|s| s.path == path) else {
            return;
        };
        if sm.checked_out == Some(id) {
            self.set_info(format!("{path} is already at {}", short_id(id)));
            cx.notify();
            return;
        }
        let top = self.workdir_path().to_path_buf();
        let risks = gitti_core::submodule::checkout_risks(&top, &self.submodules, path, id);
        if risks.is_empty() {
            self.run_submodule_checkout(path.to_string(), id, window, cx);
            return;
        }
        let mut msg = format!(
            "Checks out {} in {path} with a detached HEAD.\n\nIn the submodule:",
            short_id(id)
        );
        for r in &risks {
            msg.push_str(&format!("\n• {r}"));
        }
        let spec = ModalSpec::new("Check Out Commit in Submodule")
            .message(msg)
            .ok("Check Out Anyway")
            .danger();
        let path = path.to_string();
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            this.run_submodule_checkout(path.clone(), id, w, cx)
        });
    }

    fn run_submodule_checkout(
        &mut self,
        path: String,
        id: ObjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Where the pointer change shows: the parent submodule of a nested one.
        let owner = self
            .submodules
            .iter()
            .filter(|s| path.starts_with(&format!("{}/", s.path)))
            .max_by_key(|s| s.path.len())
            .map(|s| format!(" in {}", s.path))
            .unwrap_or_default();
        // A deliberately older commit counts as "out of date": no banner offering to undo
        // it (it comes back when another pointer moves).
        let banner: BannerKey = self
            .submodules
            .iter()
            .filter(|s| s.state == SubmoduleState::OutOfDate || s.path == path)
            .map(|s| (s.path.clone(), s.recorded))
            .collect();
        let p = path.clone();
        self.run_op(
            "Checking out a commit in the submodule",
            true,
            window,
            cx,
            move |r| r.checkout_submodule_commit(&p, id),
            move |this, _, _, _| {
                this.submodule_banner_dismissed = Some(banner);
                this.set_info(format!(
                    "{path} is at {} now — stage the pointer change{owner} and commit to record it",
                    short_id(id)
                ));
            },
        );
    }

    fn banner_key(&self) -> BannerKey {
        out_of_date(&self.submodules)
            .iter()
            .map(|s| (s.path.clone(), s.recorded))
            .collect()
    }

    /// "Submodules out of date — Update" after a checkout / pull / merge moved a pointer
    /// (until updated, or dismissed for this set of pointers).
    pub(super) fn render_submodule_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stale = out_of_date(&self.submodules);
        if stale.is_empty() || self.submodule_banner_dismissed.as_ref() == Some(&self.banner_key()) {
            return None;
        }
        let p = cx.palette().clone();
        let names: Vec<String> = stale
            .iter()
            .take(3)
            .map(|s| format!("{} ({})", s.path, s.detail()))
            .collect();
        let more = stale.len().saturating_sub(3);
        let text = format!(
            "Submodule{} out of date — {}{}",
            if stale.len() == 1 { "" } else { "s" },
            names.join(", "),
            if more > 0 { format!(" and {more} more") } else { String::new() }
        );
        let paths: Vec<String> = stale.iter().map(|s| s.path.clone()).collect();
        Some(
            h_flex()
                .probe("submodule-banner")
                .px_3()
                .py_1()
                .gap_2()
                .items_center()
                .bg(p.warning.opacity(0.12))
                .border_b_1()
                .border_color(p.border)
                .text_color(p.warning)
                .child(div().flex_1().min_w_0().child(text))
                .child(
                    Button::new("submodule-update")
                        .probe("submodule-update")
                        .label("Update")
                        .small()
                        .tooltip("git submodule update --init --recursive")
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.update_submodules(paths.clone(), w, cx)
                        })),
                )
                .child(
                    Button::new("submodule-dismiss")
                        .label("✕")
                        .small()
                        .ghost()
                        .tooltip("Hide until another pointer moves")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.submodule_banner_dismissed = Some(this.banner_key());
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// Palette: the Repository menu's submodule commands (always — without submodules they
    /// say so), open each initialised submodule.
    pub(super) fn submodule_palette_items(&self) -> Vec<PickerItem> {
        fn entry(
            label: impl Into<String>,
            detail: impl Into<String>,
            run: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
        ) -> PickerItem {
            PickerItem {
                label: label.into(),
                detail: detail.into(),
                keys: String::new(),
                color: None,
                action: None,
                run: Rc::new(run),
            }
        }
        let command = |label: &str, detail: &str, c: SubCommand| {
            entry(label, detail, move |t, w, cx| t.submodule_command(c, w, cx))
        };
        let mut items = vec![
            entry("Add Submodule…", "git submodule add", |t, w, cx| {
                t.add_submodule_dialog(w, cx)
            }),
            command(
                "Update Submodules",
                "git submodule update --init --recursive",
                SubCommand::Update,
            ),
            command(
                "Update Submodules to Remote Branches",
                "git submodule update --remote",
                SubCommand::UpdateRemote,
            ),
            command(
                "Check Out Commit in Submodule…",
                "a commit of its history, detached — commit the pointer to record it",
                SubCommand::CheckoutCommit,
            ),
            command(
                "Sync Submodule URLs",
                "git submodule sync --recursive",
                SubCommand::Sync,
            ),
            command(
                "Remove Submodule…",
                "deinit, git rm, .git/modules",
                SubCommand::Remove,
            ),
            command(
                "Run Command in Each Submodule…",
                "git submodule foreach --recursive",
                SubCommand::Foreach,
            ),
        ];
        for sm in self
            .submodules
            .iter()
            .filter(|s| s.state != SubmoduleState::Uninitialized)
        {
            let path = sm.path.clone();
            items.push(entry(
                format!("Open Submodule {}", sm.path),
                sm.state.label(),
                move |t, _, cx| t.open_submodule(&path, cx),
            ));
        }
        items
    }

    /// Row `line` of the pointer change of submodule file `file`: the summary (with an
    /// Open button), the commits that came in (+) / went away (−), notes.
    pub(super) fn render_submodule_diff_row(
        &self,
        base: Stateful<Div>,
        (src, file, line): (Src, usize, usize),
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(diff) = self.file_diff(src, file) else {
            return base.into_any_element();
        };
        let path = diff.path().to_string();
        let Some(row) = diff.submodule.as_ref().and_then(|s| s.row(&path, line)) else {
            return base.into_any_element();
        };
        let gutter = |bg: Hsla, sign: Option<char>, color: Hsla| {
            h_flex()
                .h_full()
                .flex_none()
                .w(px(self.settings.diff_font_size * 3.4 * 2. + 3.))
                .bg(bg)
                .justify_end()
                .pr_2()
                .text_color(color)
                .children(sign.map(|c| c.to_string()))
        };
        let base = base
            .bg(p.bg_diff)
            .font_family(mono_font())
            .text_size(self.settings.diff());
        match row {
            SubRow::Summary(text) => {
                let open = self
                    .submodules
                    .iter()
                    .any(|s| s.path == path && s.state != SubmoduleState::Uninitialized);
                let open_path = path.clone();
                base.child(gutter(p.bg_gutter, None, p.text_faint))
                    .child(
                        div()
                            .pl_2()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text_strong)
                            .child(text),
                    )
                    .when(open, |d| {
                        d.child(
                            Button::new(("submodule-open", line))
                                .label("Open")
                                .xsmall()
                                .ghost()
                                .tooltip("Open the submodule in a new tab")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_submodule(&open_path, cx)
                                })),
                        )
                        .pr_2()
                    })
                    .into_any_element()
            }
            SubRow::Commit { added, id, subject } => {
                let (bg, gbg, fg) = if added {
                    (p.bg_added, p.bg_added_gutter, p.text_added)
                } else {
                    (p.bg_removed, p.bg_removed_gutter, p.text_removed)
                };
                base.bg(bg)
                    .child(gutter(gbg, Some(gitti_core::submodule::sign(added)), fg))
                    .child(
                        h_flex()
                            .pl_2()
                            .gap_2()
                            .flex_1()
                            .min_w_0()
                            .child(div().flex_none().text_color(p.text_muted).child(short_id(id)))
                            .child(div().min_w_0().truncate().text_color(p.text).child(subject.to_string())),
                    )
                    .into_any_element()
            }
            SubRow::More { added, n } => base
                .child(gutter(p.bg_gutter, Some(gitti_core::submodule::sign(added)), p.text_faint))
                .child(
                    div()
                        .pl_2()
                        .italic()
                        .text_color(p.text_muted)
                        .child(format!("… {n} more")),
                )
                .into_any_element(),
            SubRow::Note(text) => base
                .child(gutter(p.bg_gutter, None, p.text_faint))
                .child(
                    div()
                        .pl_2()
                        .min_w_0()
                        .truncate()
                        .italic()
                        .text_size(self.settings.ui_small())
                        .text_color(p.text_muted)
                        .child(text),
                )
                .into_any_element(),
        }
    }

    // ------------------------------------------------------------------ scripts / tests

    /// `submodule:ACTION,PATH` — update / remote (update to the remote branch) / checkout
    /// (the commit picker) / sync / open / remove (the confirm dialog) the submodule at PATH (`*` = all for update /
    /// remote / sync), `dismiss` the banner.
    pub fn script_submodule(
        &mut self,
        action: &str,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = if path == "*" || path.is_empty() {
            Vec::new()
        } else {
            vec![path.to_string()]
        };
        match action {
            "update" => self.update_submodules(paths, window, cx),
            "remote" => self.update_submodules_remote(paths, window, cx),
            "checkout" => self.checkout_commit_picker(path, window, cx),
            "remove" => self.remove_submodule_dialog(path, window, cx),
            "sync" => self.sync_submodules(paths, window, cx),
            "open" => self.open_submodule(path, cx),
            "dismiss" => {
                self.submodule_banner_dismissed = Some(self.banner_key());
                cx.notify();
            }
            other => eprintln!("[script] unknown submodule action {other}"),
        }
    }

    /// `"<path> <state>"` per submodule (tests).
    pub fn script_submodules(&self) -> Vec<String> {
        self.submodules
            .iter()
            .map(|s| format!("{} {}", s.path, s.state.label()))
            .collect()
    }
}

/// Add Submodule's live text (`submodule::add::live_note`): a local path git refuses for
/// submodules — OK asks first, or can't run at all (`GIT_ALLOW_PROTOCOL`).
fn add_submodule_live(ctx: &gitti_core::submodule::add::AddContext, url: &str) -> ModalLive {
    match gitti_core::submodule::add::live_note(ctx, url) {
        None => ModalLive::default(),
        Some((text, true)) => ModalLive { blocked: Some(text.into()), ..Default::default() },
        Some((text, false)) => ModalLive {
            text: text.into(),
            ok_label: Some("Add Submodule…".into()),
            ..Default::default()
        },
    }
}
